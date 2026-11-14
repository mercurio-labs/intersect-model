//! Borrowed graph views; owned JSON and Element values require explicit export.
use super::{FeatureId, ObjectHandle, StorageResult, TypeDescriptor, TypeId};
use crate::core::{
    frozen::{FrozenAtom, FrozenItems, FrozenObject, FrozenValue},
    model::Element,
};
use serde::{Serialize, Serializer, ser::SerializeSeq};
use serde_json::Value;
use std::collections::btree_map;

#[cfg(test)]
#[path = "view_projection_tests.rs"]
mod projection_tests;

#[derive(Clone, Copy)]
enum ObjectData<'a> {
    Element(&'a Element),
    Frozen(FrozenObject<'a>),
}
#[derive(Clone, Copy)]
pub struct ObjectView<'a> {
    data: ObjectData<'a>,
}
impl<'a> ObjectView<'a> {
    pub fn from_element(element: &'a Element) -> Self {
        Self {
            data: ObjectData::Element(element),
        }
    }
    pub(in crate::core) fn frozen(view: FrozenObject<'a>) -> Self {
        Self {
            data: ObjectData::Frozen(view),
        }
    }
    pub fn id(&self) -> &'a str {
        match self.data {
            ObjectData::Element(e) => &e.id,
            ObjectData::Frozen(view) => view.id(),
        }
    }
    pub fn class(&self) -> &'a str {
        match self.data {
            ObjectData::Element(e) => &e.class,
            ObjectData::Frozen(view) => view.class(),
        }
    }
    pub fn type_identity(&self) -> &'a str {
        match self.data {
            ObjectData::Element(e) => &e.type_identity,
            ObjectData::Frozen(view) => view.type_identity(),
        }
    }
    pub fn resource(&self) -> &'a str {
        match self.data {
            ObjectData::Element(e) => &e.resource,
            ObjectData::Frozen(view) => view.resource(),
        }
    }
    pub fn start(&self) -> usize {
        match self.data {
            ObjectData::Element(e) => e.start,
            ObjectData::Frozen(view) => view.start(),
        }
    }
    pub fn end(&self) -> usize {
        match self.data {
            ObjectData::Element(e) => e.end,
            ObjectData::Frozen(view) => view.end(),
        }
    }
    /// The bound schema's scoped type handle, when descriptor context exists.
    /// Element views (including current SQLite reads) return None; this
    /// does not mean the object has no type identity.
    pub fn type_id(&self) -> Option<TypeId> {
        self.descriptor().map(TypeDescriptor::id)
    }
    fn descriptor(&self) -> Option<&'a TypeDescriptor> {
        match self.data {
            ObjectData::Element(_) => None,
            ObjectData::Frozen(view) => view.descriptor(),
        }
    }
    pub fn value(&self, name: &str) -> Option<ValueView<'a>> {
        match self.data {
            ObjectData::Element(element) => element.values.get(name).map(ValueView::json),
            ObjectData::Frozen(view) => view.value(name),
        }
    }
    /// Schema-scoped typed access; foreign feature handles are rejected before
    /// interpreting their numeric slot. Requires a canonical schema descriptor;
    /// Element-backed views return an error. Use value(name) for portable reads.
    pub fn value_by_feature(&self, feature: &FeatureId) -> StorageResult<Option<ValueView<'a>>> {
        let descriptor = self
            .descriptor()
            .ok_or("Object has no bound schema descriptor")?;
        let feature = descriptor.feature_descriptor(feature)?;
        Ok(self.value(feature.name()))
    }
    /// Field presence, not Ecore unsettable assignment history. Schema-created
    /// defaults are present; a raw omitted field remains absent.
    pub fn is_set(&self, name: &str) -> bool {
        self.value(name).is_some()
    }
    /// Raw explicit assignment evidence; this does not interpret feature semantics.
    pub fn initialization(
        &self,
    ) -> Option<&'a intersect_model::definitions::initialization::InitializationReceipt> {
        match self.data {
            ObjectData::Element(element) => element.initialization.as_ref(),
            ObjectData::Frozen(view) => view.initialization(),
        }
    }
    pub fn assignment_state(&self) -> Option<&'a crate::core::model::AssignmentState> {
        match self.data {
            ObjectData::Element(element) => element.assignment_state.as_ref(),
            ObjectData::Frozen(view) => view.assignment_state(),
        }
    }
    pub fn fields(&self) -> impl Iterator<Item = (&'a str, ValueView<'a>)> {
        match self.data {
            ObjectData::Element(element) => Fields::Element(element.values.iter()),
            ObjectData::Frozen(view) => Fields::Frozen(view, 0),
        }
    }
    pub fn to_element(&self) -> Element {
        Element {
            implicit: match self.data {
                ObjectData::Element(element) => element.implicit,
                ObjectData::Frozen(view) => view.implicit(),
            },
            initialization: self.initialization().cloned(),
            assignment_state: self.assignment_state().cloned(),
            id: self.id().into(),
            class: self.class().into(),
            type_identity: self.type_identity().into(),
            resource: self.resource().into(),
            start: self.start(),
            end: self.end(),
            values: self
                .fields()
                .map(|(name, value)| (name.into(), value.to_json()))
                .collect(),
        }
    }
}
enum Fields<'a> {
    Element(btree_map::Iter<'a, String, Value>),
    Frozen(FrozenObject<'a>, usize),
}
impl<'a> Iterator for Fields<'a> {
    type Item = (&'a str, ValueView<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Element(fields) => fields
                .next()
                .map(|(name, value)| (name.as_str(), ValueView::json(value))),
            Self::Frozen(view, index) => view.next_field(index),
        }
    }
}

