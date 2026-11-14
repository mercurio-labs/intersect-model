//! Handwritten adapters for the ONE canonical draft-contract binder.
use super::*;
use crate::definitions::{
    bundle::BundleValidation,
    retained_storage::reserve_serialized,
    schema::{
        FeatureInputCache, FeatureMapPreparation, PreparedFeatureInputs, SharedFeatureInputCache,
    },
};

// Existing String error exits are outside the semantic-work ledger. Bound
// this diagnostic-only allocation; never call an exhausted observer to format it.
const BIND_BUDGET_ERROR: &str = "Query work budget exhausted; no complete result";
const BIND_BUDGET_CONTEXT_PREFIX: &str =
    "Query work budget exhausted; no complete result [draft-bind ";
pub(super) fn is_budget_failure(error: &str) -> bool {
    error == BIND_BUDGET_ERROR || error.starts_with(BIND_BUDGET_CONTEXT_PREFIX)
}
pub(super) fn budget_failure_context(
    mut error: String,
    receiver: &str,
    member: &str,
    stage: &str,
    input_kind: &str,
) -> String {
    if error != BIND_BUDGET_ERROR {
        return error;
    }
    fn label(value: &str) -> &str {
        if value.is_empty() {
            return "-";
        }
        let mut end = value.len().min(64);
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        &value[..end]
    }
    // Four64-byte labels plus fixed punctuation fit384 extra UTF8 bytes.
    // Allocation failure preserves the original causal error unchanged.
    if error.try_reserve_exact(384).is_err() {
        return error;
    }
    error.push_str(" [draft-bind stage=");
    error.push_str(label(stage));
    error.push_str(" receiver=");
    error.push_str(label(receiver));
    error.push_str(" member=");
    error.push_str(label(member));
    error.push_str(" inputs=");
    error.push_str(label(input_kind));
    error.push(']');
    error
}

