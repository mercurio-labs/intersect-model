//! Source-authored Rcore `bind scope` functions as frozen-engine resolvers.
//!
//! Each scope runs on the direct executor (`rcore_fast`) when its program is
//! inside the supported subset, and on the resumable VM otherwise or whenever
//! the direct executor defers a case. Both read the same frozen link view.
//! Reading a reference that is not resolved yet resolves it first
//! (demand-driven linking); a reference that depends on itself reads as
//! pending, which both executors report as an error.
use crate::{
    core::{
        frozen::{
            ClassId, DefaultItem, Defaults, FeatureId, FrozenError, ImplicitObject, ImplicitValue,
            Implicits, Issue, LinkError, LinkValue, LinkView, Metamodel, ObjId, Resolver, Severity,
            Validators,
        },
        query::QueryBudget,
    },
    definitions::{
        bundle::DefinitionBundle, conversion::InvocationContext, generic_types::RuntimeType,
        rcore_behavior::ir_v2::OwnershipOp, rcore_scope::VerifiedScope, resources::DeclarationId,
        schema::MemberIdentity,
    },
    rcore_behavior_functions::{ExecutionContext, ExecutionLimits},
    rcore_fast::{FastClasses, FastError, FastLimits, FastProgram, V},
    scope_runtime::{self, ScopePage, ScopeRead, ScopeSchema, ScopeValue},
    semantic_bindings::BorrowedBindingWork,
};
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};

/// Work allowance for one scope invocation. Budgets apply per resolution,
/// not per evaluator step across the whole link phase.
pub const DEFAULT_SCOPE_BUDGET: usize = 1_000_000;

/// Executor steps allowed to one scope, default or implicit binding run
/// (`RcoreScopes::default_limits`).
pub const DEFAULT_SCOPE_STEPS: usize = 5_000_000;

/// Which executor runs scope functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Execution {
    /// Direct executor when the program is supported, VM otherwise.
    Fast,
    /// Resumable VM only.
    Vm,
    /// Both; any disagreement is an error. For tests and migration checks.
    Shadow,
}

/// How often each executor produced the result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScopeStats {
    /// Results produced (or, in shadow mode, confirmed) by the direct executor.
    pub fast: usize,
    /// VM invocations.
    pub vm: usize,
    /// Direct runs that deferred to the VM.
    pub deferred: usize,
    /// Deferred runs by the direct executor's reason, in reason order.
    pub deferred_by_reason: Vec<(&'static str, usize)>,
}

struct Route<'a> {
    scope: &'a VerifiedScope,
    fast: Option<FastProgram>,
    unsupported: Option<&'static str>,
    target: Option<ClassId>,
}

