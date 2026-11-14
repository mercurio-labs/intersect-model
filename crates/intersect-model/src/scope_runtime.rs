//! Source-selected scope functions over narrow, borrowed pending-model reads.
//!
//! The host supplies observations, not graph-validity authority. Every receiver,
//! member and returned model identity is checked against the selected definition.
//! This runs the existing Rcore machine and does not admit or publish a model.
pub use crate::definitions::schema::ScopeSchema;
use crate::{
    definitions::{
        conversion::InvocationContext,
        generic_types::RuntimeType,
        rcore_behavior::{
            ir,
            ir_v2::{DataWire, OwnershipOp, TypeNode, ValueWire},
            types::Scalar,
        },
        rcore_getter::VerifiedGetter,
        rcore_scope::VerifiedScope,
        resources::DeclarationId,
        schema::{EffectiveFeaturePreparation, MemberIdentity, Schema},
    },
    rcore_behavior_functions::{
        BorrowedEvaluationSession, ExecutionContext, ExecutionLimits, SessionProgress,
        inputs::{BorrowedDomain, BorrowedInputAuthority, BorrowedTarget},
    },
    semantic_bindings::BorrowedBindingWork,
};
use serde_json::Value;
use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::{BTreeMap, BTreeSet, HashSet},
    fmt,
    num::NonZeroUsize,
};

/// One source-bound singular observation. Pending and unavailable are not absence.
pub enum ScopeValue {
    Value(Value),
    Absent,
    Pending,
    Unavailable,
}
/// One collection element, or a proven end of the collection.
pub enum ScopePage {
    Value(Value),
    End,
    Pending,
    Unavailable,
}

/// A fallible read capability for one immutable linking operation. Implementors
/// retain their live source/store association and active-reference cycle guards.
/// No method grants ModelRead, mutation, or publication authority.
pub trait ScopeRead {
    fn check(&self, observation: &InvocationContext<'_>) -> Result<(), String>;
    fn class(&self, object: &str, observation: &InvocationContext<'_>) -> Result<String, String>;
    fn singular(
        &self,
        object: &str,
        member: &DeclarationId,
        observation: &InvocationContext<'_>,
    ) -> Result<ScopeValue, String>;
    fn page(
        &self,
        object: &str,
        member: &DeclarationId,
        cursor: usize,
        observation: &InvocationContext<'_>,
    ) -> Result<ScopePage, String>;
    /// Optional generic containment navigation, in canonical ownership order.
    /// Implementations use the existing checked ownership workers and reject
    /// unavailable, ambiguous, cyclic or unsupported derived ownership.
    fn ownership(
        &self,
        _: &str,
        _: OwnershipOp,
        _: &RuntimeType,
        _: usize,
        _: &ScopeSchema<'_>,
        _: &InvocationContext<'_>,
    ) -> Result<ScopePage, String> {
        Err("This scope read capability does not provide ownership navigation".into())
    }
}
#[derive(Debug)]
pub enum ScopeError {
    Pending {
        object: String,
        member: DeclarationId,
    },
    Unavailable {
        object: String,
        member: DeclarationId,
    },
    PendingOwnership {
        object: String,
    },
    UnavailableOwnership {
        object: String,
    },
    InvalidInput(String),
    Execution(Box<crate::rcore_behavior_functions::ExecutionError>),
    Boundary(crate::semantic_bindings::Error),
}
impl fmt::Display for ScopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending { object, member } => write!(
                f,
                "Scope read depends on pending reference {object} at {member:?}"
            ),
            Self::Unavailable { object, member } => write!(
                f,
                "Scope read requires unavailable resource for {object} at {member:?}"
            ),
            Self::PendingOwnership { object } => write!(
                f,
                "Scope ownership depends on pending structure for {object}"
            ),
            Self::UnavailableOwnership { object } => write!(
                f,
                "Scope ownership requires an unavailable resource for {object}"
            ),
            Self::InvalidInput(message) => f.write_str(message),
            Self::Execution(error) => write!(f, "{error}"),
            Self::Boundary(error) => write!(f, "{error}"),
        }
    }
}
impl std::error::Error for ScopeError {}

fn retention_error(message: String) -> ScopeError {
    ScopeError::Boundary(crate::semantic_bindings::Error::new(
        crate::semantic_bindings::ErrorKind::ResourceExhausted,
        message,
    ))
}

