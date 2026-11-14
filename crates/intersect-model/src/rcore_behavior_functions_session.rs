//! A source-authorized evaluation keeps one private state across explicit slices.
use super::*;
use std::num::NonZeroUsize;
#[path = "rcore_behavior_functions_borrowed_session.rs"]
mod borrowed;
pub(crate) use borrowed::BorrowedEvaluationSession;
struct NeverCancel;
impl ExecutionControl for NeverCancel {
    fn cancellation_requested(&self) -> bool {
        false
    }
}

/// A terminal result is moved out once. Further resumes return Finished without
/// executing callbacks, allocating another result, or replenishing any budget.
pub enum SessionProgress {
    Pending,
    Complete(Box<Result<Evaluation>>),
    Finished,
}
#[derive(Clone, Copy)]
enum Phase {
    Context,
    TransferReferences,
    Constants {
        function: usize,
        block: usize,
        instruction: usize,
    },
    Arguments {
        index: usize,
    },
    Entry {
        index: usize,
    },
    Running,
    Encoding(Id),
    Finished,
}
// Preserve the caller's vector allocation until bounded decode/validation;
// converting it into a boxed slice could resize/copy before those checks.
// Portable logical record allowance, identical on native and WASM.
const OWNED_REFERENCE_INPUT_BYTES: usize = 1024;
const _: () = assert!(
    std::mem::size_of::<(
        usize,
        intersect_model::definitions::conversion::ReferenceContext
    )>() + 128
        <= OWNED_REFERENCE_INPUT_BYTES
);
enum SliceInput<'a, T> {
    Borrowed(&'a [T]),
    Owned(Vec<T>),
}
impl<T> std::ops::Deref for SliceInput<'_, T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        match self {
            Self::Borrowed(value) => value,
            Self::Owned(value) => value,
        }
    }
}
// A fixed session root holds authority by reference or by value. Moving owned
// source headers into the root removes the old image/bundle/limits Boxes; it
// never clones source text, JSON arguments, executable authority or contexts.
struct BorrowedSources<'a> {
    registry: Option<intersect_model::definitions::conversion::ConversionRegistry<'a>>,
    image: &'a VerifiedExecutable,
    arguments: &'a [ValueWire],
    bundle: &'a DefinitionBundle,
    limits: &'a ExecutionLimits,
    arena: std::cell::OnceCell<Vec<crate::rcore_behavior_functions::values::Node>>,
}
struct OwnedSources {
    registry: Option<intersect_model::definitions::conversion::ConversionRegistry<'static>>,
    image: VerifiedExecutable,
    arguments: Vec<ValueWire>,
    bundle: DefinitionBundle,
    limits: ExecutionLimits,
    arena: std::cell::OnceCell<Vec<crate::rcore_behavior_functions::values::Node>>,
}
self_cell::self_cell!(
    struct BorrowedSessionCell<'input> {
        owner: BorrowedSources<'input>,
        #[covariant]
        dependent: SessionState,
    }
);
self_cell::self_cell!(
    struct OwnedSessionCell {
        owner: OwnedSources,
        #[covariant]
        dependent: SessionState,
    }
);
struct BindingSources<'a> {
    sources: BorrowedSources<'a>,
    result: std::cell::OnceCell<ValueWire>,
}
self_cell::self_cell!(
    struct BindingSessionCell<'input> {
        owner: BindingSources<'input>,
        #[covariant]
        dependent: SessionState,
    }
);
enum SessionRoot<'a> {
    Binding(BindingSessionCell<'a>),
    Borrowed(BorrowedSessionCell<'a>),
    Owned(OwnedSessionCell),
}
/// A source-authorized evaluation owns one private state across explicit slices.
/// Shape inspection and JSON payload measurement/copy can suspend without
/// publishing a partial value. Header metadata, map comparison/insertion,
/// reservations and opaque callbacks retain explicit atomic boundaries.
/// Further resumes after completion are silent; source/context/arena cleanup
/// remains an explicit owner operation rather than a bounded cleanup claim.
pub struct EvaluationSession<'a> {
    root: SessionRoot<'a>,
}
impl<'a> EvaluationSession<'a> {
    /// Verification only: actual numeric phase and exclusively moved input
    /// arena length/capacity. No pointer is used as execution authority.
    #[cfg(feature = "allocation-profile")]
    pub fn retained_numeric_phase(&self) -> Option<super::RetainedNumericPhase> {
        let state = match &self.root {
            SessionRoot::Borrowed(cell) => cell.borrow_dependent(),
            SessionRoot::Owned(cell) => cell.borrow_dependent(),
            SessionRoot::Binding(cell) => cell.borrow_dependent(),
        };
        state
            .execution
            .frames
            .last()
            .and_then(|frame| frame.numeric_progress())
    }

    #[cfg(test)]
    fn execution_mut(&mut self) -> &mut ExecutionState {
        match &mut self.root {
            SessionRoot::Binding(cell) => cell.with_dependent_mut(|_, state| &mut state.execution),
            SessionRoot::Borrowed(cell) => cell.with_dependent_mut(|_, state| &mut state.execution),
            SessionRoot::Owned(cell) => cell.with_dependent_mut(|_, state| &mut state.execution),
        }
    }
    #[cfg(test)]
    fn decoded_arguments_mut(&mut self) -> &mut Vec<Id> {
        match &mut self.root {
            SessionRoot::Binding(cell) => {
                cell.with_dependent_mut(|_, state| &mut state.decoded_arguments)
            }
            SessionRoot::Borrowed(cell) => {
                cell.with_dependent_mut(|_, state| &mut state.decoded_arguments)
            }
            SessionRoot::Owned(cell) => {
                cell.with_dependent_mut(|_, state| &mut state.decoded_arguments)
            }
        }
    }
    #[cfg(test)]
    fn context_preparation(
        &self,
    ) -> Option<&crate::rcore_behavior_functions::context::ContextPreparation> {
        let state = match &self.root {
            SessionRoot::Binding(cell) => cell.borrow_dependent(),
            SessionRoot::Borrowed(cell) => cell.borrow_dependent(),
            SessionRoot::Owned(cell) => cell.borrow_dependent(),
        };
        match &state.work {
            SessionWork::Context(value) => Some(value),
            _ => None,
        }
    }
    #[cfg(test)]
    fn references(&self) -> &[intersect_model::definitions::conversion::ReferenceContext] {
        match &self.root {
            SessionRoot::Binding(cell) => &cell.borrow_dependent().references,
            SessionRoot::Borrowed(cell) => &cell.borrow_dependent().references,
            SessionRoot::Owned(cell) => &cell.borrow_dependent().references,
        }
    }

    #[cfg(feature = "allocation-profile")]
    pub fn retained_phase_layouts() -> [usize; 3] {
        [
            std::mem::size_of::<SessionWork<'static>>(),
            std::mem::size_of::<crate::rcore_behavior_functions::context::ContextPreparation>(),
            std::mem::size_of::<Phase>(),
        ]
    }
    #[cfg(feature = "allocation-profile")]
    pub fn retained_root_layouts() -> [usize; 7] {
        retained_session_root_layouts()
    }
    #[cfg(feature = "allocation-profile")]
    pub fn retained_binding_root_layouts() -> [usize; 4] {
        [
            std::mem::size_of::<BindingSources<'static>>(),
            std::mem::size_of::<(BindingSources<'static>, SessionState<'static>)>(),
            std::mem::size_of::<crate::semantic_bindings::BindingSession<'static>>(),
            std::mem::size_of::<ValueWire>(),
        ]
    }
    #[cfg(feature = "allocation-profile")]
    pub fn retained_json_frontier(&self) -> Option<(usize, usize)> {
        let state = match &self.root {
            SessionRoot::Borrowed(cell) => cell.borrow_dependent(),
            SessionRoot::Owned(cell) => cell.borrow_dependent(),
            SessionRoot::Binding(cell) => cell.borrow_dependent(),
        };
        state
            .work
            .shape()
            .and_then(|(shape, _)| shape.resource_frontier())
    }
    #[cfg(feature = "allocation-profile")]
    pub fn retained_general_json_phase(&self) -> Option<(&'static str, &'static str, [usize; 8])> {
        let state = match &self.root {
            SessionRoot::Borrowed(c) => c.borrow_dependent(),
            SessionRoot::Owned(c) => c.borrow_dependent(),
            SessionRoot::Binding(c) => c.borrow_dependent(),
        };
        match &state.work {
            SessionWork::Decode(v) => v.payload_frontier().map(|(phase, n)| ("decode", phase, n)),
            SessionWork::Encode(v) => v.payload_frontier().map(|(phase, n)| ("encode", phase, n)),
            _ => None,
        }
    }
    #[cfg(feature = "allocation-profile")]
    pub fn retained_scalar_validation(&self) -> Option<(&'static str, usize, usize)> {
        let state = match &self.root {
            SessionRoot::Borrowed(c) => c.borrow_dependent(),
            SessionRoot::Owned(c) => c.borrow_dependent(),
            SessionRoot::Binding(c) => c.borrow_dependent(),
        };
        state
            .work
            .shape()
            .and_then(|(cursor, _)| cursor.scalar_progress())
    }
    #[cfg(feature = "allocation-profile")]
    pub fn retained_scalar_validation_layouts() -> [usize; 3] {
        intersect_model::definitions::rcore_behavior::ir_v2::RetainedValueShapeCursor::scalar_layouts()
    }
    #[cfg(feature = "allocation-profile")]
    pub fn retained_arena_ownership(&self) -> [usize; 5] {
        let state = match &self.root {
            SessionRoot::Borrowed(c) => c.borrow_dependent(),
            SessionRoot::Owned(c) => c.borrow_dependent(),
            SessionRoot::Binding(c) => c.borrow_dependent(),
        };
        [
            state.execution.consumed.value_bytes,
            state.execution.consumed.value_nodes,
            state.execution.nodes.len(),
            state.execution.nodes.capacity(),
            state.source.encoding_nodes().map_or(0, |v| v.len()),
        ]
    }
    #[cfg(test)]
    fn shape(
        &self,
    ) -> Option<&(
        intersect_model::definitions::rcore_behavior::ir_v2::RetainedValueShapeCursor<'_>,
        intersect_model::definitions::rcore_behavior::ir_v2::ValueBudget,
    )> {
        match &self.root {
            SessionRoot::Binding(cell) => cell.borrow_dependent().work.shape(),
            SessionRoot::Borrowed(cell) => cell.borrow_dependent().work.shape(),
            SessionRoot::Owned(cell) => cell.borrow_dependent().work.shape(),
        }
    }

