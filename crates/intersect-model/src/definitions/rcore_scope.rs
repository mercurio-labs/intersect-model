//! Source-authored reference scope contracts over the shared verified function IR.
//! A retained selector is data; only fresh source compilation creates a scope image.
pub use crate::definitions::rcore_behavior::function_lowering::SchemaStores;
pub use crate::definitions::rcore_behavior::model_types::RuntimeSchemaStore;
use crate::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceId, SourceKind, SourceSpan},
    generic_types::{RuntimeType, Type as ModelType},
    rcore_behavior::{
        SyntaxDiagnostic, bindings,
        ir_v2::{
            Effect, EntrySelection, Limits, SavedContext, SavedExecutable, TypeId, TypeNode,
            VerifiedExecutable,
        },
        modules::SymbolId,
        types::{CollectionKind, Scalar},
    },
    resources::DeclarationId,
    schema::MemberIdentity,
};
use serde::{Deserialize, Serialize};

/// Exact declaration ordinal in an authored Rcore source, retained for replay.
/// Deserialization does not compile or install its implementation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeBindingDescriptor {
    pub source: SourceId,
    pub declaration: usize,
}

/// Reconstruct selector data from one exact authored entry. This does not bind,
/// type-check, compile or activate any scope implementation.
pub fn source_scope_bindings(
    source: &DefinitionSource,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<Vec<ScopeBindingDescriptor>, Vec<SyntaxDiagnostic>> {
    let span = SourceSpan {
        source: source.id.clone(),
        start: 0,
        end: 0,
    };
    if !matches!(source.kind, SourceKind::Rcore | SourceKind::RcoreBehavior) {
        return Err(error(
            &span,
            "Scope selectors require an authored Rcore source",
        ));
    }
    if source.text.len() > limits.source_bytes {
        return Err(error(
            &span,
            "Scope selector source exceeds the explicit source ceiling",
        ));
    }
    source.validate().map_err(|message| error(&span, message))?;
    let document = crate::definitions::rcore_behavior::parse_with_limits(source.clone(), limits);
    if !document.diagnostics.is_empty() {
        return Err(document.diagnostics);
    }
    Ok(document
        .declarations
        .iter()
        .enumerate()
        .filter(|(_, node)| {
            matches!(
                &node.value,
                crate::definitions::rcore_behavior::ast::Declaration::ScopeBinding { .. }
            )
        })
        .map(|(declaration, _)| ScopeBindingDescriptor {
            source: source.id.clone(),
            declaration,
        })
        .collect())
}

/// Selector data for every `bind default` of one authored source, in order.
pub fn source_default_bindings(
    source: &DefinitionSource,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<Vec<ScopeBindingDescriptor>, Vec<SyntaxDiagnostic>> {
    let span = SourceSpan {
        source: source.id.clone(),
        start: 0,
        end: 0,
    };
    if !matches!(source.kind, SourceKind::Rcore | SourceKind::RcoreBehavior)
        || source.text.len() > limits.source_bytes
    {
        return Err(error(
            &span,
            "Default selectors require an authored Rcore source within the source ceiling",
        ));
    }
    source.validate().map_err(|message| error(&span, message))?;
    let document = crate::definitions::rcore_behavior::parse_with_limits(source.clone(), limits);
    if !document.diagnostics.is_empty() {
        return Err(document.diagnostics);
    }
    Ok(document
        .declarations
        .iter()
        .enumerate()
        .filter(|(_, node)| {
            matches!(
                &node.value,
                crate::definitions::rcore_behavior::ast::Declaration::DefaultBinding { .. }
            )
        })
        .map(|(declaration, _)| ScopeBindingDescriptor {
            source: source.id.clone(),
            declaration,
        })
        .collect())
}

/// Selector data for every `bind implicit` of one authored source, in order.
pub fn source_implicit_bindings(
    source: &DefinitionSource,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<Vec<ScopeBindingDescriptor>, Vec<SyntaxDiagnostic>> {
    let span = SourceSpan {
        source: source.id.clone(),
        start: 0,
        end: 0,
    };
    if !matches!(source.kind, SourceKind::Rcore | SourceKind::RcoreBehavior)
        || source.text.len() > limits.source_bytes
    {
        return Err(error(
            &span,
            "Implicit selectors require an authored Rcore source within the source ceiling",
        ));
    }
    source.validate().map_err(|message| error(&span, message))?;
    let document = crate::definitions::rcore_behavior::parse_with_limits(source.clone(), limits);
    if !document.diagnostics.is_empty() {
        return Err(document.diagnostics);
    }
    Ok(document
        .declarations
        .iter()
        .enumerate()
        .filter(|(_, node)| {
            matches!(
                &node.value,
                crate::definitions::rcore_behavior::ast::Declaration::ImplicitBinding { .. }
            )
        })
        .map(|(declaration, _)| ScopeBindingDescriptor {
            source: source.id.clone(),
            declaration,
        })
        .collect())
}

/// Selector data for every `bind validate` of one authored source, in order.
pub fn source_validate_bindings(
    source: &DefinitionSource,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<Vec<ScopeBindingDescriptor>, Vec<SyntaxDiagnostic>> {
    let span = SourceSpan {
        source: source.id.clone(),
        start: 0,
        end: 0,
    };
    if !matches!(source.kind, SourceKind::Rcore | SourceKind::RcoreBehavior)
        || source.text.len() > limits.source_bytes
    {
        return Err(error(
            &span,
            "Validation selectors require an authored Rcore source within the source ceiling",
        ));
    }
    source.validate().map_err(|message| error(&span, message))?;
    let document = crate::definitions::rcore_behavior::parse_with_limits(source.clone(), limits);
    if !document.diagnostics.is_empty() {
        return Err(document.diagnostics);
    }
    Ok(document
        .declarations
        .iter()
        .enumerate()
        .filter(|(_, node)| {
            matches!(
                &node.value,
                crate::definitions::rcore_behavior::ast::Declaration::ValidateBinding { .. }
            )
        })
        .map(|(declaration, _)| ScopeBindingDescriptor {
            source: source.id.clone(),
            declaration,
        })
        .collect())
}

/// The fields of the prelude `Issue` record that a `bind validate` function
/// returns, all `String`, in record (name) order.
pub const ISSUE_FIELDS: [&str; 5] = ["code", "feature", "message", "rule", "severity"];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeBinding {
    pub receiver: RuntimeType,
    /// The reference target; for an attribute default, the receiver.
    pub target: RuntimeType,
    /// For an attribute default, the value type (String, Bool, Int64 or an
    /// enumeration); `None` for references.
    pub attribute: Option<TypeNode>,
    pub member: MemberIdentity,
    pub declaration: DeclarationId,
    pub implementation: SymbolId,
    pub declaration_span: SourceSpan,
    pub implementation_span: SourceSpan,
}

/// A freshly compiled body with its exact reference/signature contract.
/// There is no deserialization or unchecked public constructor.
#[derive(Clone, Debug)]
pub struct VerifiedScope {
    binding: ScopeBinding,
    executable: VerifiedExecutable,
    kind: BindingKind,
}
/// Which selector a verified binding implements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingKind {
    /// `bind scope`: `(receiver, spelling) -> Sequence<target>`.
    Scope,
    /// `bind default`: `(receiver) -> Sequence<value>` for an unassigned slot.
    Default,
    /// `bind implicit`: `(receiver) -> Sequence<object>` built with `new`, added
    /// to a containment reference as implicit elements.
    Implicit,
    /// `bind validate`: `(receiver) -> Sequence<Issue>` for every object of the
    /// receiver class. The binding's `member` names the receiver class and its
    /// `target` is the receiver.
    Validate,
}
impl VerifiedScope {
    /// A `bind default` (receiver only) rather than a `bind scope`.
    pub fn is_default(&self) -> bool {
        self.kind == BindingKind::Default
    }
    /// A `bind implicit`, whose objects the engine adds to the model.
    pub fn is_implicit(&self) -> bool {
        self.kind == BindingKind::Implicit
    }
    /// A `bind validate`, whose issues the engine reports.
    pub fn is_validate(&self) -> bool {
        self.kind == BindingKind::Validate
    }
    pub fn kind(&self) -> BindingKind {
        self.kind
    }
    pub fn binding(&self) -> &ScopeBinding {
        &self.binding
    }
    pub fn executable(&self) -> &VerifiedExecutable {
        &self.executable
    }
    pub fn receiver(&self) -> &RuntimeType {
        &self.binding.receiver
    }
    pub fn target(&self) -> &RuntimeType {
        &self.binding.target
    }
    pub fn member(&self) -> &MemberIdentity {
        &self.binding.member
    }
    pub fn span(&self) -> &SourceSpan {
        &self.binding.declaration_span
    }
}

fn error(span: &SourceSpan, message: impl Into<String>) -> Vec<SyntaxDiagnostic> {
    vec![SyntaxDiagnostic {
        span: span.clone(),
        message: message.into(),
    }]
}

fn nominal(declaration: DeclarationId) -> Result<RuntimeType, String> {
    RuntimeType::new(ModelType::Classifier {
        declaration,
        arguments: vec![],
        raw: false,
    })
}

/// Compile a `bind scope` declaration from exact source through the ordinary
/// binder, checker, lowering and verifier. The first contract is deliberately
/// nongeneric: `(exact receiver model, String) -> Sequence<exact target model>`.
/// Pure/Query effects are inferred by the existing verified image.
pub fn compile_scope(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    declaration: usize,
    limits: &Limits,
) -> Result<VerifiedScope, Vec<SyntaxDiagnostic>> {
    compile_scope_with_source_limits(
        bundle,
        entry_source,
        declaration,
        limits,
        Default::default(),
    )
}

/// The same source contract compiler with an explicit per-source host ceiling.
/// Serialized package policies do not select this allowance automatically.
pub fn compile_scope_with_source_limits(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    declaration: usize,
    limits: &Limits,
    source_limits: crate::definitions::ecore::ParseLimits,
) -> Result<VerifiedScope, Vec<SyntaxDiagnostic>> {
    compile_binding(
        bundle,
        entry_source,
        declaration,
        limits,
        source_limits,
        BindingKind::Scope,
    )
}

/// Compile a `bind default` declaration through the same binder, checker,
/// lowering and verifier. Contract: `(exact receiver model) -> Sequence<exact
/// target model>`; an empty sequence leaves the reference empty.
pub fn compile_default_with_source_limits(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    declaration: usize,
    limits: &Limits,
    source_limits: crate::definitions::ecore::ParseLimits,
) -> Result<VerifiedScope, Vec<SyntaxDiagnostic>> {
    compile_binding(
        bundle,
        entry_source,
        declaration,
        limits,
        source_limits,
        BindingKind::Default,
    )
}

/// Compile a `bind validate` declaration through the same binder, checker,
/// lowering and verifier. Contract: `(exact receiver model) -> Sequence<Issue>`
/// with the prelude `Issue` record; the function stays read-only.
pub fn compile_validate_with_source_limits(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    declaration: usize,
    limits: &Limits,
    source_limits: crate::definitions::ecore::ParseLimits,
) -> Result<VerifiedScope, Vec<SyntaxDiagnostic>> {
    compile_binding(
        bundle,
        entry_source,
        declaration,
        limits,
        source_limits,
        BindingKind::Validate,
    )
}

/// Compile a `bind implicit` declaration through the same binder, checker,
/// lowering and verifier. Contract: `(exact receiver model) -> Sequence<exact
/// containment target model>`; the objects are built with `new`.
pub fn compile_implicit_with_source_limits(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    declaration: usize,
    limits: &Limits,
    source_limits: crate::definitions::ecore::ParseLimits,
) -> Result<VerifiedScope, Vec<SyntaxDiagnostic>> {
    compile_binding(
        bundle,
        entry_source,
        declaration,
        limits,
        source_limits,
        BindingKind::Implicit,
    )
}

fn compile_binding(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    declaration: usize,
    limits: &Limits,
    source_limits: crate::definitions::ecore::ParseLimits,
    kind: BindingKind,
) -> Result<VerifiedScope, Vec<SyntaxDiagnostic>> {
    let bound = bindings::bind_with_limits(bundle, entry_source, source_limits)?;
    let contract = contract(
        bundle,
        entry_source,
        &bound,
        declaration,
        source_limits,
        kind,
        None,
    )?;
    let executable =
        crate::definitions::rcore_behavior::function_lowering::compile_function_with_source_limits(
            bundle,
            entry_source,
            &contract.selection(),
            limits,
            source_limits,
        )?;
    finish(contract, executable)
}

/// Compile every listed `bind scope` (`false`) and `bind default` (`true`)
/// declaration of one source through the same contracts, with one source check
/// for lowering and one independent check for verification shared by all of
/// them. Results are in `declarations` order.
pub fn compile_bindings_with_source_limits(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    declarations: &[(usize, bool)],
    limits: &Limits,
    source_limits: crate::definitions::ecore::ParseLimits,
) -> Result<Vec<VerifiedScope>, Vec<SyntaxDiagnostic>> {
    let kinds: Vec<_> = declarations
        .iter()
        .map(|(declaration, default)| {
            let kind = if *default {
                BindingKind::Default
            } else {
                BindingKind::Scope
            };
            (*declaration, kind)
        })
        .collect();
    compile_binding_kinds_with_source_limits(bundle, entry_source, &kinds, limits, source_limits)
}

/// `compile_bindings_with_source_limits` for any mix of scope, default and
/// implicit selectors.
pub fn compile_binding_kinds_with_source_limits(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    declarations: &[(usize, BindingKind)],
    limits: &Limits,
    source_limits: crate::definitions::ecore::ParseLimits,
) -> Result<Vec<VerifiedScope>, Vec<SyntaxDiagnostic>> {
    compile_binding_kinds_with_progress(
        bundle,
        entry_source,
        declarations,
        limits,
        source_limits,
        &|_| {},
    )
}

/// `compile_binding_kinds_with_source_limits`, reporting phase markers to
/// `observe`: `binding_contracts_*` (per-declaration contract checks),
/// `binding_functions_*` (the shared lowering and verification of the
/// implementations) and `binding_signatures_*` (matching each verified entry to
/// its contract). The markers only observe; any clock belongs to the host.
pub fn compile_binding_kinds_with_progress(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    declarations: &[(usize, BindingKind)],
    limits: &Limits,
    source_limits: crate::definitions::ecore::ParseLimits,
    observe: &impl Fn(&'static str),
) -> Result<Vec<VerifiedScope>, Vec<SyntaxDiagnostic>> {
    compile_binding_kinds_with_schema_stores(
        bundle,
        entry_source,
        declarations,
        limits,
        source_limits,
        &SchemaStores::default(),
        observe,
    )
}

/// `compile_binding_kinds_with_progress` sharing runtime-schema preparations
/// through `stores`. A host compiling several binding jobs of one bundle and
/// source limits (a language-pack build) passes one `SchemaStores` to all of
/// them, from any number of threads, so each metamodel schema is prepared once
/// per side rather than once per job. Results are the same with any stores.
pub fn compile_binding_kinds_with_schema_stores(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    declarations: &[(usize, BindingKind)],
    limits: &Limits,
    source_limits: crate::definitions::ecore::ParseLimits,
    stores: &SchemaStores,
    observe: &impl Fn(&'static str),
) -> Result<Vec<VerifiedScope>, Vec<SyntaxDiagnostic>> {
    observe("binding_contracts_begin");
    // Every contract reads the same bound modules of this source: bind once.
    let bound = if declarations.is_empty() {
        None
    } else {
        Some(bindings::bind_with_limits(
            bundle,
            entry_source,
            source_limits,
        )?)
    };
    let contracts = match &bound {
        None => vec![],
        Some(bound) => declarations
            .iter()
            .map(|(declaration, kind)| {
                contract(
                    bundle,
                    entry_source,
                    bound,
                    *declaration,
                    source_limits,
                    *kind,
                    Some(&stores.lowering),
                )
            })
            .collect::<Result<Vec<_>, _>>()?,
    };
    observe("binding_contracts_end");
    observe("binding_functions_begin");
    let selections: Vec<_> = contracts.iter().map(Contract::selection).collect();
    let executables =
        crate::definitions::rcore_behavior::function_lowering::compile_functions_with_schema_stores(
            bundle,
            entry_source,
            &selections,
            limits,
            source_limits,
            stores,
        )?;
    observe("binding_functions_end");
    observe("binding_signatures_begin");
    let verified = contracts
        .into_iter()
        .zip(executables)
        .map(|(contract, executable)| finish(contract, executable))
        .collect();
    observe("binding_signatures_end");
    verified
}

/// A bound selector's checked contract, before its implementation compiles.
struct Contract {
    receiver: RuntimeType,
    target: RuntimeType,
    attribute: Option<TypeNode>,
    member: MemberIdentity,
    declaration: DeclarationId,
    implementation: SymbolId,
    declaration_span: SourceSpan,
    kind: BindingKind,
}
impl Contract {
    fn selection(&self) -> EntrySelection {
        EntrySelection {
            symbol: self.implementation.clone(),
            types: vec![],
            arguments: vec![],
        }
    }
}

fn contract(
    bundle: &DefinitionBundle,
    entry_source: &SourceId,
    bound: &bindings::BoundModules,
    declaration: usize,
    source_limits: crate::definitions::ecore::ParseLimits,
    kind: BindingKind,
    store: Option<&crate::definitions::rcore_behavior::model_types::RuntimeSchemaStore>,
) -> Result<Contract, Vec<SyntaxDiagnostic>> {
    let default = kind == BindingKind::Default;
    let fallback = SourceSpan {
        source: entry_source.clone(),
        start: 0,
        end: 0,
    };
    let module = bound
        .index
        .module(entry_source)
        .map_err(|message| error(&fallback, message))?;
    let selected = bound
        .bindings
        .iter()
        .find(|module| module.source == *entry_source)
        .and_then(|module| match kind {
            BindingKind::Scope => module.scope_bindings.get(&declaration),
            BindingKind::Default => module.default_bindings.get(&declaration),
            BindingKind::Implicit => module.implicit_bindings.get(&declaration),
            BindingKind::Validate => module.validate_bindings.get(&declaration),
        })
        .ok_or_else(|| {
            error(
                &fallback,
                match kind {
                    BindingKind::Scope => "Selected declaration is not a bound scope policy",
                    BindingKind::Default => "Selected declaration is not a bound default policy",
                    BindingKind::Implicit => "Selected declaration is not a bound implicit policy",
                    BindingKind::Validate => "Selected declaration is not a bound validation",
                },
            )
        })?;
    let declaration_span = module
        .document
        .declarations
        .get(declaration)
        .ok_or_else(|| error(&fallback, "Missing scope declaration"))?
        .span
        .clone();
    let receiver =
        nominal(selected.receiver.clone()).map_err(|message| error(&declaration_span, message))?;
    let resources =
        crate::definitions::resources::ResourceSet::new_with_limits(bundle, source_limits)
            .map_err(|failure| {
                vec![SyntaxDiagnostic {
                    span: failure.span,
                    message: failure.message,
                }]
            })?;
    // Use the same recursive member/operation target closure as ordinary Rcore
    // typing. The materializer's explicit requested set alone is not a closure.
    let mut models =
        crate::definitions::rcore_behavior::model_types::ModelTypes::with_source_resources(
            bundle,
            None,
            source_limits,
            resources,
        );
    models.attach_schema_store(store);
    let checked = models
        .runtime_schema(std::slice::from_ref(receiver.expression()))
        .map_err(|message| error(&declaration_span, message))?;
    let receiver_class = checked
        .class_key_for_type(&receiver)
        .map_err(|message| error(&declaration_span, message))?;
    let schema = checked.schema();
    if kind == BindingKind::Validate {
        // A validation names its receiver class only.
        return Ok(Contract {
            target: receiver.clone(),
            receiver,
            attribute: None,
            member: MemberIdentity::Source {
                declaration: selected.member.clone(),
            },
            declaration: selected.member.clone(),
            implementation: selected.implementation.clone(),
            declaration_span,
            kind,
        });
    }
    let member = MemberIdentity::Source {
        declaration: selected.member.clone(),
    };
    let feature = schema
        .feature_by_id(receiver_class, &member)
        .map_err(|message| error(&declaration_span, message))?;
    let attribute = if !feature.feature.reference && default {
        let node = if let Some(enumeration) = schema.enumerations.get(&feature.feature.target) {
            TypeNode::Enum(enumeration.declaration().clone())
        } else {
            TypeNode::Scalar(match feature.feature.target.as_str() {
                "EString" => Scalar::String,
                "EBoolean" => Scalar::Bool,
                "ELong" => Scalar::Int64,
                _ => {
                    return Err(error(
                        &declaration_span,
                        "Unsupported: attribute defaults support EString, EBoolean, ELong and enumerations",
                    ));
                }
            })
        };
        Some(node)
    } else {
        None
    };
    if kind == BindingKind::Implicit && (!feature.feature.reference || !feature.feature.containment)
    {
        return Err(error(
            &declaration_span,
            "Implicit binding requires a containment reference",
        ));
    }
    if kind != BindingKind::Implicit
        && attribute.is_none()
        && (!feature.feature.reference || feature.feature.containment)
    {
        return Err(error(
            &declaration_span,
            if default {
                "Default binding requires an attribute or a non-containment reference"
            } else {
                "Scope binding requires a non-containment reference feature"
            },
        ));
    }
    let target = if attribute.is_some() {
        receiver.clone()
    } else {
        let target_declaration = schema
            .declarations
            .get(&feature.feature.target)
            .ok_or_else(|| {
                error(
                    &declaration_span,
                    "Scope target requires an exact model class",
                )
            })?;
        let target = nominal(DeclarationId {
            source: target_declaration.source.clone(),
            path: target_declaration.path.clone(),
        })
        .map_err(|message| error(&declaration_span, message))?;
        if checked
            .class_key_for_type(&target)
            .map_err(|message| error(&declaration_span, message))?
            != feature.feature.target
        {
            return Err(error(
                &declaration_span,
                "Generic reference scope targets require explicit specialization; unsupported",
            ));
        }
        target
    };
    Ok(Contract {
        receiver,
        target,
        attribute,
        member,
        declaration: selected.member.clone(),
        implementation: selected.implementation.clone(),
        declaration_span,
        kind,
    })
}

/// Check a compiled implementation against its contract's exact ABI.
fn finish(
    contract: Contract,
    executable: VerifiedExecutable,
) -> Result<VerifiedScope, Vec<SyntaxDiagnostic>> {
    let Contract {
        receiver,
        target,
        attribute,
        member,
        declaration,
        implementation,
        declaration_span,
        kind,
    } = contract;
    let program = executable.program();
    let function = program
        .functions
        .get(program.entry.0 as usize)
        .ok_or_else(|| error(&declaration_span, "Missing verified scope entry function"))?;
    // Only an implicit binding may construct model objects; scopes and
    // defaults stay read-only.
    if kind != BindingKind::Implicit
        && executable.effects().get(program.entry.0 as usize) == Some(&Effect::Construct)
    {
        return Err(error(
            &function.span,
            "`new` is allowed only in functions an implicit binding reaches",
        ));
    }
    let receiver_first = function.parameters.first().is_some_and(|p| {
        program.types.get(p.0 as usize) == Some(&TypeNode::Model(receiver.clone()))
    });
    if kind == BindingKind::Validate {
        if function.parameters.len() != 1 || !receiver_first {
            return Err(error(
                &function.span,
                "Validation implementation requires exactly the receiver model parameter",
            ));
        }
        if !returns_issues(&program.types, function.result) {
            return Err(error(
                &function.span,
                "Validation implementation must return Sequence<Issue>",
            ));
        }
        let implementation_span = function.span.clone();
        return Ok(VerifiedScope {
            binding: ScopeBinding {
                receiver,
                target,
                attribute,
                member,
                declaration,
                implementation,
                declaration_span,
                implementation_span,
            },
            executable,
            kind,
        });
    }
    if kind != BindingKind::Scope {
        if function.parameters.len() != 1 || !receiver_first {
            return Err(error(
                &function.span,
                "Default implementation requires exactly the receiver model parameter",
            ));
        }
    } else if function.parameters.len() != 2
        || !receiver_first
        || program.types.get(function.parameters[1].0 as usize)
            != Some(&TypeNode::Scalar(Scalar::String))
    {
        return Err(error(
            &function.span,
            "Scope implementation requires the exact receiver model followed by String spelling",
        ));
    }
    let Some(TypeNode::Collection {
        kind: CollectionKind::Sequence,
        element,
    }) = program.types.get(function.result.0 as usize)
    else {
        return Err(error(
            &function.span,
            "Scope implementation must return Sequence of the exact reference target model",
        ));
    };
    let expected = attribute
        .clone()
        .unwrap_or_else(|| TypeNode::Model(target.clone()));
    if program.types.get(element.0 as usize) != Some(&expected) {
        return Err(error(
            &function.span,
            "Result element differs from the exact feature type",
        ));
    }
    let implementation_span = function.span.clone();
    Ok(VerifiedScope {
        binding: ScopeBinding {
            receiver,
            target,
            attribute,
            member,
            declaration,
            implementation,
            declaration_span,
            implementation_span,
        },
        executable,
        kind,
    })
}

/// Whether `result` is `Sequence<Issue>`: a sequence of the record with
/// exactly the `ISSUE_FIELDS`, each `String`.
fn returns_issues(types: &[TypeNode], result: TypeId) -> bool {
    let node = |id: TypeId| types.get(id.0 as usize);
    let Some(TypeNode::Collection {
        kind: CollectionKind::Sequence,
        element,
    }) = node(result)
    else {
        return false;
    };
    let Some(TypeNode::Record(fields)) = node(*element) else {
        return false;
    };
    fields.len() == ISSUE_FIELDS.len()
        && fields
            .iter()
            .zip(ISSUE_FIELDS)
            .all(|((name, ty), expected)| {
                name == expected && node(*ty) == Some(&TypeNode::Scalar(Scalar::String))
            })
}

/// Saved form version of `save_bindings`. Version 3 records the binding kind
/// by name (adding `bind validate`). Version 4 stores the scopes as nested
/// JSON, not as JSON text inside a string: the text was escaped once more at
/// every level a host nested it in, and unescaped again on every load.
const SAVED_BINDINGS: u32 = 4;

/// A binding kind in the saved form.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SavedKind {
    Scope,
    Default,
    Implicit,
    Validate,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedScope {
    binding: ScopeBinding,
    executable: SavedExecutable,
    kind: SavedKind,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedBindings<'a> {
    // Fields in key order: the saved text is canonical JSON (see
    // `save_bindings`).
    /// Host-chosen identity of the compiler that verified the images; a host
    /// must change it whenever the compiler changes.
    compiler: String,
    /// SHA-256 of the text of `scopes` exactly as saved, against truncation
    /// and corruption.
    digest: String,
    format: u32,
    /// The saved scopes: a JSON array, kept as its text.
    #[serde(borrow)]
    scopes: &'a serde_json::value::RawValue,
}

/// Save verified bindings for a host image cache (JSON text). Load them with
/// `load_bindings`, which skips compilation and verification.
///
/// The text is canonical JSON (`pack::canonical_json`: compact, object keys
/// sorted at every depth), so a host that nests it in a canonical JSON
/// document, such as a language-pack section, stores exactly this text and
/// the saved digest still covers what is read back.
pub fn save_bindings(scopes: &[VerifiedScope], compiler: &str) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let saved: Vec<_> = scopes
        .iter()
        .map(|scope| SavedScope {
            binding: scope.binding.clone(),
            executable: scope.executable.save(),
            kind: match scope.kind {
                BindingKind::Scope => SavedKind::Scope,
                BindingKind::Default => SavedKind::Default,
                BindingKind::Implicit => SavedKind::Implicit,
                BindingKind::Validate => SavedKind::Validate,
            },
        })
        .collect();
    let text = super::pack::canonical_json(&saved).map_err(|e| e.to_string())?;
    let digest = format!("{:x}", Sha256::digest(&text));
    let text = String::from_utf8(text).map_err(|e| e.to_string())?;
    let scopes = serde_json::value::RawValue::from_string(text).map_err(|e| e.to_string())?;
    serde_json::to_string(&SavedBindings {
        compiler: compiler.to_owned(),
        digest,
        format: SAVED_BINDINGS,
        scopes: &scopes,
    })
    .map_err(|e| e.to_string())
}

/// Load bindings that `save_bindings` saved, without compiling or verifying
/// them again. Trusted input only (see `VerifiedExecutable::load_trusted`):
/// the host's own cache, made by the same `compiler`, for exactly `bundle`.
/// Any mismatch is an error, after which the host compiles afresh.
pub fn load_bindings(
    text: &str,
    bundle: &DefinitionBundle,
    compiler: &str,
) -> Result<Vec<VerifiedScope>, String> {
    load_saved_bindings(text, bundle, compiler, true)
}

/// `load_bindings` for text whose integrity the caller already verified as a
/// whole, such as a digest-checked language-pack section: the saved digest of
/// the scopes, which would hash the same text a second time, is not
/// recomputed. Format, compiler and every image's context are checked as in
/// `load_bindings`.
pub fn load_verified_bindings(
    text: &str,
    bundle: &DefinitionBundle,
    compiler: &str,
) -> Result<Vec<VerifiedScope>, String> {
    load_saved_bindings(text, bundle, compiler, false)
}

fn load_saved_bindings(
    text: &str,
    bundle: &DefinitionBundle,
    compiler: &str,
    check_digest: bool,
) -> Result<Vec<VerifiedScope>, String> {
    use sha2::{Digest, Sha256};
    let saved: SavedBindings = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if saved.format != SAVED_BINDINGS {
        return Err("Saved bindings have another format".into());
    }
    if saved.compiler != compiler {
        return Err("Saved bindings were made by another compiler".into());
    }
    if check_digest
        && format!("{:x}", Sha256::digest(saved.scopes.get().as_bytes())) != saved.digest
    {
        return Err("Saved bindings are corrupt".into());
    }
    let scopes: Vec<SavedScope> =
        serde_json::from_str(saved.scopes.get()).map_err(|e| e.to_string())?;
    // One bundle context for every image: its context hash is computed once
    // (per distinct limits), not once per scope.
    let mut context = SavedContext::new(bundle);
    scopes
        .into_iter()
        .map(|scope| {
            Ok(VerifiedScope {
                binding: scope.binding,
                executable: VerifiedExecutable::load_trusted_in(scope.executable, &mut context)
                    .map_err(|e| e.to_string())?,
                kind: match scope.kind {
                    SavedKind::Scope => BindingKind::Scope,
                    SavedKind::Default => BindingKind::Default,
                    SavedKind::Implicit => BindingKind::Implicit,
                    SavedKind::Validate => BindingKind::Validate,
                },
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "rcore_scope_tests.rs"]
mod tests;
