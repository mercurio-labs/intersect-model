//! Explicit, fallible read capabilities over prepared source authority.
//! Raw persistence, constructors, writes, rules and domain validation are separate.
use crate::core::{query, storage};
use crate::definitions::{
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    preparation::PreparedModelDefinition,
    schema::{
        Feature, FeatureIdentityInput, MemberIdentity, ReferenceConformancePreparation, Schema,
    },
};
use query::{QueryAccess, QueryBudget, QueryLimits, QueryOutput, QueryPlan, QueryResult};
use serde_json::Value;
use std::{cell::RefCell, rc::Rc};

#[path = "semantic_read_output.rs"]
mod output;
use output::ReadOutput;
pub use output::{ReferenceReadItem, ReferenceReadValue};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadPlan {
    Stored,
    Derived,
    Delegated,
    Unsupported,
}

/// Exact effective declaration and actual receiver. Metadata does not install a getter.
#[derive(Clone, Copy)]
pub struct ReadMember<'a> {
    pub actual: &'a str,
    pub owner: &'a str,
    pub identity: &'a MemberIdentity,
    pub feature: &'a Feature,
}

/// A source-defined null branch, distinct from graph/multiplicity validity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelatedNull {
    Reject,
    ReturnNull,
}

/// One native-declared edge, scoped to the exact selected calling getter.
/// Target identity is obtained by reading `reference`; policy never supplies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelatedRead<'a> {
    pub key: &'a str,
    pub reference: &'a str,
    pub target_actual: &'a str,
    pub target_feature: &'a str,
    pub null: RelatedNull,
}

/// Fallible declared dependencies, without a raw store/provider/operation escape.
///
/// ```compile_fail
/// use intersect_model::{core::storage::ModelRead, semantic_reads::ReadDependencies};
/// fn raw<T: ModelRead + ?Sized>(_: &T) {}
/// fn escape(reader: &dyn ReadDependencies) { raw(reader); }
/// ```
pub trait ReadDependencies {
    /// Checked current-store inverse ownership kind filter; no physical authority.
    fn read_inverse_nominal(
        &self,
        key: &str,
        _occurrence: usize,
        _budget: &mut QueryBudget,
    ) -> QueryResult<ClassifiedValue> {
        Err(format!("Unsupported inverse nominal dependency {key}"))
    }
    /// Source-declared kind filter on one ordinary stored reference occurrence.
    /// It returns only a semantic identity, never target getter/store authority.
    fn read_nominal(
        &self,
        key: &str,
        _occurrence: usize,
        _budget: &mut QueryBudget,
    ) -> QueryResult<ClassifiedValue> {
        Err(format!("Unsupported nominal reference dependency {key}"))
    }
    /// Compare a declared related reference with this receiver, without exposing
    /// the receiver's identity or a raw target/store capability.
    fn read_collection_related_matches_receiver(
        &self,
        key: &str,
        _occurrence: usize,
        _budget: &mut QueryBudget,
    ) -> QueryResult<Option<bool>> {
        Err(format!("Unsupported receiver equality dependency {key}"))
    }
    /// A finite-case getter at one occurrence of a declared stored collection.
    /// Target identity stays in the shared checked reader; None means exhausted.
    fn read_collection_related(
        &self,
        key: &str,
        _occurrence: usize,
        _budget: &mut QueryBudget,
    ) -> QueryResult<Option<Value>> {
        Err(format!(
            "Unsupported collection related semantic dependency {key}"
        ))
    }
    fn read_related_physical(
        &self,
        key: &str,
        _occurrence: usize,
        _budget: &mut QueryBudget,
    ) -> QueryResult<Value> {
        Err(format!(
            "Unsupported physical related semantic dependency {key}"
        ))
    }
    fn read_physical(&self, key: &str, _budget: &mut QueryBudget) -> QueryResult<Value> {
        Err(format!("Unsupported physical semantic dependency {key}"))
    }
    fn read_classified(
        &self,
        key: &str,
        _occurrence: usize,
        _budget: &mut QueryBudget,
    ) -> QueryResult<ClassifiedValue> {
        Err(format!("Unsupported classified semantic dependency {key}"))
    }
    fn read(&self, feature: &str, budget: &mut QueryBudget) -> QueryResult<Value>;
    fn read_related(
        &self,
        key: &str,
        _occurrence: usize,
        _budget: &mut QueryBudget,
    ) -> QueryResult<Value> {
        Err(format!(
            "Unsupported related semantic read dependency {key}"
        ))
    }
}

