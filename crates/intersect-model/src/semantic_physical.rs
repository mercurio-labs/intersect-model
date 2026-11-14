//! Invocation-only dependency routes for the same audited immutable graph.
use super::*;
use crate::definitions::schema::ConformancePreparation;

/// A genuine inverse-container declaration. Only the owning audited snapshot
/// opens this route; a raw store or serialized assertion cannot do so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalRead<'a> {
    pub key: &'a str,
    pub reference: &'a str,
}
/// Source nominal filtering, with an explicit exact implemented match receiver.
/// Legitimate nonmatches differ from unsupported matching implementations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClassifiedRead<'a> {
    pub key: &'a str,
    pub reference: &'a str,
    pub nominal: &'a str,
    pub admitted_actual: &'a str,
}
/// Finite implemented actual cases over the same nominal target selector.
/// Existing single-case descriptors and their work path remain unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClassifiedCasesRead<'a> {
    pub key: &'a str,
    pub reference: &'a str,
    pub nominal: &'a str,
    pub admitted_actuals: &'a [&'a str],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalRelatedCase<'a> {
    pub actual: &'a str,
    pub feature: &'a str,
}
/// Only a declared singular edge may dispatch another physical getter on the
/// same audited graph. Neither policy nor caller supplies the target identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalRelatedRead<'a> {
    pub key: &'a str,
    pub reference: &'a str,
    pub null: RelatedNull,
    pub cases: &'a [PhysicalRelatedCase<'a>],
}
/// A finite set of exact getters reached through a stored reference collection.
/// It is disabled by default and requires the same audited physical read mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CollectionRelatedRead<'a> {
    pub key: &'a str,
    pub reference: &'a str,
    pub nominal: &'a str,
    pub cases: &'a [PhysicalRelatedCase<'a>],
}
#[derive(Debug, PartialEq)]
pub enum ClassifiedValue {
    /// A semantic reference value, never a header or target-dispatch token.
    Match(Value),
    NonMatch,
    /// A source-defined null branch or exhausted collection, not requiredness.
    Absent,
}
pub(super) struct PhysicalDependencies<
    'a,
    's,
    P: SemanticReadPolicy,
    S: SemanticReadSources = NoReadSources,