/// Routes each reference feature with a `bind scope` to its verified function.
/// Features without one go to `fallback`, or stay unresolved.
pub struct RcoreScopes<'a> {
    bundle: &'a DefinitionBundle,
    routes: BTreeMap<FeatureId, Route<'a>>,
    /// `bind default` functions; direct executor only.
    defaults: BTreeMap<FeatureId, Route<'a>>,
    /// `bind implicit` functions by containment feature, each with its
    /// receiver class, in declaration order; direct executor only.
    implicits: BTreeMap<FeatureId, Vec<(ClassId, Route<'a>)>>,
    /// `bind validate` functions with their receiver class, in declaration
    /// order; direct executor, VM fallback.
    validators: Vec<(ClassId, Route<'a>)>,
    limits: ExecutionLimits,
    budget: usize,
    fallback: Option<&'a dyn Resolver>,
    execution: Execution,
    fast: Cell<usize>,
    vm: Cell<usize>,
    deferred: Cell<usize>,
    reasons: RefCell<BTreeMap<&'static str, usize>>,
}

impl<'a> RcoreScopes<'a> {
    /// Each scope's member must be a feature of `metamodel`. Programs are
    /// compiled for the direct executor here, once.
    pub fn new(
        metamodel: &Metamodel,
        bundle: &'a DefinitionBundle,
        scopes: &'a [VerifiedScope],
    ) -> Result<Self, FrozenError> {
        let mut routes = BTreeMap::new();
        let mut defaults = BTreeMap::new();
        let mut implicits: BTreeMap<FeatureId, Vec<(ClassId, Route<'a>)>> = BTreeMap::new();
        let mut validators = Vec::new();
        let classes = FastClasses::default();
        for scope in scopes {
            if scope.is_validate() {
                let schema = metamodel.schema();
                let receiver =
                    classes
                        .class(scope.receiver(), metamodel, schema)
                        .map_err(|_| {
                            FrozenError::Scope(
                                "Validation receiver is not a metamodel class".into(),
                            )
                        })?;
                let (fast, unsupported) = match FastProgram::prepare_with(
                    scope.executable(),
                    metamodel,
                    schema,
                    &classes,
                ) {
                    Ok(program) => (Some(program), None),
                    Err(reason) => (None, Some(reason)),
                };
                validators.push((
                    receiver,
                    Route {
                        scope,
                        fast,
                        unsupported,
                        target: Some(receiver),
                    },
                ));
                continue;
            }
            let MemberIdentity::Source { declaration } = scope.member() else {
                return Err(FrozenError::Scope(
                    "Scope member has no source declaration".into(),
                ));
            };
            let feature = metamodel
                .feature_by_declaration(declaration)
                .ok_or_else(|| {
                    FrozenError::Scope("Scope member is not a feature of this metamodel".into())
                })?;
            let schema = metamodel.schema();
            let target = classes.class(scope.target(), metamodel, schema).ok();
            let (fast, unsupported) =
                match FastProgram::prepare_with(scope.executable(), metamodel, schema, &classes) {
                    Ok(program) if target.is_some() => (Some(program), None),
                    Ok(_) => (None, Some("scope target is not a metamodel class")),
                    Err(reason) => (None, Some(reason)),
                };
            let route = Route {
                scope,
                fast,
                unsupported,
                target,
            };
            if scope.is_implicit() {
                // The containment is inherited by every subclass of its owner,
                // so each binding applies to its own receiver class only.
                let receiver = classes
                    .class(scope.receiver(), metamodel, schema)
                    .ok()
                    .ok_or_else(|| {
                        FrozenError::Scope("Implicit receiver is not a metamodel class".into())
                    })?;
                implicits
                    .entry(feature)
                    .or_default()
                    .push((receiver, route));
                continue;
            }
            let (table, kind) = if scope.is_default() {
                (&mut defaults, "default")
            } else {
                (&mut routes, "scope")
            };
            if table.insert(feature, route).is_some() {
                return Err(FrozenError::Scope(format!(
                    "More than one {kind} for {}",
                    metamodel.feature_name(feature)
                )));
            }
        }
        Ok(Self {
            bundle,
            routes,
            defaults,
            implicits,
            validators,
            limits: Self::default_limits(),
            budget: DEFAULT_SCOPE_BUDGET,
            fallback: None,
            execution: Execution::Fast,
            fast: Cell::new(0),
            vm: Cell::new(0),
            deferred: Cell::new(0),
            reasons: RefCell::new(BTreeMap::new()),
        })
    }
    /// The execution limits scopes run with unless `with_limits` replaces
    /// them: the general defaults, with a step budget of 5,000,000 per run.
    /// A lookup through the SysML unit and shape libraries takes up to a few
    /// million steps and 76 nested calls (`max_frames`, default 256, bounds
    /// both executors' call depth).
    pub fn default_limits() -> ExecutionLimits {
        let mut limits = ExecutionLimits::default();
        limits.scalar.max_steps = DEFAULT_SCOPE_STEPS;
        limits
    }
    fn fast_limits(&self) -> FastLimits {
        FastLimits {
            steps: self.limits.scalar.max_steps,
            call_depth: self.limits.max_frames,
        }
    }
    pub fn with_fallback(mut self, fallback: &'a dyn Resolver) -> Self {
        self.fallback = Some(fallback);
        self
    }
    pub fn with_budget(mut self, budget: usize) -> Self {
        self.budget = budget;
        self
    }
    pub fn with_limits(mut self, limits: ExecutionLimits) -> Self {
        self.limits = limits;
        self
    }
    pub fn with_execution(mut self, execution: Execution) -> Self {
        self.execution = execution;
        self
    }
    /// Why the direct executor does not run a feature's scope, if it does not.
    pub fn unsupported(&self, feature: FeatureId) -> Option<&'static str> {
        self.routes.get(&feature).and_then(|r| r.unsupported)
    }
    pub fn stats(&self) -> ScopeStats {
        ScopeStats {
            fast: self.fast.get(),
            vm: self.vm.get(),
            deferred: self.deferred.get(),
            deferred_by_reason: self
                .reasons
                .borrow()
                .iter()
                .map(|(reason, count)| (*reason, *count))
                .collect(),
        }
    }
}

impl Resolver for RcoreScopes<'_> {
    fn resolve(
        &self,
        view: &LinkView<'_>,
        owner: ObjId,
        feature: FeatureId,
        spelling: &str,
    ) -> Result<Option<ObjId>, FrozenError> {
        if !self.routes.contains_key(&feature) {
            return match self.fallback {
                Some(fallback) => fallback.resolve(view, owner, feature, spelling),
                None => Ok(None),
            };
        }
        match self.candidates(view, owner, feature, spelling)?.as_slice() {
            [] => Ok(None),
            [one] => Ok(Some(*one)),
            _ => Err(FrozenError::Ambiguous(spelling.to_owned())),
        }
    }
}

