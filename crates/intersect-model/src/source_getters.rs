//! Fresh source getter installation and the shared query adapter.
//! No language naming, visibility or other domain policy belongs here.
use crate::{
    core::{query::QueryBudget, storage::ModelRead},
    definitions::{
        bundle::DefinitionBundle,
        ecore::ParseLimits,
        rcore_behavior::{
            ir::ValueType,
            ir_v2::{DataWire, Limits, ValueWire},
            types::Scalar,
        },
        rcore_getter::{VerifiedGetter, compile_getter},
        scalars::Builtin,
        schema::{Derivation, Expression, Feature, Package, Schema},
    },
    query::{DerivedValues, PackageDerived},
    rcore_behavior_functions::{ExecutionContext, ExecutionLimits},
    scope_runtime,
    semantic_bindings::BorrowedBindingWork,
    source_model_reads::SourceModelReads,
};
use serde_json::Value;
use std::collections::BTreeMap;

/// Private installed state, rebuilt from authored sources on package reload.
#[derive(Clone, Debug, Default)]
pub(crate) struct SourceGetters {
    entries: Vec<VerifiedGetter>,
    dispatch: BTreeMap<String, BTreeMap<String, usize>>,
}
impl SourceGetters {
    pub(crate) fn compile(
        package: &Package,
        bundle: &DefinitionBundle,
        limits: &Limits,
        source_limits: ParseLimits,
    ) -> Result<Self, crate::rcore::RcoreError> {
        Self::compile_rules(
            &package.schema,
            &package.semantics.derived,
            bundle,
            limits,
            source_limits,
        )
    }
    pub(crate) fn compile_rules(
        schema: &Schema,
        rules: &[Derivation],
        bundle: &DefinitionBundle,
        limits: &Limits,
        source_limits: ParseLimits,
    ) -> Result<Self, crate::rcore::RcoreError> {
        let mut result = Self::default();
        for rule in rules {
            let Expression::Source {
                source,
                declaration,
            } = &rule.expression
            else {
                continue;
            };
            let getter = compile_getter(bundle, source, *declaration, limits, source_limits)?;
            let receiver = schema.class_key_for_type(getter.receiver())?;
            let member = schema.feature_by_id(receiver, getter.member())?;
            if receiver != rule.class || member.feature.name != rule.feature {
                return Err(
                    "Source getter selector differs from its exact package member"
                        .to_owned()
                        .into(),
                );
            }
            let feature = member.feature;
            crate::definitions::rcore_getter::check_storage_feature(schema, feature)?;
            let index = result.entries.len();
            for actual in schema.classes.keys() {
                if !schema.conforms(actual, receiver) {
                    continue;
                }
                let member = schema.feature_by_id(actual, getter.member())?;
                if result
                    .dispatch
                    .entry(actual.clone())
                    .or_default()
                    .insert(member.feature.name.clone(), index)
                    .is_some()
                {
                    return Err("Conflicting source getters for one effective feature"
                        .to_owned()
                        .into());
                }
            }
            result.entries.push(getter);
        }
        Ok(result)
    }

    pub(crate) fn selected(&self, actual: &str, feature: &str) -> Result<&VerifiedGetter, String> {
        let index = self
            .dispatch
            .get(actual)
            .and_then(|members| members.get(feature))
            .ok_or_else(|| format!("No source getter for {actual}.{feature}"))?;
        self.entries
            .get(*index)
            .ok_or_else(|| "Invalid source getter dispatch".into())
    }
}

