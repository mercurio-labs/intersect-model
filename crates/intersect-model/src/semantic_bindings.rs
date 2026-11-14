//! Handwritten checked dispatch over the shared Rcore value/model runtime.
//! Hosts own registry mutation; pinned calls retain their selected implementation.
use crate::rcore_behavior_functions::{
    Evaluation, EvaluationSession, ExecutionContext, ExecutionControl, ExecutionError,
    ExecutionLimits, SessionProgress,
};
pub use intersect_model::definitions::semantic_bindings::Contract;
use intersect_model::definitions::{
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    rcore_behavior::{
        ir,
        ir_v2::{DataWire, Effect, TypeId, TypeNode, ValueWire},
        types::Scalar,
    },
};
use serde::Serialize;
use std::{cell::Cell, collections::BTreeMap, num::NonZeroUsize, rc::Rc};

#[cfg(feature = "rhai")]
#[path = "semantic_bindings_rhai.rs"]
pub mod rhai;

#[path = "semantic_bindings_derived.rs"]
pub mod derived;
#[path = "semantic_bindings_values.rs"]
mod typed_values;
pub use typed_values::{Bag, NominalValue, OrderedSet, Set};
#[path = "semantic_bindings_session.rs"]
mod binding_session;
pub use binding_session::{BindingProgress, BindingSession};

#[path = "semantic_bindings_work.rs"]
mod work;
pub use work::{BindingWork, BorrowedBindingWork};
#[path = "semantic_bindings_conditions.rs"]
mod conditions;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ErrorKind {
    Contract,
    Arguments,
    Result,
    MissingImplementation,
    DuplicateImplementation,
    Unsupported,
    Semantic,
    Precondition,
    Postcondition,
    ResourceExhausted,
    Cancelled,
}
#[derive(Clone, Debug, Serialize)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
    pub execution: Option<Box<ExecutionError>>,
    pub span: Option<intersect_model::definitions::bundle::SourceSpan>,
}
impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            execution: None,
            span: None,
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}
impl From<ExecutionError> for Error {
    fn from(error: ExecutionError) -> Self {
        use crate::rcore_behavior_functions::ExecutionErrorKind as K;
        let kind = match error.kind {
            K::Cancelled => ErrorKind::Cancelled,
            K::ResourceExhausted => ErrorKind::ResourceExhausted,
            K::Arguments | K::Nominal => ErrorKind::Arguments,
            K::Unsupported => ErrorKind::Unsupported,
            K::Arithmetic => ErrorKind::Semantic,
            _ => ErrorKind::Contract,
        };
        Self {
            kind,
            message: error.to_string(),
            span: error.span.as_deref().cloned(),
            execution: Some(Box::new(error)),
        }
    }
}
pub type Result<T> = std::result::Result<T, Error>;

