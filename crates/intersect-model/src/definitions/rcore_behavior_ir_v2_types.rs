//! Checked flat wire type graph mapped into the existing Rcore type engine.
use super::*;
use crate::definitions::rcore_behavior::{model_types::ModelTypes, types::Type};

pub fn import_types(
    nodes: &[TypeNode],
    table: &mut TypeTable,
    models: &mut ModelTypes<'_>,
    limits: &Limits,
) -> Result<Vec<TypeRef>, IrError> {
    super::verification::serialized_size(&nodes, limits.scalar.max_source_bytes)?;
    if nodes.len() > limits.max_types {
        return Err(IrError {
            kind: IrErrorKind::ResourceExhausted,
            message: "Function type count limit exceeded".into(),
            span: None,
        });
    }
    let mut refs = Vec::new();
    let mut depths = Vec::<usize>::new();
    let mut seen = BTreeSet::new();
    let mut work = limits.scalar.max_work;
    for node in nodes {
        spend(&mut work, 1)?;
        let mut depth = 1usize;
        let mut child = |id: TypeId| -> Result<TypeRef, IrError> {
            spend(&mut work, 1)?;
            let value = refs
                .get(id.0 as usize)
                .copied()
                .ok_or_else(|| invalid("Type children must precede their parent", None))?;
            let child_depth = depths
                .get(id.0 as usize)
                .copied()
                .ok_or_else(|| invalid("Missing type depth", None))?;
            depth = depth.max(child_depth.saturating_add(1));
            Ok(value)
        };
        let ty = match node {
            TypeNode::Scalar(s) => Type::Scalar(*s),
            TypeNode::Enum(id) => Type::Enumeration(id.clone()),
            TypeNode::Datatype(id) => Type::Datatype(id.clone()),
            TypeNode::AppliedDatatype(t) => Type::DatatypeApplication(t.expression().clone()),
            TypeNode::Model(t) => Type::ModelApplication(t.expression().clone()),
            TypeNode::Option(t) => Type::Option(child(*t)?),
            TypeNode::Result { ok, error } => Type::Result {
                ok: child(*ok)?,
                error: child(*error)?,
            },
            TypeNode::Collection { kind, element } => Type::Collection {
                kind: kind.clone(),
                element: child(*element)?,
            },
            TypeNode::Many {
                element,
                lower,
                upper,
                ordered,
                unique,
            } => Type::Many {
                element: child(*element)?,
                lower: *lower,
                upper: *upper,
                ordered: *ordered,
                unique: *unique,
            },
            TypeNode::Tuple(items) => {
                Type::Tuple(items.iter().map(|i| child(*i)).collect::<Result<_, _>>()?)
            }
            TypeNode::Record(fields) => Type::Record(
                fields
                    .iter()
                    .map(|(k, v)| Ok((k.clone(), child(*v)?)))
                    .collect::<Result<_, IrError>>()?,
            ),
            TypeNode::Function { parameters, result } => Type::Function {
                parameters: parameters
                    .iter()
                    .map(|i| child(*i))
                    .collect::<Result<_, _>>()?,
                result: child(*result)?,
            },
        };
        if depth > limits.max_type_depth {
            return Err(IrError {
                kind: IrErrorKind::ResourceExhausted,
                message: "Function type depth limit exceeded".into(),
                span: None,
            });
        }
        let id = table.intern(ty).map_err(|e| invalid(e, None))?;
        if !seen.insert(id) {
            return Err(invalid("Duplicate canonical function type", None));
        }
        if !models
            .assignable(table, id, id)
            .map_err(|e| invalid(e, None))?
        {
            return Err(invalid("Invalid source-backed function type", None));
        }
        refs.push(id);
        depths.push(depth);
    }
    Ok(refs)
}

