//! Handwritten typed navigation over shared storage projections.
use intersect_model::core::storage::{
    ModelRead, Query, StoredFeaturePreparation, StoredFeatureSelection,
};
use intersect_model::definitions::conversion::InvocationContext;
use intersect_model::definitions::schema::{
    ConformancePreparation, EffectiveFeaturePreparation, MemberIdentity, PreparationError,
    ReferenceConformancePreparation, SelectedFeature,
};
use serde_json::Value;

/// Matches are provisional until `Done`. Retain them privately for atomic results.
#[derive(Debug, PartialEq, Eq)]
pub enum IncomingStep {
    Pending,
    Match(String),
    Done,
}

enum SetupStage {
    Header,
    Members,
    Scan,
    Target,
    Domain,
    Done,
}
/// Resumable inverse selector/target qualification over one immutable admitted view.
/// Adapter calls and individual identity atoms remain atomic; errors poison it.
pub struct IncomingPreparation {
    source: String,
    member: MemberIdentity,
    target: String,
    target_class: String,
    field: String,
    revision: String,
    stage: SetupStage,
    members: Option<EffectiveFeaturePreparation>,
    scan: Option<std::vec::IntoIter<SelectedFeature>>,
    selected: Option<SelectedFeature>,
    domain: Option<ReferenceConformancePreparation>,
    failed: bool,
}
impl IncomingPreparation {
    pub fn new_observed(
        source: &str,
        member: &MemberIdentity,
        target: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<Self, String> {
        reserve_copy(&(source, member, target), observation)?;
        Ok(Self::new(source, member.clone(), target))
    }
    pub fn new(source: &str, member: MemberIdentity, target: &str) -> Self {
        Self {
            source: source.into(),
            member,
            target: target.into(),
            target_class: String::new(),
            field: String::new(),
            revision: String::new(),
            stage: SetupStage::Header,
            members: None,
            scan: None,
            selected: None,
            domain: None,
            failed: false,
        }
    }
    pub fn advance(
        &mut self,
        store: &dyn ModelRead,
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
    ) -> Result<bool, String> {
        self.advance_observed(store, schema, &InvocationContext::default())
    }
    pub fn advance_observed(
        &mut self,
        store: &dyn ModelRead,
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, String> {
        if self.failed {
            return Err("Inverse preparation has failed".into());
        }
        let result = observation
            .checkpoint()
            .and_then(|_| self.advance_inner(store, schema, observation));
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner(
        &mut self,
        store: &dyn ModelRead,
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, String> {
        observation.work(1)?;
        match self.stage {
            SetupStage::Header => {
                if !intersect_model::core::storage::revision_matches(store, &schema.revision)? {
                    return Err("Inverse query schema revision mismatch".into());
                }
                reserve_copy(&schema.revision, observation)?;
                self.revision = schema.revision.clone();
                self.members = Some(
                    EffectiveFeaturePreparation::new_observed(&self.source, observation)
                        .map_err(|e| e.to_string())?,
                );
                self.stage = SetupStage::Members;
            }
            SetupStage::Members => {
                if self
                    .members
                    .as_mut()
                    .ok_or("Missing inverse member preparation")?
                    .advance_observed(schema, observation)
                    .map_err(|e| e.to_string())?
                {
                    self.scan = Some(
                        self.members
                            .take()
                            .ok_or("Missing inverse member preparation")?
                            .finish()?
                            .into_iter(),
                    );
                    self.stage = SetupStage::Scan;
                }
            }
            SetupStage::Scan => {
                if let Some(member) = self
                    .scan
                    .as_mut()
                    .ok_or("Missing inverse member scan")?
                    .next()
                {
                    if member.identity == self.member {
                        self.selected = Some(member);
                    }
                } else {
                    self.scan = None;
                    let selected = self.selected.take().ok_or_else(|| {
                        format!(
                            "Unknown feature declaration in {}: {:?}",
                            self.source, self.member
                        )
                    })?;
                    let field = selected.resolve(schema)?.feature;
                    if field.derived || !field.reference {
                        return Err("Inverse query requires a stored reference member".into());
                    }
                    reserve_copy(&field.name, observation)?;
                    self.field = field.name.clone();
                    self.stage = SetupStage::Target;
                }
            }
            SetupStage::Target => {
                let found = store.inspect_class(&self.target, &mut |actual| {
                    reserve_copy(actual, observation)?;
                    self.domain = Some(
                        ReferenceConformancePreparation::new_observed(
                            &self.source,
                            &self.field,
                            actual,
                            observation,
                        )
                        .map_err(|e| e.to_string())?,
                    );
                    self.target_class = actual.to_owned();
                    Ok(())
                })?;
                if !found {
                    return Err("Missing inverse query target".into());
                }
                self.stage = SetupStage::Domain;
            }
            SetupStage::Domain => {
                if self
                    .domain
                    .as_mut()
                    .ok_or("Missing inverse target qualification")?
                    .advance_observed(schema, observation)
                    .map_err(|e| e.to_string())?
                {
                    if !self
                        .domain
                        .take()
                        .ok_or("Missing inverse target qualification")?
                        .finish()?
                    {
                        return Err(
                            "Inverse query target does not conform to the reference domain".into(),
                        );
                    }
                    self.stage = SetupStage::Done;
                }
            }
            SetupStage::Done => return Ok(true),
        }
        Ok(matches!(self.stage, SetupStage::Done))
    }
    pub fn finish(self) -> Result<IncomingCursor, String> {
        if self.failed || !matches!(self.stage, SetupStage::Done) {
            return Err("Inverse preparation is not complete".into());
        }
        Ok(IncomingCursor {
            source: self.source,
            member: self.member,
            target: Value::String(self.target),
            target_class: self.target_class,
            field: self.field,
            revision: self.revision,
            after: None,
            pending: None,
            exhausted: false,
            failed: false,
        })
    }
}
enum Candidate {
    Conformance {
        id: String,
        exact: bool,
        preparation: ConformancePreparation,
    },
    Projection {
        id: String,
        exact: bool,
        preparation: StoredFeaturePreparation,
    },
    Read {
        id: String,
        exact: bool,
        selection: StoredFeatureSelection,
        offset: Option<usize>,
    },
    Match {
        id: String,
        preparation: ReferenceConformancePreparation,
    },
}
/// Inverse traversal in adapter row order, once per source. Supply the same
/// immutable admitted schema/read view on every advance. Index hits still require
/// exact typed member and effective-domain qualification. Candidate/edge paging
/// and each schema continuation advance separately. Errors poison the cursor.
pub struct IncomingCursor {
    source: String,
    member: MemberIdentity,
    target: Value,
    target_class: String,
    field: String,
    revision: String,
    after: Option<u64>,
    pending: Option<Candidate>,
    exhausted: bool,
    failed: bool,
}
impl IncomingCursor {
    /// Synchronous compatibility facade over the shared preparation.
    pub fn new(
        store: &dyn ModelRead,
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        source: &str,
        member: MemberIdentity,
        target: &str,
    ) -> Result<Self, String> {
        let mut preparation = IncomingPreparation::new(source, member, target);
        while !preparation.advance(store, schema)? {}
        preparation.finish()
    }
    pub(crate) fn qualification_atom_bytes(&self) -> usize {
        self.field.len().saturating_add(self.target_class.len())
    }
    pub fn advance(
        &mut self,
        store: &dyn ModelRead,
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
    ) -> Result<IncomingStep, String> {
        self.advance_observed(store, schema, &InvocationContext::default())
    }
    pub fn advance_observed(
        &mut self,
        store: &dyn ModelRead,
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        observation: &InvocationContext<'_>,
    ) -> Result<IncomingStep, String> {
        if self.failed {
            return Err("Inverse cursor has failed".into());
        }
        let result = observation
            .checkpoint()
            .and_then(|_| self.advance_inner(store, schema, observation));
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner(
        &mut self,
        store: &dyn ModelRead,
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        observation: &InvocationContext<'_>,
    ) -> Result<IncomingStep, String> {
        observation.work(1)?;
        if schema.revision != self.revision
            || !intersect_model::core::storage::revision_matches(store, &self.revision)?
        {
            return Err("Inverse query schema revision mismatch".into());
        }
        if let Some(candidate) = self.pending.take() {
            match candidate {
                Candidate::Conformance {
                    id,
                    exact,
                    mut preparation,
                } => {
                    match preparation.advance_observed(schema, observation) {
                        Ok(false) => {
                            self.pending = Some(Candidate::Conformance {
                                id,
                                exact,
                                preparation,
                            })
                        }
                        Ok(true) => {
                            if preparation.finish()? {
                                self.pending = Some(Candidate::Projection {
                                    preparation: StoredFeaturePreparation::new_observed(
                                        &self.source,
                                        &self.member,
                                        &id,
                                        observation,
                                    )?,
                                    id,
                                    exact,
                                });
                            }
                        }
                        Err(PreparationError::Observation(error)) => return Err(error),
                        Err(PreparationError::Schema(_)) => {} // Preserve the false class predicate for invalid ancestry.
                    }
                }
                Candidate::Projection {
                    id,
                    exact,
                    mut preparation,
                } => {
                    if preparation.advance_observed(store, schema, observation)? {
                        self.pending = Some(Candidate::Read {
                            id,
                            exact,
                            selection: preparation.finish()?,
                            offset: None,
                        });
                    } else {
                        self.pending = Some(Candidate::Projection {
                            id,
                            exact,
                            preparation,
                        });
                    }
                }
                Candidate::Read {
                    id,
                    exact,
                    selection,
                    offset,
                } => {
                    let projection =
                        selection.projection_observed(store, schema, &id, observation)?;
                    if exact {
                        self.pending = Some(self.match_candidate(store, id, observation)?);
                    } else if offset.is_some() || projection.definition().upper != 1 {
                        let offset = offset.unwrap_or(0);
                        let page = projection.collection_page_observed(offset, 1, observation)?;
                        if page.len() > 1 {
                            return Err(
                                "Inverse query adapter exceeded collection page limit".into()
                            );
                        }
                        if let Some(value) = page.first() {
                            if !value.is_string() {
                                return Err(
                                    "Inverse query reference item is not an object identity".into(),
                                );
                            }
                            if value == &self.target {
                                self.pending =
                                    Some(self.match_candidate(store, id, observation)?);
                            } else {
                                self.pending = Some(Candidate::Read {
                                    id,
                                    exact: false,
                                    selection,
                                    offset: Some(
                                        offset
                                            .checked_add(1)
                                            .ok_or("Inverse query offset overflow")?,
                                    ),
                                });
                            }
                        }
                    } else {
                        let value = projection.singular_observed(observation)?;
                        if !value.is_null() && !value.is_string() {
                            return Err("Inverse query reference is not an object identity".into());
                        }
                        if value == self.target {
                            self.pending = Some(self.match_candidate(store, id, observation)?);
                        }
                    }
                }
                Candidate::Match {
                    id,
                    mut preparation,
                } => {
                    if preparation
                        .advance_observed(schema, observation)
                        .map_err(|e| e.to_string())?
                    {
                        if !preparation.finish()? {
                            return Err(
                                "Inverse query edge violates the effective reference domain".into(),
                            );
                        }
                        return Ok(IncomingStep::Match(id));
                    }
                    self.pending = Some(Candidate::Match { id, preparation });
                }
            }
            return Ok(IncomingStep::Pending);
        }
        if self.exhausted {
            return Ok(IncomingStep::Done);
        }
        let batch = store.query_candidates_observed(
            Query::Value(&self.field, &self.target),
            self.after,
            1,
            observation,
        )?;
        if batch.rows.len() > 1
            || (!batch.exhausted && batch.after <= self.after)
            || batch.rows.iter().any(|(row, _)| {
                self.after.is_some_and(|after| *row <= after)
                    || batch.after.is_none_or(|after| *row > after)
            })
        {
            return Err("Inverse query adapter returned an invalid candidate cursor".into());
        }
        self.after = batch.after;
        self.exhausted = batch.exhausted;
        let Some((_, id)) = batch.rows.into_iter().next() else {
            return Ok(if self.exhausted {
                IncomingStep::Done
            } else {
                IncomingStep::Pending
            });
        };
        let mut preparation = None;
        let found = store.inspect_class(&id, &mut |actual| {
            preparation = Some(
                ConformancePreparation::new_observed(actual, &self.source, observation)
                    .map_err(|e| e.to_string())?,
            );
            Ok(())
        })?;
        if !found {
            return Err("Missing inverse query candidate".into());
        }
        self.pending = Some(Candidate::Conformance {
            preparation: preparation.ok_or("Missing inverse candidate class projection")?,
            id,
            exact: batch.exact,
        });
        Ok(IncomingStep::Pending)
    }
    fn match_candidate(
        &self,
        store: &dyn ModelRead,
        id: String,
        observation: &InvocationContext<'_>,
    ) -> Result<Candidate, String> {
        let mut preparation = None;
        let found = store.inspect_class(&id, &mut |actual| {
            preparation = Some(
                ReferenceConformancePreparation::new_observed(
                    actual,
                    &self.field,
                    &self.target_class,
                    observation,
                )
                .map_err(|e| e.to_string())?,
            );
            Ok(())
        })?;
        if !found {
            return Err("Missing inverse query candidate".into());
        }
        // Effective receiver policies must use the actual class, not its owner.
        Ok(Candidate::Match {
            preparation: preparation.ok_or("Missing inverse candidate class projection")?,
            id,
        })
    }
}
fn reserve_copy<T: ?Sized + serde::Serialize>(
    value: &T,
    observation: &InvocationContext<'_>,
) -> Result<(), String> {
    if observation.has_observer() {
        intersect_model::definitions::retained_storage::reserve_serialized(value, observation)?;
    }
    Ok(())
}
