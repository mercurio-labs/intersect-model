//! Explicit nominal kind filtering over the SAME stored occurrence/target worker.
//! Unlike physical getter dispatch, this never opens an inverse-container path
//! or chooses a target implementation. Old policies declare no such edges.
use super::*;

/// A read-only source kind predicate on one ordinary stored reference occurrence.
/// The nominal declaration is explicit; returned identities confer no getter or
/// model-validity authority. A nonmatching target is distinct from an absent one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NominalRead<'a> {
    pub key: &'a str,
    pub reference: &'a str,
    pub nominal: &'a str,
}

impl<P: SemanticReadPolicy> SemanticReadSession<'_, P> {
    pub(super) fn preflight_nominal_dependencies(
        &self,
        member: ReadMember<'_>,
        plan: ReadPlan,
        stack: &mut Vec<FeatureIdentityInput>,
        accounting: &ReadPreflight<'_>,
    ) -> QueryResult<()> {
        let edges = self.policy.nominal_dependencies(member);
        if edges.is_empty() {
            return Ok(());
        }
        // Fully observed snapshot publication retains its explicit unsupported
        // frontier. This native route does not upgrade that separate contract.
        accounting.unsupported_shape(true)?;
        if !matches!(plan, ReadPlan::Derived | ReadPlan::Delegated) {
            return Err("Nominal dependencies require an explicit computed getter".into());
        }
        for (index, edge) in edges.iter().enumerate() {
            accounting.step()?;
            let collision = self
                .policy
                .related_dependencies(member)
                .iter()
                .any(|e| e.key == edge.key)
                || self
                    .policy
                    .physical_dependencies(member)
                    .iter()
                    .any(|e| e.key == edge.key)
                || self
                    .policy
                    .classified_dependencies(member)
                    .iter()
                    .any(|e| e.key == edge.key)
                || self
                    .policy
                    .classified_cases_dependencies(member)
                    .iter()
                    .any(|e| e.key == edge.key)
                || self
                    .policy
                    .physical_related_dependencies(member)
                    .iter()
                    .any(|e| e.key == edge.key)
                || self
                    .policy
                    .collection_related_dependencies(member)
                    .iter()
                    .any(|e| e.key == edge.key);
            if edge.key.is_empty() || collision || edges[..index].iter().any(|e| e.key == edge.key)
            {
                return Err("Duplicate or empty nominal reference dependency key".into());
            }
            let (identity, owner, field) =
                accounting.selected(self, member.actual, edge.reference)?;
            if !field.reference
                || field.derived
                || self.schema().is_container_feature(field)?
                || self.policy.plan(ReadMember {
                    actual: member.actual,
                    owner,
                    identity: &identity,
                    feature: field,
                }) != ReadPlan::Stored
            {
                return Err("Nominal dependency requires an ordinary stored reference".into());
            }
            if !accounting.class(self.schema(), edge.nominal)?
                || !accounting.conforms(self.schema(), edge.nominal, &field.target)?
            {
                return Err("Invalid nominal reference kind declaration".into());
            }
            // Always ordinary mode, even if reached from a physical caller.
            self.preflight_member_accounted(
                member.actual,
                edge.reference,
                stack,
                false,
                accounting,
            )?;
        }
        Ok(())
    }
}

impl<P: SemanticReadPolicy, S: SemanticReadSources> RelatedDependencies<'_, '_, P, S> {
    pub(super) fn nominal_work<M: ReadCallMode<P, S>>(
        &self,
        key: &str,
        occurrence: usize,
        work: &mut M::Work,
    ) -> QueryResult<ClassifiedValue> {
        PhysicalDependencies {
            same: Dependencies {
                session: self.same.session,
                actual: self.same.actual,
                id: self.same.id,
                allowed: self.same.allowed,
                sources: self.same.sources,
            },
            caller: self.caller,
        }
        .nominal_work::<M>(key, occurrence, work)
    }
}

impl<P: SemanticReadPolicy, S: SemanticReadSources> PhysicalDependencies<'_, '_, P, S> {
    pub(super) fn nominal_work<M: ReadCallMode<P, S>>(
        &self,
        key: &str,
        occurrence: usize,
        work: &mut M::Work,
    ) -> QueryResult<ClassifiedValue> {
        M::source_call(self.same.sources, work, |work| {
            let session = self.same.session;
            let edges = session.policy.nominal_dependencies(self.caller);
            let mut selected = edges.iter().filter(|edge| edge.key == key);
            let edge = selected
                .next()
                .ok_or("Undeclared nominal reference dependency key")?;
            if selected.next().is_some() {
                return Err("Duplicate nominal reference dependency key".into());
            }
            M::charge(work)?;
            {
                let accounting = ReadPreflight::observed(M::preflight_observation(work))?;
                session.preflight_nominal_dependencies(
                    self.caller,
                    session.policy.plan(self.caller),
                    &mut Vec::new(),
                    &accounting,
                )?;
            }
            // SAME occurrence selector, explicitly ordinary mode. The declaration
            // above rules out inverse-container and derived/semantic source roles.
            let Some(value) =
                self.occurrence_work_mode::<M>(edge.reference, occurrence, work, false)?
            else {
                return Ok(ClassifiedValue::Absent);
            };
            match self.select_target_work::<M, _>(
                edge.reference,
                value,
                work,
                |actual, value, observation| {
                    if self.nominal_matches(actual, edge.nominal, observation)? {
                        Ok(ClassifiedValue::Match(value))
                    } else {
                        Ok(ClassifiedValue::NonMatch)
                    }
                },
            )? {
                Some(result) => Ok(result),
                None => Ok(ClassifiedValue::Absent),
            }
        })
    }
}
