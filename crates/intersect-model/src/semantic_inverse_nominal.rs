//! Opt-in local inverse ownership witness over a current immutable model view.
//! Ordinary nominal and audited physical paths retain their existing contracts.
use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InverseNominalRead<'a> {
    pub key: &'a str,
    pub reference: &'a str,
    pub nominal: &'a str,
}
impl<P: SemanticReadPolicy> SemanticReadSession<'_, P> {
    pub(super) fn preflight_inverse_nominal_dependencies(
        &self,
        member: ReadMember<'_>,
        plan: ReadPlan,
        stack: &mut Vec<FeatureIdentityInput>,
        accounting: &ReadPreflight<'_>,
    ) -> QueryResult<()> {
        let edges = self.policy.inverse_nominal_dependencies(member);
        if edges.is_empty() {
            return Ok(());
        }
        accounting.unsupported_shape(true)?;
        if !matches!(plan, ReadPlan::Derived | ReadPlan::Delegated) {
            return Err("Inverse nominal dependencies require an explicit computed getter".into());
        }
        for (index, edge) in edges.iter().enumerate() {
            accounting.step()?;
            let collision = self
                .policy
                .nominal_dependencies(member)
                .iter()
                .any(|e| e.key == edge.key)
                || self
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
                return Err("Duplicate or empty inverse nominal dependency key".into());
            }
            let (identity, owner, field) =
                accounting.selected(self, member.actual, edge.reference)?;
            if !matches!(&*identity, MemberIdentity::Source { .. })
                || !field.reference
                || field.derived
                || field.containment
                || field.upper != 1
                || !self.schema().is_container_feature(field)?
                || self.policy.plan(ReadMember {
                    actual: member.actual,
                    owner,
                    identity: &identity,
                    feature: field,
                }) != ReadPlan::Stored
            {
                return Err(
                    "Inverse nominal dependency requires a source-defined stored inverse container"
                        .into(),
                );
            }
            if !accounting.class(self.schema(), edge.nominal)?
                || !accounting.conforms(self.schema(), edge.nominal, &field.target)?
            {
                return Err("Invalid inverse nominal kind declaration".into());
            }
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
    pub(super) fn inverse_nominal_work<M: ReadCallMode<P, S>>(
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
        .inverse_nominal_work::<M>(key, occurrence, work)
    }
}
impl<P: SemanticReadPolicy, S: SemanticReadSources> PhysicalDependencies<'_, '_, P, S> {
    pub(super) fn inverse_nominal_work<M: ReadCallMode<P, S>>(
        &self,
        key: &str,
        occurrence: usize,
        work: &mut M::Work,
    ) -> QueryResult<ClassifiedValue> {
        M::source_call(self.same.sources, work, |work| {
            if occurrence != 0 {
                return Err("Inverse nominal reference requires occurrence zero".into());
            }
            let session = self.same.session;
            let mut selected = session
                .policy
                .inverse_nominal_dependencies(self.caller)
                .iter()
                .filter(|e| e.key == key);
            let edge = selected
                .next()
                .ok_or("Undeclared inverse nominal dependency key")?;
            if selected.next().is_some() {
                return Err("Duplicate inverse nominal dependency key".into());
            }
            {
                let accounting = ReadPreflight::observed(M::preflight_observation(work))?;
                session.preflight_inverse_nominal_dependencies(
                    self.caller,
                    session.policy.plan(self.caller),
                    &mut Vec::new(),
                    &accounting,
                )?;
            }
            // Ordinary stored occurrence only. The local witness grants no
            // audited-graph capability or permission to dispatch target getters.
            let value = self
                .occurrence_work_mode::<M>(edge.reference, 0, work, false)?
                .unwrap_or(Value::Null);
            storage::checked_inverse_nominal_witness(
                session.store,
                session.schema(),
                self.same.id,
                self.same.actual,
                edge.reference,
                &value,
                &M::observation(work),
            )?;
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