/// Checked target identities, kept in the scope function's original result order.
/// They are not standalone model handles or a model-publication receipt.
pub struct ScopeCandidates {
    identities: Vec<String>,
}
impl ScopeCandidates {
    pub fn identities(&self) -> &[String] {
        &self.identities
    }
    pub fn into_identities(self) -> Vec<String> {
        self.identities
    }
}

const CONTEXT: &str = "source-scope-input";
struct Authority<'a, 'caller> {
    schema: &'a Schema,
    read: &'a dyn ScopeRead,
    work: &'a BorrowedBindingWork<'caller>,
    limits: &'a ExecutionLimits,
    failed: Cell<bool>,
    failure: RefCell<Option<ScopeError>>,
    visited: RefCell<BTreeSet<String>>,
    checks: Cell<usize>,
    unique_reads: RefCell<BTreeMap<(String, DeclarationId), UniqueRead>>,
    // Effective member resolved per (receiver class, declaration). The schema is
    // immutable for the invocation, so one resolution per pair is exact.
    members:
        RefCell<BTreeMap<String, BTreeMap<DeclarationId, &'a crate::definitions::schema::Feature>>>,
    schema_proof: OnceCell<ScopeSchema<'a>>,
}
#[derive(Default)]
struct UniqueRead {
    values: HashSet<Value>,
    last: Option<Value>,
    next: usize,
    complete: bool,
}
impl<'schema> Authority<'schema, '_> {
    fn prepared_schema(
        &self,
        observation: &InvocationContext<'_>,
    ) -> Result<&ScopeSchema<'schema>, String> {
        self.check(observation)?;
        if self.schema_proof.get().is_none() {
            let proof = ScopeSchema::prepare_observed(self.schema, observation)?;
            self.schema_proof
                .set(proof)
                .map_err(|_| "Scope schema preparation was reentered")?;
        }
        self.schema_proof
            .get()
            .ok_or_else(|| "Missing checked scope schema".into())
    }
    fn class(&self, object: &str, observation: &InvocationContext<'_>) -> Result<String, String> {
        self.check(observation)?;
        observation.work(1)?;
        let count = self
            .checks
            .get()
            .checked_add(1)
            .ok_or("Scope membership count overflow")?;
        if count > self.limits.max_membership_checks {
            return Err("Scope membership limit exceeded".into());
        }
        self.checks.set(count);
        if !self.visited.borrow().contains(object) {
            if self.visited.borrow().len() >= self.limits.max_model_elements {
                return Err("Scope model element limit exceeded".into());
            }
            observation.retain_storage(64usize.saturating_add(object.len()))?;
            self.visited.borrow_mut().insert(object.to_owned());
        }
        let actual = self.read.class(object, observation)?;
        if !self.schema.classes.contains_key(&actual)
            || !self.schema.declarations.contains_key(&actual)
        {
            return Err("Scope object has no exact source-declared class".into());
        }
        self.read.check(observation)?;
        Ok(actual)
    }
    fn require(
        &self,
        object: &str,
        expected: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<(), String> {
        let actual = self.class(object, observation)?;
        if !self
            .prepared_schema(observation)?
            .conforms_observed(&actual, expected, observation)?
        {
            return Err("Scope object does not conform to the selected model type".into());
        }
        Ok(())
    }
    fn member(
        &self,
        object: &str,
        member: &DeclarationId,
        observation: &InvocationContext<'_>,
    ) -> Result<&'schema crate::definitions::schema::Feature, String> {
        let actual = self.class(object, observation)?;
        if let Some(feature) = self
            .members
            .borrow()
            .get(&actual)
            .and_then(|by_declaration| by_declaration.get(member))
        {
            return Ok(feature);
        }
        let feature = self.resolve_member(&actual, member, observation)?;
        observation.retain_storage(128usize.saturating_add(actual.len()))?;
        self.members
            .borrow_mut()
            .entry(actual)
            .or_default()
            .insert(member.clone(), feature);
        Ok(feature)
    }
    fn resolve_member(
        &self,
        actual: &str,
        member: &DeclarationId,
        observation: &InvocationContext<'_>,
    ) -> Result<&'schema crate::definitions::schema::Feature, String> {
        let mut pending = EffectiveFeaturePreparation::new_observed(actual, observation)
            .map_err(|e| e.to_string())?;
        while !pending
            .advance_observed(self.schema, observation)
            .map_err(|e| e.to_string())?
        {}
        for entry in pending.finish().map_err(|e| e.to_string())? {
            observation.work(1)?;
            let entry = entry.resolve(self.schema)?;
            if matches!(&entry.identity, MemberIdentity::Source { declaration } if declaration == member)
            {
                return Ok(entry.feature);
            }
        }
        Err("Scope read does not select an exact effective member of its receiver".into())
    }
    fn unique_page(
        &self,
        object: &str,
        member: &DeclarationId,
        cursor: usize,
        value: Option<&Value>,
        observation: &InvocationContext<'_>,
    ) -> Result<(), String> {
        crate::definitions::retained_storage::reserve_serialized(member, observation)?;
        observation.retain_storage(128usize.saturating_add(object.len()))?;
        let key = (object.to_owned(), member.clone());
        let mut state = self
            .unique_reads
            .try_borrow_mut()
            .map_err(|_| "Scope unique-input tracker is already borrowed")?
            .remove(&key)
            .unwrap_or_default();
        if cursor == 0 && state.complete {
            state = UniqueRead::default();
        }
        match value {
            None => {
                if cursor != state.next {
                    return Err("Scope collection ended at an inconsistent cursor".into());
                }
                state.complete = true;
            }
            Some(value) => {
                crate::definitions::metadata_limits::measure_with_work(
                    value,
                    self.limits.max_value_nodes,
                    self.limits.max_value_depth,
                    self.limits.max_context_bytes,
                    &|units| observation.work(units),
                )?;
                observation.work(1usize.saturating_add(state.values.len()))?;
                if cursor == state.next {
                    if state.values.contains(value) {
                        return Err(
                            "Duplicate value in a declared-unique scoped input collection".into(),
                        );
                    }
                    // The shared stored-input equality is serde Value equality,
                    // matching FeatureShape; this is input rejection, not dedup.
                    crate::definitions::retained_storage::reserve_serialized(value, observation)?;
                    crate::definitions::retained_storage::reserve_serialized(value, observation)?;
                    observation.retain_storage(128usize.saturating_mul(1 + state.values.len()))?;
                    state
                        .values
                        .try_reserve(1)
                        .map_err(|_| "Scope unique-input allocation failed")?;
                    state.values.insert(value.clone());
                    state.last = Some(value.clone());
                    state.next = state
                        .next
                        .checked_add(1)
                        .ok_or("Scope collection cursor overflow")?;
                } else if cursor.checked_add(1) != Some(state.next)
                    || state.last.as_ref() != Some(value)
                {
                    return Err(
                        "Scope collection cursor or value changed during target preparation".into(),
                    );
                }
            }
        }
        self.check(observation)?;
        self.unique_reads
            .try_borrow_mut()
            .map_err(|_| "Scope unique-input tracker is already borrowed")?
            .insert(key, state);
        Ok(())
    }
    fn dependency(&self, object: &str, member: &DeclarationId, unavailable: bool) -> String {
        let error = if unavailable {
            ScopeError::Unavailable {
                object: object.into(),
                member: member.clone(),
            }
        } else {
            ScopeError::Pending {
                object: object.into(),
                member: member.clone(),
            }
        };
        let message = error.to_string();
        *self.failure.borrow_mut() = Some(error);
        message
    }
}
impl BorrowedInputAuthority for Authority<'_, '_> {
    fn source_scope(&self) -> bool {
        true
    }
    fn direct_ownership(
        &self,
        object: &str,
        operation: OwnershipOp,
        selected: &RuntimeType,
        cursor: usize,
        observation: &InvocationContext<'_>,
    ) -> Result<Option<Option<Value>>, String> {
        let actual = self.class(object, observation)?;
        if operation == OwnershipOp::AsType {
            if cursor != 0 {
                return Err("Checked type view requires its singular cursor".into());
            }
            let schema = self.prepared_schema(observation)?;
            let expected = schema.class_key_for_type_observed(selected, observation)?;
            let value = if schema.conforms_observed(&actual, expected, observation)? {
                observation.retain_storage(64usize.saturating_add(object.len()))?;
                Some(Value::String(object.to_owned()))
            } else {
                None
            };
            self.check(observation)?;
            return Ok(Some(value));
        }
        let value = match self.read.ownership(
            object,
            operation,
            selected,
            cursor,
            self.prepared_schema(observation)?,
            observation,
        )? {
            ScopePage::Value(value) => Some(value),
            ScopePage::End => None,
            ScopePage::Pending => {
                let error = ScopeError::PendingOwnership {
                    object: object.into(),
                };
                let message = error.to_string();
                *self.failure.borrow_mut() = Some(error);
                return Err(message);
            }
            ScopePage::Unavailable => {
                let error = ScopeError::UnavailableOwnership {
                    object: object.into(),
                };
                let message = error.to_string();
                *self.failure.borrow_mut() = Some(error);
                return Err(message);
            }
        };
        self.check(observation)?;
        Ok(Some(value))
    }
    fn original_observation(&self) -> InvocationContext<'_> {
        self.work.observer()
    }
    fn context_key(&self) -> &str {
        CONTEXT
    }
    fn check(&self, observation: &InvocationContext<'_>) -> Result<(), String> {
        observation.checkpoint()?;
        self.work.check().map_err(|e| e.to_string())?;
        if self.failed.get() {
            return Err("Scope invocation has failed".into());
        }
        self.read.check(observation)
    }
    // This mode admits only visited identities; it never enumerates the graph.
    fn object_count(&self) -> usize {
        0
    }
    fn begin_target<'a>(
        &'a self,
        target: &RuntimeType,
        observation: &InvocationContext<'_>,
    ) -> Result<Box<dyn BorrowedTarget<'a> + 'a>, String> {
        self.check(observation)?;
        let expected = self
            .prepared_schema(observation)?
            .class_key_for_type_observed(target, observation)?;
        observation.retain_storage(128usize.saturating_add(expected.len()))?;
        Ok(Box::new(Target {
            authority: self,
            expected: expected.into(),
            complete: false,
        }))
    }
    fn direct_stored(
        &self,
        object: &str,
        member: &DeclarationId,
        cursor: Option<usize>,
        observation: &InvocationContext<'_>,
    ) -> Result<Option<Option<Value>>, String> {
        let field = self.member(object, member, observation)?;
        let value = match cursor {
            None => match self.read.singular(object, member, observation)? {
                ScopeValue::Value(value) => Some(value),
                ScopeValue::Absent => Some(Value::Null),
                ScopeValue::Pending => return Err(self.dependency(object, member, false)),
                ScopeValue::Unavailable => return Err(self.dependency(object, member, true)),
            },
            Some(cursor) => match self.read.page(object, member, cursor, observation)? {
                ScopePage::Value(value) => Some(value),
                ScopePage::End => None,
                ScopePage::Pending => return Err(self.dependency(object, member, false)),
                ScopePage::Unavailable => return Err(self.dependency(object, member, true)),
            },
        };
        if let Some(cursor) = cursor
            && field.unique
        {
            self.unique_page(object, member, cursor, value.as_ref(), observation)?;
        }
        self.check(observation)?;
        Ok(Some(value))
    }
}
struct Target<'borrow, 'schema, 'caller> {
    authority: &'borrow Authority<'schema, 'caller>,
    expected: String,
    complete: bool,
}
impl<'borrow, 'schema: 'borrow, 'caller: 'borrow> BorrowedTarget<'borrow>
    for Target<'borrow, 'schema, 'caller>
{
    fn advance(&mut self, observation: &InvocationContext<'_>) -> Result<bool, String> {
        self.authority.check(observation)?;
        self.complete = true;
        Ok(true)
    }
    fn finish(
        self: Box<Self>,
        observation: &InvocationContext<'_>,
    ) -> Result<Box<dyn BorrowedDomain + 'borrow>, String> {
        if !self.complete {
            return Err("Scope target preparation is incomplete".into());
        }
        self.authority.check(observation)?;
        Ok(Box::new(Domain {
            authority: self.authority,
            expected: self.expected,
        }))
    }
}
struct Domain<'borrow, 'schema, 'caller> {
    authority: &'borrow Authority<'schema, 'caller>,
    expected: String,
}
impl BorrowedDomain for Domain<'_, '_, '_> {
    fn resolve(&self, object: &str, observation: &InvocationContext<'_>) -> Result<(), String> {
        self.authority.require(object, &self.expected, observation)
    }
    fn begin_stored(
        &self,
        _: &str,
        _: &DeclarationId,
        _: &InvocationContext<'_>,
    ) -> Result<crate::storage::StoredFeaturePreparation, String> {
        Err("Source scopes use explicit narrow member reads".into())
    }
    fn advance_stored(
        &self,
        _: &mut crate::storage::StoredFeaturePreparation,
        _: &InvocationContext<'_>,
    ) -> Result<bool, String> {
        Err("Source scopes use explicit narrow member reads".into())
    }
    fn stored<'a>(
        &'a self,
        _: &'a str,
        _: &crate::storage::StoredFeatureSelection,
        _: &InvocationContext<'_>,
    ) -> Result<crate::storage::StoredFeatureProjection<'a>, String> {
        Err("Source scopes use explicit narrow member reads".into())
    }
}