/// One sticky authority for admission, native work, scripts, reads and result checking.
/// A callback cannot recover from exhaustion/cancellation by catching its error.
#[derive(Clone)]
pub struct Budget(Rc<BudgetState>);
struct BudgetState {
    remaining: Cell<u64>,
    used: Cell<u64>,
    failure: Cell<Option<ErrorKind>>,
}
impl Budget {
    pub fn new(work: u64) -> Self {
        Self(Rc::new(BudgetState {
            remaining: Cell::new(work),
            used: Cell::new(0),
            failure: Cell::new(None),
        }))
    }
    pub fn cancel(&self) {
        if self.0.failure.get().is_none() {
            self.0.failure.set(Some(ErrorKind::Cancelled));
        }
    }
    pub fn used(&self) -> u64 {
        self.0.used.get()
    }
    pub fn check(&self) -> Result<()> {
        match self.0.failure.get() {
            Some(kind) => Err(Error::new(
                kind,
                match kind {
                    ErrorKind::Cancelled => "Binding invocation cancelled",
                    _ => "Binding work budget exhausted",
                },
            )),
            None => Ok(()),
        }
    }
    pub fn charge(&self, units: u64) -> Result<()> {
        self.check()?;
        let Some(next) = self.0.remaining.get().checked_sub(units) else {
            self.0.failure.set(Some(ErrorKind::ResourceExhausted));
            return self.check();
        };
        self.0.remaining.set(next);
        self.0.used.set(self.0.used.get().saturating_add(units));
        Ok(())
    }
    pub fn observer(&self) -> InvocationContext<'static> {
        InvocationContext::new(Rc::new(self.clone()))
    }
}
impl ExecutionControl for Budget {
    fn cancellation_requested(&self) -> bool {
        self.0.failure.get().is_some()
    }
}
impl InvocationObserver for Budget {
    fn before_invoke(&self, _: &ProviderId) -> std::result::Result<(), String> {
        self.work(1)
    }
    fn checkpoint(&self) -> std::result::Result<(), String> {
        self.check().map_err(|e| e.to_string())
    }
    fn work(&self, units: usize) -> std::result::Result<(), String> {
        self.charge(units as u64).map_err(|e| e.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ImplementationId {
    pub backend: String,
    pub revision: String,
}
/// Generated and manual adapters implement this same interface. Model semantics
/// and argument/result validation never belong to the adapter.
pub trait Provider {
    fn contract_id(&self) -> &str;
    fn identity(&self) -> ImplementationId;
    fn invoke(&self, call: &Call<'_>, arguments: &[ValueWire]) -> Result<ValueWire>;
}
pub struct NativeProvider<F> {
    contract: String,
    revision: String,
    function: F,
}
impl<F> NativeProvider<F> {
    pub fn new(contract: &Contract, revision: impl Into<String>, function: F) -> Self {
        Self {
            contract: contract.id().into(),
            revision: revision.into(),
            function,
        }
    }
}
impl<F> Provider for NativeProvider<F>
where
    F: Fn(&Call<'_>, &[ValueWire]) -> Result<ValueWire>,
{
    fn contract_id(&self) -> &str {
        &self.contract
    }
    fn identity(&self) -> ImplementationId {
        ImplementationId {
            backend: "rust".into(),
            revision: self.revision.clone(),
        }
    }
    fn invoke(&self, call: &Call<'_>, arguments: &[ValueWire]) -> Result<ValueWire> {
        (self.function)(call, arguments)
    }
}
#[derive(Clone)]
enum Implementation {
    Provider(Rc<dyn Provider>),
    Rcore,
}
#[derive(Clone)]
pub struct PinnedCall {
    contract: Contract,
    implementation: Implementation,
    identity: ImplementationId,
}
#[derive(Default)]
pub struct Registry {
    entries: BTreeMap<String, PinnedCall>,
}
impl Registry {
    /// Publish a fully prepared package atomically. Existing selections survive
    /// a rejected package; replacement remains an explicit per-entry operation.
    pub fn install_package(&mut self, package: Registry) -> Result<()> {
        if package
            .entries
            .keys()
            .any(|id| self.entries.contains_key(id))
        {
            return Err(Error::new(
                ErrorKind::DuplicateImplementation,
                "Package contains an installed contract",
            ));
        }
        for selected in package.entries.values() {
            if let Implementation::Provider(provider) = &selected.implementation
                && (provider.contract_id() != selected.contract.id()
                    || provider.identity() != selected.identity)
            {
                return Err(Error::new(
                    ErrorKind::Contract,
                    "Staged implementation identity changed",
                ));
            }
        }
        self.entries.extend(package.entries);
        Ok(())
    }
    pub fn install(&mut self, contract: &Contract, provider: Rc<dyn Provider>) -> Result<()> {
        if self.entries.contains_key(contract.id()) {
            return Err(Error::new(
                ErrorKind::DuplicateImplementation,
                "Contract already has an implementation",
            ));
        }
        let selected = Self::selected(contract, provider)?;
        self.entries.insert(contract.id().into(), selected);
        Ok(())
    }
    fn selected(contract: &Contract, provider: Rc<dyn Provider>) -> Result<PinnedCall> {
        if provider.contract_id() != contract.id() {
            return Err(Error::new(
                ErrorKind::Contract,
                "Implementation belongs to another contract revision",
            ));
        }
        let identity = provider.identity();
        if identity.revision.trim().is_empty() || identity.backend.trim().is_empty() {
            return Err(Error::new(
                ErrorKind::Contract,
                "Implementation identity is required",
            ));
        }
        Ok(PinnedCall {
            contract: contract.clone(),
            implementation: Implementation::Provider(provider),
            identity,
        })
    }
    /// Validate before replacing. Existing PinnedCall values remain unaffected.
    pub fn replace(&mut self, contract: &Contract, provider: Rc<dyn Provider>) -> Result<()> {
        if !self.entries.contains_key(contract.id()) {
            return Err(Error::new(
                ErrorKind::MissingImplementation,
                "Cannot replace an absent implementation",
            ));
        }
        let selected = Self::selected(contract, provider)?;
        self.entries.insert(contract.id().into(), selected);
        Ok(())
    }
    pub fn install_rcore(&mut self, contract: &Contract) -> Result<()> {
        if contract.reference().is_none() {
            return Err(Error::new(
                ErrorKind::MissingImplementation,
                "External declaration has no Rcore body; register a Rust or Rhai implementation",
            ));
        }
        if self.entries.contains_key(contract.id()) {
            return Err(Error::new(
                ErrorKind::DuplicateImplementation,
                "Contract already has an implementation",
            ));
        }
        self.entries.insert(
            contract.id().into(),
            PinnedCall {
                contract: contract.clone(),
                implementation: Implementation::Rcore,
                identity: ImplementationId {
                    backend: "rcore".into(),
                    revision: contract.id().into(),
                },
            },
        );
        Ok(())
    }
    pub fn remove(&mut self, contract: &Contract) -> Result<()> {
        self.entries
            .remove(contract.id())
            .map(|_| ())
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::MissingImplementation,
                    "Contract has no implementation",
                )
            })
    }
    pub fn pin(&self, contract: &Contract) -> Result<PinnedCall> {
        self.entries.get(contract.id()).cloned().ok_or_else(|| {
            Error::new(
                ErrorKind::MissingImplementation,
                "Contract has no implementation",
            )
        })
    }
}
#[derive(Debug, Serialize)]
pub struct InvocationResult {
    pub contract: String,
    pub implementation: ImplementationId,
    pub work: u64,
    pub evaluation: Evaluation,
}
pub struct Call<'a> {
    pub contract: &'a Contract,
    pub budget: Budget,
    handles: BTreeMap<usize, ModelHandle>,
}
impl Call<'_> {
    pub fn model(&self, index: usize) -> Result<ModelHandle> {
        self.budget.charge(1)?;
        if self.contract.descriptor().effect != Effect::Query {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "Pure binding has no model read capability",
            ));
        }
        self.handles.get(&index).cloned().ok_or_else(|| {
            Error::new(
                ErrorKind::Arguments,
                "Argument is not an admitted model handle",
            )
        })
    }
    pub fn result<T: NativeValue>(&self, value: T) -> Result<ValueWire> {
        value.into_typed(
            self.contract.descriptor().result,
            &self.contract.descriptor().types,
            &self.budget,
        )
    }
    pub fn argument<T: NativeValue>(&self, index: usize, arguments: &[ValueWire]) -> Result<T> {
        let value = arguments
            .get(index)
            .ok_or_else(|| Error::new(ErrorKind::Arguments, "Missing argument"))?;
        T::from_typed(value, &self.contract.descriptor().types, &self.budget)
    }
}
/// Immutable, context-bound capability. Construction follows shared model admission.
#[derive(Clone)]
pub struct ModelHandle {
    pub(crate) domain: Rc<crate::model_reference::RuntimeReferenceDomain>,
    pub(crate) wire: ValueWire,
    pub(crate) budget: Budget,
}
impl ModelHandle {
    pub fn read(&self, feature: &str) -> Result<serde_json::Value> {
        self.budget.charge(1)?;
        let DataWire::Model(payload) = &self.wire.data else {
            return Err(Error::new(ErrorKind::Arguments, "Invalid model handle"));
        };
        let schema = self.domain.schema();
        let receiver = self.domain.target_key();
        let selected = schema
            .effective_feature(receiver, feature)
            .map_err(|e| Error::new(ErrorKind::Semantic, e))?;
        let projection = self
            .domain
            .stored_feature(payload, receiver, &selected.identity)
            .map_err(|e| Error::new(ErrorKind::Semantic, e))?;
        if projection.definition().reference || projection.definition().upper != 1 {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "Binding read supports singular stored attributes",
            ));
        }
        let value = projection
            .singular_observed(&self.budget.observer())
            .map_err(|e| Error::new(ErrorKind::Semantic, e))?;
        self.budget.check()?;
        Ok(value)
    }
    pub fn read_i64(&self, feature: &str) -> Result<i64> {
        let value = self.read(feature)?;
        value
            .as_i64()
            .or_else(|| {
                (value["kind"] == "int64")
                    .then(|| {
                        value
                            .get("value")
                            .and_then(|v| v.as_str())
                            .and_then(|v| v.parse().ok())
                    })
                    .flatten()
            })
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::Arguments,
                    "Attribute is not a checked signed integer",
                )
            })
    }
}
thread_local! { static INVOKING: Cell<bool> = const { Cell::new(false) }; }
struct InvocationGuard;
impl InvocationGuard {
    fn enter() -> Result<Self> {
        if INVOKING.with(|active| active.replace(true)) {
            Err(Error::new(
                ErrorKind::Unsupported,
                "Nested binding dispatch is outside this execution profile",
            ))
        } else {
            Ok(Self)
        }
    }
}
impl Drop for InvocationGuard {
    fn drop(&mut self) {
        INVOKING.with(|active| active.set(false));
    }
}
impl PinnedCall {
    pub fn identity(&self) -> &ImplementationId {
        &self.identity
    }
    pub fn start<'a>(
        &'a self,
        arguments: &'a [ValueWire],
        context: &ExecutionContext<'a>,
        limits: &'a ExecutionLimits,
        budget: Budget,
    ) -> Result<BindingSession<'a>> {
        BindingSession::new(self, arguments, context, limits, budget)
    }
    pub fn invoke(
        &self,
        arguments: &[ValueWire],
        context: &ExecutionContext<'_>,
        limits: &ExecutionLimits,
        budget: Budget,
    ) -> Result<InvocationResult> {
        let mut session = self.start(arguments, context, limits, budget)?;
        loop {
            match session.resume(NonZeroUsize::MIN) {
                BindingProgress::Pending => {}
                BindingProgress::Complete(result) => return *result,
                BindingProgress::Finished => {
                    return Err(Error::new(
                        ErrorKind::Contract,
                        "Finished binding invocation",
                    ));
                }
            }
        }
    }
}

