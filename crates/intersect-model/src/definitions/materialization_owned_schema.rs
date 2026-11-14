//! Attempt-local owned Schema assembly; source initialization remains synchronous.
use super::*;
struct Owner<'bundle> {
    bundle: &'bundle DefinitionBundle,
    resolver: Resolver,
}
struct Cursor<'source> {
    state: MaterializationContext<'source>,
    preparation: Option<PreparedMaterialization<'source, String>>,
    storage: OwnedAlternatives,
}
self_cell::self_cell!(
    struct Holder<'bundle> {
        owner: Owner<'bundle>,
        #[covariant]
        dependent: Cursor,
    }
);
pub(crate) struct OwnedSchemaPreparation<'bundle> {
    holder: Holder<'bundle>,
    fallback: SourceSpan,
    output: Option<Schema>,
    failure: Option<crate::definitions::ecore::ObservedImportError>,
}
const STATE_BYTES: usize = 8192;
const _: () = assert!(
    std::mem::size_of::<Owner<'static>>()
        + std::mem::size_of::<Cursor<'static>>()
        + std::mem::size_of::<OwnedSchemaPreparation<'static>>()
        + 128
        <= STATE_BYTES
);
impl<'bundle> OwnedSchemaPreparation<'bundle> {
    #[cfg(test)]
    pub(crate) fn begin(
        bundle: &'bundle DefinitionBundle,
        requested: &[RuntimeType],
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        resources: ResourceSet,
        operation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Self, crate::definitions::ecore::ObservedImportError> {
        Self::begin_with_operation_cache(bundle, requested, conversions, resources, None, operation)
    }
    /// `begin`, sharing resolved runtime operation sets through `operations`
    /// (one model-type context's cache over these exact `resources`).
    pub(crate) fn begin_with_operation_cache(
        bundle: &'bundle DefinitionBundle,
        requested: &[RuntimeType],
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        resources: ResourceSet,
        operations: Option<crate::definitions::generic_resolution::RuntimeOperationCache>,
        operation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Self, crate::definitions::ecore::ObservedImportError> {
        operation
            .checkpoint()
            .map_err(crate::definitions::ecore::ObservedImportError::Observation)?;
        operation
            .retain_storage(STATE_BYTES)
            .map_err(crate::definitions::ecore::ObservedImportError::Observation)?;
        let observation = crate::definitions::ecore::ParserObservation::new(operation);
        // Keep the original canonical/request/source/registry/inventory ordering
        // and trust the exact caller-admitted resource policy; never re-clamp it.
        let InitializedMaterialization {
            resolver,
            inventory,
            fallback,
        } = observation.finish(initialize_materialization::<OwnedAlternatives>(
            bundle,
            requested,
            conversions,
            Some(resources),
            &observation,
        ))?;
        let resolver = match operations {
            Some(cache) => resolver.with_runtime_operation_cache(cache),
            None => resolver,
        };
        let holder = observation.finish(Holder::try_new(Owner { bundle, resolver }, |owner| {
            let mut state =
                new_materialization_context(owner.bundle, &owner.resolver, &observation)?;
            let preparation =
                PreparedMaterialization::begin(&mut state, inventory, requested, &fallback)?;
            Ok::<_, ImportError>(Cursor {
                state,
                preparation: Some(preparation),
                storage: OwnedAlternatives,
            })
        }))?;
        Ok(Self {
            holder,
            fallback,
            output: None,
            failure: None,
        })
    }
    pub(crate) fn advance(
        &mut self,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        operation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<bool, crate::definitions::ecore::ObservedImportError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.advance_inner(conversions, operation);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
    fn advance_inner(
        &mut self,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        operation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<bool, crate::definitions::ecore::ObservedImportError> {
        operation
            .checkpoint()
            .map_err(crate::definitions::ecore::ObservedImportError::Observation)?;
        if self.output.is_some() {
            return Ok(true);
        }
        let observation = crate::definitions::ecore::ParserObservation::new(operation);
        let fallback = &self.fallback;
        let complete = observation.finish(self.holder.with_dependent_mut(|_, cursor| {
            cursor
                .preparation
                .as_mut()
                .ok_or_else(|| {
                    ImportError::new(
                        ErrorKind::InvalidDefinition,
                        fallback,
                        "Missing owned schema cursor",
                    )
                })?
                .advance(
                    &mut cursor.state,
                    &mut cursor.storage,
                    conversions,
                    fallback,
                    &observation,
                )
        }))?;
        if complete {
            // The cursor is consumed inside its stable owner, never through an
            // external borrowed schema report. A fresh cancellation check precedes
            // every complete result transfer.
            operation
                .checkpoint()
                .map_err(crate::definitions::ecore::ObservedImportError::Observation)?;
            let schema = observation.finish(self.holder.with_dependent_mut(|_, cursor| {
                cursor
                    .preparation
                    .take()
                    .ok_or_else(|| {
                        ImportError::new(
                            ErrorKind::InvalidDefinition,
                            fallback,
                            "Missing completed owned schema",
                        )
                    })?
                    .finish(fallback)
            }))?;
            self.output = Some(schema);
        }
        Ok(self.output.is_some())
    }
    /// Transfer the completed compiler result into immutable type authority.
    /// Publication already validated the final runtime type metadata (no
    /// mutation follows it), so the authority is issued without re-validation.
    pub(crate) fn finish_checked(
        self,
        operation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<
        crate::definitions::schema::CheckedRuntimeSchema,
        crate::definitions::ecore::ObservedImportError,
    > {
        let fallback = self.fallback.clone();
        let schema = self.finish()?;
        crate::definitions::schema::CheckedRuntimeSchema::from_checked_compiler_output(
            schema, operation,
        )
        .map_err(|error| match error {
            crate::definitions::schema::PreparationError::Observation(error) => {
                crate::definitions::ecore::ObservedImportError::Observation(error)
            }
            crate::definitions::schema::PreparationError::Schema(message) => {
                crate::definitions::ecore::ObservedImportError::Import(ImportError::new(
                    ErrorKind::InvalidDefinition,
                    &fallback,
                    message,
                ))
            }
        })
    }
    pub(crate) fn finish(
        mut self,
    ) -> Result<Schema, crate::definitions::ecore::ObservedImportError> {
        if let Some(error) = self.failure.take() {
            return Err(error);
        }
        self.output.take().ok_or_else(|| {
            crate::definitions::ecore::ObservedImportError::Import(ImportError::new(
                ErrorKind::InvalidDefinition,
                &self.fallback,
                "Owned schema preparation is incomplete",
            ))
        })
    }
}
