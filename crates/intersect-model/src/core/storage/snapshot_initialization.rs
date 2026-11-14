//! Nonserializable evidence of checked fresh readonly decode, never metadata authority.
use super::*;
use crate::core::{fingerprint::digest, model::Model, query::QueryBudget};
use crate::definitions::initialization::ClassInitializationContract;

#[derive(Debug)]
pub(crate) struct VerifiedReadonlyInitialization {
    pub(super) object: String,
    pub(super) resource: String,
    pub(super) contract: ClassInitializationContract,
    pub(super) values: Vec<(String, Value)>,
}
#[derive(Debug)]
pub(crate) struct ReadonlyDecodeProof {
    schema: String,
    model: String,
    rows: Vec<VerifiedReadonlyInitialization>,
}
impl ReadonlyDecodeProof {
    #[cfg(feature = "xmi")]
    pub(crate) fn seal(
        schema: &Schema,
        model: &Model,
        rows: Vec<VerifiedReadonlyInitialization>,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Option<Self>> {
        if rows.is_empty() {
            return Ok(None);
        }
        observation.retain_storage(256)?;
        let view = ModelView::new_observed(model, observation)?;
        for row in &rows {
            row.check_actual(&view, observation)?;
        }
        let (schema, model) = Self::fingerprints(schema, model, observation)?;
        Ok(Some(Self {
            schema,
            model,
            rows,
        }))
    }
    fn fingerprints(
        schema: &Schema,
        model: &Model,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<(String, String)> {
        observation.checkpoint()?;
        // Reuse the existing resource fingerprint algorithm, with explicit
        // traversal work and temporary digest carriers. This is not an RSS claim.
        for row in &model.elements {
            observation.work(1)?;
            observation.work(row.values.len())?;
        }
        observation.retain_storage(128)?;
        let schema = digest(schema)?;
        let model = digest(model)?;
        observation.checkpoint()?;
        Ok((schema, model))
    }
    pub(crate) fn check_model(
        &self,
        schema: &Schema,
        model: &Model,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<()> {
        let (schema, model) = Self::fingerprints(schema, model, observation)?;
        if schema != self.schema || model != self.model {
            return Err(
                "Opaque readonly decode proof does not match schema/candidate fingerprint".into(),
            );
        }
        Ok(())
    }
    pub(crate) fn check_selection(&self, selection: &BTreeSet<String>) -> StorageResult<()> {
        if self
            .rows
            .iter()
            .any(|row| !selection.contains(&row.resource))
        {
            return Err("Readonly decode proof covers an unselected resource".into());
        }
        Ok(())
    }
    pub(crate) fn covers(&self, object: &str, name: &str) -> bool {
        self.rows
            .iter()
            .any(|row| row.object == object && row.values.iter().any(|(field, _)| field == name))
    }
    pub(crate) fn revalidate(
        &self,
        store: &dyn ModelRead,
        schema: &Schema,
        selector: &dyn SnapshotInitializationSelector,
        budget: &mut QueryBudget,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<()> {
        let mut bindings = Vec::new();
        for row in &self.rows {
            budget.charge()?;
            row.check_actual(store, observation)?;
            let binding = selector
                .revalidate(store, schema, &row.object, budget)?
                .ok_or("Required current snapshot initializer is not installed")?;
            if binding.expected != &row.contract {
                return Err(
                    "Current snapshot initializer does not match opaque checked decode contract"
                        .into(),
                );
            }
            let context = InitializationContext {
                schema,
                expected: binding.expected,
                sources: binding.sources,
                provider: binding.provider,
                conversions: binding.conversions,
            };
            context.validate_snapshot_binding(store, &row.object, budget)?;
            observation.retain_storage(128)?;
            bindings.push(binding);
        }
        observation.checkpoint()?;
        for binding in bindings {
            budget.charge()?;
            if binding
                .provider
                .ok_or("Required current snapshot initializer is not installed")?
                .contract()
                != binding.expected
            {
                return Err(
                    "Current snapshot initializer contract changed after revalidation".into(),
                );
            }
        }
        Ok(())
    }
}
impl VerifiedReadonlyInitialization {
    fn check_actual(
        &self,
        store: &dyn ModelRead,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<()> {
        observation.checkpoint()?;
        let mut found = false;
        store.inspect(&self.object, &mut |object| {
            found = true;
            if object.id() != self.object
                || object.resource() != self.resource
                || object.class() != self.contract.class
                || object.type_identity() != self.contract.class_identity
            {
                return Err("Opaque readonly decode proof receiver changed".into());
            }
            let receipt = object
                .initialization()
                .ok_or("Opaque readonly decode proof lost its checked receipt")?;
            if receipt.object != self.object || receipt.contract != self.contract {
                return Err("Opaque readonly decode proof receipt changed".into());
            }
            for (name, value) in &self.values {
                if !object
                    .value(name)
                    .is_some_and(|actual| actual.equals_json(value))
                {
                    return Err("Opaque readonly decode proof value changed".into());
                }
            }
            Ok(())
        })?;
        if !found {
            return Err("Opaque readonly decode proof receiver missing".into());
        }
        Ok(())
    }
}
