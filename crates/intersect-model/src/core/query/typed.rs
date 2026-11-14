//! Handwritten typed frontend. Descriptors are schema-checked at binding time;
//! the serialized plan executes through the same engine as dynamic clients.
use super::*;
use std::marker::PhantomData;

/// Implemented by a language's Rust marker types, not by core domain classes.
pub trait Entity {
    const CLASS: &'static str;
    const SCHEMA_URI: &'static str;
}
/// Values supported in typed projections. Sealed to preserve decoding guarantees.
pub trait QueryValue: sealed::Sealed + Sized {
    fn value_type() -> ValueType;
    fn valid_shape() -> bool {
        true
    }
    fn reference_schema_uri() -> Option<&'static str> {
        None
    }
    fn decode(value: Value) -> QueryResult<Self>;
}
/// Language-owned marker for one exact source datatype declaration.
/// Short names and namespace aliases are insufficient identities.
pub trait ConvertedDatatype {
    fn declaration() -> DeclarationId;
}
/// A checked nominal column retains its complete converted-value envelope.
/// Query execution validates the selected provider and destination payload;
/// standalone decoding checks the envelope identity only.
pub struct ConvertedColumn<D: ConvertedDatatype> {
    value: crate::definitions::conversion::ConvertedValue,
    marker: PhantomData<fn() -> D>,
}
impl<D: ConvertedDatatype> ConvertedColumn<D> {
    pub fn converted(&self) -> &crate::definitions::conversion::ConvertedValue {
        &self.value
    }
    pub fn into_converted(self) -> crate::definitions::conversion::ConvertedValue {
        self.value
    }
}
impl<D: ConvertedDatatype> QueryValue for ConvertedColumn<D> {
    fn value_type() -> ValueType {
        ValueType {
            scalar: ScalarType::Converted(D::declaration()),
            optional: false,
            many: false,
        }
    }
    fn decode(value: Value) -> QueryResult<Self> {
        let value: crate::definitions::conversion::ConvertedValue =
            serde_json::from_value(value)
                .map_err(|e| format!("Invalid converted query result: {e}"))?;
        let crate::definitions::conversion::ConvertedValue::Converted {
            datatype,
            value: payload,
            ..
        } = &value;
        if datatype != &D::declaration() || payload.is_null() {
            return Err(
                "Converted query result belongs to a different datatype or is absent".into(),
            );
        }
        Ok(Self {
            value,
            marker: PhantomData,
        })
    }
}
mod sealed {
    pub trait Sealed {}
    impl Sealed for String {}
    impl Sealed for i64 {}
    impl Sealed for bool {}
    impl<D: super::ConvertedDatatype> Sealed for super::ConvertedColumn<D> {}
    impl<T: super::QueryValue> Sealed for Option<T> {}
    impl<T: super::QueryValue> Sealed for Vec<T> {}
    impl<M: super::Entity> Sealed for super::ReferenceId<M> {}
}
macro_rules! scalar_value {
    ($ty:ty, $kind:ident) => {
        impl QueryValue for $ty {
            fn value_type() -> ValueType {
                ValueType {
                    scalar: ScalarType::$kind,
                    optional: false,
                    many: false,
                }
            }
            fn decode(value: Value) -> QueryResult<Self> {
                serde_json::from_value(value)
                    .map_err(|e| format!("Invalid typed query result: {e}"))
            }
        }
    };
}
scalar_value!(String, String);
scalar_value!(bool, Boolean);
impl QueryValue for i64 {
    fn value_type() -> ValueType {
        ValueType {
            scalar: ScalarType::Integer,
            optional: false,
            many: false,
        }
    }
    fn decode(value: Value) -> QueryResult<Self> {
        integer(&value).map_err(|error| format!("Invalid typed query result: {error}"))
    }
}
impl<T: QueryValue> QueryValue for Option<T> {
    fn valid_shape() -> bool {
        T::valid_shape() && !T::value_type().optional
    }
    fn reference_schema_uri() -> Option<&'static str> {
        T::reference_schema_uri()
    }
    fn value_type() -> ValueType {
        ValueType {
            optional: true,
            ..T::value_type()
        }
    }
    fn decode(value: Value) -> QueryResult<Self> {
        if value.is_null() {
            Ok(None)
        } else {
            T::decode(value).map(Some)
        }
    }
}
impl<T: QueryValue> QueryValue for Vec<T> {
    fn valid_shape() -> bool {
        T::valid_shape() && !T::value_type().optional && !T::value_type().many
    }
    fn reference_schema_uri() -> Option<&'static str> {
        T::reference_schema_uri()
    }
    fn value_type() -> ValueType {
        ValueType {
            many: true,
            ..T::value_type()
        }
    }
    fn decode(value: Value) -> QueryResult<Self> {
        let Value::Array(values) = value else {
            return Err("Expected collection result".into());
        };
        values.into_iter().map(T::decode).collect()
    }
}
/// Typed external identity, NOT an allocation handle or a historical snapshot.
pub struct ReferenceId<M: Entity> {
    pub id: String,
    marker: PhantomData<fn() -> M>,
}
impl<M: Entity> QueryValue for ReferenceId<M> {
    fn reference_schema_uri() -> Option<&'static str> {
        Some(M::SCHEMA_URI)
    }
    fn value_type() -> ValueType {
        ValueType {
            scalar: ScalarType::Reference(M::CLASS.into()),
            optional: false,
            many: false,
        }
    }
    fn decode(value: Value) -> QueryResult<Self> {
        Ok(Self {
            id: String::decode(value)?,
            marker: PhantomData,
        })
    }
}