    pub fn new(
        image: &'a VerifiedExecutable,
        arguments: &'a [ValueWire],
        context: &ExecutionContext<'a>,
        limits: &'a ExecutionLimits,
    ) -> Result<Self> {
        let (execution, registry) = initialize(image, context.conversions, limits)?;
        let references = SliceInput::Borrowed(context.references);
        let owner = BorrowedSources {
            registry,
            image,
            arguments,
            bundle: context.bundle,
            limits,
            arena: std::cell::OnceCell::new(),
        };
        let cell = BorrowedSessionCell::new(owner, move |owner| SessionState {
            source: owner,
            references,
            execution,
            decoded_arguments: Vec::new(),
            work: SessionWork::Context(
                crate::rcore_behavior_functions::context::ContextPreparation::new(),
            ),
            phase: Phase::Context,
        });
        Ok(Self {
            root: SessionRoot::Borrowed(cell),
        })
    }
    pub(crate) fn new_binding(
        image: &'a VerifiedExecutable,
        arguments: &'a [ValueWire],
        context: &ExecutionContext<'a>,
        limits: &'a ExecutionLimits,
    ) -> Result<Self> {
        let (execution, registry) = initialize(image, context.conversions, limits)?;
        let references = SliceInput::Borrowed(context.references);
        let owner = BindingSources {
            sources: BorrowedSources {
                registry,
                image,
                arguments,
                bundle: context.bundle,
                limits,
                arena: std::cell::OnceCell::new(),
            },
            result: std::cell::OnceCell::new(),
        };
        let cell = BindingSessionCell::new(owner, move |owner| SessionState {
            source: owner,
            references,
            execution,
            decoded_arguments: Vec::new(),
            work: SessionWork::Context(
                crate::rcore_behavior_functions::context::ContextPreparation::new(),
            ),
            phase: Phase::Context,
        });
        Ok(Self {
            root: SessionRoot::Binding(cell),
        })
    }
    /// A retained JSON transition continues the already charged outer Visit.
    /// It still consumes a polling quantum unit and observes sticky cancellation.
    pub(crate) fn payload_continuation(&self) -> bool {
        let state = match &self.root {
            SessionRoot::Borrowed(cell) => cell.borrow_dependent(),
            SessionRoot::Owned(cell) => cell.borrow_dependent(),
            SessionRoot::Binding(cell) => cell.borrow_dependent(),
        };
        state.payload_continuation()
    }
    #[cfg(feature = "allocation-profile")]
    pub fn retained_shape_layouts() -> (usize, usize) {
        intersect_model::definitions::rcore_behavior::ir_v2::RetainedValueShapeCursor::layouts()
    }
    pub(crate) fn binding_arguments(&self) -> &[ValueWire] {
        match &self.root {
            SessionRoot::Borrowed(cell) => cell.borrow_owner().arguments,
            SessionRoot::Owned(cell) => &cell.borrow_owner().arguments,
            SessionRoot::Binding(cell) => cell.borrow_owner().sources.arguments,
        }
    }
    pub(crate) fn set_binding_result(
        &mut self,
        value: ValueWire,
    ) -> crate::semantic_bindings::Result<()> {
        self.with_state_mut(|state| {
            state.source.install_result(value).map_or(Ok(()), |_| {
                Err(crate::semantic_bindings::Error::new(
                    crate::semantic_bindings::ErrorKind::Contract,
                    "Binding result owner already selected",
                ))
            })
        })
    }
    fn with_state_mut<R>(&mut self, operation: impl FnOnce(&mut SessionState<'_>) -> R) -> R {
        match &mut self.root {
            SessionRoot::Binding(cell) => cell.with_dependent_mut(|_, state| operation(state)),
            SessionRoot::Borrowed(cell) => cell.with_dependent_mut(|_, state| operation(state)),
            SessionRoot::Owned(cell) => cell.with_dependent_mut(|_, state| operation(state)),
        }
    }
    pub fn resume(&mut self, quantum: NonZeroUsize) -> SessionProgress {
        self.resume_with_control(quantum, &NeverCancel)
    }
    pub fn resume_with_control(
        &mut self,
        quantum: NonZeroUsize,
        control: &dyn ExecutionControl,
    ) -> SessionProgress {
        self.with_state_mut(|state| state.resume_with_control(quantum, control))
    }
    pub(crate) fn prepare_binding_step(
        &mut self,
        budget: &crate::semantic_bindings::Budget,
    ) -> crate::semantic_bindings::Result<bool> {
        self.prepare_binding_step_with_work(budget)
    }
    /// Explicit opt-in work authority; source/reference admission is unchanged.
    pub(crate) fn prepare_binding_step_with_work<W: crate::semantic_bindings::BindingWork>(
        &mut self,
        work: &W,
    ) -> crate::semantic_bindings::Result<bool> {
        self.with_state_mut(|state| state.prepare_binding_step_with_work(work))
    }
    pub(crate) fn binding_handle(
        &self,
        value: &ValueWire,
        budget: &crate::semantic_bindings::Budget,
    ) -> crate::semantic_bindings::Result<Option<crate::semantic_bindings::ModelHandle>> {
        match &self.root {
            SessionRoot::Binding(cell) => cell.borrow_dependent().binding_handle(value, budget),
            SessionRoot::Borrowed(cell) => cell.borrow_dependent().binding_handle(value, budget),
            SessionRoot::Owned(cell) => cell.borrow_dependent().binding_handle(value, budget),
        }
    }
    pub(crate) fn finish_binding_step(
        &mut self,
        budget: &crate::semantic_bindings::Budget,
    ) -> crate::semantic_bindings::Result<Option<Evaluation>> {
        self.finish_binding_step_with_work(budget)
    }
    /// Uses the existing result/transport continuation, never another evaluator.
    pub(crate) fn finish_binding_step_with_work<W: crate::semantic_bindings::BindingWork>(
        &mut self,
        work: &W,
    ) -> crate::semantic_bindings::Result<Option<Evaluation>> {
        self.with_state_mut(|state| state.finish_binding_step_with_work(work))
    }
    pub(super) fn finished_error(&mut self) -> ExecutionError {
        self.with_state_mut(|state| state.finished_error())
    }
    #[cfg(test)]
    fn retained_nodes(&self) -> &[crate::rcore_behavior_functions::values::Node] {
        let state = match &self.root {
            SessionRoot::Borrowed(c) => c.borrow_dependent(),
            SessionRoot::Owned(c) => c.borrow_dependent(),
            SessionRoot::Binding(c) => c.borrow_dependent(),
        };
        state
            .source
            .encoding_nodes()
            .unwrap_or(&state.execution.nodes)
    }
    #[cfg(test)]
    fn execution(&self) -> &ExecutionState {
        match &self.root {
            SessionRoot::Binding(cell) => &cell.borrow_dependent().execution,
            SessionRoot::Borrowed(cell) => &cell.borrow_dependent().execution,
            SessionRoot::Owned(cell) => &cell.borrow_dependent().execution,
        }
    }
    #[cfg(test)]
    fn phase(&self) -> Phase {
        match &self.root {
            SessionRoot::Binding(cell) => cell.borrow_dependent().phase,
            SessionRoot::Borrowed(cell) => cell.borrow_dependent().phase,
            SessionRoot::Owned(cell) => cell.borrow_dependent().phase,
        }
    }
    #[cfg(test)]
    fn decoder(&self) -> Option<&crate::rcore_behavior_functions::decoding::Decoding<'_>> {
        match &self.root {
            SessionRoot::Binding(cell) => cell.borrow_dependent().work.decoder(),
            SessionRoot::Borrowed(cell) => cell.borrow_dependent().work.decoder(),
            SessionRoot::Owned(cell) => cell.borrow_dependent().work.decoder(),
        }
    }
    #[cfg(test)]
    fn encoder(&self) -> Option<&crate::rcore_behavior_functions::encoding::Encoding<'_>> {
        match &self.root {
            SessionRoot::Binding(cell) => cell.borrow_dependent().work.encoder(),
            SessionRoot::Borrowed(cell) => cell.borrow_dependent().work.encoder(),
            SessionRoot::Owned(cell) => cell.borrow_dependent().work.encoder(),
        }
    }
    #[cfg(test)]
    fn arguments(&self) -> &[ValueWire] {
        match &self.root {
            SessionRoot::Binding(cell) => cell.borrow_owner().sources.arguments,
            SessionRoot::Borrowed(cell) => cell.borrow_owner().arguments,
            SessionRoot::Owned(cell) => &cell.borrow_owner().arguments,
        }
    }
}
impl EvaluationSession<'static> {
    pub fn new_owned(
        image: VerifiedExecutable,
        arguments: Vec<ValueWire>,
        bundle: DefinitionBundle,
        references: Vec<intersect_model::definitions::conversion::ReferenceContext>,
        registry: Option<intersect_model::definitions::conversion::ConversionRegistry<'static>>,
        limits: ExecutionLimits,
    ) -> Result<Self> {
        let (execution, registry) = initialize(&image, registry.as_ref(), &limits)?;
        let owner = OwnedSources {
            registry,
            image,
            arguments,
            bundle,
            limits,
            arena: std::cell::OnceCell::new(),
        };
        let cell = OwnedSessionCell::new(owner, move |owner| SessionState {
            source: owner,
            references: SliceInput::Owned(references),
            execution,
            decoded_arguments: Vec::new(),
            work: SessionWork::Context(
                crate::rcore_behavior_functions::context::ContextPreparation::new(),
            ),
            phase: Phase::Context,
        });
        Ok(Self {
            root: SessionRoot::Owned(cell),
        })
    }
}

/// Private immutable in-process authority, selected only by checked constructors.
/// Accessors borrow original headers and have no callbacks or semantic behavior.
trait SessionAuthority {
    fn registry(&self)
    -> Option<&intersect_model::definitions::conversion::ConversionRegistry<'_>>;
    fn image(&self) -> &VerifiedExecutable;
    fn arguments(&self) -> &[ValueWire];
    fn bundle(&self) -> &DefinitionBundle;
    fn limits(&self) -> &ExecutionLimits;
    fn encoding_nodes(&self) -> Option<&[crate::rcore_behavior_functions::values::Node]>;
    fn freeze_nodes(
        &self,
        nodes: Vec<crate::rcore_behavior_functions::values::Node>,
    ) -> std::result::Result<(), Vec<crate::rcore_behavior_functions::values::Node>>;
    fn result(&self) -> Option<&ValueWire> {
        None
    }
    fn install_result(&self, value: ValueWire) -> Option<ValueWire> {
        Some(value)
    }
}
impl SessionAuthority for BorrowedSources<'_> {
    fn registry(
        &self,
    ) -> Option<&intersect_model::definitions::conversion::ConversionRegistry<'_>> {
        self.registry.as_ref()
    }
    fn encoding_nodes(&self) -> Option<&[crate::rcore_behavior_functions::values::Node]> {
        self.arena.get().map(Vec::as_slice)
    }
    fn freeze_nodes(
        &self,
        nodes: Vec<crate::rcore_behavior_functions::values::Node>,
    ) -> std::result::Result<(), Vec<crate::rcore_behavior_functions::values::Node>> {
        self.arena.set(nodes)
    }

    fn image(&self) -> &VerifiedExecutable {
        self.image
    }
    fn arguments(&self) -> &[ValueWire] {
        self.arguments
    }
    fn bundle(&self) -> &DefinitionBundle {
        self.bundle
    }
    fn limits(&self) -> &ExecutionLimits {
        self.limits
    }
}
impl SessionAuthority for OwnedSources {
    fn registry(
        &self,
    ) -> Option<&intersect_model::definitions::conversion::ConversionRegistry<'_>> {
        self.registry.as_ref()
    }
    fn encoding_nodes(&self) -> Option<&[crate::rcore_behavior_functions::values::Node]> {
        self.arena.get().map(Vec::as_slice)
    }
    fn freeze_nodes(
        &self,
        nodes: Vec<crate::rcore_behavior_functions::values::Node>,
    ) -> std::result::Result<(), Vec<crate::rcore_behavior_functions::values::Node>> {
        self.arena.set(nodes)
    }

    fn image(&self) -> &VerifiedExecutable {
        &self.image
    }
    fn arguments(&self) -> &[ValueWire] {
        &self.arguments
    }
    fn bundle(&self) -> &DefinitionBundle {
        &self.bundle
    }
    fn limits(&self) -> &ExecutionLimits {
        &self.limits
    }
}
impl SessionAuthority for BindingSources<'_> {
    fn registry(
        &self,
    ) -> Option<&intersect_model::definitions::conversion::ConversionRegistry<'_>> {
        self.sources.registry()
    }
    fn encoding_nodes(&self) -> Option<&[crate::rcore_behavior_functions::values::Node]> {
        self.sources.arena.get().map(Vec::as_slice)
    }
    fn freeze_nodes(
        &self,
        nodes: Vec<crate::rcore_behavior_functions::values::Node>,
    ) -> std::result::Result<(), Vec<crate::rcore_behavior_functions::values::Node>> {
        self.sources.arena.set(nodes)
    }

    fn image(&self) -> &VerifiedExecutable {
        self.sources.image
    }
    fn arguments(&self) -> &[ValueWire] {
        self.sources.arguments
    }
    fn bundle(&self) -> &DefinitionBundle {
        self.sources.bundle
    }
    fn limits(&self) -> &ExecutionLimits {
        self.sources.limits
    }
    fn result(&self) -> Option<&ValueWire> {
        self.result.get()
    }
    fn install_result(&self, value: ValueWire) -> Option<ValueWire> {
        self.result.set(value).err()
    }
}
enum SessionWork<'a, I: super::inputs::InputFamily = super::inputs::NoBorrowedModelInputs> {
    Idle,
    Context(crate::rcore_behavior_functions::context::ContextPreparation),
    Shape(
        (
            intersect_model::definitions::rcore_behavior::ir_v2::RetainedValueShapeCursor<'a>,
            intersect_model::definitions::rcore_behavior::ir_v2::ValueBudget,
        ),
    ),
    Decode(crate::rcore_behavior_functions::decoding::Decoding<'a, I>),
    Encode(crate::rcore_behavior_functions::encoding::Encoding<'a>),
}
impl<I: super::inputs::InputFamily> SessionWork<'_, I> {
    fn shape(
        &self,
    ) -> Option<&(
        intersect_model::definitions::rcore_behavior::ir_v2::RetainedValueShapeCursor<'_>,
        intersect_model::definitions::rcore_behavior::ir_v2::ValueBudget,
    )> {
        match self {
            Self::Shape(value) => Some(value),
            _ => None,
        }
    }
    #[cfg(test)]
    fn decoder(&self) -> Option<&crate::rcore_behavior_functions::decoding::Decoding<'_, I>> {
        match self {
            Self::Decode(value) => Some(value),
            _ => None,
        }
    }
    #[cfg(test)]
    fn encoder(&self) -> Option<&crate::rcore_behavior_functions::encoding::Encoding<'_>> {
        match self {
            Self::Encode(value) => Some(value),
            _ => None,
        }
    }
}
struct SessionState<'a, I: super::inputs::InputFamily = super::inputs::NoBorrowedModelInputs> {
    source: &'a dyn SessionAuthority,
    references: SliceInput<'a, intersect_model::definitions::conversion::ReferenceContext>,
    execution: ExecutionState<I>,
    decoded_arguments: Vec<Id>,
    work: SessionWork<'a, I>,
    phase: Phase,
}
impl<'a, I: super::inputs::InputFamily> SessionState<'a, I> {
    fn payload_continuation(&self) -> bool {
        self.work
            .shape()
            .is_some_and(|(shape, _)| shape.continues_visit())
            || matches!(&self.work,SessionWork::Decode(decoder)if decoder.added_payload_continuation())
            || matches!(&self.work,SessionWork::Encode(encoder)if encoder.added_payload_continuation())
    }

