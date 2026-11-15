//! Append-only construction and demand-driven linking. Only O(1) shape checks run per
//! write; graph contracts are checked once by `LinkedModel::validate`.
use super::{
    Diagnostics, FrozenError, Violation,
    intern::{FxHashMap, Sym},
    metamodel::{ClassId, FeatureId, FeatureKind, Metamodel},
    model::{Header, Model, ModelData, ObjId, PreparedFacts, Proxy, Slot},
};
use serde_json::Value;
use std::{
    any::Any,
    cell::{OnceCell, RefCell},
    collections::{BTreeSet, HashSet},
    rc::Rc,
    sync::Arc,
};

const NONE: u32 = u32::MAX;

/// State of a load whose prepared objects (those below
/// `ModelBuilder::prepared`) are still the validated model's, in place: they
/// hold no unresolved reference, their opposites are complete, their headers
/// record their containers, and the load has changed them only by the
/// reverse references in `completed`. While this holds, linking, completion
/// and validation visit the objects after them and the ones in `touched`,
/// not the whole model. Dropping it is always sound: every phase then works
/// over the whole model.
#[derive(Clone)]
pub(super) struct Layer {
    /// Extents of the prepared arena: later slots and lists belong to the
    /// objects after the prepared ones, or were written by this load.
    pub slots: usize,
    pub lists: usize,
    /// Prepared objects that opposite completion wrote a reference into.
    pub touched: BTreeSet<ObjId>,
    /// Those references: (object, feature, target).
    pub completed: HashSet<(u32, u32, u32), super::intern::FxBuild>,
    /// Facts about the prepared model, shared with every load on top of it.
    pub facts: Arc<PreparedFacts>,
}

#[derive(Clone)]
pub struct ModelBuilder {
    pub(super) metamodel: Arc<Metamodel>,
    pub(super) data: ModelData,
    /// Objects below this index were linked and completed by an earlier load
    /// (a prepared library, `FrozenStore::from_model`): implicit elements and
    /// defaults are completed only for the objects after them.
    pub(super) prepared: usize,
    /// See `Layer`; set only by `FrozenStore::into_builder`.
    pub(super) layer: Option<Layer>,
}

impl ModelBuilder {
    pub fn new(metamodel: Arc<Metamodel>) -> Self {
        Self {
            metamodel,
            data: ModelData::default(),
            prepared: 0,
            layer: None,
        }
    }
    pub fn with_capacity(metamodel: Arc<Metamodel>, objects: usize) -> Self {
        let mut builder = Self::new(metamodel);
        builder.data.headers.reserve(objects);
        builder
    }
    pub fn metamodel(&self) -> &Arc<Metamodel> {
        &self.metamodel
    }
    /// Whether the objects of the prepared model this builder started from
    /// (`FrozenStore::from_model`) are kept in place, so that linking,
    /// completion and validation do only the work the later objects need.
    /// False for any other builder, and once a write changes a prepared
    /// object: the phases then cover the whole model, with the same result.
    pub fn keeps_prepared_in_place(&self) -> bool {
        self.layer.is_some()
    }
    pub fn lookup(&self, identity: &str) -> Option<ObjId> {
        let sym = self.data.strings.get(identity)?;
        self.data.by_id.get(&sym).copied()
    }
    pub fn create(
        &mut self,
        identity: &str,
        class: ClassId,
        resource: &str,
    ) -> Result<ObjId, FrozenError> {
        self.create_at(identity, class, resource, 0, 0)
    }
    pub fn create_at(
        &mut self,
        identity: &str,
        class: ClassId,
        resource: &str,
        start: usize,
        end: usize,
    ) -> Result<ObjId, FrozenError> {
        let info = self
            .metamodel
            .class_info(class)
            .ok_or(FrozenError::UnknownClass)?;
        if info.abstract_type {
            return Err(FrozenError::Abstract(
                self.metamodel.class_key(class).to_owned(),
            ));
        }
        if identity.is_empty() {
            return Err(FrozenError::Identity(identity.to_owned()));
        }
        let data = &mut self.data;
        let id = data.strings.intern(identity)?;
        let obj = ObjId(
            u32::try_from(data.headers.len())
                .ok()
                .filter(|n| *n != NONE)
                .ok_or(FrozenError::Capacity("objects"))?,
        );
        if data.by_id.contains_key(&id) {
            return Err(FrozenError::Identity(identity.to_owned()));
        }
        let resource = intern_resource_uri(data, resource)?;
        push_header(
            data,
            obj,
            id,
            class,
            info.stored.len(),
            resource,
            start,
            end,
        )?;
        data.by_id.insert(id, obj);
        Ok(obj)
    }

    /// Assign an attribute value. Strings, booleans and i64 numbers are compact;
    /// every other JSON value is retained losslessly.
    pub fn set_attribute(
        &mut self,
        obj: ObjId,
        feature: FeatureId,
        value: Value,
    ) -> Result<(), FrozenError> {
        self.require(obj, feature, false)?;
        self.writing(obj);
        let slot = encode_value(&mut self.data, value)?;
        *self.data.slot_mut(&self.metamodel, obj, feature)? = slot;
        Ok(())
    }
    pub fn set_str(
        &mut self,
        obj: ObjId,
        feature: FeatureId,
        text: &str,
    ) -> Result<(), FrozenError> {
        self.require(obj, feature, false)?;
        self.writing(obj);
        let sym = self.data.strings.intern(text)?;
        *self.data.slot_mut(&self.metamodel, obj, feature)? = Slot::Str(sym);
        Ok(())
    }
    /// Single-valued features are replaced; many-valued features append.
    pub fn add_ref(
        &mut self,
        obj: ObjId,
        feature: FeatureId,
        target: ObjId,
    ) -> Result<(), FrozenError> {
        if target.index() >= self.data.headers.len() {
            return Err(FrozenError::UnknownObject);
        }
        self.push_ref(obj, feature, Slot::Ref(target))
    }
    /// Record a cross-reference by spelling; `link` resolves it.
    pub fn add_proxy(
        &mut self,
        obj: ObjId,
        feature: FeatureId,
        spelling: &str,
    ) -> Result<(), FrozenError> {
        let spelling = self.data.strings.intern(spelling)?;
        let proxy =
            u32::try_from(self.data.proxies.len()).map_err(|_| FrozenError::Capacity("proxies"))?;
        self.push_ref(obj, feature, Slot::Proxy(proxy))?;
        self.data.proxies.push(Proxy {
            owner: obj,
            feature,
            spelling,
        });
        Ok(())
    }
    /// Explicitly clear a reference feature to an empty/null stored value.
    pub fn clear(&mut self, obj: ObjId, feature: FeatureId) -> Result<(), FrozenError> {
        self.require(obj, feature, true)?;
        self.writing(obj);
        let many = self.metamodel.features[feature.index()].many;
        let slot = if many {
            new_list(&mut self.data, Vec::new())?
        } else {
            Slot::Null
        };
        *self.data.slot_mut(&self.metamodel, obj, feature)? = slot;
        Ok(())
    }