#[allow(clippy::too_many_arguments)]
fn with_read_session<T>(
    image: &crate::definitions::rcore_behavior::ir_v2::VerifiedExecutable,
    context: &ExecutionContext<'_>,
    schema: &Schema,
    read: &dyn ScopeRead,
    limits: &ExecutionLimits,
    work: &BorrowedBindingWork<'_>,
    arguments: &[ValueWire],
    finish: impl FnOnce(&Authority<'_, '_>, ValueWire) -> Result<T, ScopeError>,
) -> Result<T, ScopeError> {
    let observation = work.observer();
    let authority = Authority {
        schema,
        read,
        work,
        limits,
        failed: Cell::new(false),
        failure: RefCell::new(None),
        visited: RefCell::new(BTreeSet::new()),
        checks: Cell::new(0),
        unique_reads: RefCell::new(BTreeMap::new()),
        members: RefCell::new(BTreeMap::new()),
        schema_proof: OnceCell::new(),
    };
    // Shared argument admission prepares the exact receiver domain before
    // execution. Its original observer also bounds the one schema proof.
    let mut session = BorrowedEvaluationSession::new_binding(
        image,
        arguments,
        context,
        limits,
        &authority,
        &observation,
    )
    .map_err(ScopeError::Boundary)?;
    let result = loop {
        if session.payload_continuation() {
            work.check()
        } else {
            work.charge(1)
        }
        .map_err(ScopeError::Boundary)?;
        match session.resume_with_control(NonZeroUsize::MIN, work) {
            SessionProgress::Pending => {}
            SessionProgress::Finished => {
                return Err(ScopeError::InvalidInput(
                    "Source scope session finished without a result".into(),
                ));
            }
            SessionProgress::Complete(result) => break *result,
        }
    };
    if let Some(failure) = authority.failure.borrow_mut().take() {
        return Err(failure);
    }
    let result = result.map_err(|e| ScopeError::Execution(Box::new(e)))?;
    finish(&authority, result.value)
}

