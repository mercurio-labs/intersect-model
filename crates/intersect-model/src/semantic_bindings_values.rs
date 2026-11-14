//! Explicit Rust representations over the authoritative shared value checker.
//! Collections retain kind; nominal values retain source/classifier identity.
use super::*;
use intersect_model::definitions::rcore_behavior::types::CollectionKind;

fn unsupported() -> Error {
    Error::new(
        ErrorKind::Unsupported,
        "This Rust value requires type-aware conversion through Call",
    )
}
fn scalar(value: &ValueWire, expected: Scalar) -> Result<&serde_json::Value> {
    match &value.data {
        DataWire::Scalar(v) if v.ty == ir::ValueType::Scalar(expected) => {
            ir::validate_scalar_value(v, &ir::ValueLimits::default())
                .map_err(|e| Error::new(ErrorKind::Arguments, e))?;
            Ok(&v.payload)
        }
        _ => Err(Error::new(
            ErrorKind::Arguments,
            "Incorrect scalar representation",
        )),
    }
}
macro_rules! integer {
    ($rust:ty, $scalar:ident, $getter:ident) => {
        impl NativeValue for $rust {
            fn matches(node: &TypeNode) -> bool {
                *node == TypeNode::Scalar(Scalar::$scalar)
            }
            fn from_wire(value: &ValueWire) -> Result<Self> {
                scalar(value, Scalar::$scalar)?
                    .$getter()
                    .and_then(|v| <$rust>::try_from(v).ok())
                    .ok_or_else(|| {
                        Error::new(ErrorKind::Arguments, "Integer outside exact Rust range")
                    })
            }
            fn into_wire(self, ty: TypeId) -> Result<ValueWire> {
                Ok(ValueWire {
                    ty,
                    data: DataWire::Scalar(ir::Value {
                        ty: ir::ValueType::Scalar(Scalar::$scalar),
                        payload: self.into(),
                    }),
                })
            }
        }
    };
}
integer!(i8, Int8, as_i64);
integer!(i16, Int16, as_i64);
integer!(i32, Int32, as_i64);
integer!(u8, UInt8, as_u64);
integer!(u16, UInt16, as_u64);
integer!(u32, UInt32, as_u64);
impl NativeValue for u64 {
    fn matches(node: &TypeNode) -> bool {
        *node == TypeNode::Scalar(Scalar::UInt64)
    }
    fn from_wire(value: &ValueWire) -> Result<Self> {
        scalar(value, Scalar::UInt64)?
            .get("value")
            .and_then(|v| v.as_str())
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| Error::new(ErrorKind::Arguments, "Expected exact UInt64"))
    }
    fn into_wire(self, ty: TypeId) -> Result<ValueWire> {
        Ok(ValueWire {
            ty,
            data: DataWire::Scalar(ir::Value {
                ty: ir::ValueType::Scalar(Scalar::UInt64),
                payload: serde_json::json!({"kind":"uint64","value":self.to_string()}),
            }),
        })
    }
}
impl NativeValue for () {
    fn matches(node: &TypeNode) -> bool {
        *node == TypeNode::Scalar(Scalar::Unit)
    }
    fn from_wire(value: &ValueWire) -> Result<Self> {
        scalar(value, Scalar::Unit).map(|_| ())
    }
    fn into_wire(self, ty: TypeId) -> Result<ValueWire> {
        Ok(ValueWire {
            ty,
            data: DataWire::Scalar(ir::Value {
                ty: ir::ValueType::Scalar(Scalar::Unit),
                payload: serde_json::json!({"kind":"unit"}),
            }),
        })
    }
}

