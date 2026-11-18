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
    ids: &[String],
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
                        .clone(),
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
struct ReadIndex<'a> {
    store: &'a MountedStore,
    ids: Vec<String>,
    lookup: BTreeMap<String, ObjId>,
    classes: Vec<ClassId>,
    parents: Vec<Option<ObjId>>,
    children: Vec<Vec<ObjId>>,
    roots: Vec<ObjId>,
    memo: FastMemo,
}

impl<'a> ReadIndex<'a> {
    fn new(store: &'a MountedStore) -> StorageResult<Self> {
        let ids: Vec<String> = store
            .id_page(None, usize::MAX)?
            .into_iter()
            .map(|(_, id)| id)
            .collect();
        let lookup = ids
            .iter()
            .enumerate()
            .map(|(i, id)| Ok((id.clone(), ObjId::from_index(i)?)))
            .collect::<StorageResult<BTreeMap<_, _>>>()?;
        let classes = ids
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
        let mut read = Self {
            parents: vec![None; ids.len()],
            children: vec![vec![]; ids.len()],
            roots: vec![],
            ids,
            lookup,
            classes,
            store,
            memo: FastMemo::default(),
        };
        for index in 0..read.ids.len() {
            let object = ObjId::from_index(index)?;
            let class = read.classes[index];
            let Some(info) = read.metamodel().class_info(class) else {
                return Err("Unknown containment class".into());
            };
            let features = info.stored.to_vec();
            for feature in features {
                if !read
                    .metamodel()
                    .definition(feature)
                    .is_some_and(|field| field.containment)
                {
                    continue;
                }
                let count = match read.value(object, feature).map_err(|e| e.to_string())? {
                    Some(LinkValue::Many(len)) => len,
                    Some(LinkValue::Ref(target)) => {
                        read.add_child(object, target)?;
                        continue;
                    }
                    _ => continue,
                };
                for index in 0..count {
                    if let Some(LinkValue::Ref(target)) = read
                        .item(object, feature, index)
                        .map_err(|e| e.to_string())?
                    {
                        read.add_child(object, target)?;
                    }
                }
            }
        }
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
        self.ids
            .iter()
            .enumerate()
            .filter(|(_, id)| self.store.locate(id).is_none())
            .filter_map(|(i, _)| ObjId::from_index(i).ok())
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
        self.ids.get(object.index()).map(String::as_str)
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