> {
    pub(super) same: Dependencies<'a, 's, P, S>,
    pub(super) caller: ReadMember<'s>,
}
impl<P: SemanticReadPolicy, S: SemanticReadSources> PhysicalDependencies<'_, '_, P, S> {
    pub(super) fn occurrence_work<M: ReadCallMode<P, S>>(
        &self,
        reference: &str,
        occurrence: usize,
        budget: &mut M::Work,
    ) -> QueryResult<Option<Value>> {
        self.occurrence_work_mode::<M>(reference, occurrence, budget, true)
    }
    pub(super) fn occurrence_work_mode<M: ReadCallMode<P, S>>(
        &self,
        reference: &str,
        occurrence: usize,
        budget: &mut M::Work,
        physical: bool,
    ) -> QueryResult<Option<Value>> {
        let session = self.same.session;
        let AccountedSelection {
            identity,
            feature: field,
            ..
        } = if S::OBSERVED_PREFLIGHT {
            session.selected_with_hint_for::<M, S>(
                self.same.actual,
                reference,
                budget,
                self.same.sources,
            )?
        } else {
            let (identity, owner, feature) = session.selected_for::<M, S>(
                self.same.actual,
                reference,
                budget,
                self.same.sources,
            )?;
            AccountedSelection {
                identity,
                owner,
                feature,

                position: None,
            }
        };
        if field.upper == 1 {
            if occurrence != 0 {
                return Err("Singular classified reference requires occurrence zero".into());
            }
            return session
                .read_exact_worker::<M, S>(
                    self.same.actual,
                    self.same.id,
                    reference,
                    budget,
                    None,
                    physical,
                    self.same.sources,
                )
                .map(Some);
        }
        let observation = M::observation(budget);
        let selected = prepare_semantic_selection(
            self.same.actual,
            &identity,
            self.same.id,
            session.store,
            session.schema(),
            &observation,
            self.same.sources,
            None,
        )?;
        let accounting =
            ReadPreflight::observed(S::OBSERVED_PREFLIGHT.then(|| observation.clone()))?;
        if !accounting.text_equal(selected.actual(), self.same.actual)?
            || !accounting.member_equal(selected.member(), &identity)?
            || if S::OBSERVED_PREFLIGHT {
                !std::ptr::eq(
                    selected.definition_fully_observed(session.schema(), &observation)?,
                    field,
                )
            } else {
                selected.definition(session.schema())? != field
            }
        {
            return Err("Classified source collection selection changed".into());
        }
        selected.stored_occurrence(
            session.store,
            session.schema(),
            self.same.id,
            occurrence,
            &observation,
            S::OBSERVED_PREFLIGHT,
        )
    }
    // One charged wire/identity/domain selector for old and new routes.
    // `finish` cannot borrow the raw store or retain a target authority token.
    pub(super) fn select_target_work<M: ReadCallMode<P, S>, T>(
        &self,
        reference_name: &str,
        value: Value,
        budget: &mut M::Work,
        finish: impl FnOnce(&str, Value, &InvocationContext<'_>) -> QueryResult<T>,
    ) -> QueryResult<Option<T>> {
        M::charge(budget)?;
        if value.is_null() {
            return Ok(None);
        }
        let id = value
            .as_str()
            .ok_or("Classified reference contains a non-identity value")?;
        let session = self.same.session;
        M::charge(budget)?;
        if if S::OBSERVED_PREFLIGHT {
            let accounting = ReadPreflight::observed(M::preflight_observation(budget))?;
            let mut equal = false;
            session.store.inspect_revision(&mut |revision| {
                equal = accounting.text_equal(revision, &session.schema().revision)?;
                Ok(())
            })?;
            !equal
        } else {
            session.store.revision()? != session.schema().revision
        } {
            return Err("Classified target store/schema mismatch".into());
        }
        M::charge(budget)?;
        let mut actual = None;
        session.store.inspect(id, &mut |object| {
            let valid = if S::OBSERVED_PREFLIGHT {
                let observation = M::observation(budget);
                let accounting = ReadPreflight::observed(Some(observation.clone()))?;
                accounting.class(session.schema(), object.class())?
                    && session.schema().identity_matches_observed(
                        object.class(),
                        object.type_identity(),
                        &observation,
                    )?
            } else {
                object.type_identity() == session.schema().identity(object.class())
                    && session.schema().classes.contains_key(object.class())
            };
            if !valid {
                return Err("Classified target actual identity mismatch".into());
            }
            if S::OBSERVED_PREFLIGHT {
                let observation = M::observation(budget);
                crate::definitions::retained_storage::reserve_serialized(
                    object.class(),
                    &observation,
                )?;
                observation.checkpoint()?;
            }
            actual = Some(object.class().to_owned());
            Ok(())
        })?;
        let actual = actual.ok_or("Missing classified reference target")?;
        let field = if S::ENABLED {
            Some(session.selected_for::<M, S>(
                self.same.actual,
                reference_name,
                budget,
                self.same.sources,
            )?)
        } else {
            None
        };
        let observation = M::observation(budget);
        let mut reference = if let Some((identity, _, field)) = field {
            self.same.sources.reference_preparation(
                self.same.actual,
                reference_name,
                &identity,
                field,
                &actual,
                session.schema(),
                &observation,
            )?
        } else {
            ReferenceConformancePreparation::new_observed(
                self.same.actual,
                reference_name,
                &actual,
                &observation,
            )
            .map_err(|error| error.to_string())?
        };
        loop {
            observation.work(1)?;
            if reference
                .advance_observed(session.schema(), &observation)
                .map_err(|error| error.to_string())?
            {
                break;
            }
        }
        if !reference.finish()? {
            return Err(
                "Classified reference target does not conform to its declared domain".into(),
            );
        }
        finish(&actual, value, &observation).map(Some)
    }
    pub(super) fn nominal_matches(
        &self,
        actual: &str,
        nominal: &str,
        observation: &InvocationContext<'_>,
    ) -> QueryResult<bool> {
        let mut pending = ConformancePreparation::new_observed(actual, nominal, observation)
            .map_err(|error| error.to_string())?;
        while !pending
            .advance_observed(self.same.session.schema(), observation)
            .map_err(|error| error.to_string())?
        {}
        pending.finish()
    }
    pub(super) fn target_work<M: ReadCallMode<P, S>>(
        &self,
        edge: &ClassifiedRead<'_>,
        value: Value,
        budget: &mut M::Work,
    ) -> QueryResult<ClassifiedValue> {
        match self.select_target_work::<M, _>(
            edge.reference,
            value,
            budget,
            |actual, value, observation| {
                if !self.nominal_matches(actual, edge.nominal, observation)? {
                    return Ok(ClassifiedValue::NonMatch);
                }
                if !if S::OBSERVED_PREFLIGHT {
                    observation
                        .work(1 + actual.len().max(edge.admitted_actual.len()).div_ceil(64))?;
                    actual == edge.admitted_actual
                } else {
                    actual == edge.admitted_actual
                } {
                    return Err(format!("Unsupported matching classified target {actual}"));
                }
                Ok(ClassifiedValue::Match(value))
            },
        )? {
            Some(value) => Ok(value),
            None => Ok(ClassifiedValue::Absent),
        }
    }
    pub(super) fn classified_cases_work<M: ReadCallMode<P, S>>(
        &self,
        key: &str,
        occurrence: usize,
        budget: &mut M::Work,
    ) -> QueryResult<ClassifiedValue> {
        if M::preflight_observation(budget).is_some() {
            return Err("Unsupported dependency route in fully observed snapshot reads".into());
        }

        let edges = self
            .same
            .session
            .policy
            .classified_cases_dependencies(self.caller);
        let mut matches = edges.iter().filter(|edge| edge.key == key);
        let edge = matches
            .next()
            .ok_or("Undeclared classified dependency key")?;
        if matches.next().is_some() {
            return Err("Duplicate classified dependency key".into());
        }
        // New dispatch boundary: charge before any source occurrence callback.
        M::charge(budget)?;
        let Some(value) = self.occurrence_work::<M>(edge.reference, occurrence, budget)? else {
            return Ok(ClassifiedValue::Absent);
        };
        match self.select_target_work::<M, _>(
            edge.reference,
            value,
            budget,
            |actual, value, observation| {
                if !self.nominal_matches(actual, edge.nominal, observation)? {
                    return Ok(ClassifiedValue::NonMatch);
                }
                for admitted in edge.admitted_actuals {
                    observation.work(1)?;
                    if actual == *admitted {
                        return Ok(ClassifiedValue::Match(value));
                    }
                }
                Err(format!("Unsupported matching classified target {actual}"))
            },
        )? {
            Some(value) => Ok(value),
            None => Ok(ClassifiedValue::Absent),
        }
    }
}
impl<P: SemanticReadPolicy, S: SemanticReadSources> PhysicalDependencies<'_, '_, P, S> {
    pub(super) fn collection_related_value_work<M: ReadCallMode<P, S>>(
        &self,
        key: &str,
        occurrence: usize,
        budget: &mut M::Work,
        matches_receiver: bool,
    ) -> QueryResult<Option<Value>> {
        M::source_call(self.same.sources, budget, |budget| {
            if matches_receiver {
                M::charge(budget)?;
                let mut allowed = false;
                for permitted in self
                    .same
                    .session
                    .policy
                    .receiver_equality_dependencies(self.caller)
                {
                    M::charge(budget)?;
                    if ReadPreflight::observed(M::preflight_observation(budget))?
                        .text_equal(permitted, key)?
                    {
                        allowed = true;
                    }
                }
                if !allowed {
                    return Err("Undeclared receiver equality dependency".into());
                }
            }
            M::charge(budget)?;
            let edges = self
                .same
                .session
                .policy
                .collection_related_dependencies(self.caller);
            let mut selected_edge = None;
            for edge in edges {
                M::charge(budget)?;
                if ReadPreflight::observed(M::preflight_observation(budget))?
                    .text_equal(edge.key, key)?
                {
                    if selected_edge.is_some() {
                        return Err("Duplicate collection related dependency key".into());
                    }
                    selected_edge = Some(edge);
                }
            }
            let edge = selected_edge.ok_or("Undeclared collection related dependency key")?;
            if matches_receiver {
                for case in edge.cases {
                    M::charge(budget)?;
                    let (_, _, feature) = self.same.session.selected_for::<M, S>(
                        case.actual,
                        case.feature,
                        budget,
                        self.same.sources,
                    )?;
                    if !feature.reference
                        || feature.upper != 1
                        || !self.same.session.conforms_for::<M, S>(
                            self.same.actual,
                            &feature.target,
                            budget,
                        )?
                    {
                        return Err("Receiver equality selected a foreign reference domain".into());
                    }
                }
            }
            // Charge before the same checked occurrence selection/bounds query.
            M::charge(budget)?;
            let Some(value) = self.occurrence_work::<M>(edge.reference, occurrence, budget)? else {
                return Ok(None);
            };
            let selected = self.select_target_work::<M, _>(
                edge.reference,
                value,
                budget,
                |actual, value, observation| {
                    if !self.nominal_matches(actual, edge.nominal, observation)? {
                        return Err(
                            "Collection related target violates its nominal declaration".into()
                        );
                    }
                    for case in edge.cases {
                        observation.work(1)?;
                        if S::OBSERVED_PREFLIGHT {
                            observation
                                .work(1 + actual.len().max(case.actual.len()).div_ceil(64))?;
                        }
                        if actual == case.actual {
                            return Ok((case.actual, case.feature, value));
                        }
                    }
                    Err(format!(
                        "Unsupported collection related actual target {actual}"
                    ))
                },
            )?;
            let (actual, feature, value) = selected.ok_or("Null collection related target")?;
            let object = value
                .as_str()
                .ok_or("Collection related target has no identity")?;
            M::charge(budget)?;
            let value = self.same.session.read_exact_worker::<M, S>(
                actual,
                object,
                feature,
                budget,
                None,
                true,
                self.same.sources,
            )?;
            if matches_receiver {
                M::charge(budget)?;
                return match value {
                    Value::Null => Ok(Some(Value::Bool(false))),
                    Value::String(target) => Ok(Some(Value::Bool(
                        ReadPreflight::observed(M::preflight_observation(budget))?
                            .text_equal(&target, self.same.id)?,
                    ))),
                    _ => Err("Receiver equality getter returned a non-reference value".into()),
                };
            }
            Ok(Some(value))
        })
    }
}
impl<P: SemanticReadPolicy, S: SemanticReadSources> ReadDependencies
    for PhysicalDependencies<'_, '_, P, S>
{
    fn read_inverse_nominal(
        &self,
        key: &str,
        occurrence: usize,
        budget: &mut QueryBudget,
    ) -> QueryResult<ClassifiedValue> {
        self.inverse_nominal_work::<LegacyReadMode>(key, occurrence, budget)
    }
    fn read_nominal(
        &self,
        key: &str,
        occurrence: usize,
        budget: &mut QueryBudget,
    ) -> QueryResult<ClassifiedValue> {
        self.nominal_work::<LegacyReadMode>(key, occurrence, budget)
    }
    fn read_collection_related(
        &self,
        key: &str,
        occurrence: usize,
        budget: &mut QueryBudget,
    ) -> QueryResult<Option<Value>> {
        self.read_collection_related_work::<LegacyReadMode>(key, occurrence, budget)
    }
    fn read_collection_related_matches_receiver(
        &self,
        key: &str,
        occurrence: usize,
        budget: &mut QueryBudget,
    ) -> QueryResult<Option<bool>> {
        self.read_collection_related_matches_receiver_work::<LegacyReadMode>(
            key, occurrence, budget,
        )
    }
    fn read(&self, feature: &str, budget: &mut QueryBudget) -> QueryResult<Value> {
        self.read_work::<LegacyReadMode>(feature, budget)
    }
    fn read_related(
        &self,
        key: &str,
        occurrence: usize,
        budget: &mut QueryBudget,
    ) -> QueryResult<Value> {
        self.read_related_work::<LegacyReadMode>(key, occurrence, budget)
    }
    fn read_related_physical(
        &self,
        key: &str,
        occurrence: usize,
        budget: &mut QueryBudget,
    ) -> QueryResult<Value> {
        self.read_related_physical_work::<LegacyReadMode>(key, occurrence, budget)
    }
    fn read_physical(&self, key: &str, budget: &mut QueryBudget) -> QueryResult<Value> {
        self.read_physical_work::<LegacyReadMode>(key, budget)
    }
    fn read_classified(
        &self,
        key: &str,
        occurrence: usize,
        budget: &mut QueryBudget,
    ) -> QueryResult<ClassifiedValue> {
        self.read_classified_work::<LegacyReadMode>(key, occurrence, budget)
    }
}

impl<P: SemanticReadPolicy, S: SemanticReadSources> PhysicalDependencies<'_, '_, P, S> {
    pub(super) fn read_collection_related_work<M: ReadCallMode<P, S>>(
        &self,
        key: &str,
        occurrence: usize,
        budget: &mut M::Work,
    ) -> QueryResult<Option<Value>> {
        self.collection_related_value_work::<M>(key, occurrence, budget, false)
    }
    pub(super) fn read_collection_related_matches_receiver_work<M: ReadCallMode<P, S>>(
        &self,
        key: &str,
        occurrence: usize,
        budget: &mut M::Work,
    ) -> QueryResult<Option<bool>> {
        match self.collection_related_value_work::<M>(key, occurrence, budget, true)? {
            None => Ok(None),
            Some(Value::Bool(value)) => Ok(Some(value)),
            _ => Err("Receiver equality result projection changed".into()),
        }
    }
    pub(super) fn read_work<M: ReadCallMode<P, S>>(
        &self,
        feature: &str,
        budget: &mut M::Work,
    ) -> QueryResult<Value> {
        M::source_call(self.same.sources, budget, |budget| {
            if !ReadPreflight::observed(M::preflight_observation(budget))?
                .any_key(self.same.allowed.iter().copied(), feature)?
            {
                return Err(format!("Undeclared semantic read dependency {feature}"));
            }
            self.same.session.read_exact_worker::<M, S>(
                self.same.actual,
                self.same.id,
                feature,
                budget,
                None,
                true,
                self.same.sources,
            )
        })
    }
    pub(super) fn read_related_work<M: ReadCallMode<P, S>>(
        &self,
        key: &str,
        occurrence: usize,
        budget: &mut M::Work,
    ) -> QueryResult<Value> {
        if M::preflight_observation(budget).is_some() {
            return Err("Unsupported dependency route in fully observed snapshot reads".into());
        }

        // Preserve the exact N2 singular route and target restrictions. That
        // route cannot grant physical-container authority to another graph.
        RelatedDependencies {
            same: Dependencies {
                session: self.same.session,
                actual: self.same.actual,
                id: self.same.id,
                allowed: self.same.allowed,
                sources: self.same.sources,
            },
            caller: self.caller,
        }
        .read_related_work::<M>(key, occurrence, budget)
    }
    pub(super) fn read_related_physical_work<M: ReadCallMode<P, S>>(
        &self,
        key: &str,
        occurrence: usize,
        budget: &mut M::Work,
    ) -> QueryResult<Value> {
        if M::preflight_observation(budget).is_some() {
            return Err("Unsupported dependency route in fully observed snapshot reads".into());
        }

        M::source_call(self.same.sources, budget, |budget| {
            let session = self.same.session;
            let edges = session.policy.physical_related_dependencies(self.caller);
            let mut matches = edges.iter().filter(|edge| edge.key == key);
            let edge = matches
                .next()
                .ok_or("Undeclared physical related dependency key")?;
            if matches.next().is_some() {
                return Err("Duplicate physical related dependency key".into());
            }
            if occurrence != 0 {
                return Err("Physical related singular reference requires occurrence zero".into());
            }
            // New boundary and each exact case selection have explicit precharges.
            M::charge(budget)?;
            let Some(value) = self.occurrence_work::<M>(edge.reference, occurrence, budget)? else {
                return match edge.null {
                    RelatedNull::ReturnNull => Ok(Value::Null),
                    RelatedNull::Reject => Err("Physical related reference is absent".into()),
                };
            };
            let selected = self.select_target_work::<M, _>(
                edge.reference,
                value,
                budget,
                |actual, value, observation| {
                    for case in edge.cases {
                        observation.work(1)?;
                        if S::OBSERVED_PREFLIGHT {
                            observation
                                .work(1 + actual.len().max(case.actual.len()).div_ceil(64))?;
                        }
                        if actual == case.actual {
                            return Ok((case.actual, case.feature, value));
                        }
                    }
                    Err(format!("Unsupported physical related target {actual}"))
                },
            )?;
            let Some((actual, feature, value)) = selected else {
                return match edge.null {
                    RelatedNull::ReturnNull => Ok(Value::Null),
                    RelatedNull::Reject => Err("Physical related reference is absent".into()),
                };
            };
            let id = value
                .as_str()
                .ok_or("Physical related reference has no identity")?;
            session.read_exact_worker::<M, S>(
                actual,
                id,
                feature,
                budget,
                None,
                true,
                self.same.sources,
            )
        })
    }
    pub(super) fn read_physical_work<M: ReadCallMode<P, S>>(
        &self,
        key: &str,
        budget: &mut M::Work,
    ) -> QueryResult<Value> {
        if M::preflight_observation(budget).is_some() {
            return Err("Unsupported dependency route in fully observed snapshot reads".into());
        }

        M::source_call(self.same.sources, budget, |budget| {
            let session = self.same.session;
            let roles = session.policy.physical_dependencies(self.caller);
            let mut matches = roles.iter().filter(|role| role.key == key);
            let role = matches.next().ok_or("Undeclared physical dependency key")?;
            if matches.next().is_some() {
                return Err("Duplicate physical dependency key".into());
            }
            let (identity, _, field) = session.selected(self.same.actual, role.reference)?;
            // The same complete physical audit established reciprocal opposites,
            // unique ownership and cycles for this exact retained graph. Jointly
            // require the source's genuine inverse/container declaration here.
            if !field.reference
                || field.derived
                || field.upper != 1
                || !session.schema().is_container_feature(field)?
            {
                return Err("Physical dependency is not a source-defined inverse container".into());
            }
            let observation = M::observation(budget);
            let identity_input = FeatureIdentityInput::Owned(identity);
            let selected = prepare_semantic_selection(
                self.same.actual,
                &identity_input,
                self.same.id,
                session.store,
                session.schema(),
                &observation,
                self.same.sources,
                None,
            )?;
            if selected.actual() != self.same.actual
                || selected.member() != &*identity_input
                || selected.definition(session.schema())? != field
            {
                return Err("Physical container selection changed".into());
            }
            selected.stored(
                session.store,
                session.schema(),
                self.same.id,
                &observation,
                false,
            )
        })
    }
    pub(super) fn read_classified_work<M: ReadCallMode<P, S>>(
        &self,
        key: &str,
        occurrence: usize,
        budget: &mut M::Work,
    ) -> QueryResult<ClassifiedValue> {
        M::source_call(self.same.sources, budget, |budget| {
            let edges = self
                .same
                .session
                .policy
                .classified_dependencies(self.caller);
            let selected = {
                let accounting = ReadPreflight::observed(M::preflight_observation(budget))?;
                let mut selected = None;
                for edge in edges {
                    if accounting.text_equal(edge.key, key)? {
                        if selected.is_some() {
                            return Err("Duplicate classified dependency key".into());
                        }
                        selected = Some(edge);
                    }
                }
                selected
            };
            let Some(edge) = selected else {
                return self.classified_cases_work::<M>(key, occurrence, budget);
            };
            {
                let accounting = ReadPreflight::observed(M::preflight_observation(budget))?
                    .with_sources(&self.same.sources);
                self.same.session.preflight_member_accounted(
                    self.same.actual,
                    edge.reference,
                    &mut Vec::new(),
                    true,
                    &accounting,
                )?;
            }
            match self.occurrence_work::<M>(edge.reference, occurrence, budget)? {
                Some(value) => self.target_work::<M>(edge, value, budget),
                None => Ok(ClassifiedValue::Absent),
            }
        })
    }
}
