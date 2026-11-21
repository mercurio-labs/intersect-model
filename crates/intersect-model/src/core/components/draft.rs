//! Explicit construction state for a mounted project. Raw construction can
//! mutate only the project arena; no checked model is issued until admission.
use super::*;
use crate::{
    core::frozen::{
        ClassId, DefaultItem, FeatureId, FrozenAtom, FrozenObject, ImplicitObject, ImplicitValue,
        Issue, LinkError, LinkValue, ObjId,
    },
    frozen_scopes::RcoreScopes,
    rcore_fast::{FastMemo, FastRead},
};

/// A project's construction graph with immutable dependencies. Pending references
/// are permitted here and do not confer checked-model validity.
pub struct MountedDraft {
    model: MountedModel,
}

impl MountedDraft {
    pub fn new(
        context: Arc<SharedSchema>,
        images: Vec<Arc<ComponentImage>>,
    ) -> StorageResult<Self> {
        Ok(Self {
            model: MountedModel::new(context, images)?,
        })
    }

    /// Explicit construction capability. This cannot reach a dependency arena.
    /// The parser is responsible for transactional resource construction.
    pub fn project_store_mut(&mut self) -> &mut FrozenStore {
        &mut self.model.store.project
    }
    pub fn reader(&self) -> &dyn ModelRead {
        &self.model
    }
    pub fn schema(&self) -> &Schema {
        self.model.schema()
    }

    fn check_project_ids(&self) -> StorageResult<()> {
        for (_, id) in self.model.store.project.id_page(None, usize::MAX)? {
            self.model.store.author_project(&id)?;
        }
        for uri in self.model.store.project.resource_records()?.keys() {
            self.model.store.author_project(uri)?;
        }
        Ok(())
    }

    /// Restore reciprocal endpoints of resolved, project-owned assignments using
    /// the canonical setter. Pending lexical assignments are left for the linker.
    pub fn reconcile_opposites(&mut self) -> StorageResult<()> {
        self.check_project_ids()?;
        let mut writes = Vec::new();
        for (_, id) in self.model.store.project.id_page(None, usize::MAX)? {
            self.model.inspect(&id, &mut |object| {
                for (name, field) in self.schema().features(object.class())? {
                    if field.opposite.is_none() {
                        continue;
                    }
                    if let Some(value) = object.value(&name) {
                        let atoms = value
                            .collection()
                            .map(|c| c.iter().collect::<Vec<_>>())
                            .unwrap_or_else(|| vec![value]);
                        if atoms.is_empty()
                            || atoms.iter().any(|v| {
                                matches!(v.reference(), Some(ReferenceView::Unresolved { .. }))
                            })
                        {
                            continue;
                        }
                        let value = value.to_json();
                        if !value.is_null() {
                            writes.push((id.clone(), name, value));
                        }
                    }
                }
                Ok(())
            })?;
        }
        let context = self.model.store.context.clone();
        storage::atomic(&mut self.model.store, |store| {
            for (id, name, value) in writes {
                storage::set_with_opposite(store, context.schema(), &id, &name, value)?;
            }
            Ok(())
        })
    }

    /// None means a source scope read another pending reference. The caller can
    /// retry after resolving other references; no global-name fallback occurs.
    pub fn scope_candidates(
        &self,
        scopes: &RcoreScopes<'_>,
        owner: &str,
        feature: &str,
        spelling: &str,
    ) -> StorageResult<Option<Vec<String>>> {
        self.check_project_ids()?;
        scopes.check_mounted_schema(&self.model.store.context.metamodel)?;
        let read = ReadIndex::new(&self.model.store)?;
        let object = read
            .lookup
            .get(owner)
            .copied()
            .ok_or("Missing reference owner")?;
        let class = read.class(object).ok_or("Missing reference owner class")?;
        let feature = read
            .metamodel()
            .feature(class, feature)
            .ok_or("Missing scope feature")?;
        scopes
            .mounted_candidates(&read, &read.memo, object, feature, spelling)?
            .map(|found| {
                found
                    .into_iter()
                    .map(|obj| {
                        read.identity(obj)
                            .map(str::to_owned)
                            .ok_or_else(|| "Missing scope candidate".into())
                    })
                    .collect::<StorageResult<_>>()
            })
            .transpose()
    }

