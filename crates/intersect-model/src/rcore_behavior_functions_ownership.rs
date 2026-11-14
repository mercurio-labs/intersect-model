//! Cooperative ownership navigation over the shared structural admission index.
use super::{
    collections::CollectionOutput,
    machine::{CallSite, State},
    values::{Data, Id},
    *,
};
use intersect_model::core::storage::ModelRead;
use intersect_model::definitions::rcore_behavior::ir_v2::{OwnershipOp, TypeId, TypeNode};

pub(super) struct OwnershipRead {
    domain_preparation: Option<crate::model_reference::RuntimeReferenceDomainPreparation>,
    operation: OwnershipOp,
    selected: TypeId,
    ty: TypeId,
    digest: String,
    current: String,
    cursor: usize,
    site: CallSite,
    output: CollectionOutput,
}
impl<I: super::inputs::InputFamily> State<'_, '_, '_, I> {
    pub(super) fn start_ownership(
        &mut self,
        operation: OwnershipOp,
        receiver: Id,
        selected: TypeId,
        ty: TypeId,
        site: CallSite,
    ) -> Result<()> {
        let span = self
            .image
            .call_span(site)
            .ok_or_else(|| self.invalid("Missing ownership source", None))?;
        if I::BORROWED && !I::source_scope(&self.data.domains) {
            return Err(self.error(
                ExecutionErrorKind::Unsupported,
                "Ownership navigation is unsupported by borrowed operation inputs",
                Some(span),
            ));
        }
        // Full structural admission rejects ambiguous/cyclic/broken ownership
        // before result filtering. The index is shared across target domains.
        let payload = self.admit_stored_receiver(receiver, span)?;
        let digest = payload
            .get("context")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| self.invalid("Missing ownership context", Some(span)))?;
        let current = payload
            .get("element")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| self.invalid("Missing ownership object", Some(span)))?;
        let domain_preparation = if I::source_scope(&self.data.domains) {
            I::begin_stored_target(self, selected, digest, current, span)?;
            None
        } else {
            let pending = self.prepare_reference_domain(selected, digest, Some(span))?;
            let domain = self
                .owned_domains()?
                .get(&(digest.to_owned(), self.node(receiver)?.ty))
                .ok_or_else(|| self.invalid("Missing ownership receiver domain", Some(span)))?;
            if operation == OwnershipOp::Roots {
                return Err(self.error(
                    ExecutionErrorKind::Unsupported,
                    "roots navigation is available to scope functions only",
                    Some(span),
                ));
            }
            let requires_provider = match operation {
                OwnershipOp::AsType => false,
                OwnershipOp::Roots => false,
                OwnershipOp::Contents => domain.ownership().has_derived_contents(current),
                OwnershipOp::Container | OwnershipOp::Ancestors => {
                    domain.ownership().has_derived_owners()
                }
            };
            if requires_provider {
                return Err(self.invalid(
                    "Derived containment navigation requires an executable provider",
                    Some(span),
                ));
            }
            pending
        };

        let bytes = 256usize
            .checked_add(digest.len())
            .and_then(|n| n.checked_add(current.len()))
            .ok_or_else(|| self.resource("Ownership continuation size overflow", Some(span)))?;
        self.collection_storage(bytes, span)?;
        let read = OwnershipRead {
            domain_preparation,
            operation,
            selected,
            ty,
            digest: digest.into(),
            current: current.into(),
            cursor: 0,
            site,
            output: CollectionOutput::new(ty),
        };
        let Some(frame) = self.data.frames.last_mut() else {
            return Err(self.invalid("Missing ownership frame", Some(span)));
        };
        frame.ownership = Some(read);
        Ok(())
    }
    pub(super) fn advance_ownership(&mut self) -> Result<()> {
        let mut read = self
            .data
            .frames
            .last_mut()
            .and_then(|f| f.ownership.take())
            .ok_or_else(|| self.invalid("Missing ownership continuation", None))?;
        let span = self
            .image
            .call_span(read.site)
            .ok_or_else(|| self.invalid("Missing ownership source", None))?;
        if I::advance_stored_target(self, span)? {
            let Some(frame) = self.data.frames.last_mut() else {
                return Err(self.invalid("Missing ownership frame", Some(span)));
            };
            frame.ownership = Some(read);
            return Ok(());
        }
        if read.domain_preparation.is_some() {
            self.advance_reference_domain(
                &mut read.domain_preparation,
                read.selected,
                &read.digest,
                Some(span),
            )?;
            let Some(frame) = self.data.frames.last_mut() else {
                return Err(self.invalid("Missing ownership frame", Some(span)));
            };
            frame.ownership = Some(read);
            return Ok(());
        }
        self.step(span)?;
        if I::source_scope(&self.data.domains) {
            let TypeNode::Model(selected) = self.type_node(read.selected)?.clone() else {
                return Err(self.invalid("Missing ownership target model type", Some(span)));
            };
            let raw = I::direct_ownership(
                self,
                &read.current,
                read.operation,
                &selected,
                read.cursor,
                span,
            )?
            .ok_or_else(|| {
                self.invalid("Source scope has no ownership read capability", Some(span))
            })?;
            if read.operation.singular() {
                let data = match raw {
                    Some(value) => {
                        Data::Some(self.stored_leaf(value, &read.digest, read.selected, span)?)
                    }
                    None => Data::None,
                };
                let value = self.alloc(read.ty, data, Some(span))?;
                return self.finish_ownership(value, span);
            }
            let Some(raw) = raw else {
                let value = read.output.finish(self, span)?;
                return self.finish_ownership(value, span);
            };
            let value = self.stored_leaf(raw, &read.digest, read.selected, span)?;
            read.output.append(self, value, span)?;
            read.cursor = read
                .cursor
                .checked_add(1)
                .ok_or_else(|| self.resource("Ownership cursor overflow", Some(span)))?;
            let Some(frame) = self.data.frames.last_mut() else {
                return Err(self.invalid("Missing ownership frame", Some(span)));
            };
            frame.ownership = Some(read);
            return Ok(());
        }
        let domain = self
            .owned_domains()?
            .get(&(read.digest.clone(), read.selected))
            .ok_or_else(|| self.invalid("Missing ownership domain", Some(span)))?;
        let candidate = match read.operation {
            OwnershipOp::AsType => Some(read.current.as_str()),
            // Refused when the read starts; owned domains have no root inventory.
            OwnershipOp::Roots => None,
            OwnershipOp::Container | OwnershipOp::Ancestors => {
                domain.ownership().parent(&read.current)
            }
            OwnershipOp::Contents => domain
                .ownership()
                .children(&read.current)
                .get(read.cursor)
                .map(String::as_str),
        };
        let next = if let Some(id) = candidate {
            let found = domain
                .read()
                .inspect_class(id, &mut |_| Ok(()))
                .map_err(|e| self.invalid(&e, Some(span)))?;
            if !found {
                return Err(self.invalid("Missing validated containment object", Some(span)));
            }
            let TypeNode::Model(_) = self.type_node(read.selected)? else {
                return Err(self.invalid("Missing ownership target type", Some(span)));
            };
            Some((id.to_owned(), domain.contains_object(id)))
        } else {
            None
        };
        if read.operation.singular() {
            let data = if let Some((id, true)) = next {
                let value = self.stored_leaf(
                    serde_json::Value::String(id),
                    &read.digest,
                    read.selected,
                    span,
                )?;
                Data::Some(value)
            } else {
                Data::None
            };
            let value = self.alloc(read.ty, data, Some(span))?;
            return self.finish_ownership(value, span);
        }
        let Some((id, matching)) = next else {
            let value = read.output.finish(self, span)?;
            return self.finish_ownership(value, span);
        };
        // Each continuation retains at most one current ancestor identity.
        if read.operation == OwnershipOp::Ancestors {
            self.collection_storage(id.len(), span)?;
            read.current = id.clone();
        } else {
            read.cursor = read
                .cursor
                .checked_add(1)
                .ok_or_else(|| self.resource("Contents cursor overflow", Some(span)))?;
        }
        if matching {
            let value = self.stored_leaf(
                serde_json::Value::String(id),
                &read.digest,
                read.selected,
                span,
            )?;
            read.output.append(self, value, span)?;
        }
        let Some(frame) = self.data.frames.last_mut() else {
            return Err(self.invalid("Missing ownership frame", Some(span)));
        };
        frame.ownership = Some(read);
        Ok(())
    }
    fn finish_ownership(&mut self, value: Id, span: &SourceSpan) -> Result<()> {
        let Some(frame) = self.data.frames.last_mut() else {
            return Err(self.invalid("Missing ownership result frame", Some(span)));
        };
        frame.registers.push(value);
        Ok(())
    }
}