/// Resumable cursor over stored field names. Keep the object immutable.
#[derive(Default)]
pub(super) struct FieldNames {
    last: Option<String>,
    index: usize,
    at_end: bool,
}
impl FieldNames {
    /// The first empty step marks the phase transition.
    pub(super) fn advance<'a>(&mut self, view: ObjectView<'a>) -> (Option<&'a str>, bool) {
        if self.at_end {
            return (None, true);
        }
        let name = match view.data {
            ObjectData::Element(element) => {
                use std::ops::Bound::{Excluded, Unbounded};
                let next = match self.last.as_deref() {
                    Some(last) => element
                        .values
                        .range::<str, _>((Excluded(last), Unbounded))
                        .next(),
                    None => element.values.iter().next(),
                };
                let name = next.map(|(name, _)| name.as_str());
                self.last = name.map(str::to_owned);
                name
            }
            ObjectData::Frozen(view) => view.next_field(&mut self.index).map(|(name, _)| name),
        };
        self.at_end = name.is_none();
        (name, false)
    }
}
#[derive(Clone, Copy)]
enum ValueData<'a> {
    Json(&'a Value),
    Frozen(FrozenValue<'a>),
    Null,
}
#[derive(Clone, Copy)]
pub struct ValueView<'a> {
    data: ValueData<'a>,
}