    fn push_ref(&mut self, obj: ObjId, feature: FeatureId, item: Slot) -> Result<(), FrozenError> {
        self.require(obj, feature, true)?;
        self.writing(obj);
        let many = self.metamodel.features[feature.index()].many;
        let current = *self.data.slot_mut(&self.metamodel, obj, feature)?;
        let next = match (many, current) {
            (false, _) => item,
            (true, Slot::Many(list)) => {
                self.data
                    .lists
                    .get_mut(list as usize)
                    .ok_or(FrozenError::UnknownObject)?
                    .push(item);
                return Ok(());
            }
            (true, _) => new_list(&mut self.data, vec![item])?,
        };
        *self.data.slot_mut(&self.metamodel, obj, feature)? = next;
        Ok(())
    }
    /// A direct write to a prepared object: the prepared objects are no
    /// longer the validated model's, so later phases cover the whole model.
    fn writing(&mut self, obj: ObjId) {
        if obj.index() < self.prepared {
            self.layer = None;
        }
    }
    fn require(&self, obj: ObjId, feature: FeatureId, reference: bool) -> Result<(), FrozenError> {
        let header = self.data.header(obj).ok_or(FrozenError::UnknownObject)?;
        let info = self
            .metamodel
            .feature_info(feature)
            .ok_or(FrozenError::UnknownFeature)?;
        if self.metamodel.slot(header.class, feature).is_none() {
            return Err(FrozenError::NotAFeature {
                class: self.metamodel.class_key(header.class).to_owned(),
                feature: self.metamodel.feature_name(feature).to_owned(),
            });
        }
        if (info.kind != FeatureKind::Attribute) != reference {
            return Err(FrozenError::Kind(
                self.metamodel.feature_name(feature).to_owned(),
            ));
        }
        Ok(())
    }

    /// Resolve every proxy, on demand: a resolver may read other references
    /// through the view, which resolves them first and remembers the result.
    /// A reference that depends on itself is reported as a cycle. All failures
    /// are reported together; opposites are then completed in one O(E) pass.
    pub fn link(self, resolver: &impl Resolver) -> Result<LinkedModel, Diagnostics> {
        let Self {
            metamodel,
            mut data,
            prepared,
            mut layer,
        } = self;
        let resolved: Vec<Option<ObjId>> = {
            let view = LinkView::new(
                &metamodel,
                &data,
                resolver,
                Prepared::of(prepared, layer.as_ref()),
            );
            for proxy in 0..data.proxies.len() {
                // Failures are recorded in the view; keep going to report all.
                let _ = view.resolve_proxy(proxy as u32);
            }
            let problems = view.problems.into_inner();
            if !problems.is_empty() {
                return Err(Diagnostics(problems));
            }
            view.states
                .into_inner()
                .into_iter()
                .map(|state| match state {
                    ProxyState::Resolved(target) => Some(target),
                    _ => None,
                })
                .collect()
        };
        let replace = |slot: &mut Slot| {
            if let Slot::Proxy(p) = *slot
                && let Some(Some(target)) = resolved.get(p as usize)
            {
                *slot = Slot::Ref(*target);
            }
        };
        if !data.proxies.is_empty() {
            // Prepared objects hold no unresolved reference.
            let (slots, lists) = layer.as_ref().map_or((0, 0), |l| (l.slots, l.lists));
            data.slots.iter_mut().skip(slots).for_each(replace);
            data.lists
                .iter_mut()
                .skip(lists)
                .flatten()
                .for_each(replace);
            data.proxies.clear();
        }
        complete_opposites(&metamodel, &mut data, prepared, layer.as_mut())
            .map_err(|v| Diagnostics(vec![v]))?;
        Ok(LinkedModel {
            metamodel,
            data,
            prepared,
            layer,
        })
    }
}

