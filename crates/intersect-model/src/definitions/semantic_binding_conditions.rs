//! Source-compiled contract predicates. Host declarations select exact Rcore
//! functions; neither serialized metadata nor native callbacks can mint a check.
use super::*;
use crate::definitions::rcore_behavior::{
    function_lowering::compile_function,
    ir_v2::{DataWire, ValueWire, export_type, import_types},
    model_types::ModelTypes,
    types::{Scalar, TypeTable},
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize)]
pub struct ConditionDescriptor {
    pub symbol: SymbolId,
    pub span: SourceSpan,
}
#[derive(Clone, Debug)]
pub struct Condition {
    image: VerifiedExecutable,
    // Type IDs are local to an executable. Conversion preserves nominal source
    // identity and only rebases indices between equal canonical type graphs.
    types: BTreeMap<TypeId, TypeId>,
}
impl Condition {
    pub fn executable(&self) -> &VerifiedExecutable {
        &self.image
    }
    pub fn arguments(&self, values: &[ValueWire]) -> Result<Vec<ValueWire>, String> {
        fn rebase(
            value: &ValueWire,
            types: &BTreeMap<TypeId, TypeId>,
            depth: usize,
        ) -> Result<ValueWire, String> {
            if depth > Limits::default().max_type_depth {
                return Err("Condition value depth exceeded".into());
            }
            let ty = *types
                .get(&value.ty)
                .ok_or("Condition value type is not in its checked signature")?;
            let child = |v: &ValueWire| rebase(v, types, depth + 1);
            let data = match &value.data {
                DataWire::Some(v) => DataWire::Some(Box::new(child(v)?)),
                DataWire::Ok(v) => DataWire::Ok(Box::new(child(v)?)),
                DataWire::Err(v) => DataWire::Err(Box::new(child(v)?)),
                DataWire::Collection(v) => {
                    DataWire::Collection(v.iter().map(child).collect::<Result<_, _>>()?)
                }
                DataWire::Tuple(v) => {
                    DataWire::Tuple(v.iter().map(child).collect::<Result<_, _>>()?)
                }
                DataWire::Record(v) => DataWire::Record(
                    v.iter()
                        .map(|(k, v)| Ok((k.clone(), child(v)?)))
                        .collect::<Result<_, String>>()?,
                ),
                data => data.clone(),
            };
            Ok(ValueWire { ty, data })
        }
        values.iter().map(|v| rebase(v, &self.types, 0)).collect()
    }
}

fn canonical_types(
    bundle: &DefinitionBundle,
    nodes: &[TypeNode],
) -> Result<Vec<Vec<TypeNode>>, String> {
    let limits = Limits::default();
    let mut models = ModelTypes::new(bundle)?;
    let mut table = TypeTable::default();
    let refs = import_types(nodes, &mut table, &mut models, &limits).map_err(|e| e.to_string())?;
    refs.into_iter()
        .map(|ty| {
            let mut output = vec![];
            export_type(ty, &table, &mut output, &mut BTreeMap::new(), &limits)
                .map_err(|e| e.to_string())?;
            Ok(output)
        })
        .collect()
}

impl Contract {
    /// An explicit host declaration links source-authored predicates to a checked
    /// contract. Preconditions take its arguments; postconditions additionally
    /// take its result last. Every predicate returns Bool. Effects stay read-only.
    pub fn with_conditions(
        mut self,
        bundle: &DefinitionBundle,
        entry: &SourceId,
        preconditions: &[EntrySelection],
        postconditions: &[EntrySelection],
    ) -> Result<Self, String> {
        if !self.preconditions.is_empty() || !self.postconditions.is_empty() {
            return Err("Contract conditions are already declared".into());
        }
        if preconditions.len() + postconditions.len() > 32 {
            return Err("Contract exceeds 32 conditions".into());
        }
        self.reference
            .check_context(bundle, None)
            .map_err(|e| e.to_string())?;
        let source_types = canonical_types(bundle, &self.descriptor.types)?;
        for (post, selections) in [(false, preconditions), (true, postconditions)] {
            for selection in selections {
                let image = compile_function(bundle, entry, selection, &Limits::default())
                    .map_err(|e| format!("{e:?}"))?;
                let program = image.program();
                let function = program
                    .functions
                    .get(program.entry.0 as usize)
                    .ok_or("Missing predicate entry")?;
                if program.types.get(function.result.0 as usize)
                    != Some(&TypeNode::Scalar(Scalar::Bool))
                {
                    return Err("Contract predicate must return Bool".into());
                }
                let target_types = canonical_types(bundle, &program.types)?;
                let mut expected = self.descriptor.parameters.clone();
                if post {
                    expected.push(self.descriptor.result);
                }
                if expected.len() != function.parameters.len()
                    || expected.iter().zip(&function.parameters).any(|(a, b)| {
                        source_types.get(a.0 as usize) != target_types.get(b.0 as usize)
                    })
                {
                    return Err(
                        "Contract predicate parameters must match exact argument/result types"
                            .into(),
                    );
                }
                let metadata = ConditionDescriptor {
                    symbol: selection.symbol.clone(),
                    span: function.span.clone(),
                };
                let types = source_types
                    .iter()
                    .enumerate()
                    .filter_map(|(a, ty)| {
                        target_types
                            .iter()
                            .position(|other| ty == other)
                            .map(|b| (TypeId(a as u32), TypeId(b as u32)))
                    })
                    .collect();
                let condition = Condition { image, types };
                if post {
                    self.postconditions.push(condition);
                    self.descriptor.postconditions.push(metadata);
                } else {
                    self.preconditions.push(condition);
                    self.descriptor.preconditions.push(metadata);
                }
            }
        }
        let bytes = serde_json::to_vec(&(
            &self.descriptor,
            self.preconditions
                .iter()
                .chain(&self.postconditions)
                .map(|c| c.image.program())
                .collect::<Vec<_>>(),
        ))
        .map_err(|e| e.to_string())?;
        self.descriptor.id = format!("sha256:{:x}", Sha256::digest(bytes));
        Ok(self)
    }
    pub fn preconditions(&self) -> &[Condition] {
        &self.preconditions
    }
    pub fn postconditions(&self) -> &[Condition] {
        &self.postconditions
    }
}