/// Execute an exact source-derived getter on the SAME borrowed read machine.
/// The value is VM output, not a graph-validity or model-publication receipt;
/// consuming model services retain their checked result/write boundary.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_getter(
    getter: &VerifiedGetter,
    context: &ExecutionContext<'_>,
    schema: &Schema,
    read: &dyn ScopeRead,
    owner: &str,
    limits: &ExecutionLimits,
    work: &BorrowedBindingWork<'_>,
) -> Result<ValueWire, ScopeError> {
    let run = || {
        work.charge(1).map_err(ScopeError::Boundary)?;
        if !context.references.is_empty() {
            return Err(ScopeError::InvalidInput(
                "Source getters cannot substitute owning model contexts".into(),
            ));
        }
        let image = getter.executable();
        let entry = image
            .program()
            .functions
            .get(image.program().entry.0 as usize)
            .ok_or_else(|| ScopeError::InvalidInput("Missing verified getter entry".into()))?;
        if entry.parameters.len() != 1 {
            return Err(ScopeError::InvalidInput(
                "Source getter requires one receiver".into(),
            ));
        }
        work.observer()
            .retain_storage(owner.len().saturating_add(256))
            .map_err(retention_error)?;
        let arguments = [ValueWire {
            ty: entry.parameters[0],
            data: DataWire::Model(
                serde_json::json!({"kind":"model_reference","context":CONTEXT,"element":owner}),
            ),
        }];
        with_read_session(
            image,
            context,
            schema,
            read,
            limits,
            work,
            &arguments,
            |authority, value| {
                if value.ty != entry.result {
                    return Err(ScopeError::InvalidInput(
                        "Source getter returned a foreign result type".into(),
                    ));
                }
                check_getter_references(authority, &value, &image.program().types, work)?;
                authority
                    .check(&work.observer())
                    .map_err(ScopeError::InvalidInput)?;
                Ok(value)
            },
        )
    };
    let result = run();
    work.check().map_err(ScopeError::Boundary)?;
    result
}