    /// Source context and all arguments use the normal admission machine.
    /// Stop before entering the reference body; external implementations cannot
    /// manufacture an admitted model or bypass nominal/multiplicity validation.
    fn prepare_binding_step_with_work<W: crate::semantic_bindings::BindingWork>(
        &mut self,
        work: &W,
    ) -> crate::semantic_bindings::Result<bool> {
        if matches!(self.phase, Phase::Entry { index } if index == self.source.arguments().len()) {
            return Ok(true);
        }
        self.advance(work)?;
        Ok(false)
    }
    pub(crate) fn binding_handle(
        &self,
        value: &ValueWire,
        budget: &crate::semantic_bindings::Budget,
    ) -> crate::semantic_bindings::Result<Option<crate::semantic_bindings::ModelHandle>> {
        let intersect_model::definitions::rcore_behavior::ir_v2::DataWire::Model(payload) =
            &value.data
        else {
            return Ok(None);
        };
        let (digest, _) = crate::model_reference::reference_identity(payload).map_err(|e| {
            crate::semantic_bindings::Error::new(crate::semantic_bindings::ErrorKind::Arguments, e)
        })?;
        let domain = I::owned(&self.execution.domains)
            .map_err(|message| {
                crate::semantic_bindings::Error::new(
                    crate::semantic_bindings::ErrorKind::Unsupported,
                    message,
                )
            })?
            .get(&(digest.to_owned(), value.ty))
            .ok_or_else(|| {
                crate::semantic_bindings::Error::new(
                    crate::semantic_bindings::ErrorKind::Arguments,
                    "Model argument has no admitted domain",
                )
            })?;
        budget.charge(1)?;
        Ok(Some(crate::semantic_bindings::ModelHandle {
            domain: std::rc::Rc::new(domain.clone()),
            wire: value.clone(),
            budget: budget.clone(),
        }))
    }
    /// Validate an external result through the same shape, nominal, reference,
    /// collection and transport machinery, retaining admission resource counters.
    fn finish_binding_step_with_work<W: crate::semantic_bindings::BindingWork>(
        &mut self,
        work: &W,
    ) -> crate::semantic_bindings::Result<Option<Evaluation>> {
        if matches!(self.phase, Phase::Encoding(_)) {
            let result = self.advance(work)?;
            if result.is_some() {
                self.phase = Phase::Finished;
            }
            return Ok(result);
        }
        let value = self.source.result().ok_or_else(|| {
            crate::semantic_bindings::Error::new(
                crate::semantic_bindings::ErrorKind::Contract,
                "Missing stable binding result",
            )
        })?;
        let context = ExecutionContext {
            bundle: self.source.bundle(),
            conversions: self.source.registry(),
            references: &self.references,
        };
        let mut state = State {
            image: ExecutionImage::Functions(self.source.image()),
            context: Some(&context),
            limits: self.source.limits(),
            control: Some(work),
            data: &mut self.execution,
        };
        let ty = self
            .source
            .image()
            .program()
            .functions
            .get(self.source.image().program().entry.0 as usize)
            .ok_or_else(|| state.invalid("Missing binding result type", None))?
            .result;
        if let Some(id) =
            state.prepare_value(&mut self.work, value, ty, self.source.registry(), None)?
        {
            self.phase = Phase::Encoding(id);
        }
        Ok(None)
    }
    /// Observe cancellation before each phase unit and at structural/numeric checkpoints.
    /// Provider and backend numeric calls remain atomic; arbitrary interruption is not promised.
    /// An error returned before the next checkpoint wins over later cancellation.
    pub fn resume_with_control(
        &mut self,
        quantum: NonZeroUsize,
        control: &dyn ExecutionControl,
    ) -> SessionProgress {
        if matches!(self.phase, Phase::Finished) {
            return SessionProgress::Finished;
        }
        for _ in 0..quantum.get() {
            if control.cancellation_requested() {
                let error = self.cancelled_error();
                self.phase = Phase::Finished;
                return SessionProgress::Complete(Box::new(Err(error)));
            }
            match self.advance(control) {
                Ok(Some(value)) => {
                    self.phase = Phase::Finished;
                    return SessionProgress::Complete(Box::new(Ok(value)));
                }
                Ok(None) => {}
                Err(error) => {
                    self.phase = Phase::Finished;
                    return SessionProgress::Complete(Box::new(Err(error)));
                }
            }
        }
        SessionProgress::Pending
    }
    fn cancelled_error(&mut self) -> ExecutionError {
        // A checkpoint is before the next unit, so preparation provenance must
        // name that unit rather than whichever function the last unit inspected.
        if let Phase::Constants { function, .. } = self.phase {
            self.execution.preparing = self
                .source
                .image()
                .program()
                .functions
                .get(function)
                .map(|_| FunctionId(function as u32));
        }
        let image = ExecutionImage::Functions(self.source.image());
        let span = match self.phase {
            Phase::Constants {
                function,
                block,
                instruction,
            } => image
                .block(function, block)
                .and_then(|block| block.instructions.get(instruction).map(|i| i.span)),
            Phase::Running => self.execution.frames.last().and_then(|frame| {
                if let Some(site) = frame.numeric_site() {
                    return image.call_span(site);
                }
                if let Some(pending) = &frame.operands {
                    return pending.source.span(image);
                }
                if let Some(pending) = &frame.invocation {
                    return image.call_span(pending.site);
                }
                image
                    .block(frame.function.0 as usize, frame.block)
                    .map(|block| {
                        block
                            .instructions
                            .get(frame.pc)
                            .map_or(block.span, |i| i.span)
                    })
            }),
            _ => None,
        };
        let context = ExecutionContext {
            bundle: self.source.bundle(),
            conversions: self.source.registry(),
            references: &self.references,
        };
        State {
            image,
            context: Some(&context),
            limits: self.source.limits(),
            control: None,
            data: &mut self.execution,
        }
        .error(
            ExecutionErrorKind::Cancelled,
            "Evaluation cancelled by host",
            span,
        )
    }
    pub(super) fn finished_error(&mut self) -> ExecutionError {
        let context = ExecutionContext {
            bundle: self.source.bundle(),
            conversions: self.source.registry(),
            references: &self.references,
        };
        State {
            image: ExecutionImage::Functions(self.source.image()),
            context: Some(&context),
            limits: self.source.limits(),
            control: None,
            data: &mut self.execution,
        }
        .invalid("Evaluation session has already finished", None)
    }
    fn transfer_reference_input(&mut self, control: &dyn ExecutionControl) -> Result<()> {
        // ContextPreparation has checked every input before this phase. The
        // transient view deliberately has no reference slice while one owned
        // value moves, so it cannot borrow the vector being consumed.
        let context = ExecutionContext {
            bundle: self.source.bundle(),
            conversions: self.source.registry(),
            references: &[],
        };
        let state = State {
            image: ExecutionImage::Functions(self.source.image()),
            context: Some(&context),
            limits: self.source.limits(),
            control: Some(control),
            data: &mut self.execution,
        };
        state.checkpoint(None)?;
        let SliceInput::Owned(references) = &mut self.references else {
            return Err(state.invalid("Owned reference transfer requires owned inputs", None));
        };
        if references.is_empty() {
            state
                .data
                .owned_reference_inputs
                .get_or_insert_with(BTreeMap::new);
            self.phase = Phase::Constants {
                function: 0,
                block: 0,
                instruction: 0,
            };
            return Ok(());
        }
        // Only the map record is new storage. The context's source text and
        // model are moved from the original vector; neither is copied here.
        let observation = state.invocation_observation(None)?;
        observation
            .work(1)
            .map_err(|e| state.provider_error(e, None))?;
        observation
            .retain_storage(OWNED_REFERENCE_INPUT_BYTES)
            .map_err(|e| state.provider_error(e, None))?;
        state.check_provider_status(None)?;
        let index = references.len() - 1;
        let moved = references
            .pop()
            .ok_or_else(|| state.invalid("Missing owned reference input", None))?;
        state
            .data
            .owned_reference_inputs
            .get_or_insert_with(BTreeMap::new)
            .insert(index, moved);
        Ok(())
    }
    fn advance(&mut self, control: &dyn ExecutionControl) -> Result<Option<Evaluation>> {
        if matches!(self.phase, Phase::TransferReferences) {
            self.transfer_reference_input(control)?;
            return Ok(None);
        }
        let context = ExecutionContext {
            bundle: self.source.bundle(),
            conversions: self.source.registry(),
            references: &self.references,
        };
        let mut state = State {
            image: ExecutionImage::Functions(self.source.image()),
            context: Some(&context),
            limits: self.source.limits(),
            control: Some(control),
            data: &mut self.execution,
        };
        match self.phase {
            Phase::TransferReferences => {
                return Err(state.invalid("Unexpected reference transfer view", None));
            }
            Phase::Context => {
                let SessionWork::Context(preparation) = &mut self.work else {
                    return Err(state.invalid("Missing context preparation", None));
                };
                if !preparation.advance(&mut state)? {
                    return Ok(None);
                }
                // The completed context cursor has no retained model work; reuse
                // the same fixed phase slot without new storage credit or fee.
                self.work = SessionWork::Idle;
                self.phase = if matches!(&self.references, SliceInput::Owned(_)) {
                    Phase::TransferReferences
                } else {
                    Phase::Constants {
                        function: 0,
                        block: 0,
                        instruction: 0,
                    }
                };
            }
            Phase::Constants {
                function: fi,
                block: bi,
                instruction: ii,
            } => {
                if let Some(function) = self.source.image().program().functions.get(fi) {
                    state.data.preparing = Some(FunctionId(fi as u32));
                    if let Some(block) = function.blocks.get(bi) {
                        if let Some(ins) = block.instructions.get(ii) {
                            if let InstructionKind::Constant(value) = &ins.kind {
                                let Some(id) = state.prepare_value(
                                    &mut self.work,
                                    value,
                                    ins.ty,
                                    self.source.registry(),
                                    Some(&ins.span),
                                )?
                                else {
                                    return Ok(None);
                                };
                                state.data.constants.insert((fi, bi, ii), id);
                            }
                            self.phase = Phase::Constants {
                                function: fi,
                                block: bi,
                                instruction: ii + 1,
                            };
                        } else {
                            self.phase = Phase::Constants {
                                function: fi,
                                block: bi + 1,
                                instruction: 0,
                            };
                        }
                    } else {
                        self.phase = Phase::Constants {
                            function: fi + 1,
                            block: 0,
                            instruction: 0,
                        };
                    }
                } else {
                    state.data.preparing = None;
                    let entry = self
                        .source
                        .image()
                        .program()
                        .functions
                        .get(self.source.image().program().entry.0 as usize)
                        .ok_or_else(|| state.invalid("Missing entry function", None))?;
                    if entry.parameters.len() != self.source.arguments().len() {
                        return Err(state.error(
                            ExecutionErrorKind::Arguments,
                            "Function argument count differs from signature",
                            None,
                        ));
                    }
                    self.decoded_arguments = Vec::with_capacity(self.source.arguments().len());
                    self.phase = Phase::Arguments { index: 0 };
                }
            }
            Phase::Arguments { index } => {
                let entry = self
                    .source
                    .image()
                    .program()
                    .functions
                    .get(self.source.image().program().entry.0 as usize)
                    .ok_or_else(|| state.invalid("Missing entry function", None))?;
                if let Some(wire) = self.source.arguments().get(index) {
                    let ty = *entry
                        .parameters
                        .get(index)
                        .ok_or_else(|| state.invalid("Missing argument type", None))?;
                    let Some(id) = state.prepare_value(
                        &mut self.work,
                        wire,
                        ty,
                        self.source.registry(),
                        None,
                    )?
                    else {
                        return Ok(None);
                    };
                    self.decoded_arguments.push(id);
                    self.phase = Phase::Arguments { index: index + 1 };
                } else {
                    state.frame_signature(
                        self.source.image().program().entry,
                        self.decoded_arguments.len(),
                        0,
                        None,
                    )?;
                    self.phase = Phase::Entry { index: 0 };
                }
            }
            Phase::Entry { index } => {
                state.checkpoint(None)?;
                let entry = self
                    .source
                    .image()
                    .program()
                    .functions
                    .get(self.source.image().program().entry.0 as usize)
                    .ok_or_else(|| state.invalid("Missing entry function", None))?;
                if let Some(id) = self.decoded_arguments.get(index).copied() {
                    let ty = entry
                        .parameters
                        .get(index)
                        .copied()
                        .ok_or_else(|| state.invalid("Missing entry parameter type", None))?;
                    state.permit(state.node(id)?.ty, ty, None)?;
                    self.phase = Phase::Entry { index: index + 1 };
                } else {
                    state.enter_frame(
                        self.source.image().program().entry,
                        std::mem::take(&mut self.decoded_arguments),
                        vec![],
                        None,
                        entry.result,
                    );
                    self.phase = Phase::Running;
                }
            }
            Phase::Running => {
                if let Some(result) = state.run_slice(NonZeroUsize::MIN)? {
                    self.phase = Phase::Encoding(result);
                }
            }
            Phase::Encoding(result) => {
                if matches!(self.work, SessionWork::Idle) {
                    let encoder = crate::rcore_behavior_functions::encoding::Encoding::new(
                        &mut state, result,
                    )?;
                    // The original encoder header/frame admission happens before
                    // arena ownership moves. Moving Vec retains its allocation,
                    // IDs and payload owners; no leaf is cloned or published.
                    self.source
                        .freeze_nodes(std::mem::take(&mut state.data.nodes))
                        .map_err(|_| state.invalid("Encoding arena already selected", None))?;
                    self.work = SessionWork::Encode(encoder);
                }
                let SessionWork::Encode(encoder) = &mut self.work else {
                    return Err(state.invalid("Missing result encoder", None));
                };
                let nodes = self
                    .source
                    .encoding_nodes()
                    .ok_or_else(|| state.invalid("Missing encoding arena owner", None))?;
                let Some(value) = encoder.advance(&mut state, nodes)? else {
                    return Ok(None);
                };
                self.work = SessionWork::Idle;
                state.check_provider_status(None)?;
                state.data.consumed.numeric_work = state.data.numeric.work_used();
                state
                    .data
                    .provider_receipts
                    .counters(&mut state.data.consumed);
                let (selected_providers, consumed_providers) = state
                    .data
                    .provider_receipts
                    .take()
                    .map_err(|e| state.invalid(&e, None))?;
                return Ok(Some(Evaluation {
                    observation: state.data.observation.finish(),
                    value,
                    selected_providers,
                    consumed_providers,
                    consumed: std::mem::take(&mut state.data.consumed),
                }));
            }
            Phase::Finished => {
                return Err(state.invalid("Evaluation session has already finished", None));
            }
        }
        Ok(None)
    }
}

