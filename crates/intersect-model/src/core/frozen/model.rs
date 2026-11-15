//! Arena model data and the immutable, check-free read API.
use super::{
    FrozenError,
    identity_index::IdentityIndex,
    intern::{FxHashMap, Interner, Sym},
    metamodel::{ClassId, FeatureId, FeatureKind, Metamodel},
};
use serde_json::Value;
use std::sync::{Arc, OnceLock};

/// Dense object index within one model.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjId(pub(super) u32);
impl ObjId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

const NONE: u32 = u32::MAX;

/// One stored feature value. 16 bytes; scalars never allocate.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) enum Slot {
    /// Never assigned: reads observe the schema default.
    Default,
    /// Imported element omitted this stored member; reads observe the default
    /// and export omits it, so interchange round trips stay exact.
    Absent,
    Null,
    Bool(bool),
    Int(i64),
    Str(Sym),
    Ref(ObjId),
    /// Index into `ModelData::lists`.
    Many(u32),
    /// Lossless fallback into `ModelData::json` for values with no compact form.
    Json(u32),
    /// Index into `ModelData::proxies`; only present before linking.
    Proxy(u32),
}

#[derive(Clone, Debug)]
pub(super) struct Header {
    pub class: ClassId,
    pub id: Sym,
    pub resource: u32,
    pub slots: u32,
    pub container: u32,
    pub containing: u32,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug)]