/// Runtime-checked binding to immutable schema definitions. Marker classes keep
/// predicates for different roots from being mixed by Rust callers.
pub struct Binding<'s, M: Entity> {
    schema: &'s Schema,
    conversions: Option<&'s ConversionRegistry<'s>>,
    marker: PhantomData<fn() -> M>,
}
impl<'s, M: Entity> Binding<'s, M> {
    pub fn new(schema: &'s Schema) -> QueryResult<Self> {
        if schema.uri != M::SCHEMA_URI || !schema.classes.contains_key(M::CLASS) {
            return Err(format!("Cannot bind {} to this schema", M::CLASS));
        }
        Ok(Self {
            schema,
            conversions: None,
            marker: PhantomData,
        })
    }
    /// Select a registry for exact nominal field descriptors. Execution must
    /// supply its explicit conversion context again; no global registry is installed.
    pub fn with_conversions(
        schema: &'s Schema,
        conversions: &'s ConversionRegistry<'s>,
    ) -> QueryResult<Self> {
        let mut binding = Self::new(schema)?;
        binding.conversions = Some(conversions);
        Ok(binding)
    }
    pub fn field<T: QueryValue>(&self, path: &str) -> QueryResult<Field<M, T>> {
        if !T::valid_shape() {
            return Err(
                "Nested or nullable collection items are unsupported typed query values".into(),
            );
        }
        if T::reference_schema_uri().is_some_and(|uri| uri != self.schema.uri) {
            return Err("Reference descriptor belongs to a different schema".into());
        }
        let actual = path_type_context(self.schema, M::CLASS, path, self.conversions)?;
        if actual != T::value_type() {
            return Err(format!(
                "Field {}.{path} is {actual:?}, requested {:?}",
                M::CLASS,
                T::value_type()
            ));
        }
        Ok(Field {
            expression: Expression::Field { path: path.into() },
            revision: self.schema.revision.clone(),
            marker: PhantomData,
        })
    }
    pub fn count(&self, path: &str) -> QueryResult<Field<M, i64>> {
        let actual = path_type_context(self.schema, M::CLASS, path, self.conversions)?;
        if !actual.many || actual.optional {
            return Err("Typed count requires a collection without optional navigation".into());
        }
        Ok(Field {
            expression: Expression::Count { path: path.into() },
            revision: self.schema.revision.clone(),
            marker: PhantomData,
        })
    }
    pub fn query(&self) -> TypedQuery<M, ()> {
        TypedQuery {
            plan: QueryPlan {
                schema_uri: self.schema.uri.clone(),
                schema_revision: self.schema.revision.clone(),
                from: M::CLASS.into(),
                filter: None,
                select: vec![],
                order_by: vec![],
                limit: 100,
            },
            bindings: vec![],
            marker: PhantomData,
        }
    }
}
/// Typed field expressions reject incompatible comparison operands in Rust.
/// ```compile_fail
/// use intersect_model::core::query::{Entity, Field};
/// enum Copy {}
/// impl Entity for Copy { const CLASS: &'static str = "Copy"; const SCHEMA_URI: &'static str = "urn:test"; }
/// fn invalid(field: Field<Copy, i64>) { field.eq("two".to_owned()); }
/// ```
pub struct Field<M: Entity, T: QueryValue> {
    expression: Expression,
    revision: String,
    marker: PhantomData<fn() -> (M, T)>,
}
impl<M: Entity, T: QueryValue> Clone for Field<M, T> {
    fn clone(&self) -> Self {
        Self {
            expression: self.expression.clone(),
            revision: self.revision.clone(),
            marker: PhantomData,
        }
    }
}
impl<M: Entity, T: QueryValue> Field<M, T> {
    pub fn ascending(&self) -> TypedOrdering<M> {
        self.order(false)
    }
    pub fn descending(&self) -> TypedOrdering<M> {
        self.order(true)
    }
    fn order(&self, descending: bool) -> TypedOrdering<M> {
        TypedOrdering {
            order: Ordering {
                expression: self.expression.clone(),
                descending,
            },
            revision: self.revision.clone(),
            marker: PhantomData,
        }
    }
}
impl<M: Entity, T: QueryValue> Field<M, Option<T>> {
    pub fn is_none(&self) -> TypedPredicate<M> {
        TypedPredicate {
            predicate: Predicate::IsNull {
                expression: self.expression.clone(),
            },
            bindings: vec![self.revision.clone()],
            marker: PhantomData,
        }
    }
    pub fn is_some(&self) -> TypedPredicate<M> {
        !self.is_none()
    }
}
/// Scalars expose comparisons with their underlying value, including optional
/// fields. An absent optional field satisfies no scalar comparison.
pub trait Comparable: QueryValue {
    type Input: Into<Value>;
}
impl Comparable for String {
    type Input = String;
}
impl Comparable for i64 {
    type Input = i64;
}
impl Comparable for bool {
    type Input = bool;
}
impl Comparable for Option<String> {
    type Input = String;
}
impl Comparable for Option<i64> {
    type Input = i64;
}
impl Comparable for Option<bool> {
    type Input = bool;
}
impl<M: Entity, T: Comparable> Field<M, T> {
    pub fn eq(&self, value: T::Input) -> TypedPredicate<M> {
        self.compare(Comparison::Eq, value)
    }
    pub fn ne(&self, value: T::Input) -> TypedPredicate<M> {
        self.compare(Comparison::Ne, value)
    }
    fn compare(&self, comparison: Comparison, value: T::Input) -> TypedPredicate<M> {
        self.comparison(
            comparison,
            Expression::Literal {
                value: portable_literal(value.into()),
            },
        )
    }
    pub fn eq_parameter(&self, parameter: &Parameter<T::Input>) -> TypedPredicate<M> {
        self.comparison(
            Comparison::Eq,
            Expression::Parameter {
                name: parameter.name.clone(),
            },
        )
    }
    fn comparison(&self, comparison: Comparison, right: Expression) -> TypedPredicate<M> {
        TypedPredicate {
            predicate: Predicate::Compare {
                left: self.expression.clone(),
                comparison,
                right,
            },
            bindings: vec![self.revision.clone()],
            marker: PhantomData,
        }
    }
}
pub trait OrderedValue: Comparable {}
impl OrderedValue for String {}
impl OrderedValue for i64 {}
impl OrderedValue for Option<String> {}
impl OrderedValue for Option<i64> {}
impl<M: Entity, T: OrderedValue> Field<M, T> {
    pub fn lt(&self, value: T::Input) -> TypedPredicate<M> {
        self.compare(Comparison::Lt, value)
    }
    pub fn le(&self, value: T::Input) -> TypedPredicate<M> {
        self.compare(Comparison::Le, value)
    }
    pub fn gt(&self, value: T::Input) -> TypedPredicate<M> {
        self.compare(Comparison::Gt, value)
    }
    pub fn ge(&self, value: T::Input) -> TypedPredicate<M> {
        self.compare(Comparison::Ge, value)
    }
}
impl<M: Entity, N: Entity> Field<M, Vec<ReferenceId<N>>> {
    pub fn any(&self, predicate: TypedPredicate<N>) -> TypedPredicate<M> {
        self.quantify(predicate, false)
    }
    pub fn all(&self, predicate: TypedPredicate<N>) -> TypedPredicate<M> {
        self.quantify(predicate, true)
    }
    fn quantify(&self, predicate: TypedPredicate<N>, all: bool) -> TypedPredicate<M> {
        let path = match &self.expression {
            Expression::Field { path } => path.clone(),
            _ => String::new(),
        };
        let mut bindings = predicate.bindings;
        bindings.push(self.revision.clone());
        let predicate = Box::new(predicate.predicate);
        TypedPredicate {
            predicate: if all {
                Predicate::All { path, predicate }
            } else {
                Predicate::Any { path, predicate }
            },
            bindings,
            marker: PhantomData,
        }
    }
}
pub struct Parameter<T> {
    name: String,
    marker: PhantomData<fn() -> T>,
}
impl<T: Into<Value>> Parameter<T> {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            marker: PhantomData,
        }
    }
    pub fn bind(&self, bindings: &mut Parameters, value: T) {
        bindings.insert(self.name.clone(), portable_literal(value.into()));
    }
}
/// Predicates belonging to different model roots cannot be combined.
/// ```compile_fail
/// use intersect_model::core::query::{Entity, TypedPredicate};
/// enum Copy {} enum Loan {}
/// impl Entity for Copy { const CLASS: &'static str = "Copy"; const SCHEMA_URI: &'static str = "urn:test"; }
/// impl Entity for Loan { const CLASS: &'static str = "Loan"; const SCHEMA_URI: &'static str = "urn:test"; }
/// fn invalid(a: TypedPredicate<Copy>, b: TypedPredicate<Loan>) { a.and(b); }
/// ```
pub struct TypedPredicate<M: Entity> {
    predicate: Predicate,
    bindings: Vec<String>,
    marker: PhantomData<fn() -> M>,
}
impl<M: Entity> TypedPredicate<M> {
    pub fn and(self, other: Self) -> Self {
        self.combine(other, true)
    }
    pub fn or(self, other: Self) -> Self {
        self.combine(other, false)
    }
    fn combine(mut self, other: Self, and: bool) -> Self {
        self.bindings.extend(other.bindings);
        let predicates = vec![self.predicate, other.predicate];
        Self {
            predicate: if and {
                Predicate::And { predicates }
            } else {
                Predicate::Or { predicates }
            },
            bindings: self.bindings,
            marker: PhantomData,
        }
    }
}
impl<M: Entity> std::ops::Not for TypedPredicate<M> {
    type Output = Self;
    fn not(self) -> Self {
        Self {
            predicate: Predicate::Not {
                predicate: Box::new(self.predicate),
            },
            ..self
        }
    }
}
pub struct TypedOrdering<M: Entity> {
    order: Ordering,
    revision: String,
    marker: PhantomData<fn() -> M>,
}
pub trait Projection<M: Entity> {
    type Output;
    fn columns(self) -> (Vec<Expression>, Vec<String>);
    fn decode(values: Vec<Value>) -> QueryResult<Self::Output>;
}
impl<M: Entity, T: QueryValue> Projection<M> for Field<M, T> {
    type Output = T;
    fn columns(self) -> (Vec<Expression>, Vec<String>) {
        (vec![self.expression], vec![self.revision])
    }
    fn decode(mut values: Vec<Value>) -> QueryResult<T> {
        if values.len() != 1 {
            return Err("Typed projection arity mismatch".into());
        }
        T::decode(values.remove(0))
    }
}
macro_rules! tuple_projection {
    ($($t:ident:$v:ident),+) => {
        impl<M: Entity, $($t: QueryValue),+> Projection<M> for ($(Field<M,$t>,)+) {
            type Output = ($($t,)+);
            fn columns(self) -> (Vec<Expression>, Vec<String>) {
                let ($($v,)+) = self;
                (vec![$($v.expression,)+], vec![$($v.revision,)+])
            }
            fn decode(values: Vec<Value>) -> QueryResult<Self::Output> {
                let mut values = values.into_iter();
                let result = ($($t::decode(values.next().ok_or("Typed projection arity mismatch")?)?,)+);
                if values.next().is_some() { return Err("Typed projection arity mismatch".into()); }
                Ok(result)
            }
        }
    }
}
tuple_projection!(A:a, B:b);
tuple_projection!(A:a, B:b, C:c);
tuple_projection!(A:a, B:b, C:c, D:d);
pub struct TypedQuery<M: Entity, P> {
    plan: QueryPlan,
    bindings: Vec<String>,
    marker: PhantomData<fn() -> (M, P)>,
}
impl<M: Entity, P> TypedQuery<M, P> {
    pub fn filter(mut self, predicate: TypedPredicate<M>) -> Self {
        self.bindings.extend(predicate.bindings);
        self.plan.filter = Some(match self.plan.filter {
            Some(old) => Predicate::And {
                predicates: vec![old, predicate.predicate],
            },
            None => predicate.predicate,
        });
        self
    }
    pub fn select<N: Projection<M>>(mut self, projection: N) -> TypedQuery<M, N> {
        let (expressions, bindings) = projection.columns();
        self.bindings.extend(bindings);
        self.plan.select = expressions
            .into_iter()
            .enumerate()
            .map(|(i, expression)| Column {
                name: format!("column_{}", i + 1),
                expression,
            })
            .collect();
        TypedQuery {
            plan: self.plan,
            bindings: self.bindings,
            marker: PhantomData,
        }
    }
    pub fn order_by(mut self, order: TypedOrdering<M>) -> Self {
        self.plan.order_by.push(order.order);
        self.bindings.push(order.revision);
        self
    }
    pub fn limit(mut self, limit: usize) -> Self {
        self.plan.limit = limit;
        self
    }
    pub fn plan(&self) -> QueryResult<&QueryPlan> {
        if self
            .bindings
            .iter()
            .any(|r| r != &self.plan.schema_revision)
        {
            return Err("Mixed schema revisions in typed query".into());
        }
        Ok(&self.plan)
    }
}
pub struct TypedOutput<T> {
    pub rows: Vec<T>,
    pub has_more: bool,
    pub stats: QueryStats,
}
impl<M: Entity, P: Projection<M>> TypedQuery<M, P> {
    pub fn prepare<'a>(
        &'a self,
        schema: &'a Schema,
        parameters: &'a Parameters,
        providers: &'a dyn DerivedValues,
    ) -> QueryResult<PreparedTypedQuery<'a, M, P>> {
        Ok(PreparedTypedQuery {
            prepared: self.plan()?.prepare(schema, parameters, providers)?,
            marker: PhantomData,
        })
    }
    pub fn prepare_with_conversions<'a>(
        &'a self,
        schema: &'a Schema,
        parameters: &'a Parameters,
        providers: &'a dyn DerivedValues,
        conversions: &'a ConversionRegistry<'a>,
    ) -> QueryResult<PreparedTypedQuery<'a, M, P>> {
        Ok(PreparedTypedQuery {
            prepared: self.plan()?.prepare_with_conversions(
                schema,
                parameters,
                providers,
                conversions,
            )?,
            marker: PhantomData,
        })
    }
    pub fn execute_with_conversions(
        &self,
        schema: &Schema,
        store: &dyn ModelRead,
        parameters: &Parameters,
        providers: &dyn DerivedValues,
        conversions: &ConversionRegistry<'_>,
        limits: QueryLimits,
    ) -> QueryResult<TypedOutput<P::Output>> {
        self.prepare_with_conversions(schema, parameters, providers, conversions)?
            .execute(store, limits)
    }
    pub fn execute(
        &self,
        schema: &Schema,
        store: &dyn ModelRead,
        parameters: &Parameters,
        providers: &dyn DerivedValues,
        limits: QueryLimits,
    ) -> QueryResult<TypedOutput<P::Output>> {
        let result = self
            .plan()?
            .execute(schema, store, parameters, providers, limits)?;
        Ok(TypedOutput {
            rows: result
                .rows
                .into_iter()
                .map(P::decode)
                .collect::<QueryResult<_>>()?,
            has_more: result.has_more,
            stats: result.stats,
        })
    }
}
pub struct PreparedTypedQuery<'a, M: Entity, P: Projection<M>> {
    prepared: PreparedQuery<'a>,
    marker: PhantomData<fn() -> (M, P)>,
}
impl<M: Entity, P: Projection<M>> PreparedTypedQuery<'_, M, P> {
    pub fn execute(
        &self,
        store: &dyn ModelRead,
        limits: QueryLimits,
    ) -> QueryResult<TypedOutput<P::Output>> {
        let result = self.prepared.execute(store, limits)?;
        Ok(TypedOutput {
            rows: result
                .rows
                .into_iter()
                .map(P::decode)
                .collect::<QueryResult<_>>()?,
            has_more: result.has_more,
            stats: result.stats,
        })
    }
}