pub(crate) struct SourceDerived<'a> {
    pub(crate) package: &'a Package,
    pub(crate) bundle: &'a DefinitionBundle,
    pub(crate) getters: &'a SourceGetters,
    pub(crate) limits: &'a ExecutionLimits,
}
impl DerivedValues for SourceDerived<'_> {
    fn schema_revision(&self) -> Option<&str> {
        Some(&self.package.schema.revision)
    }
    fn supports(&self, class: &str, feature: &str) -> bool {
        PackageDerived(self.package).supports(class, feature)
    }
    fn explicit_set_state(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> Result<bool, String> {
        PackageDerived(self.package).explicit_set_state(store, id, feature, budget)
    }
    fn read(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> Result<Value, String> {
        if self.getters.entries.is_empty() {
            return PackageDerived(self.package).read(store, id, feature, budget);
        }
        budget.charge()?;
        let actual = store.class(id)?.ok_or("Missing source getter object")?;
        let Some(index) = self
            .getters
            .dispatch
            .get(&actual)
            .and_then(|members| members.get(feature))
        else {
            return PackageDerived(self.package).read(store, id, feature, budget);
        };
        let getter = &self.getters.entries[*index];
        let work = BorrowedBindingWork::bind(budget);
        let reads = SourceModelReads::new(store, &self.package.schema);
        let value = scope_runtime::evaluate_getter(
            getter,
            &ExecutionContext::new(self.bundle),
            &self.package.schema,
            &reads,
            id,
            self.limits,
            &work,
        )
        .map_err(|error| error.to_string())?;
        work.observer().work(1)?;
        let member = self
            .package
            .schema
            .feature_by_id(&actual, getter.member())?;
        let value = getter_result(value, member.feature, &work.observer())?;
        work.observer().checkpoint()?;
        Ok(value)
    }
}

/// Model identities were checked on the original live read authority by the
/// shared runner. Extract their owned wire strings without copying the names.
pub(crate) fn getter_result(
    value: ValueWire,
    feature: &Feature,
    observation: &crate::definitions::conversion::InvocationContext<'_>,
) -> Result<Value, String> {
    if !feature.reference {
        return scalar_result(value, feature);
    }
    fn identity(value: ValueWire) -> Result<Value, String> {
        let DataWire::Model(mut payload) = value.data else {
            return Err("Reference getter returned a non-model value".into());
        };
        crate::model_reference::reference_identity(&payload)?;
        payload
            .as_object_mut()
            .and_then(|fields| fields.remove("element"))
            .ok_or_else(|| "Reference getter lost its checked identity".into())
    }
    if feature.upper == 1 {
        if feature.lower == 0 {
            return match value.data {
                DataWire::None => Ok(Value::Null),
                DataWire::Some(value) => identity(*value),
                _ => Err("Nullable reference getter returned a non-optional value".into()),
            };
        }
        return identity(value);
    }
    let DataWire::Collection(values) = value.data else {
        return Err("Reference collection getter returned a non-collection value".into());
    };
    observation.retain_storage(
        values
            .len()
            .checked_mul(crate::logical_size!(Value, 32))
            .ok_or("Reference getter result allocation size overflow")?,
    )?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(values.len())
        .map_err(|_| "Reference getter result allocation failed")?;
    for value in values {
        observation.work(1)?;
        result.push(identity(value)?);
    }
    Ok(Value::Array(result))
}

/// Preserve storage's canonical scalar codecs, including tagged Int64 transport.
/// Reference results use the shared live authority and the projection above;
/// custom datatypes and assignment history remain explicitly unsupported here.
pub(crate) fn scalar_result(value: ValueWire, feature: &Feature) -> Result<Value, String> {
    let value = if feature.lower == 0 {
        match value.data {
            DataWire::None => return Ok(Value::Null),
            DataWire::Some(value) => *value,
            _ => return Err("Nullable source getter returned a non-optional value".into()),
        }
    } else {
        value
    };
    let DataWire::Scalar(value) = value.data else {
        return Err("Source getter requires a scalar value".into());
    };
    let (kind, codec) = match feature.target.as_str() {
        "EString" => (Scalar::String, Builtin::EString),
        "EBoolean" => (Scalar::Bool, Builtin::EBoolean),
        "ELong" => (Scalar::Int64, Builtin::ELong),
        _ => return Err("Unsupported source getter storage codec".into()),
    };
    if value.ty != ValueType::Scalar(kind) {
        return Err("Source getter scalar differs from its checked feature type".into());
    }
    let value = value.payload;
    codec.validate(&value)?;
    Ok(value)
}