fn initialize<'providers>(
    image: &VerifiedExecutable,
    registry: Option<&intersect_model::definitions::conversion::ConversionRegistry<'providers>>,
    limits: &ExecutionLimits,
) -> Result<(
    ExecutionState,
    Option<intersect_model::definitions::conversion::ConversionRegistry<'providers>>,
)> {
    let mut execution = ExecutionState {
        observation: super::observation::Recorder::new(&limits.observation),
        consumed: Consumption::default(),
        numeric: NumericBudget::new(limits.scalar.numeric.clone()),
        nodes: vec![],
        closures: vec![],
        frames: vec![],
        constants: BTreeMap::new(),
        preparing: None,
        provider_receipts: std::rc::Rc::new(super::providers::Receipts::new(limits)),
        domains: BTreeMap::new(),
        contexts: BTreeMap::new(),
        datatypes: BTreeMap::new(),
        reference_inputs: BTreeMap::new(),
        owned_reference_inputs: None,
    };
    let state = State {
        image: ExecutionImage::Functions(image),
        context: None,
        limits,
        control: None,
        data: &mut execution,
    };
    // Verified selection evidence precedes fallible context preparation.
    for contract in &image.program().policies.conversions {
        state
            .data
            .provider_receipts
            .select(&contract.provider)
            .map_err(|e| state.resource(&e, None))?;
    }
    for contract in &image.program().policies.applied {
        state
            .data
            .provider_receipts
            .select(&contract.conversion.provider)
            .map_err(|e| state.resource(&e, None))?;
    }
    for contract in &image.program().policies.constructors {
        state
            .data
            .provider_receipts
            .select(&contract.provider)
            .map_err(|e| state.resource(&e, None))?;
    }
    // Check retained metadata before cloning an observed registry. This adapter
    // owns only an evaluation-local observer; it cannot alter the caller's registry.
    if let Some(registry) = registry {
        state.data.consumed.context_bytes = registry
            .observation_copy_bytes(
                limits.max_context_bytes,
                limits.max_value_nodes,
                limits.max_value_depth,
            )
            .map_err(|e| state.resource(&e, None))?;
    }
    let observation = intersect_model::definitions::conversion::InvocationContext::new(
        state.data.provider_receipts.clone(),
    );
    let observed_registry = registry.map(|registry| registry.clone().with_observation(observation));
    Ok((execution, observed_registry))
}

impl<I: super::inputs::InputFamily> State<'_, '_, '_, I> {
    fn prepare_value<'a>(
        &mut self,
        work: &mut SessionWork<'a, I>,
        wire: &'a ValueWire,
        expected: TypeId,
        registry: Option<&'a intersect_model::definitions::conversion::ConversionRegistry<'a>>,
        span: Option<&SourceSpan>,
    ) -> Result<Option<Id>> {
        if matches!(work, SessionWork::Idle | SessionWork::Shape(_)) {
            if !self.advance_shape(work, wire, expected, span)? {
                return Ok(None);
            }
            *work = SessionWork::Decode(crate::rcore_behavior_functions::decoding::Decoding::new(
                self, expected, wire, span,
            )?);
            return Ok(None);
        }
        let SessionWork::Decode(decoder) = work else {
            return Err(self.invalid("Missing value decoder", span));
        };
        let result = decoder.advance(self, wire, registry, span)?;
        if result.is_some() {
            *work = SessionWork::Idle;
        }
        Ok(result)
    }

    fn shape_error(
        &self,
        error: intersect_model::definitions::rcore_behavior::ir::IrError,
        span: Option<&SourceSpan>,
    ) -> ExecutionError {
        self.error(
            if error.kind
                == intersect_model::definitions::rcore_behavior::ir::IrErrorKind::ResourceExhausted
            {
                ExecutionErrorKind::ResourceExhausted
            } else {
                ExecutionErrorKind::Arguments
            },
            error.message,
            span,
        )
    }
    fn advance_shape<'a>(
        &mut self,
        work: &mut SessionWork<'a, I>,
        wire: &'a ValueWire,
        expected: TypeId,
        span: Option<&SourceSpan>,
    ) -> Result<bool> {
        use intersect_model::definitions::rcore_behavior::ir_v2::{
            RetainedValueShapeCursor, ValueBudget,
        };
        self.checkpoint(span)?;
        if matches!(work, SessionWork::Idle) {
            let available = self
                .limits
                .scalar
                .max_value_bytes
                .saturating_sub(self.data.consumed.value_bytes);
            let mut budget = ValueBudget {
                remaining_nodes: self
                    .limits
                    .max_value_nodes
                    .saturating_sub(self.data.consumed.value_nodes),
                remaining_bytes: available,
                max_depth: self.limits.max_value_depth,
                scalar: self.limits.scalar.values.clone(),
            };
            let cursor = RetainedValueShapeCursor::new(expected, &mut budget, available)
                .map_err(|e| self.shape_error(e, span))?;
            self.retained_storage(cursor.storage_bytes(), span)?;
            *work = SessionWork::Shape((cursor, budget));
        }
        let SessionWork::Shape((cursor, budget)) = work else {
            return Err(self.invalid("Missing shape cursor", span));
        };
        let before = cursor.storage_bytes();
        let result = cursor.advance(
            self.function_image()?,
            wire,
            budget,
            &mut crate::rcore_behavior_functions::values::ShapeOnly,
        );
        self.retained_storage(cursor.storage_bytes().saturating_sub(before), span)?;
        let done = result.map_err(|e| self.shape_error(e, span))?;
        if done {
            *work = SessionWork::Idle;
        }
        Ok(done)
    }
}

#[cfg(test)]
#[path = "rcore_behavior_functions_control_tests.rs"]
mod control_tests;

