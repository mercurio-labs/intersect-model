//! Lexical binding uses iterative arena traversal; instance members await typing.
use super::{
    SyntaxDiagnostic,
    ast::*,
    modules::{
        ImportBinding, ModuleIndex, ModuleInterface, SymbolId, index_modules_with_resources,
    },
};
use crate::definitions::bundle::{DefinitionBundle, SourceId, SourceSpan};
use crate::definitions::continuation::pause;
use crate::definitions::resources::{DeclarationId, ResourceSet};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    future::Future,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
};

#[path = "rcore_behavior_binding_metadata.rs"]
mod metadata;
use metadata::MetadataDriver;

const BUILTIN_TYPES: &[&str] = &[
    "Bool",
    "Int",
    "Int8",
    "Int16",
    "Int32",
    "Int64",
    "UInt8",
    "UInt16",
    "UInt32",
    "UInt64",
    "BigInt",
    "Decimal",
    "Float32",
    "Float64",
    "String",
    "Bytes",
    "Char16",
    "DateInstant",
    "Unit",
    "Option",
    "Result",
    "Sequence",
    "Set",
    "OrderedSet",
    "Bag",
];
const BUILTIN_VALUES: &[&str] = &[
    "groupBy",
    "sortedWith",
    "sortedBy",
    "map",
    "filter",
    "flatMap",
    "fold",
    "exists",
    "forAll",
    "one",
    "find",
    "union",
    "intersection",
    "difference",
    "concat",
    "distinct",
    "contains",
    "excludes",
    "count",
    "includesAll",
    "includesAny",
    "flatten",
    "first",
    "last",
    "at",
    "size",
    "isEmpty",
    "incoming",
    "container",
    "contents",
    "ancestors",
    "roots",
    "split",
    "hex",
    "identity",
    "resource",
    "asType",
    "closure",
    "Some",
    "None",
    "Ok",
    "Err",
    "Sequence",
    "Set",
    "OrderedSet",
    "Bag",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum LocalKind {
    Parameter,
    Receiver,
    Generic,
    Let,
    Pattern,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Target {
    Local {
        declaration: SourceSpan,
        kind: LocalKind,
    },
    Symbol(SymbolId),
    /// Complete visible source function set; selection belongs to the checker.
    Overload(Vec<SymbolId>),
    Builtin {
        version: u32,
        name: String,
    },
    Module(SourceId),
    Model(DeclarationId),
    Classifier(DeclarationId),
    Feature(DeclarationId),
    EnumLiteral {
        enumeration: DeclarationId,
        literal: DeclarationId,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModuleBindings {
    pub source: SourceId,
    pub expressions: BTreeMap<usize, Target>,
    pub types: BTreeMap<usize, Target>,
    pub constructors: BTreeMap<usize, Target>,
    pub providers: BTreeMap<usize, Target>,
    /// Resolved source selectors only; the scope contract compiler checks types
    /// and compiles the selected implementation before runtime installation.
    pub scope_bindings: BTreeMap<usize, BoundScopeBinding>,
    /// `bind default` selectors, resolved like scope selectors.
    pub default_bindings: BTreeMap<usize, BoundScopeBinding>,
    /// `bind implicit` selectors, resolved like scope selectors.
    pub implicit_bindings: BTreeMap<usize, BoundScopeBinding>,
    /// `bind validate` selectors. A validation binding names a class, not a
    /// feature: its `member` is the receiver class declaration itself.
    pub validate_bindings: BTreeMap<usize, BoundScopeBinding>,
    /// These receivers are lexically checked; member selection belongs to S03.
    pub instance_members: Vec<ExprId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BoundScopeBinding {
    pub receiver: DeclarationId,
    pub member: DeclarationId,
    pub implementation: SymbolId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BoundModules {
    pub index: ModuleIndex,
    pub bindings: Vec<ModuleBindings>,
}

/// Bind source-derived interfaces and bodies. No caller-supplied AST is trusted.
pub fn bind(
    bundle: &DefinitionBundle,
    entry: &SourceId,
) -> Result<BoundModules, Vec<SyntaxDiagnostic>> {
    bind_with_limits(bundle, entry, Default::default())
}

pub(crate) fn bind_with_limits(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<BoundModules, Vec<SyntaxDiagnostic>> {
    bind_with_resources(bundle, entry, limits).map(|(bound, _)| bound)
}

pub(crate) fn bind_with_resources(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<(BoundModules, ResourceSet), Vec<SyntaxDiagnostic>> {
    let (index, resources) = index_modules_with_resources(bundle, entry, limits)?;
    bind_prepared(bundle, index, resources, limits)
}

pub(super) fn bind_prepared(
    bundle: &DefinitionBundle,
    index: ModuleIndex,
    resources: ResourceSet,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<(BoundModules, ResourceSet), Vec<SyntaxDiagnostic>> {
    let entry = index.entry.clone();
    let bridge = MetadataBridge::default();
    let mut metadata = MetadataDriver::new(bundle, resources.clone(), limits);
    let future = bind_prepared_retained(index, resources, bridge.clone());
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(result) = future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            return result;
        }
        if let Err(error) = metadata.advance(
            &bridge,
            &crate::definitions::conversion::InvocationContext::default(),
        ) {
            return Err(vec![SyntaxDiagnostic {
                span: SourceSpan {
                    source: entry,
                    start: 0,
                    end: 0,
                },
                message: error.to_string(),
            }]);
        }
    }
}

// Pure AST/name traversal. Nested imports and policy resolution are handled by
// the ordinary driver outside this future; their retained preparation is separate.
async fn bind_prepared_retained(
    index: ModuleIndex,
    resources: ResourceSet,
    bridge: MetadataBridge,
) -> Result<(BoundModules, ResourceSet), Vec<SyntaxDiagnostic>> {
    let mut errors = Vec::new();
    let mut bindings = Vec::new();
    for module in &index.modules {
        pause().await;
        let mut binder = Binder {
            bridge: &bridge,
            index: &index,
            module,
            resources: &resources,
            scopes: vec![],
            errors: vec![],
            output: ModuleBindings {
                source: module.document.source.id.clone(),
                expressions: BTreeMap::new(),
                types: BTreeMap::new(),
                constructors: BTreeMap::new(),
                providers: BTreeMap::new(),
                scope_bindings: BTreeMap::new(),
                default_bindings: BTreeMap::new(),
                implicit_bindings: BTreeMap::new(),
                validate_bindings: BTreeMap::new(),
                instance_members: vec![],
            },
        };
        binder.declarations().await;
        errors.extend(binder.errors);
        bindings.push(binder.output);
    }
    errors.sort_by(|a, b| {
        (&a.span.source, a.span.start, &a.message).cmp(&(&b.span.source, b.span.start, &b.message))
    });
    if errors.is_empty() {
        Ok((BoundModules { index, bindings }, resources))
    } else {
        Err(errors)
    }
}

// No registry or observed execution context is stored in this bridge. Requests
// preserve the old parser-policy boundary and are resolved by the outer driver.
#[derive(Clone, Default)]
struct MetadataBridge(Arc<Mutex<MetadataExchange>>);
#[derive(Default)]
struct MetadataExchange {
    request: Option<MetadataQuery>,
    reply: Option<Result<Target, String>>,
}
enum MetadataQuery {
    EnumLiteral {
        classifier: DeclarationId,
        name: String,
    },
    ModelFeature {
        expression: crate::definitions::generic_types::Type,
        name: String,
    },
    ContractFeature {
        receiver: crate::definitions::generic_types::Type,
        name: String,
        span: SourceSpan,
    },
}
impl MetadataBridge {
    async fn query(&self, request: MetadataQuery) -> Result<Target, String> {
        {
            let mut state = self
                .0
                .lock()
                .map_err(|_| "Binding metadata bridge lock poisoned")?;
            if state.request.is_some() || state.reply.is_some() {
                return Err("Binding metadata bridge already has a request".into());
            }
            state.request = Some(request);
        }
        std::future::poll_fn(|_| {
            let mut state = match self.0.lock() {
                Ok(state) => state,
                Err(_) => return Poll::Ready(Err("Binding metadata bridge lock poisoned".into())),
            };
            match state.reply.take() {
                Some(reply) => Poll::Ready(reply),
                None => Poll::Pending,
            }
        })
        .await
    }
    fn take_request(&self) -> Result<Option<MetadataQuery>, String> {
        Ok(self
            .0
            .lock()
            .map_err(|_| "Binding metadata bridge lock poisoned")?
            .request
            .take())
    }
    fn publish(&self, reply: Result<Target, String>) -> Result<(), String> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| "Binding metadata bridge lock poisoned")?;
        if state.request.is_some() || state.reply.is_some() {
            return Err("Binding metadata bridge already has a request".into());
        }
        state.reply = Some(reply);
        Ok(())
    }
}
impl MetadataQuery {
    #[cfg(test)]
    fn resolve(
        self,
        bundle: &DefinitionBundle,
        limits: crate::definitions::ecore::ParseLimits,
    ) -> Result<Target, String> {
        match self {
            Self::EnumLiteral { classifier, name } => {
                let enumeration =
                    crate::definitions::compiler::import_enum_bundle(bundle, &classifier)
                        .map_err(|error| error.to_string())?;
                let literal = enumeration
                    .by_name(&name)
                    .ok_or_else(|| format!("Unknown enum literal {name}"))?;
                Ok(Target::EnumLiteral {
                    enumeration: classifier,
                    literal: literal.declaration.clone(),
                })
            }
            Self::ModelFeature { expression, name } => {
                let projection = super::model_types::ModelTypes::new(bundle)?
                    .feature_projection(&expression, &name)?;
                Ok(Target::Feature(projection.declaration))
            }
            Self::ContractFeature {
                receiver,
                name,
                span,
            } => {
                let resolver = crate::definitions::generic_resolution::Resolver::new_with_limits(
                    bundle, limits,
                )
                .map_err(|error| error.to_string())?;
                resolver
                    .declared_feature(&receiver, &name, &span)
                    .map(Target::Feature)
                    .map_err(|error| error.to_string())
            }
        }
    }
}

fn callable_target(mut symbols: Vec<SymbolId>) -> Result<Target, String> {
    symbols.sort();
    symbols.dedup();
    match symbols.as_slice() {
        [] => Err("Empty declaration candidate set".into()),
        [symbol] => Ok(Target::Symbol(symbol.clone())),
        _ => Ok(Target::Overload(symbols)),
    }
}

enum Work {
    Expr(ExprId),
    Copy(ExprId, ExprId),
    Member(ExprId, ExprId, Name, bool),
    Type(TypeId),
    Pattern(PatternId),
    PatternBindings(PatternId),
    Enter,
    Exit,
    Bind(Name, LocalKind),
}
struct Binder<'a> {
    bridge: &'a MetadataBridge,
    index: &'a ModuleIndex,
    module: &'a ModuleInterface,
    resources: &'a ResourceSet,
    scopes: Vec<BTreeMap<String, Target>>,
    errors: Vec<SyntaxDiagnostic>,
    output: ModuleBindings,
}
impl Binder<'_> {
    fn error(&mut self, span: &SourceSpan, message: impl Into<String>) {
        self.errors.push(SyntaxDiagnostic {
            span: span.clone(),
            message: message.into(),
        });
    }
    fn local(&mut self, name: &Name, kind: LocalKind) {
        let target = Target::Local {
            declaration: name.span.clone(),
            kind,
        };
        if let Some(scope) = self.scopes.last_mut() {
            if scope.contains_key(&name.value) {
                self.error(
                    &name.span,
                    format!("Duplicate local binding {}", name.value),
                );
            } else {
                scope.insert(name.value.clone(), target);
            }
        } else {
            self.error(&name.span, "Missing lexical scope");
        }
    }
    async fn name(&self, name: &Name, ty: bool) -> Result<Target, String> {
        let builtins = if ty { BUILTIN_TYPES } else { BUILTIN_VALUES };
        if let Some(builtin) = name.value.strip_prefix(BUILTIN_PREFIX) {
            return if builtins.contains(&builtin) {
                Ok(Target::Builtin {
                    version: 2,
                    name: builtin.into(),
                })
            } else {
                Err(format!("Unknown parser builtin {builtin}"))
            };
        }
        for scope in self.scopes.iter().rev() {
            pause().await;
            if let Some(target) = scope.get(&name.value)
                && (!ty
                    || matches!(
                        target,
                        Target::Local {
                            kind: LocalKind::Generic,
                            ..
                        }
                    ))
            {
                return Ok(target.clone());
            }
        }
        if let Some(binding) = self.module.imports.get(&name.value) {
            return Ok(match binding {
                ImportBinding::Module(id) => Target::Module(id.clone()),
                ImportBinding::Model(id) => Target::Model(id.clone()),
            });
        }
        if !ty {
            let mut symbols = Vec::new();
            for symbol in &self.module.symbols {
                pause().await;
                if symbol.name == [name.value.clone()] {
                    symbols.push(symbol.id.clone());
                }
            }
            if !symbols.is_empty() {
                return callable_target(symbols);
            }
        }
        if builtins.contains(&name.value.as_str()) {
            return Ok(Target::Builtin {
                version: 2,
                name: name.value.clone(),
            });
        }
        Err(format!(
            "Unresolved {} name {}",
            if ty { "type" } else { "value" },
            name.value
        ))
    }
    async fn member(&self, receiver: &Target, name: &Name) -> Result<Option<Target>, String> {
        match receiver {
            Target::Module(source) => {
                let mut found = None;
                for candidate in &self.index.modules {
                    pause().await;
                    if &candidate.document.source.id == source {
                        found = Some(candidate);
                        break;
                    }
                }
                let module = found.ok_or_else(|| {
                    format!("Unknown module source {}@{}", source.uri, source.sha256)
                })?;
                let mut matching = false;
                let mut symbols = Vec::new();
                for symbol in &module.symbols {
                    pause().await;
                    if symbol.name == [name.value.clone()] {
                        matching = true;
                        if symbol.public {
                            symbols.push(symbol.id.clone());
                        }
                    }
                }
                if !matching {
                    return Err(format!("Unknown module export {}", name.value));
                }
                if symbols.is_empty() {
                    return Err(format!("Private declaration {}", name.value));
                }
                callable_target(symbols).map(Some)
            }
            Target::Model(package) => {
                let node = self.resources.node(package)?;
                for (index, child) in node.children.iter().enumerate() {
                    pause().await;
                    if child.name.local == "eClassifiers"
                        && child.name.namespace.is_none()
                        && child.attributes.iter().any(|a| {
                            a.name.namespace.is_none()
                                && a.name.local == "name"
                                && a.value == name.value
                        })
                    {
                        let mut id = package.clone();
                        id.path.push(index);
                        return Ok(Some(Target::Classifier(id)));
                    }
                }
                Err(format!("Unknown model classifier {}", name.value))
            }
            Target::Classifier(classifier) => {
                let node = self.resources.node(classifier)?;
                let kind = node
                    .attributes
                    .iter()
                    .find(|attribute| {
                        attribute.name.namespace.as_deref() == Some(crate::definitions::ecore::XSI)
                            && attribute.name.local == "type"
                    })
                    .and_then(|attribute| attribute.type_name.as_ref())
                    .filter(|name| {
                        name.namespace.as_deref() == Some(crate::definitions::ecore::ECORE)
                    })
                    .map(|name| name.local.as_str());
                if kind == Some("EEnum") {
                    return self
                        .bridge
                        .query(MetadataQuery::EnumLiteral {
                            classifier: classifier.clone(),
                            name: name.value.clone(),
                        })
                        .await
                        .map(Some);
                }
                if kind != Some("EClass") {
                    return Err("Feature namespace requires an EClass or EEnum".into());
                }
                let expression = crate::definitions::generic_types::Type::Classifier {
                    declaration: classifier.clone(),
                    arguments: vec![],
                    raw: node.children.iter().any(|child| {
                        child.name.namespace.is_none() && child.name.local == "eTypeParameters"
                    }),
                };
                // Bind source identity through the same checked runtime projection
                // as feature typing, including inherited raw generic owners.
                self.bridge
                    .query(MetadataQuery::ModelFeature {
                        expression,
                        name: name.value.clone(),
                    })
                    .await
                    .map(Some)
            }
            _ => Ok(None),
        }
    }
    async fn path(&self, path: &Path, ty: bool) -> Result<Target, String> {
        let first = path.first().ok_or("Empty name path")?;
        let mut target = self.name(first, ty).await?;
        for name in &path[1..] {
            pause().await;
            target = self
                .member(&target, name)
                .await?
                .ok_or_else(|| format!("Not a declaration namespace: {}", first.value))?;
        }
        if ty
            && matches!(
                target,
                Target::Module(_)
                    | Target::Model(_)
                    | Target::Symbol(_)
                    | Target::Overload(_)
                    | Target::Feature(_)
                    | Target::EnumLiteral { .. }
            )
        {
            return Err("Type name must identify a classifier, generic or builtin type".into());
        }
        Ok(target)
    }
    async fn declarations(&mut self) {
        for (ordinal, item) in self.module.document.declarations.iter().enumerate() {
            pause().await;
            self.scopes = vec![BTreeMap::new()];
            let mut work = Vec::new();
            match &item.value {
                Declaration::ScopeBinding {
                    target,
                    implementation,
                }
                | Declaration::DefaultBinding {
                    target,
                    implementation,
                }
                | Declaration::ImplicitBinding {
                    target,
                    implementation,
                } => {
                    let default = matches!(&item.value, Declaration::DefaultBinding { .. });
                    let implicit = matches!(&item.value, Declaration::ImplicitBinding { .. });
                    let resolved = async {
                        if target.len() < 3 {
                            return Err("Scope target requires alias.Class.reference".to_owned());
                        }
                        let Target::Classifier(receiver) = self
                            .path(&target[..target.len() - 1].to_vec(), false)
                            .await?
                        else {
                            return Err("Scope receiver must identify a model class".to_owned());
                        };
                        let Target::Feature(member) = self.contract_feature(target).await? else {
                            return Err("Scope target must identify a reference feature".to_owned());
                        };
                        let Target::Symbol(symbol) = self.path(implementation, false).await? else {
                            return Err(
                                "Scope implementation requires one unambiguous source function"
                                    .to_owned(),
                            );
                        };
                        let module = self.index.module(&symbol.source)?;
                        if !module.symbols.iter().any(|candidate| {
                            candidate.id == symbol
                                && candidate.kind == super::modules::SymbolKind::Function
                        }) {
                            return Err(
                                "Scope implementation must identify a source function".to_owned()
                            );
                        }
                        Ok(BoundScopeBinding {
                            receiver,
                            member,
                            implementation: symbol,
                        })
                    }
                    .await;
                    let output = if implicit {
                        &mut self.output.implicit_bindings
                    } else if default {
                        &mut self.output.default_bindings
                    } else {
                        &mut self.output.scope_bindings
                    };
                    match resolved {
                        Ok(binding)
                            if output.values().any(|previous| {
                                previous.receiver == binding.receiver
                                    && previous.member == binding.member
                            }) =>
                        {
                            self.error(
                                &item.span,
                                if implicit {
                                    "Duplicate implicit binding for the same receiver and reference"
                                } else if default {
                                    "Duplicate default binding for the same receiver and reference"
                                } else {
                                    "Duplicate scope binding for the same receiver and reference"
                                },
                            )
                        }
                        Ok(binding) => {
                            output.insert(ordinal, binding);
                        }
                        Err(message) => self.error(&item.span, message),
                    }
                }
                Declaration::ValidateBinding {
                    target,
                    implementation,
                } => {
                    let resolved = async {
                        if target.len() != 2 {
                            return Err("Validation target requires alias.Class".to_owned());
                        }
                        let Target::Classifier(receiver) = self.path(target, false).await? else {
                            return Err("Validation target must identify a model class".to_owned());
                        };
                        let Target::Symbol(symbol) = self.path(implementation, false).await? else {
                            return Err(
                                "Validation implementation requires one unambiguous source function"
                                    .to_owned(),
                            );
                        };
                        let module = self.index.module(&symbol.source)?;
                        if !module.symbols.iter().any(|candidate| {
                            candidate.id == symbol
                                && candidate.kind == super::modules::SymbolKind::Function
                        }) {
                            return Err(
                                "Validation implementation must identify a source function"
                                    .to_owned(),
                            );
                        }
                        Ok(BoundScopeBinding {
                            member: receiver.clone(),
                            receiver,
                            implementation: symbol,
                        })
                    }
                    .await;
                    match resolved {
                        Ok(binding) => {
                            self.output.validate_bindings.insert(ordinal, binding);
                        }
                        Err(message) => self.error(&item.span, message),
                    }
                }
                Declaration::Initialize {
                    target, members, ..
                } => {
                    if target.len() != 2 {
                        self.error(&item.span, "Initializer target requires alias.Class");
                        continue;
                    }
                    match self.path(target, false).await {
                        Ok(Target::Classifier(_)) => {}
                        Ok(_) => {
                            self.error(&item.span, "Initializer target must identify a model class")
                        }
                        Err(message) => self.error(&item.span, message),
                    }
                    for (name, _) in members {
                        pause().await;
                        let mut path = target.clone();
                        path.push(name.clone());
                        match self.contract_feature(&path).await {
                            Ok(Target::Feature(_)) => {}
                            Ok(_) => {
                                self.error(&name.span, "Initializer member must identify a feature")
                            }
                            Err(message) => self.error(&name.span, message),
                        }
                    }
                }
                Declaration::DerivedContract {
                    target, contract, ..
                } => {
                    self.contract_provider(ordinal, target, &item.span).await;
                    let dependency = match contract {
                        DerivedContract::Count(input) if target.len() == 3 => {
                            let mut path = target[..2].to_vec();
                            path.push(input.clone());
                            Some(path)
                        }
                        DerivedContract::NoIncoming(path) => Some(path.clone()),
                        _ => None,
                    };
                    if let Some(path) = dependency {
                        match self.contract_feature(&path).await {
                            Ok(Target::Feature(_)) => {}
                            Ok(_) => self.error(
                                &item.span,
                                "Derived contract dependency must identify a feature",
                            ),
                            Err(message) => self.error(&item.span, message),
                        }
                    }
                }
                Declaration::Projection { target, input, .. } => {
                    self.contract_provider(ordinal, target, &item.span).await;
                    if target.len() == 3 {
                        let mut path = target[..2].to_vec();
                        path.push(input.clone());
                        if let Err(message) = self.contract_feature(&path).await {
                            self.error(&input.span, message);
                        }
                    }
                }
                Declaration::Function {
                    external,
                    generics,
                    parameters,
                    result,
                    body,
                    ..
                } => {
                    for generic in generics {
                        pause().await;
                        self.local(&generic.name, LocalKind::Generic);
                    }
                    for generic in generics {
                        pause().await;
                        if let Some(ty) = generic.constraint {
                            work.push(Work::Type(ty));
                        }
                    }
                    for parameter in parameters {
                        pause().await;
                        work.push(Work::Type(parameter.ty));
                        self.local(&parameter.name, LocalKind::Parameter);
                    }
                    work.push(Work::Type(*result));
                    if !external {
                        work.push(Work::Expr(*body));
                    }
                }
                Declaration::Derive {
                    target,
                    receiver,
                    result,
                    body,
                    ..
                } => {
                    self.provider(ordinal, target, false, &item.span).await;
                    self.local(receiver, LocalKind::Receiver);
                    work.push(Work::Type(*result));
                    work.push(Work::Expr(*body));
                }
                Declaration::Validate {
                    target,
                    receiver,
                    requirements,
                    ..
                } => {
                    self.provider(ordinal, target, true, &item.span).await;
                    self.local(receiver, LocalKind::Receiver);
                    for requirement in requirements {
                        pause().await;
                        work.push(Work::Expr(requirement.value.condition));
                        work.push(Work::Expr(requirement.value.message));
                    }
                }
                _ => continue,
            }
            work.reverse();
            self.walk(work).await;
        }
    }
    // Provider contracts bind declaration identities without requesting a raw
    // generic value codec. Concrete domains are checked after materialization.
    async fn contract_feature(&self, path: &Path) -> Result<Target, String> {
        use crate::definitions::generic_types::Type;
        let (name, owner) = path.split_last().ok_or("Empty provider path")?;
        let Target::Classifier(declaration) = self.path(&owner.to_vec(), false).await? else {
            return Err("Provider member requires a model class".into());
        };
        let node = self.resources.node(&declaration)?;
        let raw = node
            .children
            .iter()
            .any(|child| child.name.namespace.is_none() && child.name.local == "eTypeParameters");
        let receiver = Type::Classifier {
            declaration,
            arguments: vec![],
            raw,
        };
        self.bridge
            .query(MetadataQuery::ContractFeature {
                receiver,
                name: name.value.clone(),
                span: node.span.clone(),
            })
            .await
    }
    async fn contract_provider(&mut self, ordinal: usize, target: &Path, span: &SourceSpan) {
        match self.contract_feature(target).await {
            Ok(value) => {
                self.output.providers.insert(ordinal, value);
            }
            Err(message) => self.error(span, message),
        }
    }
    async fn provider(
        &mut self,
        ordinal: usize,
        target: &Path,
        validation: bool,
        span: &SourceSpan,
    ) {
        if target.len() < 3 {
            self.error(
                span,
                "Provider target requires a model alias, class and feature/rule name",
            );
            return;
        }
        let path = if validation && target.len() >= 3 {
            target[..target.len() - 1].to_vec()
        } else {
            target.clone()
        };
        match self.path(&path, false).await {
            Ok(value)
                if matches!(
                    (&value, validation),
                    (Target::Classifier(_), true) | (Target::Feature(_), false)
                ) =>
            {
                self.output.providers.insert(ordinal, value);
            }
            Ok(_) => self.error(
                span,
                "Provider target must identify a model class/rule or feature",
            ),
            Err(message) => self.error(span, message),
        }
    }
    async fn walk(&mut self, mut work: Vec<Work>) {
        while let Some(next) = work.pop() {
            pause().await;
            match next {
                Work::Copy(id, child) => {
                    if let Some(target) = self.output.expressions.get(&child.0).cloned() {
                        self.output.expressions.insert(id.0, target);
                    }
                }
                Work::Enter => self.scopes.push(BTreeMap::new()),
                Work::Exit => {
                    self.scopes.pop();
                }
                Work::Bind(name, kind) => self.local(&name, kind),
                Work::PatternBindings(root) => {
                    let mut pending = vec![root];
                    while let Some(id) = pending.pop() {
                        pause().await;
                        match &self.module.document.patterns[id.0].value {
                            Pattern::Binding(name) => self.local(name, LocalKind::Pattern),
                            Pattern::Constructor { arguments, .. } => {
                                pending.extend(arguments.iter().rev().copied())
                            }
                            _ => {}
                        }
                    }
                }
                Work::Type(id) => {
                    let node = &self.module.document.types[id.0];
                    match &node.value {
                        Type::Inferred => {}
                        Type::Named { path, arguments } => {
                            match self.path(path, true).await {
                                Ok(target) => {
                                    self.output.types.insert(id.0, target);
                                }
                                Err(message) => self.error(&node.span, message),
                            }
                            work.extend(arguments.iter().rev().map(|id| Work::Type(*id)));
                        }
                        Type::Optional(id) | Type::Multiplicity { element: id, .. } => {
                            work.push(Work::Type(*id))
                        }
                        Type::Tuple(ids) => work.extend(ids.iter().rev().map(|id| Work::Type(*id))),
                        Type::Record(fields) => {
                            work.extend(fields.iter().rev().map(|(_, id)| Work::Type(*id)))
                        }
                        Type::Function { parameters, result } => {
                            work.push(Work::Type(*result));
                            work.extend(parameters.iter().rev().map(|id| Work::Type(*id)));
                        }
                    }
                }
                Work::Pattern(id) => {
                    let node = &self.module.document.patterns[id.0];
                    match &node.value {
                        // Pattern bindings become visible only after all constructor names resolve.
                        Pattern::Binding(_) => {}
                        Pattern::Constructor { path, arguments } => {
                            match self.path(path, false).await {
                                Ok(target) => {
                                    self.output.constructors.insert(id.0, target);
                                }
                                Err(message) => self.error(&node.span, message),
                            }
                            work.extend(arguments.iter().rev().map(|id| Work::Pattern(*id)));
                        }
                        _ => {}
                    }
                }
                Work::Member(id, receiver, name, safe) => {
                    if let Some(target) = self.output.expressions.get(&receiver.0) {
                        match self.member(target, &name).await {
                            Ok(Some(target)) => {
                                if safe {
                                    self.error(
                                        &name.span,
                                        "Safe navigation requires an instance receiver",
                                    );
                                }
                                self.output.expressions.insert(id.0, target);
                            }
                            Ok(None) => self.output.instance_members.push(id),
                            Err(message) => self.error(&name.span, message),
                        }
                    } else {
                        self.output.instance_members.push(id);
                    }
                }
                Work::Expr(id) => {
                    let node = &self.module.document.expressions[id.0];
                    let mut ordered = Vec::new();
                    match &node.value {
                        Expr::ComprehensionIntrinsic { flat } => {
                            self.output.expressions.insert(
                                id.0,
                                Target::Builtin {
                                    version: 2,
                                    name: if *flat { "flatMap" } else { "map" }.into(),
                                },
                            );
                        }
                        Expr::Name(name) => match self.name(name, false).await {
                            Ok(target) => {
                                self.output.expressions.insert(id.0, target);
                            }
                            Err(message) => self.error(&name.span, message),
                        },
                        Expr::Group(child) => {
                            ordered.push(Work::Expr(*child));
                            ordered.push(Work::Copy(id, *child));
                        }
                        Expr::Unary { operand, .. } => ordered.push(Work::Expr(*operand)),
                        Expr::Binary { left, right, .. } => {
                            ordered.push(Work::Expr(*left));
                            ordered.push(Work::Expr(*right));
                        }
                        Expr::Member {
                            receiver,
                            name,
                            safe,
                        } => {
                            ordered.push(Work::Expr(*receiver));
                            ordered.push(Work::Member(id, *receiver, name.clone(), *safe));
                        }
                        Expr::Call {
                            callee,
                            types,
                            arguments,
                        } => {
                            ordered.push(Work::Expr(*callee));
                            ordered.extend(types.iter().map(|id| Work::Type(*id)));
                            ordered.extend(arguments.iter().map(|id| Work::Expr(*id)));
                        }
                        Expr::Index { receiver, index } => {
                            ordered.push(Work::Expr(*receiver));
                            ordered.push(Work::Expr(*index));
                        }
                        Expr::List(ids) | Expr::Tuple(ids) => {
                            ordered.extend(ids.iter().map(|id| Work::Expr(*id)))
                        }
                        Expr::Record(fields) => {
                            ordered.extend(fields.iter().map(|(_, id)| Work::Expr(*id)))
                        }
                        Expr::New { class, fields } => {
                            match self.path(class, true).await {
                                Ok(target @ Target::Classifier(_)) => {
                                    self.output.expressions.insert(id.0, target);
                                }
                                Ok(_) => self.error(&node.span, "`new` requires a model class"),
                                Err(message) => self.error(&node.span, message),
                            }
                            ordered.extend(fields.iter().map(|(_, id)| Work::Expr(*id)))
                        }
                        Expr::If {
                            condition,
                            then_branch,
                            else_branch,
                        } => {
                            ordered.push(Work::Expr(*condition));
                            ordered.push(Work::Expr(*then_branch));
                            ordered.push(Work::Expr(*else_branch));
                        }
                        Expr::Block { bindings, result } => {
                            ordered.push(Work::Enter);
                            for binding in bindings {
                                pause().await;
                                if let Some(ty) = binding.value.ty {
                                    ordered.push(Work::Type(ty));
                                }
                                ordered.push(Work::Expr(binding.value.value));
                                ordered
                                    .push(Work::Bind(binding.value.name.clone(), LocalKind::Let));
                            }
                            ordered.push(Work::Expr(*result));
                            ordered.push(Work::Exit);
                        }
                        Expr::Lambda { parameters, body } => {
                            ordered.push(Work::Enter);
                            for parameter in parameters {
                                pause().await;
                                ordered.push(Work::Type(parameter.ty));
                                ordered
                                    .push(Work::Bind(parameter.name.clone(), LocalKind::Parameter));
                            }
                            ordered.push(Work::Expr(*body));
                            ordered.push(Work::Exit);
                        }
                        Expr::Match { value, arms } => {
                            ordered.push(Work::Expr(*value));
                            for (pattern, expression) in arms {
                                pause().await;
                                ordered.push(Work::Enter);
                                ordered.push(Work::Pattern(*pattern));
                                ordered.push(Work::PatternBindings(*pattern));
                                ordered.push(Work::Expr(*expression));
                                ordered.push(Work::Exit);
                            }
                        }
                        Expr::Literal(_) => {}
                        Expr::Error => {
                            self.error(&node.span, "Cannot bind recovered syntax errors")
                        }
                    }
                    work.extend(ordered.into_iter().rev());
                }
            }
        }
    }
}

#[cfg(test)]
mod preparation_tests {
    use super::*;
    use crate::definitions::bundle::{DefinitionSource, SourceKind};

    fn drive(
        bundle: &DefinitionBundle,
        entry: &SourceId,
        quantum: usize,
        drop_after: Option<usize>,
    ) -> Result<(Option<serde_json::Value>, usize), String> {
        let (index, resources) = index_modules_with_resources(bundle, entry, Default::default())
            .map_err(|errors| format!("{errors:?}"))?;
        let allocation = resources.document(entry).ok().map(|document| {
            (
                document.source.text.as_ptr(),
                document.root.children.as_ptr(),
            )
        });
        let bridge = MetadataBridge::default();
        let mut metadata = MetadataDriver::new(bundle, resources.clone(), Default::default());
        let future = bind_prepared_retained(index, resources, bridge.clone());
        fn require_send(_: &impl Send) {}
        require_send(&future);
        let mut future = Box::pin(future);
        let mut polls = 0;
        loop {
            for _ in 0..quantum {
                if drop_after == Some(polls) {
                    return Ok((None, polls));
                }
                polls += 1;
                if polls > 100_000 {
                    return Err("Binding traversal did not finish".into());
                }
                match future
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                {
                    Poll::Ready(result) => {
                        let result = match result {
                            Ok((bound, resources)) => {
                                if let Some(allocation) = allocation {
                                    let document = resources.document(entry)?;
                                    assert_eq!(
                                        (
                                            document.source.text.as_ptr(),
                                            document.root.children.as_ptr()
                                        ),
                                        allocation
                                    );
                                }
                                Ok(bound)
                            }
                            Err(errors) => Err(errors),
                        };
                        return Ok((
                            Some(serde_json::to_value(result).map_err(|error| error.to_string())?),
                            polls,
                        ));
                    }
                    Poll::Pending => metadata
                        .advance(
                            &bridge,
                            &crate::definitions::conversion::InvocationContext::default(),
                        )
                        .map_err(|error| error.to_string())?,
                }
            }
        }
    }

    #[test]
    #[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
    fn binding_frontiers_drop_recovery_and_resource_identity_are_preserved() -> Result<(), String> {
        let valid = "rcore 1;package p version \"1\";metamodel \"urn:binding\" prefix p {enum Mode {First = 0;Second = 1;}class Node {attr label:String [0,1];attr size:Int32 [1,1] derived;attr values:Int32 [0,*];}}behavior {import model \"urn:binding\" as m;derive m.Node.size using count(values);fn mode()->m.Mode=m.Mode.First;fn f(x:Int)->Int={let x=x+1;let callback=|y:Int| x+y;match Some(x) {Some(z)=>callback(z),none=>x}};fn id<T>(value:T)->T=value;derive m.Node.label(self)->String?=self.label;}";
        let invalid =
            "rcore 1;package p version \"1\";fn bad(x:Int,x:Int)->Unknown={let y=missing;y};";
        for (case, text) in [valid, invalid].into_iter().enumerate() {
            let source = DefinitionSource::new(
                "memory:binding-preparation.rcore",
                if case == 0 {
                    SourceKind::Rcore
                } else {
                    SourceKind::RcoreBehavior
                },
                text,
            );
            let entry = source.id.clone();
            let original = source.text.as_ptr();
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![source],
                namespaces: vec![],
            };
            let expected = bind(&bundle, &entry);
            if case == 0 {
                let bound = expected.as_ref().map_err(|errors| format!("{errors:?}"))?;
                assert!(bound.bindings.iter().any(|binding| {
                    binding
                        .expressions
                        .values()
                        .any(|target| matches!(target, Target::EnumLiteral { .. }))
                }));
                assert!(bound.bindings.iter().any(|binding| {
                    binding
                        .providers
                        .values()
                        .any(|target| matches!(target, Target::Feature(_)))
                }));
            } else {
                let errors = expected.as_ref().err().ok_or("Invalid binding accepted")?;
                for message in [
                    "Duplicate local binding x",
                    "Unresolved type name Unknown",
                    "Unresolved value name missing",
                ] {
                    assert!(
                        errors.iter().any(|error| error.message == message),
                        "{errors:?}"
                    );
                }
            }
            let expected = serde_json::to_value(expected).map_err(|error| error.to_string())?;
            let mut count = None;
            for quantum in [1, 3, 256] {
                let (observed, polls) = drive(&bundle, &entry, quantum, None)?;
                assert_eq!(observed.as_ref(), Some(&expected));
                assert!(polls > 10);
                if let Some(previous) = count {
                    assert_eq!(polls, previous);
                } else {
                    count = Some(polls);
                }
            }
            for prefix in 0..count.ok_or("Missing binding poll count")? {
                let (observed, polls) = drive(&bundle, &entry, 1, Some(prefix))?;
                assert!(observed.is_none());
                assert_eq!(polls, prefix);
                assert_eq!(bundle.sources[0].text.as_ptr(), original);
                assert_eq!(
                    serde_json::to_value(bind(&bundle, &entry))
                        .map_err(|error| error.to_string())?,
                    expected
                );
            }
        }
        Ok(())
    }
}