/// A language owns source checks, exact capabilities and getter algorithms.
/// Derived and delegated getters share this one fallible dependency reader.
pub trait SemanticReadPolicy {
    fn validate(&self, definition: &PreparedModelDefinition<'_>) -> QueryResult<()>;
    fn plan(&self, member: ReadMember<'_>) -> ReadPlan;
    fn dependencies(&self, member: ReadMember<'_>) -> &[&str];
    fn nominal_dependencies(&self, _member: ReadMember<'_>) -> &[NominalRead<'_>] {
        &[]
    }
    fn inverse_nominal_dependencies(&self, _member: ReadMember<'_>) -> &[InverseNominalRead<'_>] {
        &[]
    }
    fn related_dependencies(&self, _member: ReadMember<'_>) -> &[RelatedRead<'_>] {
        &[]
    }
    fn physical_dependencies(&self, _member: ReadMember<'_>) -> &[PhysicalRead<'_>] {
        &[]
    }
    fn classified_dependencies(&self, _member: ReadMember<'_>) -> &[ClassifiedRead<'_>] {
        &[]
    }
    fn classified_cases_dependencies(&self, _member: ReadMember<'_>) -> &[ClassifiedCasesRead<'_>] {
        &[]
    }
    fn physical_related_dependencies(&self, _member: ReadMember<'_>) -> &[PhysicalRelatedRead<'_>] {
        &[]
    }
    fn collection_related_dependencies(
        &self,
        _member: ReadMember<'_>,
    ) -> &[CollectionRelatedRead<'_>] {
        &[]
    }
    /// Exact keys of collection-related singular references which may be
    /// compared to this receiver. All old policies leave this disabled.
    fn receiver_equality_dependencies(&self, _member: ReadMember<'_>) -> &[&str] {
        &[]
    }
    fn read(
        &self,
        member: ReadMember<'_>,
        dependencies: &dyn ReadDependencies,
        budget: &mut QueryBudget,
    ) -> QueryResult<Value>;
}

/// A new read-only surface: no ModelRead implementation, Deref, Package conversion
/// or raw ObjectView callback. A supported getter is not complete language support.
///
/// ```compile_fail
/// use intersect_model::semantic_reads::{SemanticReadPolicy, SemanticReadSession};
/// use intersect_model::core::storage::ModelRead;
/// fn raw<'a, P: SemanticReadPolicy>(session: &'a SemanticReadSession<'_, P>) -> &'a dyn ModelRead { session }
/// ```
pub struct SemanticReadSession<'a, P: SemanticReadPolicy> {
    definition: &'a PreparedModelDefinition<'a>,
    store: &'a dyn storage::ModelRead,
    policy: &'a P,
    // Immutable canonical member DATA only; never a model/source/getter grant.
    metadata: Option<&'a crate::definitions::preparation::PreparedPhysicalMetadata<'a>>,
}

struct BudgetObservation<'a>(RefCell<&'a mut QueryBudget>);
impl InvocationObserver for BudgetObservation<'_> {
    fn before_invoke(&self, _: &ProviderId) -> QueryResult<()> {
        Ok(())
    }
    fn work(&self, units: usize) -> QueryResult<()> {
        let mut budget = self
            .0
            .try_borrow_mut()
            .map_err(|_| "Semantic read budget is already borrowed")?;
        for _ in 0..units {
            budget.charge()?;
        }
        Ok(())
    }
}

// Optional source policy for the SAME selection/dependency implementation.
// NoReadSources is zero-sized; the public session and selector holders stay exact.
struct AccountedSelection<'s> {
    identity: FeatureIdentityInput,
    owner: &'s str,
    feature: &'s Feature,

    position: Option<usize>,
}
trait ReadMetadataSelection {
    fn selected_hint<'s>(
        &self,
        schema: &'s Schema,
        actual: &str,
        name: &str,
        observation: &InvocationContext<'_>,
    ) -> QueryResult<Option<AccountedSelection<'s>>>;
    fn selected_metadata<'s>(
        &self,
        schema: &'s Schema,
        actual: &str,
        name: &str,
        observation: &InvocationContext<'_>,
    ) -> QueryResult<Option<(FeatureIdentityInput, &'s str, &'s Feature)>>;
}
impl<S: SemanticReadSources> ReadMetadataSelection for S {
    fn selected_hint<'s>(
        &self,
        schema: &'s Schema,
        actual: &str,
        name: &str,
        observation: &InvocationContext<'_>,
    ) -> QueryResult<Option<AccountedSelection<'s>>> {
        self.metadata_selection_hint(schema, actual, name, observation)
    }
    fn selected_metadata<'s>(
        &self,
        schema: &'s Schema,
        actual: &str,
        name: &str,
        observation: &InvocationContext<'_>,
    ) -> QueryResult<Option<(FeatureIdentityInput, &'s str, &'s Feature)>> {
        self.metadata_selection(schema, actual, name, observation)
    }
}
trait SemanticReadSources: Copy {
    const ENABLED: bool = false;
    const CHECKED_PUBLICATION: bool = false;
    const OBSERVED_PREFLIGHT: bool = false;
    fn metadata_selection_hint<'s>(
        &self,
        _schema: &'s Schema,
        _actual: &str,
        _name: &str,
        _observation: &InvocationContext<'_>,
    ) -> QueryResult<Option<AccountedSelection<'s>>> {
        Ok(None)
    }
    fn metadata_selection<'s>(
        &self,
        _schema: &'s Schema,
        _actual: &str,
        _name: &str,
        _observation: &InvocationContext<'_>,
    ) -> QueryResult<Option<(FeatureIdentityInput, &'s str, &'s Feature)>> {
        Ok(None)
    }
    fn publication_target(
        &self,
        _store: &dyn storage::ModelRead,
        _id: &str,
        _observation: &InvocationContext<'_>,
    ) -> QueryResult<()> {
        Err("Unsupported checked reference publication source scope".into())
    }
    type Cursor;
    fn enter(&self, _budget: &mut QueryBudget) -> QueryResult<()> {
        Ok(())
    }
    fn complete<T>(&self, result: QueryResult<T>, _budget: &mut QueryBudget) -> QueryResult<T> {
        result
    }
    #[allow(clippy::too_many_arguments)]
    fn reference_preparation(
        &self,
        receiver: &str,
        name: &str,
        identity: &MemberIdentity,
        field: &Feature,
        actual: &str,
        schema: &Schema,
        observation: &InvocationContext<'_>,
    ) -> QueryResult<ReferenceConformancePreparation> {
        let _ = (identity, field, schema);
        ReferenceConformancePreparation::new_observed(receiver, name, actual, observation)
            .map_err(|error| error.to_string())
    }
    fn cursor(&self) -> Self::Cursor;
    fn advance(
        &self,
        pending: &mut storage::SemanticFeaturePreparation,
        store: &dyn storage::ModelRead,
        schema: &Schema,
        observation: &InvocationContext<'_>,
        cursor: &mut Self::Cursor,
    ) -> QueryResult<bool>;
}
#[derive(Clone, Copy)]
struct NoReadSources;
impl SemanticReadSources for NoReadSources {
    type Cursor = ();
    fn cursor(&self) {}
    fn advance(
        &self,
        pending: &mut storage::SemanticFeaturePreparation,
        store: &dyn storage::ModelRead,
        schema: &Schema,
        observation: &InvocationContext<'_>,
        _: &mut (),
    ) -> QueryResult<bool> {
        pending.advance(store, schema, observation)
    }
}
fn semantic_source_call<S: SemanticReadSources, T>(
    sources: S,
    budget: &mut QueryBudget,
    run: impl FnOnce(&mut QueryBudget) -> QueryResult<T>,
) -> QueryResult<T> {
    let result = sources.enter(budget).and_then(|_| run(budget));
    sources.complete(result, budget)
}
#[allow(clippy::too_many_arguments)]
fn prepare_semantic_selection<S: SemanticReadSources>(
    receiver: &str,
    identity: &FeatureIdentityInput,
    object: &str,
    store: &dyn storage::ModelRead,
    schema: &Schema,
    observation: &InvocationContext<'_>,
    sources: S,

    preflight: Option<(&str, &Feature, usize)>,
) -> QueryResult<storage::SemanticFeatureSelection> {
    let mut pending = storage::SemanticFeaturePreparation::new(receiver, identity, object);
    let mut cursor = sources.cursor();
    if let Some((owner, feature, position)) = preflight.filter(|_| !S::ENABLED) {
        while !pending.advance_preflighted_exact(
            store,
            schema,
            observation,
            owner,
            feature,
            position,
        )? {}
    } else {
        while !sources.advance(&mut pending, store, schema, observation, &mut cursor)? {}
    }
    pending.finish()
}

