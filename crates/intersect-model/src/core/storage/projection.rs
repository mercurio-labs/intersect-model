//! Exact schema-member selection over an existing immutable read view.
//! Value/domain validation remains with the shared schema and consumer codecs.
use super::{ModelRead, StorageResult};
use intersect_model::definitions::conversion::InvocationContext;
use intersect_model::definitions::schema::{
    ConformancePreparation, EffectiveFeaturePreparation, Feature, FeatureIdentityInput,
    MemberIdentity, PreparationError, SelectedFeature, SelectedFeatureInput,
};
use serde_json::Value;

enum Stage {
    Header,
    Static,
    StaticScan,
    Actual,
    Conformance,
    Effective,
    EffectiveScan,
    Done,
}
/// Resumable exact stored-member qualification. Supply the same immutable schema
/// and read view throughout preparation and use. Adapter calls and individual
/// identity atoms remain atomic; errors poison the preparation.
pub struct StoredFeaturePreparation {
    receiver: String,
    member: FeatureIdentityInput,
    object: String,
    revision: String,
    actual: String,
    stage: Stage,
    members: Option<EffectiveFeaturePreparation>,
    scan: Option<std::vec::IntoIter<SelectedFeature>>,
    selected: Option<SelectedFeatureInput>,
    static_derived: bool,
    conformance: Option<ConformancePreparation>,
    failed: bool,
}
/// A selection for one immutable admitted view, produced only by completed
/// qualification. This is not a serialized authority or a graph validator.
pub struct StoredFeatureSelection {
    object: String,
    actual: String,
    revision: String,
    selected: SelectedFeatureInput,
}
impl StoredFeaturePreparation {
    pub fn new_observed(
        receiver: &str,
        member: &MemberIdentity,
        object: &str,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Self> {
        if observation.has_observer() {
            intersect_model::definitions::retained_storage::reserve_serialized(
                &(receiver, member, object),
                observation,
            )?;
        }
        Ok(Self::new(receiver, member, object))
    }
    pub fn new(receiver: &str, member: &MemberIdentity, object: &str) -> Self {
        Self::with_member(
            receiver,
            FeatureIdentityInput::Owned(member.clone()),
            object,
        )
    }
    fn with_member(receiver: &str, member: FeatureIdentityInput, object: &str) -> Self {
        Self {
            receiver: receiver.into(),
            member,
            object: object.into(),
            revision: String::new(),
            actual: String::new(),
            stage: Stage::Header,
            members: None,
            scan: None,
            selected: None,
            static_derived: false,
            conformance: None,
            failed: false,
        }
    }
    pub fn advance(
        &mut self,
        store: &dyn ModelRead,
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
    ) -> StorageResult<bool> {
        self.advance_observed(store, schema, &InvocationContext::default())
    }
    pub fn advance_observed(
        &mut self,
        store: &dyn ModelRead,
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<bool> {
        if self.failed {
            return Err("Stored feature preparation has failed".into());
        }
        self.advance_mode::<false>(store, schema, observation, true, false, None)
    }
    #[allow(clippy::too_many_arguments)]
    fn advance_mode<const REUSE_EXACT: bool>(
        &mut self,
        store: &dyn ModelRead,
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        observation: &InvocationContext<'_>,
        require_stored: bool,
        fully_observed: bool,
        preflight: Option<(&str, &Feature, usize)>,
    ) -> StorageResult<bool> {
        if self.failed {
            return Err("Stored feature preparation has failed".into());
        }
        let result = observation.checkpoint().and_then(|_| {
            self.advance_inner::<REUSE_EXACT>(
                store,
                schema,
                observation,
                require_stored,
                fully_observed,
                preflight,
            )
        });
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    #[allow(clippy::too_many_arguments)]
    fn advance_inner<const REUSE_EXACT: bool>(
        &mut self,
        store: &dyn ModelRead,
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        observation: &InvocationContext<'_>,
        require_stored: bool,
        fully_observed: bool,
        preflight: Option<(&str, &Feature, usize)>,
    ) -> StorageResult<bool> {
        observation.work(1)?;
        match self.stage {
            Stage::Header => {
                if fully_observed {
                    store.inspect_revision(&mut |revision| {
                        observation.work(1 + revision.len().max(schema.revision.len()).div_ceil(64))
                    })?;
                }
                if !super::revision_matches(store, &schema.revision)? {
                    return Err("Stored feature read schema revision mismatch".into());
                }
                reserve_text(&schema.revision, observation)?;
                self.revision = schema.revision.clone();
                // The private physical caller completed canonical preflight
                // against this SAME schema. Keep Header first, then copy only
                // its selected metadata into the existing cursor slot.
                if REUSE_EXACT && let Some((owner, field, position)) = preflight {
                    intersect_model::definitions::schema::admit_feature_lookup_observed(
                        schema,
                        owner,
                        &field.name,
                        observation,
                    )?;
                    let current = schema
                        .classes
                        .get(owner)
                        .and_then(|class| class.features.get(&field.name))
                        .ok_or("Missing preflight member declaration")?;
                    if !std::ptr::eq(current, field) {
                        return Err("Physical preflight member/schema owner changed".into());
                    }
                    let identity = self
                        .member
                        .clone_observed(observation)?
                        .into_owned_observed(observation)?;
                    intersect_model::definitions::retained_storage::reserve_serialized(
                        &(owner, &field.name, position),
                        observation,
                    )?;
                    observation.checkpoint()?;
                    self.selected = Some(SelectedFeatureInput::from_owned(SelectedFeature {
                        owner: owner.into(),
                        name: field.name.clone(),
                        identity,
                        position,
                    }));
                    self.static_derived = field.derived;
                    self.stage = Stage::Actual;
                    return Ok(false);
                }
                self.members = Some(
                    EffectiveFeaturePreparation::new_observed(&self.receiver, observation)
                        .map_err(|e| e.to_string())?,
                );
                self.stage = Stage::Static;
            }
            Stage::Static | Stage::Effective => {
                if self
                    .members
                    .as_mut()
                    .ok_or("Missing stored member preparation")?
                    .advance_observed(schema, observation)
                    .map_err(|e| e.to_string())?
                {
                    self.scan = Some(
                        self.members
                            .take()
                            .ok_or("Missing stored member preparation")?
                            .finish()?
                            .into_iter(),
                    );
                    self.stage = if matches!(self.stage, Stage::Static) {
                        Stage::StaticScan
                    } else {
                        Stage::EffectiveScan
                    };
                }
            }
            Stage::StaticScan | Stage::EffectiveScan => {
                let next = self
                    .scan
                    .as_mut()
                    .ok_or("Missing stored member scan")?
                    .next();
                if let Some(member) = next {
                    if member.identity == *self.member {
                        self.selected = Some(SelectedFeatureInput::from_owned(member));
                    }
                } else {
                    self.scan = None;
                    let kind = if matches!(self.stage, Stage::StaticScan) {
                        &self.receiver
                    } else {
                        &self.actual
                    };
                    let selected = self.selected.as_ref().ok_or_else(|| {
                        format!("Unknown feature declaration in {kind}: {:?}", *self.member)
                    })?;
                    if fully_observed {
                        intersect_model::definitions::schema::admit_feature_lookup_observed(
                            schema,
                            selected.owner(),
                            selected.name(),
                            observation,
                        )?;
                    }
                    let feature = schema
                        .classes
                        .get(selected.owner())
                        .and_then(|class| class.features.get(selected.name()))
                        .ok_or("Missing effective feature declaration")?;
                    if matches!(self.stage, Stage::StaticScan) {
                        self.static_derived = feature.derived;
                        if !REUSE_EXACT {
                            self.selected = None;
                        }
                        self.stage = Stage::Actual;
                    } else {
                        if require_stored && (self.static_derived || feature.derived) {
                            return Err(
                                "Derived feature read requires an executable language provider"
                                    .into(),
                            );
                        }
                        self.stage = Stage::Done;
                    }
                }
            }
            Stage::Actual => {
                let found = if require_stored {
                    store.inspect_class(&self.object, &mut |actual| {
                        reserve_text(actual, observation)?;
                        self.conformance = Some(
                            ConformancePreparation::new_observed(
                                actual,
                                &self.receiver,
                                observation,
                            )
                            .map_err(|e| e.to_string())?,
                        );
                        self.actual = actual.to_owned();
                        Ok(())
                    })?
                } else {
                    store.inspect(&self.object, &mut |object| {
                        let actual = object.class();
                        if if fully_observed {
                            !schema.identity_matches_observed(
                                actual,
                                object.type_identity(),
                                observation,
                            )?
                        } else {
                            object.type_identity() != schema.identity(actual)
                        } {
                            return Err("Semantic read actual receiver identity mismatch".into());
                        }
                        reserve_text(actual, observation)?;
                        self.conformance = Some(
                            ConformancePreparation::new_observed(
                                actual,
                                &self.receiver,
                                observation,
                            )
                            .map_err(|e| e.to_string())?,
                        );
                        self.actual = actual.to_owned();
                        Ok(())
                    })?
                };
                if !found {
                    return Err("Missing feature receiver object".into());
                }
                self.stage = Stage::Conformance;
            }
            Stage::Conformance => {
                let result = self
                    .conformance
                    .as_mut()
                    .ok_or("Missing stored receiver conformance")?
                    .advance_observed(schema, observation);
                match result {
                    Ok(false) => {}
                    Ok(true) => {
                        if !self
                            .conformance
                            .take()
                            .ok_or("Missing stored receiver conformance")?
                            .finish()?
                        {
                            return Err(
                                "Stored feature receiver does not conform to the selected type"
                                    .into(),
                            );
                        }
                        // Private same-call reference reads retain only the
                        // completed static metadata. Actual identity and full
                        // conformance above remain fresh; subtypes reselect.
                        if REUSE_EXACT {
                            observation.work(
                                1 + self.actual.len().max(self.receiver.len()).div_ceil(64),
                            )?;
                            if self.actual == self.receiver && self.selected.is_some() {
                                self.stage = Stage::Done;
                                return Ok(true);
                            }
                            self.selected = None;
                        }
                        self.members = Some(
                            EffectiveFeaturePreparation::new_observed(&self.actual, observation)
                                .map_err(|e| e.to_string())?,
                        );
                        self.stage = Stage::Effective;
                    }
                    Err(PreparationError::Observation(error)) => return Err(error),
                    Err(PreparationError::Schema(_)) => {
                        return Err(
                            "Stored feature receiver does not conform to the selected type".into(),
                        );
                    }
                }
            }
            Stage::Done => return Ok(true),
        }
        Ok(matches!(self.stage, Stage::Done))
    }
    pub fn finish(self) -> StorageResult<StoredFeatureSelection> {
        if self.failed || !matches!(self.stage, Stage::Done) {
            return Err("Stored feature preparation is not complete".into());
        }
        Ok(StoredFeatureSelection {
            object: self.object,
            actual: self.actual,
            revision: self.revision,
            selected: self.selected.ok_or("Missing stored member selection")?,
        })
    }
}
fn reserve_text(text: &str, observation: &InvocationContext<'_>) -> StorageResult<()> {
    if observation.has_observer() {
        intersect_model::definitions::retained_storage::reserve_serialized(text, observation)?;
    }
    Ok(())
}
impl StoredFeatureSelection {
    fn admit_definition_maps<A: intersect_model::definitions::schema::RuntimeAlternative>(
        &self,
        schema: &intersect_model::definitions::schema::SchemaRepresentation<A>,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<()> {
        intersect_model::definitions::schema::admit_feature_lookup_observed(
            schema,
            self.selected.owner(),
            self.selected.name(),
            observation,
        )
    }
    pub fn projection<'a>(
        &self,
        store: &'a dyn ModelRead,
        schema: &'a intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        object: &'a str,
    ) -> StorageResult<StoredFeatureProjection<'a>> {
        self.projection_observed(store, schema, object, &InvocationContext::default())
    }
    pub fn projection_observed<'a>(
        &self,
        store: &'a dyn ModelRead,
        schema: &'a intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        object: &'a str,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<StoredFeatureProjection<'a>> {
        self.projection_mode(store, schema, object, observation, false)
    }
    fn projection_mode<'a>(
        &self,
        store: &'a dyn ModelRead,
        schema: &'a intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        object: &'a str,
        observation: &InvocationContext<'_>,
        fully_observed: bool,
    ) -> StorageResult<StoredFeatureProjection<'a>> {
        if fully_observed {
            observation.work(1 + schema.revision.len().max(self.revision.len()).div_ceil(64))?;
            store.inspect_revision(&mut |revision| {
                observation.work(1 + revision.len().max(self.revision.len()).div_ceil(64))
            })?;
            observation.work(1 + object.len().max(self.object.len()).div_ceil(64))?;
        }
        if schema.revision != self.revision || !super::revision_matches(store, &self.revision)? {
            return Err("Stored feature read schema revision mismatch".into());
        }
        let mut same_class = false;
        if object == self.object {
            store.inspect_class(object, &mut |actual| {
                if fully_observed {
                    observation.work(1 + actual.len().max(self.actual.len()).div_ceil(64))?;
                }
                same_class = actual == self.actual;
                Ok(())
            })?;
        }
        if !same_class {
            return Err("Stored feature selection belongs to a different receiver".into());
        }
        if fully_observed {
            self.admit_definition_maps(schema, observation)?;
        }
        let feature = schema
            .classes
            .get(self.selected.owner())
            .and_then(|class| class.features.get(self.selected.name()))
            .ok_or("Missing effective feature declaration")?;
        let identity = if fully_observed {
            // The carrier's Owned variant determines its fixed slot size;
            // alias32 is separately admitted and is not a holder-size claim.
            observation.retain_storage(crate::logical_size!(StoredFeatureProjection<'_>, 136))?;
            self.selected.identity().clone_observed(observation)?
        } else {
            // Public/default projection retains its independently owned identity
            // and original serialized-copy admission, even for internal inputs.
            if observation.has_observer() {
                intersect_model::definitions::retained_storage::reserve_serialized(
                    &**self.selected.identity(),
                    observation,
                )?;
            }
            FeatureIdentityInput::Owned((**self.selected.identity()).clone())
        };
        Ok(StoredFeatureProjection {
            store,
            object,
            feature,
            identity,
        })
    }
}

/// A prepared stored-feature read. Construction checks schema revision, receiver
/// conformance and exact effective declaration identity. It is not a certificate
/// that an arbitrary raw store's values or complete graph have been validated.
/// Normal callers supply an admitted model/snapshot and validate transported
/// values through their existing domain codecs. Derived providers are separate.
pub struct StoredFeatureProjection<'a> {
    store: &'a dyn ModelRead,
    object: &'a str,
    feature: &'a Feature,
    identity: FeatureIdentityInput,
}
impl<'a> StoredFeatureProjection<'a> {
    pub fn new(
        store: &'a dyn ModelRead,
        schema: &'a intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        receiver: &str,
        member: &MemberIdentity,
        object: &'a str,
    ) -> StorageResult<Self> {
        let mut pending = StoredFeaturePreparation::new(receiver, member, object);
        while !pending.advance(store, schema)? {}
        pending.finish()?.projection(store, schema, object)
    }
    pub fn definition(&self) -> &Feature {
        self.feature
    }
    pub fn identity(&self) -> &MemberIdentity {
        &self.identity
    }
    pub fn singular(&self) -> StorageResult<Value> {
        self.singular_observed(&InvocationContext::default())
    }
    pub fn singular_observed(&self, observation: &InvocationContext<'_>) -> StorageResult<Value> {
        observation.checkpoint()?;
        if self.feature.upper != 1 {
            return Err("Many feature requires a collection projection".into());
        }
        if let Some(value) =
            self.store
                .value_observed(self.object, &self.feature.name, observation)?
        {
            return Ok(value);
        }
        if !self.store.inspect_class(self.object, &mut |_| Ok(()))? {
            return Err("Stored feature object no longer exists".into());
        }
        // Use the same cooked schema default as Element construction. Optional
        // absence is its null default; provider/default interpretation stays in
        // definitions rather than being duplicated in the projection.
        if observation.has_observer() {
            intersect_model::definitions::retained_storage::reserve_serialized(
                &self.feature.default,
                observation,
            )?;
        }
        Ok(self.feature.default.clone())
    }
    // Same qualified projection, with no intermediate owned JSON. The raw
    // adapter's compatibility default still materializes at its old boundary.
    fn inspect_observed(
        &self,
        observation: &InvocationContext<'_>,
        consume: &mut dyn FnMut(super::ValueView<'_>) -> StorageResult<()>,
    ) -> StorageResult<()> {
        observation.checkpoint()?;
        let many = self.feature.upper != 1;
        if self.store.inspect_stored_value_observed(
            self.object,
            &self.feature.name,
            many,
            observation,
            consume,
        )? {
            return Ok(());
        }
        if many {
            return Err("Missing collection feature".into());
        }
        if !self.store.inspect_class(self.object, &mut |_| Ok(()))? {
            return Err("Stored feature object no longer exists".into());
        }
        consume(super::ValueView::json(&self.feature.default))
    }
    /// Delegate a page to the adapter; canonical memory, versioned views and
    /// SQLite avoid materializing the remaining collection. Other adapters must
    /// establish their own bounded projection capability (the ModelRead default
    /// may materialize a value). Offsets beyond the end return empty; missing or
    /// malformed collection storage remains an error. Payload size and domain
    /// validation belong to the consuming codec.
    pub fn collection_page(&self, offset: usize, limit: usize) -> StorageResult<Vec<Value>> {
        self.collection_page_observed(offset, limit, &InvocationContext::default())
    }
    pub fn collection_page_observed(
        &self,
        offset: usize,
        limit: usize,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Vec<Value>> {
        observation.checkpoint()?;
        if self.feature.upper == 1 {
            return Err("Singular feature requires a scalar projection".into());
        }
        if limit == 0 {
            return Err("Feature page limit must be positive".into());
        }
        self.store.collection_page_observed(
            self.object,
            &self.feature.name,
            offset,
            limit,
            observation,
        )
    }
}

/// Internal exact semantic selection reuses stored qualification but cannot be
/// converted into public stored-read authority. Old holders retain every field.
pub(crate) struct SemanticFeaturePreparation(StoredFeaturePreparation);
pub(crate) struct SemanticFeatureSelection(StoredFeatureSelection);
impl SemanticFeaturePreparation {
    pub(crate) fn new(receiver: &str, member: &MemberIdentity, object: &str) -> Self {
        Self(StoredFeaturePreparation::new(receiver, member, object))
    }
    pub(crate) fn advance(
        &mut self,
        store: &dyn ModelRead,
        schema: &intersect_model::definitions::schema::Schema,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<bool> {
        self.0
            .advance_mode::<false>(store, schema, observation, false, false, None)
    }
    // This call-local metadata comes only from the complete preflight of the
    // same semantic read; it carries no source-row or graph authority.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn advance_preflighted_exact(
        &mut self,
        store: &dyn ModelRead,
        schema: &intersect_model::definitions::schema::Schema,
        observation: &InvocationContext<'_>,
        owner: &str,
        feature: &Feature,
        position: usize,
    ) -> StorageResult<bool> {
        self.0.advance_mode::<true>(
            store,
            schema,
            observation,
            false,
            false,
            Some((owner, feature, position)),
        )
    }
    pub(crate) fn finish(self) -> StorageResult<SemanticFeatureSelection> {
        Ok(SemanticFeatureSelection(self.0.finish()?))
    }
}
impl SemanticFeatureSelection {
    pub(crate) fn actual(&self) -> &str {
        &self.0.actual
    }
    pub(crate) fn member(&self) -> &MemberIdentity {
        self.0.selected.identity()
    }
    pub(crate) fn definition<'a>(
        &self,
        schema: &'a intersect_model::definitions::schema::Schema,
    ) -> StorageResult<&'a Feature> {
        if schema.revision != self.0.revision {
            return Err("Semantic selection schema mismatch".into());
        }
        schema
            .classes
            .get(self.0.selected.owner())
            .and_then(|class| class.features.get(self.0.selected.name()))
            .ok_or_else(|| "Missing semantic feature declaration".into())
    }
    pub(crate) fn definition_fully_observed<'a>(
        &self,
        schema: &'a intersect_model::definitions::schema::Schema,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<&'a Feature> {
        observation.work(
            1 + schema
                .revision
                .len()
                .max(self.0.revision.len())
                .div_ceil(64),
        )?;
        self.0.admit_definition_maps(schema, observation)?;
        self.definition(schema)
    }
    pub(crate) fn stored(
        &self,
        store: &dyn ModelRead,
        schema: &intersect_model::definitions::schema::Schema,
        object: &str,
        observation: &InvocationContext<'_>,
        fully_observed: bool,
    ) -> StorageResult<Value> {
        let projection =
            self.0
                .projection_mode(store, schema, object, observation, fully_observed)?;
        if projection.definition().derived {
            return Err("Derived metadata cannot select Stored reads".into());
        }
        if projection.definition().upper == 1 {
            projection.singular_observed(observation)
        } else {
            Ok(Value::Array(projection.collection_page_observed(
                0,
                usize::MAX,
                observation,
            )?))
        }
    }
    pub(crate) fn inspect_stored(
        &self,
        store: &dyn ModelRead,
        schema: &intersect_model::definitions::schema::Schema,
        object: &str,
        observation: &InvocationContext<'_>,
        fully_observed: bool,
        consume: &mut dyn FnMut(super::ValueView<'_>) -> StorageResult<()>,
    ) -> StorageResult<()> {
        let projection =
            self.0
                .projection_mode(store, schema, object, observation, fully_observed)?;
        if projection.definition().derived {
            return Err("Derived metadata cannot select Stored reads".into());
        }
        projection.inspect_observed(observation, consume)
    }
    /// One bounded occurrence through the canonical qualified projection. This
    /// never rematerializes the complete source collection for each ordinal.
    pub(crate) fn stored_occurrence(
        &self,
        store: &dyn ModelRead,
        schema: &intersect_model::definitions::schema::Schema,
        object: &str,
        occurrence: usize,
        observation: &InvocationContext<'_>,
        fully_observed: bool,
    ) -> StorageResult<Option<Value>> {
        let projection =
            self.0
                .projection_mode(store, schema, object, observation, fully_observed)?;
        if projection.definition().derived || projection.definition().upper == 1 {
            return Err("Stored occurrence requires a genuine collection declaration".into());
        }
        Ok(projection
            .collection_page_observed(occurrence, 1, observation)?
            .pop())
    }
}