/// The same borrowed machine for an independently selected source read body.
/// The compiling facade checks its signature; it installs no derived feature.
#[allow(clippy::too_many_arguments)]
pub(crate) fn evaluate_read_function(
    image: &crate::definitions::rcore_behavior::ir_v2::VerifiedExecutable,
    context: &ExecutionContext<'_>,
    schema: &Schema,
    read: &dyn ScopeRead,
    owner: &str,
    arguments: Vec<DataWire>,
    limits: &ExecutionLimits,
    work: &BorrowedBindingWork<'_>,
) -> Result<ValueWire, ScopeError> {
    let run = || {
        work.charge(1).map_err(ScopeError::Boundary)?;
        if !context.references.is_empty() {
            return Err(ScopeError::InvalidInput(
                "Source read functions cannot substitute owning model contexts".into(),
            ));
        }
        let entry = image
            .program()
            .functions
            .get(image.program().entry.0 as usize)
            .ok_or_else(|| ScopeError::InvalidInput("Missing source read function".into()))?;
        if entry.parameters.len() != arguments.len().saturating_add(1) {
            return Err(ScopeError::InvalidInput(
                "Source read argument count differs".into(),
            ));
        }
        let receiver = *entry
            .parameters
            .first()
            .ok_or_else(|| ScopeError::InvalidInput("Missing source read receiver".into()))?;
        let storage = entry
            .parameters
            .len()
            .checked_mul(crate::logical_size!(ValueWire, 152))
            .and_then(|bytes| bytes.checked_add(owner.len()))
            .and_then(|bytes| bytes.checked_add(256))
            .ok_or_else(|| {
                ScopeError::InvalidInput("Source read argument storage overflow".into())
            })?;
        let observation = work.observer();
        observation
            .retain_storage(storage)
            .map_err(retention_error)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(entry.parameters.len())
            .map_err(|_| retention_error("Source read argument allocation failed".into()))?;
        values.push(ValueWire {
            ty: receiver,
            data: DataWire::Model(
                serde_json::json!({"kind":"model_reference","context":CONTEXT,"element":owner}),
            ),
        });
        values.extend(
            arguments
                .into_iter()
                .zip(entry.parameters.iter().skip(1))
                .map(|(data, ty)| ValueWire { ty: *ty, data }),
        );
        with_read_session(
            image,
            context,
            schema,
            read,
            limits,
            work,
            &values,
            |authority, value| {
                if value.ty != entry.result {
                    return Err(ScopeError::InvalidInput(
                        "Source read returned a foreign result type".into(),
                    ));
                }
                check_read_function_result(authority, image, &value, limits, work)?;
                authority
                    .check(&observation)
                    .map_err(ScopeError::InvalidInput)?;
                Ok(value)
            },
        )
    };
    let result = run();
    work.check().map_err(ScopeError::Boundary)?;
    result
}