    /// Bind one parsed project reference. Checked final admission validates its
    /// target type and reciprocal relationships after the complete linking pass.
    pub fn resolve_reference(
        &mut self,
        owner: &str,
        feature: &str,
        index: Option<usize>,
        target: &str,
    ) -> StorageResult<()> {
        self.model.store.author_project(owner)?;
        if self.model.class(target)?.is_none() {
            return Err("Missing component reference target".into());
        }
        storage::atomic(&mut self.model.store.project, |project| {
            project.resolve_reference(owner, feature, index, target)
        })
    }

    /// Execute the language's existing verified implicit/default programs over
    /// borrowed mounted data. Only newly constructed project objects are changed.
    pub fn complete(&mut self, scopes: &RcoreScopes<'_>) -> StorageResult<()> {
        self.check_project_ids()?;
        scopes.check_mounted_schema(&self.model.store.context.metamodel)?;
        let (ids, additions) = {
            let read = ReadIndex::new(&self.model.store)?;
            let mut additions = Vec::new();
            for owner in read.project_objects() {
                let class = read.class(owner).ok_or("Missing implicit receiver class")?;
                for feature in scopes.mounted_implicit_features() {
                    if read.metamodel().redefined_by(class, feature).is_some()
                        || read.metamodel().slot(class, feature).is_none()
                    {
                        continue;
                    }
                    let values = scopes.mounted_implicits(&read, &read.memo, owner, feature)?;
                    if !values.is_empty() {
                        additions.push((owner, feature, values));
                    }
                }
            }
            (read.ids.clone(), additions)
        };
        let context = self.model.store.context.clone();
        storage::atomic(&mut self.model.store, |store| {
            for (owner, feature, objects) in additions {
                let owner = ids.get(owner.index()).ok_or("Missing implicit owner")?;
                let name = context.metamodel.feature_name(feature);
                let mut targets = Vec::new();
                for (position, object) in objects.into_iter().enumerate() {
                    let id = format!("{owner}/implicit/{name}/{position}");
                    materialize(store, &context, &ids, owner, &id, object)?;
                    targets.push(Value::String(id));
                }
                let many = context
                    .metamodel
                    .feature_info(feature)
                    .ok_or("Missing implicit feature")?
                    .many;
                let value = if many {
                    let mut current = store
                        .value(owner, name)?
                        .and_then(|v| v.as_array().cloned())
                        .unwrap_or_default();
                    current.extend(targets);
                    Value::Array(current)
                } else {
                    scalar(targets)?
                };
                store.project.set_resolved_value(owner, name, Some(value))?;
            }
            Ok(())
        })?;
        self.reconcile_opposites()?;
        let defaults = {
            let read = ReadIndex::new(&self.model.store)?;
            let mut writes = Vec::new();
            for owner in read.project_objects() {
                let id = read.identity(owner).ok_or("Missing default receiver")?;
                let class = read.class(owner).ok_or("Missing default receiver class")?;
                for feature in scopes.mounted_default_features() {
                    if read.metamodel().redefined_by(class, feature).is_some() {
                        continue;
                    }
                    let Some((unassigned, empty, _)) =
                        read.store.project.component_slot_state(id, feature)
                    else {
                        continue;
                    };
                    if !unassigned && !empty {
                        continue;
                    }
                    let values = scopes.mounted_defaults(&read, &read.memo, owner, feature)?;
                    if values.is_empty() && empty {
                        continue;
                    }
                    let values = values
                        .into_iter()
                        .map(|v| match v {
                            DefaultItem::Value(v) => Ok(v),
                            DefaultItem::Object(obj) => read
                                .identity(obj)
                                .map(|id| Value::String(id.into()))
                                .ok_or_else(|| "Missing default target".into()),
                        })
                        .collect::<StorageResult<Vec<_>>>()?;
                    let many = read
                        .metamodel()
                        .feature_info(feature)
                        .ok_or("Missing default feature")?
                        .many;
                    writes.push((
                        id.to_owned(),
                        read.metamodel().feature_name(feature).to_owned(),
                        if many {
                            Value::Array(values)
                        } else {
                            scalar(values)?
                        },
                    ));
                }
            }
            writes
        };
        storage::atomic(&mut self.model.store.project, |project| {
            for (id, name, value) in defaults {
                project.set_resolved_value(&id, &name, Some(value))?;
            }
            Ok(())
        })?;
        self.reconcile_opposites()
    }