#[cfg(test)]
mod nested_comparison_tests {
    use super::*;
    use intersect_model::definitions::{
        bundle::{DefinitionSource, SourceKind},
        rcore_behavior::{
            function_lowering::compile_function,
            ir,
            ir_v2::{DataWire, EntrySelection, Limits},
            modules::SymbolId,
            types::Scalar,
        },
    };
    #[test]
    fn nested_comparisons_advance_at_most_one_value_pair_per_session_tick() {
        for expression in [
            "contains(xs,needle)",
            "size(Set(xs))==1",
            "size(OrderedSet(xs))==1",
            "size(Bag(xs))==2",
            "size(Sequence(xs))==2",
            "size(intersection(xs,[needle]))==1",
            "size(difference(xs,[needle]))==1",
            "size(distinct(xs))==1",
            "size(groupBy(xs,|x|x))==1",
        ] {
            let source = DefinitionSource::new(
                "memory:comparison-slices-behavior.rcore",
                SourceKind::RcoreBehavior,
                format!(
                    "rcore 1;package comparison_slices version \"1\";fn run(xs:Sequence<Bag<Int>>,needle:Bag<Int>)->Bool={expression};"
                ),
            );
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source.clone()],
                namespaces: vec![],
            };
            let image = compile_function(
                &bundle,
                &source.id,
                &EntrySelection {
                    symbol: SymbolId {
                        source: source.id.clone(),
                        declaration: 0,
                    },
                    types: vec![],
                    arguments: vec![],
                },
                &Limits::default(),
            )
            .unwrap();
            let program = image.program();
            let parameters = &program.functions[program.entry.0 as usize].parameters;
            let TypeNode::Collection { element, .. } = program.types[parameters[1].0 as usize]
            else {
                panic!("Bag")
            };
            let bag = |reverse: bool| {
                let mut numbers: Vec<_> = (0..12).collect();
                if reverse {
                    numbers.reverse();
                }
                ValueWire{ty:parameters[1],data:DataWire::Collection(numbers.into_iter().map(|n|ValueWire{ty:element,data:DataWire::Scalar(ir::Value{ty:ir::ValueType::Scalar(Scalar::Int64),payload:serde_json::json!({"kind":"int64","value":n.to_string()})})}).collect())}
            };
            let arguments = [
                ValueWire {
                    ty: parameters[0],
                    data: DataWire::Collection(vec![bag(false), bag(true)]),
                },
                bag(true),
            ];
            let context = ExecutionContext::new(&bundle);
            let limits = ExecutionLimits::default();
            let baseline = evaluate(&image, &arguments, &context, &limits).unwrap();
            if !expression.contains("Bag(xs)") && !expression.contains("Sequence(xs)") {
                assert!(baseline.consumed.collection_comparisons > 12);
            }
            assert_eq!(
                serde_json::to_value(&baseline.value).unwrap()["data"]["Scalar"]["payload"],
                true
            );
            let mut boundaries = 0;
            for quantum in [1, 7, 4096] {
                let mut session =
                    EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
                let mut before = 0;
                let mut pending = 0;
                loop {
                    match session.resume(NonZeroUsize::new(quantum).unwrap()) {
                        SessionProgress::Pending => {
                            let after = session.execution().consumed.collection_comparisons;
                            if quantum == 1 {
                                assert!(after - before <= 1, "{expression}: {before} -> {after}");
                            }
                            before = after;
                            pending += 1;
                            assert!(pending < 10000);
                        }
                        SessionProgress::Complete(result) => {
                            assert_eq!(
                                serde_json::to_value(result.unwrap()).unwrap(),
                                serde_json::to_value(&baseline).unwrap()
                            );
                            break;
                        }
                        SessionProgress::Finished => panic!("Missing result"),
                    }
                }
                if quantum == 1 {
                    assert!(pending > baseline.consumed.collection_comparisons);
                    boundaries = pending;
                }
            }
            if expression == "contains(xs,needle)" || expression == "size(Set(xs))==1" {
                struct Cancel;
                impl ExecutionControl for Cancel {
                    fn cancellation_requested(&self) -> bool {
                        true
                    }
                }
                for boundary in 0..=boundaries {
                    let mut session =
                        EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
                    for _ in 0..boundary {
                        assert!(matches!(
                            session.resume(NonZeroUsize::MIN),
                            SessionProgress::Pending
                        ));
                    }
                    let SessionProgress::Complete(result) =
                        session.resume_with_control(NonZeroUsize::MIN, &Cancel)
                    else {
                        panic!("Missing cancellation")
                    };
                    assert_eq!(result.unwrap_err().kind, ExecutionErrorKind::Cancelled);
                    assert!(matches!(
                        session.resume(NonZeroUsize::MAX),
                        SessionProgress::Finished
                    ));
                }
                for dimension in 0..3 {
                    let amount = match dimension {
                        0 => baseline.consumed.steps,
                        1 => baseline.consumed.value_bytes,
                        _ => baseline.consumed.collection_comparisons,
                    };
                    for bound in [0, amount - 1, amount, amount + 1] {
                        let mut limits = ExecutionLimits::default();
                        match dimension {
                            0 => limits.scalar.max_steps = bound,
                            1 => limits.scalar.max_value_bytes = bound,
                            _ => limits.max_collection_comparisons = bound,
                        };
                        let result = evaluate(&image, &arguments, &context, &limits);
                        assert_eq!(
                            result.is_ok(),
                            bound >= amount,
                            "dimension {dimension} bound {bound}: {result:?}"
                        );
                        if let Err(error) = result {
                            assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
                        }
                    }
                }
                assert_eq!(
                    serde_json::to_value(evaluate(&image, &arguments, &context, &limits).unwrap())
                        .unwrap(),
                    serde_json::to_value(&baseline).unwrap()
                );
            }
        }
    }
    #[test]
    fn source_record_equality_yields_between_fields_and_retains_inequality() {
        let fields = (0..24)
            .map(|n| format!("f{n}:Int"))
            .collect::<Vec<_>>()
            .join(",");
        for (operator, different, expected) in
            [("==", false, true), ("==", true, false), ("!=", true, true)]
        {
            let source = DefinitionSource::new(
                "memory:record-slices-behavior.rcore",
                SourceKind::RcoreBehavior,
                format!(
                    "rcore 1;package record_slices version \"1\";fn run(a:Option<{{{fields}}}>,b:Option<{{{fields}}}>)->Bool=a{operator}b;"
                ),
            );
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source.clone()],
                namespaces: vec![],
            };
            let image = compile_function(
                &bundle,
                &source.id,
                &EntrySelection {
                    symbol: SymbolId {
                        source: source.id.clone(),
                        declaration: 0,
                    },
                    types: vec![],
                    arguments: vec![],
                },
                &Limits::default(),
            )
            .unwrap();
            let program = image.program();
            let ty = program.functions[program.entry.0 as usize].parameters[0];
            let TypeNode::Option(record) = program.types[ty.0 as usize] else {
                panic!("Option")
            };
            let TypeNode::Record(fields) = &program.types[record.0 as usize] else {
                panic!("Record")
            };
            let value = |different: bool| {
                ValueWire{ty,data:DataWire::Some(Box::new(ValueWire{ty:record,data:DataWire::Record(fields.iter().map(|(name,ty)|(name.clone(),ValueWire{ty:*ty,data:DataWire::Scalar(ir::Value{ty:ir::ValueType::Scalar(Scalar::Int64),payload:serde_json::json!({"kind":"int64","value":if different && name=="f9" {"2"}else{"1"}})})})).collect())}))}
            };
            let arguments = [value(false), value(different)];
            let limits = ExecutionLimits::default();
            let context = ExecutionContext::new(&bundle);
            let baseline = evaluate(&image, &arguments, &context, &limits).unwrap();
            assert_eq!(
                serde_json::to_value(&baseline.value).unwrap()["data"]["Scalar"]["payload"],
                expected
            );
            let mut session =
                EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
            let mut before = 0;
            loop {
                match session.resume(NonZeroUsize::MIN) {
                    SessionProgress::Pending => {
                        let after = session.execution().consumed.collection_comparisons;
                        assert!(after - before <= 1);
                        before = after;
                    }
                    SessionProgress::Complete(result) => {
                        assert_eq!(
                            serde_json::to_value(result.unwrap()).unwrap(),
                            serde_json::to_value(&baseline).unwrap()
                        );
                        break;
                    }
                    SessionProgress::Finished => panic!("Missing result"),
                }
            }
        }
    }
    #[test]
    fn aggregate_constructors_suspend_copying_and_reject_nested_duplicates() {
        use intersect_model::definitions::rcore_behavior::ir_v2::{self, InstructionKind, Operand};
        struct Cancel;
        impl ExecutionControl for Cancel {
            fn cancellation_requested(&self) -> bool {
                true
            }
        }
        let fields = (0..24)
            .map(|i| format!("f{i}:a"))
            .collect::<Vec<_>>()
            .join(",");
        let copies = vec!["a"; 24].join(",");
        for (body, unique) in [
            (
                format!("fn run(a:Bag<Int>,b:Bag<Int>)->Bool=size([{copies}])==24;"),
                false,
            ),
            (
                format!("fn run(a:Bag<Int>,b:Bag<Int>)->Bool={{let x=({copies});size(x[0])==6}};"),
                false,
            ),
            (
                format!(
                    "fn run(a:Bag<Int>,b:Bag<Int>)->Bool={{let x={{{fields}}};size(x.f0)==6}};"
                ),
                false,
            ),
            (
                "fn run(a:Bag<Int>,b:Bag<Int>)->Set<Bag<Int>> =Set([a,b]);".to_owned(),
                true,
            ),
            (
                "fn run(a:Bag<Int>,b:Bag<Int>)->OrderedSet<Bag<Int>> =OrderedSet([a,b]);"
                    .to_owned(),
                true,
            ),
        ] {
            let source = DefinitionSource::new(
                "memory:construct-behavior.rcore",
                SourceKind::RcoreBehavior,
                format!("rcore 1;package construct version \"1\";{body}"),
            );
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source.clone()],
                namespaces: vec![],
            };
            let compiled = compile_function(
                &bundle,
                &source.id,
                &EntrySelection {
                    symbol: SymbolId {
                        source: source.id.clone(),
                        declaration: 0,
                    },
                    types: vec![],
                    arguments: vec![],
                },
                &Limits::default(),
            )
            .unwrap();
            let mut program = compiled.program().clone();
            if unique {
                let instruction = program
                    .functions
                    .iter_mut()
                    .flat_map(|f| &mut f.blocks)
                    .flat_map(|b| &mut b.instructions)
                    .find(|i| matches!(i.kind, InstructionKind::ConvertCollection { .. }))
                    .unwrap();
                instruction.kind =
                    InstructionKind::Collection(vec![Operand::Parameter(0), Operand::Parameter(1)]);
            }
            let image = ir_v2::verify(program, &bundle, &Limits::default()).unwrap();
            let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
            let TypeNode::Collection { element, .. } = image.program().types[ty.0 as usize] else {
                panic!("Bag");
            };
            for different in [false, true] {
                let bag = |reverse: bool| {
                    ValueWire { ty, data: DataWire::Collection((0..6).map(|i| {
                    let n = if reverse {5-i} else {i};
                    let n = n + if reverse && different {1} else {0};
                    ValueWire {ty: element, data: DataWire::Scalar(ir::Value {ty: ir::ValueType::Scalar(Scalar::Int64), payload: serde_json::json!({"kind":"int64","value":n.to_string()})})}
                }).collect()) }
                };
                let arguments = [bag(false), bag(true)];
                let context = ExecutionContext::new(&bundle);
                let limits = ExecutionLimits::default();
                let baseline = evaluate(&image, &arguments, &context, &limits);
                if unique && !different {
                    let error = baseline.as_ref().unwrap_err();
                    assert_eq!(error.kind, ExecutionErrorKind::Arguments);
                    assert_eq!(&*error.message, "Duplicate value in unique collection");
                } else {
                    assert!(baseline.is_ok(), "{baseline:?}");
                }
                let mut boundaries = 0;
                for quantum in [1, 7, 4096] {
                    let mut session =
                        EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
                    let mut before = 0;
                    let mut pending = 0;
                    loop {
                        let progress = session.resume(NonZeroUsize::new(quantum).unwrap());
                        let after = match &progress {
                            SessionProgress::Complete(result) => match result.as_ref() {
                                Ok(value) => value.consumed.collection_comparisons,
                                Err(error) => error.consumed.collection_comparisons,
                            },
                            _ => session.execution().consumed.collection_comparisons,
                        };
                        if quantum == 1 {
                            assert!(after - before <= 1, "{body}: {before}->{after}");
                        }
                        before = after;
                        match progress {
                            SessionProgress::Pending => {
                                pending += 1;
                                assert!(pending < 10000);
                            }
                            SessionProgress::Complete(result) => {
                                assert_eq!(
                                    serde_json::to_value(result).unwrap(),
                                    serde_json::to_value(&baseline).unwrap()
                                );
                                break;
                            }
                            SessionProgress::Finished => panic!("Missing result"),
                        }
                    }
                    if quantum == 1 {
                        assert!(pending >= 24);
                        boundaries = pending;
                    }
                }
                for boundary in 0..=boundaries {
                    let mut session =
                        EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
                    for _ in 0..boundary {
                        assert!(matches!(
                            session.resume(NonZeroUsize::MIN),
                            SessionProgress::Pending
                        ));
                    }
                    let SessionProgress::Complete(result) =
                        session.resume_with_control(NonZeroUsize::MIN, &Cancel)
                    else {
                        panic!("Missing cancellation");
                    };
                    assert_eq!(result.unwrap_err().kind, ExecutionErrorKind::Cancelled);
                    assert!(matches!(
                        session.resume(NonZeroUsize::MAX),
                        SessionProgress::Finished
                    ));
                }
                if let Ok(result) = &baseline {
                    for dimension in 0..2 {
                        let amount = if dimension == 0 {
                            result.consumed.steps
                        } else {
                            result.consumed.value_bytes
                        };
                        for bound in [0, amount - 1, amount, amount + 1] {
                            let mut limits = ExecutionLimits::default();
                            if dimension == 0 {
                                limits.scalar.max_steps = bound;
                            } else {
                                limits.scalar.max_value_bytes = bound;
                            }
                            let result = evaluate(&image, &arguments, &context, &limits);
                            assert_eq!(
                                result.is_ok(),
                                bound >= amount,
                                "{body}: dimension {dimension} bound {bound}: {result:?}"
                            );
                        }
                    }
                }
                assert_eq!(
                    serde_json::to_value(evaluate(&image, &arguments, &context, &limits)).unwrap(),
                    serde_json::to_value(&baseline).unwrap()
                );
            }
        }
    }
    #[test]
    fn structural_type_views_share_payloads_and_keep_logical_charges() {
        use intersect_model::definitions::rcore_behavior::ir_v2::{self, TypeId};
        fn argument(image: &ir_v2::VerifiedExecutable, ty: TypeId) -> ValueWire {
            let data = match image.type_node(ty).unwrap() {
                TypeNode::Many { element, .. } => DataWire::Collection(
                    (0..96)
                        .map(|n| ValueWire {
                            ty: *element,
                            data: DataWire::Scalar(ir::Value {
                                ty: ir::ValueType::Scalar(Scalar::Int64),
                                payload: serde_json::json!({"kind":"int64","value":n.to_string()}),
                            }),
                        })
                        .collect(),
                ),
                TypeNode::Tuple(types) => {
                    DataWire::Tuple(types.iter().map(|ty| argument(image, *ty)).collect())
                }
                TypeNode::Record(types) => DataWire::Record(
                    types
                        .iter()
                        .map(|(key, ty)| (key.clone(), argument(image, *ty)))
                        .collect(),
                ),
                TypeNode::Option(ty) => DataWire::Some(Box::new(argument(image, *ty))),
                TypeNode::Result { ok, .. } => DataWire::Ok(Box::new(argument(image, *ok))),
                other => panic!("Unsupported fixture {other:?}"),
            };
            ValueWire { ty, data }
        }
        for (parameter, result) in [
            ("T", "Sequence<Int>"),
            ("(T,T)", "(Sequence<Int>,Sequence<Int>)"),
            ("{a:T,b:T}", "{a:Sequence<Int>,b:Sequence<Int>}"),
            ("Option<T>", "Option<Sequence<Int>>"),
            ("Result<T,String>", "Result<Sequence<Int>,String>"),
        ] {
            let source = DefinitionSource::new(
                "memory:views-behavior.rcore",
                SourceKind::RcoreBehavior,
                format!(
                    "rcore 1;package views version \"1\";fn run<T:Sequence<Int>>(xs:{parameter})->{result} =xs;"
                ),
            );
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source.clone()],
                namespaces: vec![],
            };
            let selection = EntrySelection {
                symbol: SymbolId {
                    source: source.id.clone(),
                    declaration: 0,
                },
                types: vec![
                    TypeNode::Scalar(Scalar::Int64),
                    TypeNode::Many {
                        element: TypeId(0),
                        lower: 2,
                        upper: 128,
                        ordered: true,
                        unique: false,
                    },
                ],
                arguments: vec![TypeId(1)],
            };
            let image =
                compile_function(&bundle, &source.id, &selection, &Limits::default()).unwrap();
            let entry = &image.program().functions[image.program().entry.0 as usize];
            let ty = entry.parameters[0];
            let result_ty = entry.result;
            let arguments = [argument(&image, ty)];
            let limits = ExecutionLimits::default();
            let context = ExecutionContext::new(&bundle);
            let baseline = evaluate(&image, &arguments, &context, &limits).unwrap();
            let mut expected = arguments[0].clone();
            expected.ty = result_ty;
            assert_eq!(baseline.value, expected);
            for quantum in [1, 7, 4096] {
                let mut session =
                    EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
                loop {
                    match session.resume(NonZeroUsize::new(quantum).unwrap()) {
                        SessionProgress::Pending => {}
                        SessionProgress::Complete(result) => {
                            assert_eq!(
                                serde_json::to_value(result.unwrap()).unwrap(),
                                serde_json::to_value(&baseline).unwrap()
                            );
                            break;
                        }
                        SessionProgress::Finished => panic!("Missing result"),
                    }
                }
                let SessionRoot::Borrowed(cell) = &mut session.root else {
                    panic!("Expected borrowed fixture owner")
                };
                cell.with_dependent_mut(|source, retained| {
                    // Encoding moved the arena into its stable source owner.
                    // Inspect that actual arena through the same node lookup.
                    assert!(retained.execution.nodes.is_empty());
                    let nodes = source.encoding_nodes().unwrap();
                    let aliases: Vec<_> = nodes
                        .iter()
                        .enumerate()
                        .filter_map(|(id, node)| {
                            if let Data::View(origin) = node.data {
                                Some((id, origin))
                            } else {
                                None
                            }
                        })
                        .collect();
                    assert!(!aliases.is_empty());
                    let state = State {
                        image: ExecutionImage::Functions(&image),
                        context: Some(&context),
                        limits: &limits,
                        control: None,
                        data: &mut retained.execution,
                    };
                    for (id, origin) in aliases {
                        assert!(!matches!(nodes[origin].data, Data::View(_)));
                        let node = state.node_in(nodes, id).unwrap();
                        assert!(std::ptr::eq(node.data, &nodes[origin].data));
                        assert_eq!(node.bytes, nodes[origin].bytes);
                        assert_eq!(node.depth, nodes[origin].depth);
                    }
                });
            }
            if parameter == "T" {
                let mut program = image.program().clone();
                let TypeNode::Many { element, .. } = program.types[ty.0 as usize] else {
                    panic!("Many")
                };
                let intermediate = TypeId(program.types.len() as u32);
                program.types.push(TypeNode::Many {
                    element,
                    lower: 0,
                    upper: 128,
                    ordered: true,
                    unique: false,
                });
                let image = ir_v2::verify(program, &bundle, &Limits::default()).unwrap();
                let mut session =
                    EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
                while !matches!(session.phase(), Phase::Running) {
                    assert!(matches!(
                        session.resume(NonZeroUsize::MIN),
                        SessionProgress::Pending
                    ));
                }
                let root = session.execution().frames[0].parameters[0];
                let mut state = State {
                    image: ExecutionImage::Functions(&image),
                    context: Some(&context),
                    limits: &limits,
                    control: None,
                    data: session.execution_mut(),
                };
                let bytes = state.node(root).unwrap().bytes;
                let before = state.data.consumed.clone();
                let first = state.view(root, intermediate, None).unwrap();
                let second = state.view(first, result_ty, None).unwrap();
                assert_eq!(state.node(first).unwrap().origin, root);
                assert_eq!(state.node(second).unwrap().origin, root);
                assert_eq!(state.data.consumed.value_nodes - before.value_nodes, 2);
                assert_eq!(
                    state.data.consumed.value_bytes - before.value_bytes,
                    4 * bytes
                );
                assert_eq!(
                    state.data.consumed.collection_comparisons,
                    before.collection_comparisons
                );
                let before = state.data.consumed.clone();
                assert_eq!(state.view(second, result_ty, None).unwrap(), second);
                assert_eq!(state.data.consumed.value_nodes, before.value_nodes);
                assert_eq!(state.data.consumed.value_bytes - before.value_bytes, bytes);
                let before = state.data.consumed.clone();
                assert!(state.view(second, ty, None).is_err());
                assert_eq!(state.data.consumed.value_bytes, before.value_bytes);
                assert_eq!(state.data.consumed.value_nodes, before.value_nodes);
                let depth = state.node(root).unwrap().depth;
                let bad = state.data.nodes.len();
                state.data.nodes.push(Node {
                    ty: result_ty,
                    data: Data::View(first),
                    bytes,
                    depth,
                });
                assert!(state.node(bad).is_err());
                let bad = state.data.nodes.len();
                state.data.nodes.push(Node {
                    ty: result_ty,
                    data: Data::View(root),
                    bytes: bytes + 1,
                    depth,
                });
                assert!(state.node(bad).is_err());
            }
        }
    }

    #[test]
    fn native_decode_yields_nodes_and_nested_unique_checks_for_all_many_flags() {
        use intersect_model::definitions::rcore_behavior::{ir_v2::TypeId, types::CollectionKind};
        struct Cancel;
        impl ExecutionControl for Cancel {
            fn cancellation_requested(&self) -> bool {
                true
            }
        }
        for (kind, ordered, unique) in [
            ("Sequence", true, false),
            ("Bag", false, false),
            ("Set", false, true),
            ("OrderedSet", true, true),
        ] {
            for many in [false, true] {
                let signature = if many {
                    format!("fn run<T:{kind}<Bag<Int>>>(xs:T)->Int=size(xs);")
                } else {
                    format!("fn run(xs:{kind}<Bag<Int>>)->Int=size(xs);")
                };
                let source = DefinitionSource::new(
                    "memory:decode-behavior.rcore",
                    SourceKind::RcoreBehavior,
                    format!("rcore 1;package decode version \"1\";{signature}"),
                );
                let bundle = DefinitionBundle {
                    version: 1,
                    sources: vec![source.clone()],
                    namespaces: vec![],
                };
                let selection = EntrySelection {
                    symbol: SymbolId {
                        source: source.id.clone(),
                        declaration: 0,
                    },
                    types: if many {
                        vec![
                            TypeNode::Scalar(Scalar::Int64),
                            TypeNode::Collection {
                                kind: CollectionKind::Bag,
                                element: TypeId(0),
                            },
                            TypeNode::Many {
                                element: TypeId(1),
                                lower: 2,
                                upper: 2,
                                ordered,
                                unique,
                            },
                        ]
                    } else {
                        vec![]
                    },
                    arguments: if many { vec![TypeId(2)] } else { vec![] },
                };
                let image =
                    compile_function(&bundle, &source.id, &selection, &Limits::default()).unwrap();
                let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
                let bag = match image.program().types[ty.0 as usize] {
                    TypeNode::Collection { element, .. } | TypeNode::Many { element, .. } => {
                        element
                    }
                    _ => panic!("Outer collection"),
                };
                let TypeNode::Collection { element, .. } = image.program().types[bag.0 as usize]
                else {
                    panic!("Bag")
                };
                for different in [false, true] {
                    let value = |reverse: bool| {
                        ValueWire {ty:bag,data:DataWire::Collection((0..4).map(|i|ValueWire {ty:element,data:DataWire::Scalar(ir::Value {ty:ir::ValueType::Scalar(Scalar::Int64),payload:serde_json::json!({"kind":"int64","value":((if reverse {3-i} else {i})+if reverse&&different {1} else {0}).to_string()})})}).collect())}
                    };
                    let arguments = [ValueWire {
                        ty,
                        data: DataWire::Collection(vec![value(false), value(true)]),
                    }];
                    let context = ExecutionContext::new(&bundle);
                    let limits = ExecutionLimits::default();
                    let baseline = evaluate(&image, &arguments, &context, &limits);
                    if unique && !different {
                        let error = baseline.as_ref().unwrap_err();
                        assert_eq!(error.kind, ExecutionErrorKind::Arguments);
                        assert_eq!(&*error.message, "Duplicate value in unique collection");
                    } else {
                        assert!(baseline.is_ok(), "{baseline:?}");
                    }
                    let mut boundaries = Vec::new();
                    for quantum in [1, 7, 4096] {
                        let mut session =
                            EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
                        let mut ticks = 0;
                        let mut saw_partial = false;
                        loop {
                            let decoding = session.decoder().is_some();
                            if quantum == 1 && decoding {
                                boundaries.push(ticks);
                            }
                            let nodes = session.execution().consumed.value_nodes;
                            let comparisons = session.execution().consumed.collection_comparisons;
                            let progress = session.resume(NonZeroUsize::new(quantum).unwrap());
                            match progress {
                                SessionProgress::Pending => {
                                    if quantum == 1 && decoding {
                                        assert!(
                                            session.execution().consumed.value_nodes - nodes <= 1
                                        );
                                        assert!(
                                            session.execution().consumed.collection_comparisons
                                                - comparisons
                                                <= 1
                                        );
                                        saw_partial |= nodes > 0 && session.decoder().is_some();
                                    }
                                    ticks += 1;
                                    assert!(ticks < 10000);
                                }
                                SessionProgress::Complete(result) => {
                                    assert_eq!(
                                        serde_json::to_value(result).unwrap(),
                                        serde_json::to_value(&baseline).unwrap()
                                    );
                                    break;
                                }
                                SessionProgress::Finished => panic!("Missing result"),
                            }
                        }
                        if quantum == 1 {
                            assert!(saw_partial);
                        }
                    }
                    if kind == "OrderedSet" && !many {
                        for boundary in &boundaries {
                            let mut session =
                                EvaluationSession::new(&image, &arguments, &context, &limits)
                                    .unwrap();
                            for _ in 0..*boundary {
                                assert!(matches!(
                                    session.resume(NonZeroUsize::MIN),
                                    SessionProgress::Pending
                                ));
                            }
                            let SessionProgress::Complete(result) =
                                session.resume_with_control(NonZeroUsize::MIN, &Cancel)
                            else {
                                panic!("Cancellation");
                            };
                            assert_eq!(result.unwrap_err().kind, ExecutionErrorKind::Cancelled);
                            assert!(matches!(
                                session.resume(NonZeroUsize::MAX),
                                SessionProgress::Finished
                            ));
                        }
                        if let Ok(result) = &baseline {
                            for dimension in 0..2 {
                                let amount = if dimension == 0 {
                                    result.consumed.value_bytes
                                } else {
                                    result.consumed.collection_comparisons
                                };
                                for bound in [0, amount - 1, amount, amount + 1] {
                                    let mut limits = ExecutionLimits::default();
                                    if dimension == 0 {
                                        limits.scalar.max_value_bytes = bound;
                                    } else {
                                        limits.max_collection_comparisons = bound;
                                    }
                                    let result = evaluate(&image, &arguments, &context, &limits);
                                    assert_eq!(
                                        result.is_ok(),
                                        bound >= amount,
                                        "{kind} dimension {dimension} bound {bound}: {result:?}"
                                    );
                                }
                            }
                        }
                    }
                    if many {
                        let mut malformed = arguments.clone();
                        let DataWire::Collection(values) = &mut malformed[0].data else {
                            panic!("Collection")
                        };
                        values.pop();
                        let error = evaluate(&image, &malformed, &context, &limits).unwrap_err();
                        assert_eq!(error.kind, ExecutionErrorKind::Arguments);
                        assert_eq!(error.consumed.value_nodes, 0);
                    }
                    assert_eq!(
                        serde_json::to_value(evaluate(&image, &arguments, &context, &limits))
                            .unwrap(),
                        serde_json::to_value(&baseline).unwrap()
                    );
                }
            }
        }
    }

    #[test]
    fn argument_shape_admission_suspends_before_runtime_decoding() {
        let source = DefinitionSource::new(
            "memory:shape-behavior.rcore",
            SourceKind::RcoreBehavior,
            "rcore 1;package shape version \"1\";fn run(x:Sequence<Int32>)->Sequence<Int32> =x;",
        );
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let image = compile_function(
            &bundle,
            &source.id,
            &EntrySelection {
                symbol: SymbolId {
                    source: source.id.clone(),
                    declaration: 0,
                },
                types: vec![],
                arguments: vec![],
            },
            &Limits::default(),
        )
        .unwrap();
        let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
        let TypeNode::Collection { element, .. } = image.program().types[ty.0 as usize] else {
            panic!("Collection");
        };
        let arguments = [ValueWire {
            ty,
            data: DataWire::Collection(
                (0..24)
                    .map(|n| ValueWire {
                        ty: element,
                        data: DataWire::Scalar(ir::Value {
                            ty: ir::ValueType::Scalar(Scalar::Int32),
                            payload: serde_json::json!(n),
                        }),
                    })
                    .collect(),
            ),
        }];
        let limits = ExecutionLimits::default();
        let context = ExecutionContext::new(&bundle);
        let baseline = evaluate(&image, &arguments, &context, &limits).unwrap();
        let mut session = EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
        let mut preparation = 0;
        while !matches!(session.phase(), Phase::Arguments { index: 0 }) {
            assert!(matches!(
                session.resume(NonZeroUsize::MIN),
                SessionProgress::Pending
            ));
            preparation += 1;
        }
        let mut shape_slices = 0;
        while matches!(session.phase(), Phase::Arguments { index: 0 }) {
            assert!(matches!(
                session.resume(NonZeroUsize::MIN),
                SessionProgress::Pending
            ));
            if session.shape().is_some() {
                assert_eq!(session.execution().consumed.value_nodes, 0);
                assert!(session.execution().nodes.is_empty());
                shape_slices += 1;
            }
        }
        assert!(shape_slices > 24);
        struct Cancel;
        impl ExecutionControl for Cancel {
            fn cancellation_requested(&self) -> bool {
                true
            }
        }
        for boundary in 0..=shape_slices {
            let mut session =
                EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
            for _ in 0..preparation + boundary {
                assert!(matches!(
                    session.resume(NonZeroUsize::MIN),
                    SessionProgress::Pending
                ));
            }
            let SessionProgress::Complete(result) =
                session.resume_with_control(NonZeroUsize::MIN, &Cancel)
            else {
                panic!("Cancellation");
            };
            let error = result.unwrap_err();
            assert_eq!(error.kind, ExecutionErrorKind::Cancelled);
            assert_eq!(error.consumed.value_nodes, 0);
            assert!(matches!(
                session.resume(NonZeroUsize::MAX),
                SessionProgress::Finished
            ));
        }
        for quantum in [1, 7, 4096] {
            let mut session =
                EvaluationSession::new(&image, &arguments, &context, &limits).unwrap();
            loop {
                match session.resume(NonZeroUsize::new(quantum).unwrap()) {
                    SessionProgress::Pending => {}
                    SessionProgress::Complete(result) => {
                        assert_eq!(
                            serde_json::to_value(result.unwrap()).unwrap(),
                            serde_json::to_value(&baseline).unwrap()
                        );
                        break;
                    }
                    SessionProgress::Finished => panic!("Missing result"),
                }
            }
        }
        let mut invalid = arguments.clone();
        let DataWire::Collection(values) = &mut invalid[0].data else {
            panic!("Collection");
        };
        values[23].data = DataWire::None;
        let error = evaluate(&image, &invalid, &context, &limits).unwrap_err();
        assert_eq!(error.kind, ExecutionErrorKind::Arguments);
        assert_eq!(error.consumed.value_nodes, 0);
        assert_eq!(
            serde_json::to_value(evaluate(&image, &arguments, &context, &limits).unwrap()).unwrap(),
            serde_json::to_value(&baseline).unwrap()
        );
    }

    #[test]
    fn result_transport_yields_per_node_and_cancels_at_every_encoding_boundary() {
        struct Cancel;
        impl ExecutionControl for Cancel {
            fn cancellation_requested(&self) -> bool {
                true
            }
        }
        for body in [
            "fn run()->Sequence<{a:(Option<Int>,Result<Int,String>),b:Bag<Int>}> =[for x in [0,1,2,3] yield {a:(Some(x),if x>0 {Ok(x)} else {Err(\"zero\")}),b:Bag([x,x])}];",
            "fn run()->Option<Result<Int,String>> =Some(Err(\"no\"));",
            "fn run()->Option<Int> =None;",
            "fn run()->Sequence<Int> =[];",
        ] {
            let source = DefinitionSource::new(
                "memory:encoding-behavior.rcore",
                SourceKind::RcoreBehavior,
                format!("rcore 1;package encoding version \"1\";{body}"),
            );
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source.clone()],
                namespaces: vec![],
            };
            let image = compile_function(
                &bundle,
                &source.id,
                &EntrySelection {
                    symbol: SymbolId {
                        source: source.id.clone(),
                        declaration: 0,
                    },
                    types: vec![],
                    arguments: vec![],
                },
                &Limits::default(),
            )
            .unwrap();
            let context = ExecutionContext::new(&bundle);
            let limits = ExecutionLimits::default();
            let baseline = evaluate(&image, &[], &context, &limits).unwrap();
            let mut encoding_boundaries = 0;
            let mut preparation_boundaries = 0;
            for quantum in [1, 7, 4096] {
                let mut session = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
                let mut preparation = 0;
                while !matches!(session.phase(), Phase::Encoding(_)) {
                    assert!(matches!(
                        session.resume(NonZeroUsize::MIN),
                        SessionProgress::Pending
                    ));
                    preparation += 1;
                    assert!(preparation < 10000);
                }
                let mut before = session.execution().consumed.value_nodes;
                let mut pending = 0;
                loop {
                    let progress = session.resume(NonZeroUsize::new(quantum).unwrap());
                    let after = match &progress {
                        SessionProgress::Complete(result) => {
                            result.as_ref().as_ref().unwrap().consumed.value_nodes
                        }
                        _ => session.execution().consumed.value_nodes,
                    };
                    if quantum == 1 {
                        assert!(after - before <= 1, "{body}: {before}->{after}");
                    }
                    before = after;
                    match progress {
                        SessionProgress::Pending => {
                            pending += 1;
                            assert!(pending < 10000);
                        }
                        SessionProgress::Complete(result) => {
                            assert_eq!(
                                serde_json::to_value(result.unwrap()).unwrap(),
                                serde_json::to_value(&baseline).unwrap()
                            );
                            break;
                        }
                        SessionProgress::Finished => panic!("Missing result"),
                    }
                }
                if quantum == 1 {
                    encoding_boundaries = pending;
                    preparation_boundaries = preparation;
                }
            }
            for boundary in 0..=encoding_boundaries {
                let mut session = EvaluationSession::new(&image, &[], &context, &limits).unwrap();
                for _ in 0..preparation_boundaries + boundary {
                    assert!(matches!(
                        session.resume(NonZeroUsize::MIN),
                        SessionProgress::Pending
                    ));
                }
                assert!(matches!(session.phase(), Phase::Encoding(_)));
                let SessionProgress::Complete(result) =
                    session.resume_with_control(NonZeroUsize::MIN, &Cancel)
                else {
                    panic!("Missing cancellation");
                };
                assert_eq!(result.unwrap_err().kind, ExecutionErrorKind::Cancelled);
                assert!(matches!(
                    session.resume(NonZeroUsize::MAX),
                    SessionProgress::Finished
                ));
            }
            for dimension in 0..2 {
                // The node limit also bounds preparation metadata. Only the wide
                // result has transport node consumption above that admission floor.
                if dimension == 0 && !body.contains("b:Bag<Int>") {
                    continue;
                }
                let amount = if dimension == 0 {
                    baseline.consumed.value_nodes
                } else {
                    baseline.consumed.value_bytes
                };
                for bound in [0, amount - 1, amount, amount + 1] {
                    let mut limits = ExecutionLimits::default();
                    if dimension == 0 {
                        limits.max_value_nodes = bound;
                    } else {
                        limits.scalar.max_value_bytes = bound;
                    }
                    let result = evaluate(&image, &[], &context, &limits);
                    assert_eq!(
                        result.is_ok(),
                        bound >= amount,
                        "{body} dimension {dimension} bound {bound}: {result:?}"
                    );
                    if let Err(error) = result {
                        assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
                    }
                }
            }
            assert_eq!(
                serde_json::to_value(evaluate(&image, &[], &context, &limits).unwrap()).unwrap(),
                serde_json::to_value(&baseline).unwrap()
            );
        }
    }
}