pub struct LinkedModel {
    pub(super) metamodel: Arc<Metamodel>,
    pub(super) data: ModelData,
    /// See `ModelBuilder::prepared`.
    pub(super) prepared: usize,
    /// See `Layer`.
    pub(super) layer: Option<Layer>,
}
impl LinkedModel {
    /// Add the objects the language supplies implicitly (`bind implicit` in
    /// Rcore) after each owner's explicit contents. All are computed against
    /// the linked model before any is added, so they cannot see each other.
    /// An implicit object's identity is its owner's, the feature and its
    /// position (`<owner>/implicit/<feature>/<n>`, then `/<feature>/<n>` for
    /// what it contains), the same on every run; it is marked implicit and has
    /// no source text. Run it before `complete_defaults`, which then covers
    /// the new objects too.
    pub fn complete_implicit(mut self, implicits: &dyn Implicits) -> Result<Self, Diagnostics> {
        let features = implicits.features();
        if features.is_empty() {
            return Ok(self);
        }
        let mut additions: Vec<(ObjId, FeatureId, Vec<ImplicitObject>)> = Vec::new();
        let mut problems = Vec::new();
        {
            let none = |_: &LinkView<'_>, _: ObjId, _: FeatureId, _: &str| Ok(None);
            let view = LinkView::new(
                &self.metamodel,
                &self.data,
                &none,
                Prepared::of(self.prepared, self.layer.as_ref()),
            );
            for index in self.prepared..self.data.headers.len() {
                let obj = ObjId(index as u32);
                let class = self.data.headers[index].class;
                for feature in &features {
                    if self.metamodel.redefined_by(class, *feature).is_some()
                        || self.metamodel.slot(class, *feature).is_none()
                    {
                        continue;
                    }
                    match implicits.objects(&view, obj, *feature) {
                        Ok(objects) if objects.is_empty() => {}
                        Ok(objects) => additions.push((obj, *feature, objects)),
                        Err(error) => problems.push(Violation::Resolver(format!(
                            "implicit {}.{}: {error}",
                            self.data.strings.resolve(self.data.headers[index].id),
                            self.metamodel.feature_name(*feature)
                        ))),
                    }
                }
            }
        }
        for (owner, feature, objects) in additions {
            if let Err(error) = self.add_implicit(owner, feature, objects) {
                problems.push(Violation::Resolver(error.to_string()));
            }
        }
        if !problems.is_empty() {
            return Err(Diagnostics(problems));
        }
        complete_opposites(
            &self.metamodel,
            &mut self.data,
            self.prepared,
            self.layer.as_mut(),
        )
        .map_err(|v| Diagnostics(vec![v]))?;
        Ok(self)
    }
    fn add_implicit(
        &mut self,
        owner: ObjId,
        feature: FeatureId,
        objects: Vec<ImplicitObject>,
    ) -> Result<(), FrozenError> {
        let header = self.data.header(owner).ok_or(FrozenError::UnknownObject)?;
        let base = format!(
            "{}/implicit/{}",
            self.data.strings.resolve(header.id),
            self.metamodel.feature_name(feature)
        );
        let mut items = Vec::with_capacity(objects.len());
        for (position, object) in objects.into_iter().enumerate() {
            items.push(Slot::Ref(self.materialize(
                owner,
                &format!("{base}/{position}"),
                object,
            )?));
        }
        let many = self.metamodel.features[feature.index()].many;
        let current = *self.data.slot_mut(&self.metamodel, owner, feature)?;
        let slot = match current {
            Slot::Many(list) if many => {
                self.data
                    .lists
                    .get_mut(list as usize)
                    .ok_or(FrozenError::UnknownFeature)?
                    .extend(items);
                return Ok(());
            }
            Slot::Default | Slot::Absent | Slot::Null => {
                feature_slot(&self.metamodel, &mut self.data, feature, items)?
            }
            _ => {
                return Err(FrozenError::Kind(format!(
                    "{} already holds its one value",
                    self.metamodel.feature_name(feature)
                )));
            }
        };
        *self.data.slot_mut(&self.metamodel, owner, feature)? = slot;
        Ok(())
    }
    /// Create `object` and what it contains, in its owner's resource at the end
    /// of the owner's text.
    fn materialize(
        &mut self,
        owner: ObjId,
        identity: &str,
        object: ImplicitObject,
    ) -> Result<ObjId, FrozenError> {
        let header = self.data.header(owner).ok_or(FrozenError::UnknownObject)?;
        let (resource, at) = (header.resource, header.end);
        let obj = create_object(
            &self.metamodel,
            &mut self.data,
            identity,
            object.class,
            resource,
            at,
        )?;
        self.data.implicit.insert(obj);
        for (feature, values) in object.values {
            let name = self.metamodel.feature_name(feature).to_owned();
            let mut items = Vec::with_capacity(values.len());
            for (position, value) in values.into_iter().enumerate() {
                items.push(match value {
                    ImplicitValue::Object(child) => Slot::Ref(self.materialize(
                        obj,
                        &format!("{identity}/{name}/{position}"),
                        child,
                    )?),
                    ImplicitValue::Ref(target) => Slot::Ref(target),
                    ImplicitValue::Value(value) => encode_value(&mut self.data, value)?,
                });
            }
            let slot = feature_slot(&self.metamodel, &mut self.data, feature, items)?;
            *self.data.slot_mut(&self.metamodel, obj, feature)? = slot;
        }
        Ok(obj)
    }
    /// Fill stored features that the model leaves unassigned or empty with
    /// values the language supplies (`bind default` in Rcore); an empty result
    /// stores no value, replacing an unassigned schema default. Every default is computed
    /// against the linked model before any is written, so the result does not
    /// depend on order; defaults therefore cannot read each other. Opposites
    /// of the written references are then completed.
    pub fn complete_defaults(mut self, defaults: &dyn Defaults) -> Result<Self, Diagnostics> {
        let features = defaults.features();
        if features.is_empty() {
            return Ok(self);
        }
        let mut writes: Vec<(ObjId, FeatureId, Vec<DefaultItem>)> = Vec::new();
        let mut problems = Vec::new();
        {
            let none = |_: &LinkView<'_>, _: ObjId, _: FeatureId, _: &str| Ok(None);
            let view = LinkView::new(
                &self.metamodel,
                &self.data,
                &none,
                Prepared::of(self.prepared, self.layer.as_ref()),
            );
            for index in self.prepared..self.data.headers.len() {
                let obj = ObjId(index as u32);
                let class = self.data.headers[index].class;
                for feature in &features {
                    // A feature redefined in this class takes its value from the
                    // redefining feature, which has its own default if any.
                    if self.metamodel.redefined_by(class, *feature).is_some() {
                        continue;
                    }
                    let Some(slot) = self.metamodel.slot(class, *feature) else {
                        continue;
                    };
                    let header = &self.data.headers[index];
                    let value = self.data.slots[header.slots as usize + slot];
                    let unassigned = matches!(value, Slot::Default | Slot::Absent);
                    let empty = self.data.is_empty_slot(&self.metamodel, *feature, value);
                    if !unassigned && !empty {
                        continue;
                    }
                    match defaults.values(&view, obj, *feature) {
                        // Already holds no value.
                        Ok(values) if values.is_empty() && empty => {}
                        Ok(values) => writes.push((obj, *feature, values)),
                        Err(error) => problems.push(Violation::Resolver(format!(
                            "default {}.{}: {error}",
                            self.data.strings.resolve(header.id),
                            self.metamodel.feature_name(*feature)
                        ))),
                    }
                }
            }
        }
        for (obj, feature, values) in writes {
            let many = self.metamodel.features[feature.index()].many;
            let mut items = Vec::with_capacity(values.len());
            for value in values {
                match value {
                    DefaultItem::Object(target) => items.push(Slot::Ref(target)),
                    DefaultItem::Value(value) => match encode_value(&mut self.data, value) {
                        Ok(slot) => items.push(slot),
                        Err(error) => problems.push(Violation::Resolver(error.to_string())),
                    },
                }
            }
            let slot = match (many, items.as_slice()) {
                (false, []) => Slot::Null,
                (false, [one]) => *one,
                (false, _) => {
                    problems.push(Violation::Resolver(format!(
                        "default {}.{} has {} values",
                        self.data
                            .header(obj)
                            .map_or("", |h| self.data.strings.resolve(h.id)),
                        self.metamodel.feature_name(feature),
                        items.len()
                    )));
                    continue;
                }
                (true, _) => match new_list(&mut self.data, items) {
                    Ok(slot) => slot,
                    Err(error) => {
                        problems.push(Violation::Resolver(error.to_string()));
                        continue;
                    }
                },
            };
            match self.data.slot_mut(&self.metamodel, obj, feature) {
                Ok(target) => *target = slot,
                Err(error) => problems.push(Violation::Resolver(error.to_string())),
            }
        }
        if !problems.is_empty() {
            return Err(Diagnostics(problems));
        }
        complete_opposites(
            &self.metamodel,
            &mut self.data,
            self.prepared,
            self.layer.as_mut(),
        )
        .map_err(|v| Diagnostics(vec![v]))?;
        Ok(self)
    }
    /// One O(N + E) structural pass; the returned Model is the proof. A
    /// load that kept its prepared objects in place
    /// (`ModelBuilder::keeps_prepared_in_place`) passes over the objects it
    /// added or wrote, when the prepared ones were valid with nothing
    /// delegated; if that pass finds a problem, the whole-model pass
    /// reports it.
    pub fn validate(self) -> Result<Model, Diagnostics> {
        self.validate_delegating(None)
    }
    /// As `validate`, except that absent fields named by `delegated` are left
    /// to the language: their values come from structural value providers,
    /// which this engine does not run yet, so their bounds are not checked.
    pub fn validate_delegating(
        self,
        delegated: Option<&dyn Fn(ClassId, FeatureId) -> bool>,
    ) -> Result<Model, Diagnostics> {
        // With nothing delegated, a valid model is valid whatever a later
        // validation delegates.
        let strict = delegated.is_none().then_some(true);
        if let Some(layer) = &self.layer {
            // The prepared objects nothing wrote to are valid as they are
            // only if they were valid with nothing delegated.
            let changed = layer
                .facts
                .strict
                .get()
                .copied()
                .unwrap_or(false)
                .then(|| {
                    super::validate::validate_added(
                        &self.metamodel,
                        &self.data,
                        delegated,
                        self.prepared,
                        &layer.touched,
                    )
                })
                .flatten();
            if let Some(changed) = changed {
                let mut data = self.data;
                for (object, (container, containing)) in changed {
                    if let Some(header) = data.headers.get_mut(object as usize) {
                        header.container = container;
                        header.containing = containing;
                    }
                }
                return Ok(Model {
                    metamodel: self.metamodel,
                    data,
                    prepared: self.prepared,
                    facts: PreparedFacts::validated(strict),
                    base: self.layer.map(|layer| layer.facts),
                });
            }
        }
        let containers = super::validate::validate(&self.metamodel, &self.data, delegated)?;
        let mut data = self.data;
        data.apply_containers(containers);
        Ok(Model {
            metamodel: self.metamodel,
            data,
            prepared: self.prepared,
            facts: PreparedFacts::validated(strict),
            base: None,
        })
    }
}
impl ModelData {
    pub(super) fn apply_containers(&mut self, containers: Vec<(u32, u32)>) {
        for (header, (container, containing)) in self.headers.iter_mut().zip(containers) {
            header.container = container;
            header.containing = containing;
        }
    }
}