    pub fn issues(&self, scopes: &RcoreScopes<'_>) -> StorageResult<Vec<(String, Issue)>> {
        scopes.check_mounted_schema(&self.model.store.context.metamodel)?;
        let read = ReadIndex::new(&self.model.store)?;
        let mut issues = Vec::new();
        for owner in read.project_objects() {
            let id = read.identity(owner).ok_or("Missing validation receiver")?;
            issues.extend(
                scopes
                    .mounted_issues(&read, &read.memo, owner)?
                    .into_iter()
                    .map(|issue| (id.to_owned(), issue)),
            );
        }
        Ok(issues)
    }

    pub fn finish(self) -> StorageResult<MountedModel> {
        self.check_project_ids()?;
        self.model.validate()?;
        Ok(self.model)
    }
}

fn scalar(mut values: Vec<Value>) -> StorageResult<Value> {
    match values.len() {
        0 => Ok(Value::Null),
        1 => values.pop().ok_or_else(|| "Missing scalar value".into()),
        _ => Err("A scalar completion produced multiple values".into()),
    }
}

fn materialize(
    store: &mut MountedStore,
    context: &SharedSchema,
    ids: &[Arc<str>],
    owner: &str,
    id: &str,
    object: ImplicitObject,
) -> StorageResult<()> {
    let parent = store.get(owner)?.ok_or("Missing implicit parent")?;
    let class = context.metamodel.class_key(object.class);
    store.project.create_element(
        context.schema(),
        id,
        class,
        &parent.resource,
        parent.end,
        parent.end,
    )?;
    for (feature, values) in object.values {
        let name = context.metamodel.feature_name(feature);
        let mut atoms = Vec::new();
        for (position, value) in values.into_iter().enumerate() {
            atoms.push(match value {
                ImplicitValue::Value(value) => value,
                ImplicitValue::Ref(object) => Value::String(
                    ids.get(object.index())
                        .ok_or("Missing implicit reference")?
                        .to_string(),
                ),
                ImplicitValue::Object(child) => {
                    let child_id = format!("{id}/{name}/{position}");
                    materialize(store, context, ids, id, &child_id, child)?;
                    Value::String(child_id)
                }
            });
        }
        let many = context
            .metamodel
            .feature_info(feature)
            .ok_or("Missing implicit member")?
            .many;
        store.project.set_resolved_value(
            id,
            name,
            Some(if many {
                Value::Array(atoms)
            } else {
                scalar(atoms)?
            }),
        )?;
    }
    let mut element = store.project.get(id)?.ok_or("Missing implicit object")?;
    element.implicit = true;
    store.project.put(element)
}

impl MountedStore {
    fn borrow_object(&self, id: &str) -> StorageResult<Option<FrozenObject<'_>>> {
        match self.locate(id) {
            Some((image, local)) => image.arena.borrow_mounted(
                local,
                ReadContext {
                    store: self,
                    image: Some(image),
                    overlays: true,
                },
            ),
            None => self.project.borrow_mounted(
                id,
                ReadContext {
                    store: self,
                    image: None,
                    overlays: true,
                },
            ),
        }
    }

    fn slot_state(&self, id: &str, feature: FeatureId) -> Option<(bool, bool, bool)> {
        match self.locate(id) {
            Some((image, local)) => image.arena.component_slot_state(local, feature),
            None => self.project.component_slot_state(id, feature),
        }
    }
}

