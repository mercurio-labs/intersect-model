//! Accounting adapter for the SAME dependency preflight and exact selector.
//! Old read modes use the literal unobserved schema helpers and old fees.
use super::*;
use crate::definitions::schema::{ConformancePreparation, EffectiveFeaturePreparation};

pub(super) struct ReadPreflight<'a> {
    observation: Option<InvocationContext<'a>>,
    // Independent of fully observed snapshot semantics/shape admission.
    metadata_work: Option<InvocationContext<'a>>,
    selection: Option<&'a dyn ReadMetadataSelection>,
}
impl<'a> ReadPreflight<'a> {
    pub(super) fn legacy() -> Self {
        Self {
            observation: None,
            metadata_work: None,
            selection: None,
        }
    }
    pub(super) fn observed(observation: Option<InvocationContext<'a>>) -> QueryResult<Self> {
        if let Some(context) = &observation {
            context.work(1)?;
            context.retain_storage(crate::logical_size!(Self, 64))?;
            context.checkpoint()?;
        }
        Ok(Self {
            observation,
            metadata_work: None,
            selection: None,
        })
    }
    fn prepared(context: InvocationContext<'a>) -> QueryResult<Self> {
        context.work(1)?;
        context.retain_storage(crate::logical_size!(Self, 64))?;
        context.checkpoint()?;
        Ok(Self {
            observation: None,
            metadata_work: Some(context),
            selection: None,
        })
    }
    fn prepared_selection<'s, P: SemanticReadPolicy>(
        &self,
        session: &'s SemanticReadSession<'_, P>,
        actual: &str,
        name: &str,
    ) -> QueryResult<Option<AccountedSelection<'s>>> {
        let (Some(work), Some(metadata)) = (&self.metadata_work, session.metadata) else {
            return Ok(None);
        };
        let Some(selected) =
            metadata.semantic_feature_observed(session.schema(), actual, name, work)?
        else {
            return Ok(None);
        };
        Ok(Some(AccountedSelection {
            identity: FeatureIdentityInput::Owned(selected.identity),
            owner: selected.owner,
            feature: selected.feature,

            position: Some(selected.position),
        }))
    }
    pub(super) fn with_sources(mut self, selection: &'a dyn ReadMetadataSelection) -> Self {
        self.selection = Some(selection);
        self
    }
    pub(super) fn is_observed(&self) -> bool {
        self.observation.is_some()
    }
    pub(super) fn step(&self) -> QueryResult<()> {
        if let Some(o) = &self.observation {
            o.work(1)?;
        }
        Ok(())
    }
    pub(super) fn unsupported_shape(&self, nonempty: bool) -> QueryResult<()> {
        if self.observation.is_some() && nonempty {
            return Err("Unsupported dependency shape in fully observed snapshot preflight".into());
        }
        Ok(())
    }
    pub(super) fn text_equal(&self, a: &str, b: &str) -> QueryResult<bool> {
        if let Some(o) = &self.observation {
            o.work(1 + a.len().max(b.len()).div_ceil(64))?;
        }
        Ok(a == b)
    }
    pub(super) fn any_key<'s>(
        &self,
        values: impl IntoIterator<Item = &'s str>,
        key: &str,
    ) -> QueryResult<bool> {
        for value in values {
            if self.text_equal(value, key)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub(super) fn class(&self, schema: &Schema, name: &str) -> QueryResult<bool> {
        if let Some(o) = &self.observation {
            o.work(
                1 + (usize::BITS - schema.classes.len().leading_zeros()) as usize
                    + name.len().div_ceil(64),
            )?;
        }
        Ok(schema.classes.contains_key(name))
    }
    pub(super) fn conforms(
        &self,
        schema: &Schema,
        actual: &str,
        nominal: &str,
    ) -> QueryResult<bool> {
        let Some(o) = &self.observation else {
            return Ok(schema.conforms(actual, nominal));
        };
        let mut p =
            ConformancePreparation::new_observed(actual, nominal, o).map_err(|e| e.to_string())?;
        while !p.advance_observed(schema, o).map_err(|e| e.to_string())? {}
        p.finish()
    }
    pub(super) fn selected<'s, P: SemanticReadPolicy>(
        &self,
        session: &'s SemanticReadSession<'_, P>,
        actual: &'s str,
        name: &str,
    ) -> QueryResult<(FeatureIdentityInput, &'s str, &'s Feature)> {
        if let Some(selected) = self.prepared_selection(session, actual, name)? {
            return Ok((selected.identity, selected.owner, selected.feature));
        }
        let Some(o) = &self.observation else {
            return session
                .selected(actual, name)
                .map(|(identity, owner, feature)| {
                    (FeatureIdentityInput::Owned(identity), owner, feature)
                });
        };
        if let Some(source) = self.selection
            && let Some(field) = source.selected_metadata(session.schema(), actual, name, o)?
        {
            return Ok(field);
        }
        let mut pending =
            EffectiveFeaturePreparation::new_observed(actual, o).map_err(|e| e.to_string())?;
        while !pending
            .advance_observed(session.schema(), o)
            .map_err(|e| e.to_string())?
        {}
        for selected in pending.finish()? {
            if self.text_equal(&selected.name, name)? {
                o.work(1)?;
                crate::definitions::schema::admit_feature_lookup_observed(
                    session.schema(),
                    &selected.owner,
                    &selected.name,
                    o,
                )?;
                let field = selected.resolve(session.schema())?;
                return Ok((
                    FeatureIdentityInput::Owned(field.identity),
                    field.owner,
                    field.feature,
                ));
            }
        }
        Err(format!("Unknown feature {actual}.{name}"))
    }
    // Source-less legacy preflight may retain this ordinal for qualified reads.
    // This is its SAME complete canonical selection, from the cursor's schema.
    pub(super) fn selected_with_position<'s, P: SemanticReadPolicy>(
        &self,
        session: &'s SemanticReadSession<'_, P>,
        actual: &'s str,
        name: &str,
    ) -> QueryResult<AccountedSelection<'s>> {
        if let Some(selected) = self.prepared_selection(session, actual, name)? {
            return Ok(selected);
        }
        if self.observation.is_some() {
            // Keep observed canonical traversal/admission literal; no ordinal
            // handoff may bypass its work, storage, or source accounting.
            return self.selected_with_hint(session, actual, name);
        }
        let selected = session.schema().effective_feature(actual, name)?;
        Ok(AccountedSelection {
            identity: FeatureIdentityInput::Owned(selected.identity),
            owner: selected.owner,
            feature: selected.feature,

            position: Some(selected.position),
        })
    }
    pub(super) fn selected_with_hint<'s, P: SemanticReadPolicy>(
        &self,
        session: &'s SemanticReadSession<'_, P>,
        actual: &'s str,
        name: &str,
    ) -> QueryResult<AccountedSelection<'s>> {
        if let (Some(o), Some(source)) = (&self.observation, self.selection)
            && let Some(selected) = source.selected_hint(session.schema(), actual, name, o)?
        {
            return Ok(selected);
        }
        let (identity, owner, feature) = self.selected(session, actual, name)?;
        Ok(AccountedSelection {
            identity,
            owner,
            feature,

            position: None,
        })
    }
    pub(super) fn member_equal(&self, a: &MemberIdentity, b: &MemberIdentity) -> QueryResult<bool> {
        match &self.observation {
            Some(o) => crate::definitions::schema::member_identity_equal_observed(a, b, o),
            None => Ok(a == b),
        }
    }
    pub(super) fn contains(
        &self,
        stack: &[FeatureIdentityInput],
        identity: &MemberIdentity,
    ) -> QueryResult<bool> {
        let Some(o) = &self.observation else {
            return Ok(stack.iter().any(|old| &**old == identity));
        };
        for old in stack {
            o.work(1)?;
            let equal =
                crate::definitions::schema::member_identity_equal_observed(old, identity, o)?;
            if equal {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub(super) fn push(
        &self,
        stack: &mut Vec<FeatureIdentityInput>,
        identity: &FeatureIdentityInput,
    ) -> QueryResult<()> {
        if let Some(o) = &self.observation {
            o.work(1)?;
            if stack.len() >= 64 {
                return Err("Observed semantic preflight depth exceeded".into());
            }
            let retained = identity.clone_observed(o)?;
            o.retain_storage(crate::logical_size!(FeatureIdentityInput, 96))?;
            o.checkpoint()?;
            stack
                .try_reserve_exact(1)
                .map_err(|_| "Semantic preflight stack allocation failed")?;
            stack.push(retained);
            return Ok(());
        }
        stack.push(identity.clone());
        Ok(())
    }
}
impl<P: SemanticReadPolicy> SemanticReadSession<'_, P> {
    pub(super) fn accounting_for<'work, M: work::ReadCallMode<P, S>, S: SemanticReadSources>(
        &self,
        budget: &'work mut M::Work,
        sources: &'work S,
    ) -> QueryResult<ReadPreflight<'work>> {
        let accounting = if self.metadata.is_some() && !S::ENABLED && M::PREPARED_METADATA {
            ReadPreflight::prepared(M::observation(budget))?
        } else {
            ReadPreflight::observed(M::preflight_observation(budget))?
        };
        Ok(accounting.with_sources(sources))
    }
    pub(super) fn selected_for<'s, M: work::ReadCallMode<P, S>, S: SemanticReadSources>(
        &'s self,
        actual: &'s str,
        name: &str,
        budget: &mut M::Work,
        sources: S,
    ) -> QueryResult<(FeatureIdentityInput, &'s str, &'s Feature)> {
        self.accounting_for::<M, S>(budget, &sources)?
            .selected(self, actual, name)
    }
    pub(super) fn selected_with_hint_for<
        's,
        M: work::ReadCallMode<P, S>,
        S: SemanticReadSources,
    >(
        &'s self,
        actual: &'s str,
        name: &str,
        budget: &mut M::Work,
        sources: S,
    ) -> QueryResult<AccountedSelection<'s>> {
        self.accounting_for::<M, S>(budget, &sources)?
            .selected_with_hint(self, actual, name)
    }
    pub(super) fn conforms_for<M: work::ReadCallMode<P, S>, S: SemanticReadSources>(
        &self,
        actual: &str,
        nominal: &str,
        budget: &mut M::Work,
    ) -> QueryResult<bool> {
        ReadPreflight::observed(M::preflight_observation(budget))?.conforms(
            self.schema(),
            actual,
            nominal,
        )
    }
}