#[derive(Copy, Clone, Debug)]
enum ProxyState {
    Pending,
    Active,
    Resolved(ObjId),
    Failed,
}

/// Why a read during linking could not produce a target.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LinkError {
    /// The reference is already being resolved further up this chain.
    Cycle,
    /// The reference could not be resolved; its diagnostic is already recorded.
    Unresolved,
}
impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Cycle => "reference depends on itself",
            Self::Unresolved => "reference could not be resolved",
        })
    }
}

/// One stored value as seen during linking. References are always resolved
/// targets; reading an unresolved reference resolves it first.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum LinkValue<'a> {
    /// An imported element omitted the member.
    Absent,
    Default(&'a Value),
    Null,
    Bool(bool),
    Int(i64),
    Str(&'a str),
    Ref(ObjId),
    /// A many-valued member with this many items; read them with `item`.
    Many(usize),
    Json(&'a Value),
}

/// Read-only view handed to resolvers during linking. Containment is indexed
/// once, from containment edges that were set directly. It shows the model
/// before defaults: an optional single attribute that nothing assigned reads
/// as null, not as its metamodel default.
pub struct LinkView<'a> {
    metamodel: &'a Metamodel,
    data: &'a ModelData,
    resolver: &'a dyn Resolver,
    states: RefCell<Vec<ProxyState>>,
    problems: RefCell<Vec<Violation>>,
    containment: Containment<'a>,
    roots: OnceCell<Vec<ObjId>>,
    /// Values a resolver keeps for the lifetime of this view, one per type.
    phase: RefCell<Vec<Rc<dyn Any>>>,
}
impl<'a> LinkView<'a> {
    fn new(
        metamodel: &'a Metamodel,
        data: &'a ModelData,
        resolver: &'a dyn Resolver,
        prepared: Option<Prepared<'a>>,
    ) -> Self {
        let containment = match prepared {
            Some(prepared) => Containment::layered(metamodel, data, prepared),
            None => Containment::scanned(metamodel, data),
        };
        Self {
            metamodel,
            data,
            resolver,
            states: RefCell::new(vec![ProxyState::Pending; data.proxies.len()]),
            problems: RefCell::new(Vec::new()),
            containment,
            roots: OnceCell::new(),
            phase: RefCell::new(Vec::new()),
        }
    }
    /// The value of type `T` kept for this view, created on first use. A view
    /// lives for one phase (link, implicit elements or defaults), during which
    /// the objects and stored values cannot change and a reference, once read,
    /// keeps its target; a resolver may keep results derived from successful
    /// reads here, such as memoised pure function results.
    pub(crate) fn phase_value<T: Any + Default>(&self) -> Rc<T> {
        let mut values = self.phase.borrow_mut();
        if let Some(found) = values.iter().find_map(|v| v.clone().downcast::<T>().ok()) {
            return found;
        }
        let value = Rc::new(T::default());
        values.push(value.clone());
        value
    }
    pub fn metamodel(&self) -> &'a Metamodel {
        self.metamodel
    }
    pub fn len(&self) -> usize {
        self.data.headers.len()
    }
    pub fn is_empty(&self) -> bool {
        self.data.headers.is_empty()
    }
    pub fn lookup(&self, identity: &str) -> Option<ObjId> {
        let sym = self.data.strings.get(identity)?;
        self.data.by_id.get(&sym).copied()
    }
    pub fn identity(&self, obj: ObjId) -> Option<&'a str> {
        self.data
            .header(obj)
            .map(|h| self.data.strings.resolve(h.id))
    }
    pub fn class(&self, obj: ObjId) -> Option<ClassId> {
        self.data.header(obj).map(|h| h.class)
    }
    /// Byte span of `obj` in its resource's text (`(0, 0)` when it has none).
    pub fn span(&self, obj: ObjId) -> Option<(usize, usize)> {
        self.data.header(obj).map(|h| (h.start, h.end))
    }
    /// URI of the resource that holds `obj`.
    pub fn resource(&self, obj: ObjId) -> Option<&'a str> {
        let header = self.data.header(obj)?;
        let uri = self.data.resources.get(header.resource as usize)?;
        Some(self.data.strings.resolve(*uri))
    }
    pub fn str(&self, obj: ObjId, feature: FeatureId) -> Option<&'a str> {
        match self.data.slot(self.metamodel, obj, feature)? {
            Slot::Str(sym) => Some(self.data.strings.resolve(sym)),
            _ => None,
        }
    }
    /// Direct container from containment edges set before linking.
    pub fn container(&self, obj: ObjId) -> Option<(ObjId, FeatureId)> {
        let (parent, feature) = self.containment.parent(self.data, obj)?;
        (parent != NONE).then_some((ObjId(parent), FeatureId(feature)))
    }
    /// Objects without a container, ordered by identity; computed once per phase.
    pub fn roots(&self) -> &[ObjId] {
        self.roots.get_or_init(|| {
            let by_identity = |a: &ObjId, b: &ObjId| self.identity(*a).cmp(&self.identity(*b));
            let sorted = |rows: std::ops::Range<usize>| {
                let mut roots: Vec<ObjId> = rows
                    .map(|i| ObjId(i as u32))
                    .filter(|o| self.container(*o).is_none())
                    .collect();
                roots.sort_by(by_identity);
                roots
            };
            let count = self.data.headers.len();
            // The prepared model's roots, already in order, when this view
            // may read them: its prepared headers then record that model's
            // containers.
            let known = match &self.containment {
                Containment::Layered {
                    facts: Some((facts, prepared, pristine)),
                    ..
                } => {
                    let prepared = (*prepared).min(count);
                    if *pristine {
                        Some((
                            prepared,
                            facts.roots.get_or_init(|| {
                                let mut roots: Vec<ObjId> = (0..prepared)
                                    .map(|i| ObjId(i as u32))
                                    .filter(|o| {
                                        self.data.header(*o).is_some_and(|h| h.container == NONE)
                                    })
                                    .collect();
                                roots.sort_by(by_identity);
                                roots
                            }),
                        ))
                    } else {
                        facts.roots.get().map(|roots| (prepared, roots))
                    }
                }
                _ => None,
            };
            let Some((prepared, known)) = known else {
                return sorted(0..count);
            };
            // Merge the prepared roots that still have no container with
            // the roots among the objects after them.
            let added = sorted(prepared..count);
            let mut roots = Vec::with_capacity(known.len() + added.len());
            let mut added = added.into_iter().peekable();
            for root in known.iter().filter(|o| self.container(**o).is_none()) {
                while let Some(next) = added.next_if(|next| by_identity(next, root).is_lt()) {
                    roots.push(next);
                }
                roots.push(*root);
            }
            roots.extend(added);
            roots
        })
    }
    /// Direct children in stored-feature order, then item order.
    pub fn children(&self, obj: ObjId) -> &[ObjId] {
        self.containment.children(self.metamodel, self.data, obj)
    }
    /// The stored value of `feature`; `Ok(None)` when it is not stored in this class.
    pub fn value(
        &self,
        obj: ObjId,
        feature: FeatureId,
    ) -> Result<Option<LinkValue<'a>>, LinkError> {
        match self.data.slot(self.metamodel, obj, feature) {
            None => Ok(None),
            Some(slot) => self.view(slot, feature).map(Some),
        }
    }
    /// Item `index` of a many-valued member; `Ok(None)` past the end.
    pub fn item(
        &self,
        obj: ObjId,
        feature: FeatureId,
        index: usize,
    ) -> Result<Option<LinkValue<'a>>, LinkError> {
        let slot = match self.data.slot(self.metamodel, obj, feature) {
            Some(Slot::Many(list)) => self
                .data
                .lists
                .get(list as usize)
                .and_then(|items| items.get(index))
                .copied(),
            Some(Slot::Default | Slot::Absent) => {
                return Ok(self
                    .metamodel
                    .definition(feature)
                    .and_then(|d| d.default.as_array())
                    .and_then(|items| items.get(index))
                    .map(LinkValue::Json));
            }
            _ => None,
        };
        slot.map(|slot| self.view(slot, feature)).transpose()
    }
    fn unassigned_reads_null(&self, feature: FeatureId) -> bool {
        self.metamodel.feature_info(feature).is_some_and(|info| {
            info.kind == FeatureKind::Attribute && !info.many && info.lower == 0
        })
    }
    fn view(&self, slot: Slot, feature: FeatureId) -> Result<LinkValue<'a>, LinkError> {
        let data = self.data;
        Ok(match slot {
            Slot::Absent => LinkValue::Absent,
            // An optional single attribute that nothing assigned has no value
            // before defaults run: its metamodel default (an enum's first
            // literal, as EMF supplies it) is not a parsed or constructed
            // value, and a scope must be able to tell the two apart (KerML
            // `direction`, unset on every feature that is not a parameter).
            Slot::Default if self.unassigned_reads_null(feature) => LinkValue::Null,
            Slot::Default => self
                .metamodel
                .definition(feature)
                .map_or(LinkValue::Null, |d| LinkValue::Default(&d.default)),
            Slot::Null => LinkValue::Null,
            Slot::Bool(v) => LinkValue::Bool(v),
            Slot::Int(v) => LinkValue::Int(v),
            Slot::Str(s) => LinkValue::Str(data.strings.resolve(s)),
            Slot::Ref(o) => LinkValue::Ref(o),
            Slot::Many(l) => LinkValue::Many(data.lists.get(l as usize).map_or(0, Vec::len)),
            Slot::Json(j) => data
                .json
                .get(j as usize)
                .map_or(LinkValue::Null, LinkValue::Json),
            Slot::Proxy(p) => LinkValue::Ref(self.resolve_proxy(p)?),
        })
    }
    /// Resolve one recorded reference once and remember the outcome.
    fn resolve_proxy(&self, proxy: u32) -> Result<ObjId, LinkError> {
        let index = proxy as usize;
        let Some(record) = self.data.proxies.get(index) else {
            return Err(LinkError::Unresolved);
        };
        let state = self
            .states
            .borrow()
            .get(index)
            .copied()
            .unwrap_or(ProxyState::Failed);
        let spelling = self.data.strings.resolve(record.spelling);
        let object = || self.identity(record.owner).unwrap_or("").to_owned();
        let feature = || self.metamodel.feature_name(record.feature).to_owned();
        match state {
            ProxyState::Resolved(target) => return Ok(target),
            ProxyState::Failed => return Err(LinkError::Unresolved),
            ProxyState::Active => {
                self.problems.borrow_mut().push(Violation::ReferenceCycle {
                    object: object(),
                    feature: feature(),
                    spelling: spelling.to_owned(),
                });
                return Err(LinkError::Cycle);
            }
            ProxyState::Pending => {}
        }
        self.set_state(index, ProxyState::Active);
        // No RefCell borrow is held across the call: resolvers re-enter the view.
        let outcome = self
            .resolver
            .resolve(self, record.owner, record.feature, spelling);
        let next = match outcome {
            Ok(Some(target)) if target.index() < self.data.headers.len() => {
                ProxyState::Resolved(target)
            }
            Ok(_) => {
                self.problems.borrow_mut().push(Violation::Unresolved {
                    object: object(),
                    feature: feature(),
                    spelling: spelling.to_owned(),
                });
                ProxyState::Failed
            }
            Err(error) => {
                self.problems.borrow_mut().push(Violation::Resolver(format!(
                    "{}.{} '{spelling}': {error}",
                    object(),
                    feature()
                )));
                ProxyState::Failed
            }
        };
        self.set_state(index, next);
        match next {
            ProxyState::Resolved(target) => Ok(target),
            _ => Err(LinkError::Unresolved),
        }
    }
    fn set_state(&self, index: usize, state: ProxyState) {
        if let Some(slot) = self.states.borrow_mut().get_mut(index) {
            *slot = state;
        }
    }
    /// Interned key of a string attribute; lets resolvers index by Sym, not text.
    fn str_sym(&self, obj: ObjId, feature: FeatureId) -> Option<Sym> {
        match self.data.slot(self.metamodel, obj, feature)? {
            Slot::Str(sym) => Some(sym),
            _ => None,
        }
    }
}