// Indexes retain identities and containment topology, never copied library
// attributes or reference payloads. All program reads borrow the actual arenas.
// Immutable native containment is compiled once per image, not once per scope
// request. Iterating stored features and their targets preserves traversal order.
pub(super) struct ComponentTopology {
    pub(super) ids: Vec<Arc<str>>,
    classes: Vec<ClassId>,
    children: Vec<Vec<usize>>,
}

impl ComponentTopology {
    pub(super) fn new(model: &FrozenModel, key: &str) -> StorageResult<Self> {
        let mut ids = Vec::with_capacity(model.len());
        let mut classes = Vec::with_capacity(model.len());
        let mut children = Vec::with_capacity(model.len());
        for object in model.objects() {
            ids.push(Arc::from(qualify(key, "object", object.identity())));
            classes.push(object.class());
            let info = model
                .metamodel()
                .class_info(object.class())
                .ok_or("Unknown component class")?;
            let mut targets = Vec::new();
            for feature in &info.stored {
                if model
                    .metamodel()
                    .definition(*feature)
                    .is_some_and(|field| field.containment)
                {
                    for child in object.refs(*feature) {
                        if !targets.contains(&child.id().index()) {
                            targets.push(child.id().index());
                        }
                    }
                }
            }
            children.push(targets);
        }
        Ok(Self {
            ids,
            classes,
            children,
        })
    }
}

struct ReadIndex<'a> {
    store: &'a MountedStore,
    ids: Vec<Arc<str>>,
    lookup: BTreeMap<Arc<str>, ObjId>,
    classes: Vec<ClassId>,
    parents: Vec<Option<ObjId>>,
    children: Vec<Vec<ObjId>>,
    roots: Vec<ObjId>,
    memo: FastMemo,
    project_count: usize,
}

/// A checked, immutable mounted model and its containment index. Object values,
/// references, and source coordinates are borrowed from the actual arenas.
/// Object IDs are local to this read model.
pub struct MountedReadModel {
    model: MountedModel,
    ids: Vec<Arc<str>>,
    lookup: BTreeMap<Arc<str>, ObjId>,
    parents: Vec<Option<ObjId>>,
}

impl MountedModel {
    /// Retain the accepted project and its component images for indexed reads.
    pub fn into_read_model(self) -> StorageResult<MountedReadModel> {
        let ReadIndex {
            ids, lookup, parents, ..
        } = ReadIndex::new(&self.store)?;
        Ok(MountedReadModel {
            model: self,
            ids,
            lookup,
            parents,
        })
    }
}

impl MountedReadModel {
    pub fn model(&self) -> &MountedModel {
        &self.model
    }

    pub fn metamodel(&self) -> &Metamodel {
        &self.model.store.context.metamodel
    }

    pub fn identities(&self) -> impl ExactSizeIterator<Item = &str> {
        self.ids.iter().map(AsRef::as_ref)
    }

    pub fn identity(&self, object: ObjId) -> Option<&str> {
        self.ids.get(object.index()).map(AsRef::as_ref)
    }

    pub fn lookup(&self, identity: &str) -> Option<ObjId> {
        self.lookup.get(identity).copied()
    }

    pub fn container(&self, object: ObjId) -> Option<ObjId> {
        self.parents.get(object.index()).copied().flatten()
    }

    pub fn object(&self, object: ObjId) -> StorageResult<Option<crate::core::storage::ObjectView<'_>>> {
        let Some(identity) = self.identity(object) else {
            return Ok(None);
        };
        Ok(self
            .model
            .store
            .borrow_object(identity)?
            .map(ObjectView::frozen))
    }
}

