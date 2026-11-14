//! Flat coverage trie for unary Option/Result constructors and scalar patterns.
use super::{
    ast::{Document, Literal, Pattern, PatternId},
    bindings::{ModuleBindings, Target},
    types::{Scalar, Type, TypeRef, TypeTable},
};
use std::collections::BTreeMap;

struct Coverage {
    ty: TypeRef,
    all: bool,
    branches: BTreeMap<String, usize>,
}
pub(crate) struct Patterns {
    nodes: Vec<Coverage>,
    bounds: BTreeMap<TypeRef, TypeRef>,
}
impl Patterns {
    pub fn new(
        ty: TypeRef,
        table: &TypeTable,
        models: &mut super::model_types::ModelTypes<'_>,
        bounds: &BTreeMap<TypeRef, TypeRef>,
    ) -> Result<Self, String> {
        Ok(Self {
            bounds: bounds.clone(),
            nodes: vec![Coverage {
                ty,
                all: !inhabited(ty, table, models, bounds)?,
                branches: BTreeMap::new(),
            }],
        })
    }
    pub fn exhaustive(&self) -> bool {
        self.nodes[0].all
    }
    /// Returns exact local declaration offsets and payload types for this arm.
    pub fn arm(
        &mut self,
        doc: &Document,
        bindings: &ModuleBindings,
        table: &mut TypeTable,
        models: &mut super::model_types::ModelTypes<'_>,
        root: PatternId,
    ) -> Result<Vec<(usize, TypeRef)>, String> {
        let unit = table.intern(Type::Scalar(Scalar::Unit))?;
        let mut current = 0;
        let mut pattern = root;
        let mut ancestors = Vec::new();
        let mut locals = Vec::new();
        loop {
            if self.nodes[current].all {
                return Err("Unreachable match arm".into());
            }
            let original = self.nodes[current].ty;
            let ty = view(original, &self.bounds)?;
            let (key, payload, next) = match &doc.patterns[pattern.0].value {
                Pattern::Wildcard => {
                    self.nodes[current].all = true;
                    break;
                }
                Pattern::Binding(name) => {
                    locals.push((name.span.start, original));
                    self.nodes[current].all = true;
                    break;
                }
                Pattern::Literal(Literal::None) => {
                    if !matches!(table.get(ty)?, Type::Option(_)) {
                        return Err("None pattern requires Option".into());
                    }
                    ("None".to_owned(), unit, None)
                }
                Pattern::Literal(literal) => {
                    let Type::Scalar(scalar) = table.get(ty)? else {
                        return Err("Literal pattern requires a compatible scalar".into());
                    };
                    (scalar.literal_key(literal)?, unit, None)
                }
                Pattern::Constructor { arguments, .. }
                    if matches!(
                        bindings.constructors.get(&pattern.0),
                        Some(Target::EnumLiteral { .. })
                    ) =>
                {
                    let Some(Target::EnumLiteral {
                        enumeration,
                        literal,
                    }) = bindings.constructors.get(&pattern.0)
                    else {
                        return Err("Missing enum pattern identity".into());
                    };
                    if table.get(ty)? != &Type::Enumeration(enumeration.clone()) {
                        return Err("Enum pattern has an incompatible scrutinee type".into());
                    }
                    if !arguments.is_empty() {
                        return Err("Enum literal patterns take no arguments".into());
                    }
                    let literal = models
                        .enumeration(enumeration)?
                        .by_id(literal)
                        .ok_or("Unknown enum pattern literal")?;
                    (format!("enum:{}", literal.name), unit, None)
                }
                Pattern::Constructor { arguments, .. } => {
                    let Some(Target::Builtin { name, .. }) = bindings.constructors.get(&pattern.0)
                    else {
                        return Err(
                            "Pattern constructor must be a builtin Option/Result constructor"
                                .into(),
                        );
                    };
                    let payload = match (name.as_str(), table.get(ty)?) {
                        ("None", Type::Option(_)) => None,
                        ("Some", Type::Option(inner)) => Some(*inner),
                        ("Ok", Type::Result { ok, .. }) => Some(*ok),
                        ("Err", Type::Result { error, .. }) => Some(*error),
                        _ => {
                            return Err(
                                "Constructor pattern has an incompatible scrutinee type".into()
                            );
                        }
                    };
                    if arguments.len() != usize::from(payload.is_some()) {
                        return Err(format!("{name} pattern has incorrect constructor arity"));
                    }
                    (
                        name.clone(),
                        payload.unwrap_or(unit),
                        arguments.first().copied(),
                    )
                }
            };
            let child = if let Some(child) = self.nodes[current].branches.get(&key) {
                *child
            } else {
                let child = self.nodes.len();
                self.nodes.push(Coverage {
                    ty: payload,
                    all: !inhabited(payload, table, models, &self.bounds)?,
                    branches: BTreeMap::new(),
                });
                self.nodes[current].branches.insert(key, child);
                child
            };
            ancestors.push(current);
            current = child;
            if let Some(next) = next {
                pattern = next;
            } else {
                if self.nodes[current].all {
                    return Err("Unreachable match arm".into());
                }
                self.nodes[current].all = true;
                break;
            }
        }
        for parent in ancestors.into_iter().rev() {
            let keys: Vec<String> = match table.get(view(self.nodes[parent].ty, &self.bounds)?)? {
                Type::Enumeration(id) => models
                    .enumeration(id)?
                    .literals()
                    .iter()
                    .map(|literal| format!("enum:{}", literal.name))
                    .collect(),
                Type::Scalar(Scalar::Bool) => vec!["true".into(), "false".into()],
                Type::Option(inner) => {
                    if inhabited(*inner, table, models, &self.bounds)? {
                        vec!["Some".into(), "None".into()]
                    } else {
                        vec!["None".into()]
                    }
                }
                Type::Result { ok, error } => {
                    let mut keys = vec![];
                    if inhabited(*ok, table, models, &self.bounds)? {
                        keys.push("Ok".into());
                    }
                    if inhabited(*error, table, models, &self.bounds)? {
                        keys.push("Err".into());
                    }
                    keys
                }
                _ => vec![],
            };
            if !keys.is_empty()
                && keys.iter().all(|key| {
                    self.nodes[parent]
                        .branches
                        .get(key)
                        .is_some_and(|child| self.nodes[*child].all)
                })
            {
                self.nodes[parent].all = true;
            }
        }
        Ok(locals)
    }
}

