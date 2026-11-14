//! An explicit sibling root; ALL phase/decoder/execution bodies remain shared.
use super::*;
use crate::definitions::conversion::{InvocationContext, InvocationObserver};
use crate::rcore_behavior_functions::inputs::{
    BorrowedDomains, BorrowedInputAuthority, ScopedModelInputs,
};

struct ScopedSources<'input> {
    binding: BindingSources<'input>,
    inputs: &'input dyn BorrowedInputAuthority,
}
type ScopedState<'a> = SessionState<'a, ScopedModelInputs<'a>>;
self_cell::self_cell!(
    struct ScopedSessionCell<'input> {
        owner: ScopedSources<'input>,
        #[not_covariant]
        dependent: ScopedState,
    }
);
pub(crate) struct BorrowedEvaluationSession<'input> {
    cell: ScopedSessionCell<'input>,
}
impl<'input> BorrowedEvaluationSession<'input> {
    pub(crate) fn new_binding(
        image: &'input VerifiedExecutable,
        arguments: &'input [ValueWire],
        context: &ExecutionContext<'input>,
        limits: &'input ExecutionLimits,
        inputs: &'input dyn BorrowedInputAuthority,
        observation: &InvocationContext<'_>,
    ) -> crate::semantic_bindings::Result<Self> {
        observation.work(1).map_err(|message| {
            crate::semantic_bindings::Error::new(
                crate::semantic_bindings::ErrorKind::ResourceExhausted,
                message,
            )
        })?;
        // Admit the actual NEW holder before self_cell/observer allocation.
        // Six words cover the cell/observer ownership records in logical units;
        // this remains cumulative logical retention, not total RSS accounting.
        let root_bytes = crate::logical_size!((ScopedSources<'_>, ScopedState<'_>), 1288)
            .saturating_add(6 * crate::logical_size::LOGICAL_WORD);
        observation.retain_storage(root_bytes).map_err(|message| {
            crate::semantic_bindings::Error::new(
                crate::semantic_bindings::ErrorKind::ResourceExhausted,
                message,
            )
        })?;
        let (execution, registry) = initialize(image, context.conversions, limits)?;
        execution
            .provider_receipts
            .retain_storage(root_bytes)
            .map_err(|message| {
                crate::semantic_bindings::Error::new(
                    crate::semantic_bindings::ErrorKind::ResourceExhausted,
                    message,
                )
            })?;
        let owner = ScopedSources {
            binding: BindingSources {
                sources: BorrowedSources {
                    registry,
                    image,
                    arguments,
                    bundle: context.bundle,
                    limits,
                    arena: std::cell::OnceCell::new(),
                },
                result: std::cell::OnceCell::new(),
            },
            inputs,
        };
        let cell = ScopedSessionCell::new(owner, move |owner| SessionState {
            source: &owner.binding,
            references: SliceInput::Borrowed(&[]),
            execution: execution
                .with_domains::<ScopedModelInputs<'_>>(BorrowedDomains::new(owner.inputs)),
            decoded_arguments: Vec::new(),
            work: SessionWork::Context(
                crate::rcore_behavior_functions::context::ContextPreparation::new(),
            ),
            phase: Phase::Context,
        });
        Ok(Self { cell })
    }
    pub(crate) fn payload_continuation(&self) -> bool {
        self.cell
            .with_dependent(|_, state| state.payload_continuation())
    }
    pub(crate) fn resume_with_control(
        &mut self,
        quantum: NonZeroUsize,
        control: &dyn ExecutionControl,
    ) -> SessionProgress {
        self.cell
            .with_dependent_mut(|_, state| state.resume_with_control(quantum, control))
    }
}
