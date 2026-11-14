//! Adapter for the existing semantic read callback and its caller ledger.
use super::*;

pub(super) trait ReadCallMode<P: SemanticReadPolicy, S: SemanticReadSources> {
    type Work;
    const PREPARED_METADATA: bool = false;
    fn charge(work: &mut Self::Work) -> QueryResult<()>;
    fn observation(work: &mut Self::Work) -> InvocationContext<'_>;
    fn preflight_observation(_work: &mut Self::Work) -> Option<InvocationContext<'_>> {
        None
    }
    fn source_call<T>(
        sources: S,
        work: &mut Self::Work,
        run: impl FnOnce(&mut Self::Work) -> QueryResult<T>,
    ) -> QueryResult<T>;
    fn invoke(
        session: &SemanticReadSession<'_, P>,
        member: ReadMember<'_>,
        id: &str,
        physical: bool,
        sources: S,
        work: &mut Self::Work,
    ) -> QueryResult<Value>;
}

pub(super) struct LegacyReadMode;
impl<P: SemanticReadPolicy, S: SemanticReadSources> ReadCallMode<P, S> for LegacyReadMode {
    type Work = QueryBudget;
    const PREPARED_METADATA: bool = true;
    fn charge(work: &mut QueryBudget) -> QueryResult<()> {
        work.charge()
    }
    fn observation(work: &mut QueryBudget) -> InvocationContext<'_> {
        InvocationContext::new(Rc::new(BudgetObservation(RefCell::new(work))))
    }
    fn source_call<T>(
        sources: S,
        work: &mut QueryBudget,
        run: impl FnOnce(&mut QueryBudget) -> QueryResult<T>,
    ) -> QueryResult<T> {
        semantic_source_call(sources, work, run)
    }
    fn invoke(
        session: &SemanticReadSession<'_, P>,
        member: ReadMember<'_>,
        id: &str,
        physical: bool,
        sources: S,
        budget: &mut QueryBudget,
    ) -> QueryResult<Value> {
        let dependencies = Dependencies {
            session,
            actual: member.actual,
            id,
            allowed: session.policy.dependencies(member),
            sources,
        };
        if physical {
            session.policy.read(
                member,
                &PhysicalDependencies {
                    same: dependencies,
                    caller: member,
                },
                budget,
            )
        } else if session.policy.related_dependencies(member).is_empty()
            && session.policy.nominal_dependencies(member).is_empty()
            && session
                .policy
                .inverse_nominal_dependencies(member)
                .is_empty()
        {
            session.policy.read(member, &dependencies, budget)
        } else {
            session.policy.read(
                member,
                &RelatedDependencies {
                    same: dependencies,
                    caller: member,
                },
                budget,
            )
        }
    }
}