pub fn export_type(
    root: TypeRef,
    table: &TypeTable,
    output: &mut Vec<TypeNode>,
    mapped: &mut BTreeMap<TypeRef, TypeId>,
    limits: &Limits,
) -> Result<TypeId, IrError> {
    let mut pending = vec![(root, false)];
    let mut work = limits.scalar.max_work;
    while let Some((ty, exit)) = pending.pop() {
        spend(&mut work, 1)?;
        if mapped.contains_key(&ty) {
            continue;
        }
        let source = table.get(ty).map_err(|e| invalid(e, None))?;
        if !exit {
            pending.push((ty, true));
            pending.extend(
                crate::definitions::rcore_behavior::inference::children(source)
                    .into_iter()
                    .rev()
                    .map(|t| (t, false)),
            );
            continue;
        }
        let get = |ty: TypeRef| {
            mapped
                .get(&ty)
                .copied()
                .ok_or_else(|| invalid("Unexported child type", None))
        };
        let node = match source {
            Type::Scalar(s) => TypeNode::Scalar(*s),
            Type::Enumeration(id) => TypeNode::Enum(id.clone()),
            Type::Datatype(id) => TypeNode::Datatype(id.clone()),
            Type::DatatypeApplication(t) => {
                TypeNode::AppliedDatatype(ClosedType::new(t.clone()).map_err(|e| invalid(e, None))?)
            }
            Type::ModelApplication(t) => {
                TypeNode::Model(RuntimeType::new(t.clone()).map_err(|e| invalid(e, None))?)
            }
            Type::Model(id) => TypeNode::Model(
                RuntimeType::new(crate::definitions::generic_types::Type::Classifier {
                    declaration: id.clone(),
                    arguments: vec![],
                    raw: false,
                })
                .map_err(|e| invalid(e, None))?,
            ),
            Type::Option(t) => TypeNode::Option(get(*t)?),
            Type::Result { ok, error } => TypeNode::Result {
                ok: get(*ok)?,
                error: get(*error)?,
            },
            Type::Collection { kind, element } => TypeNode::Collection {
                kind: kind.clone(),
                element: get(*element)?,
            },
            Type::Many {
                element,
                lower,
                upper,
                ordered,
                unique,
            } => TypeNode::Many {
                element: get(*element)?,
                lower: *lower,
                upper: *upper,
                ordered: *ordered,
                unique: *unique,
            },
            Type::Tuple(items) => {
                TypeNode::Tuple(items.iter().map(|t| get(*t)).collect::<Result<_, _>>()?)
            }
            Type::Record(fields) => TypeNode::Record(
                fields
                    .iter()
                    .map(|(k, v)| Ok((k.clone(), get(*v)?)))
                    .collect::<Result<_, IrError>>()?,
            ),
            Type::Function { parameters, result } => TypeNode::Function {
                parameters: parameters
                    .iter()
                    .map(|t| get(*t))
                    .collect::<Result<_, _>>()?,
                result: get(*result)?,
            },
            _ => {
                return Err(invalid(
                    "Unresolved symbolic or rigid type cannot enter executable IR",
                    None,
                ));
            }
        };
        let id = if let Some(index) = output.iter().position(|n| n == &node) {
            TypeId(u32::try_from(index).map_err(|_| invalid("Type index overflow", None))?)
        } else {
            if output.len() >= limits.max_types {
                return Err(IrError {
                    kind: IrErrorKind::ResourceExhausted,
                    message: "Function type count limit exceeded".into(),
                    span: None,
                });
            }
            let id = TypeId(
                u32::try_from(output.len()).map_err(|_| invalid("Type index overflow", None))?,
            );
            output.push(node);
            id
        };
        mapped.insert(ty, id);
    }
    mapped
        .get(&root)
        .copied()
        .ok_or_else(|| invalid("Missing exported type", None))
}

pub fn policy_manifest(
    registry: Option<&ConversionRegistry<'_>>,
) -> Result<PolicyManifest, IrError> {
    let Some(registry) = registry else {
        return Ok(PolicyManifest::default());
    };
    fn canonical<T: Serialize>(values: Vec<T>) -> Result<Vec<T>, IrError> {
        let mut keyed = values
            .into_iter()
            .map(|value| {
                Ok((
                    serde_json::to_string(&value).map_err(|e| invalid(e.to_string(), None))?,
                    value,
                ))
            })
            .collect::<Result<Vec<_>, IrError>>()?;
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(keyed.into_iter().map(|(_, value)| value).collect())
    }
    Ok(PolicyManifest {
        conversions: canonical(
            registry
                .registered_contracts()
                .map_err(|e| invalid(e, None))?,
        )?,
        applied: canonical(
            registry
                .applied()
                .registered_contracts()
                .map_err(|e| invalid(e, None))?,
        )?,
        constructors: canonical(
            registry
                .constructors()
                .registered_contracts()
                .map_err(|e| invalid(e, None))?,
        )?,
    })
}