impl RcoreScopes<'_> {
    /// Every candidate the scope function returns, in its result order,
    /// without repeated identities.
    pub fn candidates(
        &self,
        view: &LinkView<'_>,
        owner: ObjId,
        feature: FeatureId,
        spelling: &str,
    ) -> Result<Vec<ObjId>, FrozenError> {
        let route = self
            .routes
            .get(&feature)
            .ok_or_else(|| FrozenError::Scope("No scope is bound to this feature".into()))?;
        let fast = match (self.execution, &route.fast, route.target) {
            (Execution::Fast | Execution::Shadow, Some(program), Some(target)) => {
                Some(self.run_fast(program, target, view, owner, Some(spelling)))
            }
            _ => None,
        };
        match (self.execution, fast) {
            (Execution::Shadow, Some(fast)) => {
                let vm = self.run_vm(route.scope, view, owner, spelling);
                if matches!(fast, Err(FastError::Fallback(_))) {
                    self.deferred.set(self.deferred.get() + 1);
                    return vm;
                }
                let agree = match (&fast, &vm) {
                    (Ok(a), Ok(b)) => a == b,
                    (Err(FastError::Pending), Err(_)) => true,
                    _ => false,
                };
                if !agree {
                    return Err(FrozenError::Scope(format!(
                        "Direct executor disagrees with the VM: {fast:?} vs {vm:?}"
                    )));
                }
                self.fast.set(self.fast.get() + 1);
                vm
            }
            (_, Some(Ok(found))) => {
                self.fast.set(self.fast.get() + 1);
                Ok(found)
            }
            (_, Some(Err(FastError::Pending))) => {
                self.fast.set(self.fast.get() + 1);
                Err(FrozenError::Scope(
                    "Scope read depends on a pending reference".into(),
                ))
            }
            (_, Some(Err(FastError::Fallback(reason)))) => {
                self.deferred.set(self.deferred.get() + 1);
                *self.reasons.borrow_mut().entry(reason).or_insert(0) += 1;
                self.run_vm(route.scope, view, owner, spelling)
            }
            (_, None) => self.run_vm(route.scope, view, owner, spelling),
        }
    }
    fn run_fast(
        &self,
        program: &FastProgram,
        target: ClassId,
        view: &LinkView<'_>,
        owner: ObjId,
        spelling: Option<&str>,
    ) -> Result<Vec<ObjId>, FastError> {
        let arguments = match spelling {
            Some(spelling) => vec![V::Obj(owner), V::Str(Rc::from(spelling))],
            None => vec![V::Obj(owner)],
        };
        // Pure call results are shared by every run in this view's phase.
        let memo = view.phase_value::<crate::rcore_fast::FastMemo>();
        let result = program.run_memo(view, arguments, self.fast_limits(), &memo)?;
        let V::Seq(items) = result else {
            return Err(FastError::Fallback("scope result is not a sequence"));
        };
        let mm = view.metamodel();
        let mut found: Vec<ObjId> = Vec::with_capacity(items.len());
        for item in items.iter() {
            let V::Obj(obj) = item else {
                return Err(FastError::Fallback("scope candidate is not a model object"));
            };
            let class = view
                .class(*obj)
                .ok_or(FastError::Fallback("candidate class"))?;
            if !mm.is_a(class, target) {
                return Err(FastError::Fallback(
                    "candidate does not conform to the target",
                ));
            }
            if !found.contains(obj) {
                found.push(*obj);
            }
        }
        Ok(found)
    }
    fn run_vm(
        &self,
        scope: &VerifiedScope,
        view: &LinkView<'_>,
        owner: ObjId,
        spelling: &str,
    ) -> Result<Vec<ObjId>, FrozenError> {
        self.vm.set(self.vm.get() + 1);
        let owner_id = view.identity(owner).ok_or(FrozenError::UnknownObject)?;
        let read = FrozenScopeRead { view };
        let mut caller = QueryBudget::new(self.budget);
        let work = BorrowedBindingWork::bind(&mut caller);
        let candidates = scope_runtime::evaluate(
            scope,
            &ExecutionContext::new(self.bundle),
            view.metamodel().schema(),
            &read,
            owner_id,
            spelling,
            &self.limits,
            &work,
        )
        .map_err(|error| FrozenError::Scope(error.to_string()))?;
        candidates
            .identities()
            .iter()
            .map(|id| view.lookup(id).ok_or(FrozenError::UnknownObject))
            .collect()
    }
}