fn view(mut ty: TypeRef, bounds: &BTreeMap<TypeRef, TypeRef>) -> Result<TypeRef, String> {
    let mut remaining = 16384usize;
    while let Some(bound) = bounds.get(&ty) {
        remaining = remaining
            .checked_sub(1)
            .ok_or("Pattern generic bound work limit exceeded")?;
        ty = *bound;
    }
    Ok(ty)
}

// Shared type nodes are visited once, even when both Result arms reuse a type.
fn inhabited(
    ty: TypeRef,
    table: &TypeTable,
    models: &mut super::model_types::ModelTypes<'_>,
    bounds: &BTreeMap<TypeRef, TypeRef>,
) -> Result<bool, String> {
    let mut known = BTreeMap::new();
    let mut pending = vec![(ty, false)];
    let mut remaining = 16384usize;
    while let Some((current, exiting)) = pending.pop() {
        remaining = remaining
            .checked_sub(1)
            .ok_or("Pattern generic bound work limit exceeded")?;
        if known.contains_key(&current) {
            continue;
        }
        if let Some(bound) = bounds.get(&current) {
            if exiting {
                let value = *known.get(bound).ok_or("Missing generic bound domain")?;
                known.insert(current, value);
            } else {
                pending.extend([(current, true), (*bound, false)]);
            }
            continue;
        }
        match table.get(current)? {
            Type::Enumeration(id) => {
                known.insert(current, !models.enumeration(id)?.literals().is_empty());
            }
            Type::Result { ok, error } if !exiting => {
                pending.extend([(current, true), (*error, false), (*ok, false)]);
            }
            Type::Result { ok, error } => {
                let a = *known.get(ok).ok_or("Missing Result domain")?;
                let b = *known.get(error).ok_or("Missing Result domain")?;
                known.insert(current, a || b);
            }
            _ => {
                known.insert(current, true);
            }
        }
    }
    known
        .get(&ty)
        .copied()
        .ok_or_else(|| "Missing pattern domain".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        resources::DeclarationId,
    };

    #[test]
    fn shared_empty_result_domains_do_not_expand_exponentially() {
        let source = DefinitionSource::new(
            "memory:empty.ecore",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:p'><eClassifiers xsi:type='e:EEnum' name='Empty'/></e:EPackage>",
        );
        let declaration = DeclarationId {
            source: source.id.clone(),
            path: vec![0],
        };
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        };
        let mut models = super::super::model_types::ModelTypes::new(&bundle).unwrap();
        let mut table = TypeTable::default();
        let mut ty = table.intern(Type::Enumeration(declaration)).unwrap();
        for _ in 0..62 {
            ty = table.intern(Type::Result { ok: ty, error: ty }).unwrap();
        }
        assert!(!inhabited(ty, &table, &mut models, &BTreeMap::new()).unwrap());
        let optional = table.intern(Type::Option(ty)).unwrap();
        assert!(inhabited(optional, &table, &mut models, &BTreeMap::new()).unwrap());
    }
}