impl Serialize for ValueView<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.data {
            ValueData::Null => serializer.serialize_unit(),
            ValueData::Json(value) => value.serialize(serializer),
            ValueData::Frozen(value) => match value.atom() {
                FrozenAtom::Null => serializer.serialize_unit(),
                FrozenAtom::Bool(value) => serializer.serialize_bool(value),
                FrozenAtom::Int(value) => serializer.serialize_i64(value),
                FrozenAtom::Text(value) | FrozenAtom::Identity(value) => {
                    serializer.serialize_str(value)
                }
                FrozenAtom::Pending(spelling) => serialize_pending(spelling, serializer),
                FrozenAtom::Json(value) => value.serialize(serializer),
                FrozenAtom::Many(items) => CollectionView {
                    data: CollectionData::Frozen(items),
                }
                .serialize(serializer),
            },
        }
    }
}
fn serialize_pending<S: Serializer>(spelling: &str, serializer: S) -> Result<S::Ok, S::Error> {
    struct Unresolved<'a>(&'a str);
    impl std::fmt::Display for Unresolved<'_> {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "?{}", self.0)
        }
    }
    serializer.collect_str(&Unresolved(spelling))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReferenceView<'a> {
    Resolved(ObjectHandle),
    /// A durable identity whose resource/object is not currently resident. Loading
    /// that identity explicitly rebinds identity references to the new incarnation.
    Unavailable {
        identity: &'a str,
    },
    /// Original lexical spelling; language-owned linking selects its identity.
    Unresolved {
        spelling: &'a str,
    },
}
impl<'a> ValueView<'a> {
    pub(in crate::core) fn frozen(value: FrozenValue<'a>) -> Self {
        Self {
            data: ValueData::Frozen(value),
        }
    }
    pub fn json(value: &'a Value) -> Self {
        Self {
            data: ValueData::Json(value),
        }
    }
    pub fn null() -> Self {
        Self {
            data: ValueData::Null,
        }
    }
    pub fn as_str(self) -> Option<&'a str> {
        match self.data {
            ValueData::Json(value) => value.as_str(),
            ValueData::Frozen(value) => match value.atom() {
                FrozenAtom::Text(value) | FrozenAtom::Identity(value) => Some(value),
                FrozenAtom::Json(value) => value.as_str(),
                _ => None,
            },
            _ => None,
        }
    }
    pub fn as_i64(self) -> Option<i64> {
        match self.data {
            ValueData::Json(value) => value.as_i64(),
            ValueData::Frozen(value) => match value.atom() {
                FrozenAtom::Int(value) => Some(value),
                FrozenAtom::Json(value) => value.as_i64(),
                _ => None,
            },
            _ => None,
        }
    }
    pub fn as_bool(self) -> Option<bool> {
        match self.data {
            ValueData::Json(value) => value.as_bool(),
            ValueData::Frozen(value) => match value.atom() {
                FrozenAtom::Bool(value) => Some(value),
                FrozenAtom::Json(value) => value.as_bool(),
                _ => None,
            },
            _ => None,
        }
    }
    pub fn is_string(self) -> bool {
        self.as_str().is_some()
    }
    pub fn is_boolean(self) -> bool {
        self.as_bool().is_some()
    }
    pub fn is_null(self) -> bool {
        match self.data {
            ValueData::Null => true,
            ValueData::Json(value) => value.is_null(),
            ValueData::Frozen(value) => match value.atom() {
                FrozenAtom::Null => true,
                FrozenAtom::Json(value) => value.is_null(),
                _ => false,
            },
        }
    }
    pub fn is_array(self) -> bool {
        self.collection().is_some()
    }
    /// Distinct reference state when the backing view provides canonical
    /// reference context. JSON Element-backed views return None even for a
    /// schema reference field; None is not proof that a value is an attribute.
    pub fn reference(self) -> Option<ReferenceView<'a>> {
        match self.data {
            ValueData::Frozen(value) => value.reference(),
            _ => None,
        }
    }
    pub fn collection(self) -> Option<CollectionView<'a>> {
        match self.data {
            ValueData::Frozen(value) => match value.atom() {
                FrozenAtom::Many(items) => Some(CollectionView {
                    data: CollectionData::Frozen(items),
                }),
                FrozenAtom::Json(value) => Self::json(value).collection(),
                _ => None,
            },
            ValueData::Json(Value::Array(values)) => Some(CollectionView {
                data: CollectionData::Json(values),
            }),
            _ => None,
        }
    }
    /// Compare with the Element-backed JSON spelling without copying the value.
    /// In particular, unchanged large strings and collections stay borrowed.
    pub(crate) fn equals_json(self, other: &Value) -> bool {
        match self.data {
            ValueData::Null => other.is_null(),
            ValueData::Json(value) => value == other,
            ValueData::Frozen(value) => match value.atom() {
                FrozenAtom::Null => other.is_null(),
                FrozenAtom::Bool(value) => other.as_bool() == Some(value),
                FrozenAtom::Int(value) => other.as_i64() == Some(value),
                FrozenAtom::Text(value) | FrozenAtom::Identity(value) => {
                    other.as_str() == Some(value)
                }
                FrozenAtom::Pending(value) => {
                    other.as_str().and_then(|s| s.strip_prefix('?')) == Some(value)
                }
                FrozenAtom::Json(value) => value == other,
                FrozenAtom::Many(items) => CollectionView {
                    data: CollectionData::Frozen(items),
                }
                .equals_json(other),
            },
        }
    }
    pub fn to_json(self) -> Value {
        match self.data {
            ValueData::Null => Value::Null,
            ValueData::Json(value) => value.clone(),
            ValueData::Frozen(value) => value.to_json(),
        }
    }
    /// Reserve the complete borrowed projection before making its owned copy.
    /// This is the shared cumulative logical policy, not physical heap size.
    /// Measurement and copying remain atomic; unobserved exports have no ceiling.
    pub fn to_json_observed(
        self,
        observation: &intersect_model::definitions::conversion::InvocationContext<'_>,
    ) -> StorageResult<Value> {
        if observation.has_observer() {
            intersect_model::definitions::retained_storage::reserve_serialized(&self, observation)?;
        }
        Ok(self.to_json())
    }
}
#[derive(Clone, Copy)]
enum CollectionData<'a> {
    Json(&'a [Value]),
    Frozen(FrozenItems<'a>),
}
#[derive(Clone, Copy)]
pub struct CollectionView<'a> {
    data: CollectionData<'a>,
}
impl Serialize for CollectionView<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.len()))?;
        for value in self.iter() {
            sequence.serialize_element(&value)?;
        }
        sequence.end()
    }
}
impl<'a> CollectionView<'a> {
    pub fn len(self) -> usize {
        match self.data {
            CollectionData::Json(v) => v.len(),
            CollectionData::Frozen(items) => items.len(),
        }
    }
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
    pub fn get(self, index: usize) -> Option<ValueView<'a>> {
        match self.data {
            CollectionData::Json(v) => v.get(index).map(ValueView::json),
            CollectionData::Frozen(items) => items.get(index).map(ValueView::frozen),
        }
    }
    fn equals_json(self, other: &Value) -> bool {
        other.as_array().is_some_and(|items| {
            self.len() == items.len()
                && self
                    .iter()
                    .zip(items)
                    .all(|(left, right)| left.equals_json(right))
        })
    }
    pub fn iter(self) -> impl ExactSizeIterator<Item = ValueView<'a>> {
        CollectionIter {
            collection: self,
            range: 0..self.len(),
        }
    }
}
struct CollectionIter<'a> {
    collection: CollectionView<'a>,
    range: std::ops::Range<usize>,
}
impl<'a> Iterator for CollectionIter<'a> {
    type Item = ValueView<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        self.range
            .next()
            .and_then(|index| self.collection.get(index))
    }
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        self.range
            .nth(n)
            .and_then(|index| self.collection.get(index))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.range.size_hint()
    }
}
impl ExactSizeIterator for CollectionIter<'_> {}