/// What a view knows about the prepared objects of a load (`Layer`).
#[derive(Copy, Clone)]
struct Prepared<'a> {
    /// Objects below this index record their container in their header.
    count: usize,
    /// Prepared objects written since their headers were recorded.
    touched: Option<&'a BTreeSet<ObjId>>,
    /// Facts of the prepared model, how many objects it has, and whether
    /// their headers are still that model's (so its facts may be computed
    /// from them).
    facts: Option<(&'a PreparedFacts, usize, bool)>,
}
impl<'a> Prepared<'a> {
    fn of(prepared: usize, layer: Option<&'a Layer>) -> Option<Self> {
        layer.map(|layer| Self {
            count: prepared,
            touched: Some(&layer.touched),
            facts: Some((&layer.facts, prepared, true)),
        })
    }
}

/// One object's children, listed on first use.
type ChildList = OnceCell<Box<[ObjId]>>;

/// Children per object, each list computed on first use; the cells are
/// allocated a chunk of objects at a time, on first use too.
struct LazyChildren {
    chunks: Vec<OnceCell<Box<[ChildList]>>>,
}
impl LazyChildren {
    const CHUNK: usize = 1024;
    fn new(objects: usize) -> Self {
        Self {
            chunks: (0..objects.div_ceil(Self::CHUNK))
                .map(|_| OnceCell::new())
                .collect(),
        }
    }
    fn get(&self, metamodel: &Metamodel, data: &ModelData, obj: ObjId) -> &[ObjId] {
        let Some(header) = data.header(obj) else {
            return &[];
        };
        let Some(chunk) = self.chunks.get(obj.index() / Self::CHUNK) else {
            return &[];
        };
        let chunk = chunk.get_or_init(|| (0..Self::CHUNK).map(|_| OnceCell::new()).collect());
        let Some(cell) = chunk.get(obj.index() % Self::CHUNK) else {
            return &[];
        };
        cell.get_or_init(|| {
            let mut children = Vec::new();
            let class = &metamodel.classes[header.class.index()];
            for (slot, feature) in class.stored.iter().enumerate() {
                if metamodel.features[feature.index()].kind != FeatureKind::Containment {
                    continue;
                }
                let value = data.slots[header.slots as usize + slot];
                data.slot_refs(value, &mut |child| children.push(child));
            }
            children.into_boxed_slice()
        })
    }
}