impl<'a> ReadIndex<'a> {
    fn new(store: &'a MountedStore) -> StorageResult<Self> {
        let mut ids: Vec<Arc<str>> = store
            .project
            .id_page(None, usize::MAX)?
            .into_iter()
            .map(|(_, id)| Arc::from(id))
            .collect();
        let project_count = ids.len();
        for image in &store.images {
            ids.extend(image.topology.ids.iter().cloned());
        }
        let lookup = ids
            .iter()
            .enumerate()
            .map(|(i, id)| Ok((id.clone(), ObjId::from_index(i)?)))
            .collect::<StorageResult<BTreeMap<_, _>>>()?;
        let mut classes = ids[..project_count]
            .iter()
            .map(|id| {
                let class = store.class(id)?.ok_or("Missing mounted class")?;
                store
                    .context
                    .metamodel
                    .class(&class)
                    .ok_or_else(|| "Unknown mounted class".into())
            })
            .collect::<StorageResult<Vec<_>>>()?;
        for image in &store.images {
            classes.extend_from_slice(&image.topology.classes);
        }
        let mut read = Self {
            parents: vec![None; ids.len()],
            children: vec![vec![]; ids.len()],
            roots: vec![],
            ids,
            lookup,
            classes,
            store,
            memo: FastMemo::default(),
            project_count,
        };
        let mut offset = project_count;
        for image in &store.images {
            for (parent, children) in image.topology.children.iter().enumerate() {
                for child in children {
                    read.add_child(
                        ObjId::from_index(offset + parent)?,
                        ObjId::from_index(offset + child)?,
                    )?;
                }
            }
            offset += image.topology.ids.len();
        }
        // Only project containment changes. Its edges may target a component;
        // add_child still detects conflicting ownership across that boundary.
        read.scan_containment(0..project_count)?;
        read.roots = read
            .parents
            .iter()
            .enumerate()
            .filter(|(_, parent)| parent.is_none())
            .map(|(i, _)| ObjId::from_index(i))
            .collect::<StorageResult<_>>()?;
        read.roots
            .sort_by(|a, b| read.ids[a.index()].cmp(&read.ids[b.index()]));
        Ok(read)
    }

