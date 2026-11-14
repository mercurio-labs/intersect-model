//! Handwritten intrinsic model derivations shared by direct and text consumers.
pub use intersect_model::core::query::*;
use intersect_model::core::storage::{ComputedEdit, ModelRead, Query, SettingEdit, SettingValues};
use intersect_model::definitions::{
    model_definition::ModelDefinitionView,
    scalars::Builtin,
    schema::{Derivation, Expression as SemanticExpression, Package},
};
use serde_json::{Value, json};

pub struct ModelDerived<'a>(pub ModelDefinitionView<'a>);
impl DerivedValues for ModelDerived<'_> {
    fn schema_revision(&self) -> Option<&str> {
        Some(&self.0.schema.revision)
    }
    fn explicit_set_state(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> Result<bool, String> {
        budget.charge()?;
        let class = store.class(id)?.ok_or("Missing setting-state object")?;
        let rule = self
            .0
            .semantics
            .derived
            .iter()
            .find(|rule| rule.feature == feature && self.0.schema.conforms(&class, &rule.class))
            .ok_or("No setting-state declaration")?;
        let SemanticExpression::Stored { feature, .. } = &rule.expression else {
            return Err("Derived unsettable state requires a setting-state declaration".into());
        };
        intersect_model::core::storage::is_explicitly_set(store, self.0.schema, id, feature)
    }
    fn supports(&self, class: &str, feature: &str) -> bool {
        self.0
            .semantics
            .derived
            .iter()
            .any(|r| r.feature == feature && self.0.schema.conforms(class, &r.class))
    }
    fn read(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> Result<Value, String> {
        budget.charge()?;
        let class = store.class(id)?.ok_or("Missing derived query object")?;
        let rule = self
            .0
            .semantics
            .derived
            .iter()
            .find(|r| r.feature == feature && self.0.schema.conforms(&class, &r.class))
            .ok_or_else(|| format!("No derived provider for {class}.{feature}"))?;
        evaluate_model_derivation(self.0, store, id, &class, rule, budget)
    }
}
impl SettingValues for ModelDerived<'_> {
    fn plan(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        edit: &ComputedEdit,
        budget: &mut QueryBudget,
    ) -> Result<Vec<SettingEdit>, String> {
        budget.charge()?;
        let class = store.class(id)?.ok_or("Missing computed setting object")?;
        let rule = self
            .0
            .semantics
            .derived
            .iter()
            .find(|rule| rule.feature == feature && self.0.schema.conforms(&class, &rule.class))
            .ok_or("No computed setting declaration")?;
        let SemanticExpression::Stored {
            feature,
            writable: true,
        } = &rule.expression
        else {
            return Err(
                "Cannot write a derived feature without a writable setting declaration".into(),
            );
        };
        Ok(vec![match edit {
            ComputedEdit::Set(value) => SettingEdit::Set {
                id: id.into(),
                feature: feature.clone(),
                value: value.clone(),
            },
            ComputedEdit::Append(value) => {
                let previous = store
                    .value(id, feature)?
                    .ok_or("Missing stored collection")?;
                let values = previous
                    .as_array()
                    .ok_or("Stored setting is not a collection")?;
                let mut next = Vec::new();
                for value in values {
                    budget.charge()?;
                    next.push(value.clone());
                }
                budget.charge()?;
                next.push(value.clone());
                SettingEdit::Set {
                    id: id.into(),
                    feature: feature.clone(),
                    value: Value::Array(next),
                }
            }
            ComputedEdit::Unset => SettingEdit::Unset {
                id: id.into(),
                feature: feature.clone(),
            },
        }])
    }
}

#[doc(hidden)]
pub fn evaluate_model_derivation(
    package: ModelDefinitionView<'_>,
    store: &dyn ModelRead,
    id: &str,
    class: &str,
    rule: &Derivation,
    budget: &mut QueryBudget,
) -> Result<Value, String> {
    budget.charge()?;
    match &rule.expression {
        SemanticExpression::Source { .. } => {
            Err("Source derived function requires freshly compiled getter services".into())
        }
        SemanticExpression::External { function, .. } => Err(format!(
            "External derived function {function} requires an implementation registry"
        )),
        SemanticExpression::Stored { feature, .. } => store
            .value(id, feature)?
            .ok_or_else(|| format!("Missing stored projection value {id}.{feature}")),
        SemanticExpression::Count { feature } => {
            let count = store.value_len(id, feature)?.unwrap_or(0);
            let fields = package.schema.features(class)?;
            let output = fields
                .get(&rule.feature)
                .ok_or("Missing count output feature")?;
            if output.reference || output.datatype.is_some() {
                return Err("Count output requires a builtin integer attribute".into());
            }
            let codec = match output.target.as_str() {
                "EInt" => Builtin::EInt,
                "ELong" => Builtin::ELong,
                _ => return Err("Count output requires EInt or ELong".into()),
            };
            codec.parse(&count.to_string())
        }
        SemanticExpression::NoIncoming { class, feature } => {
            for owner in store.query(Query::Value(feature, &json!(id)))? {
                budget.charge()?;
                let actual = store.class(&owner)?.ok_or("Missing indexed owner")?;
                if package.schema.conforms(&actual, class) {
                    return Ok(json!(false));
                }
            }
            Ok(json!(true))
        }
    }
}

/// Compatibility adapter over the shared model implementation. Text consumers
/// borrow the same ModelDefinitionView from their own package type.
pub struct PackageDerived<'a, P: ?Sized = Package>(pub &'a P);
impl<'a, P: ?Sized> DerivedValues for PackageDerived<'a, P>
where
    &'a P: Into<crate::definitions::model_definition::ModelDefinitionView<'a>>,
{
    fn schema_revision(&self) -> Option<&str> {
        Some(self.0.into().schema().revision.as_str())
    }
    fn explicit_set_state(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> Result<bool, String> {
        ModelDerived(self.0.into()).explicit_set_state(store, id, feature, budget)
    }
    fn supports(&self, class: &str, feature: &str) -> bool {
        ModelDerived(self.0.into()).supports(class, feature)
    }
    fn read(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> Result<Value, String> {
        ModelDerived(self.0.into()).read(store, id, feature, budget)
    }
}
impl<'a, P: ?Sized> SettingValues for PackageDerived<'a, P>
where
    &'a P: Into<crate::definitions::model_definition::ModelDefinitionView<'a>>,
{
    fn plan(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        edit: &ComputedEdit,
        budget: &mut QueryBudget,
    ) -> Result<Vec<SettingEdit>, String> {
        ModelDerived(self.0.into()).plan(store, id, feature, edit, budget)
    }
}
#[doc(hidden)]
pub fn evaluate_derivation<'a>(
    package: impl Into<crate::definitions::model_definition::ModelDefinitionView<'a>>,
    store: &dyn ModelRead,
    id: &str,
    class: &str,
    rule: &Derivation,
    budget: &mut QueryBudget,
) -> Result<Value, String> {
    evaluate_model_derivation(package.into(), store, id, class, rule, budget)
}
