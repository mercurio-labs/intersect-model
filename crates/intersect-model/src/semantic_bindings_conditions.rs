//! One checked condition loop; only its input-session factory varies.
use super::*;
use crate::definitions::semantic_bindings::Condition;

pub(super) trait ConditionEvaluation<W: BindingWork> {
    fn resume(&mut self, work: &W) -> SessionProgress;
}
impl<W: BindingWork> ConditionEvaluation<W> for EvaluationSession<'_> {
    fn resume(&mut self, work: &W) -> SessionProgress {
        self.resume_with_control(NonZeroUsize::MIN, work)
    }
}
pub(super) trait ConditionSessions<W: BindingWork> {
    type Session<'a>: ConditionEvaluation<W>
    where
        Self: 'a;
    fn before_arguments(&self, _: &[ValueWire], _: &W) -> Result<()> {
        Ok(())
    }
    fn start<'a>(
        &'a self,
        image: &'a crate::definitions::rcore_behavior::ir_v2::VerifiedExecutable,
        arguments: &'a [ValueWire],
        context: &ExecutionContext<'a>,
        limits: &'a ExecutionLimits,
        work: &W,
    ) -> Result<Self::Session<'a>>;
}
pub(super) struct OwnedConditionSessions;
impl<W: BindingWork> ConditionSessions<W> for OwnedConditionSessions {
    type Session<'a> = EvaluationSession<'a>;
    fn start<'a>(
        &'a self,
        image: &'a crate::definitions::rcore_behavior::ir_v2::VerifiedExecutable,
        arguments: &'a [ValueWire],
        context: &ExecutionContext<'a>,
        limits: &'a ExecutionLimits,
        _: &W,
    ) -> Result<Self::Session<'a>> {
        Ok(EvaluationSession::new(image, arguments, context, limits)?)
    }
}
pub(super) fn check_with<W: BindingWork, F: ConditionSessions<W>>(
    conditions: &[Condition],
    arguments: &[ValueWire],
    context: &ExecutionContext<'_>,
    limits: &ExecutionLimits,
    budget: &W,
    kind: ErrorKind,
    sessions: &F,
) -> Result<()> {
    for condition in conditions {
        budget.charge_work(1)?;
        sessions.before_arguments(arguments, budget)?;
        let arguments = condition
            .arguments(arguments)
            .map_err(|e| Error::new(ErrorKind::Contract, e))?;
        let image = condition.executable();
        let mut session = sessions.start(image, &arguments, context, limits, budget)?;
        let result = loop {
            budget.charge_work(1)?;
            match session.resume(budget) {
                SessionProgress::Pending => {}
                SessionProgress::Complete(value) => break (*value)?,
                SessionProgress::Finished => {
                    return Err(Error::new(
                        ErrorKind::Contract,
                        "Finished predicate evaluation",
                    ));
                }
            }
        };
        budget.check_work()?;
        if !bool::from_wire(&result.value)? {
            let mut error = Error::new(kind, "Contract condition failed");
            error.span = image
                .program()
                .functions
                .get(image.program().entry.0 as usize)
                .map(|f| f.span.clone());
            return Err(error);
        }
    }
    Ok(())
}
