//! Retained DFS and node-local agreement, shared with original Visit draining.
//! Formals, individual checked resolution and cache installation remain tails.
use super::{AgreementPreparation, Resolver, SignatureInventory, error, fallback};
use crate::definitions::{
    bundle::{SourceId, SourceSpan},
    ecore::{ImportError, Node, ParserObservation},
    generic_resolution::{ResolvedParameter, ResolvedType},
    resources::DeclarationId,
};
#[derive(Clone, Copy)]
enum Phase {
    Sources,
    Root,
    Visit,
    Children(usize),
    Ascend,
    Complete,
    Failed,
}
// Source index/root depth and construction-fixed polling share one inline word
// pair. No observer or mode supplied by a later resume can change this contract.
enum Scope {
    All(usize),
    Subtree(usize),
    RetainedAll(usize),
    RetainedSubtree(usize),
    SourceAll(usize),
    // Only the verification-only subtree driver constructs this scope.
    #[cfg_attr(not(test), allow(dead_code))]
    SourceSubtree(usize),
    Failed,
}
enum State<'a> {
    Walking {
        inventory: SignatureInventory,
        phase: Phase,
    },
    Agreement {
        sources: Vec<SourceId>,
        parameters: Vec<ResolvedParameter>,
        types: Vec<ResolvedType>,
        retained_payloads: Vec<SourceSpan>,
        preparation: AgreementPreparation<'a>,
        prefix_len: usize,
    },
    Publishing {
        inventory: SignatureInventory,
        node: &'a Node,
        prefix_len: usize,
    },
    Source {
        inventory: SignatureInventory,
        preparation: crate::definitions::generics::GenericTypePreparation<'a>,
    },
    // A move transfers sole vector ownership out; this variant owns no headers.
    Transition,
}
pub(crate) struct SourceInventoryPreparation<'resolver> {
    resolver: &'resolver Resolver,
    cursor: Option<DeclarationId>,
    ancestors: Vec<usize>,
    scope: Scope,
    state: State<'resolver>,
}
const STATE_BYTES: usize = 512;
const _: () =
    assert!(std::mem::size_of::<SourceInventoryPreparation<'static>>() + 128 <= STATE_BYTES);
impl<'resolver> SourceInventoryPreparation<'resolver> {
    pub(crate) fn new(
        resolver: &'resolver Resolver,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        Self::begin(resolver, observation, Scope::All(0))
    }
    pub(crate) fn new_with_retained_source_inventory_agreement(
        resolver: &'resolver Resolver,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        Self::begin(resolver, observation, Scope::RetainedAll(0))
    }
    pub(crate) fn new_with_retained_generic_source_decoding(
        resolver: &'resolver Resolver,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        Self::begin(resolver, observation, Scope::SourceAll(0))
    }
    fn begin(
        resolver: &'resolver Resolver,
        observation: &ParserObservation<'_, '_>,
        scope: Scope,
    ) -> Result<Self, ImportError> {
        observation.reserve(STATE_BYTES, &resolver.standard_id)?;
        Ok(Self {
            resolver,
            cursor: None,
            ancestors: Vec::new(),
            scope,
            state: State::Walking {
                inventory: SignatureInventory {
                    sources: Vec::new(),
                    parameters: Vec::new(),
                    types: Vec::new(),
                    raw_signatures: Vec::new(),
                    retained_payloads: Vec::new(),
                },
                phase: Phase::Sources,
            },
        })
    }
    pub(super) fn subtree(
        resolver: &'resolver Resolver,
        id: &DeclarationId,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        Self::begin_subtree(resolver, id, observation, false)
    }
    #[cfg(test)]
    fn subtree_with_retained_source_inventory_agreement(
        resolver: &'resolver Resolver,
        id: &DeclarationId,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        Self::begin_subtree(resolver, id, observation, true)
    }
    fn begin_subtree(
        resolver: &'resolver Resolver,
        id: &DeclarationId,
        observation: &ParserObservation<'_, '_>,
        retained: bool,
    ) -> Result<Self, ImportError> {
        let mut p = Self::new(resolver, observation)?;
        observation.serialized(id, &id.source)?;
        p.cursor = Some(id.clone());
        p.scope = if retained {
            Scope::RetainedSubtree(id.path.len())
        } else {
            Scope::Subtree(id.path.len())
        };
        if let State::Walking { phase, .. } = &mut p.state {
            *phase = Phase::Visit
        }
        Ok(p)
    }
    #[cfg(test)]
    pub(crate) fn subtree_with_retained_generic_source_decoding(
        resolver: &'resolver Resolver,
        id: &DeclarationId,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        let mut result =
            Self::subtree_with_retained_source_inventory_agreement(resolver, id, observation)?;
        result.scope = Scope::SourceSubtree(id.path.len());
        Ok(result)
    }
    #[cfg(test)]
    pub(crate) fn is_source_decoding_step(&self) -> bool {
        match &self.state {
            State::Agreement { preparation, .. } => preparation.is_source_decoding_step(),
            State::Source { .. } => true,
            _ => false,
        }
    }
    pub(crate) fn is_retained_agreement_step(&self) -> bool {
        matches!(
            self.scope,
            Scope::RetainedAll(_)
                | Scope::RetainedSubtree(_)
                | Scope::SourceAll(_)
                | Scope::SourceSubtree(_)
        ) && matches!(
            self.state,
            State::Agreement { .. } | State::Publishing { .. } | State::Source { .. }
        )
    }
    pub(crate) fn advance(
        &mut self,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        if matches!(self.scope, Scope::Failed) {
            return Err(self.error("Source inventory preparation has failed"));
        }
        let result = (|| {
            let source = self
                .cursor
                .as_ref()
                .map_or(&self.resolver.standard_id, |id| &id.source);
            observation.work(0, source)?;
            self.advance_inner(observation)?;
            if !matches!(
                self.scope,
                Scope::RetainedAll(_)
                    | Scope::RetainedSubtree(_)
                    | Scope::SourceAll(_)
                    | Scope::SourceSubtree(_)
            ) {
                // Preserve one source work(0), policy comparison and outer checkpoint set
                // for the entire old Visit, while draining this SAME retained algorithm.
                while matches!(self.state, State::Agreement { .. }) {
                    self.advance_inner(observation)?;
                }
            }
            Ok(matches!(
                self.state,
                State::Walking {
                    phase: Phase::Complete,
                    ..
                }
            ))
        })();
        if result.is_err() {
            self.scope = Scope::Failed;
            if let State::Walking { phase, .. } = &mut self.state {
                *phase = Phase::Failed
            }
        }
        result
    }
    fn error(&self, message: &str) -> ImportError {
        source_error(self.resolver, message)
    }
    fn advance_inner(
        &mut self,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        if matches!(self.state, State::Source { .. }) {
            return self.advance_source(observation);
        }
        if matches!(self.state, State::Agreement { .. }) {
            return self.advance_agreement(observation);
        }
        let State::Walking { inventory, phase } = &mut self.state else {
            return Err(source_error(
                self.resolver,
                "Missing source inventory phase",
            ));
        };
        match *phase {
            Phase::Sources => {
                let index = match &mut self.scope {
                    Scope::All(i) | Scope::RetainedAll(i) | Scope::SourceAll(i) => i,
                    _ => {
                        return Err(source_error(
                            self.resolver,
                            "Missing source inventory index",
                        ));
                    }
                };
                if let Some(source) = self.resolver.sources.get(*index) {
                    observation.work(1, source)?;
                    observation.serialized(source, source)?;
                    observation.reserve(32, source)?;
                    inventory.sources.push(source.clone());
                    *index += 1
                } else {
                    *index = 0;
                    *phase = Phase::Root
                }
            }
            Phase::Root => {
                let index = match self.scope {
                    Scope::All(i) | Scope::RetainedAll(i) | Scope::SourceAll(i) => i,
                    _ => {
                        return Err(source_error(
                            self.resolver,
                            "Missing source inventory index",
                        ));
                    }
                };
                if let Some(source) = self.resolver.sources.get(index) {
                    observation.work(1, source)?;
                    observation.serialized(source, source)?;
                    let path = self.cursor.take().map_or_else(Vec::new, |id| id.path);
                    self.cursor = Some(DeclarationId {
                        source: source.clone(),
                        path,
                    });
                    *phase = Phase::Visit
                } else {
                    *phase = Phase::Complete
                }
            }
            Phase::Visit => {
                let id = self
                    .cursor
                    .as_ref()
                    .ok_or_else(|| source_error(self.resolver, "Missing source cursor"))?;
                if self
                    .resolver
                    .collect_node_prefix_context(id, inventory, observation)?
                    .is_none()
                {
                    *phase = Phase::Children(0);
                    return Ok(());
                }
                let old = std::mem::replace(&mut self.state, State::Transition);
                let State::Walking { inventory, .. } = old else {
                    return Err(source_error(
                        self.resolver,
                        "Missing source inventory inputs",
                    ));
                };
                let SignatureInventory {
                    sources,
                    parameters,
                    types,
                    raw_signatures,
                    retained_payloads,
                } = inventory;
                let prefix_len = raw_signatures.len();
                self.state = State::Agreement {
                    sources,
                    parameters,
                    types,
                    retained_payloads,
                    prefix_len,
                    preparation: if matches!(
                        self.scope,
                        Scope::SourceAll(_) | Scope::SourceSubtree(_)
                    ) {
                        AgreementPreparation::begin_with_retained_source_decoding_and_result_storage(
                            raw_signatures,
                        )
                    } else {
                        AgreementPreparation::begin_with_result_storage(raw_signatures)
                    },
                };
            }
            Phase::Children(children) => {
                if children == 0 {
                    *phase = Phase::Ascend
                } else {
                    let id = self
                        .cursor
                        .as_mut()
                        .ok_or_else(|| source_error(self.resolver, "Missing source cursor"))?;
                    observation.work(1, &id.source)?;
                    for values in [&mut id.path, &mut self.ancestors] {
                        if values.len() == values.capacity() {
                            observation.reserve(32, &id.source)?;
                            values.try_reserve_exact(1).map_err(|_| {
                                error(
                                    &SourceSpan {
                                        source: id.source.clone(),
                                        start: 0,
                                        end: 0,
                                    },
                                    "Cannot retain source cursor",
                                )
                            })?;
                        }
                    }
                    id.path.push(0);
                    self.ancestors.push(children);
                    *phase = Phase::Visit;
                }
            }
            Phase::Ascend => {
                let id = self
                    .cursor
                    .as_mut()
                    .ok_or_else(|| source_error(self.resolver, "Missing source cursor"))?;
                observation.work(1, &id.source)?;
                let depth = match self.scope {
                    Scope::Subtree(d) | Scope::RetainedSubtree(d) | Scope::SourceSubtree(d) => {
                        Some(d)
                    }
                    _ => None,
                };
                if depth == Some(id.path.len()) {
                    *phase = Phase::Complete
                } else if let Some(index) = id.path.last_mut() {
                    let count =
                        self.ancestors.last().copied().ok_or_else(|| {
                            source_error(self.resolver, "Missing source ancestor")
                        })?;
                    if *index + 1 < count {
                        *index += 1;
                        *phase = Phase::Visit
                    } else {
                        id.path.pop();
                        self.ancestors.pop();
                    }
                } else {
                    match &mut self.scope {
                        Scope::All(i) | Scope::RetainedAll(i) | Scope::SourceAll(i) => *i += 1,
                        _ => {
                            return Err(source_error(
                                self.resolver,
                                "Missing source inventory index",
                            ));
                        }
                    }
                    *phase = Phase::Root
                }
            }
            Phase::Complete | Phase::Failed => {}
        }
        Ok(())
    }
    fn advance_agreement(
        &mut self,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        let id = self
            .cursor
            .as_ref()
            .ok_or_else(|| source_error(self.resolver, "Missing source cursor"))?;
        let State::Agreement { preparation, .. } = &mut self.state else {
            return Err(source_error(
                self.resolver,
                "Missing retained source agreement",
            ));
        };
        if !preparation.advance(self.resolver, id, observation)? {
            return Ok(());
        }
        let old = std::mem::replace(&mut self.state, State::Transition);
        let State::Agreement {
            sources,
            parameters,
            types,
            retained_payloads,
            preparation,
            prefix_len,
        } = old
        else {
            return Err(source_error(
                self.resolver,
                "Missing retained source agreement inputs",
            ));
        };
        let node = preparation
            .prepared_node()
            .ok_or_else(|| source_error(self.resolver, "Missing retained source agreement node"))?;
        // Consume the cursor before installing its sole moved result header. All old
        // raw rows and new admitted records have exactly one live owning vector.
        let raw_signatures = preparation.finish(id)?;
        self.state = State::Publishing {
            inventory: SignatureInventory {
                sources,
                parameters,
                types,
                raw_signatures,
                retained_payloads,
            },
            node,
            prefix_len,
        };
        let State::Publishing {
            inventory,
            node,
            prefix_len,
        } = &mut self.state
        else {
            return Err(source_error(
                self.resolver,
                "Missing source inventory publication",
            ));
        };
        // Original record admissions precede agreement pushes. Original destination
        // fees remain after ALL projections succeed, before walking/publication.
        for _ in *prefix_len..inventory.raw_signatures.len() {
            observation.work(1, &id.source)?;
            observation.reserve(32, &id.source)?;
        }
        if matches!(self.scope, Scope::SourceAll(_) | Scope::SourceSubtree(_))
            && crate::definitions::generics::type_position(node)
        {
            let old = std::mem::replace(&mut self.state, State::Transition);
            let State::Publishing { inventory, .. } = old else {
                return Err(source_error(
                    self.resolver,
                    "Missing source decoder inventory",
                ));
            };
            self.state = State::Source {
                inventory,
                preparation: crate::definitions::generics::GenericTypePreparation::begin(),
            };
            return Ok(());
        }
        let children =
            self.resolver
                .collect_node_suffix_context(id, node, inventory, observation)?;
        let old = std::mem::replace(&mut self.state, State::Transition);
        let State::Publishing { inventory, .. } = old else {
            return Err(source_error(
                self.resolver,
                "Missing source inventory publication inputs",
            ));
        };
        self.state = State::Walking {
            inventory,
            phase: Phase::Children(children),
        };
        Ok(())
    }
    fn advance_source(
        &mut self,
        observation: &ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        let id = self
            .cursor
            .as_ref()
            .ok_or_else(|| source_error(self.resolver, "Missing source cursor"))?;
        let State::Source { preparation, .. } = &mut self.state else {
            return Err(source_error(self.resolver, "Missing source decoder"));
        };
        if !preparation.advance(self.resolver.resources(id), id, observation)? {
            return Ok(());
        }
        let old = std::mem::replace(&mut self.state, State::Transition);
        let State::Source {
            mut inventory,
            preparation,
        } = old
        else {
            return Err(source_error(self.resolver, "Missing source decoder input"));
        };
        let syntax = preparation.finish(id)?;
        let checked = self
            .resolver
            .finish_type_syntax_context(id, syntax, observation)?;
        observation.reserve(32, &id.source)?;
        inventory.types.push(checked);
        self.state = State::Walking {
            inventory,
            phase: Phase::Children(0),
        };
        Ok(())
    }
    pub(crate) fn finish(self) -> Result<SignatureInventory, ImportError> {
        if matches!(self.scope, Scope::Failed)
            || !matches!(
                self.state,
                State::Walking {
                    phase: Phase::Complete,
                    ..
                }
            )
        {
            return Err(self.error("Source inventory preparation is not complete"));
        }
        match self.state {
            State::Walking { inventory, .. } => Ok(inventory),
            _ => Err(source_error(
                self.resolver,
                "Missing completed source inventory",
            )),
        }
    }
}
fn source_error(resolver: &Resolver, message: &str) -> ImportError {
    error(
        &fallback(&DeclarationId {
            source: resolver.standard_id.clone(),
            path: Vec::new(),
        }),
        message,
    )
}
#[cfg(test)]
#[path = "generic_source_inventory_preparation_tests.rs"]
mod preparation_tests;

#[cfg(test)]
#[path = "generics_decoder_inventory_compatibility_controls.rs"]
mod source_decoder_complete_inventory_controls;
