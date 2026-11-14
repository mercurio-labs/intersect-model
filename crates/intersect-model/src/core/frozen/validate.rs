//! One structural pass over a linked model: classes, multiplicities, value
//! types, reference conformance, uniqueness, single containment, containment
//! acyclicity and opposite consistency. Every violation is collected.
use super::{
    Diagnostics, Violation,
    intern::{FxBuild, FxHashMap},
    interop::decode,
    metamodel::{ClassId, FeatureId, FeatureKind, Metamodel},
    model::{ModelData, ObjId, Slot},
};
use serde_json::Value;
use std::collections::{BTreeSet, HashSet};

const NONE: u32 = u32::MAX;

/// The checks of one object's own values, shared by the whole-model pass and
/// the pass over the objects a load on a prepared model added or wrote.
struct Rows<'a> {
    mm: &'a Metamodel,
    data: &'a ModelData,
    delegated: Option<&'a dyn Fn(ClassId, FeatureId) -> bool>,
    problems: Vec<Violation>,
    /// Attribute validators are pure functions of (feature definition,
    /// value); interned scalars repeat heavily, so each distinct pair is
    /// checked once.
    checked: HashSet<(u32, u8, u64), FxBuild>,
    /// Every edge of a feature with an opposite, when `opposites` is set.
    opposite_edges: Vec<(u32, FeatureId, u32, FeatureId)>,
    opposites: bool,
}

/// Receives one containment edge (owner, feature, target) and reports a
/// second container of the target.
type Contain<'c> = dyn FnMut(&mut Vec<Violation>, u32, FeatureId, ObjId) + 'c;

fn name(data: &ModelData, obj: usize) -> String {
    data.strings.resolve(data.headers[obj].id).to_owned()
}

impl<'a> Rows<'a> {
    fn new(
        mm: &'a Metamodel,
        data: &'a ModelData,
        delegated: Option<&'a dyn Fn(ClassId, FeatureId) -> bool>,
        opposites: bool,
    ) -> Self {
        Self {
            mm,
            data,
            delegated,
            problems: Vec::new(),
            checked: HashSet::default(),
            opposite_edges: Vec::new(),
            opposites,
        }
    }