/// The containment index of a view: the first containment edge to an
/// object, in object order, then stored-feature order, then item order, is
/// its container.
enum Containment<'a> {
    /// Indexed from every containment edge of the model.
    Scanned {
        parents: Vec<(u32, u32)>,
        child_offsets: Vec<u32>,
        children: Vec<ObjId>,
    },
    /// Objects below `prepared` have the container their header records,
    /// unless an edge of a touched or later object comes first; only those
    /// objects' edges are indexed, and children are listed on demand.
    Layered {
        prepared: usize,
        /// Containers of prepared objects found among the indexed edges.
        adopted: FxHashMap<u32, (u32, u32)>,
        /// Containers of the objects after the prepared ones.
        added: Vec<(u32, u32)>,
        children: LazyChildren,
        facts: Option<(&'a PreparedFacts, usize, bool)>,
    },
}
impl<'a> Containment<'a> {
    fn scanned(metamodel: &Metamodel, data: &ModelData) -> Self {
        let n = data.headers.len();
        let mut parents = vec![(NONE, NONE); n];
        let mut child_offsets = Vec::with_capacity(n + 1);
        let mut children = Vec::new();
        for (index, header) in data.headers.iter().enumerate() {
            child_offsets.push(children.len() as u32);
            let class = &metamodel.classes[header.class.index()];
            for (slot, feature) in class.stored.iter().enumerate() {
                if metamodel.features[feature.index()].kind != FeatureKind::Containment {
                    continue;
                }
                let value = data.slots[header.slots as usize + slot];
                data.slot_refs(value, &mut |child| {
                    children.push(child);
                    if let Some(parent) = parents.get_mut(child.index())
                        && parent.0 == NONE
                    {
                        *parent = (index as u32, feature.0);
                    }
                });
            }
        }
        child_offsets.push(children.len() as u32);
        Self::Scanned {
            parents,
            child_offsets,
            children,
        }
    }
    fn layered(metamodel: &Metamodel, data: &ModelData, prepared: Prepared<'a>) -> Self {
        let n = data.headers.len();
        let count = prepared.count.min(n);
        let mut adopted: FxHashMap<u32, (u32, u32)> = FxHashMap::default();
        let mut added = vec![(NONE, NONE); n - count];
        let touched = prepared
            .touched
            .into_iter()
            .flatten()
            .map(|row| row.index())
            .filter(|row| *row < count);
        for index in touched.chain(count..n) {
            let header = &data.headers[index];
            let class = &metamodel.classes[header.class.index()];
            for (slot, feature) in class.stored.iter().enumerate() {
                if metamodel.features[feature.index()].kind != FeatureKind::Containment {
                    continue;
                }
                let value = data.slots[header.slots as usize + slot];
                data.slot_refs(value, &mut |child| {
                    let edge = (index as u32, feature.0);
                    if child.index() < count {
                        adopted.entry(child.0).or_insert(edge);
                    } else if let Some(parent) = added.get_mut(child.index() - count)
                        && parent.0 == NONE
                    {
                        *parent = edge;
                    }
                });
            }
        }
        Self::Layered {
            prepared: count,
            adopted,
            added,
            children: LazyChildren::new(n),
            facts: prepared.facts,
        }
    }
    /// The (container, feature) of `obj`, NONE when it has none.
    fn parent(&self, data: &ModelData, obj: ObjId) -> Option<(u32, u32)> {
        match self {
            Self::Scanned { parents, .. } => parents.get(obj.index()).copied(),
            Self::Layered {
                prepared,
                adopted,
                added,
                ..
            } => {
                if obj.index() >= *prepared {
                    return added.get(obj.index() - prepared).copied();
                }
                let header = data.header(obj)?;
                let recorded = (header.container, header.containing);
                Some(match adopted.get(&obj.0) {
                    // An indexed edge of the recorded container itself is
                    // that container's first edge to the object.
                    Some(found) if recorded.0 == NONE || found.0 <= recorded.0 => *found,
                    _ => recorded,
                })
            }
        }
    }
    fn children<'s>(&'s self, metamodel: &Metamodel, data: &ModelData, obj: ObjId) -> &'s [ObjId] {
        match self {
            Self::Scanned {
                child_offsets,
                children,
                ..
            } => {
                let i = obj.index();
                match (child_offsets.get(i), child_offsets.get(i + 1)) {
                    (Some(a), Some(b)) => children.get(*a as usize..*b as usize).unwrap_or(&[]),
                    _ => &[],
                }
            }
            Self::Layered { children, .. } => children.get(metamodel, data, obj),
        }
    }
}

/// How serious one language issue is.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
}
impl Severity {
    /// `"error"`, `"warning"` or `"info"`.
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "error" => Some(Self::Error),
            "warning" => Some(Self::Warning),
            "info" => Some(Self::Info),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }
}

/// One problem a language validation rule (`bind validate` in Rcore) reports
/// about an object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub severity: Severity,
    /// A stable code naming the rule, for example the Pilot's issue code.
    pub code: String,
    pub message: String,
    /// The name of the feature the issue concerns, if the rule names one.
    pub feature: Option<String>,
    /// An identifier of the rule, when the rule names one (needed where
    /// several rules share a code).
    pub rule: Option<String>,
}

/// An issue with the object it concerns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectIssue {
    pub object: ObjId,
    pub issue: Issue,
}

/// The code of the issue `Model::check` reports when a validation itself
/// fails (a budget, an unsupported construct, a malformed result).
pub const INTERNAL_VALIDATION: &str = "internal.validation";

