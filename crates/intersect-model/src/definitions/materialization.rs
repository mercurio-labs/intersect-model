//! Explicit closed-set materialization through the existing structural lowerer.
use super::*;
use crate::definitions::{
    bundle::{DefinitionBundle, SourceSpan},
    generic_resolution::{
        PreparedRuntimeMembers, Resolver, RuntimeMembersPreparation, SpecializedSignature,
    },
    generic_types::{ClosedType, RuntimeType, Type},
    resources::{DeclarationId, ResourceSet},
};
use std::collections::BTreeSet;
#[path = "materialization_policy.rs"]
mod policy;
use policy::CompilationPolicyInputs;
#[path = "materialization_prepared.rs"]
mod prepared;
use prepared::PreparedMaterialization;
#[path = "materialization_owned_schema.rs"]
mod owned_schema;
pub(crate) use owned_schema::OwnedSchemaPreparation;
#[path = "nominal_reference_bitmaps.rs"]
mod nominal_bitmaps;
use nominal_bitmaps::NominalReferenceIndex;

fn key(ty: &RuntimeType) -> String {
    format!("__type_{}", ty.identity().replace(':', "_"))
}
// Preserve each caller's existing schema diagnostic while keeping observation
// failures typed through the importer's receipt. Admit the input before cloning.
fn runtime_type(
    expression: &Type,
    source: &crate::definitions::bundle::SourceId,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    schema_error: impl FnOnce(String) -> ImportError,
) -> Result<RuntimeType, ImportError> {
    observation.serialized(expression, source)?;
    runtime_type_owned(expression.clone(), source, observation, schema_error)
}
fn runtime_type_owned(
    expression: Type,
    source: &crate::definitions::bundle::SourceId,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    schema_error: impl FnOnce(String) -> ImportError,
) -> Result<RuntimeType, ImportError> {
    observation
        .preparation(source, |context| {
            match RuntimeType::new_observed(expression, context) {
                Ok(value) => Ok(Ok(value)),
                Err(crate::definitions::schema::PreparationError::Schema(message)) => {
                    Ok(Err(message))
                }
                Err(error @ crate::definitions::schema::PreparationError::Observation(_)) => {
                    Err(error)
                }
            }
        })?
        .map_err(schema_error)
}
pub(super) fn classifier_type(
    declaration: &DeclarationId,
    raw: bool,
    node: &Xml,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<RuntimeType, ImportError> {
    observation.serialized(declaration, &node.opening.source)?;
    runtime_type_owned(
        Type::Classifier {
            declaration: declaration.clone(),
            arguments: vec![],
            raw,
        },
        &node.opening.source,
        observation,
        |e| node.error(e),
    )
}
fn has_formals(
    node: &Xml,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<bool, ImportError> {
    for child in &node.children {
        observation.work(1, &node.opening.source)?;
        if child.name == "eTypeParameters" {
            return Ok(true);
        }
    }
    Ok(false)
}
// Bound discovered work before retaining it, including ancestors not processed yet.
// Keeping processed identities prevents overlapping inheritance paths requeueing them.
fn enqueue_type(
    ty: RuntimeType,
    pending: &mut Vec<RuntimeType>,
    discovered: &mut BTreeSet<String>,
    span: &SourceSpan,
) -> Result<(), ImportError> {
    let identity = key(&ty);
    enqueue_type_with_identity(ty, identity, pending, discovered, span)
}
fn enqueue_type_with_identity(
    ty: RuntimeType,
    identity: String,
    pending: &mut Vec<RuntimeType>,
    discovered: &mut BTreeSet<String>,
    span: &SourceSpan,
) -> Result<(), ImportError> {
    if discovered.contains(&identity) {
        return Ok(());
    }
    if discovered.len() >= 4096 {
        return Err(ImportError::new(
            ErrorKind::ResourceLimit,
            span,
            "Materialization type budget exceeded",
        ));
    }
    discovered.insert(identity);
    pending.push(ty);
    Ok(())
}
fn definition_id(ty: &Type) -> Option<&DeclarationId> {
    match ty {
        Type::Classifier { declaration, .. } => Some(declaration),
        _ => None,
    }
}
// Reference candidates come only from the exact reports already checked and
// admitted by this materialization. Their canonical local keys need no second
// runtime-type copy or XML declaration import. Attributes retain their native
// provider-dependent target lowering.
#[derive(Clone, Copy)]
enum RuntimeCandidate<'a> {
    Reference {
        receiver: &'a RuntimeType,
        key: &'a str,
    },
    Attribute(&'a RuntimeType),
}
// Allocation-free transient views over complete retained candidate inputs.
#[derive(Clone, Copy)]
enum CandidateInputs<'input, 'source> {
    #[cfg(test)]
    Views(&'input [RuntimeCandidate<'input>]),
    Reports(&'input [(String, std::rc::Rc<PreparedRuntimeMembers<'source>>)]),
    Attributes(&'input [(RuntimeType, bool)]),
}
impl<'input> CandidateInputs<'input, '_> {
    fn len(self) -> usize {
        match self {
            #[cfg(test)]
            Self::Views(values) => values.len(),
            Self::Reports(values) => values.len(),
            Self::Attributes(values) => values.len(),
        }
    }
    fn get(self, index: usize) -> Option<RuntimeCandidate<'input>> {
        match self {
            #[cfg(test)]
            Self::Views(values) => values.get(index).copied(),
            Self::Reports(values) => {
                values
                    .get(index)
                    .map(|(key, report)| RuntimeCandidate::Reference {
                        receiver: &report.receiver,
                        key,
                    })
            }
            Self::Attributes(values) => values
                .get(index)
                .map(|(ty, _)| RuntimeCandidate::Attribute(ty)),
        }
    }
}
impl<'a> RuntimeCandidate<'a> {
    fn reference(
        key: &'a str,
        report: &'a PreparedRuntimeMembers<'_>,
        context: &Context<'_, '_>,
    ) -> Result<Self, ImportError> {
        let declaration = definition_id(report.receiver.expression()).ok_or_else(|| {
            ImportError::new(
                ErrorKind::InvalidDefinition,
                &report.declaration_span,
                "Expected closed classifier",
            )
        })?;
        // Preserve target lowering's source checks once, using the same exact
        // declaration. A generated local key cannot supply a missing source name.
        let header = context.xml_header(declaration)?;
        required(&header, "name")?;
        if required(&header, "xsi:type")? != "ecore:EClass" {
            return Err(header.error("Runtime reference candidate requires an EClass"));
        }
        Ok(Self::Reference {
            receiver: &report.receiver,
            key,
        })
    }
    fn receiver(&self) -> &RuntimeType {
        match self {
            Self::Reference { receiver, .. } | Self::Attribute(receiver) => receiver,
        }
    }
    fn local_target(&self, context: &mut Context<'_, '_>, at: &Xml) -> Result<String, ImportError> {
        match self {
            Self::Reference { key, .. } => {
                context.observation.string(key.len(), &at.opening.source)?;
                Ok((*key).to_owned())
            }
            Self::Attribute(candidate) => {
                let target = runtime_attribute_target(context, candidate, at)?;
                local_type(&target).map_err(|e| at.error(e))
            }
        }
    }
}