    /// Check object `index`. `contain` receives each of its containment
    /// edges in stored order.
    fn check(&mut self, index: usize, contain: &mut Contain<'_>) {
        let (mm, data, delegated) = (self.mm, self.data, self.delegated);
        let problems = &mut self.problems;
        let header = &data.headers[index];
        let Some(class) = mm.classes.get(header.class.index()) else {
            problems.push(Violation::UnknownClass(name(data, index)));
            return;
        };
        if class.abstract_type {
            problems.push(Violation::Abstract {
                object: name(data, index),
                class: mm.class_key(header.class).to_owned(),
            });
        }
        for (slot_index, feature) in class.stored.iter().enumerate() {
            let info = &mm.features[feature.index()];
            let Some(definition) = mm.definition(*feature) else {
                continue;
            };
            let slot = data.slots[header.slots as usize + slot_index];
            if delegated.is_some_and(|d| d(header.class, *feature))
                && data.is_empty_slot(mm, *feature, slot)
            {
                continue;
            }
            // A redefined feature is checked through its redefining feature.
            if mm.redefined_by(header.class, *feature).is_some()
                && data.is_empty_slot(mm, *feature, slot)
            {
                continue;
            }
            let items: &[Slot] = match slot {
                Slot::Many(list) => data.lists.get(list as usize).map_or(&[], Vec::as_slice),
                _ => std::slice::from_ref(&data.slots[header.slots as usize + slot_index]),
            };
            let count = match slot {
                Slot::Default | Slot::Absent => match &definition.default {
                    Value::Null => 0,
                    Value::Array(items) => items.len(),
                    _ => 1,
                },
                Slot::Null => 0,
                Slot::Many(_) => items.len(),
                _ => 1,
            };
            if !info.many && matches!(slot, Slot::Many(_)) {
                problems.push(Violation::Kind {
                    object: name(data, index),
                    feature: mm.feature_name(*feature).to_owned(),
                });
                continue;
            }
            if (count as u64) < u64::from(info.lower)
                || info.upper.is_some_and(|u| count as u64 > u64::from(u))
            {
                problems.push(Violation::Multiplicity {
                    object: name(data, index),
                    feature: mm.feature_name(*feature).to_owned(),
                    count,
                    lower: info.lower,
                    upper: info.upper,
                });
            }
            if matches!(slot, Slot::Default | Slot::Absent | Slot::Null) {
                continue;
            }
            match info.kind {
                FeatureKind::Attribute => {
                    if items
                        .iter()
                        .any(|i| matches!(i, Slot::Ref(_) | Slot::Proxy(_)))
                    {
                        problems.push(Violation::Kind {
                            object: name(data, index),
                            feature: mm.feature_name(*feature).to_owned(),
                        });
                        continue;
                    }
                    // Datatype validators take one value; many-valued attributes
                    // are checked item by item.
                    for item in items {
                        let key = match item {
                            Slot::Bool(v) => Some((1, u64::from(*v))),
                            Slot::Int(v) => Some((2, *v as u64)),
                            Slot::Str(s) => Some((3, u64::from(s.0))),
                            _ => None,
                        };
                        if let Some((tag, bits)) = key
                            && !self.checked.insert((info.definition, tag, bits))
                        {
                            continue;
                        }
                        let value = decode(data, *item);
                        if let Err(error) = mm.schema.validate_attribute(definition, &value) {
                            problems.push(Violation::Attribute {
                                object: name(data, index),
                                feature: mm.feature_name(*feature).to_owned(),
                                error,
                            });
                        }
                    }
                }
                FeatureKind::Reference | FeatureKind::Containment => {
                    for item in items {
                        let target = match item {
                            Slot::Ref(target) if target.index() < data.headers.len() => *target,
                            Slot::Proxy(_) => {
                                problems.push(Violation::Unresolved {
                                    object: name(data, index),
                                    feature: mm.feature_name(*feature).to_owned(),
                                    spelling: String::new(),
                                });
                                continue;
                            }
                            _ => {
                                problems.push(Violation::Kind {
                                    object: name(data, index),
                                    feature: mm.feature_name(*feature).to_owned(),
                                });
                                continue;
                            }
                        };
                        if !mm.conforms(data.headers[target.index()].class, *feature) {
                            problems.push(Violation::TargetType {
                                object: name(data, index),
                                feature: mm.feature_name(*feature).to_owned(),
                                target: name(data, target.index()),
                            });
                        }
                        if info.kind == FeatureKind::Containment {
                            contain(problems, index as u32, *feature, target);
                        }
                        if let Some(opposite) = info.opposite.filter(|_| self.opposites) {
                            self.opposite_edges
                                .push((index as u32, *feature, target.0, opposite));
                        }
                    }
                    if info.unique && info.many && has_duplicate(items) {
                        problems.push(Violation::Duplicate {
                            object: name(data, index),
                            feature: mm.feature_name(*feature).to_owned(),
                        });
                    }
                }
            }
        }
    }
}

/// Returns each object's (container, containing feature), or NONE pairs.
/// `delegated` names fields whose values a language supplies (structural
/// value providers); when such a field is empty it is not checked here.
pub(super) fn validate(
    mm: &Metamodel,
    data: &ModelData,
    delegated: Option<&dyn Fn(ClassId, FeatureId) -> bool>,
) -> Result<Vec<(u32, u32)>, Diagnostics> {
    let mut containers = vec![(NONE, NONE); data.headers.len()];
    let mut rows = Rows::new(mm, data, delegated, true);
    for index in 0..data.headers.len() {
        rows.check(index, &mut |problems, owner, feature, target| {
            let entry = &mut containers[target.index()];
            if entry.0 != NONE {
                problems.push(Violation::MultipleContainers(name(data, target.index())));
            } else {
                *entry = (owner, feature.0);
            }
        });
    }
    let Rows {
        mut problems,
        opposite_edges,
        ..
    } = rows;

    // Containment acyclicity: each object is entered once (white/grey/black).
    let mut state = vec![0u8; containers.len()];
    let mut path = Vec::new();
    for start in 0..containers.len() {
        let mut at = start as u32;
        while at != NONE && state[at as usize] == 0 {
            state[at as usize] = 1;
            path.push(at);
            at = containers[at as usize].0;
        }
        if at != NONE && state[at as usize] == 1 {
            problems.push(Violation::ContainmentCycle(name(data, at as usize)));
        }
        for visited in path.drain(..) {
            state[visited as usize] = 2;
        }
    }

    // Opposite consistency over the collected edge set.
    let present: HashSet<(u32, u32, u32), FxBuild> = opposite_edges
        .iter()
        .map(|(a, f, b, _)| (*a, f.0, *b))
        .collect();
    for (a, _, b, g) in &opposite_edges {
        if !present.contains(&(*b, g.0, *a)) {
            problems.push(Violation::Opposite {
                object: name(data, *b as usize),
                feature: mm.feature_name(*g).to_owned(),
            });
        }
    }

    if problems.is_empty() {
        Ok(containers)
    } else {
        Err(Diagnostics(problems))
    }
}