// Private source-only policy for the same completed immutable metadata/cache.
// The canonical worker still validates receiver names/identities/defaults; only
// already completed owner-local inventories and ascending insertion are reused.
// Only physical input maps omit derived payloads after complete selection.
pub(super) struct FrozenSourceInputs<'call, 'definition> {
    metadata: &'call crate::definitions::preparation::PreparedPhysicalMetadata<'definition>,
    cache: SharedFeatureInputCache,
}
impl<'call, 'definition> FrozenSourceInputs<'call, 'definition> {
    pub(super) fn new(
        metadata: &'call crate::definitions::preparation::PreparedPhysicalMetadata<'definition>,
        schema: &Schema,
        cache: &SharedFeatureInputCache,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Option<Self>> {
        let Some(actual) = metadata.frozen_feature_input_cache(schema, observation)? else {
            return Ok(None);
        };
        observation.work(2)?;
        if !metadata.matches_schema(schema) || !std::sync::Arc::ptr_eq(&actual, cache) {
            return Err("Frozen owner inventory schema/cache association changed".into());
        }
        observation.checkpoint()?;
        Ok(Some(Self {
            metadata,
            cache: actual,
        }))
    }
}
impl crate::definitions::schema::DefaultSourceInputs for FrozenSourceInputs<'_, '_> {
    const ASCENDING_IDENTITY_APPEND: bool = true;
    const REUSE_OWNER_INVENTORY: bool = true;
    const PHYSICAL_INPUTS_ONLY: bool = true;
    fn owner_inventory_complete<A: crate::definitions::schema::RuntimeAlternative>(
        &self,
        schema: &crate::definitions::schema::SchemaRepresentation<A>,
        cache: &SharedFeatureInputCache,
        owner: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, crate::definitions::schema::PreparationError> {
        use crate::definitions::schema::PreparationError;
        observation
            .checkpoint()
            .map_err(PreparationError::Observation)?;
        observation.work(2).map_err(PreparationError::Observation)?;
        if !self
            .metadata
            .matches_schema(self.metadata.definition().schema())
            || !std::ptr::eq(
                self.metadata.definition().schema() as *const _ as *const (),
                schema as *const _ as *const (),
            )
            || !std::sync::Arc::ptr_eq(&self.cache, cache)
        {
            return Err("Frozen owner inventory schema/cache association changed".into());
        }
        let complete =
            FeatureInputCache::owner_inventory_complete_observed(cache, owner, observation)?;
        observation
            .checkpoint()
            .map_err(PreparationError::Observation)?;
        Ok(complete)
    }
}

pub(super) enum DraftBindWork<'work, 'caller> {
    Legacy(&'work mut QueryBudget),
    Observed(&'work InvocationContext<'caller>),
}
pub(super) enum BindFields {
    Legacy(BTreeMap<String, crate::definitions::schema::Feature>),
    Observed(PreparedFeatureInputs),
    Retained(std::sync::Arc<PreparedFeatureInputs>),
}
impl BindFields {
    fn observed_inputs(&self) -> StorageResult<&PreparedFeatureInputs> {
        match self {
            Self::Observed(fields) => Ok(fields),
            Self::Retained(fields) => Ok(fields.as_ref()),
            Self::Legacy(_) => Err("Legacy fields have no observed input proof".into()),
        }
    }
    pub(super) fn input_kind(&self) -> &'static str {
        match self {
            Self::Legacy(_) => "Legacy",
            Self::Observed(_) => "Observed",
            Self::Retained(_) => "Retained",
        }
    }
    pub(super) fn coverage(&self, work: &DraftBindWork<'_, '_>) -> StorageResult<BTreeSet<String>> {
        if let Self::Legacy(fields) = self {
            return work.coverage(fields);
        }
        let fields = self.observed_inputs()?;
        let DraftBindWork::Observed(observation) = work else {
            return Err("Shared frozen fields require their observed binder".into());
        };
        let mut result = BTreeSet::new();
        fields.visit_fields_observed(observation, |name, field| {
            observation.work(1).map_err(|e| {
                budget_failure_context(
                    e,
                    fields.receiver(),
                    name,
                    "coverage_visit",
                    self.input_kind(),
                )
            })?;
            if !field.derived {
                fields
                    .admit_coverage_insert_observed(name, observation)
                    .map_err(|e| {
                        budget_failure_context(
                            e,
                            fields.receiver(),
                            name,
                            "coverage_insert",
                            self.input_kind(),
                        )
                    })?;
                work.key(name).map_err(|e| {
                    budget_failure_context(
                        e,
                        fields.receiver(),
                        name,
                        "coverage_name",
                        self.input_kind(),
                    )
                })?;
                result.insert(name.to_owned());
            }
            Ok(())
        })?;
        Ok(result)
    }
    #[cfg(test)]
    pub(super) fn member<'a>(
        &'a self,
        schema: &'a Schema,
        class: &'a str,
        member: &DraftMemberDisposition,
        work: &DraftBindWork<'_, '_>,
    ) -> StorageResult<(&'a crate::definitions::schema::Feature, bool)> {
        self.member_among(schema, class, member, work, &mut None)
    }
    /// `member`, for the members of one contract: `effective` keeps the
    /// class's effective features between calls, so a legacy bind computes
    /// them once per contract instead of once per member. It must start as
    /// `None` and be used for this `schema` and `class` only.
    pub(super) fn member_among<'a>(
        &'a self,
        schema: &'a Schema,
        class: &'a str,
        member: &DraftMemberDisposition,
        work: &DraftBindWork<'_, '_>,
        effective: &mut Option<Vec<crate::definitions::schema::EffectiveFeature<'a>>>,
    ) -> StorageResult<(&'a crate::definitions::schema::Feature, bool)> {
        if let Self::Legacy(_) = self {
            let members = match effective {
                Some(members) => members,
                None => effective.insert(schema.effective_features(class)?),
            };
            // The first match by name, as `Schema::effective_feature` selects.
            let selected = members
                .iter()
                .find(|candidate| candidate.feature.name == member.name)
                .ok_or_else(|| format!("Unknown feature {class}.{}", member.name))?;
            return Ok((selected.feature, selected.identity == member.identity));
        }
        let fields = self.observed_inputs()?;
        let DraftBindWork::Observed(observation) = work else {
            return Err("Shared frozen fields require their observed binder".into());
        };
        let (field, identity) = fields
            .member_row_observed(&member.name, observation)?
            .ok_or_else(|| {
                format!(
                    "Unsupported unknown or derived physical member {class}.{}",
                    member.name
                )
            })?;
        // This variant is produced ONLY by the source-qualified physical
        // metadata binder. Complete canonical map DATA includes derived rows,
        // but a retained data hit cannot grant a physical derived disposition.
        // Legacy/dynamic and generic metadata consumers keep their old policy.
        if matches!(self, Self::Retained(_)) && field.derived {
            return Err(format!(
                "Unsupported unknown or derived physical member {class}.{}",
                member.name
            ));
        }
        Ok((field, identity == Some(&member.identity)))
    }
}
impl DraftBindWork<'_, '_> {
    pub(super) fn charge(&mut self) -> StorageResult<()> {
        match self {
            Self::Legacy(budget) => budget.charge(),
            Self::Observed(observation) => observation.work(1),
        }
    }
    pub(super) fn storage(&self, bytes: usize) -> StorageResult<()> {
        if let Self::Observed(observation) = self {
            observation.retain_storage(bytes)?;
            observation.checkpoint()?;
        }
        Ok(())
    }
    pub(super) fn retain<T: ?Sized + Serialize>(&self, value: &T) -> StorageResult<()> {
        if let Self::Observed(observation) = self {
            reserve_serialized(value, observation)?;
            observation.checkpoint()?;
        }
        Ok(())
    }
    pub(super) fn key(&self, value: &str) -> StorageResult<()> {
        self.storage(128)?;
        self.retain(value)
    }
    pub(super) fn validate_sources(
        &mut self,
        schema: &Schema,
        sources: &DefinitionBundle,
        completed: Option<&crate::definitions::preparation::CompletedCompiledSources<'_>>,
    ) -> StorageResult<()> {
        if let (Self::Observed(observation), Some(completed)) = (&*self, completed) {
            // Paid borrow/association check against the exact immutable compiler
            // owner. Reuse only complete canonical SOURCE validation: provider
            // contracts, identities, inventories and final drift still run below.
            observation.work(3)?;
            if !completed.belongs_to(schema, sources) {
                return Err("Frozen compiled source/schema owner association changed".into());
            }
            observation.checkpoint()?;
            return Ok(());
        }
        let observation = match self {
            Self::Legacy(_) => return sources.validate(),
            Self::Observed(observation) => *observation,
        };
        // Borrowed validation indexes are admitted before their first advance.
        // Payload hashing remains the canonical 256-byte source continuation.
        let mut entries = sources.namespaces.len();
        for source in &sources.sources {
            observation.work(1)?;
            entries = entries
                .checked_add(1)
                .and_then(|n| n.checked_add(source.dependencies.len()))
                .ok_or("Frozen source validation storage overflow")?;
        }
        let bytes = entries
            .checked_mul(128)
            .and_then(|n| n.checked_add(512))
            .ok_or("Frozen source validation storage overflow")?;
        self.storage(bytes)?;
        let mut pending = BundleValidation::new(sources);
        loop {
            observation.work(1)?;
            if pending.advance(&|_| Ok(()))? {
                return Ok(());
            }
        }
    }
    pub(super) fn source_ids(&self, sources: &DefinitionBundle) -> StorageResult<Vec<SourceId>> {
        let Self::Observed(observation) = self else {
            return Ok(sources
                .sources
                .iter()
                .map(|source| source.id.clone())
                .collect());
        };
        let slots = sources
            .sources
            .len()
            .checked_mul(crate::logical_size!(SourceId, 48))
            .and_then(|n| n.checked_add(128))
            .ok_or("Frozen source vector storage overflow")?;
        self.storage(slots)?;
        let mut ids = Vec::with_capacity(sources.sources.len());
        for source in &sources.sources {
            observation.work(1)?;
            self.retain(&source.id)?;
            ids.push(source.id.clone());
        }
        Ok(ids)
    }
    pub(super) fn provider_contracts<'p>(
        &mut self,
        provider: &'p dyn DraftPhysicalDefaults,
    ) -> StorageResult<&'p [DraftClassContract]> {
        if matches!(self, Self::Observed(_)) {
            self.charge()?;
        }
        Ok(provider.contracts())
    }
    pub(super) fn identity(&self, schema: &Schema, class: &str) -> StorageResult<()> {
        if matches!(self, Self::Observed(_)) {
            // Admit the canonical identity formatter's owned atoms/temporary
            // path strings before schema.identity allocates them.
            if let Some(instantiation) = schema.instantiations.get(class) {
                self.retain(instantiation.identity())?;
            } else if let Some(declaration) = schema.declarations.get(class) {
                self.retain(declaration)?;
            } else {
                self.retain(&(schema.uri.as_str(), class))?;
            }
        }
        Ok(())
    }
    pub(super) fn fields(
        &mut self,
        schema: &Schema,
        class: &str,
        input_cache: &mut Option<SharedFeatureInputCache>,
        metadata: Option<&crate::definitions::preparation::PreparedPhysicalMetadata<'_>>,
    ) -> StorageResult<BindFields> {
        match self {
            Self::Legacy(_) => Ok(BindFields::Legacy(schema.features(class)?)),
            Self::Observed(observation) => {
                if let Some(metadata) = metadata {
                    if let Some(fields) =
                        metadata.frozen_feature_inputs(schema, class, observation)?
                    {
                        return Ok(BindFields::Retained(fields));
                    }
                    if input_cache.is_none() {
                        *input_cache = metadata.frozen_feature_input_cache(schema, observation)?;
                    }
                }
                if input_cache.is_none() {
                    const CACHE: usize = 256;
                    const {
                        assert!(
                            std::mem::size_of::<FeatureInputCache>()
                                + 2 * std::mem::size_of::<usize>()
                                + std::mem::size_of::<std::sync::Mutex<()>>()
                                <= CACHE
                        );
                    }
                    observation.work(2)?;
                    observation.retain_storage(CACHE)?;
                    observation.checkpoint()?;
                    *input_cache = Some(std::sync::Arc::new(std::sync::Mutex::new(
                        FeatureInputCache::default(),
                    )));
                }
                observation.work(1)?;
                let cache = input_cache
                    .as_ref()
                    .ok_or("Missing frozen source inputs")?
                    .clone();
                let sources = match metadata {
                    Some(metadata) => {
                        FrozenSourceInputs::new(metadata, schema, &cache, observation)?
                    }
                    None => None,
                };
                let mut pending =
                    FeatureMapPreparation::with_source_inputs_observed(class, cache, observation)
                        .map_err(|error| error.to_string())?;
                loop {
                    observation.work(1)?;
                    let complete = match &sources {
                        Some(sources) => {
                            pending.advance_with_default_sources(schema, observation, sources)
                        }
                        None => pending.advance_observed(schema, observation),
                    }
                    .map_err(|error| error.to_string())?;
                    if complete {
                        break;
                    }
                }
                Ok(BindFields::Observed(pending.finish_inputs()?))
            }
        }
    }
    pub(super) fn coverage(
        &self,
        fields: &BTreeMap<String, crate::definitions::schema::Feature>,
    ) -> StorageResult<BTreeSet<String>> {
        if matches!(self, Self::Legacy(_)) {
            return Ok(fields
                .iter()
                .filter(|(_, field)| !field.derived)
                .map(|(name, _)| name.clone())
                .collect());
        }
        let mut result = BTreeSet::new();
        for (name, field) in fields {
            if let Self::Observed(observation) = self {
                observation.work(1)?;
            }
            if !field.derived {
                self.key(name)?;
                result.insert(name.clone());
            }
        }
        Ok(result)
    }
    pub(super) fn final_contracts(
        &mut self,
        provider: &dyn DraftPhysicalDefaults,
        contracts: &BTreeMap<String, Cow<'_, DraftClassContract>>,
    ) -> StorageResult<()> {
        match self {
            Self::Legacy(_) => {
                if provider
                    .contracts()
                    .iter()
                    .ne(contracts.values().map(Cow::as_ref))
                {
                    return Err(
                        "Draft physical capability changed or has noncanonical class order".into(),
                    );
                }
                Ok(())
            }
            Self::Observed(observation) => {
                observation.work(1)?;
                let live = provider.contracts();
                compare_draft_contracts(contracts, live, &mut || observation.work(1))
            }
        }
    }
}
