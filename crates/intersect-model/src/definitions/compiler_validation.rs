//! Retained structural validation over the compiler's immutable lowered schema.
use super::*;
use std::ops::Bound::{Excluded, Unbounded};

#[derive(Clone, Copy)]
enum Stage {
    Class,
    Source,
    Operations,
    FeaturesBegin,
    Features,
    Field,
    Reference,
    OppositeBegin,
    OppositeFeatures,
    Opposite,
    TargetConformance,
    OwnerConformanceBegin,
    OwnerConformance,
    Containment,
    Done,
}

pub(super) struct LoweredSchemaValidation<'schema> {
    stage: Stage,
    projected_operations: bool,
    retained: bool,
    cache: Option<SharedFeatureInputCache>,
    class: &'schema str,
    have_class: bool,
    field: &'schema str,
    have_field: bool,
    node_index: Option<usize>,
    source_search: usize,
    source_remaining: usize,
    feature_preparation: Option<FeatureMapPreparation>,
    fields: Option<PreparedFeatureInputs>,
    opposite_fields: Option<PreparedFeatureInputs>,
    reference: Option<ReferenceStructurePreparation>,
    conformance: Option<ConformancePreparation>,
    failed: bool,
    completed_inputs: Option<CompletedLoweredInputs>,
    capture_source_members: bool,
    capture_member_count: usize,
}
// Pure completed field DATA from the ordinary compiler's final-declaration
// validation. This is not a final-schema validation or semantic-provider permit.
// Only the private compiler pipeline can create it; publication borrows it from
// the same stable compiled model or text owner as the final schema.
pub(crate) struct CompletedLoweredInputs {
    fields: BTreeMap<String, std::sync::Arc<PreparedFeatureInputs>>,
    cache: Option<SharedFeatureInputCache>,
}
const _: () = assert!(std::mem::size_of::<CompletedLoweredInputs>() <= 256);
impl CompletedLoweredInputs {
    pub(crate) fn fields_observed(
        &self,
        receiver: &str,
        observation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Option<std::sync::Arc<PreparedFeatureInputs>>, String> {
        let height = (usize::BITS - self.fields.len().leading_zeros()) as usize;
        let units = height
            .checked_mul(12)
            .and_then(|n| n.checked_mul(1 + receiver.len().div_ceil(64)))
            .and_then(|n| n.checked_add(2))
            .ok_or("Completed compiler input lookup work overflow")?;
        observation.work(units)?;
        observation.retain_storage(32)?;
        observation.checkpoint()?;
        Ok(self.fields.get(receiver).cloned())
    }
    pub(crate) fn cache_observed(
        &self,
        observation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Option<SharedFeatureInputCache>, String> {
        observation.work(2)?;
        if self.cache.is_some() {
            observation.retain_storage(32)?;
        }
        observation.checkpoint()?;
        Ok(self.cache.clone())
    }
}

// The owning wrapper pins this exact immutable schema, so cursors borrow its
// canonical keys. Synchronous callers keep stack-local state and their original
// receipt scope; only retained publication admits a holder allocation.
self_cell::self_cell!(
    struct FrozenValidation {
        owner: Schema,
        #[covariant]
        dependent: LoweredSchemaValidation,
    }
);
pub(super) struct FrozenLoweredSchema {
    holder: FrozenValidation,
}
const VALIDATION_BYTES: usize = 6144;
const _: () = assert!(
    std::mem::size_of::<Schema>() + std::mem::size_of::<LoweredSchemaValidation<'static>>() + 128
        <= VALIDATION_BYTES
);
impl FrozenLoweredSchema {
    pub(super) fn begin(
        schema: Schema,
        root: &Xml,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        observation.reserve(VALIDATION_BYTES, &root.opening.source)?;
        let holder = FrozenValidation::try_new(schema, |_| {
            LoweredSchemaValidation::new_retained(root, observation)
        })?;
        Ok(Self { holder })
    }
    pub(super) fn advance(
        &mut self,
        root: &Xml,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        self.holder
            .with_dependent_mut(|schema, pending| pending.advance(root, schema, observation))
    }
    pub(super) fn finish(self, root: &Xml) -> Result<Schema, ImportError> {
        if !self
            .holder
            .with_dependent(|_, pending| matches!(pending.stage, Stage::Done) && !pending.failed)
        {
            return Err(root.error("Lowered schema validation is incomplete or failed"));
        }
        Ok(self.holder.into_owner())
    }
}
impl<'schema> LoweredSchemaValidation<'schema> {
    pub(super) fn new_with_completed_inputs(
        root: &Xml,
        projected_operations: bool,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        // A private opt-in over the SAME validation transitions. Existing public
        // imports and materialized owners keep their original constructors.
        observation.reserve(256, &root.opening.source)?;
        let mut state = Self::new(root, projected_operations, observation)?;
        if state.cache.is_none() {
            observation.reserve(128 + 32, &root.opening.source)?;
            state.cache = Some(std::sync::Arc::new(std::sync::Mutex::new(
                FeatureInputCache::default(),
            )));
        }
        state.completed_inputs = Some(CompletedLoweredInputs {
            fields: BTreeMap::new(),
            cache: None,
        });
        Ok(state)
    }
    pub(super) fn finish_completed_inputs(
        mut self,
        root: &Xml,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<CompletedLoweredInputs, ImportError> {
        if self.failed || !matches!(self.stage, Stage::Done) {
            return Err(root.error("Compiler field input capture is incomplete or failed"));
        }
        observation.work(2, &root.opening.source)?;
        observation.reserve(32, &root.opening.source)?;
        let mut inputs = self
            .completed_inputs
            .take()
            .ok_or_else(|| root.error("Compiler field input capture was not selected"))?;
        inputs.cache = self.cache.take();
        Ok(inputs)
    }
    pub(super) fn new(
        root: &Xml,
        projected_operations: bool,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        Self::with_policy(root, projected_operations, false, observation)
    }
    fn new_retained(
        root: &Xml,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        Self::with_policy(root, false, true, observation)
    }
    fn with_policy(
        root: &Xml,
        projected_operations: bool,
        retained: bool,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        let cache = if projected_operations {
            None
        } else {
            observation.reserve(128 + 32, &root.opening.source)?;
            Some(std::sync::Arc::new(std::sync::Mutex::new(
                FeatureInputCache::default(),
            )))
        };
        Ok(Self {
            stage: Stage::Class,
            projected_operations,
            retained,
            cache,
            class: "",
            have_class: false,
            field: "",
            have_field: false,
            node_index: None,
            source_search: 0,
            source_remaining: 0,
            feature_preparation: None,
            fields: None,
            opposite_fields: None,
            reference: None,
            conformance: None,
            failed: false,
            completed_inputs: None,
            capture_source_members: false,
            capture_member_count: 0,
        })
    }

    fn node<'a>(&self, root: &'a Xml) -> &'a Xml {
        self.node_index
            .and_then(|index| root.children.get(index))
            .unwrap_or(root)
    }

    fn fields(&self) -> Result<&PreparedFeatureInputs, String> {
        self.fields
            .as_ref()
            .ok_or_else(|| "Missing compiler fields".into())
    }

    fn field(&self) -> Result<&Feature, String> {
        self.fields()?
            .get(self.field)
            .ok_or_else(|| "Missing compiler field".into())
    }

    fn opposite(&self) -> Result<(&str, &str), String> {
        self.field()?
            .opposite
            .as_deref()
            .and_then(|value| value.split_once('/'))
            .ok_or_else(|| "Invalid opposite identity".into())
    }

    fn inconsistent(&self) -> String {
        format!("Inconsistent opposite {}.{}", self.class, self.field)
    }

    fn begin_features(
        &self,
        receiver: &str,
        source: &crate::definitions::bundle::SourceId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<FeatureMapPreparation, ImportError> {
        observation.preparation(source, |context| {
            if self.cache.is_none() {
                FeatureMapPreparation::new_observed(receiver, context)
            } else {
                FeatureMapPreparation::with_source_inputs_observed(
                    receiver,
                    self.cache
                        .as_ref()
                        .ok_or("Missing compiler feature input cache")?
                        .clone(),
                    context,
                )
            }
        })
    }

    fn advance_features(
        &mut self,
        schema: &Schema,
        source: &crate::definitions::bundle::SourceId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Option<PreparedFeatureInputs>, ImportError> {
        let pending = self
            .feature_preparation
            .as_mut()
            .ok_or_else(|| Self::node_error(source, "Missing compiler feature preparation"))?;
        if !observation.preparation(source, |context| {
            context.work(1).map_err(PreparationError::Observation)?;
            pending.advance_observed(schema, context)
        })? {
            return Ok(None);
        }
        let inputs = self
            .feature_preparation
            .take()
            .ok_or_else(|| Self::node_error(source, "Missing complete compiler features"))?
            .finish_inputs()
            .map_err(|message| Self::node_error(source, &message))?;
        if self.projected_operations && self.completed_inputs.is_none() {
            observation
                .preparation(source, |context| {
                    inputs
                        .into_owned_observed(context)
                        .map(PreparedFeatureInputs::Owned)
                })
                .map(Some)
        } else {
            Ok(Some(inputs))
        }
    }

    fn node_error(source: &crate::definitions::bundle::SourceId, message: &str) -> ImportError {
        ImportError::new(
            ErrorKind::InvalidDefinition,
            &crate::definitions::bundle::SourceSpan {
                source: source.clone(),
                start: 0,
                end: 0,
            },
            message,
        )
    }

    fn advance_conformance(
        &mut self,
        schema: &Schema,
        source: &crate::definitions::bundle::SourceId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Option<bool>, ImportError> {
        let pending = self.conformance.as_mut().ok_or_else(|| {
            ImportError::new(
                ErrorKind::InvalidDefinition,
                &crate::definitions::bundle::SourceSpan {
                    source: source.clone(),
                    start: 0,
                    end: 0,
                },
                "Missing compiler conformance",
            )
        })?;
        // Match Schema::conforms_observed: schema errors mean nonconformance;
        // observation failures remain typed failures, never a false predicate.
        let result = observation.preparation(source, |context| {
            context.work(1).map_err(PreparationError::Observation)?;
            match pending.advance_observed(schema, context) {
                Ok(done) => Ok(if done { Some(true) } else { None }),
                Err(PreparationError::Schema(_)) => Ok(Some(false)),
                Err(error @ PreparationError::Observation(_)) => Err(error),
            }
        })?;
        match result {
            None => Ok(None),
            Some(false) => {
                self.conformance = None;
                Ok(Some(false))
            }
            Some(true) => {
                Ok(Some(self.conformance.take().is_some_and(|pending| {
                    matches!(pending.finish(), Ok(true))
                })))
            }
        }
    }

    pub(super) fn advance(
        &mut self,
        root: &Xml,
        schema: &'schema Schema,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        if self.failed {
            return Err(self
                .node(root)
                .error("Lowered schema validation has failed"));
        }
        let result = self.advance_inner(root, schema, observation);
        if result.is_err() {
            self.failed = true;
        }
        // Original validation diagnoses all per-class failures at its source
        // classifier. ParserObservation separately retains observer categories.
        result.map_err(|error| self.node(root).error(error.message))
    }

    fn advance_inner(
        &mut self,
        root: &Xml,
        schema: &'schema Schema,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        observation.work(0, &root.opening.source)?;
        let node = self.node(root);
        let source = &node.opening.source;
        match self.stage {
            Stage::Class => {
                let next = if self.have_class {
                    schema
                        .classes
                        .range::<str, _>((Excluded(self.class), Unbounded))
                        .next()
                } else {
                    schema.classes.iter().next()
                };
                let Some((name, _)) = next else {
                    self.stage = Stage::Done;
                    return Ok(true);
                };
                observation.work(1, &root.opening.source)?;
                self.class = name;
                self.have_class = true;
                self.have_field = false;
                self.node_index = None;
                self.source_remaining = root.children.len();
                self.stage = Stage::Source;
            }
            Stage::Source => {
                if self.source_remaining == 0 {
                    self.stage = Stage::Operations;
                } else {
                    if self.retained {
                        observation.work(1, &root.opening.source)?;
                    }
                    if let Some(candidate) = root.children.get(self.source_search)
                        && candidate.attrs.get("name").map(String::as_str) == Some(self.class)
                    {
                        self.node_index = Some(self.source_search);
                        self.stage = Stage::Operations;
                    }
                    self.source_remaining -= 1;
                    self.source_search = (self.source_search + 1) % root.children.len();
                }
            }
            Stage::Operations => {
                // Public imports retain their projected-operation validation.
                // Its operation-group resolver is still a synchronous boundary.
                if self.projected_operations {
                    observation.preparation(source, |context| {
                        schema.resolve_native_operation_groups(self.class, context)
                    })?;
                }
                self.stage = Stage::FeaturesBegin;
            }
            Stage::FeaturesBegin => {
                self.capture_source_members = true;
                self.capture_member_count = 0;
                self.feature_preparation =
                    Some(self.begin_features(self.class, source, observation)?);
                self.stage = Stage::Features;
            }
            Stage::Features => {
                if let Some(fields) = self.advance_features(schema, source, observation)? {
                    self.fields = Some(fields);
                    self.stage = Stage::Field;
                }
            }
            Stage::Field => {
                let fields = self
                    .fields
                    .as_ref()
                    .ok_or_else(|| node.error("Missing compiler fields"))?;
                let Some((name, feature)) = fields.next_field(if self.have_field {
                    Some(self.field)
                } else {
                    None
                }) else {
                    // The entire canonical input cache is retained, including
                    // rows from source-ineligible / Programmatic / cap-miss maps.
                    // Prepay its eventual bookkeeping release for EVERY complete
                    // producer map before deciding whether to retain its map.
                    if self.completed_inputs.is_some() {
                        let cleanup = self
                            .capture_member_count
                            .checked_mul(16)
                            .and_then(|n| n.checked_add(4))
                            .ok_or_else(|| node.error("Compiler input cleanup work overflow"))?;
                        observation.work(cleanup, source)?;
                    }
                    if self.capture_source_members
                        && schema.declarations.contains_key(self.class)
                        && let Some(completed) = &mut self.completed_inputs
                        && completed.fields.len() < 4096
                    {
                        // Move the SAME completed source map. Only an actual
                        // retained map allocates its key/index/Arc below.
                        let height =
                            (usize::BITS - completed.fields.len().leading_zeros()) as usize;
                        let insertion = height
                            .checked_add(1)
                            .and_then(|n| n.checked_mul(12))
                            .and_then(|n| n.checked_mul(1 + self.class.len().div_ceil(64)))
                            .ok_or_else(|| node.error("Compiler input insertion work overflow"))?;
                        observation.work(insertion, source)?;
                        observation.reserve(128 + 128 + 32, source)?;
                        observation.serialized(self.class, source)?;
                        let fields = self
                            .fields
                            .take()
                            .ok_or_else(|| node.error("Missing completed compiler fields"))?;
                        completed
                            .fields
                            .insert(self.class.to_owned(), std::sync::Arc::new(fields));
                    }
                    self.fields = None;
                    self.stage = Stage::Class;
                    return Ok(false);
                };
                observation.work(1, source)?;
                if self.completed_inputs.is_some() {
                    observation.work(1, source)?;
                    self.capture_member_count = self
                        .capture_member_count
                        .checked_add(1)
                        .ok_or_else(|| node.error("Compiler input field count overflow"))?;
                    // Programmatic identities capture schema.revision, which
                    // grammar import may change. Never retain those maps.
                    self.capture_source_members &= matches!(
                        fields.member_identity(name),
                        Some(MemberIdentity::Source { .. })
                    );
                }
                if feature.reference && !schema.is_reference_target(&feature.target) {
                    return Err(node.error(format!("Unknown reference target {}", feature.target)));
                }
                self.reference = Some(observation.preparation(source, |context| {
                    if self.projected_operations {
                        ReferenceStructurePreparation::new_observed(self.class, feature, context)
                    } else {
                        ReferenceStructurePreparation::prepared_observed(self.class, name, context)
                    }
                })?);
                // Effective fields may own default projections; borrow the
                // cursor from their exact declaring class in the frozen schema.
                let owner = fields
                    .owner(name)
                    .ok_or_else(|| node.error("Missing effective feature owner"))?;
                self.field = schema
                    .classes
                    .get(owner)
                    .and_then(|class| class.features.get_key_value(name))
                    .map(|(name, _)| name.as_str())
                    .ok_or_else(|| node.error("Missing effective source feature name"))?;
                self.have_field = true;
                self.stage = Stage::Reference;
            }
            Stage::Reference => {
                let pending = self
                    .reference
                    .as_mut()
                    .ok_or_else(|| node.error("Missing compiler reference preparation"))?;
                let fields = self
                    .fields
                    .as_ref()
                    .ok_or_else(|| node.error("Missing compiler fields"))?;
                if observation.preparation(source, |context| {
                    context.work(1).map_err(PreparationError::Observation)?;
                    if self.projected_operations {
                        pending.advance_observed(schema, context)
                    } else {
                        pending.advance_prepared_observed(schema, fields, context)
                    }
                })? {
                    self.reference
                        .take()
                        .ok_or_else(|| node.error("Missing complete compiler reference"))?
                        .finish()
                        .map_err(|message| node.error(message))?;
                    self.stage = Stage::OppositeBegin;
                }
            }
            Stage::OppositeBegin => {
                if self
                    .field()
                    .map_err(|message| node.error(message))?
                    .opposite
                    .is_some()
                {
                    let (owner, _) = self.opposite().map_err(|message| node.error(message))?;
                    self.feature_preparation =
                        Some(self.begin_features(owner, source, observation)?);
                    self.stage = Stage::OppositeFeatures;
                } else {
                    self.stage = Stage::Field;
                }
            }
            Stage::OppositeFeatures => {
                if let Some(fields) = self.advance_features(schema, source, observation)? {
                    self.opposite_fields = Some(fields);
                    self.stage = Stage::Opposite;
                }
            }
            Stage::Opposite => {
                let (owner, other) = self.opposite().map_err(|message| node.error(message))?;
                let other = self
                    .opposite_fields
                    .as_ref()
                    .and_then(|fields| fields.get(other))
                    .ok_or_else(|| node.error("Unknown opposite"))?;
                let feature = self.field().map_err(|message| node.error(message))?;
                let declaration = self
                    .fields()
                    .map_err(|message| node.error(message))?
                    .owner(self.field)
                    .ok_or_else(|| node.error("Missing effective feature owner"))?;
                if !other.reference
                    || other.opposite.as_deref()
                        != Some(
                            observation
                                .formatted(&format_args!("{declaration}/{}", feature.name), source)?
                                .as_str(),
                        )
                {
                    return Err(node.error(self.inconsistent()));
                }
                self.conformance = Some(observation.preparation(source, |context| {
                    ConformancePreparation::new_observed(&feature.target, owner, context)
                })?);
                self.stage = Stage::TargetConformance;
            }
            Stage::TargetConformance | Stage::OwnerConformance => {
                if let Some(conforms) = self.advance_conformance(schema, source, observation)? {
                    if !conforms {
                        return Err(node.error(self.inconsistent()));
                    }
                    self.stage = if matches!(self.stage, Stage::TargetConformance) {
                        Stage::OwnerConformanceBegin
                    } else {
                        Stage::Containment
                    };
                }
            }
            Stage::OwnerConformanceBegin => {
                let (_, other) = self.opposite().map_err(|message| node.error(message))?;
                let other = self
                    .opposite_fields
                    .as_ref()
                    .and_then(|fields| fields.get(other))
                    .ok_or_else(|| node.error("Unknown opposite"))?;
                let declaration = self
                    .fields()
                    .map_err(|message| node.error(message))?
                    .owner(self.field)
                    .ok_or_else(|| node.error("Missing effective feature owner"))?;
                self.conformance = Some(observation.preparation(source, |context| {
                    ConformancePreparation::new_observed(declaration, &other.target, context)
                })?);
                self.stage = Stage::OwnerConformance;
            }
            Stage::Containment => {
                let (_, other) = self.opposite().map_err(|message| node.error(message))?;
                let other = self
                    .opposite_fields
                    .as_ref()
                    .and_then(|fields| fields.get(other))
                    .ok_or_else(|| node.error("Unknown opposite"))?;
                if self
                    .field()
                    .map_err(|message| node.error(message))?
                    .containment
                    && other.containment
                {
                    return Err(node.error(self.inconsistent()));
                }
                self.opposite_fields = None;
                self.stage = Stage::Field;
            }
            Stage::Done => return Ok(true),
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::conversion::{InvocationContext, InvocationObserver, ProviderId};
    use crate::definitions::ecore::ObservedImportError;
    use std::{cell::Cell, collections::BTreeSet, rc::Rc};

    const SOURCE: &str = "<ecore:EPackage xmlns:ecore='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers xsi:type='ecore:EClass' name='C' eSuperTypes='#//A'><eStructuralFeatures xsi:type='ecore:EAttribute' name='label' eType='ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString' defaultValueLiteral='café'/></eClassifiers><eClassifiers xsi:type='ecore:EClass' name='A'><eStructuralFeatures xsi:type='ecore:EReference' name='bs' eType='#//B' upperBound='-1' containment='true' eOpposite='#//B/a'/></eClassifiers><eClassifiers xsi:type='ecore:EClass' name='B'><eStructuralFeatures xsi:type='ecore:EReference' name='a' eType='#//A' eOpposite='#//A/bs'/></eClassifiers></ecore:EPackage>";

    fn input(move_fields: bool) -> (Xml, Schema) {
        let source = DefinitionSource::new(
            "memory:retained-validation.ecore",
            SourceKind::Ecore,
            SOURCE,
        );
        let document = Document::parse(source).unwrap();
        let mut root = Xml::from_node(&document.root, &document.source.text).unwrap();
        let operation = InvocationContext::default();
        let observation = crate::definitions::ecore::ParserObservation::new(&operation);
        let schema = lower_schema_output_with_datatypes(
            &mut root,
            "revision".into(),
            BTreeMap::new(),
            BTreeMap::new(),
            None,
            SchemaLoweringPolicy {
                validate_projected_operations: false,
                move_feature_declarations: move_fields,
                constructor_contracts: vec![],
                reference_top: None,
                attribute_domain_defaults: BTreeMap::new(),
            },
            &observation,
        )
        .unwrap();
        (root, schema)
    }

    #[derive(Default)]
    struct Meter {
        work: Cell<usize>,
        cancelled: Cell<bool>,
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            panic!("no providers in this schema")
        }
        fn checkpoint(&self) -> Result<(), String> {
            if self.cancelled.get() {
                Err("retained validation cancelled".into())
            } else {
                Ok(())
            }
        }
        fn work(&self, units: usize) -> Result<(), String> {
            self.checkpoint()?;
            self.work.set(self.work.get() + units);
            Ok(())
        }
        fn retain_storage(&self, _: usize) -> Result<(), String> {
            self.checkpoint()
        }
    }

    #[test]
    fn retained_validation_slices_real_features_references_and_opposites_without_mutation() {
        for move_fields in [false, true] {
            let (root, schema) = input(move_fields);
            let original = serde_json::to_vec(&schema).unwrap();
            let source = root.source_payload.clone();
            let meter = Rc::new(Meter::default());
            let weak = Rc::downgrade(&meter);
            let operation = InvocationContext::new(meter.clone());
            let observation = crate::definitions::ecore::ParserObservation::new(&operation);
            let mut pending = LoweredSchemaValidation::new_retained(&root, &observation).unwrap();
            let mut stages = BTreeSet::new();
            let mut advances = 0;
            loop {
                stages.insert(pending.stage as u8);
                let before = meter.work.get();
                let done = pending.advance(&root, &schema, &observation).unwrap();
                assert!(
                    meter.work.get() - before < 100,
                    "unexpected whole-graph validation step"
                );
                advances += 1;
                assert!(advances < 5000);
                if done {
                    break;
                }
            }
            for stage in [
                Stage::Features,
                Stage::Reference,
                Stage::OppositeFeatures,
                Stage::TargetConformance,
                Stage::OwnerConformance,
            ] {
                assert!(
                    stages.contains(&(stage as u8)),
                    "a real validation phase was skipped"
                );
            }
            assert_eq!(
                schema.classes["C"].features["label"].default,
                serde_json::json!("café")
            );
            assert_eq!(serde_json::to_vec(&schema).unwrap(), original);
            assert_eq!(root.source_payload, source);
            drop(observation);
            drop(operation);
            drop(meter);
            assert!(
                weak.upgrade().is_none(),
                "continuation retained its invocation observer"
            );
        }
    }

    #[test]
    fn owning_validation_holder_precharges_its_exact_record_and_preserves_schema_ownership() {
        struct Storage {
            used: Cell<usize>,
            cap: usize,
        }
        impl InvocationObserver for Storage {
            fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
                panic!("no provider calls")
            }
            fn retain_storage(&self, bytes: usize) -> Result<(), String> {
                let next = self.used.get() + bytes;
                if next > self.cap {
                    return Err("validation holder storage denied".into());
                }
                self.used.set(next);
                Ok(())
            }
        }
        let (root, schema) = input(true);
        for (cap, retained) in [(0, 0), (6143, 0), (6144, 6144), (6303, 6144)] {
            let meter = Rc::new(Storage {
                used: Cell::new(0),
                cap,
            });
            let operation = InvocationContext::new(meter.clone());
            let observation = crate::definitions::ecore::ParserObservation::new(&operation);
            assert!(
                matches!(observation.finish(FrozenLoweredSchema::begin(schema.clone(), &root, &observation)),
                Err(ObservedImportError::Observation(message)) if message == "validation holder storage denied")
            );
            assert_eq!(meter.used.get(), retained);
        }
        let exact = Rc::new(Storage {
            used: Cell::new(0),
            cap: 6304,
        });
        let operation = InvocationContext::new(exact.clone());
        let observation = crate::definitions::ecore::ParserObservation::new(&operation);
        let pending = FrozenLoweredSchema::begin(schema.clone(), &root, &observation).unwrap();
        assert_eq!(exact.used.get(), 6304);
        assert!(
            pending.finish(&root).is_err(),
            "a holder alone cannot publish a schema"
        );
        let initial = InvocationContext::default();
        let observation = crate::definitions::ecore::ParserObservation::new(&initial);
        let pointer = schema.classes["A"].name.as_ptr();
        let expected = serde_json::to_vec(&schema).unwrap();
        let mut pending = FrozenLoweredSchema::begin(schema, &root, &observation).unwrap();
        loop {
            let complete = pending.advance(&root, &observation).unwrap();
            let mut moved = Some(pending);
            pending = moved.take().unwrap();
            if complete {
                break;
            }
        }
        let schema = pending.finish(&root).unwrap();
        assert_eq!(schema.classes["A"].name.as_ptr(), pointer);
        assert_eq!(serde_json::to_vec(&schema).unwrap(), expected);
    }

    #[test]
    fn every_retained_validation_advance_cancels_terminally_and_fresh_attempts_recover() {
        let (root, schema) = input(true);
        let original = serde_json::to_vec(&schema).unwrap();
        let initial = InvocationContext::default();
        let observation = crate::definitions::ecore::ParserObservation::new(&initial);
        let mut baseline = LoweredSchemaValidation::new_retained(&root, &observation).unwrap();
        let mut total = 0;
        while !baseline.advance(&root, &schema, &observation).unwrap() {
            total += 1;
        }
        assert!(total > 100);
        for boundary in 0..=total {
            let meter = Rc::new(Meter::default());
            let operation = InvocationContext::new(meter.clone());
            let observation = crate::definitions::ecore::ParserObservation::new(&operation);
            let mut pending = LoweredSchemaValidation::new_retained(&root, &observation).unwrap();
            for _ in 0..boundary {
                assert!(!pending.advance(&root, &schema, &observation).unwrap());
            }
            meter.cancelled.set(true);
            assert_eq!(
                observation.finish(pending.advance(&root, &schema, &observation)),
                Err(ObservedImportError::Observation(
                    "retained validation cancelled".into()
                ))
            );
            meter.cancelled.set(false);
            assert!(
                pending
                    .advance(&root, &schema, &observation)
                    .unwrap_err()
                    .message
                    .contains("has failed")
            );
            let mut recovery = LoweredSchemaValidation::new_retained(&root, &observation).unwrap();
            while !recovery.advance(&root, &schema, &observation).unwrap() {}
            assert_eq!(serde_json::to_vec(&schema).unwrap(), original);
        }
    }

    #[test]
    fn retained_output_lowers_one_field_at_a_time_and_cannot_finish_or_resume_failed_work() {
        let extra = (0..12).map(|index| format!("<eStructuralFeatures xsi:type='ecore:EAttribute' name='f{index}' eType='ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString'/>"))
            .collect::<String>();
        let text = SOURCE.replace(
            "defaultValueLiteral='café'/>",
            &format!("defaultValueLiteral='café'/>{extra}"),
        );
        let source = DefinitionSource::new("memory:retained-output.ecore", SourceKind::Ecore, text);
        let document = Document::parse(source).unwrap();
        let root = Xml::from_node(&document.root, &document.source.text).unwrap();
        let initial = InvocationContext::default();
        let observation = crate::definitions::ecore::ParserObservation::new(&initial);
        let mut expected = None;
        for move_fields in [false, true] {
            let begin = |root: &mut Xml, observation: &crate::definitions::ecore::ParserObservation<'_, '_>| {
                SchemaOutputPreparation::begin(
                    root,
                    "revision".into(),
                    BTreeMap::new(),
                    BTreeMap::new(),
                    None,
                    SchemaLoweringPolicy {
                        validate_projected_operations: false,
                        move_feature_declarations: move_fields,
                        constructor_contracts: vec![],
                        reference_top: None,
                        attribute_domain_defaults: BTreeMap::new(),
                    },
                    observation,
                )
            };
            let mut working = root.clone();
            assert!(
                begin(&mut working, &observation)
                    .unwrap()
                    .finish(&working)
                    .is_err()
            );
            let mut pending = begin(&mut working, &observation).unwrap();
            let mut advances = 0;
            loop {
                let fields = |pending: &SchemaOutputPreparation| {
                    pending
                        .schema
                        .classes
                        .values()
                        .map(|class| class.features.len())
                        .sum::<usize>()
                        + pending
                            .current_class
                            .as_ref()
                            .map_or(0, |(_, class)| class.features.len())
                };
                let before = fields(&pending);
                let done = pending.advance(&mut working, None, &observation).unwrap();
                assert!(
                    fields(&pending) - before <= 1,
                    "a whole class was lowered in one advance"
                );
                advances += 1;
                if done {
                    break;
                }
            }
            assert!(advances > 20);
            let schema = pending.finish(&working).unwrap();
            assert_eq!(schema.classes["C"].features.len(), 13);
            let bytes = serde_json::to_vec(&schema).unwrap();
            match &expected {
                Some(expected) => assert_eq!(&bytes, expected),
                None => expected = Some(bytes),
            }
            assert_eq!(working.source_payload, root.source_payload);
            for boundary in 0..advances {
                let meter = Rc::new(Meter::default());
                let operation = InvocationContext::new(meter.clone());
                let observation = crate::definitions::ecore::ParserObservation::new(&operation);
                let mut working = root.clone();
                let mut pending = begin(&mut working, &observation).unwrap();
                for _ in 0..boundary {
                    assert!(!pending.advance(&mut working, None, &observation).unwrap());
                }
                meter.cancelled.set(true);
                assert!(matches!(
                    observation.finish(pending.advance(&mut working, None, &observation)),
                    Err(ObservedImportError::Observation(_))
                ));
                meter.cancelled.set(false);
                assert!(
                    pending
                        .advance(&mut working, None, &observation)
                        .unwrap_err()
                        .message
                        .contains("has failed")
                );
                assert!(pending.finish(&working).is_err());
                let mut recovered = root.clone();
                let mut pending = begin(&mut recovered, &observation).unwrap();
                while !pending.advance(&mut recovered, None, &observation).unwrap() {}
                assert_eq!(
                    serde_json::to_vec(&pending.finish(&recovered).unwrap()).unwrap(),
                    *expected.as_ref().unwrap()
                );
            }
        }
    }

    #[test]
    fn retained_validation_preserves_first_error_and_exact_classifier_source_span() {
        let (root, original) = input(false);
        for case in 0..5 {
            let mut schema = original.clone();
            let (name, message) = match case {
                0 => {
                    schema
                        .classes
                        .get_mut("A")
                        .unwrap()
                        .features
                        .get_mut("bs")
                        .unwrap()
                        .target = "Missing".into();
                    ("A", "Unknown reference target Missing")
                }
                1 => {
                    schema
                        .classes
                        .get_mut("B")
                        .unwrap()
                        .features
                        .get_mut("a")
                        .unwrap()
                        .opposite = None;
                    ("A", "Inconsistent opposite A.bs")
                }
                2 => {
                    schema
                        .classes
                        .get_mut("B")
                        .unwrap()
                        .features
                        .get_mut("a")
                        .unwrap()
                        .reference = false;
                    ("A", "Inconsistent opposite A.bs")
                }
                3 => {
                    schema
                        .classes
                        .get_mut("B")
                        .unwrap()
                        .features
                        .get_mut("a")
                        .unwrap()
                        .containment = true;
                    ("A", "Container reference must have upper bound 1: A.bs")
                }
                _ => {
                    schema.classes.get_mut("C").unwrap().parents = vec!["C".into()];
                    ("C", "Cyclic inheritance at C")
                }
            };
            let node = root
                .children
                .iter()
                .find(|node| node.attrs.get("name").map(String::as_str) == Some(name))
                .unwrap();
            let before = serde_json::to_vec(&schema).unwrap();
            for projected in [false, true] {
                let operation = InvocationContext::default();
                let observation = crate::definitions::ecore::ParserObservation::new(&operation);
                let mut pending =
                    LoweredSchemaValidation::new(&root, projected, &observation).unwrap();
                let error = loop {
                    match pending.advance(&root, &schema, &observation) {
                        Ok(false) => {}
                        Ok(true) => panic!("invalid schema was accepted"),
                        Err(error) => break error,
                    }
                };
                assert_eq!(error, node.error(message));
                assert_eq!(serde_json::to_vec(&schema).unwrap(), before);
            }
        }
    }

    // Same native lowering source; final declarations derive from the actual
    // lowered source member IDs before the one validation pass begins.
    fn completed_input_fixture() -> (Xml, Schema) {
        let (root, mut schema) = input(false);
        for (name, class) in &schema.classes {
            let member = class.feature_declarations.values().next().unwrap();
            schema.declarations.insert(
                name.clone(),
                ClassifierDeclaration {
                    source: member.source.clone(),
                    path: member.path[..member.path.len() - 1].to_vec(),
                    namespace: schema.uri.clone(),
                    name: name.clone(),
                },
            );
        }
        (root, schema)
    }

    #[test]
    fn completed_field_capture_final_sources_programmatic_fallback_and_defaults() {
        let (root, schema) = completed_input_fixture();
        let operation = InvocationContext::default();
        let observation = crate::definitions::ecore::ParserObservation::new(&operation);
        let mut pending =
            LoweredSchemaValidation::new_with_completed_inputs(&root, true, &observation).unwrap();
        while !pending.advance(&root, &schema, &observation).unwrap() {}
        let captured = pending
            .finish_completed_inputs(&root, &observation)
            .unwrap();
        assert_eq!(captured.fields.len(), schema.classes.len());
        for (class, expected) in &schema.classes {
            let actual = captured.fields.get(class).unwrap();
            assert_eq!(
                actual
                    .field_iter()
                    .map(|(name, field)| (name.to_owned(), field.clone()))
                    .collect::<BTreeMap<_, _>>(),
                schema.features(class).unwrap()
            );
            for name in expected.features.keys() {
                assert!(matches!(
                    actual.member_identity(name),
                    Some(MemberIdentity::Source { .. })
                ));
            }
        }
        assert_eq!(
            captured.fields["C"].get("label").unwrap().default,
            serde_json::json!("café")
        );
        let mut late = schema.clone();
        late.revision.push_str("/grammar-reference-top-revision");
        for (class, actual) in &captured.fields {
            assert_eq!(
                actual
                    .field_iter()
                    .map(|(name, field)| (name.to_owned(), field.clone()))
                    .collect::<BTreeMap<_, _>>(),
                late.features(class).unwrap()
            );
        }
        let mut programmatic = schema.clone();
        programmatic.declarations.clear();
        for class in programmatic.classes.values_mut() {
            class.feature_declarations.clear();
            class.feature_order.clear();
        }
        let programmatic_meter = Rc::new(Meter::default());
        let programmatic_operation = InvocationContext::new(programmatic_meter.clone());
        let programmatic_observation =
            crate::definitions::ecore::ParserObservation::new(&programmatic_operation);
        let mut pending = LoweredSchemaValidation::new_with_completed_inputs(
            &root,
            true,
            &programmatic_observation,
        )
        .unwrap();
        while !pending
            .advance(&root, &programmatic, &programmatic_observation)
            .unwrap()
        {}
        let cold = pending
            .finish_completed_inputs(&root, &programmatic_observation)
            .unwrap();
        assert!(
            cold.cache.is_some(),
            "Programmatic-only cache retention was not exercised"
        );
        let selected = programmatic
            .classes
            .keys()
            .map(|class| programmatic.features(class).unwrap().len())
            .sum::<usize>();
        let release_floor = 16 * selected + 4 * programmatic.classes.len();
        assert!(programmatic_meter.work.get() >= release_floor);
        assert!(
            cold.fields.is_empty(),
            "Programmatic revision identities acquired retained-source authority"
        );
        let mut wrong = schema.clone();
        wrong.declarations.get_mut("C").unwrap().path.push(999);
        let mut pending =
            LoweredSchemaValidation::new_with_completed_inputs(&root, true, &observation).unwrap();
        let error = loop {
            match pending.advance(&root, &wrong, &observation) {
                Ok(false) => {}
                Ok(true) => panic!("Wrong final owner accepted"),
                Err(error) => break error,
            }
        };
        assert!(
            error
                .message
                .contains("Member declaration has wrong owner C")
        );
        assert!(
            pending
                .finish_completed_inputs(&root, &observation)
                .is_err()
        );
    }

    #[test]
    fn completed_field_capture_producer_exact_short_storage_and_cancel_do_not_publish() {
        struct Limits {
            work: Cell<usize>,
            bytes: Cell<usize>,
            checkpoints: Cell<usize>,
            work_limit: usize,
            byte_limit: usize,
            cancel_at: Option<usize>,
        }
        impl InvocationObserver for Limits {
            fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
                Err("No compiler semantic provider authority".into())
            }
            fn checkpoint(&self) -> Result<(), String> {
                let n = self.checkpoints.get();
                self.checkpoints.set(n + 1);
                if self.cancel_at == Some(n) {
                    Err("completed field capture cancelled".into())
                } else {
                    Ok(())
                }
            }
            fn work(&self, n: usize) -> Result<(), String> {
                self.checkpoint()?;
                let next = self.work.get().checked_add(n).ok_or("work overflow")?;
                if next > self.work_limit {
                    return Err("completed field capture work denied".into());
                }
                self.work.set(next);
                Ok(())
            }
            fn retain_storage(&self, n: usize) -> Result<(), String> {
                self.checkpoint()?;
                let next = self.bytes.get().checked_add(n).ok_or("storage overflow")?;
                if next > self.byte_limit {
                    return Err("completed field capture storage denied".into());
                }
                self.bytes.set(next);
                Ok(())
            }
        }
        let (root, schema) = completed_input_fixture();
        let run = |work_limit, byte_limit, cancel_at| {
            let meter = Rc::new(Limits {
                work: Cell::new(0),
                bytes: Cell::new(0),
                checkpoints: Cell::new(0),
                work_limit,
                byte_limit,
                cancel_at,
            });
            let operation = InvocationContext::new(meter.clone());
            let observation = crate::definitions::ecore::ParserObservation::new(&operation);
            let result = (|| {
                let mut pending =
                    LoweredSchemaValidation::new_with_completed_inputs(&root, true, &observation)?;
                while !pending.advance(&root, &schema, &observation)? {}
                pending.finish_completed_inputs(&root, &observation)
            })();
            (result, meter)
        };
        let (complete, measured) = run(usize::MAX, usize::MAX, None);
        let complete = complete.unwrap();
        assert_eq!(complete.fields.len(), 3);
        let units = measured.work.get();
        let bytes = measured.bytes.get();
        let checkpoints = measured.checkpoints.get();
        assert!(units > 0 && bytes > 0 && checkpoints > 4);
        assert!(run(units, bytes, None).0.is_ok());
        for (work, storage, stop, expected) in [
            (units - 1, bytes, None, "work denied"),
            (units, bytes - 1, None, "storage denied"),
            (units, bytes, Some(checkpoints / 2), "cancelled"),
        ] {
            let (result, meter) = run(work, storage, stop);
            assert!(result.err().unwrap().message.contains(expected));
            assert!(meter.work.get() <= work && meter.bytes.get() <= storage);
        }
        assert!(
            run(units, bytes, None).0.is_ok(),
            "Fresh canonical producer did not recover"
        );
    }
}