impl<'a, P: SemanticReadPolicy> SemanticReadSession<'a, P> {
    pub fn new(
        definition: &'a PreparedModelDefinition<'a>,
        store: &'a dyn storage::ModelRead,
        policy: &'a P,
    ) -> QueryResult<Self> {
        policy.validate(definition)?;
        Ok(Self {
            definition,
            store,
            policy,
            metadata: None,
        })
    }
    /// Reuse one exact prepared owner's immutable canonical member metadata.
    /// Policy decisions and live storage checks still run for every read.
    /// Source-enabled and fully observed read modes retain their literal paths.
    pub fn new_prepared(
        metadata: &'a crate::definitions::preparation::PreparedPhysicalMetadata<'a>,
        store: &'a dyn storage::ModelRead,
        policy: &'a P,
    ) -> QueryResult<Self> {
        let mut session = Self::new(metadata.definition(), store, policy)?;
        session.metadata = Some(metadata);
        Ok(session)
    }
    fn schema(&self) -> &Schema {
        self.definition.schema()
    }
    fn selected<'b>(
        &'b self,
        actual: &'b str,
        name: &str,
    ) -> QueryResult<(MemberIdentity, &'b str, &'b Feature)> {
        let selected = self.schema().effective_feature(actual, name)?;
        Ok((selected.identity, selected.owner, selected.feature))
    }
    fn unavailable(actual: &str, member: &MemberIdentity) -> String {
        format!("Unsupported semantic read {actual}: {member:?}")
    }
    fn preflight_member(
        &self,
        actual: &str,
        name: &str,
        stack: &mut Vec<FeatureIdentityInput>,
    ) -> QueryResult<ReadPlan> {
        self.preflight_member_mode(actual, name, stack, false)
    }
    fn preflight_member_mode(
        &self,
        actual: &str,
        name: &str,
        stack: &mut Vec<FeatureIdentityInput>,
        physical: bool,
    ) -> QueryResult<ReadPlan> {
        self.preflight_member_accounted(actual, name, stack, physical, &ReadPreflight::legacy())
    }
    fn preflight_member_accounted(
        &self,
        actual: &str,
        name: &str,
        stack: &mut Vec<FeatureIdentityInput>,
        physical: bool,
        accounting: &ReadPreflight<'_>,
    ) -> QueryResult<ReadPlan> {
        self.preflight_selected_accounted(actual, name, stack, physical, accounting, false, false)
            .map(|(plan, _)| plan)
    }
    // Return the exact selection already consumed by this SAME preflight. The
    // observed snapshot caller can move it into execution without selecting the
    // immutable source again. Other callers retain their existing selection.
    #[allow(clippy::too_many_arguments)]
    fn preflight_selected_accounted<'s>(
        &'s self,
        actual: &'s str,
        name: &str,
        stack: &mut Vec<FeatureIdentityInput>,
        physical: bool,
        accounting: &ReadPreflight<'_>,
        capture_hint: bool,
        capture_position: bool,
    ) -> QueryResult<(ReadPlan, AccountedSelection<'s>)> {
        let AccountedSelection {
            identity,
            owner,
            feature,

            position,
        } = if capture_position {
            accounting.selected_with_position(self, actual, name)?
        } else if capture_hint {
            accounting.selected_with_hint(self, actual, name)?
        } else {
            let (identity, owner, feature) = accounting.selected(self, actual, name)?;
            AccountedSelection {
                identity,
                owner,
                feature,

                position: None,
            }
        };
        let member = ReadMember {
            actual,
            owner,
            identity: &identity,
            feature,
        };
        let plan = self.policy.plan(member);
        if plan == ReadPlan::Unsupported {
            return Err(Self::unavailable(actual, &identity));
        }
        if plan == ReadPlan::Stored && feature.derived {
            return Err("Derived metadata cannot select Stored reads".into());
        }
        if plan == ReadPlan::Derived && !feature.derived {
            return Err("Non-derived metadata requires an explicit Delegated read plan".into());
        }
        if accounting.contains(stack, &identity)? {
            return Err("Recursive semantic read dependency".into());
        }
        accounting.push(stack, &identity)?;
        for dependency in self.policy.dependencies(member) {
            accounting.step()?;
            self.preflight_member_accounted(actual, dependency, stack, physical, accounting)?;
        }
        self.preflight_nominal_dependencies(member, plan, stack, accounting)?;
        self.preflight_inverse_nominal_dependencies(member, plan, stack, accounting)?;
        let related = self.policy.related_dependencies(member);
        accounting.unsupported_shape(!related.is_empty())?;
        if !related.is_empty() && plan == ReadPlan::Stored {
            return Err("Stored reads cannot declare related getters".into());
        }
        for (index, edge) in related.iter().enumerate() {
            if edge.key.is_empty() || related[..index].iter().any(|other| other.key == edge.key) {
                return Err("Duplicate or empty related semantic dependency key".into());
            }
            let (_, _, reference) = accounting.selected(self, actual, edge.reference)?;
            if !reference.reference || reference.upper != 1 {
                return Err("Related dependency requires a singular reference role".into());
            }
            self.preflight_member_accounted(actual, edge.reference, stack, physical, accounting)?;
            self.preflight_member_accounted(
                edge.target_actual,
                edge.target_feature,
                stack,
                physical,
                accounting,
            )?;
        }
        let containers = self.policy.physical_dependencies(member);
        accounting.unsupported_shape(!containers.is_empty())?;
        if !containers.is_empty() {
            if !physical || plan == ReadPlan::Stored {
                return Err(
                    "Unsupported physical semantic dependency outside its audited graph".into(),
                );
            }
            for (index, role) in containers.iter().enumerate() {
                if role.key.is_empty()
                    || containers[..index]
                        .iter()
                        .any(|other| other.key == role.key)
                {
                    return Err("Duplicate or empty physical dependency key".into());
                }
                let (_, _, field) = accounting.selected(self, actual, role.reference)?;
                if !field.reference
                    || field.derived
                    || field.upper != 1
                    || !self.schema().is_container_feature(field)?
                {
                    return Err("Physical dependency requires a genuine stored inverse-container declaration".into());
                }
            }
        }
        let classified = self.policy.classified_dependencies(member);
        if !classified.is_empty() && !physical {
            return Err("Unsupported classified dependency outside its audited graph".into());
        }
        if !classified.is_empty() && plan == ReadPlan::Stored {
            return Err("Stored reads cannot declare classified getters".into());
        }
        for (index, edge) in classified.iter().enumerate() {
            accounting.step()?;
            if edge.key.is_empty()
                || accounting
                    .any_key(classified[..index].iter().map(|other| other.key), edge.key)?
            {
                return Err("Duplicate or empty classified dependency key".into());
            }
            let (reference_identity, reference_owner, reference) =
                accounting.selected(self, actual, edge.reference)?;
            if !reference.reference
                || (reference.upper != 1
                    && if accounting.is_observed() {
                        self.policy.plan(ReadMember {
                            actual,
                            owner: reference_owner,
                            identity: &reference_identity,
                            feature: reference,
                        })
                    } else {
                        self.policy.plan(ReadMember {
                            actual,
                            owner: accounting.selected(self, actual, edge.reference)?.1,
                            identity: &accounting.selected(self, actual, edge.reference)?.0,
                            feature: reference,
                        })
                    } != ReadPlan::Stored)
            {
                return Err(
                    "Classified dependency requires a singular or stored collection reference"
                        .into(),
                );
            }
            if !accounting.class(self.schema(), edge.nominal)?
                || !accounting.class(self.schema(), edge.admitted_actual)?
                || !accounting.conforms(self.schema(), edge.admitted_actual, edge.nominal)?
            {
                return Err("Invalid classified target declaration".into());
            }
            self.preflight_member_accounted(actual, edge.reference, stack, physical, accounting)?;
        }
        let cases = self.policy.classified_cases_dependencies(member);
        let physical_related = self.policy.physical_related_dependencies(member);
        accounting.unsupported_shape(!cases.is_empty() || !physical_related.is_empty())?;
        if !cases.is_empty() || !physical_related.is_empty() {
            if !physical || plan == ReadPlan::Stored {
                return Err(
                    "Unsupported new physical dependency outside its audited getter".into(),
                );
            }
            let old_key = |key: &str| {
                related.iter().any(|edge| edge.key == key)
                    || containers.iter().any(|edge| edge.key == key)
                    || classified.iter().any(|edge| edge.key == key)
            };
            for (index, edge) in cases.iter().enumerate() {
                if edge.key.is_empty()
                    || old_key(edge.key)
                    || cases[..index].iter().any(|other| other.key == edge.key)
                    || physical_related.iter().any(|other| other.key == edge.key)
                {
                    return Err("Duplicate or empty physical cases dependency key".into());
                }
                let (_, _, reference) = accounting.selected(self, actual, edge.reference)?;
                if !reference.reference || reference.upper != 1 {
                    return Err("Classified cases require a singular reference role".into());
                }
                if edge.admitted_actuals.is_empty()
                    || !accounting.class(self.schema(), edge.nominal)?
                {
                    return Err("Invalid finite classified target declaration".into());
                }
                for (case_index, admitted) in edge.admitted_actuals.iter().enumerate() {
                    if admitted.is_empty()
                        || edge.admitted_actuals[..case_index].contains(admitted)
                        || !accounting.class(self.schema(), admitted)?
                        || !accounting.conforms(self.schema(), admitted, edge.nominal)?
                        || !accounting.conforms(self.schema(), admitted, &reference.target)?
                    {
                        return Err("Invalid finite classified target declaration".into());
                    }
                }
                self.preflight_member_accounted(
                    actual,
                    edge.reference,
                    stack,
                    physical,
                    accounting,
                )?;
            }
            for (index, edge) in physical_related.iter().enumerate() {
                if edge.key.is_empty()
                    || old_key(edge.key)
                    || physical_related[..index]
                        .iter()
                        .any(|other| other.key == edge.key)
                {
                    return Err("Duplicate or empty physical related dependency key".into());
                }
                let (_, _, reference) = accounting.selected(self, actual, edge.reference)?;
                if !reference.reference || reference.upper != 1 || edge.cases.is_empty() {
                    return Err(
                        "Physical related dependency requires singular finite target cases".into(),
                    );
                }
                self.preflight_member_accounted(
                    actual,
                    edge.reference,
                    stack,
                    physical,
                    accounting,
                )?;
                for (case_index, target) in edge.cases.iter().enumerate() {
                    accounting.step()?;
                    if target.actual.is_empty()
                        || target.feature.is_empty()
                        || edge.cases[..case_index]
                            .iter()
                            .any(|other| other.actual == target.actual)
                    {
                        return Err("Duplicate or empty physical related target case".into());
                    }
                    if !accounting.class(self.schema(), target.actual)?
                        || !accounting.conforms(self.schema(), target.actual, &reference.target)?
                    {
                        return Err("Physical related target is outside its declared domain".into());
                    }
                    self.preflight_member_accounted(
                        target.actual,
                        target.feature,
                        stack,
                        physical,
                        accounting,
                    )?;
                }
            }
        }
        let collection_related = self.policy.collection_related_dependencies(member);
        if !collection_related.is_empty() {
            if !physical || plan == ReadPlan::Stored {
                return Err(
                    "Unsupported collection related dependency outside its audited getter".into(),
                );
            }
            for (index, edge) in collection_related.iter().enumerate() {
                accounting.step()?;
                if edge.key.is_empty()
                    || accounting.any_key(
                        collection_related[..index].iter().map(|old| old.key),
                        edge.key,
                    )?
                    || accounting.any_key(related.iter().map(|old| old.key), edge.key)?
                    || accounting.any_key(containers.iter().map(|old| old.key), edge.key)?
                    || accounting.any_key(classified.iter().map(|old| old.key), edge.key)?
                    || accounting.any_key(cases.iter().map(|old| old.key), edge.key)?
                    || accounting.any_key(physical_related.iter().map(|old| old.key), edge.key)?
                {
                    return Err("Duplicate or empty collection related dependency key".into());
                }
                let (reference_identity, owner, reference) =
                    accounting.selected(self, actual, edge.reference)?;
                if !reference.reference
                    || reference.upper == 1
                    || self.policy.plan(ReadMember {
                        actual,
                        owner,
                        identity: &reference_identity,
                        feature: reference,
                    }) != ReadPlan::Stored
                    || edge.cases.is_empty()
                    || !accounting.class(self.schema(), edge.nominal)?
                {
                    return Err("Collection related dependency requires a stored reference collection and finite cases".into());
                }
                self.preflight_member_accounted(
                    actual,
                    edge.reference,
                    stack,
                    physical,
                    accounting,
                )?;
                for (case_index, target) in edge.cases.iter().enumerate() {
                    accounting.step()?;
                    if target.actual.is_empty()
                        || target.feature.is_empty()
                        || accounting.any_key(
                            edge.cases[..case_index].iter().map(|old| old.actual),
                            target.actual,
                        )?
                        || !accounting.class(self.schema(), target.actual)?
                        || !accounting.conforms(self.schema(), target.actual, edge.nominal)?
                        || !accounting.conforms(self.schema(), target.actual, &reference.target)?
                    {
                        return Err("Invalid collection related actual target case".into());
                    }
                    self.preflight_member_accounted(
                        target.actual,
                        target.feature,
                        stack,
                        physical,
                        accounting,
                    )?;
                }
            }
        }
        let receiver_equality = self.policy.receiver_equality_dependencies(member);
        if !receiver_equality.is_empty() {
            if !physical || plan == ReadPlan::Stored {
                return Err("Receiver equality requires an audited getter".into());
            }
            for (index, key) in receiver_equality.iter().enumerate() {
                accounting.step()?;
                if key.is_empty()
                    || accounting.any_key(receiver_equality[..index].iter().copied(), key)?
                {
                    return Err("Duplicate or empty receiver equality key".into());
                }
                let mut found = None;
                for edge in collection_related {
                    if accounting.text_equal(edge.key, key)? {
                        found = Some(edge);
                        break;
                    }
                }
                let edge = found
                    .ok_or("Receiver equality requires an existing collection-related route")?;
                for target in edge.cases {
                    accounting.step()?;
                    let (_, _, feature) =
                        accounting.selected(self, target.actual, target.feature)?;
                    if !feature.reference
                        || feature.upper != 1
                        || !accounting.conforms(self.schema(), actual, &feature.target)?
                    {
                        return Err(
                            "Receiver equality requires a singular reference to the caller domain"
                                .into(),
                        );
                    }
                }
            }
        }
        stack.pop();
        Ok((
            plan,
            AccountedSelection {
                identity,
                owner,
                feature,

                position,
            },
        ))
    }
    fn preflight_path(&self, class: &str, path: &str, include_subtypes: bool) -> QueryResult<()> {
        query::path_type(self.schema(), class, path)?;
        if path.starts_with('$') {
            return Ok(());
        }
        let mut current = class.to_owned();
        for (index, name) in path.split('.').enumerate() {
            let selected = self.schema().effective_feature(&current, name)?;
            for actual in self.schema().classes.keys().filter(|actual| {
                if index == 0 && !include_subtypes {
                    *actual == &current
                } else {
                    self.schema().conforms(actual, &current)
                }
            }) {
                self.preflight_member(actual, name, &mut Vec::new())?;
            }
            current = selected.feature.target.clone();
        }
        Ok(())
    }
    fn check_receiver(&self, actual: &str, id: &str) -> QueryResult<()> {
        if self.store.revision()? != self.schema().revision {
            return Err("Semantic read/store schema mismatch".into());
        }
        let mut found = false;
        self.store.inspect(id, &mut |object| {
            if object.class() != actual || object.type_identity() != self.schema().identity(actual)
            {
                return Err("Semantic read actual receiver identity mismatch".into());
            }
            found = true;
            Ok(())
        })?;
        if !found {
            return Err("Missing semantic read receiver".into());
        }
        Ok(())
    }
    fn read_exact(
        &self,
        actual: &str,
        id: &str,
        name: &str,
        budget: &mut QueryBudget,
    ) -> QueryResult<Value> {
        self.read_exact_guarded(actual, id, name, budget, None)
    }
    fn read_exact_guarded(
        &self,
        actual: &str,
        id: &str,
        name: &str,
        budget: &mut QueryBudget,
        reference: Option<ReferenceContract<'_>>,
    ) -> QueryResult<Value> {
        self.read_exact_mode(actual, id, name, budget, reference, false)
    }
    #[cfg(test)]
    pub(crate) fn physical_value(
        &self,
        actual: &str,
        id: &str,
        name: &str,
        budget: &mut QueryBudget,
    ) -> QueryResult<Value> {
        self.read_exact_mode(actual, id, name, budget, None, true)
    }
    fn read_exact_mode(
        &self,
        actual: &str,
        id: &str,
        name: &str,
        budget: &mut QueryBudget,
        reference: Option<ReferenceContract<'_>>,
        physical: bool,
    ) -> QueryResult<Value> {
        self.read_exact_mode_with_sources(
            actual,
            id,
            name,
            budget,
            reference,
            physical,
            NoReadSources,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn read_exact_mode_with_sources<S: SemanticReadSources>(
        &self,
        actual: &str,
        id: &str,
        name: &str,
        budget: &mut QueryBudget,
        reference: Option<ReferenceContract<'_>>,
        physical: bool,
        sources: S,
    ) -> QueryResult<Value> {
        self.read_exact_worker::<LegacyReadMode, S>(
            actual, id, name, budget, reference, physical, sources,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn read_exact_worker<M: ReadCallMode<P, S>, S: SemanticReadSources>(
        &self,
        actual: &str,
        id: &str,
        name: &str,
        budget: &mut M::Work,
        reference: Option<ReferenceContract<'_>>,
        physical: bool,
        sources: S,
    ) -> QueryResult<Value> {
        self.read_exact_output_worker::<M, S, output::JsonOutput>(
            actual,
            id,
            name,
            budget,
            reference,
            physical,
            sources,
            output::JsonOutput,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn read_exact_output_worker<M: ReadCallMode<P, S>, S: SemanticReadSources, O: ReadOutput>(
        &self,
        actual: &str,
        id: &str,
        name: &str,
        budget: &mut M::Work,
        reference: Option<ReferenceContract<'_>>,
        physical: bool,
        sources: S,
        output: O,
    ) -> QueryResult<O::Output> {
        M::source_call(sources, budget, |budget| {
            let (plan, selection) = if S::OBSERVED_PREFLIGHT {
                let accounting = self.accounting_for::<M, S>(budget, &sources)?;
                self.preflight_selected_accounted(
                    actual,
                    name,
                    &mut Vec::new(),
                    physical,
                    &accounting,
                    true,
                    false,
                )?
            } else if !S::ENABLED {
                // Complete the SAME preflight before reusing immutable metadata.
                let (plan, selected) = {
                    let accounting = self.accounting_for::<M, S>(budget, &sources)?;
                    self.preflight_selected_accounted(
                        actual,
                        name,
                        &mut Vec::new(),
                        physical,
                        &accounting,
                        false,
                        true,
                    )?
                };
                if physical || (plan == ReadPlan::Stored && selected.position.is_some()) {
                    (plan, selected)
                } else {
                    // Ordinary computed/observed reads retain their literal
                    // second selection and cursor, with no ordinal handoff.
                    drop(selected);
                    let (identity, owner, feature) =
                        self.selected_for::<M, S>(actual, name, budget, sources)?;
                    (
                        plan,
                        AccountedSelection {
                            identity,
                            owner,
                            feature,

                            position: None,
                        },
                    )
                }
            } else {
                let plan = {
                    let accounting = self.accounting_for::<M, S>(budget, &sources)?;
                    self.preflight_member_accounted(
                        actual,
                        name,
                        &mut Vec::new(),
                        physical,
                        &accounting,
                    )?
                };
                let (identity, owner, feature) =
                    self.selected_for::<M, S>(actual, name, budget, sources)?;
                (
                    plan,
                    AccountedSelection {
                        identity,
                        owner,
                        feature,

                        position: None,
                    },
                )
            };
            let AccountedSelection {
                identity,
                owner,
                feature,

                position,
            } = selection;
            // Existing exact preparation work is charged to this same caller budget.
            // Retained-storage accounting/legacy traversal remain unqualified S09 work.
            let selected = {
                let observation = M::observation(budget);
                prepare_semantic_selection(
                    actual,
                    &identity,
                    id,
                    self.store,
                    self.schema(),
                    &observation,
                    sources,
                    position.map(|position| (owner, feature, position)),
                )?
            };
            {
                let observation = M::observation(budget);
                let accounting =
                    ReadPreflight::observed(S::OBSERVED_PREFLIGHT.then(|| observation.clone()))?;
                if !accounting.text_equal(selected.actual(), actual)? {
                    return Err("Semantic read actual receiver identity mismatch".into());
                }
                if !accounting.member_equal(selected.member(), &identity)?
                    || if S::OBSERVED_PREFLIGHT {
                        !std::ptr::eq(
                            selected.definition_fully_observed(self.schema(), &observation)?,
                            feature,
                        )
                    } else {
                        selected.definition(self.schema())? != feature
                    }
                {
                    return Err("Semantic read effective selection changed".into());
                }
            }
            if let Some(reference) = reference {
                let (current, _, field) =
                    self.selected_for::<M, S>(reference.actual, reference.name, budget, sources)?;
                let current_observation = M::observation(budget);
                let accounting = ReadPreflight::observed(
                    S::OBSERVED_PREFLIGHT.then(|| current_observation.clone()),
                )?;
                if !accounting.member_equal(&current, reference.identity)?
                    || !field.reference
                    || field.upper != 1
                {
                    return Err("Related semantic reference selection changed".into());
                }
                let observation = current_observation;
                let mut pending = sources.reference_preparation(
                    reference.actual,
                    reference.name,
                    &current,
                    field,
                    actual,
                    self.schema(),
                    &observation,
                )?;
                loop {
                    observation.work(1)?;
                    if pending
                        .advance_observed(self.schema(), &observation)
                        .map_err(|error| error.to_string())?
                    {
                        break;
                    }
                }
                if !pending.finish()? {
                    return Err("Related semantic reference target does not conform".into());
                }
            }
            if S::CHECKED_PUBLICATION {
                let observation = M::observation(budget);
                sources.publication_target(self.store, id, &observation)?;
            }
            match plan {
                ReadPlan::Stored => {
                    if S::CHECKED_PUBLICATION && feature.reference && feature.upper != 1 {
                        return Err("Unsupported stored reference collection publication".into());
                    }
                    let observation = M::observation(budget);
                    output.stored(
                        &selected,
                        self.store,
                        self.schema(),
                        id,
                        &observation,
                        S::OBSERVED_PREFLIGHT,
                    )
                }
                ReadPlan::Derived | ReadPlan::Delegated => {
                    let member = ReadMember {
                        actual,
                        owner,
                        identity: &identity,
                        feature,
                    };
                    let value = M::invoke(self, member, id, physical, sources, budget)?;
                    if O::OBSERVE_COMPUTED {
                        let observation = M::observation(budget);
                        output.computed(value, &observation)
                    } else {
                        // Normal JSON output preserves its original zero-adapter
                        // allocation path after provider invocation.
                        output.computed(value, &InvocationContext::default())
                    }
                }
                ReadPlan::Unsupported => Err(Self::unavailable(actual, &identity)),
            }
        })
    }
    /// Checked reference transport through the SAME policy/preflight/live read
    /// engine. Canonical Stored values borrow internally; getter and arbitrary
    /// adapter results retain their existing owned-value fallback. The result
    /// is published only after semantic/source completion; target checks remain
    /// the consuming language's responsibility.
    pub fn reference_value(
        &self,
        actual: &str,
        id: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> QueryResult<ReferenceReadValue> {
        self.read_exact_output_worker::<LegacyReadMode, NoReadSources, output::ReferenceOutput>(
            actual,
            id,
            feature,
            budget,
            None,
            false,
            NoReadSources,
            output::ReferenceOutput,
        )
    }
    /// `actual` is exact, not a subtype-inclusive selector. It is checked on every read.
    pub fn value(
        &self,
        actual: &str,
        id: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> QueryResult<Value> {
        self.read_exact(actual, id, feature, budget)
    }
    pub fn value_len(
        &self,
        actual: &str,
        id: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> QueryResult<usize> {
        self.read_exact(actual, id, feature, budget)?
            .as_array()
            .map(Vec::len)
            .ok_or_else(|| "Semantic length requires a collection".into())
    }
    /// Validate the complete selection before invoking any store callback.
    pub fn project(
        &self,
        actual: &str,
        id: &str,
        features: &[&str],
        budget: &mut QueryBudget,
    ) -> QueryResult<Vec<(String, Value)>> {
        for name in features {
            self.preflight_member(actual, name, &mut Vec::new())?;
        }
        features
            .iter()
            .map(|name| Ok(((*name).into(), self.read_exact(actual, id, name, budget)?)))
            .collect()
    }
    pub fn inspect_all(
        &self,
        actual: &str,
        id: &str,
        budget: &mut QueryBudget,
    ) -> QueryResult<Vec<(String, Value)>> {
        let members = self.schema().effective_features(actual)?;
        let names: Vec<_> = members
            .iter()
            .map(|member| member.feature.name.as_str())
            .collect();
        self.project(actual, id, &names, budget)
    }
    /// The existing query path walker supplies reference navigation and validation.
    pub fn navigate(
        &self,
        actual: &str,
        id: &str,
        path: &str,
        budget: &mut QueryBudget,
    ) -> QueryResult<Value> {
        self.preflight_path(actual, path, false)?;
        query::policy_path(self.schema(), self.store, self, budget, id, actual, path)
    }
    pub fn query(
        &self,
        plan: &QueryPlan,
        parameters: &query::Parameters,
        limits: QueryLimits,
    ) -> QueryResult<QueryOutput> {
        plan.execute_policy(self.schema(), self.store, parameters, self, limits)
    }
}

struct Dependencies<'a, 's, P: SemanticReadPolicy, S: SemanticReadSources = NoReadSources> {
    session: &'s SemanticReadSession<'a, P>,
    actual: &'s str,
    id: &'s str,
    allowed: &'s [&'s str],
    sources: S,
}
struct ReferenceContract<'a> {
    actual: &'a str,
    name: &'a str,
    identity: &'a MemberIdentity,
}
// Only getters with declared related edges use this new invocation-local holder.
// The original same-object adapter and session concrete fields remain unchanged.
struct RelatedDependencies<'a, 's, P: SemanticReadPolicy, S: SemanticReadSources = NoReadSources> {
    same: Dependencies<'a, 's, P, S>,
    caller: ReadMember<'s>,
}
impl<P: SemanticReadPolicy, S: SemanticReadSources> ReadDependencies
    for RelatedDependencies<'_, '_, P, S>
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
}
impl<P: SemanticReadPolicy, S: SemanticReadSources> ReadDependencies
    for Dependencies<'_, '_, P, S>
{
    fn read(&self, feature: &str, budget: &mut QueryBudget) -> QueryResult<Value> {
        self.read_work::<LegacyReadMode>(feature, budget)
    }
}
impl<P: SemanticReadPolicy> QueryAccess for &SemanticReadSession<'_, P> {
    fn before_store(self, limits: QueryLimits) -> QueryResult<()> {
        // Preserve validation/limit order; a failed existing charge has no
        // successful fee and prevents the first raw revision/candidate callback.
        if limits.max_steps == 0 {
            QueryBudget::new(0).charge()?;
        }
        Ok(())
    }
    fn check_object(
        self,
        _: &Schema,
        _: &dyn storage::ModelRead,
        id: &str,
        class: &str,
    ) -> QueryResult<()> {
        self.check_receiver(class, id)
    }

    fn check_path(self, _: &Schema, class: &str, path: &str) -> QueryResult<()> {
        self.preflight_path(class, path, true)
    }
    fn read_field(
        self,
        _: &Schema,
        _: &dyn storage::ModelRead,
        id: &str,
        class: &str,
        field: &Feature,
        budget: &mut QueryBudget,
    ) -> QueryResult<Value> {
        self.read_exact(class, id, &field.name, budget)
    }
    fn count_stored(self, _: &Schema, class: &str, name: &str) -> QueryResult<bool> {
        self.preflight_member(class, name, &mut Vec::new())?;
        // The first semantic slice conservatively uses the qualified path even
        // for Stored counts, so exact receiver selection cannot be bypassed.
        Ok(false)
    }
    fn anchor_stored(self, schema: &Schema, class: &str, name: &str) -> QueryResult<bool> {
        for actual in schema
            .classes
            .keys()
            .filter(|actual| schema.conforms(actual, class))
        {
            if self.preflight_member(actual, name, &mut Vec::new())? != ReadPlan::Stored {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[path = "semantic_nominal.rs"]
mod nominal;
pub use nominal::NominalRead;

#[path = "semantic_physical.rs"]
mod physical;
use physical::PhysicalDependencies;
pub use physical::{
    ClassifiedCasesRead, ClassifiedRead, ClassifiedValue, CollectionRelatedRead, PhysicalRead,
    PhysicalRelatedCase, PhysicalRelatedRead,
};

#[cfg(test)]
#[path = "semantic_owner_tests.rs"]
mod owner_tests;

#[cfg(test)]
#[path = "semantic_handoff_tests.rs"]
mod handoff_tests;

#[cfg(test)]
#[path = "semantic_source_layout_tests.rs"]
mod source_layout_tests;

// Shared bodies; legacy callback signatures/holders above are unchanged.
impl<P: SemanticReadPolicy, S: SemanticReadSources> RelatedDependencies<'_, '_, P, S> {
    pub(super) fn read_work<M: ReadCallMode<P, S>>(
        &self,
        feature: &str,
        budget: &mut M::Work,
    ) -> QueryResult<Value> {
        self.same.read_work::<M>(feature, budget)
    }
}

impl<P: SemanticReadPolicy, S: SemanticReadSources> RelatedDependencies<'_, '_, P, S> {
    pub(super) fn read_related_work<M: ReadCallMode<P, S>>(
        &self,
        key: &str,
        occurrence: usize,
        budget: &mut M::Work,
    ) -> QueryResult<Value> {
        M::source_call(self.same.sources, budget, |budget| {
            let session = self.same.session;
            let edges = session.policy.related_dependencies(self.caller);
            let mut matches = edges.iter().filter(|edge| edge.key == key);
            let edge = matches
                .next()
                .ok_or_else(|| format!("Undeclared related semantic read dependency {key}"))?;
            if matches.next().is_some() {
                return Err("Duplicate related semantic dependency key".into());
            }
            if occurrence != 0 {
                return Err("Related singular reference requires occurrence zero".into());
            }
            let prepared = session.metadata.is_some() && !S::ENABLED && M::PREPARED_METADATA;
            let (identity, _, reference) = if prepared {
                session.selected_for::<M, S>(
                    self.same.actual,
                    edge.reference,
                    budget,
                    self.same.sources,
                )?
            } else {
                let (identity, owner, feature) =
                    session.selected(self.same.actual, edge.reference)?;
                (FeatureIdentityInput::Owned(identity), owner, feature)
            };
            if !reference.reference || reference.upper != 1 {
                return Err("Related dependency requires a singular reference role".into());
            }
            if prepared {
                let accounting = session.accounting_for::<M, S>(budget, &self.same.sources)?;
                session.preflight_member_accounted(
                    self.same.actual,
                    edge.reference,
                    &mut Vec::new(),
                    false,
                    &accounting,
                )?;
                session.preflight_member_accounted(
                    edge.target_actual,
                    edge.target_feature,
                    &mut Vec::new(),
                    false,
                    &accounting,
                )?;
            } else {
                // Preserve literal source/observed/default callback admission.
                session.preflight_member(self.same.actual, edge.reference, &mut Vec::new())?;
                session.preflight_member(
                    edge.target_actual,
                    edge.target_feature,
                    &mut Vec::new(),
                )?;
            }
            let value = session.read_exact_worker::<M, S>(
                self.same.actual,
                self.same.id,
                edge.reference,
                budget,
                None,
                false,
                self.same.sources,
            )?;
            M::charge(budget)?;
            if value.is_null() {
                return match edge.null {
                    RelatedNull::ReturnNull => Ok(Value::Null),
                    RelatedNull::Reject => Err("Related semantic reference is absent".into()),
                };
            }
            let id = value
                .as_str()
                .ok_or("Related semantic reference contains a non-identity value")?;
            session.read_exact_worker::<M, S>(
                edge.target_actual,
                id,
                edge.target_feature,
                budget,
                Some(ReferenceContract {
                    actual: self.same.actual,
                    name: edge.reference,
                    identity: &identity,
                }),
                false,
                self.same.sources,
            )
        })
    }
}

impl<P: SemanticReadPolicy, S: SemanticReadSources> Dependencies<'_, '_, P, S> {
    pub(super) fn read_work<M: ReadCallMode<P, S>>(
        &self,
        feature: &str,
        budget: &mut M::Work,
    ) -> QueryResult<Value> {
        M::source_call(self.sources, budget, |budget| {
            if !ReadPreflight::observed(M::preflight_observation(budget))?
                .any_key(self.allowed.iter().copied(), feature)?
            {
                return Err(format!("Undeclared semantic read dependency {feature}"));
            }
            self.session.read_exact_worker::<M, S>(
                self.actual,
                self.id,
                feature,
                budget,
                None,
                false,
                self.sources,
            )
        })
    }
}

#[path = "semantic_read_work.rs"]
mod work;
use work::{LegacyReadMode, ReadCallMode};
#[path = "semantic_preflight.rs"]
mod preflight;
use preflight::ReadPreflight;

#[path = "semantic_inverse_nominal.rs"]
mod inverse_nominal;
pub use inverse_nominal::InverseNominalRead;
