//! Handwritten Rcore package facade over shared compilation, storage and execution.
//! Sources are authoritative. Reload rebuilds definitions and operation bindings.
pub use crate::rcore_behavior_functions::{
    Evaluation, ExecutionError, ExecutionErrorKind, ExecutionLimits,
};
use crate::{
    api::{BasicModel, ModelEdit},
    frozen::FrozenStore,
    model::Model,
    model_reference::ReferenceDomain,
    storage::{self, ModelRead, ModelStore},
};
pub use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceId, SourceKind},
    rcore_behavior::{
        SyntaxDiagnostic,
        ir::{Value as ScalarValue, ValueType as ScalarType},
        ir_v2::{DataWire, ValueWire},
        types::Scalar,
    },
};
use intersect_model::definitions::{
    conversion::{ConversionRegistry, ReferenceContext},
    generic_types::{RuntimeType, Type},
    rcore_behavior::{
        self,
        ir_v2::{EntrySelection, Limits},
        modules::SymbolKind,
    },
    rcore_operations::{OperationSelection, VerifiedOperation, compile_operation},
    schema::Schema,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Errors retain compilation locations and structured evaluator failure kinds.
#[derive(Debug)]
pub enum RcoreError {
    Compilation(Vec<SyntaxDiagnostic>),
    Model(String),
    MissingImplementation { class: String, operation: String },
    Execution(Box<ExecutionError>),
}
impl std::fmt::Display for RcoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Compilation(errors) => {
                for error in errors {
                    writeln!(
                        f,
                        "{}:{}: {}",
                        error.span.source.uri, error.span.start, error.message
                    )?;
                }
                Ok(())
            }
            Self::Model(message) => f.write_str(message),
            Self::MissingImplementation { class, operation } => {
                write!(f, "Missing Rcore implementation for {class}.{operation}")
            }
            Self::Execution(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for RcoreError {}
impl From<String> for RcoreError {
    fn from(value: String) -> Self {
        Self::Model(value)
    }
}
impl From<Vec<SyntaxDiagnostic>> for RcoreError {
    fn from(value: Vec<SyntaxDiagnostic>) -> Self {
        Self::Compilation(value)
    }
}
type Result<T> = std::result::Result<T, RcoreError>;

/// A model backed by the shared checked API. Local Rcore class names are
/// resolved once to exact runtime class identities; reads and edits are shared.
pub struct RcoreModel {
    inner: BasicModel<FrozenStore>,
    class_keys: BTreeMap<String, String>,
    initializer_classes: BTreeSet<String>,
}
impl std::ops::Deref for RcoreModel {
    type Target = BasicModel<FrozenStore>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl std::ops::DerefMut for RcoreModel {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
impl RcoreModel {
    /// Construct an explicitly selected runtime type, including generic applications.
    pub fn create_typed(
        &mut self,
        id: &str,
        ty: &RuntimeType,
        resource: &str,
    ) -> std::result::Result<(), String> {
        let key = self.inner.schema().class_key_for_type(ty)?.to_owned();
        self.require_default_creation(&key)?;
        self.inner.create(id, &key, resource)
    }
    pub fn create(
        &mut self,
        id: &str,
        class: &str,
        resource: &str,
    ) -> std::result::Result<(), String> {
        let key = self
            .class_keys
            .get(class)
            .ok_or_else(|| format!("Unknown local Rcore class {class}"))?;
        self.require_default_creation(key)?;
        self.inner.create(id, key, resource)
    }
    fn require_default_creation(&self, class: &str) -> std::result::Result<(), String> {
        if self.initializer_classes.contains(class) {
            Err(format!(
                "Class {class} requires RcorePackage::create_initialized"
            ))
        } else {
            Ok(())
        }
    }
}

/// A verified package, not a deserializable executable image. Native provider
/// registration and generic operation specialization are separate binding work.
#[derive(Clone, Debug)]
pub struct RcorePackage {
    bundle: DefinitionBundle,
    entry: SourceId,
    // One schema owner plus checked model rules; source authority stays in bundle.
    definition: intersect_model::definitions::schema::Package,
    namespace: String,
    types: Vec<RuntimeType>,
    operations: Vec<intersect_model::definitions::rcore::StructuralOperation>,
    implementations: BTreeMap<(String, String), VerifiedOperation>,
    getters: crate::source_getters::SourceGetters,
    bindings: Vec<Binding>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    class: String,
    operation: String,
    function: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedModel {
    version: u32,
    bundle: DefinitionBundle,
    entry: SourceId,
    namespace: String,
    types: Vec<RuntimeType>,
    bindings: Vec<Binding>,
    model: Model,
}

/// Compile one standalone source. Use `compile_rcore_at` for a stable authored URI.
pub fn compile_rcore(source: &str) -> Result<RcorePackage> {
    compile_rcore_at("memory:main.rcore", source)
}
pub fn compile_rcore_at(uri: &str, text: &str) -> Result<RcorePackage> {
    let source = DefinitionSource::new(uri, SourceKind::Rcore, text);
    let entry = source.id.clone();
    compile_rcore_bundle(
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        &entry,
    )
}
/// Dependencies are exact sources supplied by the host, never implicit I/O.
/// Structural sources use Rcore; standalone behavior modules use RcoreBehavior.
/// Only explicitly listed dependency edges are available to source imports.
pub fn compile_rcore_bundle(bundle: DefinitionBundle, entry: &SourceId) -> Result<RcorePackage> {
    compile_selected(bundle, entry, None)
}

/// Compile explicit closed/raw generic applications and a selected package root.
/// Selection is retained with saved models and checked again during reload.
pub fn compile_rcore_bundle_selected(
    bundle: DefinitionBundle,
    entry: &SourceId,
    namespace: &str,
    types: &[RuntimeType],
) -> Result<RcorePackage> {
    compile_selected(bundle, entry, Some((namespace, types)))
}

/// Compile selected-entry initializer declarations for explicitly named native
/// providers. This selects contracts only: each construction still needs the
/// exact `ClassInitializer`, and reload must explicitly select providers again.
/// Imported initializer declarations remain unsupported. No implementation is
/// stored in the package or supplied by serialized model data.
pub fn compile_rcore_bundle_with_initializers(
    bundle: DefinitionBundle,
    entry: &SourceId,
    providers: &[intersect_model::definitions::conversion::ProviderId],
) -> Result<RcorePackage> {
    compile_selected_with_initializers(bundle, entry, None, Some(providers))
}

/// Explicit runtime type selection with the same native initializer contract
/// selection as `compile_rcore_bundle_with_initializers`.
pub fn compile_rcore_bundle_selected_with_initializers(
    bundle: DefinitionBundle,
    entry: &SourceId,
    namespace: &str,
    types: &[RuntimeType],
    providers: &[intersect_model::definitions::conversion::ProviderId],
) -> Result<RcorePackage> {
    compile_selected_with_initializers(bundle, entry, Some((namespace, types)), Some(providers))
}

fn compile_selected(
    bundle: DefinitionBundle,
    entry: &SourceId,
    selection: Option<(&str, &[RuntimeType])>,
) -> Result<RcorePackage> {
    compile_selected_with_initializers(bundle, entry, selection, None)
}

fn compile_selected_with_initializers(
    bundle: DefinitionBundle,
    entry: &SourceId,
    selection: Option<(&str, &[RuntimeType])>,
    providers: Option<&[intersect_model::definitions::conversion::ProviderId]>,
) -> Result<RcorePackage> {
    let diagnostic = |message: String| {
        RcoreError::Compilation(vec![SyntaxDiagnostic {
            span: intersect_model::definitions::bundle::SourceSpan {
                source: entry.clone(),
                start: 0,
                end: 0,
            },
            message,
        }])
    };
    bundle.validate().map_err(diagnostic)?;
    if bundle
        .sources
        .iter()
        .any(|s| !matches!(s.kind, SourceKind::Rcore | SourceKind::RcoreBehavior))
    {
        return Err(diagnostic(
            "Rcore-only compilation accepts Rcore structure and behavior sources".into(),
        ));
    }
    if bundle.sources.len() > 128 || bundle.sources.iter().any(|s| s.text.len() > 256_000) {
        return Err(diagnostic(
            "Rcore compilation source budget exceeded".into(),
        ));
    }
    let (namespace, operations, types) = if let Some((namespace, types)) = selection {
        let (_, operations) =
            intersect_model::definitions::rcore::resolve_structure(&bundle, entry)?;
        (namespace.to_owned(), operations, types.to_vec())
    } else {
        let structure = intersect_model::definitions::rcore::compile_structure(&bundle, entry)?;
        let types = structure
            .schema
            .declarations
            .iter()
            .filter(|(key, _)| structure.schema.classes.contains_key(*key))
            .map(|(_, d)| {
                RuntimeType::new(Type::Classifier {
                    declaration: intersect_model::definitions::resources::DeclarationId {
                        source: d.source.clone(),
                        path: d.path.clone(),
                    },
                    arguments: vec![],
                    raw: false,
                })
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        (structure.namespace, structure.operations, types)
    };
    rcore_behavior::checking::check(&bundle, entry)?;
    let modules = rcore_behavior::modules::index_modules(&bundle, entry)?;
    let mut has_rules = false;
    for module in &modules.modules {
        for declaration in &module.document.declarations {
            match &declaration.value {
                rcore_behavior::ast::Declaration::Derive { .. }
                | rcore_behavior::ast::Declaration::Validate { .. } => {
                    // This first consumer activates only the selected authored
                    // entry, never silently consumes rules from another module.
                    if module.document.source.id != *entry {
                        return Err(RcoreError::Compilation(vec![SyntaxDiagnostic {
                            span: declaration.span.clone(),
                            message: "Imported model-rule activation is unsupported by the Rcore package API".into(),
                        }]));
                    }
                    has_rules = true;
                }
                rcore_behavior::ast::Declaration::Initialize {
                    provider, version, ..
                } => {
                    if module.document.source.id != *entry
                        || ((provider.value != "intersect.constant-attributes"
                            || version.value != "1")
                            && !providers.is_some_and(|providers| {
                                providers.iter().any(|selected| {
                                    selected.name == provider.value
                                        && selected.version == version.value
                                })
                            }))
                    {
                        return Err(RcoreError::Compilation(vec![SyntaxDiagnostic {
                            span: declaration.span.clone(),
                            message: "Direct Rcore initialization supports only selected-entry intersect.constant-attributes@1; imported or external provider activation requires an explicit adapter".into(),
                        }]));
                    }
                    has_rules = true;
                }
                rcore_behavior::ast::Declaration::Projection { .. }
                | rcore_behavior::ast::Declaration::RuleConversion(_)
                | rcore_behavior::ast::Declaration::DerivedContract { .. } => {
                    return Err(RcoreError::Compilation(vec![SyntaxDiagnostic {
                        span: declaration.span.clone(),
                        message: "Derived/validation and provider contract adapters are unsupported by the Rcore package API; use the language package compiler for provider contracts; explicit operation bindings are supported".into(),
                    }]));
                }
                _ => {}
            }
        }
    }
    // Use the runtime materializer also used by invocation, preserving one schema
    // identity across direct model edits, reference admission and execution.
    let mut schema = intersect_model::definitions::compiler::materialize_runtime_bundle(
        &bundle, entry, &namespace, &types, None,
    )
    .map_err(|e| {
        RcoreError::Compilation(vec![SyntaxDiagnostic {
            span: e.span,
            message: e.message,
        }])
    })?;
    let unsupported_external = || {
        RcoreError::Compilation(vec![SyntaxDiagnostic {
            // The lowered External expression retains no declaration span.
            // Report an explicit package-level rejection, never guess that an
            // earlier builtin declaration owns this unsupported implementation.
            span: intersect_model::definitions::bundle::SourceSpan {
                source: entry.clone(),
                start: 0,
                end: 1,
            },
            message: "Derived/validation and provider contract adapters are unsupported by the Rcore package API for this rule; External model-rule activation requires an explicit provider adapter; declaration localization is unavailable".into(),
        }])
    };
    let semantics = if has_rules {
        let source = &modules.module(entry).map_err(diagnostic)?.document.source;
        let semantics = intersect_model::definitions::compiler::compile_semantics_source(
            &mut schema,
            source,
            intersect_model::definitions::ecore::ParseLimits {
                source_bytes: 256_000,
            },
            false,
        )
        .map_err(|errors| {
            RcoreError::Compilation(
                errors
                    .into_iter()
                    .map(|error| SyntaxDiagnostic {
                        // The legacy lowerer uses a DTO source label. This
                        // invocation consumes only the exact selected entry;
                        // preserve every supplied offset and original detail.
                        span: intersect_model::definitions::bundle::SourceSpan {
                            source: entry.clone(),
                            start: error.start,
                            end: error.end,
                        },
                        message: format!(
                            "Derived/validation and provider contract adapters are unsupported by the Rcore package API for this rule; detail: {}",
                            error.message
                        ),
                    })
                    .collect(),
            )
        })?;
        if semantics.derived.iter().any(|rule| {
            matches!(
                &rule.expression,
                intersect_model::definitions::schema::Expression::External { .. }
            )
        }) {
            // Registry-backed source activation must use the existing F10-F12
            // adapter, not an implicit local-function fallback or second engine.
            return Err(unsupported_external());
        }
        semantics
    } else {
        // Preserve structural-only packages that declare an unimplemented
        // derived field. Explicit rule evaluation checks coverage before use.
        intersect_model::definitions::schema::Semantics {
            scope_bindings: vec![],
            rule_conversions: vec![],
            version: 1,
            scope: "unique_names".into(),
            derived: vec![],
            initializers: vec![],
            validations: vec![],
        }
    };
    let definition = intersect_model::definitions::schema::Package {
        version: crate::definitions::schema::MODEL_PACKAGE_VERSION,
        schema,
        semantics,
        // This private runtime context owns no second source/schema copy.
        // RcorePackage.bundle remains the exact source/reload authority.
        sources: BTreeMap::new(),
        compilation: None,
    };
    let getters = crate::source_getters::SourceGetters::compile(
        &definition,
        &bundle,
        &Limits::default(),
        intersect_model::definitions::ecore::ParseLimits {
            source_bytes: 256_000,
        },
    )?;
    Ok(RcorePackage {
        bundle,
        entry: entry.clone(),
        definition,
        namespace,
        types,
        operations,
        implementations: BTreeMap::new(),
        getters,
        bindings: vec![],
    })
}
/// Read-only model-rule output, not serialized executable authority.
#[derive(Clone, Serialize)]
pub struct ModelRuleEvaluation {
    pub derived: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
    pub diagnostics: Vec<intersect_model::definitions::schema::Diagnostic>,
}

impl RcorePackage {
    /// Evaluate installed local builtin/source derivations and model validations.
    /// Findings are returned as diagnostics; structural edits/save retain their
    /// existing policy. Derived navigation, registry implementations,
    /// imported rules and automatic dependency/invalidation services stay unsupported.
    /// The shared legacy full-model walk is not an S09 cooperative guarantee.
    pub fn evaluate_model_rules(
        &self,
        model: &BasicModel<impl ModelStore>,
    ) -> Result<ModelRuleEvaluation> {
        self.check_model(model)?;
        intersect_model::definitions::compiler::check_semantics(
            &self.definition.semantics,
            &self.definition.schema,
        )
        .map_err(RcoreError::Model)?;
        let mut output = ModelRuleEvaluation {
            derived: BTreeMap::new(),
            diagnostics: vec![],
        };
        crate::model_rules::apply_with_provider(
            &self.definition,
            model,
            &mut output.derived,
            &mut output.diagnostics,
            &crate::source_getters::SourceDerived {
                package: &self.definition,
                bundle: &self.bundle,
                getters: &self.getters,
                limits: &ExecutionLimits::default(),
            },
        )?;
        Ok(output)
    }

    /// Read an installed derived feature on the caller's original work ledger.
    /// Source entries use the same borrowed evaluator as source reference scopes.
    /// This observes the receiver and its dependencies; it does not validate the
    /// whole model or certify construction/language validity.
    pub fn read_derived(
        &self,
        model: &BasicModel<impl ModelStore>,
        id: &str,
        feature: &str,
        budget: &mut crate::core::query::QueryBudget,
        limits: &ExecutionLimits,
    ) -> Result<serde_json::Value> {
        budget.charge()?;
        if model.schema().revision != self.definition.schema.revision {
            return Err("Model belongs to a different compiled Rcore package"
                .to_owned()
                .into());
        }
        use crate::query::DerivedValues;
        crate::source_getters::SourceDerived {
            package: &self.definition,
            bundle: &self.bundle,
            getters: &self.getters,
            limits,
        }
        .read(model, id, feature, budget)
        .map_err(RcoreError::Model)
    }
    pub fn schema(&self) -> &Schema {
        &self.definition.schema
    }
    pub fn sources(&self) -> &DefinitionBundle {
        &self.bundle
    }
    /// Borrow checked model metadata for structural preparation and semantic
    /// reads. This installs no additional providers or text capability.
    pub fn prepared_definition(
        &self,
    ) -> crate::definitions::preparation::PreparedModelDefinition<'_> {
        crate::definitions::preparation::PreparedModelDefinition::from_rcore_parts(
            crate::definitions::model_definition::StructureSelection {
                bundle: &self.bundle,
                entry: &self.entry,
                namespace: &self.namespace,
                materialized_types: &self.types,
            },
            &self.definition.schema,
            &self.definition.semantics,
        )
    }

    pub fn entry(&self) -> &SourceId {
        &self.entry
    }

    /// Bind a declared operation to a named, nongeneric function in this source's
    /// behavior section. Signature and body checks precede installation; failure
    /// leaves the existing package unchanged. Duplicate bindings are rejected.
    pub fn bind_operation(&mut self, class: &str, operation: &str, function: &str) -> Result<()> {
        let (class_key, declaration) = self
            .definition
            .schema
            .declarations
            .iter()
            .find(|(key, d)| {
                d.source == self.entry
                    && d.name == class
                    && self.definition.schema.classes.contains_key(*key)
            })
            .ok_or_else(|| format!("Unknown Rcore class {class}"))?;
        let key = (class_key.clone(), operation.to_owned());
        if self.implementations.contains_key(&key) {
            return Err("Duplicate operation implementation".to_owned().into());
        }
        let selected = self
            .operations
            .iter()
            .find(|op| {
                op.name == operation
                    && op.owner.source == declaration.source
                    && op.owner.path == declaration.path
            })
            .ok_or_else(|| format!("Unknown declared operation {class}.{operation}"))?;
        let receiver = RuntimeType::new(Type::Classifier {
            declaration: selected.owner.clone(),
            arguments: vec![],
            raw: false,
        })?;
        let modules = rcore_behavior::modules::index_modules(&self.bundle, &self.entry)?;
        let module = modules.module(&self.entry)?;
        let functions: Vec<_> = module
            .symbols
            .iter()
            .filter(|s| s.name == [function] && s.kind == SymbolKind::Function)
            .collect();
        let symbol = match functions.as_slice() {
            [symbol] => *symbol,
            [] => return Err(format!("Missing behavior function {function}").into()),
            _ => return Err(format!(
                "Ambiguous behavior function {function}; explicit overload binding is unsupported"
            )
            .into()),
        };
        let selection = OperationSelection {
            receiver,
            declaration: selected.declaration.clone(),
            materialized_types: self.types.clone(),
            implementation: EntrySelection {
                symbol: symbol.id.clone(),
                types: vec![],
                arguments: vec![],
            },
        };
        let verified =
            compile_operation(&self.bundle, &self.entry, &selection, &Limits::default())?;
        self.implementations.insert(key, verified);
        self.bindings.push(Binding {
            class: class.into(),
            operation: operation.into(),
            function: function.into(),
        });
        Ok(())
    }
    fn wrap_model(&self, inner: BasicModel<FrozenStore>) -> RcoreModel {
        let mut counts = BTreeMap::<&str, usize>::new();
        for d in self
            .definition
            .schema
            .declarations
            .values()
            .filter(|d| d.source == self.entry)
        {
            *counts.entry(&d.name).or_default() += 1;
        }
        let class_keys = self
            .definition
            .schema
            .declarations
            .iter()
            .filter(|(key, d)| {
                d.source == self.entry
                    && self.definition.schema.classes.contains_key(*key)
                    && counts.get(d.name.as_str()) == Some(&1)
            })
            .map(|(key, d)| (d.name.clone(), key.clone()))
            .collect();
        let initializer_classes = self
            .definition
            .semantics
            .initializers
            .iter()
            .map(|rule| rule.class.clone())
            .collect();
        RcoreModel {
            inner,
            class_keys,
            initializer_classes,
        }
    }
    pub fn new_model(&self) -> Result<RcoreModel> {
        Ok(self.wrap_model(BasicModel::new(self.definition.schema.clone())?))
    }
    /// Create with the exact selected constant initializer, then authored values,
    /// through the shared atomic ModelEdit service. The caller supplies its work
    /// budget; no host provider or alternate source context is inferred.
    pub fn create_initialized(
        &self,
        model: &mut RcoreModel,
        class: &str,
        request: storage::Construction<'_>,
        budget: &mut crate::query::QueryBudget,
    ) -> Result<intersect_model::definitions::initialization::InitializationReceipt> {
        if model.inner.schema() != &self.definition.schema {
            return Err("Model belongs to a different compiled Rcore package"
                .to_owned()
                .into());
        }
        let key = model
            .class_keys
            .get(class)
            .ok_or_else(|| format!("Unknown local Rcore class {class}"))?;
        let initializers = crate::initialization::PackageInitializers::with_source_context(
            &self.definition,
            &self.bundle,
        )?;
        initializers
            .create_installed_for_edit(&mut model.inner, key, request, budget)
            .map_err(RcoreError::Model)
    }
    /// Resolve the source-compiled contract for one exact selected runtime type.
    /// A matching descriptor does not install or certify native code. Providers
    /// remain language/host-owned and are supplied per construction.
    pub fn initializer_contract(
        &self,
        ty: &RuntimeType,
    ) -> Result<intersect_model::definitions::initialization::ClassInitializationContract> {
        let key = self.definition.schema.class_key_for_type(ty)?;
        let initializers = crate::initialization::PackageInitializers::with_source_context(
            &self.definition,
            &self.bundle,
        )?;
        Ok(initializers.contract(key)?.clone())
    }

    /// Construct one exact runtime type through the shared atomic initializer.
    /// The provider sees the current read-only graph, and its entire plan passes
    /// the existing member, target, readonly, receipt and rollback checks. The
    /// caller's original budget is passed unchanged; no allowance is created.
    /// Source-context preparation and existing synchronous validation retain
    /// their current accounting limits; this is not whole-operation S09 proof.
    pub fn create_typed_initialized(
        &self,
        model: &mut RcoreModel,
        ty: &RuntimeType,
        request: storage::Construction<'_>,
        provider: &dyn storage::ClassInitializer,
        conversions: Option<&ConversionRegistry<'_>>,
        budget: &mut crate::query::QueryBudget,
    ) -> Result<intersect_model::definitions::initialization::InitializationReceipt> {
        if model.inner.schema() != &self.definition.schema {
            return Err("Model belongs to a different compiled Rcore package"
                .to_owned()
                .into());
        }
        let key = self.definition.schema.class_key_for_type(ty)?;
        let initializers = crate::initialization::PackageInitializers::with_source_context(
            &self.definition,
            &self.bundle,
        )?;
        initializers
            .create(
                &mut model.inner,
                key,
                request,
                Some(provider),
                conversions,
                budget,
            )
            .map_err(RcoreError::Model)
    }

    fn check_model(&self, model: &BasicModel<impl ModelStore>) -> Result<()> {
        if model.schema() != &self.definition.schema {
            return Err("Model belongs to a different compiled Rcore package"
                .to_owned()
                .into());
        }
        model.validate()?;
        if !self.definition.semantics.initializers.is_empty() {
            let initializers = crate::initialization::PackageInitializers::with_source_context(
                &self.definition,
                &self.bundle,
            )?;
            // The raw BasicModel facade remains structural. Crossing this
            // package boundary also requires the exact declared constructor
            // receipt; serialized receipts never authorize another provider.
            storage::visit_views(model, |element| {
                if self
                    .definition
                    .semantics
                    .initializers
                    .iter()
                    .any(|rule| rule.class == element.class())
                    && element.initialization().map(|receipt| &receipt.contract)
                        != Some(initializers.contract(element.class())?)
                {
                    return Err(format!(
                        "Missing or stale Rcore initialization receipt for {}",
                        element.id()
                    ));
                }
                Ok(())
            })?;
        }
        Ok(())
    }
    /// Read-only operation invocation through the shared checked evaluator.
    /// The receiver is supplied as an object identity; remaining data arguments
    /// receive their declared type IDs and are checked by the shared runtime.
    /// Nested values retain their explicit type IDs in the existing wire contract.
    pub fn invoke(
        &self,
        model: &BasicModel<impl ModelStore>,
        receiver: &str,
        operation: &str,
        arguments: &[DataWire],
        limits: &ExecutionLimits,
    ) -> Result<Evaluation> {
        self.check_model(model)?;
        let class = model
            .class(receiver)?
            .ok_or_else(|| format!("Unknown receiver {receiver}"))?;
        let verified = self
            .implementations
            .get(&(class.clone(), operation.into()))
            .ok_or_else(|| RcoreError::MissingImplementation {
                class,
                operation: operation.into(),
            })?;
        let program = verified.executable().program();
        let function = program
            .functions
            .get(program.entry.0 as usize)
            .ok_or_else(|| "Missing verified entry function".to_owned())?;
        if function.parameters.len() != arguments.len() + 1 {
            return Err("Operation argument count differs from its declaration"
                .to_owned()
                .into());
        }
        let reference = ReferenceContext {
            bundle: self.bundle.clone(),
            entry: self.entry.clone(),
            namespace: self.namespace.clone(),
            conversions: vec![],
            materialized_types: self.types.clone(),
            applied_conversions: vec![],
            constructors: vec![],
            model: serde_json::to_value(storage::export(model)?).map_err(|e| e.to_string())?,
        };
        let conversions = ConversionRegistry::default();
        let domain = ReferenceDomain::new_runtime(
            verified.binding().receiver.clone(),
            reference.clone(),
            &conversions,
        )
        .map_err(|e| e.to_string())?;
        let receiver_type = *function
            .parameters
            .first()
            .ok_or_else(|| "Missing receiver type".to_owned())?;
        let mut values = vec![ValueWire {
            ty: receiver_type,
            data: DataWire::Model(domain.bind(receiver).map_err(|e| e.to_string())?),
        }];
        values.extend(
            arguments
                .iter()
                .zip(function.parameters.iter().skip(1))
                .map(|(data, ty)| ValueWire {
                    ty: *ty,
                    data: data.clone(),
                }),
        );
        let references = [reference];
        let context = crate::rcore_behavior_functions::ExecutionContext {
            bundle: &self.bundle,
            conversions: None,
            references: &references,
        };
        crate::rcore_operations::invoke(verified, &values, &context, limits)
            .map_err(|e| RcoreError::Execution(Box::new(e)))
    }
    /// Persist exact sources, explicit bindings and model data, never executable
    /// authority. Host code owns writing these bytes to storage.
    pub fn save(&self, model: &BasicModel<impl ModelStore>) -> Result<Vec<u8>> {
        self.check_model(model)?;
        let saved = SavedModel {
            version: 2,
            bundle: self.bundle.clone(),
            entry: self.entry.clone(),
            namespace: self.namespace.clone(),
            types: self.types.clone(),
            bindings: self.bindings.clone(),
            model: storage::export(model)?,
        };
        serde_json::to_vec(&saved).map_err(|e| RcoreError::Model(e.to_string()))
    }
    /// Recompile exact retained sources and binding contracts, then validate data.
    /// No legacy package/schema format is accepted. Hosts bound input byte sizes.
    pub fn reload(bytes: &[u8]) -> Result<(Self, RcoreModel)> {
        Self::reload_selected_initializers(bytes, None)
    }

    /// Recompile exact saved sources with explicit native initializer contract
    /// selection. Saved data never reconstitutes a provider; subsequent creation
    /// must receive a newly supplied exact provider through the shared service.
    pub fn reload_with_initializers(
        bytes: &[u8],
        providers: &[intersect_model::definitions::conversion::ProviderId],
    ) -> Result<(Self, RcoreModel)> {
        Self::reload_selected_initializers(bytes, Some(providers))
    }

    fn reload_selected_initializers(
        bytes: &[u8],
        providers: Option<&[intersect_model::definitions::conversion::ProviderId]>,
    ) -> Result<(Self, RcoreModel)> {
        let saved: SavedModel = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if saved.version != 2 {
            return Err("Unsupported Rcore saved-model version".to_owned().into());
        }
        let mut package = compile_selected_with_initializers(
            saved.bundle,
            &saved.entry,
            Some((&saved.namespace, &saved.types)),
            providers,
        )?;
        for binding in saved.bindings {
            package.bind_operation(&binding.class, &binding.operation, &binding.function)?;
        }
        let store = FrozenStore::from_materialized(package.schema(), &saved.model)?;
        let model = BasicModel::from_store(package.definition.schema.clone(), store)?;
        package.check_model(&model)?;
        let model = package.wrap_model(model);
        Ok((package, model))
    }
}