impl Implicits for RcoreScopes<'_> {
    fn features(&self) -> Vec<FeatureId> {
        self.implicits.keys().copied().collect()
    }
    fn objects(
        &self,
        view: &LinkView<'_>,
        obj: ObjId,
        feature: FeatureId,
    ) -> Result<Vec<ImplicitObject>, FrozenError> {
        let routes = self
            .implicits
            .get(&feature)
            .ok_or_else(|| FrozenError::Scope("No implicit binding for this feature".into()))?;
        let class = view.class(obj).ok_or(FrozenError::UnknownObject)?;
        let mut objects = Vec::new();
        for (receiver, route) in routes {
            if !view.metamodel().is_a(class, *receiver) {
                continue;
            }
            let Some(program) = &route.fast else {
                return Err(FrozenError::Scope(format!(
                    "Unsupported: implicit binding needs the direct executor ({})",
                    route.unsupported.unwrap_or("not prepared")
                )));
            };
            self.fast.set(self.fast.get() + 1);
            let result = program
                .run(view, vec![V::Obj(obj)], self.fast_limits())
                .map_err(|error| match error {
                    FastError::Pending => {
                        FrozenError::Scope("Implicit binding read a pending reference".into())
                    }
                    FastError::Fallback(reason) => FrozenError::Scope(format!(
                        "Unsupported: implicit binding outside the direct executor's subset ({reason})"
                    )),
                })?;
            let V::Seq(items) = result else {
                return Err(FrozenError::Scope(
                    "Implicit binding result is not a sequence".into(),
                ));
            };
            for item in items.iter() {
                objects.push(implicit_object(item)?);
            }
        }
        Ok(objects)
    }
}

