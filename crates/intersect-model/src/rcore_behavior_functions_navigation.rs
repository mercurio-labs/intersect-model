//! Handwritten cooperative stored collection reads over admitted model contexts.
use super::{
    collections::CollectionOutput,
    machine::{CallSite, State},
    values::{Data, Id},
    *,
};
use intersect_model::definitions::rcore_behavior::ir_v2::TypeId;

pub(super) struct StoredRead {
    qualification: Option<intersect_model::core::storage::StoredFeaturePreparation>,
    selected: Option<intersect_model::core::storage::StoredFeatureSelection>,
    domain_preparation: Option<crate::model_reference::RuntimeReferenceDomainPreparation>,
    digest: String,
    element: TypeId,
    receiver: Id,
    ty: TypeId,
    site: CallSite,
    cursor: usize,
    output: CollectionOutput,
}
#[cfg(test)]
impl StoredRead {
    pub(super) fn preparing_domain(&self) -> bool {
        self.domain_preparation.is_some()
    }
}
impl<I: super::inputs::InputFamily> State<'_, '_, '_, I> {
    fn observe_stored(
        &mut self,
        receiver: Id,
        site: CallSite,
        cursor: Option<usize>,
        value: Option<&serde_json::Value>,
    ) -> Result<()> {
        if !self.data.observation.enabled() {
            return Ok(());
        }
        let origin = self.node(receiver)?.origin;
        let image = self.image;
        let span = image
            .call_span(site)
            .ok_or_else(|| self.invalid("Missing observed read source", None))?;
        let member = image
            .stored_declaration(site)
            .ok_or_else(|| self.invalid("Missing observed read declaration", Some(span)))?;
        let payload = self
            .data
            .nodes
            .get(origin)
            .ok_or_else(|| self.invalid("Missing observed receiver", Some(span)))?;
        let Data::Model(payload) = &payload.data else {
            return Err(self.invalid("Observed receiver is not a model", Some(span)));
        };
        let context = payload
            .get("context")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| self.invalid("Missing observed context", Some(span)))?;
        let element = payload
            .get("element")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| self.invalid("Missing observed element", Some(span)))?;
        self.data.observation.read(super::observation::ReadInput {
            kind: if cursor.is_some() {
                ReadKind::StoredPage
            } else {
                ReadKind::StoredSingular
            },
            context,
            element,
            member,
            source: span,
            cursor,
            source_type: None,
            matched: None,
            value,
        });
        Ok(())
    }
    pub(super) fn start_stored_read(
        &mut self,
        receiver: Id,
        ty: TypeId,
        site: CallSite,
    ) -> Result<()> {
        let span = self
            .image
            .call_span(site)
            .ok_or_else(|| self.invalid("Missing stored read source", None))?;
        if I::BORROWED
            && !I::source_scope(&self.data.domains)
            && !matches!(
                self.type_node(ty)?,
                intersect_model::definitions::rcore_behavior::ir_v2::TypeNode::Scalar(_)
            )
        {
            return Err(self.error(
                ExecutionErrorKind::Unsupported,
                "Borrowed operation supports source-declared singular scalar stored reads",
                Some(span),
            ));
        }
        self.collection_storage(128, span)?;
        let payload = self.admit_stored_receiver(receiver, span)?;
        let digest = crate::model_reference::reference_identity(&payload)
            .map_err(|e| self.invalid(&e, Some(span)))?
            .0
            .to_owned();
        self.collection_storage(digest.len(), span)?;
        let element = match self.type_node(ty)? {
            intersect_model::definitions::rcore_behavior::ir_v2::TypeNode::Many {
                element, ..
            } => *element,
            intersect_model::definitions::rcore_behavior::ir_v2::TypeNode::Option(inner) => *inner,
            _ => ty,
        };
        let read = StoredRead {
            qualification: None,
            selected: None,
            domain_preparation: None,
            digest,
            element,
            receiver,
            ty,
            site,
            cursor: 0,
            output: CollectionOutput::new(ty),
        };
        let Some(frame) = self.data.frames.last_mut() else {
            return Err(self.invalid("Missing stored read frame", Some(span)));
        };
        frame.stored_read = Some(read);
        Ok(())
    }
    pub(super) fn advance_stored_read(&mut self) -> Result<()> {
        let mut read = self
            .data
            .frames
            .last_mut()
            .and_then(|f| f.stored_read.take())
            .ok_or_else(|| self.invalid("Missing stored read continuation", None))?;
        let span = self
            .image
            .call_span(read.site)
            .ok_or_else(|| self.invalid("Missing stored read source", None))?;
        if I::advance_stored_target(self, span)? {
            let Some(frame) = self.data.frames.last_mut() else {
                return Err(self.invalid("Missing stored read frame", Some(span)));
            };
            frame.stored_read = Some(read);
            return Ok(());
        }
        if read.domain_preparation.is_some() {
            self.advance_reference_domain(
                &mut read.domain_preparation,
                read.element,
                &read.digest,
                Some(span),
            )?;
            let Some(frame) = self.data.frames.last_mut() else {
                return Err(self.invalid("Missing stored read frame", Some(span)));
            };
            frame.stored_read = Some(read);
            return Ok(());
        }
        self.step(span)?;
        let many = matches!(
            self.type_node(read.ty)?,
            intersect_model::definitions::rcore_behavior::ir_v2::TypeNode::Many { .. }
        );
        let member = self
            .image
            .stored_declaration(read.site)
            .ok_or_else(|| self.invalid("Missing stored member declaration", Some(span)))?;
        let direct = I::direct_stored(
            self,
            read.receiver,
            member,
            many.then_some(read.cursor),
            span,
        )?;
        if direct.is_none() && read.selected.is_none() {
            if let Some(pending) = &mut read.qualification {
                if self.advance_stored_projection(read.receiver, pending, span)? {
                    read.selected = Some(
                        read.qualification
                            .take()
                            .ok_or_else(|| {
                                self.invalid("Missing stored qualification", Some(span))
                            })?
                            .finish()
                            .map_err(|e| self.invalid(&e, Some(span)))?,
                    );
                }
            } else {
                let declaration = self
                    .image
                    .stored_declaration(read.site)
                    .ok_or_else(|| self.invalid("Missing stored member declaration", Some(span)))?;
                read.qualification =
                    Some(self.begin_stored_projection(read.receiver, declaration, span)?);
            }
            let Some(frame) = self.data.frames.last_mut() else {
                return Err(self.invalid("Missing stored read frame", Some(span)));
            };
            frame.stored_read = Some(read);
            return Ok(());
        }
        let selected = read.selected.as_ref();
        if !matches!(
            self.type_node(read.ty)?,
            intersect_model::definitions::rcore_behavior::ir_v2::TypeNode::Many { .. }
        ) {
            let observation = self.invocation_observation(None)?;
            let raw = match direct {
                Some(raw) => {
                    raw.ok_or_else(|| self.invalid("Missing singular scope read", Some(span)))?
                }
                None => self
                    .selected_stored_projection(
                        read.receiver,
                        selected
                            .ok_or_else(|| self.invalid("Missing stored selection", Some(span)))?,
                        span,
                    )?
                    .singular_observed(&observation)
                    .map_err(|e| match self.check_provider_status(Some(span)) {
                        Err(error) => error,
                        Ok(()) => self.invalid(&e, Some(span)),
                    })?,
            };
            self.check_provider_status(Some(span))?;
            self.observe_stored(read.receiver, read.site, None, Some(&raw))?;
            if !raw.is_null()
                && matches!(
                    self.type_node(read.element)?,
                    intersect_model::definitions::rcore_behavior::ir_v2::TypeNode::Model(_)
                )
            {
                let preparing = if I::BORROWED {
                    let object = raw.as_str().ok_or_else(|| {
                        self.invalid("Stored reference requires an identity", Some(span))
                    })?;
                    I::begin_stored_target(self, read.element, &read.digest, object, span)?
                } else {
                    read.domain_preparation =
                        self.prepare_reference_domain(read.element, &read.digest, Some(span))?;
                    read.domain_preparation.is_some()
                };
                if preparing {
                    let Some(frame) = self.data.frames.last_mut() else {
                        return Err(self.invalid("Missing stored read frame", Some(span)));
                    };
                    frame.stored_read = Some(read);
                    return Ok(());
                }
            }
            let value = self.stored_singular_value(raw, &read.digest, read.ty, span)?;
            let Some(frame) = self.data.frames.last_mut() else {
                return Err(self.invalid("Missing stored read frame", Some(span)));
            };
            frame.registers.push(value);
            return Ok(());
        }
        // The indexed admitted model pages a single edge. Never materialize the
        // remaining array or rescan its prefix to resume this continuation.
        let observation = self.invocation_observation(None)?;
        let raw = match direct {
            Some(raw) => raw,
            None => self
                .selected_stored_projection(
                    read.receiver,
                    selected.ok_or_else(|| self.invalid("Missing stored selection", Some(span)))?,
                    span,
                )?
                .collection_page_observed(read.cursor, 1, &observation)
                .map_err(|e| match self.check_provider_status(Some(span)) {
                    Err(error) => error,
                    Ok(()) => self.invalid(&e, Some(span)),
                })?
                .into_iter()
                .next(),
        };
        self.check_provider_status(Some(span))?;
        self.observe_stored(read.receiver, read.site, Some(read.cursor), raw.as_ref())?;
        let (_, element, lower, upper) = self.collection_info(read.ty)?;
        if let Some(raw) = raw {
            if matches!(
                self.type_node(element)?,
                intersect_model::definitions::rcore_behavior::ir_v2::TypeNode::Model(_)
            ) {
                let preparing = if I::BORROWED {
                    let object = raw.as_str().ok_or_else(|| {
                        self.invalid("Stored reference requires an identity", Some(span))
                    })?;
                    I::begin_stored_target(self, element, &read.digest, object, span)?
                } else {
                    read.domain_preparation =
                        self.prepare_reference_domain(element, &read.digest, Some(span))?;
                    read.domain_preparation.is_some()
                };
                if preparing {
                    let Some(frame) = self.data.frames.last_mut() else {
                        return Err(self.invalid("Missing stored read frame", Some(span)));
                    };
                    frame.stored_read = Some(read);
                    return Ok(());
                }
            }
            if upper >= 0 && read.cursor >= upper as usize {
                return Err(self.invalid("Stored collection exceeds its upper bound", Some(span)));
            }
            let Data::Model(payload) = self.node(read.receiver)?.data else {
                return Err(self.invalid("Missing stored model receiver", Some(span)));
            };
            let digest = payload
                .get("context")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| self.invalid("Missing stored model context", Some(span)))?
                .to_owned();
            let value = self.stored_leaf(raw, &digest, element, span)?;
            // Admission has already checked stored uniqueness. Preserve the
            // original values/order instead of normalizing or deduplicating.
            read.output.append(self, value, span)?;
            read.cursor = read
                .cursor
                .checked_add(1)
                .ok_or_else(|| self.resource("Stored collection cursor overflow", Some(span)))?;
            let Some(frame) = self.data.frames.last_mut() else {
                return Err(self.invalid("Missing stored read frame", Some(span)));
            };
            frame.stored_read = Some(read);
        } else {
            if read.cursor < lower {
                return Err(self.invalid("Stored collection violates its lower bound", Some(span)));
            }
            let value = read.output.finish(self, span)?;
            let Some(frame) = self.data.frames.last_mut() else {
                return Err(self.invalid("Missing stored read result frame", Some(span)));
            };
            frame.registers.push(value);
        }
        Ok(())
    }
}

