//! Handwritten model-rule orchestration over existing model/query services.
//! Source checking and provider installation belong to the caller. This moves
//! the existing Xtext-independent walk; it adds no evaluator or dispatch policy.
use crate::core::storage::{self, ModelRead, Query};
use crate::definitions::schema::Diagnostic;
use crate::query;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Recompute derived outputs and append existing model-rule diagnostics.
///
/// Derived output is cleared first; a later provider/storage failure retains
/// the already-computed prefix. Existing diagnostics are preserved. Callers
/// retain atomic publication/rollback responsibilities. The legacy unbounded
/// model walk and MAX query budget remain explicit, unqualified S09 boundaries.
pub fn apply_with_provider<'a>(
    package: impl Into<crate::definitions::model_definition::ModelDefinitionView<'a>>,
    store: &impl ModelRead,
    derived_output: &mut BTreeMap<String, BTreeMap<String, Value>>,
    diagnostics_output: &mut Vec<Diagnostic>,
    derived: &dyn query::DerivedValues,
) -> Result<(), String> {
    apply_model_with_provider(
        package.into(),
        store,
        derived_output,
        diagnostics_output,
        derived,
    )
}
/// The same model-rule walk over borrowed model metadata, without text packaging.
pub fn apply_model_with_provider(
    package: crate::definitions::model_definition::ModelDefinitionView<'_>,
    store: &impl ModelRead,
    derived_output: &mut BTreeMap<String, BTreeMap<String, Value>>,
    diagnostics_output: &mut Vec<Diagnostic>,
    derived: &dyn query::DerivedValues,
) -> Result<(), String> {
    derived_output.clear();
    // Preserve the existing native model-rule policy. External adapters enforce
    // their own shared, sticky registry budget (100,000 units in the web host).
    let mut budget = intersect_model::core::query::QueryBudget::new(usize::MAX);
    storage::visit_views(store, |element| {
        for rule in &package.semantics.derived {
            if !package.schema.conforms(element.class(), &rule.class) {
                continue;
            }
            let value = derived.read(store, element.id(), &rule.feature, &mut budget)?;
            derived_output
                .entry(element.id().to_owned())
                .or_default()
                .insert(rule.feature.clone(), value);
        }
        Ok(())
    })?;
    for rule in &package.semantics.validations {
        let mut targets = BTreeSet::new();
        for class in package
            .schema
            .classes
            .keys()
            .filter(|c| package.schema.conforms(c, &rule.class))
        {
            for id in store.query(Query::Class(class))? {
                if !store.inspect(&id, &mut |element| {
                    if let Some(value) = element.value(&rule.feature) {
                        if let Some(list) = value.collection() {
                            for value in list.iter() {
                                if let Some(id) = value.as_str() {
                                    targets.insert(id.to_owned());
                                }
                            }
                        } else if let Some(id) = value.as_str() {
                            targets.insert(id.to_owned());
                        }
                    }
                    Ok(())
                })? {
                    return Err("Missing indexed instance".into());
                }
            }
        }
        for target in targets {
            let mut incoming = Vec::new();
            for id in store.query(Query::Value(&rule.feature, &json!(target)))? {
                if !store.inspect(&id, &mut |e| {
                    if package.schema.conforms(e.class(), &rule.class)
                        && let Some(value) = e.value(&rule.feature)
                    {
                        let count = value.collection().map_or_else(
                            || usize::from(value.as_str() == Some(&target)),
                            |a| a.iter().filter(|v| v.as_str() == Some(&target)).count(),
                        );
                        for _ in 0..count {
                            incoming.push((e.resource().to_owned(), e.start(), e.end()));
                        }
                    }
                    Ok(())
                })? {
                    return Err("Missing indexed owner".into());
                }
            }
            if incoming.len() > rule.max_incoming {
                for (uri, start, end) in incoming {
                    diagnostics_output.push(Diagnostic::new(&uri, start, end, &rule.message));
                }
            }
        }
    }
    Ok(())
}