trait AlternativeStorage {
    const BORROW_OWNER_PATHS: bool = false;
    const SHARE_REFERENCE_ARRAYS: bool = false;
    fn reference_array(
        &mut self,
        _: &[usize],
        _: CandidateInputs<'_, '_>,
        _: &crate::definitions::ecore::ParserObservation<'_, '_>,
        at: &Xml,
    ) -> Result<
        <Self::Target as crate::definitions::schema::RuntimeAlternative>::Alternatives,
        ImportError,
    > {
        Err(at.error("Reference array input storage is unavailable"))
    }
    type Target: crate::definitions::schema::RuntimeAlternative;
    fn domains(
        &mut self,
        report: PreparedRuntimeMembers<'_>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<
        Vec<<Self::Target as crate::definitions::schema::RuntimeAlternative>::Source>,
        ImportError,
    >;
    fn reference(
        &mut self,
        key: &str,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        source: &crate::definitions::bundle::SourceId,
    ) -> Result<Self::Target, ImportError>;
    fn attribute(
        &mut self,
        key: String,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        source: &crate::definitions::bundle::SourceId,
    ) -> Result<Self::Target, ImportError>;
}
struct OwnedAlternatives;
impl AlternativeStorage for OwnedAlternatives {
    type Target = String;
    fn domains(
        &mut self,
        report: PreparedRuntimeMembers<'_>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<crate::definitions::generic_resolution::RuntimeFeatureDomain>, ImportError>
    {
        report.into_feature_domains_observed(observation)
    }
    fn reference(
        &mut self,
        key: &str,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        source: &crate::definitions::bundle::SourceId,
    ) -> Result<String, ImportError> {
        observation.string(key.len(), source)?;
        Ok(key.to_owned())
    }
    fn attribute(
        &mut self,
        key: String,
        _: &crate::definitions::ecore::ParserObservation<'_, '_>,
        _: &crate::definitions::bundle::SourceId,
    ) -> Result<String, ImportError> {
        Ok(key)
    }
}
#[derive(Default)]
struct SharedAlternatives {
    keys: crate::definitions::schema::RuntimeTargetInputs,
}
impl AlternativeStorage for SharedAlternatives {
    type Target = crate::definitions::schema::SharedRuntimeTarget;
    fn domains(
        &mut self,
        report: PreparedRuntimeMembers<'_>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<crate::definitions::generic_resolution::RuntimeFeatureDomain>, ImportError>
    {
        report.into_feature_domains_observed(observation)
    }
    fn reference(
        &mut self,
        key: &str,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        source: &crate::definitions::bundle::SourceId,
    ) -> Result<Self::Target, ImportError> {
        observation.preparation(source, |context| {
            self.keys
                .intern_observed(key, context)
                .map_err(crate::definitions::schema::PreparationError::Observation)
        })
    }
    fn attribute(
        &mut self,
        key: String,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        source: &crate::definitions::bundle::SourceId,
    ) -> Result<Self::Target, ImportError> {
        self.reference(&key, observation, source)
    }
}
#[derive(Default)]
struct SharedDomainAlternatives {
    keys: crate::definitions::schema::RuntimeTargetInputs,
    domains: crate::definitions::schema::RuntimeDomainInputs,
}
impl AlternativeStorage for SharedDomainAlternatives {
    const BORROW_OWNER_PATHS: bool = true;
    type Target = crate::definitions::schema::SharedRuntimeDomainTarget;
    fn domains(
        &mut self,
        report: PreparedRuntimeMembers<'_>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<crate::definitions::schema::SharedRuntimeDomain>, ImportError> {
        report.into_shared_feature_domains_observed(&mut self.domains, observation)
    }
    fn reference(
        &mut self,
        key: &str,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        source: &crate::definitions::bundle::SourceId,
    ) -> Result<Self::Target, ImportError> {
        observation.preparation(source, |context| {
            self.keys
                .intern_observed(key, context)
                .map(crate::definitions::schema::SharedRuntimeDomainTarget::from)
                .map_err(crate::definitions::schema::PreparationError::Observation)
        })
    }
    fn attribute(
        &mut self,
        key: String,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        source: &crate::definitions::bundle::SourceId,
    ) -> Result<Self::Target, ImportError> {
        self.reference(&key, observation, source)
    }
}
struct SharedArrayAlternatives<
    A: crate::definitions::schema::RuntimeAlternative = crate::definitions::schema::SharedRuntimeArrayTarget,
> {
    keys: crate::definitions::schema::RuntimeTargetInputs,
    domains: crate::definitions::schema::RuntimeDomainInputs,
    arrays: Vec<A::Alternatives>,
}
impl<A: crate::definitions::schema::RuntimeAlternative> Default for SharedArrayAlternatives<A> {
    fn default() -> Self {
        Self {
            keys: Default::default(),
            domains: crate::definitions::schema::RuntimeDomainInputs::with_observed_tables(
                A::OBSERVED_SOURCE_TABLES,
            ),
            arrays: Vec::new(),
        }
    }
}
impl<A> AlternativeStorage for SharedArrayAlternatives<A>
where
    A: crate::definitions::schema::RuntimeAlternative<
            Source = crate::definitions::schema::SharedRuntimeDomain,
        > + From<crate::definitions::schema::SharedRuntimeTarget>,
{
    const BORROW_OWNER_PATHS: bool = true;
    const SHARE_REFERENCE_ARRAYS: bool = true;
    type Target = A;
    fn domains(
        &mut self,
        report: PreparedRuntimeMembers<'_>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Vec<crate::definitions::schema::SharedRuntimeDomain>, ImportError> {
        report.into_shared_feature_domains_observed(&mut self.domains, observation)
    }
    fn reference(
        &mut self,
        key: &str,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        source: &crate::definitions::bundle::SourceId,
    ) -> Result<Self::Target, ImportError> {
        observation.preparation(source, |context| {
            self.keys
                .intern_observed(key, context)
                .map(A::from)
                .map_err(crate::definitions::schema::PreparationError::Observation)
        })
    }
    fn attribute(
        &mut self,
        key: String,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        source: &crate::definitions::bundle::SourceId,
    ) -> Result<Self::Target, ImportError> {
        self.reference(&key, observation, source)
    }
    fn reference_array(
        &mut self,
        selected: &[usize],
        possible: CandidateInputs<'_, '_>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        at: &Xml,
    ) -> Result<A::Alternatives, ImportError> {
        let source = &at.opening.source;
        let selected_key = |index: usize| -> Result<&str, ImportError> {
            match possible.get(index) {
                Some(RuntimeCandidate::Reference { key, .. }) => Ok(key),
                _ => Err(at.error("Invalid reference alternative input index")),
            }
        };
        // Search immutable serialized input values, after the caller has freshly
        // run every candidate admission. No source or membership result is stored.
        let mut low = 0;
        let mut high = self.arrays.len();
        while low < high {
            observation.work(1, source)?;
            let mid = low + (high - low) / 2;
            let stored = &self.arrays[mid];
            let mut order = std::cmp::Ordering::Equal;
            for (index, key) in selected.iter().zip(stored.iter()) {
                observation.work(1, source)?;
                order = selected_key(*index)?.cmp(key.as_ref());
                if order != std::cmp::Ordering::Equal {
                    break;
                }
            }
            if order == std::cmp::Ordering::Equal {
                order = selected.len().cmp(&stored.len());
            }
            match order {
                std::cmp::Ordering::Less => high = mid,
                std::cmp::Ordering::Greater => low = mid + 1,
                std::cmp::Ordering::Equal => {
                    observation.reserve(32, source)?;
                    return Ok(stored.clone());
                }
            }
        }
        let bytes = observation.size(
            selected
                .len()
                .checked_mul(32)
                .and_then(|n| n.checked_add(40)),
            source,
        )?;
        observation.reserve(bytes, source)?;
        let mut targets = Vec::new();
        targets.try_reserve_exact(selected.len()).map_err(|_| {
            ImportError::new(
                ErrorKind::ResourceLimit,
                &at.opening,
                "Runtime alternative array allocation failed",
            )
        })?;
        for index in selected {
            observation.work(1, source)?;
            targets.push(self.reference(selected_key(*index)?, observation, source)?);
        }
        if self.arrays.len() == self.arrays.capacity() {
            let next = observation.size(
                self.arrays.capacity().checked_mul(2).map(|n| n.max(1)),
                source,
            )?;
            let bytes =
                observation.size((next - self.arrays.capacity()).checked_mul(32), source)?;
            observation.reserve(bytes, source)?;
            self.arrays
                .try_reserve_exact(next - self.arrays.len())
                .map_err(|_| {
                    ImportError::new(
                        ErrorKind::ResourceLimit,
                        &at.opening,
                        "Runtime alternative input table allocation failed",
                    )
                })?;
        }
        observation.work(self.arrays.len() - low, source)?;
        observation.reserve(32, source)?;
        let array = A::Alternatives::from(targets);
        self.arrays.insert(low, array.clone());
        Ok(array)
    }
}
impl RuntimeCandidate<'_> {
    fn alternative_target<P: AlternativeStorage>(
        &self,
        context: &mut Context<'_, '_>,
        at: &Xml,
        storage: &mut P,
    ) -> Result<P::Target, ImportError> {
        match self {
            Self::Reference { key, .. } => {
                storage.reference(key, context.observation, &at.opening.source)
            }
            Self::Attribute(_) => {
                let target = self.local_target(context, at)?;
                storage.attribute(target, context.observation, &at.opening.source)
            }
        }
    }
}
// Candidate results retain their exact order while the complete source domains
// remain in prepared reports until class/opposite lowering has finished.
struct PendingFeatureDomain<A: crate::definitions::schema::RuntimeAlternative = String> {
    alternatives: A::Alternatives,
    unresolved: Vec<crate::definitions::schema::UnresolvedRuntimeFeatureCandidate>,
    lexical_target: Option<String>,
    default: Value,
}
struct CandidateSelection<A: crate::definitions::schema::RuntimeAlternative> {
    alternatives: A::Alternatives,
    unresolved: Vec<crate::definitions::schema::UnresolvedRuntimeFeatureCandidate>,
    reference_selection: Vec<usize>,
}

// Handwritten candidate continuation over the same source-bound checker.
// No implementation can publish alternatives from a failed or unfinished scan.
// Owned execution progress only. Authority remains with the checked source
// binding; this private state cannot publish a schema or authorize a candidate.
struct CandidateScan<A: crate::definitions::schema::RuntimeAlternative> {
    observe_membership: bool,
    reference: bool,
    next_candidate: usize,
    reference_selection: Vec<usize>,
    alternatives: BTreeSet<A>,
    reference_alternatives: Vec<A>,
    reference_capacity: usize,
    unresolved: Vec<crate::definitions::schema::UnresolvedRuntimeFeatureCandidate>,
    failed: bool,
    published: bool,
}
impl<A: crate::definitions::schema::RuntimeAlternative> CandidateScan<A> {
    fn new(reference: bool, mut reference_selection: Vec<usize>) -> Self {
        reference_selection.clear();
        Self {
            observe_membership: false,
            reference,
            next_candidate: 0,
            reference_selection,
            alternatives: BTreeSet::new(),
            reference_alternatives: Vec::new(),
            reference_capacity: 0,
            unresolved: Vec::new(),
            failed: false,
            published: false,
        }
    }
}
struct CandidatePreparation<'selection, 'source, A: crate::definitions::schema::RuntimeAlternative>
{
    admission:
        &'selection crate::definitions::generic_resolution::CheckedRuntimeFeatureDomain<'source>,
    possible: CandidateInputs<'selection, 'source>,
    scan: &'selection mut CandidateScan<A>,
}
impl<'selection, 'source, A: crate::definitions::schema::RuntimeAlternative>
    CandidatePreparation<'selection, 'source, A>
{
    #[cfg(test)]
    fn new(
        admission: &'selection crate::definitions::generic_resolution::CheckedRuntimeFeatureDomain<
            'source,
        >,
        possible: &'selection [RuntimeCandidate<'source>],
        scan: &'selection mut CandidateScan<A>,
    ) -> Self {
        Self::from_inputs(admission, CandidateInputs::Views(possible), scan)
    }
    fn from_inputs(
        admission: &'selection crate::definitions::generic_resolution::CheckedRuntimeFeatureDomain<
            'source,
        >,
        possible: CandidateInputs<'selection, 'source>,
        scan: &'selection mut CandidateScan<A>,
    ) -> Self {
        Self {
            admission,
            possible,
            scan,
        }
    }
    fn advance<P: AlternativeStorage<Target = A>>(
        &mut self,
        context: &mut Context<'_, '_>,
        field: &Xml,
        storage: &mut P,
        comparisons: &mut usize,
    ) -> Result<bool, ImportError> {
        if self.scan.failed || self.scan.published {
            return Err(field.error("Runtime candidate preparation has failed"));
        }
        let result = self.advance_inner(context, field, storage, comparisons);
        if result.is_err() {
            self.scan.failed = true;
        }
        result
    }
    fn advance_inner<P: AlternativeStorage<Target = A>>(
        &mut self,
        context: &mut Context<'_, '_>,
        field: &Xml,
        storage: &mut P,
        comparisons: &mut usize,
    ) -> Result<bool, ImportError> {
        if self.scan.next_candidate == self.possible.len() {
            return Ok(true);
        }
        let candidate_index = self.scan.next_candidate;
        let prepared_candidate = self
            .possible
            .get(candidate_index)
            .ok_or_else(|| field.error("Missing retained runtime candidate"))?;
        self.scan.next_candidate += 1;
        let observation = context.observation;
        let candidate = prepared_candidate.receiver();
        observation.work(1, &field.opening.source)?;
        *comparisons = comparisons.checked_sub(1).ok_or_else(|| {
            field.unsupported("Runtime feature candidate comparison budget exceeded")
        })?;
        let membership = if self.scan.observe_membership {
            self.admission.accepts_observed(candidate, observation)
        } else {
            self.admission.accepts(candidate)
        };
        let accepted = match membership {
            Ok(accepted) => accepted,
            Err(error) if error.kind == ErrorKind::Unsupported => {
                let target = prepared_candidate.local_target(context, field)?;
                self.scan.unresolved.push(
                    crate::definitions::schema::UnresolvedRuntimeFeatureCandidate {
                        candidate: candidate.clone(),
                        target,
                        reason: error.to_string(),
                    },
                );
                return Ok(self.scan.next_candidate == self.possible.len());
            }
            Err(error) => return Err(error),
        };
        if accepted {
            if self.scan.reference && P::SHARE_REFERENCE_ARRAYS {
                if self.scan.reference_selection.len() == self.scan.reference_selection.capacity() {
                    let next = observation
                        .size(
                            self.scan
                                .reference_selection
                                .capacity()
                                .checked_mul(2)
                                .map(|n| n.max(1)),
                            &field.opening.source,
                        )?
                        .min(self.possible.len());
                    let bytes = observation.size(
                        (next - self.scan.reference_selection.capacity()).checked_mul(8),
                        &field.opening.source,
                    )?;
                    observation.reserve(bytes, &field.opening.source)?;
                    self.scan
                        .reference_selection
                        .try_reserve_exact(next - self.scan.reference_selection.len())
                        .map_err(|_| {
                            ImportError::new(
                                ErrorKind::ResourceLimit,
                                &field.opening,
                                "Runtime reference selection allocation failed",
                            )
                        })?;
                }
                self.scan.reference_selection.push(candidate_index);
                return Ok(self.scan.next_candidate == self.possible.len());
            }
            let target = prepared_candidate.alternative_target(context, field, storage)?;
            if self.scan.reference {
                if self.scan.reference_alternatives.len() == self.scan.reference_capacity {
                    let next = observation
                        .size(
                            self.scan
                                .reference_capacity
                                .checked_mul(2)
                                .map(|slots| slots.max(1)),
                            &field.opening.source,
                        )?
                        .min(self.possible.len());
                    let slots = next - self.scan.reference_capacity;
                    let bytes = observation.size(slots.checked_mul(32), &field.opening.source)?;
                    observation.reserve(bytes, &field.opening.source)?;
                    self.scan
                        .reference_alternatives
                        .try_reserve_exact(next - self.scan.reference_alternatives.len())
                        .map_err(|_| {
                            ImportError::new(
                                ErrorKind::ResourceLimit,
                                &field.opening,
                                "Runtime reference alternative allocation failed",
                            )
                        })?;
                    self.scan.reference_capacity = next;
                }
                self.scan.reference_alternatives.push(target);
            } else {
                observation.reserve(128, &field.opening.source)?;
                self.scan.alternatives.insert(target);
            }
        }
        Ok(self.scan.next_candidate == self.possible.len())
    }
    fn finish<P: AlternativeStorage<Target = A>>(
        self,
        storage: &mut P,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
        field: &Xml,
    ) -> Result<CandidateSelection<A>, ImportError> {
        if self.scan.failed
            || self.scan.published
            || self.scan.next_candidate != self.possible.len()
        {
            return Err(field.error("Runtime candidate preparation is incomplete or failed"));
        }
        // A publication error is terminal even when the source view is rebound.
        self.scan.failed = true;
        let alternatives = if self.scan.reference && P::SHARE_REFERENCE_ARRAYS {
            storage.reference_array(
                &self.scan.reference_selection,
                self.possible,
                observation,
                field,
            )?
        } else {
            if A::COLLECTION_HEADER_BYTES != 0 {
                observation.reserve(A::COLLECTION_HEADER_BYTES, &field.opening.source)?;
            }
            let values = if self.scan.reference {
                std::mem::take(&mut self.scan.reference_alternatives)
            } else {
                std::mem::take(&mut self.scan.alternatives)
                    .into_iter()
                    .collect()
            };
            values.into()
        };
        self.scan.failed = false;
        self.scan.published = true;
        Ok(CandidateSelection {
            alternatives,
            unresolved: std::mem::take(&mut self.scan.unresolved),
            reference_selection: std::mem::take(&mut self.scan.reference_selection),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FeaturePreparationPhase {
    Begin,
    Scan,
    Publish,
    Commit,
    Complete,
    Failed,
}
struct ActiveFeaturePreparation<A: crate::definitions::schema::RuntimeAlternative> {
    field: std::rc::Rc<Xml>,
    default: Value,
    lexical_target: Option<String>,
    scan: CandidateScan<A>,
}
struct FeatureDomainSelection<A: crate::definitions::schema::RuntimeAlternative> {
    domains: BTreeMap<String, Vec<PendingFeatureDomain<A>>>,
    attribute_defaults: BTreeMap<(String, DeclarationId), Value>,
}
// One attempt over fixed complete checked reports. Only transient source views
// are reconstructed at each advance; all semantic checks still execute freshly.
// Owned current-attempt progress, independent of transient source/observer views.
struct FeatureDomainProgress<A: crate::definitions::schema::RuntimeAlternative> {
    observe_membership: bool,
    phase: FeaturePreparationPhase,
    report: usize,
    domain: usize,
    comparisons: usize,
    reference_selection: Vec<usize>,
    checked: Vec<PendingFeatureDomain<A>>,
    domains: BTreeMap<String, Vec<PendingFeatureDomain<A>>>,
    attribute_defaults: BTreeMap<(String, DeclarationId), Value>,
    active: Option<ActiveFeaturePreparation<A>>,
}
impl<A: crate::definitions::schema::RuntimeAlternative> FeatureDomainProgress<A> {
    fn new() -> Self {
        Self {
            observe_membership: false,
            phase: FeaturePreparationPhase::Begin,
            report: 0,
            domain: 0,
            comparisons: 65_536,
            reference_selection: Vec::new(),
            checked: Vec::new(),
            domains: BTreeMap::new(),
            attribute_defaults: BTreeMap::new(),
            active: None,
        }
    }
}
struct FeatureDomainPreparation<'a, 'source, P: AlternativeStorage> {
    context: &'a mut MaterializationContext<'source>,
    reports: &'a [(String, std::rc::Rc<PreparedRuntimeMembers<'source>>)],
    nominal: Option<&'a NominalReferenceIndex<'source>>,
    reference_candidates: CandidateInputs<'a, 'source>,
    attribute_candidates: CandidateInputs<'a, 'source>,
    storage: &'a mut P,
    at: &'a Xml,
    progress: FeatureDomainProgress<P::Target>,
}
impl<'a, 'source, P: AlternativeStorage> FeatureDomainPreparation<'a, 'source, P> {
    #[cfg(test)]
    fn new(
        context: &'a mut MaterializationContext<'source>,
        reports: &'a [(String, std::rc::Rc<PreparedRuntimeMembers<'source>>)],
        reference_candidates: CandidateInputs<'a, 'source>,
        attribute_candidates: CandidateInputs<'a, 'source>,
        storage: &'a mut P,
        at: &'a Xml,
    ) -> Self {
        Self {
            context,
            reports,
            nominal: None,
            reference_candidates,
            attribute_candidates,
            storage,
            at,
            progress: FeatureDomainProgress::new(),
        }
    }
    fn from_inputs(
        context: &'a mut MaterializationContext<'source>,
        inputs: &'a FeaturePreparationInputs<'source>,
        storage: &'a mut P,
        progress: FeatureDomainProgress<P::Target>,
    ) -> Self {
        Self {
            context,
            reports: &inputs.reports,
            nominal: inputs.nominal.as_deref(),
            reference_candidates: CandidateInputs::Reports(&inputs.reports),
            attribute_candidates: CandidateInputs::Attributes(&inputs.candidates),
            storage,
            at: &inputs.root,
            progress,
        }
    }
    #[cfg(test)]
    fn with_progress(mut self, progress: FeatureDomainProgress<P::Target>) -> Self {
        self.progress = progress;
        self
    }
    fn into_progress(self) -> FeatureDomainProgress<P::Target> {
        self.progress
    }
    fn advance(
        &mut self,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        if self.progress.phase == FeaturePreparationPhase::Failed {
            return Err(self.at.error("Feature-domain preparation has failed"));
        }
        if self.progress.phase == FeaturePreparationPhase::Complete {
            return Ok(true);
        }
        let result = observation
            .work(0, &self.at.opening.source)
            .and_then(|()| self.advance_inner(conversions, observation));
        if result.is_err() {
            self.progress.phase = FeaturePreparationPhase::Failed;
        }
        result
    }
    fn advance_inner(
        &mut self,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        let mut context = Context {
            state: self.context,
            observation,
        };
        let reports = self.reports;
        let Some((name, report)) = reports.get(self.progress.report) else {
            self.progress.phase = FeaturePreparationPhase::Complete;
            return Ok(true);
        };
        if self.progress.phase == FeaturePreparationPhase::Commit {
            if !self.progress.checked.is_empty() {
                observation.string(name.len(), &report.declaration_span.source)?;
                observation.reserve(128, &report.declaration_span.source)?;
                self.progress
                    .domains
                    .insert(name.clone(), std::mem::take(&mut self.progress.checked));
            }
            self.progress.report += 1;
            self.progress.domain = 0;
            self.progress.phase = FeaturePreparationPhase::Begin;
            return Ok(false);
        }
        let Some(admission) = report.checked_feature_domain(self.progress.domain) else {
            self.progress.phase = FeaturePreparationPhase::Commit;
            return Ok(false);
        };
        let domain = admission.source();
        if self.progress.phase == FeaturePreparationPhase::Begin {
            let field = context.xml_read(domain.projection.source.declaration)?;
            let (default, lexical_target) = if domain.reference {
                let upper = field
                    .attrs
                    .get("upperBound")
                    .map(String::as_str)
                    .unwrap_or("1");
                (
                    if upper == "1" {
                        Value::Null
                    } else {
                        Value::Array(vec![])
                    },
                    None,
                )
            } else {
                runtime_attribute_default(&mut context, domain, conversions)?
            };
            let mut scan = CandidateScan::new(
                domain.reference,
                std::mem::take(&mut self.progress.reference_selection),
            );
            scan.observe_membership = self.progress.observe_membership;
            let bitmap = if domain.reference && P::SHARE_REFERENCE_ARRAYS {
                match self.nominal {
                    Some(index) => index.start_selection(&admission, observation)?,
                    None => false,
                }
            } else {
                false
            };
            if bitmap {
                // Retain the original logical candidate-pair ceiling. Actual
                // bitmap/word work is separately observed by the new algorithm.
                self.progress.comparisons = self
                    .progress
                    .comparisons
                    .checked_sub(self.reference_candidates.len())
                    .ok_or_else(|| {
                        field.unsupported("Runtime feature candidate comparison budget exceeded")
                    })?;
            }
            self.progress.active = Some(ActiveFeaturePreparation {
                field,
                default,
                lexical_target,
                scan,
            });
            self.progress.phase = FeaturePreparationPhase::Scan;
            return Ok(false);
        }
        let possible = if domain.reference {
            self.reference_candidates
        } else {
            self.attribute_candidates
        };
        let active = self
            .progress
            .active
            .as_mut()
            .ok_or_else(|| self.at.error("Missing active feature-domain preparation"))?;
        if self.progress.phase == FeaturePreparationPhase::Scan {
            let bitmap_complete = if domain.reference && P::SHARE_REFERENCE_ARRAYS {
                match self.nominal {
                    Some(index) => {
                        index.advance_selection(&mut active.scan, observation, &active.field)?
                    }
                    None => None,
                }
            } else {
                None
            };
            let complete = match bitmap_complete {
                Some(complete) => complete,
                None => CandidatePreparation::from_inputs(&admission, possible, &mut active.scan)
                    .advance(
                    &mut context,
                    &active.field,
                    self.storage,
                    &mut self.progress.comparisons,
                )?,
            };
            if complete {
                self.progress.phase = FeaturePreparationPhase::Publish;
            }
            return Ok(false);
        }
        if self.progress.phase != FeaturePreparationPhase::Publish {
            return Err(self.at.error("Invalid feature-domain preparation phase"));
        }
        observation.serialized(&active.default, &active.field.opening.source)?;
        observation.reserve(128 + 32, &active.field.opening.source)?;
        let selection = CandidatePreparation::from_inputs(&admission, possible, &mut active.scan)
            .finish(self.storage, observation, &active.field)?;
        self.progress.reference_selection = selection.reference_selection;
        self.progress.checked.push(PendingFeatureDomain {
            alternatives: selection.alternatives,
            unresolved: selection.unresolved,
            lexical_target: active.lexical_target.take(),
            default: active.default.clone(),
        });
        if !domain.reference
            && domain.projection.storage_owner.expression() == report.receiver.expression()
        {
            let key = (name.clone(), domain.projection.source.declaration.clone());
            if let Some(previous) = self
                .progress
                .attribute_defaults
                .insert(key, active.default.clone())
                && previous != active.default
            {
                return Err(active.field.error("Conflicting source attribute defaults"));
            }
        }
        self.progress.active = None;
        self.progress.domain += 1;
        self.progress.phase = FeaturePreparationPhase::Begin;
        Ok(false)
    }
    fn finish(self) -> Result<FeatureDomainSelection<P::Target>, ImportError> {
        if self.progress.phase != FeaturePreparationPhase::Complete {
            return Err(self
                .at
                .error("Feature-domain preparation is incomplete or failed"));
        }
        Ok(FeatureDomainSelection {
            domains: self.progress.domains,
            attribute_defaults: self.progress.attribute_defaults,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReportPreparationPhase {
    Next,
    Members,
    Parents,
    Register,
    Complete,
    Failed,
}
struct ActiveReport<'source> {
    name: String,
    report: std::rc::Rc<PreparedRuntimeMembers<'source>>,
    next_parent: usize,
}
type ReportSelections<'source> = BTreeMap<
    String,
    (
        crate::definitions::generic_resolution::GenericOperationSet,
        std::borrow::Cow<'source, SourceSpan>,
    ),
>;
// Attempt-local progress remains inside the fixed complete source owner. It
// stores no observer, provider registry, or reusable conformance outcome cache.
struct ReportPreparation<'source> {
    pending: Vec<RuntimeType>,
    discovered: BTreeSet<String>,
    selections: ReportSelections<'source>,
    active: Option<ActiveReport<'source>>,
    phase: ReportPreparationPhase,
    member_preparation: Option<RuntimeMembersPreparation<'source, 'source>>,
    member_name: Option<String>,
}
impl<'source> ReportPreparation<'source> {
    fn new(requested: &[RuntimeType], fallback: &SourceSpan) -> Result<Self, ImportError> {
        let mut pending = Vec::new();
        let mut discovered = BTreeSet::new();
        for ty in requested {
            enqueue_type(ty.clone(), &mut pending, &mut discovered, fallback)?;
        }
        Ok(Self {
            pending,
            discovered,
            selections: BTreeMap::new(),
            active: None,
            phase: ReportPreparationPhase::Next,
            member_preparation: None,
            member_name: None,
        })
    }
    fn new_observed(
        requested: &[RuntimeType],
        fallback: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Self, ImportError> {
        let mut pending = Vec::new();
        let mut discovered = BTreeSet::new();
        let bytes = observation.size(
            requested
                .len()
                .checked_mul(32)
                .and_then(|n| n.checked_add(128)),
            &fallback.source,
        )?;
        observation.reserve(bytes, &fallback.source)?;
        pending.try_reserve_exact(requested.len()).map_err(|_| {
            ImportError::new(
                ErrorKind::ResourceLimit,
                fallback,
                "Report type allocation failed",
            )
        })?;
        for ty in requested {
            observation.work(1, &fallback.source)?;
            let bytes = observation.size(ty.identity().len().checked_add(7), &fallback.source)?;
            observation.string(bytes, &fallback.source)?;
            let identity = key(ty);
            if discovered.contains(&identity) {
                continue;
            }
            observation.reserve(128, &fallback.source)?;
            enqueue_type_with_identity(
                ty.clone(),
                identity,
                &mut pending,
                &mut discovered,
                fallback,
            )?;
        }
        Ok(Self {
            pending,
            discovered,
            selections: BTreeMap::new(),
            active: None,
            phase: ReportPreparationPhase::Next,
            member_preparation: None,
            member_name: None,
        })
    }
    fn advance(
        &mut self,
        context: &mut Context<'source, '_>,
        inventory: &mut crate::definitions::generic_resolution::SignatureInventory,
        fallback: &SourceSpan,
    ) -> Result<bool, ImportError> {
        self.advance_with_polling(
            context,
            inventory,
            fallback,
            GenericAgreementPolling::Compatibility,
        )
    }
    fn advance_with_polling(
        &mut self,
        context: &mut Context<'source, '_>,
        inventory: &mut crate::definitions::generic_resolution::SignatureInventory,
        fallback: &SourceSpan,
        polling: GenericAgreementPolling,
    ) -> Result<bool, ImportError> {
        if self.phase == ReportPreparationPhase::Failed {
            return Err(ImportError::new(
                ErrorKind::InvalidDefinition,
                fallback,
                "Report preparation has failed",
            ));
        }
        if self.phase == ReportPreparationPhase::Complete {
            return Ok(true);
        }
        // Check cooperative control separately from old ordered work/storage
        // fees, including transitions whose semantic work was already admitted.
        let result = context
            .observation
            .preparation(&fallback.source, |operation| {
                operation
                    .checkpoint()
                    .map_err(crate::definitions::schema::PreparationError::Observation)
            })
            .and_then(|()| {
                if self.phase == ReportPreparationPhase::Members {
                    self.advance_inner(
                        context,
                        inventory,
                        fallback,
                        polling == GenericAgreementPolling::RetainedGenericSourceDecoding,
                    )
                } else {
                    context
                        .observation
                        .work(0, &fallback.source)
                        .and_then(|()| {
                            self.advance_inner(
                                context,
                                inventory,
                                fallback,
                                polling == GenericAgreementPolling::RetainedGenericSourceDecoding,
                            )
                        })
                }
            })
            .and_then(|mut done| {
                if polling == GenericAgreementPolling::Compatibility {
                    // A compatibility advance corresponds to one ORIGINAL
                    // observed owner macrostep. Drain only its new silent
                    // subdivisions using the very same retained algorithm.
                    // This deliberate public synchronous tail earns no bounded
                    // polling claim. No mandatory old checkpoint is suppressed.
                    while self.is_silent_agreement_step() {
                        done = self.advance_inner(
                            context,
                            inventory,
                            fallback,
                            polling == GenericAgreementPolling::RetainedGenericSourceDecoding,
                        )?;
                    }
                }
                Ok(done)
            });
        if result.is_err() {
            self.phase = ReportPreparationPhase::Failed;
        }
        result
    }
    #[cfg(test)]
    fn is_source_decoding_step(&self) -> bool {
        self.phase == ReportPreparationPhase::Members
            && self
                .member_preparation
                .as_ref()
                .is_some_and(|preparation| preparation.is_source_decoding_step())
    }
    fn is_silent_agreement_step(&self) -> bool {
        self.phase == ReportPreparationPhase::Members
            && self
                .member_preparation
                .as_ref()
                .is_some_and(|preparation| preparation.is_silent_agreement_step())
    }
    fn advance_inner(
        &mut self,
        context: &mut Context<'source, '_>,
        inventory: &mut crate::definitions::generic_resolution::SignatureInventory,
        fallback: &SourceSpan,
        retain_source_decoding: bool,
    ) -> Result<bool, ImportError> {
        let resolver = context.state.resolver;
        let observation = context.observation;
        if self.phase == ReportPreparationPhase::Next {
            let Some(ty) = self.pending.pop() else {
                self.phase = ReportPreparationPhase::Complete;
                return Ok(true);
            };
            let top = crate::definitions::schema::eobject_reference_top_declaration_ref()
                .map_err(|e| ImportError::new(ErrorKind::InvalidDefinition, fallback, e))?;
            if definition_id(ty.expression()) == Some(top) {
                context.state.reference_top = Some(top.clone());
                resolver.extend_pinned_eobject_inventory(inventory)?;
            }
            let name = key(&ty);
            if context.state.types.contains_key(&name) {
                return Ok(false);
            }
            if context.state.types.len() >= 4096 {
                return Err(ImportError::new(
                    ErrorKind::ResourceLimit,
                    fallback,
                    "Materialization type budget exceeded",
                ));
            }
            self.member_name = Some(name);
            self.member_preparation = Some(RuntimeMembersPreparation::begin(
                resolver,
                std::borrow::Cow::Owned(ty),
                false,
            ));
            self.phase = ReportPreparationPhase::Members;
            return Ok(false);
        }
        if self.phase == ReportPreparationPhase::Members {
            let preparation = self.member_preparation.as_mut().ok_or_else(|| {
                ImportError::new(
                    ErrorKind::InvalidDefinition,
                    fallback,
                    "Missing runtime member preparation",
                )
            })?;
            if !preparation.advance_with_source_decoding(observation, retain_source_decoding)? {
                return Ok(false);
            }
            let (report, ty) = self
                .member_preparation
                .take()
                .ok_or_else(|| {
                    ImportError::new(
                        ErrorKind::InvalidDefinition,
                        fallback,
                        "Missing completed runtime member preparation",
                    )
                })?
                .finish_prepared()?;
            let name = self.member_name.take().ok_or_else(|| {
                ImportError::new(
                    ErrorKind::InvalidDefinition,
                    fallback,
                    "Missing runtime member selection name",
                )
            })?;
            self.selections.insert(
                name.clone(),
                (
                    resolver.resolved_runtime_operations(&ty)?,
                    report.declaration_span.clone(),
                ),
            );
            self.active = Some(ActiveReport {
                name,
                report: std::rc::Rc::new(report),
                next_parent: 0,
            });
            self.phase = ReportPreparationPhase::Parents;
            return Ok(false);
        }
        if self.phase == ReportPreparationPhase::Parents {
            let active = self.active.as_mut().ok_or_else(|| {
                ImportError::new(
                    ErrorKind::InvalidDefinition,
                    fallback,
                    "Missing active report",
                )
            })?;
            if let Some(parent) = active.report.parents.get(active.next_parent) {
                let parent =
                    context.runtime_type(parent, &active.report.declaration_span.source, |e| {
                        ImportError::new(ErrorKind::Unsupported, &active.report.declaration_span, e)
                    })?;
                enqueue_type(parent, &mut self.pending, &mut self.discovered, fallback)?;
                active.next_parent += 1;
            } else {
                self.phase = ReportPreparationPhase::Register;
            }
            return Ok(false);
        }
        if self.phase != ReportPreparationPhase::Register {
            return Err(ImportError::new(
                ErrorKind::InvalidDefinition,
                fallback,
                "Invalid report preparation phase",
            ));
        }
        let active = self.active.take().ok_or_else(|| {
            ImportError::new(
                ErrorKind::InvalidDefinition,
                fallback,
                "Missing prepared report",
            )
        })?;
        context.state.types.insert(active.name, active.report);
        self.phase = ReportPreparationPhase::Next;
        Ok(false)
    }
    fn finish(self, fallback: &SourceSpan) -> Result<ReportSelections<'source>, ImportError> {
        if self.phase != ReportPreparationPhase::Complete {
            return Err(ImportError::new(
                ErrorKind::InvalidDefinition,
                fallback,
                "Report preparation is incomplete or failed",
            ));
        }
        Ok(self.selections)
    }
}
struct ClassSelection {
    root: Xml,
    instantiations: BTreeMap<String, RuntimeType>,
    raw_reference_bounds: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    runtime_warnings: Vec<crate::definitions::generic_resolution::Warning>,
    operation_templates: BTreeMap<
        String,
        BTreeMap<DeclarationId, crate::definitions::generic_resolution::SpecializedMember>,
    >,
}
// Current-attempt class lowering retains its complete source reports and output
// package. Source checking/projection use the same routines and fresh observer.
struct ClassPreparation<'source> {
    reports: std::vec::IntoIter<(String, std::rc::Rc<PreparedRuntimeMembers<'source>>)>,
    root: Xml,
    instantiations: BTreeMap<String, RuntimeType>,
    raw_reference_bounds: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    runtime_warnings: Vec<crate::definitions::generic_resolution::Warning>,
    operation_templates: BTreeMap<
        String,
        BTreeMap<DeclarationId, crate::definitions::generic_resolution::SpecializedMember>,
    >,
    complete: bool,
    failed: bool,
}
impl<'source> ClassPreparation<'source> {
    fn new(
        root: Xml,
        reports: Vec<(String, std::rc::Rc<PreparedRuntimeMembers<'source>>)>,
    ) -> Self {
        Self {
            reports: reports.into_iter(),
            root,
            instantiations: BTreeMap::new(),
            raw_reference_bounds: BTreeMap::new(),
            runtime_warnings: Vec::new(),
            operation_templates: BTreeMap::new(),
            complete: false,
            failed: false,
        }
    }
    fn advance(&mut self, context: &mut Context<'source, '_>) -> Result<bool, ImportError> {
        if self.failed {
            return Err(self.root.error("Class preparation has failed"));
        }
        if self.complete {
            return Ok(true);
        }
        let result = context
            .observation
            .work(0, &self.root.opening.source)
            .and_then(|()| self.advance_inner(context));
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner(&mut self, context: &mut Context<'source, '_>) -> Result<bool, ImportError> {
        let Some((name, report)) = self.reports.next() else {
            self.complete = true;
            return Ok(true);
        };
        let observation = context.observation;
        context.state.open_features = report
            .feature_domains
            .iter()
            .filter(|domain| {
                domain.projection.storage_owner.expression() == report.receiver.expression()
            })
            .map(|domain| {
                (
                    domain.projection.source.declaration.clone(),
                    domain.reference,
                )
            })
            .collect();
        let expression = report.receiver.expression();
        let id = definition_id(expression).ok_or_else(|| self.root.error("Missing classifier"))?;
        let mut node = context.xml(id)?;
        node.attrs.insert("name".into(), name.clone());
        let parents = report
            .parents
            .iter()
            .map(|ty| {
                context
                    .runtime_type(ty, &node.opening.source, |e| node.error(e))
                    .map(|ty| format!("#//{}", key(&ty)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        node.attrs.insert("eSuperTypes".into(), parents.join(" "));
        node.children.retain(|n| {
            !matches!(
                n.name.as_str(),
                "eTypeParameters" | "eGenericSuperTypes" | "eSuperTypes"
            )
        });
        let mut signatures = Vec::new();
        let mut templates = BTreeMap::new();
        for member in report
            .members
            .iter()
            .filter(|member| member.owner.value() == expression)
        {
            if observation
                .preparation(&node.opening.source, |context| {
                    member.requires_schema_template_observed(context)
                })
                .map_err(|e| node.error(e.message))?
            {
                templates.insert(
                    member.declaration.value().clone(),
                    member.owned_copy_observed(observation)?,
                );
                continue;
            }
            signatures.extend(
                member
                    .signatures
                    .owned_copy_observed(&node.opening.source, observation)?,
            );
        }
        self.runtime_warnings.extend(report.warnings.clone());
        let mut constraints = BTreeMap::new();
        for member in &report.members {
            for bound in report
                .reference_bounds
                .iter()
                .filter(|b| member.declaration.value() == b.feature.value())
            {
                let targets = bound
                    .bounds
                    .iter()
                    .map(|ty| {
                        context
                            .target(ty.value(), &node)
                            .map(|key| key.trim_start_matches("#//").to_owned())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if !targets.is_empty() {
                    constraints.insert(member.name.to_string(), targets);
                }
            }
        }
        if !constraints.is_empty() {
            self.raw_reference_bounds.insert(name.clone(), constraints);
        }
        context.project(&mut node, &signatures, &templates)?;
        self.operation_templates.insert(name.clone(), templates);
        context
            .state
            .declarations
            .insert(name.clone(), context.provenance(id)?);
        if matches!(expression,Type::Classifier {arguments,raw,..} if *raw || !arguments.is_empty())
        {
            self.instantiations.insert(name, report.receiver.clone());
        }
        self.root.children.push(node);
        Ok(false)
    }
    fn finish(self) -> Result<ClassSelection, ImportError> {
        if self.failed || !self.complete {
            return Err(self.root.error("Class preparation is incomplete or failed"));
        }
        Ok(ClassSelection {
            root: self.root,
            instantiations: self.instantiations,
            raw_reference_bounds: self.raw_reference_bounds,
            runtime_warnings: self.runtime_warnings,
            operation_templates: self.operation_templates,
        })
    }
}
struct OwnedClassInputs<'source> {
    selections: ReportSelections<'source>,
    selection: FeatureDomainSelection<crate::definitions::schema::SharedRuntimeSourceArrayTarget>,
}
#[path = "publication_validation.rs"]
mod publication_validation;
struct PendingRevisionPublication<'source> {
    assembly: Option<
        ClassedPublicationPreparation<
            'source,
            crate::definitions::schema::SharedRuntimeSourceArrayTarget,
        >,
    >,
    inputs: Option<
        ClassedPublication<'source, crate::definitions::schema::SharedRuntimeSourceArrayTarget>,
    >,
    tail: Option<Box<publication_validation::PublicationTail<'source>>>,
    digest: Option<Box<resource_compiler::MapRevisionPreparation>>,
    metadata_digest: Option<resource_compiler::MetadataRevisionPreparation>,
    metadata_revision_done: bool,
    revision_done: bool,
}
const PENDING_PUBLICATION_BYTES: usize = 4096;
const _: () = assert!(
    std::mem::size_of::<PendingRevisionPublication<'static>>() <= PENDING_PUBLICATION_BYTES
);
// Reports, feature preparation and class preparation are mutually exclusive.
// Reuse one admitted inline slot instead of increasing the frozen holder fee.
// The admitted owner envelope covers this inline phase slot; boxing would add a new allocation and change the frozen fee contract.
#[allow(clippy::large_enum_variant)]
enum AssemblyProgress<'a> {
    Empty,
    Reports(ReportPreparation<'a>),
    Features(FeaturePreparationInputs<'a>),
    Classes {
        preparation: Option<ClassPreparation<'a>>,
        inputs: Option<OwnedClassInputs<'a>>,
    },
}
struct MaterializationContext<'a> {
    source_inventory:
        Option<Box<crate::definitions::generic_resolution::SourceInventoryPreparation<'a>>>,
    reuse_compiler_type_inputs: bool,
    pending_revision: Option<Box<PendingRevisionPublication<'a>>>,
    progress: AssemblyProgress<'a>,
    readonly_xml: std::cell::RefCell<BTreeMap<DeclarationId, std::rc::Rc<Xml>>>,
    open_features: BTreeMap<DeclarationId, bool>,
    reference_top: Option<DeclarationId>,
    bundle: &'a DefinitionBundle,
    resolver: &'a Resolver,
    resources: &'a ResourceSet,
    standard: &'a ResourceSet,
    standard_id: &'a crate::definitions::bundle::SourceId,
    types: BTreeMap<String, std::rc::Rc<PreparedRuntimeMembers<'a>>>,
    enums: BTreeMap<String, crate::definitions::enums::Enumeration>,
    datatypes: BTreeMap<String, crate::definitions::datatypes::Datatype>,
    declarations: BTreeMap<String, ClassifierDeclaration>,
}
impl<'a> MaterializationContext<'a> {
    // Shared immutable phase inspection for creation-fixed polling and tests.
    // Borrowing the admitted progress slot allocates and copies no input data.
    fn reports(&self) -> Option<&ReportPreparation<'a>> {
        match &self.progress {
            AssemblyProgress::Reports(value) => Some(value),
            _ => None,
        }
    }
    #[cfg(test)]
    fn features(&self) -> Option<&FeaturePreparationInputs<'a>> {
        match &self.progress {
            AssemblyProgress::Features(value) => Some(value),
            _ => None,
        }
    }
    #[cfg(test)]
    fn class(&self) -> Option<&ClassPreparation<'a>> {
        match &self.progress {
            AssemblyProgress::Classes { preparation, .. } => preparation.as_ref(),
            _ => None,
        }
    }
    #[cfg(test)]
    fn class_inputs(&self) -> Option<&OwnedClassInputs<'a>> {
        match &self.progress {
            AssemblyProgress::Classes { inputs, .. } => inputs.as_ref(),
            _ => None,
        }
    }

    fn take_reports(&mut self) -> Option<ReportPreparation<'a>> {
        match std::mem::replace(&mut self.progress, AssemblyProgress::Empty) {
            AssemblyProgress::Reports(value) => Some(value),
            other => {
                self.progress = other;
                None
            }
        }
    }
    fn set_reports(&mut self, value: ReportPreparation<'a>) {
        self.progress = AssemblyProgress::Reports(value);
    }
    fn has_features(&self) -> bool {
        matches!(self.progress, AssemblyProgress::Features(_))
    }
    fn take_features(&mut self) -> Option<FeaturePreparationInputs<'a>> {
        match std::mem::replace(&mut self.progress, AssemblyProgress::Empty) {
            AssemblyProgress::Features(value) => Some(value),
            other => {
                self.progress = other;
                None
            }
        }
    }
    fn set_features(&mut self, value: FeaturePreparationInputs<'a>) {
        self.progress = AssemblyProgress::Features(value);
    }
    fn has_class(&self) -> bool {
        matches!(
            self.progress,
            AssemblyProgress::Classes {
                preparation: Some(_),
                ..
            }
        )
    }
    fn take_class(&mut self) -> Option<ClassPreparation<'a>> {
        match &mut self.progress {
            AssemblyProgress::Classes { preparation, .. } => preparation.take(),
            _ => None,
        }
    }
    fn set_class(&mut self, value: ClassPreparation<'a>) {
        match &mut self.progress {
            AssemblyProgress::Classes { preparation, .. } => *preparation = Some(value),
            _ => {
                self.progress = AssemblyProgress::Classes {
                    preparation: Some(value),
                    inputs: None,
                }
            }
        }
    }
    fn take_class_inputs(&mut self) -> Option<OwnedClassInputs<'a>> {
        match &mut self.progress {
            AssemblyProgress::Classes { inputs, .. } => inputs.take(),
            _ => None,
        }
    }
    fn set_class_inputs(&mut self, value: OwnedClassInputs<'a>) {
        match &mut self.progress {
            AssemblyProgress::Classes { inputs, .. } => *inputs = Some(value),
            _ => {
                self.progress = AssemblyProgress::Classes {
                    preparation: None,
                    inputs: Some(value),
                }
            }
        }
    }
}
// Invocation observers are borrowed only by this short-lived operation view.
// Persistent source state never captures cancellation/control or host resources.
struct Context<'source, 'operation> {
    state: &'operation mut MaterializationContext<'source>,
    observation: &'operation crate::definitions::ecore::ParserObservation<'operation, 'operation>,
}
impl Context<'_, '_> {
    // Shared descriptors are immutable compiler input, never membership proof.
    // Keep each caller's diagnostic and a fresh shape/observer check on every use.
    fn runtime_type(
        &self,
        expression: &Type,
        source: &crate::definitions::bundle::SourceId,
        schema_error: impl FnOnce(String) -> ImportError,
    ) -> Result<RuntimeType, ImportError> {
        if !self.state.reuse_compiler_type_inputs {
            return runtime_type(expression, source, self.observation, schema_error);
        }
        self.observation
            .preparation(source, |operation| {
                match self.state.resolver.runtime_input_prepared_observed(
                    std::borrow::Cow::Borrowed(expression),
                    operation,
                ) {
                    Ok(value) => Ok(Ok(value)),
                    Err(crate::definitions::schema::PreparationError::Schema(message)) => {
                        Ok(Err(message))
                    }
                    Err(error @ crate::definitions::schema::PreparationError::Observation(_)) => {
                        Err(error)
                    }
                }
            })?
            .map_err(schema_error)
    }
    fn xml(&self, id: &DeclarationId) -> Result<Xml, ImportError> {
        self.xml_selection(id, false)
    }
    // Full immutable source projection. Generated mutable trees still use xml().
    fn xml_read(&self, id: &DeclarationId) -> Result<std::rc::Rc<Xml>, ImportError> {
        self.observation.work(1, &id.source)?;
        {
            let views = self.state.readonly_xml.try_borrow().map_err(|_| {
                ImportError::new(
                    ErrorKind::InvalidDefinition,
                    &SourceSpan {
                        source: id.source.clone(),
                        start: 0,
                        end: 0,
                    },
                    "Read-only XML projection is already borrowed",
                )
            })?;
            if let Some(view) = views.get(id) {
                return Ok(std::rc::Rc::clone(view));
            }
        }
        let retain = self
            .state
            .readonly_xml
            .try_borrow()
            .map_err(|_| {
                ImportError::new(
                    ErrorKind::InvalidDefinition,
                    &SourceSpan {
                        source: id.source.clone(),
                        start: 0,
                        end: 0,
                    },
                    "Read-only XML projection is already borrowed",
                )
            })?
            .len()
            < 4096;
        if retain {
            self.observation.serialized(id, &id.source)?;
            self.observation.reserve(128, &id.source)?;
        }
        self.observation.reserve(32, &id.source)?;
        let view = std::rc::Rc::new(self.xml(id)?);
        if retain {
            self.state
                .readonly_xml
                .try_borrow_mut()
                .map_err(|_| {
                    ImportError::new(
                        ErrorKind::InvalidDefinition,
                        &SourceSpan {
                            source: id.source.clone(),
                            start: 0,
                            end: 0,
                        },
                        "Read-only XML projection is already borrowed",
                    )
                })?
                .insert(id.clone(), std::rc::Rc::clone(&view));
        }
        Ok(view)
    }
    fn xml_header(&self, id: &DeclarationId) -> Result<Xml, ImportError> {
        self.xml_selection(id, true)
    }
    fn xml_selection(&self, id: &DeclarationId, header: bool) -> Result<Xml, ImportError> {
        let set = if id.source == *self.state.standard_id {
            &self.state.standard
        } else {
            &self.state.resources
        };
        let span = SourceSpan {
            source: id.source.clone(),
            start: 0,
            end: 0,
        };
        let node = set
            .node(id)
            .map_err(|e| ImportError::new(ErrorKind::InvalidDefinition, &span, e))?;
        let doc = set
            .document(&id.source)
            .map_err(|e| ImportError::new(ErrorKind::InvalidDefinition, &span, e))?;
        self.observation.serialized(&id.path, &id.source)?;
        if header {
            Xml::header_from_node_at_observed(
                node,
                &doc.source.text,
                id.path.clone(),
                self.observation,
            )
        } else {
            Xml::from_node_at_observed(node, &doc.source.text, id.path.clone(), self.observation)
        }
    }
    fn provenance(&self, id: &DeclarationId) -> Result<ClassifierDeclaration, ImportError> {
        let node = self.xml_read(id)?;
        let mut parent = id.clone();
        parent.path.pop();
        let package = self.xml_header(&parent)?;
        Ok(ClassifierDeclaration {
            source: id.source.clone(),
            path: id.path.clone(),
            name: required(&node, "name")?,
            namespace: required(&package, "nsURI")?,
        })
    }
    fn target(&mut self, ty: &Type, at: &Xml) -> Result<String, ImportError> {
        let closed = self
            .observation
            .preparation(&at.opening.source, |context| {
                match self
                    .state
                    .resolver
                    .runtime_input_prepared_observed(std::borrow::Cow::Borrowed(ty), context)
                {
                    Ok(value) => Ok(Ok(value)),
                    Err(crate::definitions::schema::PreparationError::Schema(message)) => {
                        Ok(Err(message))
                    }
                    Err(error @ crate::definitions::schema::PreparationError::Observation(_)) => {
                        Err(error)
                    }
                }
            })?
            .map_err(|message| at.unsupported(message))?;
        let id = definition_id(ty).ok_or_else(|| at.error("Expected closed classifier"))?;
        let key_bytes = closed
            .identity()
            .len()
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(7))
            .ok_or_else(|| at.error("Materialized key storage overflow"))?;
        self.observation.string(key_bytes, &at.opening.source)?;
        let local = key(&closed);
        let shared = if self.state.types.contains_key(&local) {
            Some(self.xml_read(id)?)
        } else {
            None
        };
        let mut owned;
        let node = if let Some(view) = &shared {
            view.as_ref()
        } else {
            owned = self.xml(id)?;
            owned.attrs.remove("xmi:id");
            &owned
        };
        let name = required(node, "name")?;
        let kind = required(node, "xsi:type")?;
        if kind == "ecore:EClass" {
            let top = crate::definitions::schema::eobject_reference_top_declaration()
                .map_err(|e| at.error(e))?;
            if *id == top {
                self.state.reference_top = Some(top);
                if self.state.types.contains_key(&local) {
                    return Ok(format!("#//{local}"));
                }
                return Ok(format!(
                    "#//{}",
                    crate::definitions::schema::EOBJECT_REFERENCE_TOP
                ));
            }
            let target = local;
            if !self.state.types.contains_key(&target) {
                return Err(at.unsupported(format!(
                    "Explicit materialization set is missing reference/operation target {}",
                    closed.identity()
                )));
            }
            return Ok(format!("#//{target}"));
        }
        if let Type::Classifier { arguments, .. } = ty
            && !arguments.is_empty()
        {
            let checked = self.state.resolver.instantiate_datatype(
                &ClosedType::new(closed.expression().clone()).map_err(|e| at.unsupported(e))?,
            )?;
            let mut bundle = self.state.bundle.clone();
            if id.source == *self.state.standard_id
                && !bundle
                    .sources
                    .iter()
                    .any(|source| source.id == *self.state.standard_id)
            {
                bundle.sources.push(standard_datatype_source());
            }
            let datatype = import_datatype_bundle(&bundle, id)?
                .specialize(&checked)
                .map_err(|e| at.error(e))?;
            let target = local;
            self.state
                .declarations
                .insert(target.clone(), self.provenance(id)?);
            self.state.datatypes.insert(target.clone(), datatype);
            return Ok(format!("#//{target}"));
        }
        if id.source == *self.state.standard_id {
            return Ok(format!(
                "ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//{name}"
            ));
        }
        let target = local;
        self.state
            .declarations
            .insert(target.clone(), self.provenance(id)?);
        if kind == "ecore:EEnum" {
            self.state.enums.insert(
                target.clone(),
                lower_enumeration_observed(node, Some(self.observation))?,
            );
        } else if kind == "ecore:EDataType" {
            self.state.datatypes.insert(
                target.clone(),
                lower_datatype_observed(node, Some(self.observation))?,
            );
        } else {
            return Err(at.error("Unsupported materialized classifier kind"));
        }
        Ok(format!("#//{target}"))
    }
    fn project(
        &mut self,
        node: &mut Xml,
        signatures: &[SpecializedSignature],
        templates: &BTreeMap<
            DeclarationId,
            crate::definitions::generic_resolution::SpecializedMember,
        >,
    ) -> Result<(), ImportError> {
        if node.name == "eAnnotations" {
            return Ok(());
        }
        if templates.contains_key(&node.declaration) {
            return prepare_template(node);
        }
        for signature in signatures.iter().filter(|s| s.owner == node.declaration) {
            let mut targets = Vec::new();
            if let Some(reference) = self.state.open_features.get(&node.declaration) {
                if *reference {
                    for ty in &signature.specialized {
                        let erased =
                            self.runtime_type(ty, &node.opening.source, |e| node.unsupported(e))?;
                        let declaration = definition_id(ty)
                            .ok_or_else(|| node.error("Expected reference classifier"))?;
                        let source = self.xml_read(declaration)?;
                        if required(&source, "xsi:type")? != "ecore:EClass" {
                            return Err(node.error("Reference domain erasure must be a class"));
                        }
                        let top = crate::definitions::schema::eobject_reference_top_declaration()
                            .map_err(|e| node.error(e))?;
                        if self.state.types.contains_key(&key(&erased)) || *declaration == top {
                            targets.push(self.target(ty, node)?);
                        } else {
                            // The checked domain retains its exact erasure identity;
                            // only this structural target falls back for an omitted class.
                            self.state.reference_top = Some(top);
                            targets.push(format!(
                                "#//{}",
                                crate::definitions::schema::EOBJECT_REFERENCE_TOP
                            ));
                        }
                    }
                } else {
                    targets.push("#//__runtime_open_attribute".to_owned());
                }
            } else {
                for ty in &signature.specialized {
                    targets.push(self.target(ty, node)?);
                }
            }
            if targets.is_empty() {
                node.attrs.remove(&signature.feature);
            } else {
                node.attrs
                    .insert(signature.feature.clone(), targets.join(" "));
            }
            node.children.retain(|child| {
                child.name != signature.feature
                    && !matches!(child.name.as_str(), "eGenericType" | "eGenericExceptions")
            });
        }
        node.attrs.remove("xmi:id");
        if matches!(node.name.as_str(), "eOperations" | "eParameters")
            && let Some(kind) = node.attrs.get("xsi:type")
        {
            let expected = if node.name == "eOperations" {
                "ecore:EOperation"
            } else {
                "ecore:EParameter"
            };
            if kind != expected {
                return Err(node.error("Invalid operation/parameter kind"));
            }
            node.attrs.remove("xsi:type");
        }
        resource_compiler::reference_children(node, &["eOpposite"], self.observation)?;
        if let Some(href) = node.attrs.get("eOpposite").cloned() {
            let (declaration, opposite) = resource_compiler::resolve_opposite(
                self.state.resources,
                &node.declaration.source,
                node,
                &href,
                self.observation,
            )?;
            let target = required(node, "eType")?;
            let target = target
                .strip_prefix("#//")
                .and_then(|key| self.state.types.get(key))
                .ok_or_else(|| node.error("Opposite requires a materialized reference target"))?;
            let mut candidates = target
                .members
                .iter()
                .filter(|member| member.declaration == declaration);
            let member = candidates.next().ok_or_else(|| {
                node.error("Opposite is not a member of the specialized reference target")
            })?;
            if candidates.next().is_some() {
                return Err(node.error("Ambiguous specialized opposite declaration"));
            }
            let owner =
                self.runtime_type(&member.owner, &node.opening.source, |e| node.unsupported(e))?;
            node.attrs.insert(
                "eOpposite".into(),
                format!("#//{}/{}", key(&owner), required(&opposite, "name")?),
            );
        }
        for child in &mut node.children {
            self.project(child, signatures, templates)?;
        }
        Ok(())
    }
}
// Candidate discovery never requires an executable codec for unrelated source metadata.
pub(super) fn retain_attribute_candidate(
    candidates: &mut BTreeMap<String, (RuntimeType, bool)>,
    ty: RuntimeType,
    required: bool,
    source: &crate::definitions::bundle::SourceId,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    observation.work(1, source)?;
    if let Some(entry) = candidates.get_mut(ty.identity()) {
        entry.1 |= required;
    } else {
        observation.reserve(128, source)?;
        observation.string(ty.identity().len(), source)?;
        candidates.insert(ty.identity().to_owned(), (ty, required));
    }
    Ok(())
}
pub(super) fn publish_attribute_candidates(
    candidates: BTreeMap<String, (RuntimeType, bool)>,
    source: &crate::definitions::bundle::SourceId,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<Vec<(RuntimeType, bool)>, ImportError> {
    let mut result = Vec::new();
    for candidate in candidates.into_values() {
        observation.work(1, source)?;
        observation.reserve(32, source)?;
        result.push(candidate);
    }
    Ok(result)
}
fn runtime_attribute_candidates(
    context: &Context<'_, '_>,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    domains: &[crate::definitions::generic_resolution::RuntimeDomainRef<'_>],
) -> Result<Vec<(RuntimeType, bool)>, ImportError> {
    let mut candidates = BTreeMap::new();
    let mut add = |ty: RuntimeType, required: bool| -> Result<(), ImportError> {
        retain_attribute_candidate(
            &mut candidates,
            ty,
            required,
            context.state.standard_id,
            context.observation,
        )
    };
    let mut only_references = true;
    for domain in domains {
        context.observation.work(1, context.state.standard_id)?;
        if !domain.reference {
            only_references = false;
            break;
        }
    }
    if only_references {
        return Ok(vec![]);
    }
    for source in &context.state.bundle.sources {
        context.observation.work(1, &source.id)?;
        if !matches!(source.kind, SourceKind::Ecore | SourceKind::Rcore) {
            continue;
        }
        let document = context.state.resources.document(&source.id).map_err(|e| {
            ImportError::new(
                ErrorKind::InvalidDefinition,
                &SourceSpan {
                    source: source.id.clone(),
                    start: 0,
                    end: 0,
                },
                e,
            )
        })?;
        context.observation.reserve(32, &source.id)?;
        let mut pending = vec![Xml::from_node_at_observed(
            &document.root,
            &source.text,
            vec![],
            context.observation,
        )?];
        while let Some(node) = pending.pop() {
            context.observation.work(1, &source.id)?;
            if node.name == "eClassifiers" && !has_formals(&node, context.observation)? {
                let native = match node.attrs.get("xsi:type").map(String::as_str) {
                    Some("ecore:EDataType") => {
                        lower_datatype_observed(&node, Some(context.observation))?
                            .codec()
                            .is_ok()
                    }
                    Some("ecore:EEnum") => true,
                    _ => false,
                };
                if native {
                    add(
                        classifier_type(&node.declaration, false, &node, context.observation)?,
                        false,
                    )?;
                }
            }
            for child in node.children {
                context.observation.work(1, &source.id)?;
                if matches!(child.name.as_str(), "eSubpackages" | "eClassifiers") {
                    context.observation.reserve(32, &source.id)?;
                    pending.push(child);
                }
            }
        }
    }
    for domain in domains {
        context.observation.work(1, context.state.standard_id)?;
        if domain.reference
            || context
                .observation
                .preparation(context.state.standard_id, |observation| {
                    domain.erasure.has_raw_observed(observation)
                })?
        {
            continue;
        }
        let erasure = context
            .observation
            .preparation(context.state.standard_id, |observation| {
                domain.erasure.retained_clone_observed(observation)
            })?;
        add(erasure, true)?;
    }
    if let Some(registry) = conversions {
        for contract in registry.registered_contracts().map_err(|e| {
            ImportError::new(
                ErrorKind::InvalidDefinition,
                &SourceSpan {
                    source: (*context.state.standard_id).clone(),
                    start: 0,
                    end: 0,
                },
                e,
            )
        })? {
            let node = context.xml(&contract.datatype)?;
            add(
                runtime_type_owned(
                    Type::Classifier {
                        declaration: contract.datatype,
                        arguments: vec![],
                        raw: has_formals(&node, context.observation)?,
                    },
                    &node.opening.source,
                    context.observation,
                    |e| node.error(e),
                )?,
                true,
            )?;
        }
        for contract in registry.applied().registered_contracts().map_err(|e| {
            ImportError::new(
                ErrorKind::InvalidDefinition,
                &SourceSpan {
                    source: (*context.state.standard_id).clone(),
                    start: 0,
                    end: 0,
                },
                e,
            )
        })? {
            add(RuntimeType::from(contract.application), true)?;
        }
    }
    publish_attribute_candidates(candidates, context.state.standard_id, context.observation)
}
fn runtime_attribute_target(
    context: &mut Context<'_, '_>,
    ty: &RuntimeType,
    field: &Xml,
) -> Result<String, ImportError> {
    let target = context.target(ty.expression(), field)?;
    if local_type(&target).is_ok() {
        return Ok(target);
    }
    let id = definition_id(ty.expression())
        .ok_or_else(|| field.error("Missing attribute candidate declaration"))?;
    let datatype = lower_datatype_observed(&context.xml(id)?, Some(context.observation))?;
    let target = key(ty);
    context
        .state
        .declarations
        .insert(target.clone(), context.provenance(id)?);
    context.state.datatypes.insert(target.clone(), datatype);
    Ok(format!("#//{target}"))
}
fn runtime_attribute_default(
    context: &mut Context<'_, '_>,
    domain: crate::definitions::generic_resolution::RuntimeDomainRef<'_>,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
) -> Result<(Value, Option<String>), ImportError> {
    let mut field = context.xml(domain.projection.source.declaration)?;
    if context
        .observation
        .preparation(&field.opening.source, |observation| {
            domain.erasure.has_raw_observed(observation)
        })?
    {
        if field.attrs.contains_key("defaultValueLiteral") {
            return Err(field.unsupported("Open generic lexical default requires a source-established exact application codec"));
        }
        return Ok((
            if field
                .attrs
                .get("upperBound")
                .is_none_or(|upper| upper == "1")
            {
                Value::Null
            } else {
                Value::Array(vec![])
            },
            None,
        ));
    }
    let target = runtime_attribute_target(context, domain.erasure, &field)?;
    field.attrs.insert("eType".into(), target.clone());
    field
        .children
        .retain(|child| !matches!(child.name.as_str(), "eGenericType" | "eType"));
    let lowered = lower_feature(
        &field,
        &context.state.enums,
        &context.state.datatypes,
        conversions,
        None,
        Some(context.observation),
    )?;
    Ok((
        lowered.default,
        Some(local_type(&target).map_err(|e| field.error(e))?),
    ))
}

pub(super) fn prepare_template(node: &mut Xml) -> Result<(), ImportError> {
    node.attrs.remove("xmi:id");
    if let Some(kind) = node.attrs.remove("xsi:type") {
        let expected = if node.name == "eOperations" {
            "ecore:EOperation"
        } else {
            "ecore:EParameter"
        };
        if kind != expected {
            return Err(node.error("Invalid operation/parameter kind"));
        }
    }
    node.attrs.remove("eType");
    let operation = node.name == "eOperations";
    if operation {
        node.attrs.remove("eExceptions");
    }
    node.children.retain(|child| match child.name.as_str() {
        "eType" | "eGenericType" => false,
        "eTypeParameters" | "eGenericExceptions" | "eExceptions" => !operation,
        _ => true,
    });
    for child in &mut node.children {
        if child.name == "eParameters" {
            prepare_template(child)?;
        }
    }
    Ok(())
}

/// Materialize an explicit closed set plus its ancestors. Missing target types
/// reject; this does not eagerly expand an unbounded universe of applications.
pub fn materialize_closed_types(
    bundle: &DefinitionBundle,
    requested: &[ClosedType],
) -> Result<Schema, ImportError> {
    materialize_context(
        bundle,
        &requested.iter().map(RuntimeType::from).collect::<Vec<_>>(),
        None,
    )
}
/// Materialize closed applications using explicitly supplied datatype providers.
pub fn materialize_closed_types_with_conversions(
    bundle: &DefinitionBundle,
    requested: &[ClosedType],
    conversions: &crate::definitions::conversion::ConversionRegistry<'_>,
) -> Result<Schema, ImportError> {
    materialize_context(
        bundle,
        &requested.iter().map(RuntimeType::from).collect::<Vec<_>>(),
        Some(conversions),
    )
}
/// Materialize explicit closed or raw runtime classifier identities.
pub fn materialize_runtime_types(
    bundle: &DefinitionBundle,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
) -> Result<Schema, ImportError> {
    materialize_context(bundle, requested, conversions)
}

/// Test control for draining the owned schema cursor over admitted inputs.
#[cfg(test)]
fn materialize_runtime_types_from_resources(
    bundle: &DefinitionBundle,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    resources: ResourceSet,
) -> Result<Schema, ImportError> {
    let operation = crate::definitions::conversion::InvocationContext::default();
    let observation = crate::definitions::ecore::ParserObservation::new(&operation);
    observation.imported(standard_datatype_source_id(), |context| {
        let mut preparation =
            OwnedSchemaPreparation::begin(bundle, requested, conversions, resources, context)?;
        while !preparation.advance(conversions, context)? {}
        preparation.finish()
    })
}
fn materialize_context(
    bundle: &DefinitionBundle,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
) -> Result<Schema, ImportError> {
    materialize_context_observed(
        bundle,
        requested,
        conversions,
        &crate::definitions::ecore::ParserObservation::new(
            &crate::definitions::conversion::InvocationContext::default(),
        ),
    )
}
fn materialize_context_observed(
    bundle: &DefinitionBundle,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<Schema, ImportError> {
    materialize_context_with_inputs_observed(bundle, requested, conversions, None, observation)
}
fn materialize_context_with_inputs_observed(
    bundle: &DefinitionBundle,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    parsed: Option<ResourceSet>,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<Schema, ImportError> {
    materialize_context_with_storage_observed(
        bundle,
        requested,
        conversions,
        parsed,
        &mut OwnedAlternatives,
        observation,
    )
}
fn materialize_context_with_storage_observed<P: AlternativeStorage>(
    bundle: &DefinitionBundle,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    parsed: Option<ResourceSet>,
    storage: &mut P,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<crate::definitions::schema::SchemaRepresentation<P::Target>, ImportError> {
    let InitializedMaterialization {
        resolver,
        inventory,
        fallback,
    } = initialize_materialization::<P>(bundle, requested, conversions, parsed, observation)?;
    let mut state = new_materialization_context(bundle, &resolver, observation)?;
    materialize_prepared_context(
        &mut state,
        inventory,
        requested,
        conversions,
        storage,
        &fallback,
        observation,
    )
}
struct InitializedMaterialization {
    resolver: Resolver,
    inventory: crate::definitions::generic_resolution::SignatureInventory,
    fallback: SourceSpan,
}
fn initialize_materialization<P: AlternativeStorage>(
    bundle: &DefinitionBundle,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    parsed: Option<ResourceSet>,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<InitializedMaterialization, ImportError> {
    let (mut resolver, fallback) = initialize_materialization_resolver::<P>(
        bundle,
        requested,
        conversions,
        parsed,
        observation,
    )?;
    let inventory = resolver.resolve_materialization_inventory_observed(observation)?;
    Ok(InitializedMaterialization {
        resolver,
        inventory,
        fallback,
    })
}
fn initialize_materialization_resolver<P: AlternativeStorage>(
    bundle: &DefinitionBundle,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    parsed: Option<ResourceSet>,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<(Resolver, SourceSpan), ImportError> {
    resource_compiler::require_canonical_sources(bundle)?;
    let standard_source = standard_datatype_source_observed(observation)?;
    let fallback = observation.span(&standard_source.id, 0, 0)?;
    check_requested_materialization_types(requested, &fallback)?;
    let resolver = if P::BORROW_OWNER_PATHS {
        Resolver::with_standard_source_and_borrowed_cursors(
            bundle,
            standard_source,
            parsed,
            observation,
        )?
    } else {
        Resolver::with_standard_source_and_inputs(bundle, standard_source, parsed, observation)?
    };
    Ok((
        bind_materialization_resolver::<P>(resolver, conversions)?,
        fallback,
    ))
}
fn check_requested_materialization_types(
    requested: &[RuntimeType],
    fallback: &SourceSpan,
) -> Result<(), ImportError> {
    if requested.is_empty() || requested.len() > 1024 {
        return Err(ImportError::new(
            ErrorKind::ResourceLimit,
            fallback,
            "Materialization requires 1..1024 explicit types",
        ));
    }
    Ok(())
}
fn bind_materialization_resolver<P: AlternativeStorage>(
    resolver: Resolver,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
) -> Result<Resolver, ImportError> {
    let mut resolver = match conversions {
        Some(registry) => resolver.with_conversions(registry)?,
        None => resolver,
    };
    if P::BORROW_OWNER_PATHS {
        resolver = resolver.with_borrowed_owner_inputs();
    }
    Ok(resolver)
}
fn new_materialization_context<'source>(
    bundle: &'source DefinitionBundle,
    resolver: &'source Resolver,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<MaterializationContext<'source>, ImportError> {
    let (resources, standard, standard_source_id) = resolver.resource_sets();
    observation.work(0, standard_source_id)?;
    Ok(MaterializationContext {
        source_inventory: None,
        reuse_compiler_type_inputs: false,
        pending_revision: None,
        progress: AssemblyProgress::Empty,
        readonly_xml: Default::default(),
        open_features: BTreeMap::new(),
        reference_top: None,
        bundle,
        resolver,
        resources,
        standard_id: standard_source_id,
        standard,
        types: BTreeMap::new(),
        enums: BTreeMap::new(),
        datatypes: BTreeMap::new(),
        declarations: BTreeMap::new(),
    })
}
fn materialize_prepared_context<P: AlternativeStorage>(
    state: &mut MaterializationContext<'_>,
    inventory: crate::definitions::generic_resolution::SignatureInventory,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    storage: &mut P,
    fallback: &SourceSpan,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<crate::definitions::schema::SchemaRepresentation<P::Target>, ImportError> {
    let mut preparation = PreparedMaterialization::begin(state, inventory, requested, fallback)?;
    while !preparation.advance(state, storage, conversions, fallback, observation)? {}
    preparation.finish(fallback)
}

struct FeaturePreparationInputs<'source> {
    nominal: Option<Box<NominalReferenceIndex<'source>>>,
    root: Xml,
    reports: Vec<(String, std::rc::Rc<PreparedRuntimeMembers<'source>>)>,
    candidates: Vec<(RuntimeType, bool)>,
    selections: ReportSelections<'source>,
}
fn prepare_feature_inputs<'source>(
    state: &mut MaterializationContext<'source>,
    selections: ReportSelections<'source>,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    fallback: &SourceSpan,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    use_nominal_bitmaps: bool,
) -> Result<FeaturePreparationInputs<'source>, ImportError> {
    let context = Context { state, observation };
    let first = context
        .state
        .types
        .values()
        .next()
        .and_then(|report| definition_id(report.receiver.expression()))
        .ok_or_else(|| {
            ImportError::new(
                ErrorKind::InvalidDefinition,
                fallback,
                "Missing requested classifier",
            )
        })?;
    let mut package = first.clone();
    package.path.pop();
    let mut root = context.xml_header(&package)?;
    root.name = "ecore:EPackage".into();
    root.attrs.remove("xmi:id");
    root.attrs.remove("xsi:type");
    attrs(&root, &["name", "nsURI", "nsPrefix"])?;
    let reports: Vec<_> = context
        .state
        .types
        .iter()
        .map(|(key, report)| (key.clone(), report.clone()))
        .collect();
    let all_domains: Vec<_> = reports
        .iter()
        .flat_map(|(_, report)| report.feature_domains.iter())
        .collect();
    let candidates = runtime_attribute_candidates(&context, conversions, &all_domains)?;
    for (name, report) in &reports {
        RuntimeCandidate::reference(name, report, &context)?;
    }
    drop(all_domains);
    let nominal = if use_nominal_bitmaps {
        NominalReferenceIndex::from_reports(&reports, &root, observation)?
    } else {
        None
    };
    Ok(FeaturePreparationInputs {
        nominal,
        root,
        reports,
        candidates,
        selections,
    })
}
struct PreparedClasses<'source, A: crate::definitions::schema::RuntimeAlternative> {
    selections: ReportSelections<'source>,
    selection: FeatureDomainSelection<A>,
    classes: ClassSelection,
}
// Shared publication tail. Class preparation is consumed exactly once, whether
// driven synchronously or by the owned public lifecycle.
struct ClassedPublication<'source, A: crate::definitions::schema::RuntimeAlternative> {
    schema: crate::definitions::schema::SchemaRepresentation<A>,
    inventory: crate::definitions::generic_resolution::SignatureInventory,
    selections: ReportSelections<'source>,
    root: Xml,
}
fn materialize_classed_context<'source, P: AlternativeStorage>(
    state: &mut MaterializationContext<'source>,
    inventory: crate::definitions::generic_resolution::SignatureInventory,
    prepared: PreparedClasses<'source, P::Target>,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    storage: &mut P,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    buffered_revision: bool,
) -> Result<crate::definitions::schema::SchemaRepresentation<P::Target>, ImportError> {
    let publication = prepare_classed_publication(
        state,
        inventory,
        prepared,
        conversions,
        storage,
        observation,
    )?;
    let publication = hash_runtime_feature_domains(publication, buffered_revision, observation)?;
    finish_classed_publication(publication, conversions, observation)
}
fn hash_runtime_feature_domains<'source, A: crate::definitions::schema::RuntimeAlternative>(
    publication: ClassedPublication<'source, A>,
    buffered_revision: bool,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<ClassedPublication<'source, A>, ImportError> {
    let ClassedPublication {
        mut schema,
        inventory,
        selections,
        root,
    } = publication;
    if !schema.runtime_feature_domains.is_empty() {
        let value = (&schema.revision, &schema.runtime_feature_domains);
        schema.revision = if buffered_revision {
            resource_compiler::json_digest_buffered(&value, &root.opening.source, observation)
        } else {
            resource_compiler::json_digest(&value, &root.opening.source, observation)
        }
        .map_err(|e| root.error(e.message))?;
    }
    Ok(ClassedPublication {
        schema,
        inventory,
        selections,
        root,
    })
}

fn finish_classed_publication<A: crate::definitions::schema::RuntimeAlternative>(
    publication: ClassedPublication<'_, A>,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<crate::definitions::schema::SchemaRepresentation<A>, ImportError> {
    let ClassedPublication {
        mut schema,
        inventory,
        selections,
        root,
    } = publication;
    schema.generic_signatures = Some(inventory);
    let prepared_types = observation.preparation(&root.opening.source, |context| {
        crate::definitions::schema::ValidatedRuntimeTypes::prepare_observed(&schema, context)
    })?;
    for receiver in schema.runtime_feature_domains.keys() {
        validate_publication_defaults(&schema, receiver, &root, conversions)?;
    }
    let mut contracts = BTreeMap::new();
    for (class, (declared, span)) in &selections {
        let (key, operations) = prepare_publication_operations(
            &schema,
            class,
            declared,
            span,
            &prepared_types,
            &root,
            observation,
        )?;
        contracts.insert(key, operations);
    }
    schema.declared_operation_selections = Some(contracts);
    schema.revision = resource_compiler::json_digest(
        &(&schema.revision, &schema.declared_operation_selections),
        &root.opening.source,
        observation,
    )
    .map_err(|e| root.error(e.message))?;
    // Installing declared selections changes schema ownership. Obtain a fresh
    // immutable proof before validating those groups; no proof crosses mutation.
    let prepared_types = observation.preparation(&root.opening.source, |context| {
        crate::definitions::schema::ValidatedRuntimeTypes::prepare_observed(&schema, context)
    })?;
    for (class, (_, span)) in &selections {
        observation
            .preparation(&span.source, |context| {
                schema.validate_declared_operation_inventory_prepared(
                    class,
                    &prepared_types,
                    context,
                )
            })
            .map_err(|e| ImportError::new(ErrorKind::InvalidDefinition, span, e.message))?;
    }
    Ok(schema)
}

// Retains the same compiler output, its source tree and unpublished metadata.
// Structural validation still precedes every runtime metadata installation.
struct ClassedPublicationPreparation<'source, A: crate::definitions::schema::RuntimeAlternative> {
    output: Option<SchemaOutputPreparation>,
    output_inputs: Option<(String, SchemaLoweringPolicy)>,
    reports: BTreeMap<String, std::rc::Rc<PreparedRuntimeMembers<'source>>>,
    pending_feature_domains: BTreeMap<String, Vec<PendingFeatureDomain<A>>>,
    lowered: Option<Schema>,
    root: Xml,
    inventory: crate::definitions::generic_resolution::SignatureInventory,
    selections: ReportSelections<'source>,
    instantiations: BTreeMap<String, RuntimeType>,
    raw_reference_bounds: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    runtime_warnings: Vec<crate::definitions::generic_resolution::Warning>,
    operation_templates: BTreeMap<
        String,
        BTreeMap<DeclarationId, crate::definitions::generic_resolution::SpecializedMember>,
    >,
    runtime_feature_domains: BTreeMap<
        String,
        Vec<crate::definitions::schema::CheckedRuntimeFeatureDomainRepresentation<A>>,
    >,
    relation_contracts: Vec<crate::definitions::conversion::ConversionContract>,
    validation: Option<Box<FrozenLoweredSchema>>,
    validated: bool,
}
fn begin_classed_publication<'source, A: crate::definitions::schema::RuntimeAlternative>(
    state: &mut MaterializationContext<'source>,
    inventory: crate::definitions::generic_resolution::SignatureInventory,
    prepared: PreparedClasses<'source, A>,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<ClassedPublicationPreparation<'source, A>, ImportError> {
    let PreparedClasses {
        selections,
        selection,
        classes,
    } = prepared;
    let resolver = state.resolver;
    let bundle = state.bundle;
    let context = Context { state, observation };
    let ClassSelection {
        root,
        instantiations,
        raw_reference_bounds,
        runtime_warnings,
        operation_templates,
    } = classes;
    let FeatureDomainSelection {
        domains: pending_feature_domains,
        attribute_defaults: attribute_domain_defaults,
    } = selection;
    // Bound-only providers are compilation dependencies even without stored values.
    // Retain their datatypes and exact contracts so replay can reconstruct the checks.
    let mut relation_contracts = Vec::new();
    for id in resolver.consumed_relation_datatypes() {
        let mut node = context.xml(&id)?;
        node.attrs.remove("xmi:id");
        let datatype = lower_datatype_observed(&node, Some(observation))?;
        let registry =
            conversions.ok_or_else(|| node.error("Missing consumed relation registry"))?;
        let contract = registry
            .require(&datatype)
            .map_err(|message| node.error(message))?;
        retain_compilation_contract(
            contract,
            &mut relation_contracts,
            &node.opening.source,
            observation,
        )?;
        let closed = classifier_type(&id, false, &node, observation)?;
        let name = key(&closed);
        context
            .state
            .declarations
            .insert(name.clone(), context.provenance(&id)?);
        context.state.datatypes.insert(name, datatype);
    }
    let revision = materialization_revision(
        bundle,
        context.state.types.keys(),
        &root.opening,
        observation,
    )?;
    let output_inputs = Some((
        revision,
        SchemaLoweringPolicy {
            validate_projected_operations: false,
            move_feature_declarations: context.state.reuse_compiler_type_inputs,
            constructor_contracts: resolver.consumed_constructor_contracts(),
            reference_top: context.state.reference_top.take(),
            attribute_domain_defaults,
        },
    ));
    let reports = std::mem::take(&mut context.state.types);
    Ok(ClassedPublicationPreparation {
        output: None,
        output_inputs,
        reports,
        pending_feature_domains,
        lowered: None,
        root,
        inventory,
        selections,
        instantiations,
        raw_reference_bounds,
        runtime_warnings,
        operation_templates,
        runtime_feature_domains: BTreeMap::new(),
        relation_contracts,
        validation: None,
        validated: false,
    })
}
impl<'source, A: crate::definitions::schema::RuntimeAlternative>
    ClassedPublicationPreparation<'source, A>
{
    #[cfg(test)]
    fn diagnostic_phase(&self) -> &'static str {
        if !self.reports.is_empty() {
            "publication_domains"
        } else if self.output_inputs.is_some() {
            "publication_output_begin"
        } else if self.output.is_some() {
            "publication_lowering"
        } else if !self.validated {
            "publication_validation"
        } else {
            "publication_install"
        }
    }
    fn advance(
        &mut self,
        state: &mut MaterializationContext<'source>,
        storage: &mut impl AlternativeStorage<Target = A>,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        observation.work(0, &self.root.opening.source)?;
        if let Some((name, report)) = self.reports.pop_first() {
            let root = &self.root;
            observation.work(1, &root.opening.source)?;
            let report = std::rc::Rc::try_unwrap(report).map_err(|_| {
                root.error("Feature domain report remains shared after class lowering")
            })?;
            let domains = storage.domains(report, observation)?;
            let outcomes = self.pending_feature_domains.remove(&name);
            if domains.is_empty() {
                if outcomes.as_ref().is_some_and(|rows| !rows.is_empty()) {
                    return Err(root.error("Checked feature domain outcome count mismatch"));
                }
                return Ok(false);
            }
            let outcomes =
                outcomes.ok_or_else(|| root.error("Missing checked feature domain outcomes"))?;
            if domains.len() != outcomes.len() {
                return Err(root.error("Checked feature domain outcome count mismatch"));
            }
            observation.reserve(128, &root.opening.source)?;
            let mut checked = Vec::new();
            for (source, outcome) in domains.into_iter().zip(outcomes) {
                observation.work(1, &root.opening.source)?;
                observation.reserve(128 + 32, &root.opening.source)?;
                checked.push(
                    crate::definitions::schema::CheckedRuntimeFeatureDomainRepresentation {
                        source,
                        alternatives: outcome.alternatives,
                        unresolved: outcome.unresolved,
                        lexical_target: outcome.lexical_target,
                        default: outcome.default,
                    },
                );
            }
            observation.reserve(128, &root.opening.source)?;
            self.runtime_feature_domains.insert(name, checked);
            return Ok(false);
        }
        if let Some((revision, policy)) = self.output_inputs.take() {
            if !self.pending_feature_domains.is_empty() {
                return Err(self
                    .root
                    .error("Unconsumed checked feature domain outcomes"));
            }
            self.output = Some(SchemaOutputPreparation::begin(
                &mut self.root,
                revision,
                std::mem::take(&mut state.enums),
                std::mem::take(&mut state.datatypes),
                conversions,
                policy,
                observation,
            )?);
            return Ok(false);
        }
        if let Some(output) = &mut self.output {
            if output.advance(&mut self.root, conversions, observation)? {
                self.lowered = Some(
                    self.output
                        .take()
                        .ok_or_else(|| self.root.error("Missing complete output lowering"))?
                        .finish(&self.root)?,
                );
            }
            return Ok(false);
        }
        if !self.validated {
            if let Some(validation) = &mut self.validation {
                if validation.advance(&self.root, observation)? {
                    self.validated = true;
                }
            } else {
                let schema = self
                    .lowered
                    .take()
                    .ok_or_else(|| self.root.error("Missing lowered schema"))?;
                self.validation = Some(Box::new(FrozenLoweredSchema::begin(
                    schema,
                    &self.root,
                    observation,
                )?));
            }
            return Ok(false);
        }
        Ok(true)
    }
}
fn finish_prepared_classed_publication<
    'source,
    A: crate::definitions::schema::RuntimeAlternative,