fn check_read_function_result(
    authority: &Authority<'_, '_>,
    image: &crate::definitions::rcore_behavior::ir_v2::VerifiedExecutable,
    value: &ValueWire,
    limits: &ExecutionLimits,
    work: &BorrowedBindingWork<'_>,
) -> Result<(), ScopeError> {
    use crate::definitions::rcore_behavior::ir_v2::{
        IrError, RetainedValueShapeCursor, ValueBudget, ValueVisitor,
    };
    struct LiveLeaves<'a, 'schema, 'caller> {
        authority: &'a Authority<'schema, 'caller>,
    }
    impl ValueVisitor for LiveLeaves<'_, '_, '_> {
        fn nominal_leaf(&mut self, ty: &TypeNode, payload: &Value) -> Result<(), IrError> {
            let TypeNode::Model(target) = ty else {
                return Ok(());
            };
            let observation = self.authority.work.observer();
            let checked = || -> Result<(), String> {
                let (context, object) = crate::model_reference::reference_identity(payload)?;
                if context != CONTEXT {
                    return Err("Source read result belongs to another model context".into());
                }
                let schema = self.authority.prepared_schema(&observation)?;
                let target = schema.class_key_for_type_observed(target, &observation)?;
                self.authority.require(object, target, &observation)
            };
            checked()
                .map_err(|error| crate::definitions::rcore_behavior::ir_v2::invalid(error, None))
        }
        fn unique_collection(
            &mut self,
            _: crate::definitions::rcore_behavior::ir_v2::TypeId,
            _: &[ValueWire],
        ) -> Result<(), IrError> {
            // The VM retains cardinality/uniqueness validation. This pass uses
            // its common shape cursor to recheck live nominal result leaves.
            Ok(())
        }
    }
    let mut budget = ValueBudget {
        remaining_nodes: limits.max_value_nodes,
        remaining_bytes: limits.scalar.max_value_bytes,
        max_depth: limits.max_value_depth,
        scalar: limits.scalar.values.clone(),
    };
    let mut cursor =
        RetainedValueShapeCursor::new(value.ty, &mut budget, limits.scalar.max_value_bytes)
            .map_err(|error| ScopeError::InvalidInput(error.to_string()))?;
    let observation = work.observer();
    observation
        .retain_storage(cursor.storage_bytes())
        .map_err(retention_error)?;
    let mut visitor = LiveLeaves { authority };
    loop {
        work.charge(1).map_err(ScopeError::Boundary)?;
        let before = cursor.storage_bytes();
        let result = cursor.advance(image, value, &mut budget, &mut visitor);
        observation
            .retain_storage(cursor.storage_bytes().saturating_sub(before))
            .map_err(retention_error)?;
        if result.map_err(|error| ScopeError::InvalidInput(error.to_string()))? {
            return Ok(());
        }
    }
}