    fn scan_containment(&mut self, range: std::ops::Range<usize>) -> StorageResult<()> {
        for index in range {
            let object = ObjId::from_index(index)?;
            let class = self.classes[index];
            let Some(info) = self.metamodel().class_info(class) else {
                return Err("Unknown containment class".into());
            };
            let features = info.stored.to_vec();
            for feature in features {
                if !self
                    .metamodel()
                    .definition(feature)
                    .is_some_and(|field| field.containment)
                {
                    continue;
                }
                let count = match self.value(object, feature).map_err(|e| e.to_string())? {
                    Some(LinkValue::Many(len)) => len,
                    Some(LinkValue::Ref(target)) => {
                        self.add_child(object, target)?;
                        continue;
                    }
                    _ => continue,
                };
                for index in 0..count {
                    if let Some(LinkValue::Ref(target)) = self
                        .item(object, feature, index)
                        .map_err(|e| e.to_string())?
                    {
                        self.add_child(object, target)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn add_child(&mut self, parent: ObjId, child: ObjId) -> StorageResult<()> {
        let previous = self
            .parents
            .get_mut(child.index())
            .ok_or("Missing contained object")?;
        if previous.is_some_and(|previous| previous != parent) {
            return Err("Multiple containment owners".into());
        }
        *previous = Some(parent);
        let children = self
            .children
            .get_mut(parent.index())
            .ok_or("Missing containment owner")?;
        if !children.contains(&child) {
            children.push(child);
        }
        Ok(())
    }

    fn project_objects(&self) -> impl Iterator<Item = ObjId> + '_ {
        (0..self.project_count).filter_map(|i| ObjId::from_index(i).ok())
    }

    fn atom<'b>(
        &'b self,
        atom: FrozenAtom<'b>,
        unassigned: bool,
    ) -> Result<LinkValue<'b>, LinkError> {
        Ok(match atom {
            FrozenAtom::Null => LinkValue::Null,
            FrozenAtom::Bool(value) => LinkValue::Bool(value),
            FrozenAtom::Int(value) => LinkValue::Int(value),
            FrozenAtom::Text(value) => LinkValue::Str(value),
            FrozenAtom::Identity(id) => {
                LinkValue::Ref(*self.lookup.get(id).ok_or(LinkError::Unresolved)?)
            }
            FrozenAtom::Pending(_) => return Err(LinkError::Unresolved),
            FrozenAtom::Json(value) if unassigned => LinkValue::Default(value),
            FrozenAtom::Json(value) => LinkValue::Json(value),
            FrozenAtom::Many(items) => LinkValue::Many(items.len()),
        })
    }
}

impl FastRead for ReadIndex<'_> {
    fn metamodel(&self) -> &Metamodel {
        &self.store.context.metamodel
    }
    fn class(&self, object: ObjId) -> Option<ClassId> {
        self.classes.get(object.index()).copied()
    }
    fn identity(&self, object: ObjId) -> Option<&str> {
        self.ids.get(object.index()).map(AsRef::as_ref)
    }
    fn resource(&self, object: ObjId) -> Option<&str> {
        self.store
            .borrow_object(self.identity(object)?)
            .ok()??
            .resource()
            .into()
    }
    fn object_count(&self) -> usize {
        self.ids.len()
    }
    fn container(&self, object: ObjId) -> Option<ObjId> {
        self.parents.get(object.index()).copied().flatten()
    }
    fn children(&self, object: ObjId) -> &[ObjId] {
        self.children.get(object.index()).map_or(&[], Vec::as_slice)
    }
    fn roots(&self) -> &[ObjId] {
        &self.roots
    }
    fn value(&self, object: ObjId, feature: FeatureId) -> Result<Option<LinkValue<'_>>, LinkError> {
        let id = self.identity(object).ok_or(LinkError::Unresolved)?;
        let Some((unassigned, _, optional_null)) = self.store.slot_state(id, feature) else {
            return Ok(None);
        };
        if optional_null {
            return Ok(Some(LinkValue::Null));
        }
        let view = self
            .store
            .borrow_object(id)
            .map_err(|_| LinkError::Unresolved)?
            .ok_or(LinkError::Unresolved)?;
        let Some(value) = view.component_value(feature) else {
            return Ok(Some(LinkValue::Absent));
        };
        self.atom(value.component_atom(), unassigned).map(Some)
    }
    fn item(
        &self,
        object: ObjId,
        feature: FeatureId,
        index: usize,
    ) -> Result<Option<LinkValue<'_>>, LinkError> {
        let id = self.identity(object).ok_or(LinkError::Unresolved)?;
        let view = self
            .store
            .borrow_object(id)
            .map_err(|_| LinkError::Unresolved)?
            .ok_or(LinkError::Unresolved)?;
        let Some(value) = view
            .component_value(feature)
            .and_then(|v| v.collection())
            .and_then(|v| v.get(index))
        else {
            return Ok(None);
        };
        self.atom(value.component_atom(), false).map(Some)
    }
}