/// Language validation rules (`bind validate`), run over a validated model.
pub trait Validators {
    /// Whether no class has a rule.
    fn is_empty(&self) -> bool;
    /// The issues of every rule whose class `obj` conforms to, in rule order.
    /// An error is a failure of the rules themselves, not an issue.
    fn issues(&self, view: &LinkView<'_>, obj: ObjId) -> Result<Vec<Issue>, FrozenError>;
}

impl Model {
    /// The objects of the prepared model the load started from (see
    /// `FrozenStore::from_model`); 0 for a model built from scratch.
    pub fn prepared(&self) -> usize {
        self.prepared
    }
    /// Run `validators` over the objects this load added (those at or after
    /// `prepared()`), in object order; each object's issues are in rule
    /// order. A rule that fails is reported as an error issue with the code
    /// `INTERNAL_VALIDATION`, never a panic. The model is not changed.
    pub fn check(&self, validators: &dyn Validators) -> Vec<ObjectIssue> {
        let mut found = Vec::new();
        if validators.is_empty() {
            return found;
        }
        let none = |_: &LinkView<'_>, _: ObjId, _: FeatureId, _: &str| Ok(None);
        // Every header of a validated model records its container. The
        // prepared model's roots may be read if a load computed them, never
        // computed here: this load may have given its roots containers.
        let view = LinkView::new(
            &self.metamodel,
            &self.data,
            &none,
            Some(Prepared {
                count: self.data.headers.len(),
                touched: None,
                facts: self
                    .base
                    .as_deref()
                    .map(|facts| (facts, self.prepared, false)),
            }),
        );
        for index in self.prepared..self.data.headers.len() {
            let object = ObjId(index as u32);
            match validators.issues(&view, object) {
                Ok(issues) => found.extend(
                    issues
                        .into_iter()
                        .map(|issue| ObjectIssue { object, issue }),
                ),
                Err(error) => found.push(ObjectIssue {
                    object,
                    issue: Issue {
                        severity: Severity::Error,
                        code: INTERNAL_VALIDATION.to_owned(),
                        message: format!("validation failed: {error}"),
                        feature: None,
                        rule: None,
                    },
                }),
            }
        }
        found
    }
}

/// Pluggable scoping. Implementations take `&self` because resolution is
/// re-entrant: resolving one reference may read others through the view.
/// Memo tables belong in interior cells and live for one link phase.
/// One value a language default supplies.
#[derive(Clone, Debug, PartialEq)]
pub enum DefaultItem {
    /// A reference target.
    Object(ObjId),
    /// An attribute value in the canonical JSON form.
    Value(Value),
}

/// One object a language adds implicitly (`bind implicit` in Rcore), with its
/// stored feature values.
#[derive(Clone, Debug, PartialEq)]
pub struct ImplicitObject {
    pub class: ClassId,
    pub values: Vec<(FeatureId, Vec<ImplicitValue>)>,
}
/// One value of an implicit object's feature.
#[derive(Clone, Debug, PartialEq)]
pub enum ImplicitValue {
    /// A contained object, created with the parent.
    Object(ImplicitObject),
    /// A reference to an existing object.
    Ref(ObjId),
    /// An attribute value in the canonical JSON form.
    Value(Value),
}

/// Objects a language adds to containment references (`bind implicit`).
pub trait Implicits {
    /// The containment features that have an implicit binding.
    fn features(&self) -> Vec<FeatureId>;
    /// The objects to add to `feature` of `obj`, after its explicit contents.
    fn objects(
        &self,
        view: &LinkView<'_>,
        obj: ObjId,
        feature: FeatureId,
    ) -> Result<Vec<ImplicitObject>, FrozenError>;
}

/// Values a language supplies for stored features a model leaves unassigned
/// or empty.
pub trait Defaults {
    /// The features that have a default.
    fn features(&self) -> Vec<FeatureId>;
    /// The default values of `feature` on `obj`; empty stores no value.
    fn values(
        &self,
        view: &LinkView<'_>,
        obj: ObjId,
        feature: FeatureId,
    ) -> Result<Vec<DefaultItem>, FrozenError>;
}

pub trait Resolver {
    fn resolve(
        &self,
        view: &LinkView<'_>,
        owner: ObjId,
        feature: FeatureId,
        spelling: &str,
    ) -> Result<Option<ObjId>, FrozenError>;
}
impl<F> Resolver for F
where
    F: Fn(&LinkView<'_>, ObjId, FeatureId, &str) -> Result<Option<ObjId>, FrozenError>,
{
    fn resolve(
        &self,
        view: &LinkView<'_>,
        owner: ObjId,
        feature: FeatureId,
        spelling: &str,
    ) -> Result<Option<ObjId>, FrozenError> {
        self(view, owner, feature, spelling)
    }
}

/// Spelling is the target's model identity.
pub struct ById;
impl Resolver for ById {
    fn resolve(
        &self,
        view: &LinkView<'_>,
        _: ObjId,
        _: FeatureId,
        spelling: &str,
    ) -> Result<Option<ObjId>, FrozenError> {
        Ok(view.lookup(spelling))
    }
}

/// Spelling is the unique value of one string attribute. The index is built
/// once, on first use, and lives only for this link phase.
pub struct ByName {
    feature: FeatureId,
    index: OnceCell<FxHashMap<Sym, Option<ObjId>>>,
}
impl ByName {
    pub fn new(feature: FeatureId) -> Self {
        Self {
            feature,
            index: OnceCell::new(),
        }
    }
}
impl Resolver for ByName {
    fn resolve(
        &self,
        view: &LinkView<'_>,
        _: ObjId,
        _: FeatureId,
        spelling: &str,
    ) -> Result<Option<ObjId>, FrozenError> {
        let feature = self.feature;
        let index = self.index.get_or_init(|| {
            let mut index = FxHashMap::default();
            for i in 0..view.len() {
                let obj = ObjId(i as u32);
                if let Some(sym) = view.str_sym(obj, feature) {
                    index
                        .entry(sym)
                        .and_modify(|found: &mut Option<ObjId>| *found = None)
                        .or_insert(Some(obj));
                }
            }
            index
        });
        let Some(sym) = view.data.strings.get(spelling) else {
            return Ok(None);
        };
        match index.get(&sym) {
            Some(Some(obj)) => Ok(Some(*obj)),
            Some(None) => Err(FrozenError::Ambiguous(spelling.to_owned())),
            None => Ok(None),
        }
    }
}

pub(super) fn intern_resource_uri(data: &mut ModelData, uri: &str) -> Result<u32, FrozenError> {
    let sym = data.strings.intern(uri)?;
    if let Some(index) = data.resources.iter().rposition(|r| *r == sym) {
        return u32::try_from(index).map_err(|_| FrozenError::Capacity("resources"));
    }
    let index =
        u32::try_from(data.resources.len()).map_err(|_| FrozenError::Capacity("resources"))?;
    data.resources.push(sym);
    data.resource_records.push(None);
    Ok(index)
}
pub(super) fn intern_resource_record(
    data: &mut ModelData,
    uri: &str,
    record: crate::core::resources::ResourceRecord,
) -> Result<(), FrozenError> {
    let index = intern_resource_uri(data, uri)? as usize;
    if let Some(slot) = data.resource_records.get_mut(index) {
        *slot = Some(record);
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
fn push_header(
    data: &mut ModelData,
    obj: ObjId,
    id: Sym,
    class: ClassId,
    stored: usize,
    resource: u32,
    start: usize,
    end: usize,
) -> Result<(), FrozenError> {
    let slots = u32::try_from(data.slots.len()).map_err(|_| FrozenError::Capacity("slots"))?;
    data.slots
        .extend(std::iter::repeat_n(Slot::Default, stored));
    data.headers.push(Header {
        class,
        id,
        resource,
        slots,
        container: NONE,
        containing: NONE,
        start,
        end,
    });
    debug_assert_eq!(data.headers.len(), obj.index() + 1);
    Ok(())
}
/// A new object with a fresh identity, in an already interned resource.
fn create_object(
    metamodel: &Metamodel,
    data: &mut ModelData,
    identity: &str,
    class: ClassId,
    resource: u32,
    at: usize,
) -> Result<ObjId, FrozenError> {
    let info = metamodel
        .class_info(class)
        .ok_or(FrozenError::UnknownClass)?;
    if info.abstract_type {
        return Err(FrozenError::Abstract(metamodel.class_key(class).to_owned()));
    }
    let id = data.strings.intern(identity)?;
    let obj = ObjId(
        u32::try_from(data.headers.len())
            .ok()
            .filter(|n| *n != NONE)
            .ok_or(FrozenError::Capacity("objects"))?,
    );
    if data.by_id.insert(id, obj).is_some() {
        return Err(FrozenError::Identity(identity.to_owned()));
    }
    push_header(data, obj, id, class, info.stored.len(), resource, at, at)?;
    Ok(obj)
}
/// Store `items` as the value of `feature`: a list for a many-valued feature,
/// otherwise its one value or null.
fn feature_slot(
    metamodel: &Metamodel,
    data: &mut ModelData,
    feature: FeatureId,
    items: Vec<Slot>,
) -> Result<Slot, FrozenError> {
    let many = metamodel.features[feature.index()].many;
    match (many, items.as_slice()) {
        (true, _) => new_list(data, items),
        (false, []) => Ok(Slot::Null),
        (false, [one]) => Ok(*one),
        (false, _) => Err(FrozenError::Kind(format!(
            "{} holds one value, not {}",
            metamodel.feature_name(feature),
            items.len()
        ))),
    }
}

pub(super) fn new_list(data: &mut ModelData, items: Vec<Slot>) -> Result<Slot, FrozenError> {
    let list = u32::try_from(data.lists.len()).map_err(|_| FrozenError::Capacity("lists"))?;
    data.lists.push(items);
    Ok(Slot::Many(list))
}
pub(super) fn encode_value(data: &mut ModelData, value: Value) -> Result<Slot, FrozenError> {
    Ok(match value {
        Value::Null => Slot::Null,
        Value::Bool(v) => Slot::Bool(v),
        Value::String(v) => Slot::Str(data.strings.intern(&v)?),
        Value::Number(ref n) if n.is_i64() => match n.as_i64() {
            Some(v) => Slot::Int(v),
            None => json_slot(data, value)?,
        },
        Value::Array(items) => {
            let items = items
                .into_iter()
                .map(|item| encode_list_item(data, item))
                .collect::<Result<_, _>>()?;
            new_list(data, items)?
        }
        value => json_slot(data, value)?,
    })
}
/// A collection member is an atom: nested JSON stays lossless rather than
/// becoming another arena collection. Shared by replacement and append.
pub(super) fn encode_list_item(data: &mut ModelData, value: Value) -> Result<Slot, FrozenError> {
    match value {
        Value::Array(_) | Value::Object(_) => json_slot(data, value),
        value => encode_value(data, value),
    }
}
pub(super) fn json_slot(data: &mut ModelData, value: Value) -> Result<Slot, FrozenError> {
    let index = u32::try_from(data.json.len()).map_err(|_| FrozenError::Capacity("json values"))?;
    data.json.push(value);
    Ok(Slot::Json(index))
}

/// For each edge a.f -> b with opposite g, ensure b.g contains a. Existing
/// reverse edges are found through one hash set, so this pass is O(E).
///
/// With a `layer`, only the edges of the objects after the prepared ones are
/// visited: the prepared model's own edges already have their reverse ends,
/// and a prepared object refers to a later one only through a reverse edge
/// this function wrote, which the layer remembers.
fn complete_opposites(
    mm: &Metamodel,
    data: &mut ModelData,
    prepared: usize,
    layer: Option<&mut Layer>,
) -> Result<(), Violation> {
    let Some(layer) = layer else {
        return complete_edges(mm, data, 0, |_, _, _| true);
    };
    let first = prepared.min(data.headers.len());
    complete_edges(mm, data, first, |a, b, g| {
        if b.index() >= first {
            return true;
        }
        // A prepared object holds a reference to a later one only if an
        // earlier pass of this load wrote it.
        let new = layer.completed.insert((b.0, g.0, a.0));
        if new {
            layer.touched.insert(b);
        }
        new
    })
}

/// `complete_opposites` over the edges of the objects from `first` on.
/// `missing(a, b, g)` is asked, for a reverse edge b.g -> a that no visited
/// edge supplies, whether it is indeed missing.
fn complete_edges(
    mm: &Metamodel,
    data: &mut ModelData,
    first: usize,
    mut missing: impl FnMut(ObjId, ObjId, FeatureId) -> bool,
) -> Result<(), Violation> {
    let mut edges: Vec<(ObjId, FeatureId, ObjId, FeatureId)> = Vec::new();
    for (index, header) in data.headers.iter().enumerate().skip(first) {
        let class = &mm.classes[header.class.index()];
        for (slot, feature) in class.stored.iter().enumerate() {
            let Some(opposite) = mm.features[feature.index()].opposite else {
                continue;
            };
            let value = data.slots[header.slots as usize + slot];
            data.slot_refs(value, &mut |target| {
                edges.push((ObjId(index as u32), *feature, target, opposite));
            });
        }
    }
    if edges.is_empty() {
        return Ok(());
    }
    let mut present: HashSet<(u32, u32, u32), super::intern::FxBuild> =
        edges.iter().map(|(a, f, b, _)| (a.0, f.0, b.0)).collect();
    for (a, _, b, g) in edges {
        if !present.insert((b.0, g.0, a.0)) || !missing(a, b, g) {
            continue;
        }
        let many = mm.features[g.index()].many;
        let current = *data
            .slot_mut(mm, b, g)
            .map_err(|e| Violation::Resolver(e.to_string()))?;
        let next = match (current, many) {
            (Slot::Many(list), true) => {
                if let Some(items) = data.lists.get_mut(list as usize) {
                    items.push(Slot::Ref(a));
                }
                continue;
            }
            (_, true) => new_list(data, vec![Slot::Ref(a)])
                .map_err(|e| Violation::Resolver(e.to_string()))?,
            (Slot::Default | Slot::Null, false) => Slot::Ref(a),
            (_, false) => {
                return Err(Violation::Opposite {
                    object: data.strings.resolve(data.headers[b.index()].id).to_owned(),
                    feature: mm.feature_name(g).to_owned(),
                });
            }
        };
        *data
            .slot_mut(mm, b, g)
            .map_err(|e| Violation::Resolver(e.to_string()))? = next;
    }
    Ok(())
}