pub(super) struct Proxy {
    pub owner: ObjId,
    pub feature: FeatureId,
    pub spelling: Sym,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Carried {
    pub initialization: Option<intersect_model::definitions::initialization::InitializationReceipt>,
    pub assignment_state: Option<crate::core::model::AssignmentState>,
}

#[derive(Clone, Default)]
pub(super) struct ModelData {
    pub headers: Vec<Header>,
    pub slots: Vec<Slot>,
    pub lists: Vec<Vec<Slot>>,
    pub json: Vec<Value>,
    pub proxies: Vec<Proxy>,
    pub strings: Interner,
    pub by_id: IdentityIndex,
    pub resources: Vec<Sym>,
    pub resource_records: Vec<Option<crate::core::resources::ResourceRecord>>,
    /// Construction evidence carried through import and export unchanged. The
    /// frozen engine does not interpret or update it.
    pub carried: FxHashMap<ObjId, Box<Carried>>,
    /// Objects the language added (`bind implicit`); they have no source text.
    pub implicit: std::collections::BTreeSet<ObjId>,
}
impl ModelData {
    pub fn header(&self, obj: ObjId) -> Option<&Header> {
        self.headers.get(obj.index())
    }
    pub fn slot(&self, mm: &Metamodel, obj: ObjId, feature: FeatureId) -> Option<Slot> {
        let header = self.headers.get(obj.index())?;
        let slot = mm.read_slot(header.class, feature)?;
        self.slots.get(header.slots as usize + slot).copied()
    }
    /// No value: absent, null, an empty list, or a default that is null or empty.
    pub fn is_empty_slot(&self, mm: &Metamodel, feature: FeatureId, slot: Slot) -> bool {
        match slot {
            Slot::Absent | Slot::Null => true,
            Slot::Default => mm.definition(feature).is_none_or(|d| match &d.default {
                serde_json::Value::Null => true,
                serde_json::Value::Array(items) => items.is_empty(),
                _ => false,
            }),
            Slot::Many(list) => self.lists.get(list as usize).is_none_or(Vec::is_empty),
            _ => false,
        }
    }
    pub fn slot_mut(
        &mut self,
        mm: &Metamodel,
        obj: ObjId,
        feature: FeatureId,
    ) -> Result<&mut Slot, FrozenError> {
        let header = self
            .headers
            .get(obj.index())
            .ok_or(FrozenError::UnknownObject)?;
        let slot = mm
            .slot(header.class, mm.write_feature(header.class, feature))
            .ok_or_else(|| FrozenError::NotAFeature {
                class: mm.class_key(header.class).to_owned(),
                feature: mm.feature_name(feature).to_owned(),
            })?;
        self.slots
            .get_mut(header.slots as usize + slot)
            .ok_or(FrozenError::UnknownObject)
    }
    pub fn container(&self, obj: ObjId) -> Option<(ObjId, FeatureId)> {
        let header = self.headers.get(obj.index())?;
        (header.container != NONE)
            .then_some((ObjId(header.container), FeatureId(header.containing)))
    }
    /// Visit the canonical edge targets of one stored slot.
    pub fn slot_refs(&self, slot: Slot, visit: &mut impl FnMut(ObjId)) {
        match slot {
            Slot::Ref(target) => visit(target),
            Slot::Many(list) => {
                for item in self.lists.get(list as usize).map_or(&[][..], Vec::as_slice) {
                    if let Slot::Ref(target) = item {
                        visit(*target);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Facts about one validated model that a load on top of it
/// (`FrozenStore::from_model`) would otherwise recompute. They are pure
/// functions of the model's objects, so every clone of the model shares them.
#[derive(Default)]
pub(super) struct PreparedFacts {
    /// Whether every object passes the structural checks of its own values
    /// with no field delegated to the language (`LinkedModel::validate`).
    pub strict: OnceLock<bool>,
    /// The objects without a container, ordered by identity.
    pub roots: OnceLock<Vec<ObjId>>,
}
impl PreparedFacts {
    /// Facts of a model whose strict validity is already known.
    pub fn validated(strict: Option<bool>) -> Arc<Self> {
        let facts = Self::default();
        if let Some(strict) = strict {
            let _ = facts.strict.set(strict);
        }
        Arc::new(facts)
    }
}

/// A linked, structurally validated model. Constructible only through
/// [`super::LinkedModel::validate`] or [`Model::from_model`]; reads perform no
/// further checks.
#[derive(Clone)]
pub struct Model {
    pub(super) metamodel: Arc<Metamodel>,
    pub(super) data: ModelData,
    /// Objects below this index came from a prepared model the load started
    /// from (`FrozenStore::from_model`) and were checked when it was
    /// prepared; `check` visits only the objects after them.
    pub(super) prepared: usize,
    /// Facts about this model, for loads on top of it.
    pub(super) facts: Arc<PreparedFacts>,
    /// Facts about the prepared model this one was loaded on top of, when
    /// its objects below `prepared` are still that model's, in place.
    pub(super) base: Option<Arc<PreparedFacts>>,
}

impl Model {
    pub fn metamodel(&self) -> &Metamodel {
        &self.metamodel
    }
    pub fn len(&self) -> usize {
        self.data.headers.len()
    }
    pub fn is_empty(&self) -> bool {
        self.data.headers.is_empty()
    }
    /// Package origin of a resource; None also covers an unknown resource URI.
    pub fn resource_package(
        &self,
        uri: &str,
    ) -> Option<&crate::core::project_usages::PackageIdentity> {
        self.data
            .resources
            .iter()
            .zip(&self.data.resource_records)
            .find(|(symbol, _)| self.data.strings.resolve(**symbol) == uri)
            .and_then(|(_, record)| record.as_ref()?.package.as_ref())
    }
    /// Package-owned resource URIs in stable URI order.
    pub fn resources_of_package(
        &self,
        identity: &crate::core::project_usages::PackageIdentity,
    ) -> Vec<&str> {
        let mut uris: Vec<_> = self
            .data
            .resources
            .iter()
            .zip(&self.data.resource_records)
            .filter(|(_, record)| {
                record.as_ref().and_then(|record| record.package.as_ref()) == Some(identity)
            })
            .map(|(symbol, _)| self.data.strings.resolve(*symbol))
            .collect();
        uris.sort_unstable();
        uris
    }
    pub fn objects(&self) -> impl ExactSizeIterator<Item = Obj<'_>> + '_ {
        (0..self.data.headers.len()).map(|i| Obj {
            model: self,
            id: ObjId(i as u32),
        })
    }
    pub fn obj(&self, id: ObjId) -> Option<Obj<'_>> {
        (id.index() < self.data.headers.len()).then_some(Obj { model: self, id })
    }
    pub fn lookup(&self, identity: &str) -> Option<Obj<'_>> {
        let sym = self.data.strings.get(identity)?;
        self.data.by_id.get(&sym).map(|id| Obj {
            model: self,
            id: *id,
        })
    }
    /// Build the reverse-reference index in one O(N + E) pass.
    pub fn inverse(&self) -> Inverse {
        let n = self.data.headers.len();
        let mut counts = vec![0u32; n + 1];
        self.each_edge(|_, _, target| counts[target.index() + 1] += 1);
        for i in 0..n {
            counts[i + 1] += counts[i];
        }
        let mut cursor = counts.clone();
        let total = counts[n] as usize;
        let mut sources = vec![(ObjId(0), FeatureId(0)); total];
        self.each_edge(|source, feature, target| {
            let at = &mut cursor[target.index()];
            sources[*at as usize] = (source, feature);
            *at += 1;
        });
        Inverse {
            offsets: counts,
            sources,
        }
    }
    fn each_edge(&self, mut visit: impl FnMut(ObjId, FeatureId, ObjId)) {
        let mm = &self.metamodel;
        for (index, header) in self.data.headers.iter().enumerate() {
            let Some(class) = mm.classes.get(header.class.index()) else {
                continue;
            };
            for (slot, feature) in class.stored.iter().enumerate() {
                if mm.features[feature.index()].kind == FeatureKind::Attribute {
                    continue;
                }
                if let Some(value) = self.data.slots.get(header.slots as usize + slot) {
                    self.data.slot_refs(*value, &mut |target| {
                        visit(ObjId(index as u32), *feature, target)
                    });
                }
            }
        }
    }
}

/// Compressed reverse references: for each target, its (source, feature) edges.
pub struct Inverse {
    offsets: Vec<u32>,
    sources: Vec<(ObjId, FeatureId)>,
}
impl Inverse {
    pub fn referrers(&self, target: ObjId) -> &[(ObjId, FeatureId)] {
        let i = target.index();
        match (self.offsets.get(i), self.offsets.get(i + 1)) {
            (Some(a), Some(b)) => self.sources.get(*a as usize..*b as usize).unwrap_or(&[]),
            _ => &[],
        }
    }
    pub fn edge_count(&self) -> usize {
        self.sources.len()
    }
}

/// Borrowed object handle. Copy, two words, no reference counting.
#[derive(Copy, Clone)]
pub struct Obj<'m> {
    model: &'m Model,
    id: ObjId,
}

/// Borrowed view of one feature value.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ValueRef<'m> {
    Default(&'m Value),
    Null,
    Bool(bool),
    Int(i64),
    Str(&'m str),
    Ref(ObjId),
    Many(Items<'m>),
    Json(&'m Value),
}

/// Borrowed many-valued feature contents.
#[derive(Copy, Clone)]
pub struct Items<'m> {
    model: &'m Model,
    items: &'m [Slot],
}
impl<'m> Items<'m> {
    pub fn len(self) -> usize {
        self.items.len()
    }
    pub fn is_empty(self) -> bool {
        self.items.is_empty()
    }
    pub fn get(self, index: usize) -> Option<ValueRef<'m>> {
        self.items
            .get(index)
            .and_then(|slot| value_ref(self.model, None, *slot))
    }
    pub fn iter(self) -> impl Iterator<Item = ValueRef<'m>> + 'm {
        let model = self.model;
        self.items
            .iter()
            .filter_map(move |slot| value_ref(model, None, *slot))
    }
}
impl std::fmt::Debug for Items<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}
impl PartialEq for Items<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.model, other.model) && self.items == other.items
    }
}

fn value_ref(model: &Model, feature: Option<FeatureId>, slot: Slot) -> Option<ValueRef<'_>> {
    let data = &model.data;
    Some(match slot {
        Slot::Default | Slot::Absent => {
            ValueRef::Default(&model.metamodel.definition(feature?)?.default)
        }
        Slot::Null => ValueRef::Null,
        Slot::Bool(v) => ValueRef::Bool(v),
        Slot::Int(v) => ValueRef::Int(v),
        Slot::Str(s) => ValueRef::Str(data.strings.resolve(s)),
        Slot::Ref(o) => ValueRef::Ref(o),
        Slot::Many(l) => ValueRef::Many(Items {
            model,
            items: data.lists.get(l as usize)?.as_slice(),
        }),
        Slot::Json(j) => ValueRef::Json(data.json.get(j as usize)?),
        // A validated model holds no proxies.
        Slot::Proxy(_) => return None,
    })
}