/// Recheck every returned identity on the live source loan before storage
/// projection. Preserve occurrences/order; the VM's exact Many result contract
/// separately checks cardinality and uniqueness, so no deduplication occurs.
fn check_getter_references(
    authority: &Authority<'_, '_>,
    value: &ValueWire,
    types: &[TypeNode],
    work: &BorrowedBindingWork<'_>,
) -> Result<(), ScopeError> {
    let invalid = |message: &str| ScopeError::InvalidInput(message.into());
    let (element, values): (_, &[ValueWire]) = match types.get(value.ty.0 as usize) {
        Some(TypeNode::Model(_)) => (value.ty, std::slice::from_ref(value)),
        Some(TypeNode::Option(element))
            if matches!(types.get(element.0 as usize), Some(TypeNode::Model(_))) =>
        {
            match &value.data {
                DataWire::None => return Ok(()),
                DataWire::Some(value) => (*element, std::slice::from_ref(value.as_ref())),
                _ => return Err(invalid("Optional reference getter has an invalid result")),
            }
        }
        Some(TypeNode::Many { element, .. })
            if matches!(types.get(element.0 as usize), Some(TypeNode::Model(_))) =>
        {
            let DataWire::Collection(values) = &value.data else {
                return Err(invalid("Reference collection getter has an invalid result"));
            };
            (*element, values)
        }
        _ => return Ok(()),
    };
    let Some(TypeNode::Model(target)) = types.get(element.0 as usize) else {
        return Err(invalid("Reference getter lost its exact model type"));
    };
    let observation = work.observer();
    let target_key = authority
        .prepared_schema(&observation)
        .and_then(|schema| schema.class_key_for_type_observed(target, &observation))
        .map_err(ScopeError::InvalidInput)?;
    for value in values {
        work.charge(1).map_err(ScopeError::Boundary)?;
        if value.ty != element {
            return Err(invalid("Reference getter returned a foreign element type"));
        }
        let DataWire::Model(payload) = &value.data else {
            return Err(invalid("Reference getter returned a non-model element"));
        };
        let (context, object) = crate::model_reference::reference_identity(payload)
            .map_err(ScopeError::InvalidInput)?;
        if context != CONTEXT {
            return Err(invalid("Reference getter returned another model context"));
        }
        authority
            .require(object, target_key, &observation)
            .map_err(ScopeError::InvalidInput)?;
    }
    Ok(())
}

