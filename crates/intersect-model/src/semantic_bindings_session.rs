//! Resumable checked dispatch. Selection/context/budget stay fixed across slices.
//! Native and Rhai callbacks are atomic cooperative units; no preemption promise.
use super::*;

pub enum BindingProgress {
    Pending,
    Complete(Box<Result<InvocationResult>>),
    Finished,
}
pub(super) enum Phase {
    Admission,
    Preconditions,
    Body,
    Result,
    Postconditions(Box<Evaluation>),
    Finished,
}
pub struct BindingSession<'a> {
    selected: &'a PinnedCall,
    context: ExecutionContext<'a>,
    limits: &'a ExecutionLimits,
    budget: Budget,
    session: EvaluationSession<'a>,
    phase: Phase,
}
impl<'a> BindingSession<'a> {
    pub(super) fn new(
        selected: &'a PinnedCall,
        arguments: &'a [ValueWire],
        context: &ExecutionContext<'a>,
        limits: &'a ExecutionLimits,
        budget: Budget,
    ) -> Result<Self> {
        budget.check()?;
        let _guard = InvocationGuard::enter()?;
        let session = EvaluationSession::new_binding(
            selected.contract.admission_image(),
            arguments,
            context,
            limits,
        )?;
        Ok(Self {
            selected,
            context: ExecutionContext {
                bundle: context.bundle,
                conversions: context.conversions,
                references: context.references,
            },
            limits,
            budget,
            session,
            phase: Phase::Admission,
        })
    }
    pub fn identity(&self) -> &ImplementationId {
        &self.selected.identity
    }
    pub fn resume(&mut self, quantum: NonZeroUsize) -> BindingProgress {
        if matches!(self.phase, Phase::Finished) {
            return BindingProgress::Finished;
        }
        let _guard = match InvocationGuard::enter() {
            Ok(guard) => guard,
            Err(error) => {
                self.phase = Phase::Finished;
                return BindingProgress::Complete(Box::new(Err(error)));
            }
        };
        for _ in 0..quantum.get() {
            // Shape Visit admission is charged before entering its JSON resource
            // continuation. Extra suspension units do not invent phase fees;
            // cancellation/sticky errors and the polling quantum still apply.
            let admission = if self.session.payload_continuation() {
                self.budget.check()
            } else {
                self.budget.charge(1)
            };
            let result = admission.and_then(|_| self.step());
            match result {
                Ok(Some(evaluation)) => {
                    self.phase = Phase::Finished;
                    return BindingProgress::Complete(Box::new(Ok(InvocationResult {
                        contract: self.selected.contract.id().into(),
                        implementation: self.selected.identity.clone(),
                        work: self.budget.used(),
                        evaluation,
                    })));
                }
                Ok(None) => {}
                Err(error) => {
                    self.phase = Phase::Finished;
                    return BindingProgress::Complete(Box::new(Err(error)));
                }
            }
        }
        BindingProgress::Pending
    }
    fn step(&mut self) -> Result<Option<Evaluation>> {
        step_binding(
            self.selected,
            &self.context,
            self.limits,
            &self.budget,
            &mut self.session,
            &mut self.phase,
        )
    }
}