impl<'m> Obj<'m> {
    pub fn id(self) -> ObjId {
        self.id
    }
    fn header(self) -> &'m Header {
        // Obj is only constructed for in-range ids of `model`.
        &self.model.data.headers[self.id.index()]
    }
    pub fn class(self) -> ClassId {
        self.header().class
    }
    pub fn class_key(self) -> &'m str {
        self.model.metamodel.class_key(self.header().class)
    }
    pub fn identity(self) -> &'m str {
        self.model.data.strings.resolve(self.header().id)
    }
    pub fn resource(self) -> &'m str {
        let data = &self.model.data;
        data.resources
            .get(self.header().resource as usize)
            .map_or("", |sym| data.strings.resolve(*sym))
    }
    pub fn span(self) -> (usize, usize) {
        (self.header().start, self.header().end)
    }
    pub fn container(self) -> Option<(Obj<'m>, FeatureId)> {
        self.model.data.container(self.id).map(|(id, f)| {
            (
                Obj {
                    model: self.model,
                    id,
                },
                f,
            )
        })
    }
    /// O(1) by FeatureId. None when the feature is not stored in this class.
    pub fn get(self, feature: FeatureId) -> Option<ValueRef<'m>> {
        let slot = self
            .model
            .data
            .slot(&self.model.metamodel, self.id, feature)?;
        value_ref(self.model, Some(feature), slot)
    }
    pub fn str(self, feature: FeatureId) -> Option<&'m str> {
        match self.get(feature)? {
            ValueRef::Str(s) => Some(s),
            ValueRef::Default(Value::String(s)) | ValueRef::Json(Value::String(s)) => Some(s),
            _ => None,
        }
    }
    /// Targets of a single or many-valued reference feature.
    pub fn refs(self, feature: FeatureId) -> impl Iterator<Item = Obj<'m>> + 'm {
        let model = self.model;
        let base = self.header().slots as usize;
        let items: &'m [Slot] = match model.metamodel.read_slot(self.class(), feature) {
            Some(slot) => match model.data.slots.get(base + slot) {
                Some(Slot::Many(list)) => model
                    .data
                    .lists
                    .get(*list as usize)
                    .map_or(&[], Vec::as_slice),
                Some(_) => model
                    .data
                    .slots
                    .get(base + slot..base + slot + 1)
                    .unwrap_or(&[]),
                None => &[],
            },
            None => &[],
        };
        items.iter().filter_map(move |item| match item {
            Slot::Ref(id) => Some(Obj { model, id: *id }),
            _ => None,
        })
    }
    pub fn feature(self, name: &str) -> Option<FeatureId> {
        self.model.metamodel.feature(self.class(), name)
    }
}