impl Validators for RcoreScopes<'_> {
    fn is_empty(&self) -> bool {
        self.validators.is_empty()
    }
    fn issues(&self, view: &LinkView<'_>, obj: ObjId) -> Result<Vec<Issue>, FrozenError> {
        let class = view.class(obj).ok_or(FrozenError::UnknownObject)?;
        let mut issues = Vec::new();
        for (receiver, route) in &self.validators {
            if !view.metamodel().is_a(class, *receiver) {
                continue;
            }
            let fast = match (self.execution, &route.fast) {
                (Execution::Fast | Execution::Shadow, Some(program)) => {
                    let memo = view.phase_value::<crate::rcore_fast::FastMemo>();
                    Some(
                        program
                            .run_memo(view, vec![V::Obj(obj)], self.fast_limits(), &memo)
                            .and_then(|value| fast_issues(&value)),
                    )
                }
                _ => None,
            };
            let found = match fast {
                Some(Ok(found)) if self.execution != Execution::Shadow => {
                    self.fast.set(self.fast.get() + 1);
                    found
                }
                Some(Ok(found)) => {
                    let vm = self.run_vm_validate(route.scope, view, obj)?;
                    if vm != found {
                        return Err(FrozenError::Scope(format!(
                            "Direct executor disagrees with the VM: {found:?} vs {vm:?}"
                        )));
                    }
                    self.fast.set(self.fast.get() + 1);
                    vm
                }
                Some(Err(FastError::Pending)) => {
                    return Err(FrozenError::Scope(
                        "Validation read a pending reference".into(),
                    ));
                }
                Some(Err(FastError::Fallback(reason))) => {
                    self.deferred.set(self.deferred.get() + 1);
                    *self.reasons.borrow_mut().entry(reason).or_insert(0) += 1;
                    self.run_vm_validate(route.scope, view, obj)?
                }
                None => self.run_vm_validate(route.scope, view, obj)?,
            };
            issues.extend(found);
        }
        Ok(issues)
    }
}

impl RcoreScopes<'_> {
    /// Run one validation on the resumable VM.
    fn run_vm_validate(
        &self,
        scope: &VerifiedScope,
        view: &LinkView<'_>,
        owner: ObjId,
    ) -> Result<Vec<Issue>, FrozenError> {
        self.vm.set(self.vm.get() + 1);
        let owner_id = view.identity(owner).ok_or(FrozenError::UnknownObject)?;
        let read = FrozenScopeRead { view };
        let mut caller = QueryBudget::new(self.budget);
        let work = BorrowedBindingWork::bind(&mut caller);
        let value = scope_runtime::evaluate_read_function(
            scope.executable(),
            &ExecutionContext::new(self.bundle),
            view.metamodel().schema(),
            &read,
            owner_id,
            Vec::new(),
            &self.limits,
            &work,
        )
        .map_err(|error| FrozenError::Scope(error.to_string()))?;
        vm_issues(&value.data)
    }
}

/// One `Issue` record's fields, all strings, as an engine issue.
fn issue_of(field: impl Fn(&str) -> Option<String>) -> Result<Issue, FrozenError> {
    let text = |name: &str| {
        field(name).ok_or_else(|| FrozenError::Scope(format!("Issue field {name} is not a String")))
    };
    let severity = text("severity")?;
    let feature = text("feature")?;
    let rule = text("rule")?;
    Ok(Issue {
        severity: Severity::named(&severity).ok_or_else(|| {
            FrozenError::Scope(format!(
                "Unknown issue severity {severity:?}; expected error, warning or info"
            ))
        })?,
        code: text("code")?,
        message: text("message")?,
        feature: (!feature.is_empty()).then_some(feature),
        rule: (!rule.is_empty()).then_some(rule),
    })
}

