//! Handwritten model preparation and physical metadata; text wrappers belong to Xtext.
#[path = "model_preparation.rs"]
mod model_preparation;
use super::{bundle::DefinitionBundle, schema::Schema};
pub use model_preparation::{
    BuiltinRuleCoverage, CheckedModelOrigin, PreparedModelDefinition, PreparedPersistenceContext,
    PreparedPhysicalMetadata, SemanticRequirement, SemanticRequirementClass,
    SemanticRequirementClasses, SemanticRequirements,
};

// Borrowed completion provenance from one actual OwnedModelDefinition.
// Fresh production follows checked structural/behavior compilation and an exact
// source-allocation transfer. The explicit native-host model cache loader checks
// retained source identities and discards optional completed physical inputs.
// Origin correspondence is separate: native migration tokens or replay bind the
// original and converted source worlds before source-aware preparation.
// No metadata flag or public raw-schema constructor creates these private views.
// Schema and source bundle are borrowed from the same stable model owner.
pub(crate) struct CompletedCompiledSources<'owner> {
    schema: &'owner Schema,
    bundle: &'owner DefinitionBundle,
    physical_inputs: Option<&'owner super::compiler::CompletedLoweredInputs>,
}
impl<'owner> CompletedCompiledSources<'owner> {
    pub(crate) fn from_owned_model(
        owner: &'owner super::model_definition::OwnedModelDefinition,
    ) -> Self {
        Self {
            schema: owner.schema(),
            bundle: owner.sources(),
            physical_inputs: owner.completed_physical_inputs(),
        }
    }

    pub(crate) fn belongs_to(&self, schema: &Schema, bundle: &DefinitionBundle) -> bool {
        std::ptr::eq(self.schema, schema) && std::ptr::eq(self.bundle, bundle)
    }
    pub(crate) fn physical_fields_observed(
        &self,
        schema: &Schema,
        receiver: &str,
        observation: &super::conversion::InvocationContext<'_>,
    ) -> Result<Option<std::sync::Arc<super::schema::PreparedFeatureInputs>>, String> {
        observation.work(2)?;
        if !std::ptr::eq(self.schema, schema) {
            return Err("Completed compiler field data belongs to another actual schema".into());
        }
        if let Some(inputs) = self.physical_inputs {
            return inputs.fields_observed(receiver, observation);
        }
        let _ = receiver;
        Ok(None)
    }
    pub(crate) fn physical_cache_observed(
        &self,
        schema: &Schema,
        observation: &super::conversion::InvocationContext<'_>,
    ) -> Result<Option<super::schema::SharedFeatureInputCache>, String> {
        observation.work(2)?;
        if !std::ptr::eq(self.schema, schema) {
            return Err("Completed compiler input cache belongs to another actual schema".into());
        }
        if let Some(inputs) = self.physical_inputs {
            return inputs.cache_observed(observation);
        }
        Ok(None)
    }
}