/// Whether every object below `prepared` passes the checks of its own values
/// with no field delegated. Containment and opposites are not looked at: a
/// validated model satisfies them whatever was delegated.
pub(super) fn rows_valid(mm: &Metamodel, data: &ModelData, prepared: usize) -> bool {
    let mut rows = Rows::new(mm, data, None, false);
    for index in 0..prepared.min(data.headers.len()) {
        rows.check(index, &mut |_, _, _, _| {});
        if !rows.problems.is_empty() {
            return false;
        }
    }
    true
}

/// `validate` for a load on top of a prepared model whose objects (those
/// below `prepared`) were valid with nothing delegated, keep the containers
/// their headers record, and have changed only by the references added to
/// the objects in `touched`. Only those objects and the ones after the
/// prepared model are checked; a prepared object nothing wrote to has the
/// values and targets it was validated with.
///
/// Returns the containers that changed, by object, when the model is valid.
/// `None` means a whole-model `validate` is needed: it finds the same
/// problems and reports them in its own order.
pub(super) fn validate_added(
    mm: &Metamodel,
    data: &ModelData,
    delegated: Option<&dyn Fn(ClassId, FeatureId) -> bool>,
    prepared: usize,
    touched: &BTreeSet<ObjId>,
) -> Option<FxHashMap<u32, (u32, u32)>> {
    let checked_here = |row: u32| row as usize >= prepared || touched.contains(&ObjId(row));
    // The container of each target of a containment edge checked here.
    let mut seen: FxHashMap<u32, (u32, u32)> = FxHashMap::default();
    let mut rows = Rows::new(mm, data, delegated, true);
    let order = touched
        .iter()
        .map(|row| row.index())
        .filter(|row| *row < prepared)
        .chain(prepared..data.headers.len());
    for index in order {
        rows.check(index, &mut |problems, owner, feature, target| {
            // A prepared target keeps the one container its header records,
            // unless that container's edges are checked here too.
            let kept = data
                .headers
                .get(target.index())
                .filter(|_| target.index() < prepared)
                .is_some_and(|header| header.container != NONE && !checked_here(header.container));
            if kept || seen.insert(target.0, (owner, feature.0)).is_some() {
                problems.push(Violation::MultipleContainers(name(data, target.index())));
            }
        });
        if !rows.problems.is_empty() {
            return None;
        }
    }
    // A containment cycle passes through an edge checked here: the prepared
    // model had none.
    let container = |row: u32| -> u32 {
        match seen.get(&row) {
            Some((owner, _)) => *owner,
            None if (row as usize) < prepared => data.headers[row as usize].container,
            None => NONE,
        }
    };
    for start in seen.keys() {
        let mut at = container(*start);
        let mut steps = 0usize;
        while at != NONE {
            steps += 1;
            if at == *start || steps > data.headers.len() {
                return None;
            }
            at = container(at);
        }
    }
    // Opposite consistency of the edges checked here. The other end is
    // either checked here too, or read where it is stored.
    let present: HashSet<(u32, u32, u32), FxBuild> = rows
        .opposite_edges
        .iter()
        .map(|(a, f, b, _)| (*a, f.0, *b))
        .collect();
    for (a, _, b, g) in &rows.opposite_edges {
        let found = if checked_here(*b) {
            present.contains(&(*b, g.0, *a))
        } else {
            let mut found = false;
            if let Some(slot) = data.headers.get(*b as usize).and_then(|header| {
                let slot = mm.slot(header.class, *g)?;
                data.slots.get(header.slots as usize + slot)
            }) {
                data.slot_refs(*slot, &mut |target| found |= target.0 == *a);
            }
            found
        };
        if !found {
            return None;
        }
    }
    Some(seen)
}

fn has_duplicate(items: &[Slot]) -> bool {
    let ids = items.iter().filter_map(|i| match i {
        Slot::Ref(ObjId(id)) => Some(*id),
        _ => None,
    });
    if items.len() <= 16 {
        let ids: Vec<u32> = ids.collect();
        ids.iter()
            .enumerate()
            .any(|(i, a)| ids[i + 1..].contains(a))
    } else {
        let mut ids: Vec<u32> = ids.collect();
        ids.sort_unstable();
        ids.windows(2).any(|w| w[0] == w[1])
    }
}