/// The issues a direct run returned.
fn fast_issues(value: &V) -> Result<Vec<Issue>, FastError> {
    let V::Seq(items) = value else {
        return Err(FastError::Fallback("validation result is not a sequence"));
    };
    items
        .iter()
        .map(|item| {
            issue_of(|name| match item.field(name) {
                Some(V::Str(text)) => Some(text.to_string()),
                _ => None,
            })
            .map_err(|_| FastError::Fallback("validation issue"))
        })
        .collect()
}

/// The issues a VM run returned.
fn vm_issues(
    value: &crate::definitions::rcore_behavior::ir_v2::DataWire,
) -> Result<Vec<Issue>, FrozenError> {
    use crate::definitions::rcore_behavior::ir_v2::DataWire;
    let DataWire::Collection(items) = value else {
        return Err(FrozenError::Scope(
            "Validation result is not a sequence".into(),
        ));
    };
    items
        .iter()
        .map(|item| {
            let DataWire::Record(fields) = &item.data else {
                return Err(FrozenError::Scope(
                    "Validation issue is not a record".into(),
                ));
            };
            issue_of(|name| match fields.get(name).map(|field| &field.data) {
                Some(DataWire::Scalar(scalar)) => scalar.payload.as_str().map(str::to_owned),
                _ => None,
            })
        })
        .collect()
}

/// The engine form of an object `new` described.
fn implicit_object(value: &V) -> Result<ImplicitObject, FrozenError> {
    let V::New(object) = value else {
        return Err(FrozenError::Scope(
            "Implicit binding must return objects built with `new`".into(),
        ));
    };
    let mut values = Vec::with_capacity(object.values.len());
    for (feature, value) in &object.values {
        let mut items = Vec::new();
        implicit_values(value, &mut items)?;
        values.push((*feature, items));
    }
    Ok(ImplicitObject {
        class: object.class,
        values,
    })
}

/// A `new` feature value as engine values: options and collections flatten
/// to their items.
fn implicit_values(value: &V, items: &mut Vec<ImplicitValue>) -> Result<(), FrozenError> {
    match value {
        V::None => {}
        V::Some(inner) => implicit_values(inner, items)?,
        V::Seq(values) => {
            for value in values.iter() {
                implicit_values(value, items)?;
            }
        }
        V::New(_) => items.push(ImplicitValue::Object(implicit_object(value)?)),
        V::Obj(obj) => items.push(ImplicitValue::Ref(*obj)),
        V::Str(text) => items.push(ImplicitValue::Value(Value::String(text.to_string()))),
        V::Bool(value) => items.push(ImplicitValue::Value(Value::Bool(*value))),
        V::Int(value) => items.push(ImplicitValue::Value(Value::from(*value))),
        V::Enum(value) => items.push(ImplicitValue::Value(Value::clone(value))),
        V::Fun(_) => {
            return Err(FrozenError::Scope("A function is not a model value".into()));
        }
        V::Record(_) => {
            return Err(FrozenError::Scope("A record is not a model value".into()));
        }
    }
    Ok(())
}