macro_rules! collection {
    ($rust:ident, $kind:ident, $items:expr, $wrap:expr) => {
        impl<T: NativeValue> NativeValue for $rust<T> {
            fn matches(node: &TypeNode) -> bool { matches!(node,TypeNode::Collection { kind: CollectionKind::$kind, .. }) }
            fn matches_type(types: &[TypeNode],ty: TypeId) -> bool {
                matches!(types.get(ty.0 as usize),Some(TypeNode::Collection {kind: CollectionKind::$kind,element}) if T::matches_type(types,*element))
            }
            fn from_wire(_: &ValueWire) -> Result<Self> { Err(unsupported()) }
            fn into_wire(self,_: TypeId) -> Result<ValueWire> { Err(unsupported()) }
            fn from_typed(value: &ValueWire,types: &[TypeNode],budget: &Budget) -> Result<Self> {
                budget.charge(1)?;
                if !Self::matches_type(types,value.ty) { return Err(Error::new(ErrorKind::Arguments,"Collection kind or element differs")); }
                let DataWire::Collection(items) = &value.data else { return Err(Error::new(ErrorKind::Arguments,"Expected collection")); };
                let items = items.iter().map(|v| T::from_typed(v,types,budget)).collect::<Result<Vec<_>>>()?;
                Ok(($wrap)(items))
            }
            fn into_typed(self,ty: TypeId,types: &[TypeNode],budget: &Budget) -> Result<ValueWire> {
                budget.charge(1)?;
                if !Self::matches_type(types,ty) { return Err(Error::new(ErrorKind::Result,"Collection kind or element differs")); }
                let Some(TypeNode::Collection {element,..}) = types.get(ty.0 as usize) else { return Err(unsupported()); };
                let items: Vec<T> = ($items)(self);
                Ok(ValueWire { ty, data: DataWire::Collection(items.into_iter().map(|v| v.into_typed(*element,types,budget)).collect::<Result<_>>()?) })
            }
        }
    }
}
#[derive(Clone, Debug)]
pub struct Set<T>(pub Vec<T>);
#[derive(Clone, Debug)]
pub struct OrderedSet<T>(pub Vec<T>);
#[derive(Clone, Debug)]
pub struct Bag<T>(pub Vec<T>);
collection!(Vec, Sequence, |v| v, |v| v);
collection!(Set, Set, |v: Set<T>| v.0, Set);
collection!(OrderedSet, OrderedSet, |v: OrderedSet<T>| v.0, OrderedSet);
collection!(Bag, Bag, |v: Bag<T>| v.0, Bag);

impl<T: NativeValue> NativeValue for Option<T> {
    fn matches(node: &TypeNode) -> bool {
        matches!(node, TypeNode::Option(_))
    }
    fn matches_type(types: &[TypeNode], ty: TypeId) -> bool {
        matches!(types.get(ty.0 as usize),Some(TypeNode::Option(element)) if T::matches_type(types,*element))
    }
    fn from_wire(_: &ValueWire) -> Result<Self> {
        Err(unsupported())
    }
    fn into_wire(self, _: TypeId) -> Result<ValueWire> {
        Err(unsupported())
    }
    fn from_typed(value: &ValueWire, types: &[TypeNode], budget: &Budget) -> Result<Self> {
        budget.charge(1)?;
        if !Self::matches_type(types, value.ty) {
            return Err(Error::new(ErrorKind::Arguments, "Option element differs"));
        }
        match &value.data {
            DataWire::None => Ok(None),
            DataWire::Some(v) => T::from_typed(v, types, budget).map(Some),
            _ => Err(Error::new(ErrorKind::Arguments, "Expected Option")),
        }
    }
    fn into_typed(self, ty: TypeId, types: &[TypeNode], budget: &Budget) -> Result<ValueWire> {
        budget.charge(1)?;
        if !Self::matches_type(types, ty) {
            return Err(Error::new(ErrorKind::Result, "Option element differs"));
        }
        let Some(TypeNode::Option(element)) = types.get(ty.0 as usize) else {
            return Err(unsupported());
        };
        Ok(ValueWire {
            ty,
            data: match self {
                None => DataWire::None,
                Some(v) => DataWire::Some(Box::new(v.into_typed(*element, types, budget)?)),
            },
        })
    }
}

/// Opaque representation for enums/custom datatypes. Rust code may inspect the
/// payload, but returning it as another nominal type is an explicit error.
#[derive(Clone, Debug)]
pub struct NominalValue {
    identity: TypeNode,
    data: DataWire,
}
impl NominalValue {
    pub fn identity(&self) -> &TypeNode {
        &self.identity
    }
    pub fn data(&self) -> &DataWire {
        &self.data
    }
}
impl NativeValue for NominalValue {
    fn matches(node: &TypeNode) -> bool {
        matches!(
            node,
            TypeNode::Enum(_) | TypeNode::Datatype(_) | TypeNode::AppliedDatatype(_)
        )
    }
    fn from_wire(_: &ValueWire) -> Result<Self> {
        Err(unsupported())
    }
    fn into_wire(self, _: TypeId) -> Result<ValueWire> {
        Err(unsupported())
    }
    fn from_typed(value: &ValueWire, types: &[TypeNode], budget: &Budget) -> Result<Self> {
        budget.charge(1)?;
        let identity = types
            .get(value.ty.0 as usize)
            .filter(|n| Self::matches(n))
            .ok_or_else(unsupported)?
            .clone();
        Ok(Self {
            identity,
            data: value.data.clone(),
        })
    }
    fn into_typed(self, ty: TypeId, types: &[TypeNode], budget: &Budget) -> Result<ValueWire> {
        budget.charge(1)?;
        if types.get(ty.0 as usize) != Some(&self.identity) {
            return Err(Error::new(
                ErrorKind::Result,
                "Nominal identity differs from result contract",
            ));
        }
        Ok(ValueWire {
            ty,
            data: self.data,
        })
    }
}
