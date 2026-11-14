//! Optional immutable URI-resolution inputs for one exact resolver context.
//! Only successful address/fragment selections are retained. Classifier, scope,
//! signature and provider checks remain fresh at each semantic use.
use super::{Resolver, error};
use crate::definitions::{
    bundle::{SourceId, SourceSpan},
    ecore::{ImportError, ParserObservation},
    resources::DeclarationId,
};
use std::collections::BTreeMap;
type Selections = BTreeMap<SourceId, BTreeMap<String, DeclarationId>>;
#[derive(Clone, Debug, Default)]
pub(super) struct ReferenceInputs {
    standard: Selections,
    resources: Selections,
}
impl Resolver {
    pub(crate) fn enable_source_reference_inputs(
        &self,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        let mut inputs = self
            .prepared_reference_inputs
            .try_borrow_mut()
            .map_err(|_| {
                error(
                    &super::fallback(&DeclarationId {
                        source: self.standard_id.clone(),
                        path: Vec::new(),
                    }),
                    "Reference input context is already borrowed",
                )
            })?;
        if inputs.is_none() {
            observation.reserve(256, &self.standard_id)?;
            *inputs = Some(Box::new(ReferenceInputs::default()));
        }
        Ok(())
    }
    pub(super) fn reference_input_context(
        &self,
        standard: bool,
        source: &SourceId,
        href: &str,
        span: &SourceSpan,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<DeclarationId, ImportError> {
        let enabled = {
            let inputs = self
                .prepared_reference_inputs
                .try_borrow()
                .map_err(|_| error(span, "Reference input context is already borrowed"))?;
            if let Some(inputs) = inputs.as_ref() {
                observation.work(
                    1 + href.len().div_ceil(256) + source.uri.len().div_ceil(256),
                    source,
                )?;
                let selections = if standard {
                    &inputs.standard
                } else {
                    &inputs.resources
                };
                if let Some(target) = selections.get(source).and_then(|rows| rows.get(href)) {
                    // The result remains an owned public identity; no sharing is
                    // claimed for this copy. Admission precedes the clone.
                    observation.serialized(target, source)?;
                    return Ok(target.clone());
                }
                true
            } else {
                false
            }
        };
        let resources = if standard {
            &self.standard
        } else {
            &self.resources
        };
        let target = resources.resolve_context(source, href, observation)?;
        if enabled {
            let mut inputs = self
                .prepared_reference_inputs
                .try_borrow_mut()
                .map_err(|_| error(span, "Reference input context is already borrowed"))?;
            let inputs = inputs
                .as_mut()
                .ok_or_else(|| error(span, "Missing reference input context"))?;
            let selections = if standard {
                &mut inputs.standard
            } else {
                &mut inputs.resources
            };
            let new_source = !selections.contains_key(source);
            if new_source {
                observation.serialized(source, source)?;
                observation.reserve(128, source)?;
            }
            observation.string(href.len(), source)?;
            observation.reserve(128, source)?;
            observation.serialized(&target, source)?;
            // Do not retain a partial lookup if any preflight was rejected.
            if new_source {
                selections.insert(source.clone(), BTreeMap::new());
            }
            selections
                .get_mut(source)
                .ok_or_else(|| error(span, "Missing reference source selections"))?
                .insert(href.to_owned(), target.clone());
        }
        Ok(target)
    }
}