impl crate::frozen_scopes::IndexedScopeRead for ReadIndex<'_> {
    fn lookup(&self, identity: &str) -> Option<ObjId> {
        self.lookup.get(identity).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::components::validation_tests::fixture;
    use crate::frozen::{ById, ModelBuilder};
    use serde_json::json;

    fn compare_full_scan(model: &MountedModel) -> StorageResult<()> {
        let cached = ReadIndex::new(&model.store)?;
        let mut full = ReadIndex::new(&model.store)?;
        full.parents.fill(None);
        for children in &mut full.children {
            children.clear();
        }
        full.scan_containment(0..full.ids.len())?;
        assert_eq!(cached.parents, full.parents);
        assert_eq!(cached.children, full.children);
        for (index, id) in cached.ids.iter().enumerate() {
            assert_eq!(cached.lookup.get(id), Some(&ObjId::from_index(index)?));
            assert_eq!(
                cached.metamodel().class_key(cached.classes[index]),
                model.class(id)?.ok_or("Missing class")?
            );
            assert_eq!(
                cached.roots.contains(&ObjId::from_index(index)?),
                full.parents[index].is_none()
            );
        }
        assert_eq!(
            cached.project_objects().count(),
            model.project_object_count()?
        );
        Ok(())
    }

    #[test]
    fn component_topology_preserves_order_offsets_restore_and_project_changes() -> StorageResult<()>
    {
        let fixture = fixture::Fixture::new()?;
        let mm = fixture.context.metamodel();
        let class = mm.class(&fixture.node).ok_or("Node class")?;
        let children = mm.feature(class, "children").ok_or("Children feature")?;
        let mut builder = ModelBuilder::new(mm);
        let root = builder
            .create("root", class, "memory:/tree")
            .map_err(|e| e.to_string())?;
        let first = builder
            .create("first", class, "memory:/tree")
            .map_err(|e| e.to_string())?;
        let second = builder
            .create("second", class, "memory:/tree")
            .map_err(|e| e.to_string())?;
        builder
            .add_ref(root, children, second)
            .map_err(|e| e.to_string())?;
        builder
            .add_ref(root, children, first)
            .map_err(|e| e.to_string())?;
        let native = builder
            .link(&ById)
            .map_err(|e| format!("{e:?}"))?
            .validate()
            .map_err(|e| format!("{e:?}"))?;
        let second_image =
            ComponentImage::prepare(fixture.context.clone(), "urn:tree:second", native.clone())?;
        let (first_image, snapshot) = ComponentImage::prepare_with_snapshot(
            fixture.context.clone(),
            "urn:tree:first",
            native,
        )?;
        let first_image = ComponentImage::restore(
            fixture.context.clone(),
            &first_image.descriptor(),
            &snapshot,
        )?;
        let mut model = MountedModel::new(
            fixture.context.clone(),
            vec![first_image.clone(), second_image],
        )?;
        compare_full_scan(&model)?;
        let read = ReadIndex::new(&model.store)?;
        let root = read.lookup[first_image.object_id("root")?];
        let names: Vec<_> = read
            .children(root)
            .iter()
            .filter_map(|id| read.identity(*id))
            .collect();
        assert_eq!(
            names,
            vec![
                first_image.object_id("second")?,
                first_image.object_id("first")?
            ]
        );
        drop(read);
        model.create("project", &fixture.node, "memory:/project")?;
        model.create("child", &fixture.node, "memory:/project")?;
        model.set("project", "children", json!(["child"]))?;
        compare_full_scan(&model)?;
        let mut fork = model.fork()?;
        fork.set("project", "children", json!([]))?;
        compare_full_scan(&fork)?;
        compare_full_scan(&model)?;
        assert!(Arc::ptr_eq(&model.store.images[0], &fork.store.images[0]));
        let accepted = model.into_read_model()?;
        let mounted_id = first_image.object_id("first")?;
        let mounted = accepted.lookup(mounted_id).ok_or("Missing component child")?;
        let parent = accepted.container(mounted).ok_or("Missing component owner")?;
        assert_eq!(accepted.identity(parent), Some(first_image.object_id("root")?));
        let object = accepted.object(mounted)?.ok_or("Missing component object")?;
        assert_eq!(object.id(), mounted_id);
        assert_eq!(object.class(), fixture.node);
        let child = accepted.lookup("child").ok_or("Missing project child")?;
        assert_eq!(accepted.container(child), accepted.lookup("project"));
        assert!(accepted.model().shares_component_with(&fork, "urn:tree:first"));
        // Parsing may introduce an invalid cross-boundary owner. Cached
        // topology must still expose the component child's existing owner.
        storage::atomic(&mut fork.store.project, |store| {
            store.set_resolved_value(
                "project",
                "children",
                Some(json!([first_image.object_id("first")?])),
            )
        })?;
        assert!(ReadIndex::new(&fork.store).is_err());
        Ok(())
    }
}