/// Execute one checked `(owner, spelling) -> Sequence<target>` source scope.
/// The read capability and original caller budget are retained for the entire
/// call; no model snapshot, JSON graph, native provider or second VM is created.
#[allow(clippy::too_many_arguments)]
pub fn evaluate(
    scope: &VerifiedScope,
    context: &ExecutionContext<'_>,
    schema: &Schema,
    read: &dyn ScopeRead,
    owner: &str,
    spelling: &str,
    limits: &ExecutionLimits,
    work: &BorrowedBindingWork<'_>,
) -> Result<ScopeCandidates, ScopeError> {
    let run = || -> Result<ScopeCandidates, ScopeError> {
        work.charge(1).map_err(ScopeError::Boundary)?;
        if !context.references.is_empty() {
            return Err(ScopeError::InvalidInput(
                "Source scopes cannot substitute owning model contexts".into(),
            ));
        }
        let image = scope.executable();
        let binding = scope.binding();
        let observation = work.observer();
        let entry = image
            .program()
            .functions
            .get(image.program().entry.0 as usize)
            .ok_or_else(|| ScopeError::InvalidInput("Missing source scope entry".into()))?;
        if entry.parameters.len() != 2 {
            return Err(ScopeError::InvalidInput(
                "Invalid source scope parameter count".into(),
            ));
        }
        observation
            .retain_storage(
                owner
                    .len()
                    .saturating_add(spelling.len())
                    .saturating_add(512),
            )
            .map_err(retention_error)?;
        let arguments = [
            ValueWire {
                ty: entry.parameters[0],
                data: DataWire::Model(
                    serde_json::json!({"kind":"model_reference","context":CONTEXT,"element":owner}),
                ),
            },
            ValueWire {
                ty: entry.parameters[1],
                data: DataWire::Scalar(ir::Value {
                    ty: ir::ValueType::Scalar(Scalar::String),
                    payload: Value::String(spelling.into()),
                }),
            },
        ];
        with_read_session(
            image,
            context,
            schema,
            read,
            limits,
            work,
            &arguments,
            |authority, value| {
                let DataWire::Collection(values) = value.data else {
                    return Err(ScopeError::InvalidInput(
                        "Source scope did not return a candidate sequence".into(),
                    ));
                };
                let Some(TypeNode::Collection { element, .. }) =
                    image.program().types.get(entry.result.0 as usize)
                else {
                    return Err(ScopeError::InvalidInput(
                        "Source scope result is not a sequence type".into(),
                    ));
                };
                let target_key = authority
                    .prepared_schema(&observation)
                    .and_then(|schema| {
                        schema.class_key_for_type_observed(&binding.target, &observation)
                    })
                    .map_err(ScopeError::InvalidInput)?;
                let mut identities = Vec::new();
                let mut seen = BTreeSet::new();
                for value in values {
                    work.charge(1).map_err(ScopeError::Boundary)?;
                    if value.ty != *element {
                        return Err(ScopeError::InvalidInput(
                            "Source scope candidate has the wrong model type".into(),
                        ));
                    }
                    let DataWire::Model(payload) = value.data else {
                        return Err(ScopeError::InvalidInput(
                            "Source scope candidate is not a model reference".into(),
                        ));
                    };
                    let (digest, object) = crate::model_reference::reference_identity(&payload)
                        .map_err(ScopeError::InvalidInput)?;
                    if digest != CONTEXT {
                        return Err(ScopeError::InvalidInput(
                            "Source scope candidate belongs to another model context".into(),
                        ));
                    }
                    authority
                        .require(object, target_key, &observation)
                        .map_err(ScopeError::InvalidInput)?;
                    // A scope exposes identities rather than sequence occurrences.
                    // Validate EVERY occurrence before stable deduplication, so an
                    // invalid later candidate can never be concealed by an earlier hit.
                    let comparisons =
                        16usize.saturating_mul(1 + seen.len().max(1).ilog2() as usize);
                    observation
                        .work(comparisons.saturating_mul(1 + object.len().div_ceil(64)))
                        .map_err(retention_error)?;
                    if seen.contains(object) {
                        continue;
                    }
                    observation
                        .retain_storage(96usize.saturating_add(object.len().saturating_mul(2)))
                        .map_err(retention_error)?;
                    seen.insert(object.to_owned());
                    identities.push(object.into());
                }
                authority
                    .check(&observation)
                    .map_err(ScopeError::InvalidInput)?;
                Ok(ScopeCandidates { identities })
            },
        )
    };
    let result = run();
    work.check().map_err(ScopeError::Boundary)?;
    result
}

#[cfg(test)]
#[path = "scope_runtime_tests.rs"]
mod tests;