fn check_conditions(
    conditions: &[intersect_model::definitions::semantic_bindings::Condition],
    arguments: &[ValueWire],
    context: &ExecutionContext<'_>,
    limits: &ExecutionLimits,
    budget: &Budget,
    kind: ErrorKind,
) -> Result<()> {
    conditions::check_with(
        conditions,
        arguments,
        context,
        limits,
        budget,
        kind,
        &conditions::OwnedConditionSessions,
    )
}

/// Typed generated-adapter conversions. Semantic validation occurs before these
/// conversions and again after the provider returns; there is no numeric narrowing.
pub trait NativeValue: Sized {
    fn matches(node: &TypeNode) -> bool;
    fn from_wire(value: &ValueWire) -> Result<Self>;
    fn into_wire(self, ty: TypeId) -> Result<ValueWire>;
    fn matches_type(types: &[TypeNode], ty: TypeId) -> bool {
        types.get(ty.0 as usize).is_some_and(Self::matches)
    }
    fn from_typed(value: &ValueWire, types: &[TypeNode], budget: &Budget) -> Result<Self> {
        budget.charge(1)?;
        if !Self::matches_type(types, value.ty) {
            return Err(Error::new(
                ErrorKind::Arguments,
                "Rust type differs from contract type",
            ));
        }
        Self::from_wire(value)
    }
    fn into_typed(self, ty: TypeId, types: &[TypeNode], budget: &Budget) -> Result<ValueWire> {
        budget.charge(1)?;
        if !Self::matches_type(types, ty) {
            return Err(Error::new(
                ErrorKind::Result,
                "Rust result type differs from contract type",
            ));
        }
        self.into_wire(ty)
    }
}
impl NativeValue for i64 {
    fn matches(node: &TypeNode) -> bool {
        *node == TypeNode::Scalar(Scalar::Int64)
    }
    fn from_wire(value: &ValueWire) -> Result<Self> {
        if let DataWire::Scalar(ir::Value {
            ty: ir::ValueType::Scalar(Scalar::Int64),
            payload,
        }) = &value.data
        {
            return payload
                .get("value")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| Error::new(ErrorKind::Arguments, "Expected exact Int64 payload"));
        }
        Err(Error::new(ErrorKind::Arguments, "Expected Int64"))
    }
    fn into_wire(self, ty: TypeId) -> Result<ValueWire> {
        Ok(ValueWire {
            ty,
            data: DataWire::Scalar(ir::Value {
                ty: ir::ValueType::Scalar(Scalar::Int64),
                payload: intersect_model::definitions::scalars::int64_wire(self),
            }),
        })
    }
}
impl NativeValue for bool {
    fn matches(node: &TypeNode) -> bool {
        *node == TypeNode::Scalar(Scalar::Bool)
    }
    fn from_wire(value: &ValueWire) -> Result<Self> {
        match &value.data {
            DataWire::Scalar(ir::Value {
                ty: ir::ValueType::Scalar(Scalar::Bool),
                payload,
            }) => payload
                .as_bool()
                .ok_or_else(|| Error::new(ErrorKind::Arguments, "Expected Bool")),
            _ => Err(Error::new(ErrorKind::Arguments, "Expected Bool")),
        }
    }
    fn into_wire(self, ty: TypeId) -> Result<ValueWire> {
        Ok(ValueWire {
            ty,
            data: DataWire::Scalar(ir::Value {
                ty: ir::ValueType::Scalar(Scalar::Bool),
                payload: self.into(),
            }),
        })
    }
}
impl NativeValue for String {
    fn matches(node: &TypeNode) -> bool {
        *node == TypeNode::Scalar(Scalar::String)
    }
    fn from_wire(value: &ValueWire) -> Result<Self> {
        match &value.data {
            DataWire::Scalar(ir::Value {
                ty: ir::ValueType::Scalar(Scalar::String),
                payload,
            }) => payload
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| Error::new(ErrorKind::Arguments, "Expected String")),
            _ => Err(Error::new(ErrorKind::Arguments, "Expected String")),
        }
    }
    fn into_wire(self, ty: TypeId) -> Result<ValueWire> {
        Ok(ValueWire {
            ty,
            data: DataWire::Scalar(ir::Value {
                ty: ir::ValueType::Scalar(Scalar::String),
                payload: self.into(),
            }),
        })
    }
}