#[cfg(test)]
#[path = "rcore_behavior_owned_reference_inputs_tests.rs"]
mod owned_reference_input_controls;

#[cfg(test)]
mod retained_string_transport_tests {
    use super::*;
    use intersect_model::definitions::{
        bundle::{DefinitionSource, SourceKind},
        rcore_behavior::{
            function_lowering::compile_function,
            ir,
            ir_v2::{DataWire, EntrySelection, Limits},
            modules::SymbolId,
            types::Scalar,
        },
    };
    fn fixture(text: &str) -> (DefinitionBundle, VerifiedExecutable, Vec<ValueWire>) {
        let source = DefinitionSource::new(
            "memory:retained-string-transport.rcore",
            SourceKind::RcoreBehavior,
            "rcore 1;package leafTransport version \"1\";fn echo(value:String)->String=value;",
        );
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let image = compile_function(
            &bundle,
            &source.id,
            &EntrySelection {
                symbol: SymbolId {
                    source: source.id.clone(),
                    declaration: 0,
                },
                types: vec![],
                arguments: vec![],
            },
            &Limits::default(),
        )
        .unwrap();
        let ty = image.program().functions[image.program().entry.0 as usize].parameters[0];
        let args = vec![ValueWire {
            ty,
            data: DataWire::Scalar(ir::Value {
                ty: ir::ValueType::Scalar(Scalar::String),
                payload: serde_json::Value::String(text.into()),
            }),
        }];
        (bundle, image, args)
    }
    fn progress(session: &EvaluationSession<'_>) -> Option<(Id, usize, usize)> {
        session
            .encoder()
            .as_ref()
            .and_then(|e| e.string_copy_progress())
    }
    fn until_encoding(session: &mut EvaluationSession<'_>) -> Id {
        for _ in 0..10000 {
            if let Phase::Encoding(id) = session.phase() {
                return id;
            }
            assert!(matches!(
                session.resume(NonZeroUsize::MIN),
                SessionProgress::Pending
            ));
        }
        panic!("encoding not reached")
    }
    fn source<'s>(session: &'s EvaluationSession<'_>, id: Id) -> &'s str {
        let super::super::values::Data::Scalar(v) = &session.retained_nodes()[id].data else {
            panic!("arena String owner")
        };
        v.payload.as_str().unwrap()
    }
    #[test]
    fn retained_string_transport_bounds_actual_copy_and_keeps_the_arena_owner() {
        let (frame, cursor, encoder) = super::super::encoding::Encoding::retained_string_layout();
        assert!(frame <= 128 && cursor <= 64);
        eprintln!(
            "retained-string-layout frame={frame} cursor={cursor} encoder={encoder} session={}",
            std::mem::size_of::<EvaluationSession<'_>>()
        );
        let unicode = ['"', '\\', '\n', '\t', '\0', '\u{e9}', '\u{1f600}']
            .into_iter()
            .collect::<String>()
            .repeat(513);
        for text in [String::new(), "x".repeat(16), "x".repeat(4097), unicode] {
            let (bundle, image, args) = fixture(&text);
            let context = ExecutionContext::new(&bundle);
            let limits = ExecutionLimits::default();
            let baseline = evaluate(&image, &args, &context, &limits).unwrap();
            let mut session = EvaluationSession::new(&image, &args, &context, &limits).unwrap();
            let id = until_encoding(&mut session);
            let pointer = source(&session, id).as_ptr();
            let mut previous = 0;
            let mut charged = None;
            let mut pending = 0;
            loop {
                assert_eq!(source(&session, id).as_ptr(), pointer);
                assert_eq!(source(&session, id), text);
                match session.resume(NonZeroUsize::MIN) {
                    SessionProgress::Pending => {
                        let (owner, copied, capacity) = progress(&session).unwrap();
                        assert_eq!(owner, id);
                        assert_eq!(capacity, text.len());
                        assert!(copied > previous && copied - previous <= 256);
                        assert!(text.is_char_boundary(copied));
                        previous = copied;
                        pending += 1;
                        let consumption =
                            serde_json::to_value(&session.execution().consumed).unwrap();
                        if let Some(old) = &charged {
                            assert_eq!(old, &consumption);
                        } else {
                            charged = Some(consumption);
                        }
                    }
                    SessionProgress::Complete(result) => {
                        assert!(text.len() - previous <= 256);
                        assert_eq!(
                            serde_json::to_value(result.unwrap()).unwrap(),
                            serde_json::to_value(&baseline).unwrap()
                        );
                        break;
                    }
                    SessionProgress::Finished => panic!("missing result"),
                }
            }
            assert!(pending >= text.len().div_ceil(256).saturating_sub(1));
            if text.is_ascii() {
                assert_eq!(pending, text.len().div_ceil(256).saturating_sub(1));
            }
            assert!(matches!(
                session.resume(NonZeroUsize::MAX),
                SessionProgress::Finished
            ));
        }
    }
    #[test]
    fn retained_string_transport_cancel_and_drop_every_actual_copy_boundary_and_retry() {
        struct Cancel;
        impl ExecutionControl for Cancel {
            fn cancellation_requested(&self) -> bool {
                true
            }
        }
        let text = "x".repeat(4097);
        let (bundle, image, args) = fixture(&text);
        let context = ExecutionContext::new(&bundle);
        let limits = ExecutionLimits::default();
        let baseline = evaluate(&image, &args, &context, &limits).unwrap();
        for boundary in 0..16 {
            for cancel in [false, true] {
                let mut session = EvaluationSession::new(&image, &args, &context, &limits).unwrap();
                until_encoding(&mut session);
                for _ in 0..=boundary {
                    assert!(matches!(
                        session.resume(NonZeroUsize::MIN),
                        SessionProgress::Pending
                    ));
                }
                let (_, copied, _) = progress(&session).unwrap();
                assert_eq!(copied, (boundary + 1) * 256);
                if cancel {
                    let SessionProgress::Complete(outcome) =
                        session.resume_with_control(NonZeroUsize::MIN, &Cancel)
                    else {
                        panic!("cancel")
                    };
                    let error = outcome.unwrap_err();
                    assert_eq!(error.kind, ExecutionErrorKind::Cancelled);
                    assert_eq!(error.consumed.value_bytes, baseline.consumed.value_bytes);
                    assert_eq!(error.consumed.value_nodes, baseline.consumed.value_nodes);
                    assert!(matches!(
                        session.resume(NonZeroUsize::MAX),
                        SessionProgress::Finished
                    ));
                }
                drop(session);
                assert_eq!(
                    serde_json::to_value(evaluate(&image, &args, &context, &limits).unwrap())
                        .unwrap(),
                    serde_json::to_value(&baseline).unwrap()
                );
            }
        }
        let mut denied = limits.clone();
        denied.scalar.max_value_bytes = baseline.consumed.value_bytes - 1;
        let original = evaluate(&image, &args, &context, &denied).unwrap_err();
        let mut session = EvaluationSession::new(&image, &args, &context, &denied).unwrap();
        loop {
            match session.resume(NonZeroUsize::MIN) {
                SessionProgress::Pending => assert!(progress(&session).is_none()),
                SessionProgress::Complete(outcome) => {
                    assert_eq!(
                        serde_json::to_value(outcome.unwrap_err()).unwrap(),
                        serde_json::to_value(original).unwrap()
                    );
                    break;
                }
                SessionProgress::Finished => panic!("missing denial"),
            }
        }
    }
}