pub(super) struct IncomingRead {
    observed_query: Option<usize>,
    matched_count: usize,
    domain_preparation: Option<crate::model_reference::RuntimeReferenceDomainPreparation>,
    source: TypeId,
    digest: String,
    site: CallSite,
    selector: Option<(intersect_model::definitions::schema::MemberIdentity, String)>,
    qualification: Option<crate::navigation::IncomingPreparation>,
    cursor: Option<crate::navigation::IncomingCursor>,
    output: CollectionOutput,
}
impl<I: super::inputs::InputFamily> State<'_, '_, '_, I> {
    pub(super) fn start_incoming(
        &mut self,
        receiver: Id,
        source: TypeId,
        declaration: &intersect_model::definitions::resources::DeclarationId,
        ty: TypeId,
        site: CallSite,
    ) -> Result<()> {
        let span = self
            .image
            .call_span(site)
            .ok_or_else(|| self.invalid("Missing inverse source", None))?;
        if I::BORROWED {
            return Err(self.error(
                ExecutionErrorKind::Unsupported,
                "Incoming navigation is unsupported by borrowed operation inputs",
                Some(span),
            ));
        }
        let payload = self.admit_stored_receiver(receiver, span)?;
        let digest = payload
            .get("context")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| self.invalid("Missing inverse context", Some(span)))?;
        let target = payload
            .get("element")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| self.invalid("Missing inverse target", Some(span)))?;
        let domain_preparation = self.prepare_reference_domain(source, digest, Some(span))?;
        let receiver_ty = self.node(receiver)?.ty;
        let domain = self
            .owned_domains()?
            .get(&(digest.to_owned(), receiver_ty))
            .ok_or_else(|| self.invalid("Missing inverse source domain", Some(span)))?;
        let intersect_model::definitions::rcore_behavior::ir_v2::TypeNode::Model(_) =
            self.type_node(source)?
        else {
            return Err(self.invalid("Missing inverse model type", Some(span)));
        };
        let observation = self.invocation_observation(None)?;
        intersect_model::definitions::retained_storage::reserve_serialized(
            &(digest, declaration, target),
            &observation,
        )
        .map_err(|e| self.provider_error(e, Some(span)))?;
        let member = intersect_model::definitions::schema::MemberIdentity::Source {
            declaration: declaration.clone(),
        };
        let bytes = [
            256,
            digest.len(),
            target.len(),
            domain.schema().revision.len(),
            declaration.source.uri.len(),
            declaration.source.sha256.len(),
            declaration.path.len().saturating_mul(8),
        ]
        .into_iter()
        .try_fold(0usize, |sum, n| sum.checked_add(n))
        .ok_or_else(|| self.resource("Inverse continuation size overflow", Some(span)))?;
        // Charge the known selector/context atoms. Qualification-derived atoms
        // are charged when the private cursor becomes available.
        self.collection_storage(bytes, span)?;
        let observed_query = self.data.observation.read(super::observation::ReadInput {
            kind: ReadKind::IncomingStart,
            context: digest,
            element: target,
            member: declaration,
            source: span,
            cursor: Some(0),
            source_type: Some(source),
            matched: None,
            value: None,
        });
        let read = IncomingRead {
            observed_query,
            matched_count: 0,
            domain_preparation,
            source,
            digest: digest.into(),
            site,
            selector: Some((member, target.into())),
            qualification: None,
            cursor: None,
            output: CollectionOutput::new(ty),
        };
        let Some(frame) = self.data.frames.last_mut() else {
            return Err(self.invalid("Missing inverse frame", Some(span)));
        };
        frame.incoming = Some(read);
        Ok(())
    }
    pub(super) fn advance_incoming(&mut self) -> Result<()> {
        let mut read = self
            .data
            .frames
            .last_mut()
            .and_then(|f| f.incoming.take())
            .ok_or_else(|| self.invalid("Missing inverse continuation", None))?;
        let span = self
            .image
            .call_span(read.site)
            .ok_or_else(|| self.invalid("Missing inverse source", None))?;
        if read.domain_preparation.is_some() {
            self.advance_reference_domain(
                &mut read.domain_preparation,
                read.source,
                &read.digest,
                Some(span),
            )?;
            let Some(frame) = self.data.frames.last_mut() else {
                return Err(self.invalid("Missing inverse frame", Some(span)));
            };
            frame.incoming = Some(read);
            return Ok(());
        }
        if let Some((member, target)) = read.selector.take() {
            self.step(span)?;
            let domain = self
                .owned_domains()?
                .get(&(read.digest.clone(), read.source))
                .ok_or_else(|| self.invalid("Missing inverse domain", Some(span)))?;
            let observation = self.invocation_observation(None)?;
            intersect_model::definitions::retained_storage::reserve_serialized(
                domain.target_key(),
                &observation,
            )
            .map_err(|e| self.provider_error(e, Some(span)))?;
            let key = domain.target_key().to_owned();
            self.collection_storage(key.len(), span)?;
            read.qualification = Some(
                crate::navigation::IncomingPreparation::new_observed(
                    &key,
                    &member,
                    &target,
                    &observation,
                )
                .map_err(|e| match self.check_provider_status(Some(span)) {
                    Err(error) => error,
                    Ok(()) => self.invalid(&e, Some(span)),
                })?,
            );
            let Some(frame) = self.data.frames.last_mut() else {
                return Err(self.invalid("Missing inverse frame", Some(span)));
            };
            frame.incoming = Some(read);
            return Ok(());
        }
        if let Some(qualification) = read.qualification.as_mut() {
            self.step(span)?;
            let domain = self
                .owned_domains()?
                .get(&(read.digest.clone(), read.source))
                .ok_or_else(|| self.invalid("Missing inverse domain", Some(span)))?;
            let observation = self.invocation_observation(None)?;
            if qualification
                .advance_observed(domain.read(), domain.schema(), &observation)
                .map_err(|e| match self.check_provider_status(Some(span)) {
                    Err(error) => error,
                    Ok(()) => self.invalid(&e, Some(span)),
                })?
            {
                let cursor = read
                    .qualification
                    .take()
                    .ok_or_else(|| self.invalid("Missing inverse preparation", Some(span)))?
                    .finish()
                    .map_err(|e| self.invalid(&e, Some(span)))?;
                self.collection_storage(cursor.qualification_atom_bytes(), span)?;
                read.cursor = Some(cursor);
            }
            self.check_provider_status(Some(span))?;
            let Some(frame) = self.data.frames.last_mut() else {
                return Err(self.invalid("Missing inverse frame", Some(span)));
            };
            frame.incoming = Some(read);
            return Ok(());
        }
        // Charge a candidate plus up to two adapter lookahead streams. Schema
        // transitions use the same conservative charge without hiding scans.
        for _ in 0..3 {
            self.step(span)?;
        }
        let domain = self
            .owned_domains()?
            .get(&(read.digest.clone(), read.source))
            .ok_or_else(|| self.invalid("Missing inverse domain", Some(span)))?;
        let observation = self.invocation_observation(None)?;
        let next = read
            .cursor
            .as_mut()
            .ok_or_else(|| self.invalid("Missing inverse cursor", Some(span)))?
            .advance_observed(domain.read(), domain.schema(), &observation)
            .map_err(|e| match self.check_provider_status(Some(span)) {
                Err(error) => error,
                Ok(()) => self.invalid(&e, Some(span)),
            })?;
        self.check_provider_status(Some(span))?;
        match next {
            crate::navigation::IncomingStep::Match(id) => {
                read.matched_count += 1;
                self.data.observation.incoming_followup(
                    read.observed_query,
                    ReadKind::IncomingMatch,
                    Some(&id),
                    read.matched_count,
                );
                let value = self.stored_leaf(
                    serde_json::Value::String(id),
                    &read.digest,
                    read.source,
                    span,
                )?;
                read.output.append(self, value, span)?;
            }
            crate::navigation::IncomingStep::Pending => (),
            crate::navigation::IncomingStep::Done => {
                self.data.observation.incoming_followup(
                    read.observed_query,
                    ReadKind::IncomingEnd,
                    None,
                    read.matched_count,
                );
                let value = read.output.finish(self, span)?;
                let Some(frame) = self.data.frames.last_mut() else {
                    return Err(self.invalid("Missing inverse result frame", Some(span)));
                };
                frame.registers.push(value);
                return Ok(());
            }
        }
        let Some(frame) = self.data.frames.last_mut() else {
            return Err(self.invalid("Missing inverse frame", Some(span)));
        };
        frame.incoming = Some(read);
        Ok(())
    }
}