>(
    state: &mut MaterializationContext<'source>,
    pending: ClassedPublicationPreparation<'source, A>,
    hash_runtime_metadata: bool,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<ClassedPublication<'source, A>, ImportError> {
    let ClassedPublicationPreparation {
        output: _,
        output_inputs: _,
        reports: _,
        pending_feature_domains: _,
        lowered: _,
        root,
        inventory,
        selections,
        instantiations,
        raw_reference_bounds,
        runtime_warnings,
        operation_templates,
        runtime_feature_domains,
        relation_contracts,
        validation,
        validated,
    } = pending;
    if !validated {
        return Err(root.error("Publication lowering is incomplete"));
    }
    let mut schema = validation
        .ok_or_else(|| root.error("Missing complete lowered schema"))?
        .finish(&root)?
        .into_empty_runtime_representation::<A>()
        .map_err(|error| root.error(error))?;
    for (class, templates) in &operation_templates {
        let operations = &mut schema
            .classes
            .get_mut(class)
            .ok_or_else(|| root.error("Missing template owner"))?
            .operations;
        for operation in operations {
            if let Some(template) = templates.get(&operation.declaration) {
                crate::definitions::operation_templates::project(operation, template)
                    .map_err(|e| root.error(e))?;
            }
        }
    }
    // The lowerer returns canonical sorted contracts. This merge only appends
    // previously absent entries, so length detects change without a deep snapshot.
    let previous_contract_count = schema.conversion_contracts.len();
    for contract in relation_contracts {
        observation.work(1, &root.opening.source)?;
        if !schema.conversion_contracts.contains(&contract) {
            observation.reserve(32, &root.opening.source)?;
            schema.conversion_contracts.push(contract);
        }
    }
    if schema.conversion_contracts.len() != previous_contract_count {
        observe_contract_sort(
            schema.conversion_contracts.len(),
            &root.opening.source,
            observation,
        )?;
        schema
            .conversion_contracts
            .sort_by(|a, b| a.datatype.cmp(&b.datatype));
        schema.revision = resource_compiler::json_digest(
            &(&schema.revision, &schema.conversion_contracts),
            &root.opening.source,
            observation,
        )
        .map_err(|e| root.error(e.message))?;
    }
    schema.declarations = std::mem::take(&mut state.declarations);
    schema.instantiations = instantiations;
    schema.raw_reference_bounds = raw_reference_bounds;
    schema.runtime_feature_domains = runtime_feature_domains;
    schema.runtime_warnings = runtime_warnings;
    if hash_runtime_metadata
        && (!schema.raw_reference_bounds.is_empty() || !schema.runtime_warnings.is_empty())
    {
        schema.revision = resource_compiler::json_digest(
            &(
                &schema.revision,
                &schema.raw_reference_bounds,
                &schema.runtime_warnings,
            ),
            &root.opening.source,
            observation,
        )
        .map_err(|e| root.error(e.message))?;
    }
    Ok(ClassedPublication {
        schema,
        inventory,
        selections,
        root,
    })
}
fn prepare_classed_publication<'source, P: AlternativeStorage>(
    state: &mut MaterializationContext<'source>,
    inventory: crate::definitions::generic_resolution::SignatureInventory,
    prepared: PreparedClasses<'source, P::Target>,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    storage: &mut P,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<ClassedPublication<'source, P::Target>, ImportError> {
    let mut pending =
        begin_classed_publication(state, inventory, prepared, conversions, observation)?;
    while !pending.advance(state, storage, conversions, observation)? {}
    finish_prepared_classed_publication(state, pending, true, observation)
}