#[cfg(test)]
#[path = "rcore_behavior_functions_input_string_tests.rs"]
mod retained_input_string_tests;

/// Verification-only fixed-holder layout observations; no authority/budget credit.
#[cfg(feature = "allocation-profile")]
pub fn retained_session_root_layouts() -> [usize; 7] {
    [
        std::mem::size_of::<EvaluationSession<'static>>(),
        std::mem::size_of::<BorrowedSources<'static>>(),
        std::mem::size_of::<OwnedSources>(),
        std::mem::size_of::<SessionState<'static>>(),
        std::mem::size_of::<(BorrowedSources<'static>, SessionState<'static>)>(),
        std::mem::size_of::<(OwnedSources, SessionState<'static>)>(),
        std::mem::align_of::<SessionState<'static>>(),
    ]
}

#[cfg(target_pointer_width = "64")]
const _: () = {
    use std::mem::size_of;
    const COPY: usize = {
        let json = intersect_model::definitions::retained_json_frontier::CopyJob::OWNER_BYTES;
        let numeric = super::numeric::Numeric::OWNER_BYTES;
        if json > numeric { json } else { numeric }
    };
    assert!(
        size_of::<EvaluationSession<'static>>()
            + size_of::<(BorrowedSources<'static>, SessionState<'static>)>()
            + COPY
            <= 1344
    );
    assert!(
        size_of::<EvaluationSession<'static>>()
            + size_of::<(OwnedSources, SessionState<'static>)>()
            + COPY
            <= 2032
    );
    assert!(
        size_of::<crate::semantic_bindings::BindingSession<'static>>()
            + size_of::<(BindingSources<'static>, SessionState<'static>)>()
            + COPY
            <= 1568
    );
};
#[cfg(target_arch = "wasm32")]
const _: () = {
    use std::mem::size_of;
    const COPY: usize = {
        let json = intersect_model::definitions::retained_json_frontier::CopyJob::OWNER_BYTES;
        let numeric = super::numeric::Numeric::OWNER_BYTES;
        if json > numeric { json } else { numeric }
    };
    assert!(
        size_of::<EvaluationSession<'static>>()
            + size_of::<(BorrowedSources<'static>, SessionState<'static>)>()
            + COPY
            <= 712
    );
    assert!(
        size_of::<EvaluationSession<'static>>()
            + size_of::<(OwnedSources, SessionState<'static>)>()
            + COPY
            <= 1068
    );
    assert!(
        size_of::<crate::semantic_bindings::BindingSession<'static>>()
            + size_of::<(BindingSources<'static>, SessionState<'static>)>()
            + COPY
            <= 832
    );
};

#[cfg(test)]
#[path = "rcore_behavior_functions_borrowed_layout_tests.rs"]
mod borrowed_input_layout_tests;
