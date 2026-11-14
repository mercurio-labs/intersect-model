//! Source-authorized contracts shared by native, Rcore and optional script adapters.
//! Descriptors are inspection data; only source compilation creates a contract.
use crate::definitions::{
    bundle::{DefinitionBundle, SourceId, SourceSpan},
    rcore_behavior::{
        ast::Declaration,
        function_lowering::compile_contract,
        ir_v2::{
            Effect, EntrySelection, FunctionIdentity, Limits, TypeId, TypeNode, VerifiedExecutable,
        },
        modules::SymbolId,
    },
    rcore_operations::{OperationBinding, OperationSelection},
};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[path = "semantic_binding_conditions.rs"]
mod conditions;
pub use conditions::{Condition, ConditionDescriptor};

/// Generated inspection data over checked effective members. Runtime adapters
/// use these same records; unsupported members retain an explicit explanation.
#[derive(Clone, Debug, Serialize)]
pub struct ModelProperties {
    pub parameter: usize,
    pub receiver: crate::definitions::generic_types::RuntimeType,
    pub class_name: String,
    pub properties: Vec<ModelProperty>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ModelProperty {
    pub name: String,
    pub identity: crate::definitions::schema::MemberIdentity,
    pub scalar: Option<crate::definitions::rcore_behavior::types::Scalar>,
    pub optional: bool,
    pub unsupported: Option<String>,
}

fn model_properties(
    bundle: &DefinitionBundle,
    descriptor: &Descriptor,
) -> Result<Vec<ModelProperties>, String> {
    use crate::definitions::rcore_behavior::{
        model_types::ModelTypes,
        types::{Scalar, Type, TypeTable},
    };
    if !descriptor.parameters.iter().any(|id| {
        matches!(
            descriptor.types.get(id.0 as usize),
            Some(TypeNode::Model(_))
        )
    }) {
        return Ok(Vec::new());
    }
    let mut models = ModelTypes::new(bundle)?;
    let mut table = TypeTable::default();
    let mut bindings = Vec::new();
    for (parameter, id) in descriptor.parameters.iter().enumerate() {
        let Some(TypeNode::Model(receiver)) = descriptor.types.get(id.0 as usize) else {
            continue;
        };
        let checked = models.runtime_schema(std::slice::from_ref(receiver.expression()))?;
        let class_key = checked.class_key_for_type(receiver)?;
        let members = checked
            .schema()
            .effective_features(class_key)?
            .into_iter()
            .map(|member| (member.identity, member.feature.clone()))
            .collect::<Vec<_>>();
        let class_name = models.classifier_name(receiver)?.to_owned();
        let mut properties = Vec::new();
        for (identity, feature) in members {
            let mut scalar = None;
            let unsupported = if feature.reference {
                Some("Reference traversal is unsupported by Rhai bindings".into())
            } else if feature.derived || feature.volatile {
                Some("Rhai properties require stored attributes".into())
            } else if feature.upper != 1 {
                Some("Collection properties are unsupported by Rhai bindings".into())
            } else {
                match models.feature_application(&mut table, receiver.expression(), &feature.name) {
                    Ok(projected) => {
                        let ty = match table.get(projected.value)? {
                            Type::Option(element) => table.get(*element)?,
                            ty => ty,
                        };
                        match ty {
                            Type::Scalar(
                                ty @ (Scalar::String
                                | Scalar::Bool
                                | Scalar::Int8
                                | Scalar::Int16
                                | Scalar::Int32
                                | Scalar::Int64),
                            ) => {
                                scalar = Some(*ty);
                                None
                            }
                            _ => Some("Property type is unsupported by Rhai bindings".into()),
                        }
                    }
                    Err(message) => Some(message),
                }
            };
            properties.push(ModelProperty {
                name: feature.name,
                identity,
                scalar,
                optional: feature.lower == 0,
                unsupported,
            });
        }
        bindings.push(ModelProperties {
            parameter,
            receiver: receiver.clone(),
            class_name,
            properties,
        });
    }
    Ok(bindings)
}

#[derive(Clone, Debug, Serialize)]
pub struct Descriptor {
    pub external: bool,
    pub id: String,
    pub name: String,
    pub symbol: SymbolId,
    pub span: SourceSpan,
    pub parameter_names: Vec<String>,
    pub parameters: Vec<TypeId>,
    pub result: TypeId,
    pub types: Vec<TypeNode>,
    pub effect: Effect,
    pub operation: Option<OperationBinding>,
    pub preconditions: Vec<ConditionDescriptor>,
    pub postconditions: Vec<ConditionDescriptor>,
}

/// A source-checked signature, with an optional Rcore reference implementation.
#[derive(Clone, Debug)]
pub struct Contract {
    descriptor: Descriptor,
    reference: VerifiedExecutable,
    model_properties: Vec<ModelProperties>,
    preconditions: Vec<Condition>,
    postconditions: Vec<Condition>,
}

/// Bounded source compiler for authoring bindings. External declarations need no
/// Rcore body; implementations are installed separately by a host.
pub struct SourceContracts {
    /// Exact rule source with external signatures retained for type checking.
    pub model_behavior: String,
    pub bundle: DefinitionBundle,
    pub model_source: crate::definitions::bundle::DefinitionSource,
    pub namespace: String,
    pub receiver_types: Vec<crate::definitions::generic_types::RuntimeType>,
    pub model_classes:
        std::collections::BTreeMap<String, crate::definitions::generic_types::RuntimeType>,
    pub contracts: Vec<Contract>,
}
pub fn compile_sources(structure: &str, behavior: &str) -> Result<SourceContracts, String> {
    compile_sources_with_model_uri(structure, behavior, "memory:bindings/model.rcore")
}

/// Keep the authored model's source identity aligned with the caller's model
/// package, including nominal enum values retained in model snapshots.
pub fn compile_sources_with_model_uri(
    structure: &str,
    behavior: &str,
    model_uri: &str,
) -> Result<SourceContracts, String> {
    use crate::definitions::{
        bundle::{DefinitionSource, SourceKind},
        generic_types::{RuntimeType, Type},
    };
    if structure.len() > 262_144 || behavior.len() > 262_144 {
        return Err("Binding definition exceeds 256 KiB".into());
    }
    let model = DefinitionSource::new(model_uri, SourceKind::Rcore, structure);
    let entry = if behavior.trim().is_empty() {
        model.clone()
    } else {
        let mut source = DefinitionSource::new(
            "memory:bindings/behavior.rcore",
            SourceKind::RcoreBehavior,
            behavior,
        );
        source.dependencies.push(model.id.clone());
        source
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: if entry.id == model.id {
            vec![model.clone()]
        } else {
            vec![model.clone(), entry.clone()]
        },
        namespaces: vec![],
    };
    let structural = crate::definitions::rcore::compile_structure(&bundle, &model.id)
        .map_err(|e| format!("{e:?}"))?;
    let schema = structural.schema;
    let model_classes = structural
        .classifiers
        .iter()
        .filter(|c| schema.classes.contains_key(&c.name))
        .map(|c| {
            RuntimeType::new(Type::Classifier {
                declaration: c.declaration.clone(),
                arguments: vec![],
                raw: false,
            })
            .map(|ty| (c.name.clone(), ty))
        })
        .collect::<Result<std::collections::BTreeMap<_, _>, _>>()?;
    let receivers = model_classes.values().cloned().collect::<Vec<_>>();
    let document = crate::definitions::rcore_behavior::parse(entry.clone());
    if !document.diagnostics.is_empty() {
        return Err(format!("{:?}", document.diagnostics));
    }
    let mut contracts = Vec::new();
    for (ordinal, declaration) in document.declarations.iter().enumerate() {
        let Declaration::Function { name, .. } = &declaration.value else {
            continue;
        };
        if contracts.len() >= 64 {
            return Err("Binding package exceeds 64 functions".into());
        }
        let selection = EntrySelection {
            symbol: SymbolId {
                source: entry.id.clone(),
                declaration: ordinal,
            },
            types: vec![],
            arguments: vec![],
        };
        let mut contract = Contract::function(&bundle, &entry.id, &selection)?;
        if let Some(TypeNode::Model(receiver)) = contract
            .descriptor
            .parameters
            .first()
            .and_then(|id| contract.descriptor.types.get(id.0 as usize))
            && let Some(operation) = structural.operations.iter().find(|op|
                op.name == name.value && matches!(receiver.expression(), Type::Classifier { declaration, .. } if declaration == &op.owner)) {
                contract = Contract::operation(&bundle, &entry.id, &OperationSelection {
                    receiver: receiver.clone(), declaration: operation.declaration.clone(),
                    materialized_types: receivers.clone(), implementation: selection,
                })?;
            }
        contracts.push(contract);
    }
    if contracts.is_empty() {
        return Err("Declare at least one Rcore function".into());
    }
    Ok(SourceContracts {
        model_behavior: model_behavior(&document)?,
        bundle,
        model_source: model,
        namespace: schema.uri,
        receiver_types: receivers,
        model_classes,
        contracts,
    })
}

/// Reference bodies remain standalone registry entries. External signatures stay
/// in the model compiler so derived calls are checked against their contracts.
pub fn model_behavior(
    document: &crate::definitions::rcore_behavior::ast::Document,
) -> Result<String, String> {
    let mut bytes = document.source.text.as_bytes().to_vec();
    if document.source.kind == crate::definitions::bundle::SourceKind::Rcore {
        let lexed = crate::definitions::rcore_behavior::lex(document.source.clone());
        let tokens = crate::definitions::rcore::behavior_tokens(
            &document.source,
            &lexed.tokens,
            document.source.text.len(),
        )
        .map_err(|errors| format!("{errors:?}"))?;
        for byte in &mut bytes {
            if !matches!(*byte, b'\r' | b'\n') {
                *byte = b' ';
            }
        }
        for token in tokens {
            let range = token.span.start..token.span.end;
            bytes[range.clone()].copy_from_slice(&document.source.text.as_bytes()[range]);
        }
    }
    for declaration in &document.declarations {
        if matches!(
            declaration.value,
            Declaration::Function {
                external: false,
                ..
            }
        ) {
            for byte in &mut bytes[declaration.span.start..declaration.span.end] {
                if !matches!(*byte, b'\r' | b'\n') {
                    *byte = b' ';
                }
            }
        }
    }
    String::from_utf8(bytes).map_err(|e| e.to_string())
}
impl Contract {
    pub fn function(
        bundle: &DefinitionBundle,
        entry: &SourceId,
        selection: &EntrySelection,
    ) -> Result<Self, String> {
        let image = compile_contract(bundle, entry, selection, &Limits::default())
            .map_err(|errors| format!("{errors:?}"))?;
        Self::from_image(bundle, image, None)
    }
    pub fn operation(
        bundle: &DefinitionBundle,
        entry: &SourceId,
        selection: &OperationSelection,
    ) -> Result<Self, String> {
        let image = compile_contract(bundle, entry, &selection.implementation, &Limits::default())
            .map_err(|errors| format!("{errors:?}"))?;
        let binding = crate::definitions::rcore_operations::verify_binding(
            bundle,
            &image,
            &selection.receiver,
            &selection.declaration,
            &selection.materialized_types,
            &Limits::default(),
        )
        .map_err(|errors| format!("{errors:?}"))?;
        Self::from_image(bundle, image, Some(binding))
    }
    fn from_image(
        bundle: &DefinitionBundle,
        reference: VerifiedExecutable,
        operation: Option<OperationBinding>,
    ) -> Result<Self, String> {
        let program = reference.program();
        let entry = program
            .functions
            .get(program.entry.0 as usize)
            .ok_or("Missing entry")?;
        let FunctionIdentity::Named { symbol, .. } = &entry.identity else {
            return Err("A binding requires a named function".into());
        };
        let source = bundle
            .sources
            .iter()
            .find(|s| s.id == symbol.source)
            .ok_or("Missing source")?;
        let document = crate::definitions::rcore_behavior::parse(source.clone());
        let declaration = document
            .declarations
            .get(symbol.declaration)
            .ok_or("Missing declaration")?;
        let Declaration::Function {
            name,
            parameters,
            external,
            ..
        } = &declaration.value
        else {
            return Err("Binding declaration is not a function".into());
        };
        let mut descriptor = Descriptor {
            external: *external,
            id: String::new(),
            name: name.value.clone(),
            symbol: symbol.clone(),
            span: entry.span.clone(),
            parameter_names: parameters.iter().map(|p| p.name.value.clone()).collect(),
            parameters: entry.parameters.clone(),
            result: entry.result,
            types: program.types.clone(),
            effect: reference
                .function_effect(program.entry)
                .map_err(|e| e.to_string())?,
            operation,
            preconditions: vec![],
            postconditions: vec![],
        };
        // Include exact dependencies/policies and the operation declaration, not just its name.
        let bytes = serde_json::to_vec(&(&descriptor, &program.sources, &program.policies))
            .map_err(|e| e.to_string())?;
        descriptor.id = format!("sha256:{:x}", Sha256::digest(bytes));
        let model_properties = model_properties(bundle, &descriptor)?;
        Ok(Self {
            descriptor,
            reference,
            model_properties,
            preconditions: vec![],
            postconditions: vec![],
        })
    }
    pub fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }
    pub fn model_properties(&self) -> &[ModelProperties] {
        &self.model_properties
    }
    pub fn reference(&self) -> Option<&VerifiedExecutable> {
        (!self.descriptor.external).then_some(&self.reference)
    }
    /// Checked signature/type/context evidence used by shared argument/result admission.
    /// An external entry has no executable blocks.
    pub fn admission_image(&self) -> &VerifiedExecutable {
        &self.reference
    }
    pub fn id(&self) -> &str {
        &self.descriptor.id
    }
    /// Stable generated symbol avoids overload and user-identifier collisions.
    pub fn script_name(&self) -> String {
        if self.descriptor.external {
            self.descriptor.name.clone()
        } else {
            format!("binding_{}", self.descriptor.symbol.declaration)
        }
    }
    pub fn rhai_stub(&self) -> String {
        let describe = |id: TypeId| {
            self.descriptor
                .types
                .get(id.0 as usize)
                .map(|ty| format!("{ty:?}"))
                .unwrap_or_else(|| "unknown".into())
        };
        let annotations = self
            .descriptor
            .parameters
            .iter()
            .enumerate()
            .map(|(i, ty)| {
                format!(
                    "// arg{i} ({}): {}\n",
                    self.descriptor.parameter_names[i],
                    describe(*ty)
                )
            })
            .collect::<String>();
        let parameters = (0..self.descriptor.parameters.len())
            .map(|i| {
                if self.descriptor.external {
                    self.descriptor.parameter_names[i].clone()
                } else {
                    format!("arg{i}")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "// Contract: {}\n// {} -> {}\n{}// User-owned implementation. Regeneration must preserve this body.\nfn {}({}) {{\n    throw \"Implementation required\";\n}}\n",
            self.id(),
            self.descriptor.name,
            describe(self.descriptor.result),
            annotations,
            self.script_name(),
            parameters,
        )
    }
}