fn validate_publication_defaults<A: crate::definitions::schema::RuntimeAlternative>(
    schema: &crate::definitions::schema::SchemaRepresentation<A>,
    receiver: &str,
    root: &Xml,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
) -> Result<(), ImportError> {
    for field in schema
        .features(receiver)
        .map_err(|e| root.error(e))?
        .values()
    {
        if !field.reference && !schema.feature_domains(receiver, &field.name).is_empty() {
            schema
                .require_runtime_attribute_conversion(receiver, field, conversions)
                .map_err(|e| root.error(e))?;
            if field.upper == 1 && !field.default.is_null() {
                schema
                    .validate_runtime_attribute(receiver, field, &field.default, conversions)
                    .map_err(|e| root.error(format!("Invalid receiver-specific default: {e}")))?;
            }
        }
    }
    Ok(())
}
fn prepare_publication_operations<A: crate::definitions::schema::RuntimeAlternative>(
    schema: &crate::definitions::schema::SchemaRepresentation<A>,
    class: &str,
    declared: &crate::definitions::generic_resolution::GenericOperationSet,
    span: &SourceSpan,
    prepared_types: &crate::definitions::schema::ValidatedRuntimeTypes<'_, A>,
    root: &Xml,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<(String, crate::definitions::operations::DeclaredOperationSet), ImportError> {
    observation.work(1, &span.source)?;
    let effective = observation
        .preparation(&span.source, |context| {
            schema.effective_operations_observed(class, context)
        })
        .map_err(|e| ImportError::new(ErrorKind::InvalidDefinition, span, e.message))?;
    let mut groups = Vec::new();
    for group in &declared.operations {
        observation.work(1, &span.source)?;
        let mut operations = Vec::new();
        for member in std::iter::once(&group.selected)
            .chain(&group.selected_projections)
            .chain(&group.overridden)
        {
            observation.work(1, &span.source)?;
            let owner = if let Some(projection) = &member.projection {
                observation.preparation(&span.source, |context| {
                    projection
                        .storage_owner
                        .retained_clone_observed(context)
                        .map(|_| ())
                })?;
                projection.storage_owner.clone()
            } else {
                runtime_type(&member.member.owner, &span.source, observation, |e| {
                    root.error(e)
                })?
            };
            let owner_key = observation.preparation(&span.source, |context| {
                prepared_types.class_key_for_type_observed(&owner, context)
            })?;
            let mut selected_entry = None;
            for entry in &effective {
                observation.work(1, &span.source)?;
                if entry.operation.declaration == member.member.declaration
                    && entry.owner == owner_key
                {
                    selected_entry = Some(entry);
                    break;
                }
            }
            let entry = selected_entry
                .ok_or_else(|| root.error("Missing declared operation projection"))?;
            let template = if observation
                .preparation(&span.source, |context| {
                    member.member.requires_schema_template_observed(context)
                })
                .map_err(|e| root.error(e.message))?
            {
                observation.serialized(&member.member, &span.source)?;
                Some(member.member.clone())
            } else {
                None
            };
            observation.serialized(entry.operation, &span.source)?;
            let mut operation = entry.operation.clone();
            if let Some(template) = &template {
                observation
                    .preparation(&span.source, |context| {
                        crate::definitions::operation_templates::project_observed(
                            &mut operation,
                            template,
                            context,
                        )
                    })
                    .map_err(|e| root.error(e.message))?;
            }
            observation.serialized(&member.projection, &span.source)?;
            observation.string(owner_key.len(), &span.source)?;
            observation.serialized(&member.raw_parameters, &span.source)?;
            observation.reserve(32, &span.source)?;
            operations.push(crate::definitions::operations::DeclaredOperation {
                projection: member.projection.clone(),
                template,
                owner: owner_key.to_owned(),
                operation,
                raw_parameters: member.raw_parameters.clone(),
            });
        }
        let selected = operations
            .first()
            .ok_or_else(|| root.error("Empty declared operation group"))?;
        let signature = observation
            .preparation(&span.source, |context| {
                crate::definitions::declared_operations::erased_signature_observed(
                    &selected.operation,
                    &selected.raw_parameters,
                    context,
                )
            })
            .map_err(|e| root.error(e.message))?;
        observation.serialized(&group.selected.member.declaration, &span.source)?;
        let selected_owners = if group.selected_projections.is_empty() {
            vec![]
        } else {
            let mut owners = Vec::new();
            for operation in std::iter::once(&group.selected).chain(&group.selected_projections) {
                observation.work(1, &span.source)?;
                observation.serialized(&operation.member.owner, &span.source)?;
                observation.reserve(32, &span.source)?;
                owners.push(operation.member.owner.clone());
            }
            owners
        };
        observation.reserve(32, &span.source)?;
        groups.push(crate::definitions::operations::DeclaredOperationGroup {
            signature,
            selected: group.selected.member.declaration.clone(),
            selected_owners,
            operations,
        });
    }
    observation.string(class.len(), &span.source)?;
    observation.preparation(&span.source, |context| {
        declared
            .receiver
            .retained_clone_observed(context)
            .map(|_| ())
    })?;
    observation.reserve(128, &span.source)?;
    Ok((
        class.to_owned(),
        crate::definitions::operations::DeclaredOperationSet {
            receiver: declared.receiver.clone(),
            groups,
        },
    ))
}
// Preserve the existing ordered-set revision contract. Entries borrow immutable
// Complete owned source inputs. No observer/provider execution resources live here.
struct MaterializationSourceInputs {
    bundle: DefinitionBundle,
    subset: std::cell::OnceCell<DefinitionBundle>,
    resolver: std::cell::OnceCell<Resolver>,
    loading: Option<Box<SourceLoadingInputs>>,
}
impl MaterializationSourceInputs {
    fn selected_bundle(&self) -> &DefinitionBundle {
        self.subset.get().unwrap_or(&self.bundle)
    }
}
#[path = "materialization_source_preparation.rs"]
mod source_preparation;
use source_preparation::{MaterializationState, SourceLoadingInputs};
self_cell::self_cell!(
    struct OwnedMaterializationContext {
        owner: MaterializationSourceInputs,
        #[covariant]
        dependent: MaterializationState,
    }
);
// Portable logical preflight for the pinned holder's owner/dependent allocation.
// Existing borrowed ownership families retain their previous accounting.
const OWNED_CONTEXT_BYTES: usize = 2048;
const _: () = assert!(
    std::mem::size_of::<MaterializationSourceInputs>()
        + std::mem::size_of::<MaterializationState<'static>>()
        + std::mem::size_of::<MaterializationContext<'static>>()
        + 128
        <= OWNED_CONTEXT_BYTES
);

// Internal runtime driver assembly. Fixed owned inputs outlive each fresh observer.
// Provider policy is fixed by the surrounding compilation attempt; this is not yet
// a public resumable API accepting replacement registries between advances.
// This choice is fixed by construction, never an advance argument. Compatibility
// keeps the original public callback/cancellation macrosteps; the opt-in contract
// also exposes bounded legacy/agreement subdivisions. Other compiler tails stay.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GenericAgreementPolling {
    Compatibility,
    Retained,
    RetainedSourceInventory,
    RetainedGenericSourceDecoding,
}
struct OwnedMaterializationPreparation {
    generic_agreement_polling: GenericAgreementPolling,
    nominal_bitmaps: bool,
    observe_membership: bool,
    buffered_revision: bool,
    incremental_revision: bool,
    publication_started: bool,
    policy: CompilationPolicyInputs,
    holder: OwnedMaterializationContext,
    inventory: Option<crate::definitions::generic_resolution::SignatureInventory>,
    fallback: SourceSpan,
    reports_complete: bool,
    features_complete: bool,
    classes_complete: bool,
    feature_progress:
        Option<FeatureDomainProgress<crate::definitions::schema::SharedRuntimeSourceArrayTarget>>,
    storage: SharedArrayAlternatives<crate::definitions::schema::SharedRuntimeSourceArrayTarget>,
    failed: bool,
}
impl OwnedMaterializationPreparation {
    fn advance_reports(
        &mut self,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        if self.failed {
            return Err(ImportError::new(
                ErrorKind::InvalidDefinition,
                &self.fallback,
                "Owned materialization preparation has failed",
            ));
        }
        let result = self.holder.with_dependent_mut(|_, state| {
            let state = state.ready_mut(&self.fallback)?;
            // Completed advances still honor the current cancellation observer.
            observation.work(0, &self.fallback.source)?;
            let continuation = (self.generic_agreement_polling
                != GenericAgreementPolling::Compatibility
                && state
                    .reports()
                    .is_some_and(ReportPreparation::is_silent_agreement_step))
                || (matches!(
                    self.generic_agreement_polling,
                    GenericAgreementPolling::RetainedSourceInventory
                        | GenericAgreementPolling::RetainedGenericSourceDecoding
                ) && state
                    .source_inventory
                    .as_ref()
                    .is_some_and(|preparation| preparation.is_retained_agreement_step()));
            if continuation {
                // The original owner macrostep already admitted its policy
                // comparison. Still revalidate CURRENT registry metadata before
                // every new subdivision, without billing that same macrostep
                // twice. This whole synchronous comparison remains an explicit
                // unqualified physical-work tail; it is not a cached proof.
                let context = crate::definitions::conversion::InvocationContext::default();
                let revalidation = crate::definitions::ecore::ParserObservation::new(&context);
                self.policy
                    .verify(conversions, &self.fallback, &revalidation)?;
            } else {
                self.policy
                    .verify(conversions, &self.fallback, observation)?;
            }
            if self.reports_complete {
                return Ok(true);
            }
            if let Some(mut preparation) = state.source_inventory.take() {
                if preparation.advance(observation)? {
                    let inventory = preparation.finish()?;
                    // Prepared formal/cache installation is still a synchronous
                    // boundary, separate from retained source walking.
                    state
                        .resolver
                        .install_materialization_inventory(&inventory, observation)?;
                    self.inventory = Some(inventory);
                } else {
                    state.source_inventory = Some(preparation);
                }
                return Ok(false);
            }
            let inventory = self.inventory.as_mut().ok_or_else(|| {
                ImportError::new(
                    ErrorKind::InvalidDefinition,
                    &self.fallback,
                    "Missing owned signature inventory",
                )
            })?;
            let mut preparation = state.take_reports().ok_or_else(|| {
                ImportError::new(
                    ErrorKind::InvalidDefinition,
                    &self.fallback,
                    "Missing owned report preparation",
                )
            })?;
            let mut context = Context { state, observation };
            let result = preparation.advance_with_polling(
                &mut context,
                inventory,
                &self.fallback,
                self.generic_agreement_polling,
            );
            context.state.set_reports(preparation);
            result
        });
        match result {
            Ok(done) => {
                self.reports_complete = done;
                Ok(done)
            }
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }
    fn advance_features(
        &mut self,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        if self.failed || !self.reports_complete {
            return Err(ImportError::new(
                ErrorKind::InvalidDefinition,
                &self.fallback,
                "Owned report preparation is incomplete or failed",
            ));
        }
        let result = self.holder.with_dependent_mut(|_, state| {
            let state = state.ready_mut(&self.fallback)?;
            observation.work(0, &self.fallback.source)?;
            self.policy
                .verify(conversions, &self.fallback, observation)?;
            if self.features_complete {
                return Ok(true);
            }
            if !state.has_features() {
                let selections = state
                    .take_reports()
                    .ok_or_else(|| {
                        ImportError::new(
                            ErrorKind::InvalidDefinition,
                            &self.fallback,
                            "Missing completed owned reports",
                        )
                    })?
                    .finish(&self.fallback)?;
                let inputs = prepare_feature_inputs(
                    state,
                    selections,
                    conversions,
                    &self.fallback,
                    observation,
                    self.nominal_bitmaps,
                )?;
                state.set_features(inputs);
                let mut progress = FeatureDomainProgress::new();
                progress.observe_membership = self.observe_membership;
                self.feature_progress = Some(progress);
                return Ok(false);
            }
            let inputs = state.take_features().ok_or_else(|| {
                ImportError::new(
                    ErrorKind::InvalidDefinition,
                    &self.fallback,
                    "Missing retained feature inputs",
                )
            })?;
            let progress = self
                .feature_progress
                .take()
                .ok_or_else(|| inputs.root.error("Missing retained feature progress"))?;
            let mut preparation =
                FeatureDomainPreparation::from_inputs(state, &inputs, &mut self.storage, progress);
            let result = preparation.advance(conversions, observation);
            self.feature_progress = Some(preparation.into_progress());
            state.set_features(inputs);
            result
        });
        match result {
            Ok(done) => {
                self.features_complete = done;
                Ok(done)
            }
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }
    fn advance_classes(
        &mut self,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<bool, ImportError> {
        if self.failed || !self.features_complete {
            return Err(ImportError::new(
                ErrorKind::InvalidDefinition,
                &self.fallback,
                "Owned feature preparation is incomplete or failed",
            ));
        }
        let result = self.holder.with_dependent_mut(|_, state| {
            let state = state.ready_mut(&self.fallback)?;
            observation.work(0, &self.fallback.source)?;
            self.policy
                .verify(conversions, &self.fallback, observation)?;
            if self.classes_complete {
                return Ok(true);
            }
            if !state.has_class() {
                let inputs = state.take_features().ok_or_else(|| {
                    ImportError::new(
                        ErrorKind::InvalidDefinition,
                        &self.fallback,
                        "Missing complete feature inputs",
                    )
                })?;
                let progress = self
                    .feature_progress
                    .take()
                    .ok_or_else(|| inputs.root.error("Missing complete feature progress"))?;
                let selection = FeatureDomainPreparation::from_inputs(
                    state,
                    &inputs,
                    &mut self.storage,
                    progress,
                )
                .finish()?;
                let FeaturePreparationInputs {
                    nominal: _,
                    root,
                    reports,
                    candidates,
                    selections,
                } = inputs;
                drop(candidates);
                state.set_class(ClassPreparation::new(root, reports));
                state.set_class_inputs(OwnedClassInputs {
                    selections,
                    selection,
                });
            }
            let mut preparation = state.take_class().ok_or_else(|| {
                ImportError::new(
                    ErrorKind::InvalidDefinition,
                    &self.fallback,
                    "Missing owned class preparation",
                )
            })?;
            let mut context = Context { state, observation };
            let result = preparation.advance(&mut context);
            context.state.set_class(preparation);
            result
        });
        match result {
            Ok(done) => {
                self.classes_complete = done;
                Ok(done)
            }
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }
    fn begin_incremental_publication(
        &mut self,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<(), ImportError> {
        if self.failed || !self.classes_complete || self.publication_started {
            return Err(ImportError::new(
                ErrorKind::InvalidDefinition,
                &self.fallback,
                "Class publication cannot begin in this state",
            ));
        }
        observation.reserve(PENDING_PUBLICATION_BYTES, &self.fallback.source)?;
        let inventory = self.inventory.take().ok_or_else(|| {
            ImportError::new(
                ErrorKind::InvalidDefinition,
                &self.fallback,
                "Owned materialization already consumed",
            )
        })?;
        self.holder.with_dependent_mut(|_, state| {
            let state = state.ready_mut(&self.fallback)?;
            self.policy
                .verify(conversions, &self.fallback, observation)?;
            let OwnedClassInputs {
                selections,
                selection,
            } = state.take_class_inputs().ok_or_else(|| {
                ImportError::new(
                    ErrorKind::InvalidDefinition,
                    &self.fallback,
                    "Missing owned class publication inputs",
                )
            })?;
            let classes = state
                .take_class()
                .ok_or_else(|| {
                    ImportError::new(
                        ErrorKind::InvalidDefinition,
                        &self.fallback,
                        "Missing complete owned classes",
                    )
                })?
                .finish()?;
            let assembly = begin_classed_publication(
                state,
                inventory,
                PreparedClasses {
                    selections,
                    selection,
                    classes,
                },
                conversions,
                observation,
            )?;
            state.pending_revision = Some(Box::new(PendingRevisionPublication {
                assembly: Some(assembly),
                inputs: None,
                tail: None,
                digest: None,
                metadata_digest: None,
                metadata_revision_done: false,
                revision_done: false,
            }));
            Ok(())
        })?;
        self.publication_started = true;
        Ok(())
    }
    fn advance_incremental_publication(
        &mut self,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Option<crate::definitions::schema::RuntimeSourceArraySchemaInputs>, ImportError>
    {
        self.holder.with_dependent_mut(|_, state| {
            let state = state.ready_mut(&self.fallback)?;
            observation.work(0, &self.fallback.source)?;
            self.policy
                .verify(conversions, &self.fallback, observation)?;
            let mut pending = state.pending_revision.take().ok_or_else(|| {
                ImportError::new(
                    ErrorKind::InvalidDefinition,
                    &self.fallback,
                    "Missing private publication inputs",
                )
            })?;
            if let Some(assembly) = &mut pending.assembly {
                if assembly.advance(state, &mut self.storage, conversions, observation)? {
                    let assembly = pending.assembly.take().ok_or_else(|| {
                        ImportError::new(
                            ErrorKind::InvalidDefinition,
                            &self.fallback,
                            "Missing complete publication assembly",
                        )
                    })?;
                    let inputs =
                        finish_prepared_classed_publication(state, assembly, false, observation)?;
                    pending.revision_done = inputs.schema.runtime_feature_domains.is_empty();
                    pending.inputs = Some(inputs);
                }
                state.pending_revision = Some(pending);
                return Ok(None);
            }
            if !pending.metadata_revision_done {
                let inputs = pending.inputs.as_mut().ok_or_else(|| {
                    ImportError::new(
                        ErrorKind::InvalidDefinition,
                        &self.fallback,
                        "Missing metadata publication inputs",
                    )
                })?;
                if let Some(digest) = &mut pending.metadata_digest {
                    if digest.advance(&inputs.root.opening.source, observation)? {
                        let values = pending
                            .metadata_digest
                            .take()
                            .ok_or_else(|| {
                                inputs.root.error("Missing completed metadata revision")
                            })?
                            .finish()
                            .map_err(|error| inputs.root.error(error))?;
                        inputs.schema.revision = values.revision;
                        inputs.schema.raw_reference_bounds = values.raw_reference_bounds;
                        inputs.schema.runtime_warnings = values.runtime_warnings;
                        pending.metadata_revision_done = true;
                    }
                } else if inputs.schema.raw_reference_bounds.is_empty()
                    && inputs.schema.runtime_warnings.is_empty()
                {
                    pending.metadata_revision_done = true;
                } else {
                    pending.metadata_digest =
                        Some(resource_compiler::MetadataRevisionPreparation::new(
                            std::mem::take(&mut inputs.schema.revision),
                            std::mem::take(&mut inputs.schema.raw_reference_bounds),
                            std::mem::take(&mut inputs.schema.runtime_warnings),
                            &inputs.root.opening.source,
                            observation,
                        )?);
                }
                state.pending_revision = Some(pending);
                return Ok(None);
            }
            if !pending.revision_done {
                let inputs = pending.inputs.as_mut().ok_or_else(|| {
                    ImportError::new(
                        ErrorKind::InvalidDefinition,
                        &self.fallback,
                        "Missing revision publication inputs",
                    )
                })?;
                if let Some(digest) = &mut pending.digest {
                    if digest.advance(
                        &inputs.schema.revision,
                        &inputs.schema.runtime_feature_domains,
                        &inputs.root.opening.source,
                        observation,
                    )? {
                        inputs.schema.revision = pending
                            .digest
                            .take()
                            .ok_or_else(|| inputs.root.error("Missing completed revision"))?
                            .finish()
                            .map_err(|e| inputs.root.error(e))?;
                        pending.revision_done = true;
                    }
                } else {
                    pending.digest =
                        Some(Box::new(resource_compiler::MapRevisionPreparation::new(
                            &inputs.root.opening.source,
                            observation,
                        )?));
                }
                state.pending_revision = Some(pending);
                return Ok(None);
            }
            if let Some(tail) = &mut pending.tail {
                if tail.advance(conversions, observation)? {
                    let schema = pending
                        .tail
                        .take()
                        .ok_or_else(|| {
                            ImportError::new(
                                ErrorKind::InvalidDefinition,
                                &self.fallback,
                                "Missing completed publication tail",
                            )
                        })?
                        .finish()?;
                    return Ok(Some(schema));
                }
            } else {
                let inputs = pending.inputs.take().ok_or_else(|| {
                    ImportError::new(
                        ErrorKind::InvalidDefinition,
                        &self.fallback,
                        "Missing pending publication tail inputs",
                    )
                })?;
                pending.tail = Some(publication_validation::PublicationTail::new(
                    inputs,
                    state.reuse_compiler_type_inputs,
                    observation,
                )?);
            }
            state.pending_revision = Some(pending);
            Ok(None)
        })
    }
    fn finish(
        &mut self,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<crate::definitions::schema::RuntimeSourceArraySchemaInputs, ImportError> {
        if self.failed || !self.features_complete {
            return Err(ImportError::new(
                ErrorKind::InvalidDefinition,
                &self.fallback,
                "Owned report preparation is incomplete or failed",
            ));
        }
        // Private synchronous callers drive the same class transitions here.
        // Public advances reach this method only after the class phase completes.
        while !self.classes_complete {
            self.advance_classes(conversions, observation)?;
        }
        let inventory = self.inventory.take().ok_or_else(|| {
            ImportError::new(
                ErrorKind::InvalidDefinition,
                &self.fallback,
                "Owned materialization already consumed",
            )
        })?;
        // Consuming the remaining compiler is terminal even when it rejects input.
        self.failed = true;
        self.holder.with_dependent_mut(|_, state| {
            let state = state.ready_mut(&self.fallback)?;
            observation.work(0, &self.fallback.source)?;
            self.policy
                .verify(conversions, &self.fallback, observation)?;
            let OwnedClassInputs {
                selections,
                selection,
            } = state.take_class_inputs().ok_or_else(|| {
                ImportError::new(
                    ErrorKind::InvalidDefinition,
                    &self.fallback,
                    "Missing owned class publication inputs",
                )
            })?;
            let classes = state
                .take_class()
                .ok_or_else(|| {
                    ImportError::new(
                        ErrorKind::InvalidDefinition,
                        &self.fallback,
                        "Missing complete owned classes",
                    )
                })?
                .finish()?;
            materialize_classed_context(
                state,
                inventory,
                PreparedClasses {
                    selections,
                    selection,
                    classes,
                },
                conversions,
                &mut self.storage,
                observation,
                self.buffered_revision,
            )
        })
    }
}

/// The exact input is returned on success or failure; source recovery never needs
/// a clone or reconstruction. The schema, when present, owns all published data.
pub struct OwnedRuntimeSourceArrayMaterialization {
    pub bundle: DefinitionBundle,
    pub result: Result<
        crate::definitions::schema::RuntimeSourceArraySchemaInputs,
        crate::definitions::ecore::ObservedImportError,
    >,
}
// One shared owned initialization path for synchronous and staged consumers.
#[allow(clippy::too_many_arguments)]
fn begin_owned_materialization(
    bundle: DefinitionBundle,
    entry_source: &crate::definitions::bundle::SourceId,
    namespace: &str,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    operation: &crate::definitions::conversion::InvocationContext<'_>,
    observe_inventory: bool,
) -> Result<(OwnedMaterializationPreparation, Xml), Box<OwnedRuntimeSourceArrayMaterialization>> {
    if let Err(error) = operation.checkpoint() {
        return Err(Box::new(OwnedRuntimeSourceArrayMaterialization {
            bundle,
            result: Err(crate::definitions::ecore::ObservedImportError::Observation(
                error,
            )),
        }));
    }
    let observation = crate::definitions::ecore::ParserObservation::new(operation);
    let selection = match select_materialization_inputs_with_cursor_policy(
        &bundle,
        entry_source,
        namespace,
        &observation,
        None,
        observe_inventory,
    ) {
        Ok(value) => value,
        Err(error) => {
            return Err(Box::new(OwnedRuntimeSourceArrayMaterialization {
                bundle,
                result: observation.finish(Err(error)),
            }));
        }
    };
    let policy = match CompilationPolicyInputs::capture(
        conversions,
        &selection.root.opening,
        &observation,
    ) {
        Ok(policy) => policy,
        Err(error) => {
            return Err(Box::new(OwnedRuntimeSourceArrayMaterialization {
                bundle,
                result: observation.finish(Err(error)),
            }));
        }
    };
    let selected = selection.subset.as_ref().unwrap_or(&bundle);
    let initialized = if observe_inventory {
        initialize_materialization_resolver::<
            SharedArrayAlternatives<crate::definitions::schema::SharedRuntimeSourceArrayTarget>,
        >(
            selected,
            requested,
            conversions,
            selection.resources,
            &observation,
        )
        .map(|(resolver, fallback)| (resolver, None, fallback))
    } else {
        initialize_materialization::<
            SharedArrayAlternatives<crate::definitions::schema::SharedRuntimeSourceArrayTarget>,
        >(
            selected,
            requested,
            conversions,
            selection.resources,
            &observation,
        )
        .map(|initialized| {
            (
                initialized.resolver,
                Some(initialized.inventory),
                initialized.fallback,
            )
        })
    };
    let (resolver, inventory, fallback) = match initialized {
        Ok(value) => value,
        Err(error) => {
            return Err(Box::new(OwnedRuntimeSourceArrayMaterialization {
                bundle,
                result: observation.finish(Err(error)),
            }));
        }
    };
    if let Err(error) = observation.reserve(OWNED_CONTEXT_BYTES, &fallback.source) {
        return Err(Box::new(OwnedRuntimeSourceArrayMaterialization {
            bundle,
            result: observation.finish(Err(error)),
        }));
    }
    let owner = MaterializationSourceInputs {
        bundle,
        subset: selection
            .subset
            .map_or_else(Default::default, std::cell::OnceCell::from),
        resolver: std::cell::OnceCell::from(resolver),
        loading: None,
    };
    let holder = match OwnedMaterializationContext::try_new_or_recover(owner, |inputs| {
        let mut state = new_materialization_context(
            inputs.selected_bundle(),
            inputs.resolver()?,
            &observation,
        )?;
        if observe_inventory {
            inputs
                .resolver()?
                .enable_source_reference_inputs(&observation)?;
            state.source_inventory = Some(Box::new(
                crate::definitions::generic_resolution::SourceInventoryPreparation::new(
                    inputs.resolver()?,
                    &observation,
                )?,
            ));
        }
        state.set_reports(ReportPreparation::new(requested, &fallback)?);
        Ok(MaterializationState::Ready(Box::new(state)))
    }) {
        Ok(holder) => holder,
        Err((owner, error)) => {
            return Err(Box::new(OwnedRuntimeSourceArrayMaterialization {
                bundle: owner.bundle,
                result: observation.finish(Err(error)),
            }));
        }
    };
    let preparation = OwnedMaterializationPreparation {
        generic_agreement_polling: GenericAgreementPolling::Compatibility,
        nominal_bitmaps: false,
        policy,
        holder,
        inventory,
        fallback,
        observe_membership: false,
        buffered_revision: false,
        incremental_revision: false,
        publication_started: false,
        reports_complete: false,
        features_complete: false,
        classes_complete: false,
        feature_progress: None,
        storage: SharedArrayAlternatives::default(),
        failed: false,
    };
    Ok((preparation, selection.root))
}
fn advance_owned_materialization(
    preparation: &mut OwnedMaterializationPreparation,
    root: &Xml,
    entry_source: &crate::definitions::bundle::SourceId,
    namespace: &str,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<Option<crate::definitions::schema::RuntimeSourceArraySchemaInputs>, ImportError> {
    if !preparation.reports_complete {
        preparation.advance_reports(conversions, observation)?;
        return Ok(None);
    }
    if !preparation.features_complete {
        preparation.advance_features(conversions, observation)?;
        return Ok(None);
    }
    if !preparation.classes_complete {
        preparation.advance_classes(conversions, observation)?;
        return Ok(None);
    }
    if preparation.incremental_revision {
        if !preparation.publication_started {
            preparation.begin_incremental_publication(conversions, observation)?;
            return Ok(None);
        }
        let Some(mut schema) =
            preparation.advance_incremental_publication(conversions, observation)?
        else {
            return Ok(None);
        };
        finish_materialization_selection(&mut schema, root, entry_source, namespace, observation)?;
        return Ok(Some(schema));
    }
    let mut schema = preparation.finish(conversions, observation)?;
    finish_materialization_selection(&mut schema, root, entry_source, namespace, observation)?;
    Ok(Some(schema))
}
/// Source and selection ownership recovered after preparation, including errors.
pub struct OwnedRuntimeSourceArrayPreparationResult {
    pub entry_source: crate::definitions::bundle::SourceId,
    pub namespace: String,
    pub materialization: OwnedRuntimeSourceArrayMaterialization,
}
/// An immutable completed schema plus the exact transferred source inputs.
pub struct OwnedCheckedRuntimeSourceArrayPreparationResult {
    pub entry_source: crate::definitions::bundle::SourceId,
    pub namespace: String,
    pub bundle: DefinitionBundle,
    pub result: Result<
        crate::definitions::schema::CheckedRuntimeSchema<
            crate::definitions::schema::SharedRuntimeSourceArrayTarget,
        >,
        crate::definitions::ecore::ObservedImportError,
    >,
}
/// Attempt-local owned preparation. No registry or host observer is retained.
/// Compatibility constructors initialize synchronously; the compiled-signature
/// profile retains source parsing/indexing before report, feature and class stages.
/// Resolver installation and node-local work retain synchronous boundaries.
/// Finish publishes only complete results, without a universal host latency bound.
pub struct OwnedRuntimeSourceArrayPreparation {
    preparation: OwnedMaterializationPreparation,
    root: Option<Xml>,
    entry_source: crate::definitions::bundle::SourceId,
    namespace: String,
    schema: Option<crate::definitions::schema::RuntimeSourceArraySchemaInputs>,
    failure: Option<crate::definitions::ecore::ObservedImportError>,
}
impl OwnedRuntimeSourceArrayPreparation {
    /// Transfer complete inputs into one fresh attempt. Errors recover the exact
    /// original bundle and selection, with no reconstruction or source clone.
    pub fn begin(
        bundle: DefinitionBundle,
        entry_source: crate::definitions::bundle::SourceId,
        namespace: String,
        requested: &[RuntimeType],
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        operation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Self, Box<OwnedRuntimeSourceArrayPreparationResult>> {
        match begin_owned_materialization(
            bundle,
            &entry_source,
            &namespace,
            requested,
            conversions,
            operation,
            false,
        ) {
            Ok((preparation, root)) => Ok(Self {
                preparation,
                root: Some(root),
                entry_source,
                namespace,
                schema: None,
                failure: None,
            }),
            Err(materialization) => Err(Box::new(OwnedRuntimeSourceArrayPreparationResult {
                entry_source,
                namespace,
                materialization: *materialization,
            })),
        }
    }
    /// Also observe each fresh candidate membership check through the shared
    /// predicate. Compatibility begin retains its previous receipt scope.
    /// This does not claim fully bounded initialization, publication or cleanup.
    pub fn begin_with_membership_observation(
        bundle: DefinitionBundle,
        entry_source: crate::definitions::bundle::SourceId,
        namespace: String,
        requested: &[RuntimeType],
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        operation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Self, Box<OwnedRuntimeSourceArrayPreparationResult>> {
        let mut session = Self::begin(
            bundle,
            entry_source,
            namespace,
            requested,
            conversions,
            operation,
        )?;
        let observation = crate::definitions::ecore::ParserObservation::new(operation);
        let enabled = observation.finish(
            session
                .preparation
                .holder
                .borrow_owner()
                .resolver()
                .and_then(|resolver| resolver.enable_parameter_bound_inputs(&observation)),
        );
        if let Err(error) = enabled {
            session.failure = Some(error);
            return Err(Box::new(session.finish()));
        }
        session.preparation.observe_membership = true;
        Ok(session)
    }
    /// Observe fresh membership and buffer revision-hash writes in 256-byte
    /// blocks. Original begin APIs retain their complete accounting policy.
    /// This option does not claim resumable publication or complete S09 admission.
    pub fn begin_with_buffered_revision(
        bundle: DefinitionBundle,
        entry_source: crate::definitions::bundle::SourceId,
        namespace: String,
        requested: &[RuntimeType],
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        operation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Self, Box<OwnedRuntimeSourceArrayPreparationResult>> {
        let mut session = Self::begin_with_membership_observation(
            bundle,
            entry_source,
            namespace,
            requested,
            conversions,
            operation,
        )?;
        session.preparation.buffered_revision = true;
        Ok(session)
    }
    /// Hash complete source-domain rows across advances without publishing a
    /// partial schema. Type validation yields; initialization and per-class checks
    /// retain synchronous boundaries.
    pub fn begin_with_incremental_revision(
        bundle: DefinitionBundle,
        entry_source: crate::definitions::bundle::SourceId,
        namespace: String,
        requested: &[RuntimeType],
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        operation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Self, Box<OwnedRuntimeSourceArrayPreparationResult>> {
        let mut session = Self::begin_with_buffered_revision(
            bundle,
            entry_source,
            namespace,
            requested,
            conversions,
            operation,
        )?;
        session.preparation.incremental_revision = true;
        if let Err(error) = session
            .preparation
            .holder
            .borrow_owner()
            .resolver()
            .map(|resolver| resolver.enable_attempt_classifiers())
        {
            session.failure = Some(crate::definitions::ecore::ObservedImportError::Import(
                error,
            ));
            return Err(Box::new(session.finish()));
        }
        if let Err(message) = session.preparation.storage.domains.enable_indexed_inputs() {
            session.failure = Some(crate::definitions::ecore::ObservedImportError::Import(
                session.preparation.error(message),
            ));
            return Err(Box::new(session.finish()));
        }
        Ok(session)
    }
    /// Opt-in source-local nominal constraint code and structural ancestry bitmaps.
    /// Complex generic/provider cases retain the existing source-bound checker.
    pub fn begin_with_nominal_bitmaps(
        bundle: DefinitionBundle,
        entry_source: crate::definitions::bundle::SourceId,
        namespace: String,
        requested: &[RuntimeType],
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        operation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Self, Box<OwnedRuntimeSourceArrayPreparationResult>> {
        let mut session = Self::begin_with_incremental_revision(
            bundle,
            entry_source,
            namespace,
            requested,
            conversions,
            operation,
        )?;
        session.preparation.nominal_bitmaps = true;
        Ok(session)
    }
    /// Retain bundle validation and XML/resource preparation before inventory
    /// and report execution. Canonical-source scanning, node-local checks, resolver installation and
    /// cleanup still have synchronous boundaries. No partial model is published.
    pub fn begin_with_compiled_signature_inputs(
        bundle: DefinitionBundle,
        entry_source: crate::definitions::bundle::SourceId,
        namespace: String,
        requested: &[RuntimeType],
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        operation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Self, Box<OwnedRuntimeSourceArrayPreparationResult>> {
        Self::begin_source_inputs(
            bundle,
            entry_source,
            namespace,
            requested,
            conversions,
            operation,
            false,
        )
    }
    /// Also retain canonical-source-kind traversal before bundle validation and
    /// parsing. Definition errors are deferred to advance; fixed input storage
    /// and sealed provider metadata are admitted during construction. This does
    /// not make source copies, diagnostic payloads or later resolver setup resumable.
    pub fn begin_with_retained_canonical_sources(
        bundle: DefinitionBundle,
        entry_source: crate::definitions::bundle::SourceId,
        namespace: String,
        requested: &[RuntimeType],
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        operation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Self, Box<OwnedRuntimeSourceArrayPreparationResult>> {
        Self::begin_source_inputs(
            bundle,
            entry_source,
            namespace,
            requested,
            conversions,
            operation,
            true,
        )
    }
    /// Retain legacy-reference decoding and raw/generic agreement between public
    /// advances. This explicit polling contract adds cancellation/checkpoint
    /// boundaries; original constructors preserve their full old transcripts.
    /// The contract is fixed for the complete attempt. Original nonzero semantic
    /// and macrostep policy fees remain unchanged. Current provider metadata is
    /// checked on every advance; repeated physical policy comparison and other
    /// individual compiler algorithms remain named synchronous tails.
    pub fn begin_with_retained_generic_agreement(
        bundle: DefinitionBundle,
        entry_source: crate::definitions::bundle::SourceId,
        namespace: String,
        requested: &[RuntimeType],
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        operation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Self, Box<OwnedRuntimeSourceArrayPreparationResult>> {
        let mut preparation = Self::begin_with_retained_canonical_sources(
            bundle,
            entry_source,
            namespace,
            requested,
            conversions,
            operation,
        )?;
        preparation.preparation.generic_agreement_polling = GenericAgreementPolling::Retained;
        Ok(preparation)
    }
    /// Retain node-local source-inventory agreement as well as report agreement.
    /// This construction-fixed polling contract adds explicit checkpoints/work(0)
    /// boundaries; original constructors preserve both earlier full transcripts.
    /// Original nonzero fees remain attached to their original node macrostep.
    /// Formals, individual checked resolution, repeated current-policy comparison
    /// and cache installation remain explicit synchronous boundaries.
    pub fn begin_with_retained_source_inventory_agreement(
        bundle: DefinitionBundle,
        entry_source: crate::definitions::bundle::SourceId,
        namespace: String,
        requested: &[RuntimeType],
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        operation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Self, Box<OwnedRuntimeSourceArrayPreparationResult>> {
        let mut preparation = Self::begin_with_retained_canonical_sources(
            bundle,
            entry_source,
            namespace,
            requested,
            conversions,
            operation,
        )?;
        preparation.preparation.generic_agreement_polling =
            GenericAgreementPolling::RetainedSourceInventory;
        Ok(preparation)
    }
    /// Retain GenericType source scanning in addition to inventory/report raw
    /// agreement. This additive contract is fixed for the attempt; previous
    /// constructors retain their complete original callback transcripts.
    /// Scans advance in bounded chunks. Checked reference/formal/binding walks,
    /// parameter decoding, erasure, policy revalidation and other compiler work
    /// remain explicit synchronous tails. Nonzero fees and limits are unchanged.
    pub fn begin_with_retained_generic_source_decoding(
        bundle: DefinitionBundle,
        entry_source: crate::definitions::bundle::SourceId,
        namespace: String,
        requested: &[RuntimeType],
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        operation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Self, Box<OwnedRuntimeSourceArrayPreparationResult>> {
        let mut preparation = Self::begin_with_retained_canonical_sources(
            bundle,
            entry_source,
            namespace,
            requested,
            conversions,
            operation,
        )?;
        preparation.preparation.generic_agreement_polling =
            GenericAgreementPolling::RetainedGenericSourceDecoding;
        Ok(preparation)
    }
    fn begin_source_inputs(
        bundle: DefinitionBundle,
        entry_source: crate::definitions::bundle::SourceId,
        namespace: String,
        requested: &[RuntimeType],
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        operation: &crate::definitions::conversion::InvocationContext<'_>,
        retain_canonical: bool,
    ) -> Result<Self, Box<OwnedRuntimeSourceArrayPreparationResult>> {
        match source_preparation::begin(
            bundle,
            &entry_source,
            &namespace,
            requested,
            conversions,
            operation,
            retain_canonical,
        ) {
            Ok(preparation) => Ok(Self {
                preparation,
                root: None,
                entry_source,
                namespace,
                schema: None,
                failure: None,
            }),
            Err(materialization) => Err(Box::new(OwnedRuntimeSourceArrayPreparationResult {
                entry_source,
                namespace,
                materialization: *materialization,
            })),
        }
    }
    /// Advance one report/feature stage or the explicit synchronous compiler tail.
    /// A fresh observer is used each time; any failure is terminal for this attempt.
    pub fn advance(
        &mut self,
        conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
        operation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<bool, crate::definitions::ecore::ObservedImportError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let observation = crate::definitions::ecore::ParserObservation::new(operation);
        let result = operation
            .checkpoint()
            .map_err(crate::definitions::ecore::ObservedImportError::Observation)
            .and_then(|()| {
                if self.root.is_none() {
                    let loaded = observation
                        .finish(self.preparation.advance_sources(conversions, &observation))?;
                    self.root = loaded;
                    return Ok(false);
                }
                let root = self.root.as_ref().ok_or_else(|| {
                    crate::definitions::ecore::ObservedImportError::Import(
                        self.preparation.error("Missing selected source root"),
                    )
                })?;
                if self.schema.is_some() {
                    return observation
                        .finish(self.preparation.policy.verify(
                            conversions,
                            &root.opening,
                            &observation,
                        ))
                        .map(|()| true);
                }
                observation
                    .finish(advance_owned_materialization(
                        &mut self.preparation,
                        root,
                        &self.entry_source,
                        &self.namespace,
                        conversions,
                        &observation,
                    ))
                    .map(|schema| {
                        if let Some(schema) = schema {
                            self.schema = Some(schema);
                            true
                        } else {
                            false
                        }
                    })
            });
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
    /// Transfer the freshly completed compiler result into immutable type authority.
    /// Wire exports and ordinary finish retain their existing mutable representation.
    /// Source replay, providers and model values still require their own fresh checks.
    pub fn finish_checked(
        self,
        operation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> OwnedCheckedRuntimeSourceArrayPreparationResult {
        let completed = self.finish();
        let result = completed.materialization.result.and_then(|schema| {
            crate::definitions::schema::CheckedRuntimeSchema::from_checked_compiler_output(
                schema, operation,
            )
            .map_err(|error| match error {
                crate::definitions::schema::PreparationError::Observation(error) => {
                    crate::definitions::ecore::ObservedImportError::Observation(error)
                }
                crate::definitions::schema::PreparationError::Schema(message) => {
                    crate::definitions::ecore::ObservedImportError::Import(ImportError::new(
                        ErrorKind::InvalidDefinition,
                        &SourceSpan {
                            source: completed.entry_source.clone(),
                            start: 0,
                            end: 0,
                        },
                        message,
                    ))
                }
            })
        });
        OwnedCheckedRuntimeSourceArrayPreparationResult {
            entry_source: completed.entry_source,
            namespace: completed.namespace,
            bundle: completed.materialization.bundle,
            result,
        }
    }
    /// Consume the attempt. Incomplete or failed work cannot publish a schema.
    pub fn finish(self) -> OwnedRuntimeSourceArrayPreparationResult {
        let result = match self.failure {
            Some(error) => Err(error),
            None => self.schema.ok_or_else(|| {
                crate::definitions::ecore::ObservedImportError::Import(
                    self.preparation
                        .error("Owned runtime source preparation is incomplete"),
                )
            }),
        };
        let owner = self.preparation.holder.into_owner();
        OwnedRuntimeSourceArrayPreparationResult {
            entry_source: self.entry_source,
            namespace: self.namespace,
            materialization: OwnedRuntimeSourceArrayMaterialization {
                bundle: owner.bundle,
                result,
            },
        }
    }
}
/// Synchronous consumer of the same owned preparation stages. Caller-owned
/// selection strings remain borrowed, preserving existing ownership receipts.
pub fn materialize_owned_runtime_source_array_inputs_observed(
    bundle: DefinitionBundle,
    entry_source: &crate::definitions::bundle::SourceId,
    namespace: &str,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    operation: &crate::definitions::conversion::InvocationContext<'_>,
) -> OwnedRuntimeSourceArrayMaterialization {
    let (mut preparation, root) = match begin_owned_materialization(
        bundle,
        entry_source,
        namespace,
        requested,
        conversions,
        operation,
        false,
    ) {
        Ok(prepared) => prepared,
        Err(failure) => return *failure,
    };
    let observation = crate::definitions::ecore::ParserObservation::new(operation);
    let result = (|| {
        loop {
            if let Some(schema) = advance_owned_materialization(
                &mut preparation,
                &root,
                entry_source,
                namespace,
                conversions,
                &observation,
            )? {
                return Ok(schema);
            }
        }
    })();
    let result = observation.finish(result);
    let owner = preparation.holder.into_owner();
    OwnedRuntimeSourceArrayMaterialization {
        bundle: owner.bundle,
        result,
    }
}

fn materialization_revision<'a>(
    bundle: &DefinitionBundle,
    selected: impl IntoIterator<Item = &'a String>,
    span: &SourceSpan,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<String, ImportError> {
    let mut sources = BTreeSet::new();
    for source in &bundle.sources {
        observation.work(1, &source.id)?;
        let mut dependencies = BTreeSet::new();
        for dependency in &source.dependencies {
            observation.work(1, &source.id)?;
            if !dependencies.contains(&dependency) {
                observation.reserve(128, &source.id)?;
                dependencies.insert(dependency);
            }
        }
        observation.reserve(128, &source.id)?;
        sources.insert((&source.id, dependencies));
    }
    let mut namespaces = BTreeSet::new();
    for namespace in &bundle.namespaces {
        observation.work(1, &namespace.source)?;
        let entry = (&namespace.namespace, &namespace.source);
        if !namespaces.contains(&entry) {
            observation.reserve(128, &namespace.source)?;
            namespaces.insert(entry);
        }
    }
    let mut selections = BTreeSet::new();
    for selection in selected {
        observation.work(1, &span.source)?;
        if !selections.contains(&selection) {
            observation.reserve(128, &span.source)?;
            selections.insert(selection);
        }
    }
    resource_compiler::json_digest(
        &(sources, namespaces, selections),
        &span.source,
        observation,
    )
    .map_err(|e| ImportError::new(e.kind, span, e.message))
}

pub(super) fn materialize_structure_selection_with_limits(
    selected: crate::definitions::model_definition::StructureSelection<'_>,
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    imports: Option<resource_compiler::DeclarationSelection<'_>>,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<Schema, ImportError> {
    let resources = resource_compiler::selected_context_with_cursor_policy(
        selected.bundle,
        selected.entry,
        selected.namespace,
        observation,
        limits,
        false,
    )?;
    let selection = finish_materialization_input_selection(
        selected.bundle,
        selected.entry,
        resources,
        observation,
        imports,
        true,
    )?;
    materialize_selected_with_storage(
        selected.bundle,
        selected.entry,
        selected.namespace,
        selected.materialized_types,
        conversions,
        &mut OwnedAlternatives,
        observation,
        selection,
    )
}

/// Materialize a checked package selection without requiring grammar or semantic sources.
/// This uses the same reachable resources, namespace and revision as package compilation.
pub fn materialize_runtime_bundle(
    bundle: &DefinitionBundle,
    entry_source: &crate::definitions::bundle::SourceId,
    namespace: &str,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
) -> Result<Schema, ImportError> {
    materialize_runtime_bundle_context(
        bundle,
        entry_source,
        namespace,
        requested,
        conversions,
        &crate::definitions::ecore::ParserObservation::new(
            &crate::definitions::conversion::InvocationContext::default(),
        ),
    )
}
/// Materialize the same checked runtime selection with an explicit source-size
/// policy. Admitted parsed resources are passed through to the shared owner;
/// this never silently replaces a larger caller policy with the default ceiling.
pub fn materialize_runtime_bundle_with_limits(
    bundle: &DefinitionBundle,
    entry_source: &crate::definitions::bundle::SourceId,
    namespace: &str,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<Schema, ImportError> {
    resource_compiler::require_canonical_sources(bundle)?;
    let context = crate::definitions::conversion::InvocationContext::default();
    let observation = crate::definitions::ecore::ParserObservation::new(&context);
    let selected = resource_compiler::selected_context_with_cursor_policy(
        bundle,
        entry_source,
        namespace,
        &observation,
        limits,
        false,
    )?;
    let selection = finish_materialization_input_selection(
        bundle,
        entry_source,
        selected,
        &observation,
        None,
        false,
    )?;
    materialize_selected_with_storage(
        bundle,
        entry_source,
        namespace,
        requested,
        conversions,
        &mut OwnedAlternatives,
        &observation,
        selection,
    )
}
/// Observe resource admission, package selection and selected source copying.
/// Generic resolver preparation and schema materialization remain synchronous
/// allocation boundaries, including their additional compatibility resource sets.
pub fn materialize_runtime_bundle_observed(
    bundle: &DefinitionBundle,
    entry_source: &crate::definitions::bundle::SourceId,
    namespace: &str,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    context: &crate::definitions::conversion::InvocationContext<'_>,
) -> Result<Schema, crate::definitions::ecore::ObservedImportError> {
    context
        .checkpoint()
        .map_err(crate::definitions::ecore::ObservedImportError::Observation)?;
    let observation = crate::definitions::ecore::ParserObservation::new(context);
    observation.finish(materialize_runtime_bundle_context(
        bundle,
        entry_source,
        namespace,
        requested,
        conversions,
        &observation,
    ))
}
fn materialize_runtime_bundle_context(
    bundle: &DefinitionBundle,
    entry_source: &crate::definitions::bundle::SourceId,
    namespace: &str,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<Schema, ImportError> {
    materialize_runtime_bundle_context_with_storage(
        bundle,
        entry_source,
        namespace,
        requested,
        conversions,
        &mut OwnedAlternatives,
        observation,
    )
}
/// Compile complete shared schema inputs directly from the exact source bundle.
/// Candidate/provider checks use the same materializer; only key ownership differs.
pub fn materialize_runtime_bundle_inputs_observed(
    bundle: &DefinitionBundle,
    entry_source: &crate::definitions::bundle::SourceId,
    namespace: &str,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    context: &crate::definitions::conversion::InvocationContext<'_>,
) -> Result<
    crate::definitions::schema::RuntimeSchemaInputs,
    crate::definitions::ecore::ObservedImportError,
> {
    context
        .checkpoint()
        .map_err(crate::definitions::ecore::ObservedImportError::Observation)?;
    let observation = crate::definitions::ecore::ParserObservation::new(context);
    observation.finish(materialize_runtime_bundle_context_with_storage(
        bundle,
        entry_source,
        namespace,
        requested,
        conversions,
        &mut SharedAlternatives::default(),
        &observation,
    ))
}
/// Compile complete shared source-domain inputs using the same materializer.
pub fn materialize_runtime_domain_inputs_observed(
    bundle: &DefinitionBundle,
    entry_source: &crate::definitions::bundle::SourceId,
    namespace: &str,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    context: &crate::definitions::conversion::InvocationContext<'_>,
) -> Result<
    crate::definitions::schema::RuntimeDomainSchemaInputs,
    crate::definitions::ecore::ObservedImportError,
> {
    context
        .checkpoint()
        .map_err(crate::definitions::ecore::ObservedImportError::Observation)?;
    let observation = crate::definitions::ecore::ParserObservation::new(context);
    observation.finish(materialize_runtime_bundle_context_with_storage(
        bundle,
        entry_source,
        namespace,
        requested,
        conversions,
        &mut SharedDomainAlternatives::default(),
        &observation,
    ))
}
/// Complete immutable alternative arrays. This is input ownership, not cached admission.
pub fn materialize_runtime_array_inputs_observed(
    bundle: &DefinitionBundle,
    entry_source: &crate::definitions::bundle::SourceId,
    namespace: &str,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    context: &crate::definitions::conversion::InvocationContext<'_>,
) -> Result<
    crate::definitions::schema::RuntimeArraySchemaInputs,
    crate::definitions::ecore::ObservedImportError,
> {
    context
        .checkpoint()
        .map_err(crate::definitions::ecore::ObservedImportError::Observation)?;
    let observation = crate::definitions::ecore::ParserObservation::new(context);
    observation.finish(materialize_runtime_bundle_context_with_storage(
        bundle,
        entry_source,
        namespace,
        requested,
        conversions,
        &mut SharedArrayAlternatives::<crate::definitions::schema::SharedRuntimeArrayTarget>::default(),
        &observation,
    ))
}
/// Full shared inputs selecting fresh ordered scanning and indexed fallback.
pub fn materialize_runtime_array_view_inputs_observed(
    bundle: &DefinitionBundle,
    entry_source: &crate::definitions::bundle::SourceId,
    namespace: &str,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    context: &crate::definitions::conversion::InvocationContext<'_>,
) -> Result<
    crate::definitions::schema::RuntimeArrayViewSchemaInputs,
    crate::definitions::ecore::ObservedImportError,
> {
    context
        .checkpoint()
        .map_err(crate::definitions::ecore::ObservedImportError::Observation)?;
    let observation = crate::definitions::ecore::ParserObservation::new(context);
    observation.finish(materialize_runtime_bundle_context_with_storage(
        bundle,
        entry_source,
        namespace,
        requested,
        conversions,
        &mut SharedArrayAlternatives::<crate::definitions::schema::SharedRuntimeArrayViewTarget>::default(),
        &observation,
    ))
}

pub fn materialize_runtime_source_array_inputs_observed(
    bundle: &DefinitionBundle,
    entry_source: &crate::definitions::bundle::SourceId,
    namespace: &str,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    context: &crate::definitions::conversion::InvocationContext<'_>,
) -> Result<
    crate::definitions::schema::RuntimeSourceArraySchemaInputs,
    crate::definitions::ecore::ObservedImportError,
> {
    context
        .checkpoint()
        .map_err(crate::definitions::ecore::ObservedImportError::Observation)?;
    let observation = crate::definitions::ecore::ParserObservation::new(context);
    observation.finish(
        materialize_runtime_bundle_context_with_storage(
            bundle,
            entry_source,
            namespace,
            requested,
            conversions,
            &mut SharedArrayAlternatives::<
                crate::definitions::schema::SharedRuntimeSourceArrayTarget,
            >::default(),
            &observation,
        ),
    )
}
fn materialize_runtime_bundle_context_with_storage<P: AlternativeStorage>(
    bundle: &DefinitionBundle,
    entry_source: &crate::definitions::bundle::SourceId,
    namespace: &str,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    storage: &mut P,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<crate::definitions::schema::SchemaRepresentation<P::Target>, ImportError> {
    let selection =
        select_materialization_inputs(bundle, entry_source, namespace, observation, None)?;
    materialize_selected_with_storage(
        bundle,
        entry_source,
        namespace,
        requested,
        conversions,
        storage,
        observation,
        selection,
    )
}

// The same selected resource owner feeds every storage policy and package caller.
#[allow(clippy::too_many_arguments)]
fn materialize_selected_with_storage<P: AlternativeStorage>(
    bundle: &DefinitionBundle,
    entry_source: &crate::definitions::bundle::SourceId,
    namespace: &str,
    requested: &[RuntimeType],
    conversions: Option<&crate::definitions::conversion::ConversionRegistry<'_>>,
    storage: &mut P,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    selection: SelectedMaterialization,
) -> Result<crate::definitions::schema::SchemaRepresentation<P::Target>, ImportError> {
    let selected = selection.subset.as_ref().unwrap_or(bundle);
    let mut schema = materialize_context_with_storage_observed(
        selected,
        requested,
        conversions,
        selection.resources,
        storage,
        observation,
    )?;
    finish_materialization_selection(
        &mut schema,
        &selection.root,
        entry_source,
        namespace,
        observation,
    )?;
    Ok(schema)
}
struct SelectedMaterialization {
    root: Xml,
    resources: Option<ResourceSet>,
    subset: Option<DefinitionBundle>,
}
fn select_materialization_inputs(
    bundle: &DefinitionBundle,
    entry_source: &crate::definitions::bundle::SourceId,
    namespace: &str,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    package_imports: Option<resource_compiler::DeclarationSelection<'_>>,
) -> Result<SelectedMaterialization, ImportError> {
    select_materialization_inputs_with_cursor_policy(
        bundle,
        entry_source,
        namespace,
        observation,
        package_imports,
        false,
    )
}
fn select_materialization_inputs_with_cursor_policy(
    bundle: &DefinitionBundle,
    entry_source: &crate::definitions::bundle::SourceId,
    namespace: &str,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    package_imports: Option<resource_compiler::DeclarationSelection<'_>>,
    borrowed_cursors: bool,
) -> Result<SelectedMaterialization, ImportError> {
    resource_compiler::require_canonical_sources(bundle)?;
    let (resources, entry, reachable) = if borrowed_cursors {
        resource_compiler::selected_context_with_cursor_policy(
            bundle,
            entry_source,
            namespace,
            observation,
            crate::definitions::ecore::ParseLimits::default(),
            true,
        )?
    } else {
        resource_compiler::selected_context_observed(bundle, entry_source, namespace, observation)?
    };
    finish_materialization_input_selection(
        bundle,
        entry_source,
        (resources, entry, reachable),
        observation,
        package_imports,
        false,
    )
}
fn finish_materialization_input_selection(
    bundle: &DefinitionBundle,
    entry_source: &crate::definitions::bundle::SourceId,
    selected: (
        ResourceSet,
        DeclarationId,
        BTreeSet<crate::definitions::bundle::SourceId>,
    ),
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    package_imports: Option<resource_compiler::DeclarationSelection<'_>>,
    reuse_subset: bool,
) -> Result<SelectedMaterialization, ImportError> {
    let (mut resources, entry, reachable) = selected;
    if let Some(imports) = package_imports {
        resource_compiler::retain_declarations(&resources, &reachable, imports, observation)?;
    }
    let document = resources.document(&entry.source).map_err(|e| {
        ImportError::new(
            ErrorKind::InvalidDefinition,
            &SourceSpan {
                source: entry.source.clone(),
                start: 0,
                end: 0,
            },
            e,
        )
    })?;
    let node = resources
        .node(&entry)
        .map_err(|e| ImportError::new(ErrorKind::InvalidDefinition, &document.root.span, e))?;
    let root =
        Xml::header_from_node_at_observed(node, &document.source.text, entry.path, observation)?;
    let reachable = if document.source.kind == SourceKind::Rcore {
        std::borrow::Cow::Owned(resource_compiler::complete_dependencies(
            bundle,
            &reachable,
            observation,
        )?)
    } else {
        std::borrow::Cow::Borrowed(&reachable)
    };
    // Reuse is exact only when selection keeps the complete input bundle.
    // Subsets preserve their original source/namespace filtering and parse path.
    let mut complete_selection = true;
    for source in &bundle.sources {
        observation.work(1, &source.id)?;
        if !reachable.contains(&source.id) {
            complete_selection = false;
            break;
        }
    }
    if complete_selection {
        Ok(SelectedMaterialization {
            root,
            resources: Some(resources),
            subset: None,
        })
    } else {
        observation.reserve(128, entry_source)?;
        let mut sources = Vec::new();
        for source in &bundle.sources {
            observation.work(1, &source.id)?;
            if reachable.contains(&source.id) {
                observation.serialized(source, &source.id)?;
                observation.reserve(32, &source.id)?;
                sources.push(source.clone());
            }
        }
        let mut namespaces = Vec::new();
        for binding in &bundle.namespaces {
            observation.work(1, &binding.source)?;
            if reachable.contains(&binding.source) {
                observation.serialized(binding, &binding.source)?;
                observation.reserve(32, &binding.source)?;
                namespaces.push(binding.clone());
            }
        }
        let selected = DefinitionBundle {
            version: bundle.version,
            sources,
            namespaces,
        };
        if reuse_subset {
            resources.retain_selected_observed(&reachable, observation)?;
        }
        Ok(SelectedMaterialization {
            root,
            resources: reuse_subset.then_some(resources),
            subset: Some(selected),
        })
    }
}
fn finish_materialization_selection<A: crate::definitions::schema::RuntimeAlternative>(
    schema: &mut crate::definitions::schema::SchemaRepresentation<A>,
    root: &Xml,
    entry_source: &crate::definitions::bundle::SourceId,
    namespace: &str,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<(), ImportError> {
    schema.uri = required(root, "nsURI")?;
    schema.name = required(root, "name")?;
    schema.revision = resource_compiler::json_digest(
        &(&schema.revision, entry_source, namespace),
        &root.opening.source,
        observation,
    )
    .map_err(|e| root.error(e.message))?;
    Ok(())
}

#[cfg(test)]
mod admission_tests {
    use super::*;

    #[test]
    fn revision_sets_preserve_literal_order_deduplication_and_observed_boundaries() {
        use crate::definitions::{
            bundle::{DefinitionSource, NamespaceBinding, SourceKind},
            conversion::{InvocationContext, InvocationObserver, ProviderId},
            ecore::ObservedImportError,
        };
        use std::{cell::Cell, rc::Rc};
        struct Probe {
            storage: Cell<usize>,
            work: Cell<usize>,
            calls: Cell<usize>,
            max_storage: usize,
            max_work: usize,
            cancel: Option<usize>,
        }
        impl Probe {
            fn new(storage: usize, work: usize, cancel: Option<usize>) -> Rc<Self> {
                Rc::new(Self {
                    storage: Cell::new(0),
                    work: Cell::new(0),
                    calls: Cell::new(0),
                    max_storage: storage,
                    max_work: work,
                    cancel,
                })
            }
            fn checkpoint(&self) -> Result<(), String> {
                let call = self.calls.get();
                self.calls.set(call + 1);
                if self.cancel == Some(call) {
                    Err("revision cancelled".into())
                } else {
                    Ok(())
                }
            }
        }
        impl InvocationObserver for Probe {
            fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
                panic!("revision hashing cannot invoke providers")
            }
            fn work(&self, units: usize) -> Result<(), String> {
                self.checkpoint()?;
                let next = self.work.get() + units;
                if next > self.max_work {
                    return Err("revision work denied".into());
                }
                self.work.set(next);
                Ok(())
            }
            fn retain_storage(&self, bytes: usize) -> Result<(), String> {
                self.checkpoint()?;
                let next = self.storage.get() + bytes;
                if next > self.max_storage {
                    return Err("revision storage denied".into());
                }
                self.storage.set(next);
                Ok(())
            }
        }
        let mut a = DefinitionSource::new("memory:a", SourceKind::Ecore, "<a/>");
        let b = DefinitionSource::new("memory:b", SourceKind::Ecore, "<b/>");
        a.dependencies = vec![b.id.clone(), b.id.clone()];
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![b.clone(), a.clone()],
            namespaces: vec![
                NamespaceBinding {
                    namespace: "urn:b".into(),
                    source: b.id.clone(),
                },
                NamespaceBinding {
                    namespace: "urn:a".into(),
                    source: a.id.clone(),
                },
            ],
        };
        let span = SourceSpan {
            source: a.id.clone(),
            start: 4,
            end: 8,
        };
        let selected = ["B".to_owned(), "A".to_owned(), "A".to_owned()];
        let run = |bundle: &DefinitionBundle, probe: Rc<Probe>| {
            let context = InvocationContext::new(probe);
            let observation = crate::definitions::ecore::ParserObservation::new(&context);
            observation.finish(materialization_revision(
                bundle,
                &selected,
                &span,
                &observation,
            ))
        };
        // Seven unique set entries *128, plus digest string64+64 =1024.
        // Nine input visits (including repeated dependency/selection), 126
        // compact JSON emissions and one formatting unit =136. Python hashlib
        // independently pins the 519-byte sorted/deduplicated JSON sequence.
        let expected = "8897c20696c91b672b3f0e27faf5210c8104ed160fcb86cb5d445610cb0180bb";
        let full = Probe::new(1024, 136, None);
        assert_eq!(run(&bundle, full.clone()).unwrap(), expected);
        assert_eq!((full.storage.get(), full.work.get()), (1024, 136));
        for prefix in [128, 256, 384, 512, 640, 768, 896, 1024] {
            assert_eq!(
                run(&bundle, Probe::new(prefix - 1, usize::MAX, None)).unwrap_err(),
                ObservedImportError::Observation("revision storage denied".into())
            );
        }
        for work in [135, 136, 137] {
            let result = run(&bundle, Probe::new(1024, work, None));
            if work < 136 {
                assert_eq!(
                    result.unwrap_err(),
                    ObservedImportError::Observation("revision work denied".into())
                );
            } else {
                assert_eq!(result.unwrap(), expected);
            }
        }
        assert_eq!(run(&bundle, Probe::new(1025, 137, None)).unwrap(), expected);
        for call in 0..full.calls.get() {
            assert_eq!(
                run(&bundle, Probe::new(1024, 136, Some(call))).unwrap_err(),
                ObservedImportError::Observation("revision cancelled".into())
            );
            assert_eq!(run(&bundle, Probe::new(1024, 136, None)).unwrap(), expected);
        }
        let mut reordered = bundle.clone();
        reordered.sources.reverse();
        reordered.namespaces.reverse();
        assert_eq!(
            run(&reordered, Probe::new(1024, 136, None)).unwrap(),
            expected
        );
        assert_eq!(bundle.sources[1].dependencies, vec![b.id.clone(), b.id]);
        assert_eq!(bundle.sources[1].text, "<a/>");
        assert_eq!((span.start, span.end), (4, 8));
    }

    #[test]
    fn inherited_reference_materialization_keeps_observed_schema_identity() {
        use crate::definitions::{
            bundle::{DefinitionSource, SourceKind},
            conversion::{InvocationContext, InvocationObserver, ProviderId},
        };
        use std::{cell::Cell, rc::Rc};
        struct Storage {
            used: Cell<usize>,
            limit: usize,
        }
        impl InvocationObserver for Storage {
            fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
                panic!("no providers")
            }
            fn retain_storage(&self, bytes: usize) -> Result<(), String> {
                let next = self.used.get() + bytes;
                if next > self.limit {
                    return Err("storage denied".into());
                }
                self.used.set(next);
                Ok(())
            }
        }
        let features = (0..4).map(|i| format!("<eStructuralFeatures x:type='e:EReference' name='r{i}'><eGenericType eTypeParameter='#//Root/T'/></eStructuralFeatures>")).collect::<String>();
        let children = (0..7)
            .map(|i| format!("<eClassifiers x:type='e:EClass' name='C{i}' eSuperTypes='#//Root'/>"))
            .collect::<String>();
        let source = DefinitionSource::new(
            "memory:job-expansion.ecore",
            SourceKind::Ecore,
            format!(
                "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:job-expansion'><eClassifiers x:type='e:EClass' name='Root'><eTypeParameters name='T'><eBounds eClassifier='#//Root'/></eTypeParameters>{features}</eClassifiers>{children}</e:EPackage>"
            ),
        );
        let source = crate::definitions::migration_test_fixture::source(source);
        let requested = (0..8)
            .map(|i| {
                RuntimeType::new(Type::Classifier {
                    declaration: DeclarationId {
                        source: source.id.clone(),
                        path: vec![i],
                    },
                    arguments: vec![],
                    raw: i == 0,
                })
                .unwrap()
            })
            .collect::<Vec<_>>();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let plain_resolver = Resolver::new(&bundle).unwrap();
        let mut resolver = Resolver::new(&bundle).unwrap();
        let unobserved = InvocationContext::default();
        let observation = crate::definitions::ecore::ParserObservation::new(&unobserved);
        resolver
            .resolve_materialization_inventory_observed(&observation)
            .unwrap();
        for receiver in &requested {
            let prepared = resolver
                .prepare_runtime_members_observed(receiver, &observation)
                .unwrap();
            let mut public = plain_resolver.runtime_members(receiver).unwrap();
            assert_eq!(public.source_members.len(), 4);
            assert!(prepared.source_members.is_empty());
            public.source_members.clear();
            assert_eq!(*prepared, public);
            for checked in prepared.checked_feature_domains() {
                for candidate in &requested {
                    assert_eq!(
                        checked.accepts(candidate),
                        plain_resolver.runtime_feature_accepts(
                            &checked.source().owned_copy_observed(&observation).unwrap(),
                            candidate
                        )
                    );
                }
                // A clone loses the private proof. Public input is always rechecked.
                for case in 0..4 {
                    let mut forged = checked.source().owned_copy_observed(&observation).unwrap();
                    match case {
                        0 => forged.reference = false,
                        1 => forged.expression = requested[1].expression().clone(),
                        2 => forged.projection.source.name.push_str("_forged"),
                        _ => forged.projection.source.declaration.source.sha256 = "0".repeat(64),
                    }
                    assert!(
                        plain_resolver
                            .runtime_feature_accepts(&forged, &requested[1])
                            .is_err()
                    );
                }
            }
        }
        let receipt = Rc::new(Storage {
            used: Cell::new(0),
            limit: usize::MAX,
        });
        let context = InvocationContext::new(receipt.clone());
        let observed = materialize_runtime_bundle_observed(
            &bundle,
            &source.id,
            "urn:job-expansion",
            &requested,
            None,
            &context,
        )
        .unwrap();
        let plain =
            materialize_runtime_bundle(&bundle, &source.id, "urn:job-expansion", &requested, None)
                .unwrap();
        assert_eq!(
            serde_json::to_value(&observed).unwrap(),
            serde_json::to_value(plain).unwrap()
        );
        let validation_receipt = Rc::new(Storage {
            used: Cell::new(0),
            limit: usize::MAX,
        });
        let validation_context = InvocationContext::new(validation_receipt.clone());
        let prepared = crate::definitions::schema::ValidatedRuntimeTypes::prepare_observed(
            &observed,
            &validation_context,
        )
        .unwrap();
        let validation_bytes = validation_receipt.used.get();
        assert!(validation_bytes > 0);
        assert!(prepared.belongs_to(&observed));
        let copied_schema = observed.clone();
        assert!(!prepared.belongs_to(&copied_schema));
        for receiver in &requested {
            assert_eq!(
                prepared
                    .class_key_for_type_observed(receiver, &validation_context)
                    .unwrap(),
                observed.class_key_for_type(receiver).unwrap(),
            );
        }
        let lookup_bytes = validation_receipt.used.get() - validation_bytes;
        assert!(lookup_bytes > 0 && lookup_bytes < validation_bytes);
        assert_eq!(
            copied_schema
                .validate_declared_operation_inventory_prepared(
                    copied_schema.classes.keys().next().unwrap(),
                    &prepared,
                    &validation_context,
                )
                .unwrap_err()
                .to_string(),
            "Runtime type validation belongs to a different schema",
        );
        let denied = Rc::new(Storage {
            used: Cell::new(0),
            limit: validation_bytes - 1,
        });
        assert!(matches!(
            crate::definitions::schema::ValidatedRuntimeTypes::prepare_observed(
                &observed,
                &InvocationContext::new(denied),
            ),
            Err(crate::definitions::schema::PreparationError::Observation(_)),
        ));
        let mut forged = observed.clone();
        let domain = forged
            .runtime_feature_domains
            .values_mut()
            .next()
            .unwrap()
            .first_mut()
            .unwrap();
        domain.alternatives.push(domain.alternatives[0].clone());
        let error = match crate::definitions::schema::ValidatedRuntimeTypes::prepare_observed(
            &forged,
            &InvocationContext::default(),
        ) {
            Ok(_) => panic!("forged schema cannot acquire validation proof"),
            Err(error) => error.to_string(),
        };
        assert_eq!(error, "Duplicate runtime domain alternative");
        assert_eq!(forged.class_key_for_type(&requested[0]).unwrap_err(), error);
        let domains = &observed.runtime_feature_domains;
        assert_eq!(domains.len(), 8);
        for fields in domains.values() {
            assert_eq!(fields.len(), 4);
            for field in fields {
                assert_eq!(field.alternatives.len(), 8);
            }
        }
        // Denial must still propagate through the typed observation boundary.
        let denied = InvocationContext::new(Rc::new(Storage {
            used: Cell::new(0),
            limit: receipt.used.get() - 1,
        }));
        assert_eq!(
            materialize_runtime_bundle_observed(
                &bundle,
                &source.id,
                "urn:job-expansion",
                &requested,
                None,
                &denied
            )
            .unwrap_err(),
            crate::definitions::ecore::ObservedImportError::Observation("storage denied".into())
        );
    }

    #[test]
    fn pending_and_processed_type_identities_share_one_admission_ceiling() {
        let source = standard_datatype_source().id;
        let span = SourceSpan {
            source: source.clone(),
            start: 0,
            end: 0,
        };
        let ty = |index| {
            RuntimeType::new(Type::Classifier {
                declaration: DeclarationId {
                    source: source.clone(),
                    path: vec![index],
                },
                arguments: vec![],
                raw: false,
            })
            .unwrap()
        };
        let mut pending = Vec::new();
        let mut discovered = BTreeSet::new();
        for index in 0..4096 {
            enqueue_type(ty(index), &mut pending, &mut discovered, &span).unwrap();
        }
        assert_eq!(pending.len(), 4096);
        // Repeated inheritance paths add neither pending storage nor new identities.
        for _ in 0..8 {
            for index in 0..4096 {
                enqueue_type(ty(index), &mut pending, &mut discovered, &span).unwrap();
            }
        }
        assert_eq!(pending.len(), 4096);
        assert_eq!(discovered.len(), 4096);
        pending.clear(); // Processing does not refund the discovered-type budget.
        enqueue_type(ty(0), &mut pending, &mut discovered, &span).unwrap();
        assert!(pending.is_empty());
        let error = enqueue_type(ty(4096), &mut pending, &mut discovered, &span).unwrap_err();
        assert_eq!(error.kind, ErrorKind::ResourceLimit);
        assert_eq!(error.message, "Materialization type budget exceeded");
        assert!(pending.is_empty());
        assert_eq!(discovered.len(), 4096);
        let mut fresh = BTreeSet::new();
        enqueue_type(ty(4096), &mut pending, &mut fresh, &span).unwrap();
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn overlapping_ancestor_requests_preserve_the_complete_materialized_schema() {
        use crate::definitions::bundle::{DefinitionSource, SourceKind};
        let source = DefinitionSource::new(
            "memory:diamond.ecore",
            SourceKind::Ecore,
            r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:diamond"><eClassifiers x:type="e:EClass" name="Root"/><eClassifiers x:type="e:EClass" name="Left" eSuperTypes="#//Root"/><eClassifiers x:type="e:EClass" name="Right" eSuperTypes="#//Root"/><eClassifiers x:type="e:EClass" name="Leaf" eSuperTypes="#//Left #//Right"/></e:EPackage>"##,
        );
        let source = crate::definitions::migration_test_fixture::source(source);
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let ty = |index| {
            RuntimeType::new(Type::Classifier {
                declaration: DeclarationId {
                    source: source.id.clone(),
                    path: vec![index],
                },
                arguments: vec![],
                raw: false,
            })
            .unwrap()
        };
        let baseline = materialize_runtime_types(&bundle, &[ty(3)], None).unwrap();
        assert_eq!(baseline.classes.len(), 4);
        for order in [[3, 2, 1, 3, 0], [0, 3, 1, 2, 3]] {
            let requested = order.into_iter().map(ty).collect::<Vec<_>>();
            let schema = materialize_runtime_types(&bundle, &requested, None).unwrap();
            assert_eq!(
                serde_json::to_value(schema).unwrap(),
                serde_json::to_value(&baseline).unwrap()
            );
        }
    }
}

#[cfg(test)]
mod readonly_xml_controls {
    use super::*;
    use crate::definitions::conversion::{InvocationContext, InvocationObserver, ProviderId};
    use std::{cell::Cell, rc::Rc};
    fn fixture(foreign: bool) -> DefinitionBundle {
        let child = if foreign {
            "<f:thing xmlns:f='urn:foreign'/>"
        } else {
            "<eAnnotations source='doc'><details key='ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¾Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Â¦Ãƒâ€šÃ‚Â¡ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â©ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â°ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¦ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¸ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¹ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â¦ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Â¦ÃƒÂ¢Ã¢â€šÂ¬Ã…â€œÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â¦ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¡ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬' value='retained payload'/></eAnnotations><eStructuralFeatures xsi:type='ecore:EAttribute' name='label' eType='ecore:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString'/><eOperations name='echo'><eAnnotations source='ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¾Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Â¦Ãƒâ€šÃ‚Â¡ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â©ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â°ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¦ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¸ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¹ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â¦ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Â¦ÃƒÂ¢Ã¢â€šÂ¬Ã…â€œÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â¦ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¡ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬'/></eOperations>"
        };
        DefinitionBundle {
            version: 1,
            namespaces: vec![],
            sources: vec![DefinitionSource::new(
                "memory:readonly-projection.ecore",
                SourceKind::Ecore,
                format!(
                    "<ecore:EPackage xmlns:ecore='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:readonly'><eClassifiers xsi:type='ecore:EClass' name='Root'>{child}</eClassifiers></ecore:EPackage>"
                ),
            )],
        }
    }
    fn context<'a>(
        bundle: &'a DefinitionBundle,
        resolver: &'a Resolver,
    ) -> MaterializationContext<'a> {
        let (resources, standard, standard_id) = resolver.resource_sets();
        MaterializationContext {
            source_inventory: None,
            reuse_compiler_type_inputs: false,
            pending_revision: None,
            progress: AssemblyProgress::Empty,
            readonly_xml: Default::default(),
            open_features: Default::default(),
            reference_top: None,
            bundle,
            resolver,
            resources,
            standard,
            standard_id,
            types: Default::default(),
            enums: Default::default(),
            datatypes: Default::default(),
            declarations: Default::default(),
        }
    }
    fn same_xml(left: &Xml, right: &Xml) {
        assert_eq!(left.name, right.name);
        assert_eq!(left.attrs, right.attrs);
        assert_eq!(left.attr_spans, right.attr_spans);
        assert_eq!(left.opening, right.opening);
        assert_eq!(left.payload, right.payload);
        assert_eq!(left.declaration, right.declaration);
        assert_eq!(left.source_payload, right.source_payload);
        assert_eq!(left.children.len(), right.children.len());
        for (a, b) in left.children.iter().zip(&right.children) {
            same_xml(a, b);
        }
    }
    struct Meter {
        bytes: Cell<usize>,
        work: Cell<usize>,
        limit: usize,
        deny_work: Option<usize>,
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            panic!("XML projection cannot invoke providers")
        }
        fn retain_storage(&self, bytes: usize) -> Result<(), String> {
            let next = self
                .bytes
                .get()
                .checked_add(bytes)
                .ok_or("XML storage overflow")?;
            if next > self.limit {
                return Err("XML storage denied".into());
            }
            self.bytes.set(next);
            Ok(())
        }
        fn work(&self, steps: usize) -> Result<(), String> {
            let next = self
                .work
                .get()
                .checked_add(steps)
                .ok_or("XML work overflow")?;
            if self.deny_work.is_some_and(|limit| next >= limit) {
                return Err("XML work denied".into());
            }
            self.work.set(next);
            Ok(())
        }
    }
    #[test]
    fn compiler_type_inputs_share_full_descriptors_and_observe_warm_admission() {
        let bundle = fixture(false);
        let source = &bundle.sources[0];
        let initial = InvocationContext::default();
        let initial_obs = crate::definitions::ecore::ParserObservation::new(&initial);
        let mut resolver = Resolver::new(&bundle).unwrap();
        resolver
            .resolve_materialization_inventory_observed(&initial_obs)
            .unwrap();
        resolver
            .enable_parameter_bound_inputs(&initial_obs)
            .unwrap();
        let expression = Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![0],
            },
            arguments: vec![],
            raw: true,
        };
        let mut state = context(&bundle, &resolver);
        state.reuse_compiler_type_inputs = true;
        let first = Context {
            state: &mut state,
            observation: &initial_obs,
        }
        .runtime_type(&expression, &source.id, |e| {
            ImportError::new(
                ErrorKind::Unsupported,
                &SourceSpan {
                    source: source.id.clone(),
                    start: 0,
                    end: 0,
                },
                e,
            )
        })
        .unwrap();
        let run = |state: &mut MaterializationContext<'_>, limit, deny_work| {
            let meter = Rc::new(Meter {
                bytes: Cell::new(0),
                work: Cell::new(0),
                limit,
                deny_work,
            });
            let invocation = InvocationContext::new(meter.clone());
            let obs = crate::definitions::ecore::ParserObservation::new(&invocation);
            let result = Context {
                state,
                observation: &obs,
            }
            .runtime_type(&expression, &source.id, |e| {
                ImportError::new(
                    ErrorKind::Unsupported,
                    &SourceSpan {
                        source: source.id.clone(),
                        start: 0,
                        end: 0,
                    },
                    e,
                )
            });
            (obs.finish(result), meter.bytes.get(), meter.work.get())
        };
        let (result, bytes, work) = run(&mut state, usize::MAX, None);
        let second = result.unwrap();
        assert_eq!(first, second);
        assert!(std::ptr::eq(first.expression(), second.expression()));
        assert!(std::ptr::eq(first.identity(), second.identity()));
        assert!(bytes > 0 && work > 0);
        assert!(run(&mut state, bytes, None).0.is_ok());
        assert!(run(&mut state, bytes - 1, None).0.is_err());
        for stop in 0..=work {
            assert!(run(&mut state, usize::MAX, Some(stop)).0.is_err());
        }
        assert_eq!(run(&mut state, bytes, None).0.unwrap(), first);
        let invalid = Type::Parameter {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![0, 0],
            },
        };
        let failure = Context {
            state: &mut state,
            observation: &initial_obs,
        }
        .runtime_type(&invalid, &source.id, |e| {
            ImportError::new(
                ErrorKind::Unsupported,
                &SourceSpan {
                    source: source.id.clone(),
                    start: 0,
                    end: 0,
                },
                e,
            )
        })
        .unwrap_err();
        assert_eq!(failure.kind, ErrorKind::Unsupported);
    }
    #[test]
    fn full_readonly_xml_shares_every_field_and_keeps_mutable_generation_independent() {
        let bundle = fixture(false);
        let source = bundle.sources[0].clone();
        let resolver = Resolver::new(&bundle).unwrap();
        let meter = Rc::new(Meter {
            bytes: Cell::new(0),
            work: Cell::new(0),
            limit: usize::MAX,
            deny_work: None,
        });
        let invocation = InvocationContext::new(meter.clone());
        let observation = crate::definitions::ecore::ParserObservation::new(&invocation);
        let mut state = context(&bundle, &resolver);
        let context = Context {
            state: &mut state,
            observation: &observation,
        };
        let id = DeclarationId {
            source: source.id.clone(),
            path: vec![0],
        };
        let first = context.xml_read(&id).unwrap();
        let bytes = meter.bytes.get();
        let second = context.xml_read(&id).unwrap();
        assert!(Rc::ptr_eq(&first, &second));
        assert_eq!(meter.bytes.get(), bytes);
        assert_eq!(context.state.readonly_xml.borrow().len(), 1);
        let mut generated = context.xml(&id).unwrap();
        same_xml(&first, &generated);
        assert_eq!(first.children.len(), 3);
        assert_eq!(
            first.children[0].children[0].attrs["key"],
            "ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¾Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Â¦Ãƒâ€šÃ‚Â¡ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â©ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â°ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¦ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¸ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¹ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â¦ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Â¦ÃƒÂ¢Ã¢â€šÂ¬Ã…â€œÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â¦ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¡ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬"
        );
        assert!(
            first.children[2]
                .source_payload
                .contains("ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¾Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Â¦Ãƒâ€šÃ‚Â¡ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â©ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â°ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¦ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¸ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¹ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â¦ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Â¦ÃƒÂ¢Ã¢â€šÂ¬Ã…â€œÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â¦ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¡ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬")
        );
        generated.attrs.insert("name".into(), "Changed".into());
        generated.children.clear();
        assert_eq!(first.attrs["name"], "Root");
        assert_eq!(first.children.len(), 3);
        assert_eq!(source, bundle.sources[0]);
    }
    #[test]
    fn full_readonly_xml_precharges_publication_cancels_every_checkpoint_and_recovers() {
        let bundle = fixture(false);
        let source = bundle.sources[0].clone();
        let id = DeclarationId {
            source: source.id.clone(),
            path: vec![0],
        };
        let run = |limit, deny_work| {
            let resolver = Resolver::new(&bundle).unwrap();
            let meter = Rc::new(Meter {
                bytes: Cell::new(0),
                work: Cell::new(0),
                limit,
                deny_work,
            });
            let invocation = InvocationContext::new(meter.clone());
            let observation = crate::definitions::ecore::ParserObservation::new(&invocation);
            let mut state = context(&bundle, &resolver);
            let context = Context {
                state: &mut state,
                observation: &observation,
            };
            let result = observation.finish(context.xml_read(&id)).map(|_| ());
            let entries = context.state.readonly_xml.borrow().len();
            (result, meter.bytes.get(), meter.work.get(), entries)
        };
        let baseline = run(usize::MAX, None);
        assert_eq!(baseline.0, Ok(()));
        assert_eq!(baseline.3, 1);
        assert_eq!(run(baseline.1, None).0, Ok(()));
        let denied = run(baseline.1 - 1, None);
        assert_eq!(
            denied.0.unwrap_err(),
            crate::definitions::ecore::ObservedImportError::Observation(
                "XML storage denied".into()
            )
        );
        assert_eq!(denied.3, 0);
        for step in 1..=baseline.2 {
            let denied = run(usize::MAX, Some(step));
            assert_eq!(
                denied.0.unwrap_err(),
                crate::definitions::ecore::ObservedImportError::Observation(
                    "XML work denied".into()
                )
            );
            assert_eq!(denied.3, 0);
        }
        assert_eq!(run(baseline.1, None).0, Ok(()));
        assert_eq!(source, bundle.sources[0]);
    }
    #[test]
    fn full_readonly_xml_preserves_foreign_children_without_caching_lookup_errors() {
        let bundle = fixture(true);
        let resolver = Resolver::new(&bundle).unwrap();
        let invocation = InvocationContext::default();
        let observation = crate::definitions::ecore::ParserObservation::new(&invocation);
        let mut state = context(&bundle, &resolver);
        let context = Context {
            state: &mut state,
            observation: &observation,
        };
        let id = DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![0],
        };
        let ordinary_view = context.xml(&id).unwrap();
        let view = context.xml_read(&id).unwrap();
        same_xml(&ordinary_view, &view);
        assert!(view.children[0].name.contains("urn:foreign"));
        // Projection preserves unsupported information; it is not native support.
        let requested = RuntimeType::new(Type::Classifier {
            declaration: id.clone(),
            arguments: vec![],
            raw: false,
        })
        .unwrap();
        let failed = materialize_runtime_bundle(
            &bundle,
            &bundle.sources[0].id,
            "urn:readonly",
            &[requested],
            None,
        )
        .unwrap_err();
        assert_eq!(failed.kind, ErrorKind::Unsupported);
        assert_eq!(failed.span.source, bundle.sources[0].id);
        let missing = DeclarationId {
            source: id.source.clone(),
            path: vec![99],
        };
        let ordinary = context.xml(&missing).err().unwrap();
        assert_eq!(context.xml_read(&missing).err().unwrap(), ordinary);
        assert_eq!(context.xml_read(&missing).err().unwrap(), ordinary);
        assert_eq!(context.state.readonly_xml.borrow().len(), 1);
        assert_eq!(ordinary.span.source, bundle.sources[0].id);
    }
}

#[cfg(test)]
mod ordered_reference_candidate_controls {
    use super::*;
    use crate::definitions::{
        bundle::{DefinitionSource, SourceKind},
        conversion::{InvocationContext, InvocationObserver, ProviderId},
    };
    use std::{cell::Cell, rc::Rc};
    struct Budget {
        bytes: Cell<usize>,
        limit: usize,
        checkpoints: Cell<usize>,
        stop: usize,
    }
    impl InvocationObserver for Budget {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            panic!("candidate control cannot invoke providers")
        }
        fn checkpoint(&self) -> Result<(), String> {
            if self.checkpoints.get() == self.stop {
                return Err("ordered candidate cancelled".into());
            }
            self.checkpoints.set(self.checkpoints.get() + 1);
            Ok(())
        }
        fn retain_storage(&self, bytes: usize) -> Result<(), String> {
            let next = self
                .bytes
                .get()
                .checked_add(bytes)
                .filter(|n| *n <= self.limit)
                .ok_or("ordered candidate storage denied")?;
            self.bytes.set(next);
            Ok(())
        }
    }
    #[test]
    fn ordered_reference_alternatives_keep_complete_sort_membership_and_attributes() {
        let source = DefinitionSource::new(
            "memory:ordered-reference-candidates.ecore",
            SourceKind::Ecore,
            r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:ordered-candidates"><eClassifiers x:type="e:EClass" name="Root"><eTypeParameters name="T"><eBounds eClassifier="#//Root"/></eTypeParameters><eTypeParameters name="S"><eBounds eClassifier="http://www.eclipse.org/emf/2002/Ecore#//EString"/></eTypeParameters><eStructuralFeatures x:type="e:EReference" name="a"><eGenericType eTypeParameter="#//Root/T"/></eStructuralFeatures><eStructuralFeatures x:type="e:EReference" name="b"><eGenericType eTypeParameter="#//Root/T"/></eStructuralFeatures><eStructuralFeatures x:type="e:EAttribute" name="label" defaultValueLiteral="cafÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¾Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Â¦Ãƒâ€šÃ‚Â¡ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â©"><eGenericType eTypeParameter="#//Root/S"/></eStructuralFeatures></eClassifiers><eClassifiers x:type="e:EClass" name="Zeta" eSuperTypes="#//Root"/><eClassifiers x:type="e:EClass" name="Alpha" eSuperTypes="#//Root"/><eClassifiers x:type="e:EClass" name="Middle" eSuperTypes="#//Root"/></e:EPackage>"##,
        );
        let source = crate::definitions::migration_test_fixture::source(source);
        let requested = (0..4)
            .map(|index| {
                RuntimeType::new(Type::Classifier {
                    declaration: DeclarationId {
                        source: source.id.clone(),
                        path: vec![index],
                    },
                    arguments: vec![],
                    raw: index == 0,
                })
                .unwrap()
            })
            .collect::<Vec<_>>();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let expected = materialize_context(&bundle, &requested, None).unwrap();
        let keys = expected.classes.keys().cloned().collect::<Vec<_>>();
        assert_eq!(keys.len(), 4);
        let mut references = 0;
        let mut attributes = 0;
        for domains in expected.runtime_feature_domains.values() {
            for domain in domains {
                if domain.source.reference {
                    assert_eq!(domain.alternatives, keys);
                    assert!(domain.alternatives.windows(2).all(|rows| rows[0] < rows[1]));
                    references += 1;
                } else {
                    assert_eq!(domain.alternatives, ["EString"]);
                    assert_eq!(
                        domain.default,
                        Value::String("cafÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¾Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Â¦Ãƒâ€šÃ‚Â¡ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã†â€™ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â©".into())
                    );
                    attributes += 1;
                }
            }
        }
        assert_eq!((references, attributes), (8, 4));
        let run = |limit, stop| {
            let meter = Rc::new(Budget {
                bytes: Cell::new(0),
                limit,
                checkpoints: Cell::new(0),
                stop,
            });
            let context = InvocationContext::new(meter.clone());
            let observation = crate::definitions::ecore::ParserObservation::new(&context);
            (
                observation.finish(materialize_context_observed(
                    &bundle,
                    &requested,
                    None,
                    &observation,
                )),
                meter,
            )
        };
        let (actual, meter) = run(usize::MAX, usize::MAX);
        assert_eq!(actual.unwrap(), expected);
        let (exact, _) = run(meter.bytes.get(), usize::MAX);
        assert_eq!(
            serde_json::to_value(exact.unwrap()).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        assert!(matches!(
            run(meter.bytes.get() - 1, usize::MAX).0,
            Err(crate::definitions::ecore::ObservedImportError::Observation(
                _
            ))
        ));
        for stop in [0, meter.checkpoints.get() / 2, meter.checkpoints.get() - 1] {
            assert!(matches!(
                run(usize::MAX, stop).0,
                Err(crate::definitions::ecore::ObservedImportError::Observation(
                    _
                ))
            ));
            assert_eq!(run(usize::MAX, usize::MAX).0.unwrap(), expected);
        }
        assert_eq!(bundle.sources[0], source);
    }
}

#[cfg(test)]
mod parsed_input_adoption_controls {
    use super::*;
    use crate::definitions::ecore::ParserObservation;
    use crate::definitions::{
        bundle::{DefinitionSource, NamespaceBinding},
        conversion::InvocationContext,
    };
    fn fixture() -> (DefinitionBundle, Vec<RuntimeType>) {
        let source = DefinitionSource::new(
            "memory:parsed-adoption.ecore",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='m' nsURI='urn:parsed-adoption'><eClassifiers x:type='e:EClass' name='Root'><eStructuralFeatures x:type='e:EReference' name='links' upperBound='-1' eType='#//Root'/></eClassifiers><eClassifiers x:type='e:EClass' name='Child' eSuperTypes='#//Root'/></e:EPackage>",
        );
        let source = crate::definitions::migration_test_fixture::source(source);
        let types = (0..2)
            .map(|i| {
                RuntimeType::new(Type::Classifier {
                    declaration: DeclarationId {
                        source: source.id.clone(),
                        path: vec![i],
                    },
                    arguments: vec![],
                    raw: false,
                })
                .unwrap()
            })
            .collect();
        let bundle = DefinitionBundle {
            version: 1,
            namespaces: vec![NamespaceBinding {
                namespace: "urn:parsed-adoption".into(),
                source: source.id.clone(),
            }],
            sources: vec![source],
        };
        (bundle, types)
    }
    #[test]
    fn complete_parsed_input_adoption_moves_exact_documents_and_keeps_full_semantic_results() {
        let (bundle, types) = fixture();
        let context = InvocationContext::default();
        let observation = ParserObservation::new(&context);
        let parsed = ResourceSet::new(&bundle).unwrap();
        let root = &parsed.document(&bundle.sources[0].id).unwrap().root as *const _;
        let standard = standard_datatype_source();
        let adopted = Resolver::with_standard_source_and_inputs(
            &bundle,
            standard,
            Some(parsed),
            &observation,
        )
        .unwrap();
        assert!(std::ptr::eq(
            root,
            &adopted
                .resource_sets()
                .0
                .document(&bundle.sources[0].id)
                .unwrap()
                .root
        ));
        let fresh = Resolver::new(&bundle).unwrap();
        assert_eq!(
            serde_json::to_value(adopted.resolve_all().unwrap()).unwrap(),
            serde_json::to_value(fresh.resolve_all().unwrap()).unwrap()
        );
        let parsed = ResourceSet::new(&bundle).unwrap();
        let reused = materialize_context_with_inputs_observed(
            &bundle,
            &types,
            None,
            Some(parsed),
            &observation,
        )
        .unwrap();
        let fresh = materialize_context_observed(&bundle, &types, None, &observation).unwrap();
        assert_eq!(reused, fresh);
        assert_eq!(
            serde_json::to_value(&reused).unwrap(),
            serde_json::to_value(&fresh).unwrap()
        );
    }
    #[test]
    fn package_adoption_keeps_subset_fallback_revisions_and_rejects_stale_resource_views() {
        let (bundle, types) = fixture();
        let source = &bundle.sources[0];
        let context = InvocationContext::default();
        let complete = materialize_runtime_bundle_observed(
            &bundle,
            &source.id,
            "urn:parsed-adoption",
            &types,
            None,
            &context,
        )
        .unwrap();
        let mut subset = bundle.clone();
        let unused = DefinitionSource::new(
            "memory:unused.ecore",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='unused' nsURI='urn:unused'><eClassifiers x:type='e:EClass' name='Foreign'/></e:EPackage>",
        );
        let unused = crate::definitions::migration_test_fixture::source(unused);
        subset.namespaces.push(NamespaceBinding {
            namespace: "urn:unused".into(),
            source: unused.id.clone(),
        });
        subset.sources.push(unused);
        let filtered = materialize_runtime_bundle_observed(
            &subset,
            &source.id,
            "urn:parsed-adoption",
            &types,
            None,
            &context,
        )
        .unwrap();
        assert_eq!(filtered, complete);
        assert_eq!(
            serde_json::to_value(filtered).unwrap(),
            serde_json::to_value(&complete).unwrap()
        );
        let stale = ResourceSet::new(&bundle).unwrap();
        let changed = DefinitionSource::new(
            source.id.uri.clone(),
            SourceKind::Rcore,
            source.text.replace("`Child`", "`Renamed`"),
        );
        let changed_bundle = DefinitionBundle {
            version: 1,
            namespaces: vec![NamespaceBinding {
                namespace: "urn:parsed-adoption".into(),
                source: changed.id.clone(),
            }],
            sources: vec![changed],
        };
        let observation = ParserObservation::new(&context);
        let reused = Resolver::with_standard_source_and_inputs(
            &changed_bundle,
            standard_datatype_source(),
            Some(stale),
            &observation,
        )
        .unwrap();
        let fresh = Resolver::new(&changed_bundle).unwrap();
        assert_eq!(
            serde_json::to_value(reused.resolve_all().unwrap()).unwrap(),
            serde_json::to_value(fresh.resolve_all().unwrap()).unwrap()
        );
        let mut invalid = changed_bundle.clone();
        invalid.sources[0].text.push('x');
        assert!(
            Resolver::with_standard_source_and_inputs(
                &invalid,
                standard_datatype_source(),
                Some(ResourceSet::new(&bundle).unwrap()),
                &observation
            )
            .is_err()
        );
    }
}

#[cfg(test)]
#[path = "materialization_candidate_tests.rs"]
mod candidate_preparation_tests;

#[cfg(test)]
mod cold_phase_diagnostic {
    use super::*;
    use crate::definitions::conversion::{InvocationContext, InvocationObserver, ProviderId};
    use std::{cell::Cell, rc::Rc};
    #[derive(Default)]
    struct Meter {
        work: Cell<usize>,
        bytes: Cell<usize>,
    }
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("Unexpected provider".into())
        }
        fn work(&self, n: usize) -> Result<(), String> {
            self.work
                .set(self.work.get().checked_add(n).ok_or("Work overflow")?);
            Ok(())
        }
        fn retain_storage(&self, n: usize) -> Result<(), String> {
            self.bytes
                .set(self.bytes.get().checked_add(n).ok_or("Storage overflow")?);
            Ok(())
        }
    }
    fn phase(session: &OwnedRuntimeSourceArrayPreparation) -> &'static str {
        let pending = &session.preparation;
        if pending
            .holder
            .with_dependent(|_, state| matches!(state, MaterializationState::Loading(_)))
        {
            return "source_resources";
        }
        if pending
            .holder
            .with_dependent(|_, state| matches!(state, MaterializationState::Ready(state) if state.source_inventory.is_some()))
        {
            return "source_inventory";
        }
        if !pending.reports_complete {
            return "reports";
        }
        if !pending.features_complete {
            return "features";
        }
        if !pending.classes_complete {
            return "classes";
        }
        if !pending.publication_started {
            return "publication_begin";
        }
        pending.holder.with_dependent(|_, state| match state {
            MaterializationState::Loading(_) => "source_resources",
            MaterializationState::Ready(state) => match &state.pending_revision {
                None => "publication_complete",
                Some(pending) if pending.assembly.is_some() => pending
                    .assembly
                    .as_ref()
                    .map_or("publication_missing", |assembly| {
                        assembly.diagnostic_phase()
                    }),
                Some(pending) if !pending.metadata_revision_done => "metadata_revision",
                Some(pending) if !pending.revision_done => "revision",
                Some(pending) => pending
                    .tail
                    .as_ref()
                    .map_or("tail_begin", |tail| tail.diagnostic_phase()),
            },
        })
    }
    #[test]
    fn exact_large_domain_cold_phase_receipts_are_diagnostic_only() {
        run_cold_phase_diagnostic();
    }
    fn run_cold_phase_diagnostic() {
        let features=(0..4).map(|i|format!("<eStructuralFeatures x:type=\"e:EReference\" name=\"r{i}\"><eGenericType eTypeParameter=\"#//Root/T\"/></eStructuralFeatures>")).collect::<String>();
        let children = (0..127)
            .map(|i| {
                format!("<eClassifiers x:type=\"e:EClass\" name=\"C{i}\" eSuperTypes=\"#//Root\"/>")
            })
            .collect::<String>();
        let text = format!(
            "<e:EPackage xmlns:e=\"http://www.eclipse.org/emf/2002/Ecore\" xmlns:x=\"http://www.w3.org/2001/XMLSchema-instance\" name=\"m\" nsURI=\"urn:job-expansion\"><eClassifiers x:type=\"e:EClass\" name=\"Root\"><eTypeParameters name=\"T\"><eBounds eClassifier=\"#//Root\"/></eTypeParameters>{features}</eClassifiers>{children}</e:EPackage>"
        );
        let source = crate::definitions::bundle::DefinitionSource::new(
            "memory:job-expansion.ecore",
            crate::definitions::bundle::SourceKind::Ecore,
            text,
        );
        assert_eq!(
            source.id.sha256,
            "f1e38abc4457ce56324d15be77e39b28f30d4d8fe4c8a856c5bc070802371557"
        );
        let source = crate::definitions::migration_test_fixture::source(source);
        let types = (0..128)
            .map(|i| {
                RuntimeType::new(crate::definitions::generic_types::Type::Classifier {
                    declaration: DeclarationId {
                        source: source.id.clone(),
                        path: vec![i],
                    },
                    arguments: vec![],
                    raw: i == 0,
                })
                .unwrap()
            })
            .collect::<Vec<_>>();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let meter = Rc::new(Meter::default());
        let operation = InvocationContext::new(meter.clone());
        let mut session = OwnedRuntimeSourceArrayPreparation::begin_with_compiled_signature_inputs(
            bundle,
            source.id,
            "urn:job-expansion".into(),
            &types,
            None,
            &operation,
        )
        .unwrap_or_else(|_| panic!("begin failed"));
        let mut phases = BTreeMap::<&str, (usize, usize, usize, usize)>::new();
        phases.insert(
            "initialize",
            (meter.work.get(), meter.bytes.get(), 1, meter.work.get()),
        );
        for advances in 0..1000000 {
            let phase = phase(&session);
            let before = (meter.work.get(), meter.bytes.get());
            let complete = session.advance(None, &operation).unwrap();
            let work = meter.work.get() - before.0;
            let bytes = meter.bytes.get() - before.1;
            let entry = phases.entry(phase).or_default();
            entry.0 += work;
            entry.1 += bytes;
            entry.2 += 1;
            entry.3 = entry.3.max(work);
            if complete {
                break;
            }
            assert!(advances < 999999);
        }
        let schema = session.finish().materialization.result.unwrap();
        let unique_arrays = schema
            .runtime_feature_domains
            .values()
            .flatten()
            .map(|domain| domain.alternatives.as_ptr() as usize)
            .collect::<BTreeSet<_>>();
        println!(
            "DIAGNOSTIC_UNRESTRICTED_ONLY domains={} distinct_retained_alternative_arrays={}",
            schema
                .runtime_feature_domains
                .values()
                .map(Vec::len)
                .sum::<usize>(),
            unique_arrays.len()
        );
        let encoded = serde_json::to_vec(&schema).unwrap();
        assert_eq!(
            schema
                .runtime_feature_domains
                .values()
                .map(Vec::len)
                .sum::<usize>(),
            512
        );
        assert_eq!(unique_arrays.len(), 1);
        println!(
            "DIAGNOSTIC_UNRESTRICTED_ONLY serialized_bytes={}",
            encoded.len()
        );
        let digest = format!("{:x}", Sha256::digest(&encoded));
        // The historical XML-input DTO digest is not the canonical Rcore revision.
        // This probe reports timing/accounting data; semantic equivalence is tested
        // by the materialization and cancellation controls above.
        println!(
            "DIAGNOSTIC_UNRESTRICTED_ONLY phase=(work,retained_bytes,advances,max_advance_work): {phases:?}"
        );
        println!(
            "DIAGNOSTIC_UNRESTRICTED_ONLY total_work={} total_storage={} dto_sha256={digest}",
            meter.work.get(),
            meter.bytes.get()
        );
    }
}

#[cfg(test)]
#[path = "materialization_schema_capture_tests.rs"]
mod schema_capture_tests;

#[cfg(test)]
#[path = "retained_generic_agreement_materialization_tests.rs"]
mod retained_generic_agreement_materialization_tests;

#[cfg(test)]
#[test]
fn source_decoder_complete_real_layout_first_checkpoint() {
    let decoder = crate::definitions::generics::retained_decoder_layout();
    let entry =
        crate::definitions::generic_resolution::AgreementPreparation::measured_decoder_entry_layout(
        );
    let type_expression =
        std::mem::size_of::<crate::definitions::generic_types::TypeExpressionPreparation<'_>>();
    let inventory = std::mem::size_of::<
        crate::definitions::generic_resolution::SourceInventoryPreparation<'_>,
    >() + 128;
    let inner = std::mem::size_of::<MaterializationSourceInputs>()
        + std::mem::size_of::<MaterializationState<'static>>()
        + std::mem::size_of::<MaterializationContext<'static>>()
        + 128;
    let public = std::mem::size_of::<OwnedRuntimeSourceArrayPreparation>() + 128;
    assert!(decoder[0] <= 128 && decoder[2] <= 64 && decoder[3] <= 128);
    assert!(
        entry[0] <= 256
            && type_expression <= 144
            && inventory <= 512
            && inner <= 2048
            && public <= 2048
    );
    println!(
        "SOURCE_DECODER_REAL_LAYOUT {}",
        serde_json::json!({"decoder":decoder,"generic_input_projection_agreement":entry,"type_expression":type_expression,"inventory_plus128":inventory,"inner_plus128":inner,"public_plus128":public})
    );
}
