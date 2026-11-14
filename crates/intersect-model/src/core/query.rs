//! Handwritten, read-only model query plans shared by typed and dynamic clients.
//! Domain behavior is supplied by a language-owned `DerivedValues` adapter.
use crate::core::storage::{ModelRead, Query as Lookup};
use crate::definitions::{conversion::ConversionRegistry, resources::DeclarationId};
use intersect_model::definitions::schema::{Feature, Schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

mod typed;
pub use typed::*;
pub type QueryResult<T> = Result<T, String>;
pub type Parameters = BTreeMap<String, Value>;
type FeatureTable = BTreeMap<String, BTreeMap<String, Feature>>;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Expression {
    Field { path: String },
    Count { path: String },
    Literal { value: Value },
    Parameter { name: String },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Predicate {
    IsNull {
        expression: Expression,
    },
    Compare {
        left: Expression,
        comparison: Comparison,
        right: Expression,
    },
    And {
        predicates: Vec<Predicate>,
    },
    Or {
        predicates: Vec<Predicate>,
    },
    Not {
        predicate: Box<Predicate>,
    },
    Any {
        path: String,
        predicate: Box<Predicate>,
    },
    All {
        path: String,
        predicate: Box<Predicate>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub name: String,
    pub expression: Expression,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ordering {
    pub expression: Expression,
    #[serde(default)]
    pub descending: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryPlan {
    pub schema_uri: String,
    pub schema_revision: String,
    pub from: String,
    #[serde(default)]
    pub filter: Option<Predicate>,
    pub select: Vec<Column>,
    #[serde(default)]
    pub order_by: Vec<Ordering>,
    pub limit: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScalarType {
    String,
    Integer,
    Boolean,
    Reference(String),
    /// Exact custom datatype identity; admitted only with an explicit registry.
    Converted(DeclarationId),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueType {
    pub scalar: ScalarType,
    pub optional: bool,
    pub many: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct QueryStats {
    pub candidates: usize,
    pub reads: usize,
    pub access: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct QueryOutput {
    pub schema_revision: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub has_more: bool,
    pub stats: QueryStats,
}
#[derive(Clone, Copy, Debug)]
pub struct QueryLimits {
    pub max_steps: usize,
    pub max_results: usize,
}
impl Default for QueryLimits {
    fn default() -> Self {
        Self {
            max_steps: 100_000,
            max_results: 1_000,
        }
    }
}
/// A language adapter must read from the supplied store and honor this budget.
/// Unknown derivations are errors, never missing facts or false predicates.
pub trait DerivedValues {
    /// Exact schema binding required by computed feature state services.
    /// Legacy query providers can omit this until they implement that contract.
    fn schema_revision(&self) -> Option<&str> {
        None
    }
    /// Explicit assignment state for a derived-unsettable feature. Implementations
    /// must read language-owned backing state from this store, never infer history
    /// from equality with a default. Ordinary computed getters need not implement it.
    fn explicit_set_state(
        &self,
        _store: &dyn ModelRead,
        _id: &str,
        _feature: &str,
        _budget: &mut QueryBudget,
    ) -> QueryResult<bool> {
        Err("Derived unsettable state requires a setting-state provider".into())
    }
    fn supports(&self, class: &str, feature: &str) -> bool;
    fn read(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> QueryResult<Value>;
}
pub struct NoDerivedValues;
impl DerivedValues for NoDerivedValues {
    fn supports(&self, _: &str, _: &str) -> bool {
        false
    }
    fn read(
        &self,
        _: &dyn ModelRead,
        _: &str,
        feature: &str,
        _: &mut QueryBudget,
    ) -> QueryResult<Value> {
        Err(format!("No provider for derived feature {feature}"))
    }
}
pub struct QueryBudget {
    remaining: usize,
    pub reads: usize,
}
impl QueryBudget {
    pub fn new(steps: usize) -> Self {
        Self {
            remaining: steps,
            reads: 0,
        }
    }
    /// Read the remaining allowance without changing callback or charge behavior.
    pub fn remaining(&self) -> usize {
        self.remaining
    }
    /// Reject a native callback that replaced its ledger. Keep the observed
    /// prefix and invalidate further work without charging fabricated capacity.
    pub(crate) fn invalidate_replacement(&mut self, observed_reads: usize) {
        self.reads = self.reads.max(observed_reads);
        self.remaining = 0;
    }
    pub fn charge(&mut self) -> QueryResult<()> {
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or("Query work budget exhausted; no complete result")?;
        self.reads += 1;
        Ok(())
    }
    /// Charge counter-only work with the same partial-exhaustion behavior as
    /// repeated `charge` calls. Observers retain their own checkpoints and borrows.
    pub fn charge_many(&mut self, units: usize) -> QueryResult<()> {
        let consumed = units.min(self.remaining);
        let Some(reads) = self.reads.checked_add(consumed) else {
            // `reads` is public. Preserve the original build-profile overflow
            // behavior, including the exact partial ledger, for altered counters.
            for _ in 0..units {
                self.charge()?;
            }
            return Ok(());
        };
        self.remaining -= consumed;
        self.reads = reads;
        if consumed < units {
            return Err("Query work budget exhausted; no complete result".into());
        }
        Ok(())
    }
}

fn scalar(
    schema: &Schema,
    feature: &Feature,
    conversions: Option<&ConversionRegistry<'_>>,
) -> QueryResult<ScalarType> {
    if feature.reference {
        return Ok(ScalarType::Reference(feature.target.clone()));
    }
    if let Some(registry) = conversions
        && let Some(datatype) = schema.datatypes.get(&feature.target).filter(|datatype| {
            feature.datatype.as_ref() == Some(datatype.declaration())
                && datatype.application().is_none()
                && matches!(
                    datatype.binding(),
                    crate::definitions::datatypes::Binding::Unsupported { .. }
                )
        })
    {
        schema.require_attribute_conversion(feature, Some(registry))?;
        return Ok(ScalarType::Converted(datatype.declaration().clone()));
    }
    if schema.datatypes.contains_key(&feature.target)
        || schema.enumerations.contains_key(&feature.target)
    {
        return Err(format!("Unsupported query datatype {}", feature.target));
    }
    match feature.target.as_str() {
        "EString" => Ok(ScalarType::String),
        "EInt" | "ELong" | "ELongObject" => Ok(ScalarType::Integer),
        "EBoolean" => Ok(ScalarType::Boolean),
        other => Err(format!("Unsupported query datatype {other}")),
    }
}
fn feature(schema: &Schema, class: &str, name: &str) -> QueryResult<Feature> {
    schema
        .features(class)?
        .remove(name)
        .ok_or_else(|| format!("Unknown feature {class}.{name}"))
}
/// Resolve a path before reading any rows, including optionality across navigation.
pub fn path_type(schema: &Schema, class: &str, path: &str) -> QueryResult<ValueType> {
    path_type_context(schema, class, path, None)
}
/// Resolve custom converted columns with an explicitly selected language registry.
/// This does not admit nominal comparisons, ordering or implicit conversions.
pub fn path_type_with_conversions(
    schema: &Schema,
    class: &str,
    path: &str,
    conversions: &ConversionRegistry<'_>,
) -> QueryResult<ValueType> {
    path_type_context(schema, class, path, Some(conversions))
}
fn path_type_context(
    schema: &Schema,
    class: &str,
    path: &str,
    conversions: Option<&ConversionRegistry<'_>>,
) -> QueryResult<ValueType> {
    if !schema.classes.contains_key(class) {
        return Err(format!("Unknown query class {class}"));
    }
    if matches!(path, "$id" | "$class" | "$resource") {
        return Ok(ValueType {
            scalar: ScalarType::String,
            optional: false,
            many: false,
        });
    }
    let parts: Vec<_> = path.split('.').collect();
    if parts.is_empty() || parts.len() > 16 || parts.iter().any(|p| p.is_empty()) {
        return Err("Invalid query path (maximum 16 segments)".into());
    }
    let mut current = class.to_owned();
    let mut optional = false;
    for (index, name) in parts.iter().enumerate() {
        let field = feature(schema, &current, name)?;
        optional |= field.lower == 0 && field.upper == 1;
        if index + 1 == parts.len() {
            return Ok(ValueType {
                scalar: scalar(schema, &field, conversions)?,
                optional,
                many: field.upper != 1,
            });
        }
        if !field.reference || field.upper != 1 {
            return Err(format!(
                "Path {path}: navigation requires a singular reference at {current}.{name}"
            ));
        }
        current = field.target;
    }
    Err("Empty query path".into())
}
fn check_providers(
    schema: &Schema,
    class: &str,
    path: &str,
    providers: &dyn DerivedValues,
) -> QueryResult<()> {
    if path.starts_with('$') {
        return Ok(());
    }
    let mut current = class.to_owned();
    for name in path.split('.') {
        let field = feature(schema, &current, name)?;
        if field.derived {
            for actual in schema
                .classes
                .keys()
                .filter(|c| schema.conforms(c, &current))
            {
                if !providers.supports(actual, name) {
                    return Err(format!("Unsupported derived feature {actual}.{name}"));
                }
            }
        }
        current = field.target;
    }
    Ok(())
}
fn literal_type(value: &Value) -> QueryResult<ValueType> {
    let scalar = match value {
        Value::String(_) => ScalarType::String,
        Value::Bool(_) => ScalarType::Boolean,
        Value::Number(n) if n.as_i64().is_some() => ScalarType::Integer,
        Value::Object(_) if exact_integer(value).is_ok() => ScalarType::Integer,
        _ => return Err("Query literals must be strings, signed integers, or booleans".into()),
    };
    Ok(ValueType {
        scalar,
        optional: false,
        many: false,
    })
}
fn validate_value(value: &Value, ty: &ValueType) -> QueryResult<()> {
    if ty.many {
        let items = value
            .as_array()
            .ok_or("Expected a collection query value")?;
        let item = ValueType {
            many: false,
            optional: false,
            scalar: ty.scalar.clone(),
        };
        for v in items {
            validate_value(v, &item)?;
        }
        return Ok(());
    }
    if value.is_null() && ty.optional {
        return Ok(());
    }
    let valid = match ty.scalar {
        ScalarType::String | ScalarType::Reference(_) => value.is_string(),
        ScalarType::Boolean => value.is_boolean(),
        ScalarType::Integer => integer(value).is_ok(),
        // The runner validates exact destination/provider/payload before shape checks.
        ScalarType::Converted(_) => value.is_object(),
    };
    if valid {
        Ok(())
    } else {
        Err(format!("Query value does not satisfy {ty:?}"))
    }
}

// Bound the complete tagged input before the shared codec copies/validates it.
// No floating-point conversion participates in comparison or typed projection.
fn exact_integer(value: &Value) -> QueryResult<i64> {
    let object = value
        .as_object()
        .ok_or("Expected exact int64 query value")?;
    if object.len() != 2
        || object.get("kind").and_then(Value::as_str) != Some("int64")
        || object
            .get("value")
            .and_then(Value::as_str)
            .is_none_or(|text| text.len() > 20)
    {
        return Err("Invalid exact int64 query value".into());
    }
    intersect_model::definitions::scalars::Builtin::ELong
        .format(value)?
        .ok_or("Expected present int64 query value")?
        .parse::<i64>()
        .map_err(|error| format!("Invalid exact int64 query value: {error}"))
}

fn integer(value: &Value) -> QueryResult<i64> {
    match value.as_i64() {
        Some(value) => Ok(value),
        None => exact_integer(value),
    }
}

fn portable_literal(value: Value) -> Value {
    match value.as_i64() {
        Some(value) => intersect_model::definitions::scalars::int64_wire(value),
        None => value,
    }
}

fn validate_integer_field(feature: &Feature, value: &Value) -> QueryResult<()> {
    if feature.datatype.is_some()
        || !matches!(feature.target.as_str(), "EInt" | "ELong" | "ELongObject")
        || feature.reference
    {
        return Ok(());
    }
    if value.is_null() && feature.lower == 0 && feature.upper == 1 {
        return Ok(());
    }
    let validate = |value: &Value| {
        if feature.target == "EInt" {
            intersect_model::definitions::scalars::Builtin::EInt.validate(value)
        } else {
            exact_integer(value).map(|_| ())
        }
    };
    if feature.upper == 1 {
        validate(value)?;
    } else {
        for item in value
            .as_array()
            .ok_or("Expected an int64 collection query value")?
        {
            validate(item)?;
        }
    }
    Ok(())
}

struct Validator<'a, A = &'a dyn DerivedValues> {
    schema: &'a Schema,
    providers: A,
    parameters: Parameters,
    conversions: Option<&'a ConversionRegistry<'a>>,
    nodes: usize,
}
impl<A: QueryAccess> Validator<'_, A> {
    fn expression(&mut self, class: &str, expr: &Expression) -> QueryResult<ValueType> {
        self.nodes += 1;
        if self.nodes > 512 {
            return Err("Query expression budget exceeded".into());
        }
        match expr {
            Expression::Field { path } | Expression::Count { path } => {
                let ty = path_type_context(self.schema, class, path, self.conversions)?;
                self.providers.check_path(self.schema, class, path)?;
                if matches!(expr, Expression::Count { .. }) {
                    if !ty.many {
                        return Err(format!("Count requires a collection: {path}"));
                    }
                    Ok(ValueType {
                        scalar: ScalarType::Integer,
                        optional: ty.optional,
                        many: false,
                    })
                } else {
                    Ok(ty)
                }
            }
            Expression::Literal { value } => literal_type(value),
            Expression::Parameter { name } => {
                let value = self
                    .parameters
                    .get(name)
                    .ok_or_else(|| format!("Missing query parameter {name}"))?;
                literal_type(value)
            }
        }
    }
    fn predicate(&mut self, class: &str, pred: &Predicate, depth: usize) -> QueryResult<()> {
        self.nodes += 1;
        if depth > 24 || self.nodes > 512 {
            return Err("Query predicate nesting or expression budget exceeded".into());
        }
        match pred {
            Predicate::IsNull { expression } => {
                self.expression(class, expression)?;
            }
            Predicate::Compare {
                left,
                comparison,
                right,
            } => {
                let a = self.expression(class, left)?;
                let b = self.expression(class, right)?;
                if matches!(a.scalar, ScalarType::Converted(_))
                    || matches!(b.scalar, ScalarType::Converted(_))
                {
                    return Err("Converted query values support projection only; comparison semantics require a language adapter".into());
                }
                if a.many || b.many || a.scalar != b.scalar {
                    return Err("Comparison requires compatible scalar types".into());
                }
                if !matches!(comparison, Comparison::Eq | Comparison::Ne)
                    && !matches!(a.scalar, ScalarType::String | ScalarType::Integer)
                {
                    return Err("Ordered comparisons require strings or integers".into());
                }
            }
            Predicate::And { predicates } | Predicate::Or { predicates } => {
                for p in predicates {
                    self.predicate(class, p, depth + 1)?;
                }
            }
            Predicate::Not { predicate } => self.predicate(class, predicate, depth + 1)?,
            Predicate::Any { path, predicate } | Predicate::All { path, predicate } => {
                let ty = path_type_context(self.schema, class, path, self.conversions)?;
                self.providers.check_path(self.schema, class, path)?;
                let ScalarType::Reference(target) = ty.scalar else {
                    return Err("Any/all requires a reference collection".into());
                };
                if !ty.many {
                    return Err("Any/all requires a reference collection".into());
                }
                self.predicate(&target, predicate, depth + 1)?;
            }
        }
        Ok(())
    }
}

impl QueryPlan {
    pub fn prepare<'a>(
        &'a self,
        schema: &'a Schema,
        parameters: &'a Parameters,
        providers: &'a dyn DerivedValues,
    ) -> QueryResult<PreparedQuery<'a>> {
        self.prepare_context(schema, parameters, providers, None)
    }
    pub fn prepare_with_conversions<'a>(
        &'a self,
        schema: &'a Schema,
        parameters: &'a Parameters,
        providers: &'a dyn DerivedValues,
        conversions: &'a ConversionRegistry<'a>,
    ) -> QueryResult<PreparedQuery<'a>> {
        self.prepare_context(schema, parameters, providers, Some(conversions))
    }
    fn prepare_context<'a>(
        &'a self,
        schema: &'a Schema,
        parameters: &'a Parameters,
        providers: &'a dyn DerivedValues,
        conversions: Option<&'a ConversionRegistry<'a>>,
    ) -> QueryResult<PreparedQuery<'a>> {
        self.validate_context(schema, parameters, providers, conversions)?;
        Ok(PreparedQuery {
            plan: self,
            schema,
            parameters,
            providers,
            conversions,
            features: schema
                .classes
                .keys()
                .map(|name| Ok((name.clone(), schema.features(name)?)))
                .collect::<QueryResult<_>>()?,
        })
    }
    pub fn validate(
        &self,
        schema: &Schema,
        parameters: &Parameters,
        providers: &dyn DerivedValues,
    ) -> QueryResult<()> {
        self.validate_context(schema, parameters, providers, None)
    }
    pub fn validate_with_conversions(
        &self,
        schema: &Schema,
        parameters: &Parameters,
        providers: &dyn DerivedValues,
        conversions: &ConversionRegistry<'_>,
    ) -> QueryResult<()> {
        self.validate_context(schema, parameters, providers, Some(conversions))
    }
    fn validate_context(
        &self,
        schema: &Schema,
        parameters: &Parameters,
        providers: &dyn DerivedValues,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> QueryResult<()> {
        self.validate_access(schema, parameters, providers, conversions)
    }
    fn validate_access<A: QueryAccess>(
        &self,
        schema: &Schema,
        parameters: &Parameters,
        providers: A,
        conversions: Option<&ConversionRegistry<'_>>,
    ) -> QueryResult<()> {
        if self.schema_uri != schema.uri || self.schema_revision != schema.revision {
            return Err("Query belongs to a different schema identity or revision".into());
        }
        if !schema.classes.contains_key(&self.from) {
            return Err(format!("Unknown query class {}", self.from));
        }
        if self.select.is_empty() || self.select.len() > 32 || self.order_by.len() > 8 {
            return Err("Select 1Ã¢â‚¬â€œ32 columns and at most 8 ordering expressions".into());
        }
        let mut validator = Validator {
            schema,
            providers,
            parameters: parameters.clone(),
            conversions,
            nodes: 0,
        };
        let mut names = BTreeSet::new();
        for column in &self.select {
            if column.name.is_empty() || !names.insert(&column.name) {
                return Err("Query column names must be nonempty and unique".into());
            }
            validator.expression(&self.from, &column.expression)?;
        }
        for order in &self.order_by {
            let ty = validator.expression(&self.from, &order.expression)?;
            if ty.many {
                return Err("Cannot order by a collection".into());
            }
            if matches!(ty.scalar, ScalarType::Converted(_)) {
                return Err("Converted query values do not define ordering".into());
            }
        }
        if let Some(filter) = &self.filter {
            validator.predicate(&self.from, filter, 0)?;
        }
        Ok(())
    }
    pub fn execute(
        &self,
        schema: &Schema,
        store: &dyn ModelRead,
        parameters: &Parameters,
        providers: &dyn DerivedValues,
        limits: QueryLimits,
    ) -> QueryResult<QueryOutput> {
        self.prepare(schema, parameters, providers)?
            .execute(store, limits)
    }
    pub fn execute_with_conversions(
        &self,
        schema: &Schema,
        store: &dyn ModelRead,
        parameters: &Parameters,
        providers: &dyn DerivedValues,
        conversions: &ConversionRegistry<'_>,
        limits: QueryLimits,
    ) -> QueryResult<QueryOutput> {
        self.prepare_with_conversions(schema, parameters, providers, conversions)?
            .execute(store, limits)
    }
    pub(crate) fn execute_policy<A: QueryAccess>(
        &self,
        schema: &Schema,
        store: &dyn ModelRead,
        parameters: &Parameters,
        providers: A,
        limits: QueryLimits,
    ) -> QueryResult<QueryOutput> {
        self.validate_access(schema, parameters, providers, None)?;
        let features = schema
            .classes
            .keys()
            .map(|name| Ok((name.clone(), schema.features(name)?)))
            .collect::<QueryResult<FeatureTable>>()?;
        self.execute_access(
            schema, store, parameters, providers, &features, None, limits,
        )
    }
    fn execute_validated(
        &self,
        prepared: &PreparedQuery<'_>,
        store: &dyn ModelRead,
        limits: QueryLimits,
    ) -> QueryResult<QueryOutput> {
        self.execute_access(
            prepared.schema,
            store,
            prepared.parameters,
            prepared.providers,
            &prepared.features,
            prepared.conversions,
            limits,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn execute_access<A: QueryAccess>(
        &self,
        schema: &Schema,
        store: &dyn ModelRead,
        parameters: &Parameters,
        providers: A,
        features: &FeatureTable,
        conversions: Option<&ConversionRegistry<'_>>,
        limits: QueryLimits,
    ) -> QueryResult<QueryOutput> {
        if self.limit == 0 || self.limit > limits.max_results {
            return Err(format!(
                "Query limit must be between 1 and {}",
                limits.max_results
            ));
        }
        providers.before_store(limits)?;
        if store.revision()? != schema.revision {
            return Err("Query/store schema revision mismatch".into());
        }
        let mut runner = Runner {
            schema,
            store,
            providers,
            parameters,
            budget: QueryBudget::new(limits.max_steps),
            features,
            conversions,
        };
        let anchor = self
            .filter
            .as_ref()
            .and_then(|p| anchor(p, schema, &self.from, parameters));
        let anchor = match anchor {
            Some((field, value)) if providers.anchor_stored(schema, &self.from, &field)? => {
                Some((field, value))
            }
            _ => None,
        };
        let mut candidates = 0;
        let mut rows = Vec::new();
        let mut accept = |id: String| -> QueryResult<bool> {
            runner.budget.charge()?;
            candidates += 1;
            let class = store.class(&id)?.ok_or("Missing query candidate")?;
            if !schema.conforms(&class, &self.from) {
                return Ok(false);
            }
            if let Some(filter) = &self.filter
                && !runner.predicate(&id, &class, filter)?
            {
                return Ok(false);
            }
            let keys = self
                .order_by
                .iter()
                .map(|o| runner.expression(&id, &class, &o.expression))
                .collect::<QueryResult<Vec<_>>>()?;
            let values = self
                .select
                .iter()
                .map(|c| runner.expression(&id, &class, &c.expression))
                .collect::<QueryResult<Vec<_>>>()?;
            rows.push((keys, values));
            Ok(self.order_by.is_empty() && rows.len() > self.limit)
        };
        let access = if let Some((field, value)) = anchor {
            // The backend decides indexed versus scan access. This existing API
            // materializes candidate IDs; it is not a bounded-memory claim.
            for id in store.query(Lookup::Value(&field, &value))? {
                if accept(id)? {
                    break;
                }
            }
            format!("feature lookup: {field} (backend index policy)")
        } else {
            let mut after = None;
            'pages: loop {
                let page = store.id_page(after, 128)?;
                if page.is_empty() {
                    break;
                }
                for (cursor, id) in page {
                    after = Some(cursor);
                    if accept(id)? {
                        break 'pages;
                    }
                }
            }
            "identity pages with subtype filtering".into()
        };
        if !self.order_by.is_empty() {
            rows.sort_by(|(a, _), (b, _)| {
                for ((a, b), order) in a.iter().zip(b).zip(&self.order_by) {
                    let cmp = compare_values(a, b);
                    let cmp = if order.descending { cmp.reverse() } else { cmp };
                    if !cmp.is_eq() {
                        return cmp;
                    }
                }
                std::cmp::Ordering::Equal
            });
        }
        let has_more = rows.len() > self.limit;
        rows.truncate(self.limit);
        Ok(QueryOutput {
            schema_revision: schema.revision.clone(),
            columns: self.select.iter().map(|c| c.name.clone()).collect(),
            rows: rows.into_iter().map(|(_, v)| v).collect(),
            has_more,
            stats: QueryStats {
                candidates,
                reads: runner.budget.reads,
                access,
            },
        })
    }
}

/// Borrows immutable schema, plan, parameters and provider definitions. Reuse
/// against later transaction states; each execution reads current values and
/// checks the store revision. This does not retain a model snapshot or cache.
pub struct PreparedQuery<'a> {
    plan: &'a QueryPlan,
    schema: &'a Schema,
    parameters: &'a Parameters,
    providers: &'a dyn DerivedValues,
    features: FeatureTable,
    conversions: Option<&'a ConversionRegistry<'a>>,
}
impl PreparedQuery<'_> {
    pub fn execute(&self, store: &dyn ModelRead, limits: QueryLimits) -> QueryResult<QueryOutput> {
        self.plan.execute_validated(self, store, limits)
    }
}

fn anchor(
    pred: &Predicate,
    schema: &Schema,
    class: &str,
    parameters: &Parameters,
) -> Option<(String, Value)> {
    match pred {
        Predicate::Compare {
            left: Expression::Field { path },
            comparison: Comparison::Eq,
            right,
        } if !path.contains('.') && !path.starts_with('$') => {
            let field = feature(schema, class, path).ok()?;
            if field.derived || field.reference || field.upper != 1 {
                return None;
            }
            let value = match right {
                Expression::Literal { value } => value,
                Expression::Parameter { name } => parameters.get(name)?,
                _ => return None,
            };
            let value = if matches!(scalar(schema, &field, None).ok()?, ScalarType::Integer) {
                let value = integer(value).ok()?;
                if matches!(field.target.as_str(), "ELong" | "ELongObject") {
                    intersect_model::definitions::scalars::int64_wire(value)
                } else {
                    Value::from(value)
                }
            } else {
                value.clone()
            };
            Some((path.clone(), value))
        }
        Predicate::And { predicates } => predicates
            .iter()
            .find_map(|p| anchor(p, schema, class, parameters)),
        _ => None,
    }
}
fn compare_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    if matches!(a, Value::Number(_) | Value::Object(_))
        && matches!(b, Value::Number(_) | Value::Object(_))
        && let (Ok(a), Ok(b)) = (integer(a), integer(b))
    {
        return a.cmp(&b);
    }
    match (a, b) {
        (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
        (Value::Null, _) => std::cmp::Ordering::Less,
        (_, Value::Null) => std::cmp::Ordering::Greater,
        (Value::String(a), Value::String(b)) => a.cmp(b),
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        _ => std::cmp::Ordering::Equal, // validated expression types exclude mixed comparisons
    }
}
struct Runner<'a, A = &'a dyn DerivedValues> {
    schema: &'a Schema,
    store: &'a dyn ModelRead,
    providers: A,
    parameters: &'a Parameters,
    budget: QueryBudget,
    features: &'a FeatureTable,
    conversions: Option<&'a ConversionRegistry<'a>>,
}
impl<A: QueryAccess> Runner<'_, A> {
    fn path(&mut self, id: &str, class: &str, path: &str) -> QueryResult<Value> {
        read_path(
            self.schema,
            self.store,
            self.providers,
            self.features,
            self.conversions,
            &mut self.budget,
            id,
            class,
            path,
        )
    }
    fn expression(&mut self, id: &str, class: &str, expr: &Expression) -> QueryResult<Value> {
        self.budget.charge()?;
        match expr {
            Expression::Field { path } => self.path(id, class, path),
            Expression::Count { path } => {
                if !path.contains('.')
                    && let Some(field) =
                        self.features.get(class).and_then(|fields| fields.get(path))
                    && !field.derived
                    && self.providers.count_stored(self.schema, class, path)?
                {
                    self.budget.charge()?;
                    return self
                        .store
                        .value_len(id, path)?
                        .map(Value::from)
                        .ok_or_else(|| format!("Missing or invalid collection {class}.{path}"));
                }
                let value = self.path(id, class, path)?;
                if value.is_null() {
                    Ok(Value::Null)
                } else {
                    Ok(Value::from(
                        value.as_array().ok_or("Count requires collection")?.len(),
                    ))
                }
            }
            Expression::Literal { value } => Ok(value.clone()),
            Expression::Parameter { name } => self
                .parameters
                .get(name)
                .cloned()
                .ok_or_else(|| format!("Missing query parameter {name}")),
        }
    }
    fn predicate(&mut self, id: &str, class: &str, predicate: &Predicate) -> QueryResult<bool> {
        self.budget.charge()?;
        match predicate {
            Predicate::IsNull { expression } => {
                Ok(self.expression(id, class, expression)?.is_null())
            }
            Predicate::Compare {
                left,
                comparison,
                right,
            } => {
                let a = self.expression(id, class, left)?;
                let b = self.expression(id, class, right)?;
                // Optional values do not satisfy scalar comparisons, including ne.
                if a.is_null() || b.is_null() {
                    return Ok(false);
                }
                let cmp = compare_values(&a, &b);
                Ok(match comparison {
                    Comparison::Eq => cmp.is_eq(),
                    Comparison::Ne => !cmp.is_eq(),
                    Comparison::Lt => cmp.is_lt(),
                    Comparison::Le => !cmp.is_gt(),
                    Comparison::Gt => cmp.is_gt(),
                    Comparison::Ge => !cmp.is_lt(),
                })
            }
            Predicate::And { predicates } => {
                for p in predicates {
                    if !self.predicate(id, class, p)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Predicate::Or { predicates } => {
                for p in predicates {
                    if self.predicate(id, class, p)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Predicate::Not { predicate } => Ok(!self.predicate(id, class, predicate)?),
            Predicate::Any { path, predicate } => self.quantify(id, class, path, predicate, false),
            Predicate::All { path, predicate } => self.quantify(id, class, path, predicate, true),
        }
    }
    fn quantify(
        &mut self,
        id: &str,
        class: &str,
        path: &str,
        predicate: &Predicate,
        all: bool,
    ) -> QueryResult<bool> {
        let value = self.path(id, class, path)?;
        if value.is_null() {
            return Ok(false);
        }
        let ty = path_type_context(self.schema, class, path, self.conversions)?;
        let ScalarType::Reference(target) = ty.scalar else {
            return Err("Expected reference collection".into());
        };
        for value in value.as_array().ok_or("Expected collection")? {
            self.budget.charge()?;
            let next = value.as_str().ok_or("Invalid reference identity")?;
            let actual = self
                .store
                .class(next)?
                .ok_or_else(|| format!("Unavailable reference target {next}"))?;
            if !self.schema.conforms(&actual, &target) {
                return Err("Incompatible collection target".into());
            }
            let matched = self.predicate(next, &actual, predicate)?;
            if matched != all {
                return Ok(!all);
            }
        }
        Ok(all)
    }
}

/// Internal execution policy: legacy providers retain their original branches.
/// New semantic sessions supply one distinct fallible policy, not a ModelRead overlay.
pub(crate) trait QueryAccess: Copy {
    fn before_store(self, _limits: QueryLimits) -> QueryResult<()> {
        Ok(())
    }
    fn check_object(
        self,
        _schema: &Schema,
        _store: &dyn ModelRead,
        _id: &str,
        _class: &str,
    ) -> QueryResult<()> {
        Ok(())
    }

    fn check_path(self, schema: &Schema, class: &str, path: &str) -> QueryResult<()>;
    fn read_field(
        self,
        schema: &Schema,
        store: &dyn ModelRead,
        id: &str,
        class: &str,
        field: &Feature,
        budget: &mut QueryBudget,
    ) -> QueryResult<Value>;
    fn count_stored(self, schema: &Schema, class: &str, name: &str) -> QueryResult<bool>;
    fn anchor_stored(self, schema: &Schema, class: &str, name: &str) -> QueryResult<bool>;
}
impl QueryAccess for &dyn DerivedValues {
    fn check_path(self, schema: &Schema, class: &str, path: &str) -> QueryResult<()> {
        check_providers(schema, class, path, self)
    }
    fn read_field(
        self,
        _: &Schema,
        store: &dyn ModelRead,
        id: &str,
        _: &str,
        field: &Feature,
        budget: &mut QueryBudget,
    ) -> QueryResult<Value> {
        if field.derived {
            self.read(store, id, &field.name, budget)
        } else {
            Ok(store.value(id, &field.name)?.unwrap_or(Value::Null))
        }
    }
    fn count_stored(self, _: &Schema, _: &str, _: &str) -> QueryResult<bool> {
        Ok(true)
    }
    fn anchor_stored(self, _: &Schema, _: &str, _: &str) -> QueryResult<bool> {
        Ok(true)
    }
}
trait FeatureLookup {
    fn feature(&self, class: &str, name: &str) -> QueryResult<&Feature>;
}
impl FeatureLookup for FeatureTable {
    fn feature(&self, class: &str, name: &str) -> QueryResult<&Feature> {
        self.get(class)
            .and_then(|fields| fields.get(name))
            .ok_or_else(|| format!("Unknown feature {class}.{name}"))
    }
}
impl FeatureLookup for Schema {
    fn feature(&self, class: &str, name: &str) -> QueryResult<&Feature> {
        let member = self.effective_feature(class, name)?;
        self.classes
            .get(member.owner)
            .and_then(|owner| owner.features.get(name))
            .ok_or_else(|| format!("Unknown feature {class}.{name}"))
    }
}
/// Same path walker as query evaluation; the direct semantic path resolves
/// metadata lazily rather than retaining another expanded feature catalog.
pub(crate) fn policy_path<A: QueryAccess>(
    schema: &Schema,
    store: &dyn ModelRead,
    providers: A,
    budget: &mut QueryBudget,
    id: &str,
    class: &str,
    path: &str,
) -> QueryResult<Value> {
    read_path(
        schema, store, providers, schema, None, budget, id, class, path,
    )
}
#[allow(clippy::too_many_arguments)]
fn read_path<A: QueryAccess>(
    schema: &Schema,
    store: &dyn ModelRead,
    providers: A,
    fields: &impl FeatureLookup,
    conversions: Option<&ConversionRegistry<'_>>,
    budget: &mut QueryBudget,
    id: &str,
    class: &str,
    path: &str,
) -> QueryResult<Value> {
    budget.charge()?;
    providers.check_object(schema, store, id, class)?;
    if path == "$id" {
        return Ok(Value::String(id.into()));
    }
    if path == "$class" {
        return Ok(Value::String(class.into()));
    }
    if path == "$resource" {
        return Ok(Value::String(
            store.get(id)?.ok_or("Missing query object")?.resource,
        ));
    }
    let parts: Vec<_> = path.split('.').collect();
    let mut id = id.to_owned();
    let mut class = class.to_owned();
    for (index, name) in parts.iter().enumerate() {
        budget.charge()?;
        let field = fields.feature(&class, name)?;
        let value = providers.read_field(schema, store, &id, &class, field, budget)?;
        if let Some(items) = value.as_array() {
            for _ in items {
                budget.charge()?;
            }
        }
        validate_integer_field(field, &value)?;
        let scalar = scalar(schema, field, conversions)?;
        if matches!(scalar, ScalarType::Converted(_)) {
            let registry = conversions.ok_or("Converted query requires explicit providers")?;
            let validate =
                |value: &Value| schema.validate_attribute_with_conversions(field, value, registry);
            if field.upper != 1 {
                for item in value
                    .as_array()
                    .ok_or("Expected converted collection query value")?
                {
                    validate(item)?;
                }
            } else if !value.is_null() {
                validate(&value)?;
            }
        }
        validate_value(
            &value,
            &ValueType {
                scalar,
                optional: field.lower == 0 && field.upper == 1,
                many: field.upper != 1,
            },
        )?;
        if index + 1 == parts.len() || value.is_null() {
            return Ok(value);
        }
        id = value
            .as_str()
            .ok_or("Reference path contains a non-identity value")?
            .to_owned();
        class = store
            .class(&id)?
            .ok_or_else(|| format!("Unavailable reference target {id}"))?;
        if !schema.conforms(&class, &field.target) {
            return Err("Reference target has an incompatible class".into());
        }
    }
    Err("Empty query path".into())
}