impl Defaults for RcoreScopes<'_> {
    fn features(&self) -> Vec<FeatureId> {
        self.defaults.keys().copied().collect()
    }
    fn values(
        &self,
        view: &LinkView<'_>,
        obj: ObjId,
        feature: FeatureId,
    ) -> Result<Vec<DefaultItem>, FrozenError> {
        let route = self
            .defaults
            .get(&feature)
            .ok_or_else(|| FrozenError::Scope("No default is bound to this feature".into()))?;
        let (Some(program), Some(target)) = (&route.fast, route.target) else {
            return Err(FrozenError::Scope(format!(
                "Unsupported: default needs the direct executor ({})",
                route.unsupported.unwrap_or("not prepared")
            )));
        };
        self.fast.set(self.fast.get() + 1);
        let failed = |error| match error {
            FastError::Pending => FrozenError::Scope("Default read a pending reference".into()),
            FastError::Fallback(reason) => FrozenError::Scope(format!(
                "Unsupported: default outside the direct executor's subset ({reason})"
            )),
        };
        if route.scope.binding().attribute.is_none() {
            return self
                .run_fast(program, target, view, obj, None)
                .map(|found| found.into_iter().map(DefaultItem::Object).collect())
                .map_err(failed);
        }
        let result = program
            .run(view, vec![V::Obj(obj)], self.fast_limits())
            .map_err(failed)?;
        let V::Seq(items) = result else {
            return Err(failed(FastError::Fallback(
                "default result is not a sequence",
            )));
        };
        items
            .iter()
            .map(|item| match item {
                V::Str(text) => Ok(DefaultItem::Value(Value::String(text.to_string()))),
                V::Bool(value) => Ok(DefaultItem::Value(Value::Bool(*value))),
                V::Int(value) => Ok(DefaultItem::Value(Value::from(*value))),
                V::Enum(value) => Ok(DefaultItem::Value(Value::clone(value))),
                _ => Err(failed(FastError::Fallback(
                    "attribute default values other than String, Bool, Int64 and enumerations",
                ))),
            })
            .collect()
    }
}

/// `ScopeRead` over a frozen link view. Object identities are the model's
/// string identities; members are mapped through their source declarations.
pub struct FrozenScopeRead<'v, 'a> {
    view: &'v LinkView<'a>,
}

impl<'v, 'a> FrozenScopeRead<'v, 'a> {
    pub fn new(view: &'v LinkView<'a>) -> Self {
        Self { view }
    }
    fn object(&self, object: &str) -> Result<ObjId, String> {
        self.view
            .lookup(object)
            .ok_or_else(|| "Missing Rcore scope object".into())
    }
    /// A stored member of the object's class, with its multiplicity checked.
    fn member(
        &self,
        object: &str,
        declaration: &DeclarationId,
        many: bool,
    ) -> Result<(ObjId, FeatureId), String> {
        let obj = self.object(object)?;
        let mm = self.view.metamodel();
        let class = self.view.class(obj).ok_or("Missing Rcore scope object")?;
        let feature = mm
            .feature_by_declaration(declaration)
            .filter(|f| mm.slot(class, *f).is_some())
            .ok_or("Rcore scope member is not a stored feature of the receiver")?;
        let info = mm
            .feature_info(feature)
            .ok_or("Unknown Rcore scope member")?;
        let volatile = mm.definition(feature).is_some_and(|d| d.volatile);
        if info.many != many || info.derived || volatile {
            return Err(if many {
                "Rcore scope requires an admitted stored collection".into()
            } else {
                "Rcore scope requires an admitted singular stored feature".into()
            });
        }
        Ok((obj, feature))
    }
    fn json(&self, value: LinkValue<'_>) -> Result<Value, String> {
        Ok(match value {
            LinkValue::Absent | LinkValue::Null => Value::Null,
            LinkValue::Default(v) | LinkValue::Json(v) => v.clone(),
            LinkValue::Bool(v) => Value::Bool(v),
            LinkValue::Int(v) => Value::from(v),
            LinkValue::Str(v) => Value::String(v.to_owned()),
            LinkValue::Ref(o) => Value::String(
                self.view
                    .identity(o)
                    .ok_or("Missing Rcore scope target")?
                    .to_owned(),
            ),
            LinkValue::Many(_) => return Err("Nested Rcore scope collection".into()),
        })
    }
}