pub(super) trait PhaseSession<W: BindingWork> {
    fn prepare(&mut self, work: &W) -> Result<bool>;
    fn finish(&mut self, work: &W) -> Result<Option<Evaluation>>;
    fn resume(&mut self, work: &W) -> SessionProgress;
    fn binding_arguments(&self) -> &[ValueWire];
    fn before_post_copy(&self, _: &ValueWire, _: &W) -> Result<()> {
        Ok(())
    }
    fn conditions(
        &self,
        conditions: &[crate::definitions::semantic_bindings::Condition],
        arguments: &[ValueWire],
        context: &ExecutionContext<'_>,
        limits: &ExecutionLimits,
        work: &W,
        kind: ErrorKind,
    ) -> Result<()>;
    fn invoke_provider(
        &mut self,
        provider: &Rc<dyn Provider>,
        contract: &Contract,
        identity: &ImplementationId,
        work: &W,
    ) -> Result<()>;
}
impl PhaseSession<Budget> for EvaluationSession<'_> {
    fn prepare(&mut self, work: &Budget) -> Result<bool> {
        self.prepare_binding_step(work)
    }
    fn finish(&mut self, work: &Budget) -> Result<Option<Evaluation>> {
        self.finish_binding_step(work)
    }
    fn resume(&mut self, work: &Budget) -> SessionProgress {
        self.resume_with_control(NonZeroUsize::MIN, work)
    }
    fn binding_arguments(&self) -> &[ValueWire] {
        EvaluationSession::binding_arguments(self)
    }
    fn conditions(
        &self,
        conditions: &[crate::definitions::semantic_bindings::Condition],
        arguments: &[ValueWire],
        context: &ExecutionContext<'_>,
        limits: &ExecutionLimits,
        work: &Budget,
        kind: ErrorKind,
    ) -> Result<()> {
        check_conditions(conditions, arguments, context, limits, work, kind)
    }
    fn invoke_provider(
        &mut self,
        provider: &Rc<dyn Provider>,
        contract: &Contract,
        _: &ImplementationId,
        budget: &Budget,
    ) -> Result<()> {
        let mut handles = BTreeMap::new();
        if contract.descriptor().effect == Effect::Query {
            for (index, value) in self.binding_arguments().iter().enumerate() {
                if let Some(handle) = self.binding_handle(value, budget)? {
                    handles.insert(index, handle);
                }
            }
        }
        let call = Call {
            contract,
            budget: budget.clone(),
            handles,
        };
        let result = provider.invoke(&call, self.binding_arguments());
        budget.check()?;
        self.set_binding_result(result?)?;
        Ok(())
    }
}
pub(super) fn step_binding<W: BindingWork, S: PhaseSession<W>>(
    selected: &PinnedCall,
    context: &ExecutionContext<'_>,
    limits: &ExecutionLimits,
    budget: &W,
    session: &mut S,
    phase: &mut Phase,
) -> Result<Option<Evaluation>> {
    let contract = &selected.contract;
    match &*phase {
        Phase::Admission => {
            if session.prepare(budget)? {
                *phase = Phase::Preconditions;
            }
        }
        Phase::Preconditions => {
            session.conditions(
                contract.preconditions(),
                session.binding_arguments(),
                context,
                limits,
                budget,
                ErrorKind::Precondition,
            )?;
            *phase = Phase::Body;
        }
        Phase::Body => match &selected.implementation {
            Implementation::Rcore => match session.resume(budget) {
                SessionProgress::Pending => {}
                SessionProgress::Complete(result) => {
                    *phase = Phase::Postconditions(Box::new((*result)?))
                }
                SessionProgress::Finished => {
                    return Err(Error::new(ErrorKind::Contract, "Finished Rcore evaluation"));
                }
            },
            Implementation::Provider(provider) => {
                if provider.contract_id() != contract.id()
                    || provider.identity() != selected.identity
                {
                    return Err(Error::new(
                        ErrorKind::Contract,
                        "Pinned implementation identity changed",
                    ));
                }
                session.invoke_provider(provider, contract, &selected.identity, budget)?;
                *phase = Phase::Result;
            }
        },
        Phase::Result => {
            if let Some(result) = session.finish(budget).map_err(|mut e| {
                if e.kind == ErrorKind::Arguments {
                    e.kind = ErrorKind::Result;
                }
                e
            })? {
                *phase = Phase::Postconditions(Box::new(result));
            }
        }
        Phase::Postconditions(_) => {
            let Phase::Postconditions(evaluation) = std::mem::replace(phase, Phase::Finished)
            else {
                return Err(Error::new(ErrorKind::Contract, "Missing binding result"));
            };
            if !contract.postconditions().is_empty() {
                session.before_post_copy(&evaluation.value, budget)?;
                let mut values = session.binding_arguments().to_vec();
                values.push(evaluation.value.clone());
                session.conditions(
                    contract.postconditions(),
                    &values,
                    context,
                    limits,
                    budget,
                    ErrorKind::Postcondition,
                )?;
            }
            budget.check_work()?;
            return Ok(Some(*evaluation));
        }
        Phase::Finished => {
            return Err(Error::new(ErrorKind::Contract, "Finished binding session"));
        }
    }
    Ok(None)
}