impl ScopeRead for FrozenScopeRead<'_, '_> {
    fn check(&self, observation: &InvocationContext<'_>) -> Result<(), String> {
        // The link view cannot change during the phase; only cancellation applies.
        observation.checkpoint()
    }
    fn class(&self, object: &str, observation: &InvocationContext<'_>) -> Result<String, String> {
        observation.work(1)?;
        let obj = self.object(object)?;
        let class = self.view.class(obj).ok_or("Missing Rcore scope object")?;
        Ok(self.view.metamodel().class_key(class).to_owned())
    }
    fn singular(
        &self,
        object: &str,
        member: &DeclarationId,
        observation: &InvocationContext<'_>,
    ) -> Result<ScopeValue, String> {
        observation.work(1)?;
        let (obj, feature) = self.member(object, member, false)?;
        match self.view.value(obj, feature) {
            Err(LinkError::Cycle | LinkError::Unresolved) => Ok(ScopeValue::Pending),
            Ok(None) | Ok(Some(LinkValue::Absent)) => Ok(ScopeValue::Absent),
            Ok(Some(value)) => self.json(value).map(ScopeValue::Value),
        }
    }
    fn page(
        &self,
        object: &str,
        member: &DeclarationId,
        cursor: usize,
        observation: &InvocationContext<'_>,
    ) -> Result<ScopePage, String> {
        observation.work(1)?;
        let (obj, feature) = self.member(object, member, true)?;
        if matches!(
            self.view.value(obj, feature),
            Ok(Some(LinkValue::Absent)) | Ok(None)
        ) {
            return Err("Missing Rcore scope collection".into());
        }
        match self.view.item(obj, feature, cursor) {
            Err(LinkError::Cycle | LinkError::Unresolved) => Ok(ScopePage::Pending),
            Ok(None) => Ok(ScopePage::End),
            Ok(Some(value)) => self.json(value).map(ScopePage::Value),
        }
    }
    fn ownership(
        &self,
        object: &str,
        operation: OwnershipOp,
        selected: &RuntimeType,
        cursor: usize,
        schema: &ScopeSchema<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<ScopePage, String> {
        observation.work(1)?;
        let obj = self.object(object)?;
        let expected = schema.class_key_for_type_observed(selected, observation)?;
        let mm = self.view.metamodel();
        let conforms = |candidate: ObjId| -> Result<bool, String> {
            let class = self
                .view
                .class(candidate)
                .ok_or("Missing Rcore scope object")?;
            schema.conforms_observed(mm.class_key(class), expected, observation)
        };
        let mut matched = 0usize;
        let mut found = None;
        match operation {
            OwnershipOp::AsType => {
                return Err("Checked type views use the shared source authority".into());
            }
            OwnershipOp::Container => {
                if let Some((parent, _)) = self.view.container(obj)
                    && cursor == 0
                    && conforms(parent)?
                {
                    found = Some(parent);
                }
            }
            OwnershipOp::Ancestors => {
                let mut current = obj;
                let mut steps = 0usize;
                while let Some((parent, _)) = self.view.container(current) {
                    steps += 1;
                    if steps > self.view.len() {
                        return Err("Rcore scope containment cycle".into());
                    }
                    observation.work(1)?;
                    if conforms(parent)? {
                        if matched == cursor {
                            found = Some(parent);
                            break;
                        }
                        matched += 1;
                    }
                    current = parent;
                }
            }
            OwnershipOp::Roots => {
                for root in self.view.roots() {
                    observation.work(1)?;
                    if conforms(*root)? {
                        if matched == cursor {
                            found = Some(*root);
                            break;
                        }
                        matched += 1;
                    }
                }
            }
            OwnershipOp::Contents => {
                for child in self.view.children(obj) {
                    observation.work(1)?;
                    if conforms(*child)? {
                        if matched == cursor {
                            found = Some(*child);
                            break;
                        }
                        matched += 1;
                    }
                }
            }
        }
        match found {
            Some(target) => Ok(ScopePage::Value(Value::String(
                self.view
                    .identity(target)
                    .ok_or("Missing Rcore scope target")?
                    .to_owned(),
            ))),
            None => Ok(ScopePage::End),
        }
    }
}
