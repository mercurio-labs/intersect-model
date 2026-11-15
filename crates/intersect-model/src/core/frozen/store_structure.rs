//! One-pass structural acceptance over the arena (see
//! `storage::StructureFacts`). Every object must be in a form whose staged
//! audit outcome is known to be success; any other form answers false and
//! leaves the decision, and every diagnostic, to the staged audit.
use super::{FrozenStore, ObjId, Slot, is_live};
use crate::core::{
    frozen::{
        ClassId, FeatureId,
        intern::FxHashMap,
        metamodel::{FeatureKind, Metamodel},
    },
    storage::{FeatureRuleKind, StructureFacts},
};
use serde_json::Value;

const NONE: u32 = u32::MAX;

struct Rule {
    name: String,
    /// Position among the object's slots; None for a feature it does not store.
    slot: Option<usize>,
    feature: Option<FeatureId>,
    lower: usize,
    upper: i32,
    unique: bool,
    kind: FeatureRuleKind,
}

struct Plan {
    class: String,
    rules: Vec<Rule>,
}

/// The value the staged audit reads from a slot.
enum Read<'a> {
    /// No value, or a null one.
    Empty,
    Text,
    Target,
    Targets(&'a [Slot]),
    /// Anything else.
    Other,
}

fn within(count: usize, lower: usize, upper: i32) -> bool {
    count >= lower && !(upper >= 0 && count > upper as usize)
}

impl FrozenStore {
    fn structure_plan(
        &self,
        mm: &Metamodel,
        class: ClassId,
        facts: &dyn StructureFacts,
    ) -> Option<Plan> {
        let info = mm.class_info(class)?;
        if info.abstract_type {
            return None;
        }
        let key = mm.class_key(class);
        let rules = facts.class(key, info.identity.as_ref())?;
        let mut plan = Vec::with_capacity(rules.len());
        for rule in &rules {
            let feature = mm.feature(class, rule.name);
            let slot = feature.and_then(|feature| mm.slot(class, feature));
            plan.push(Rule {
                name: rule.name.to_owned(),
                slot,
                feature,
                lower: rule.lower,
                upper: rule.upper,
                unique: rule.unique,
                kind: rule.kind,
            });
        }
        for stored in info.stored.iter() {
            // The audit rejects a stored value of a feature the schema lacks.
            let name = mm.feature_name(*stored);
            if !rules.iter().any(|rule| rule.name == name) {
                return None;
            }
            // An assigned single attribute equal to a non-null default makes
            // the store present explicit assignment state.
            if let (Some(feature), Some(definition)) =
                (mm.feature_info(*stored), mm.definition(*stored))
                && feature.kind == FeatureKind::Attribute
                && !feature.many
                && !definition.default.is_null()
            {
                return None;
            }
        }
        Some(Plan {
            class: key.to_owned(),
            rules: plan,
        })
    }

    fn structure_read<'a>(&'a self, mm: &Metamodel, rule: &Rule, base: usize) -> Read<'a> {
        let data = &self.builder.data;
        let Some(slot) = rule.slot.and_then(|slot| data.slots.get(base + slot)) else {
            return Read::Empty;
        };
        match *slot {
            Slot::Absent | Slot::Null => Read::Empty,
            Slot::Default => match rule.feature.and_then(|feature| mm.definition(feature)) {
                None => Read::Empty,
                Some(definition) => match &definition.default {
                    Value::Null => Read::Empty,
                    Value::Array(items) if items.is_empty() => Read::Targets(&[]),
                    _ => Read::Other,
                },
            },
            Slot::Str(_) => Read::Text,
            Slot::Ref(_) => Read::Target,
            Slot::Many(list) => data
                .lists
                .get(list as usize)
                .map_or(Read::Other, |items| Read::Targets(items)),
            Slot::Bool(_) | Slot::Int(_) | Slot::Json(_) | Slot::Proxy(_) => Read::Other,
        }
    }

    /// Whether every class with objects has only the feature forms
    /// `accepts_structure_rows` handles. Only a screen: it spares an audit
    /// the fact preparation when acceptance cannot succeed.
    pub(super) fn may_accept_structure_rows(&self) -> bool {
        if self.builder.prepared != 0 || self.prefix.is_some() {
            return false;
        }
        let mm = &*self.builder.metamodel;
        self.postings()
            .classes
            .iter()
            .filter(|(_, rows)| !rows.is_empty())
            .all(|(class, _)| {
                mm.class_info(*class).is_some_and(|info| {
                    !info.abstract_type
                        && info.features.iter().all(|feature| {
                            mm.definition(*feature).is_some_and(|field| {
                                field.opposite.is_none()
                                    && field.datatype.is_none()
                                    && !field.unsettable
                                    && if field.derived {
                                        !field.containment
                                    } else {
                                        field.reference
                                            || (!field.containment && field.target == "EString")
                                    }
                            })
                        })
                })
            })
    }

    pub(super) fn accepts_structure_rows(&self, facts: &dyn StructureFacts) -> bool {
        let data = &self.builder.data;
        let mm = &*self.builder.metamodel;
        if self.builder.prepared != 0 || self.prefix.is_some() {
            return false;
        }
        let count = data.headers.len();
        if u32::try_from(count).is_err() {
            return false;
        }
        let dense = data.by_id.len() == count;
        let live = |row: ObjId| dense || is_live(data, row);
        let mut plans: Vec<Option<Option<Plan>>> = Vec::new();
        plans.resize_with(mm.class_count(), || None);
        let mut resources: Vec<Option<bool>> = vec![None; data.resources.len()];
        let mut conforms: FxHashMap<(usize, usize, usize), bool> = FxHashMap::default();
        // Container row of each object; an object has at most one.
        let mut container = vec![NONE; count];
        let mut distinct: Vec<u32> = Vec::new();
        for index in 0..count {
            let row = ObjId(index as u32);
            if !live(row) {
                continue;
            }
            let header = &data.headers[index];
            // Initialization receipts and assignment state need the audit.
            if !data.carried.is_empty() && data.carried.contains_key(&row) {
                return false;
            }
            let Some(loaded) = resources.get_mut(header.resource as usize) else {
                return false;
            };
            let loaded = *loaded.get_or_insert_with(|| {
                data.resources
                    .get(header.resource as usize)
                    .is_some_and(|uri| facts.resource_loaded(data.strings.resolve(*uri)))
            });
            if !loaded {
                return false;
            }
            let class = header.class.index();
            let Some(plan) = plans.get_mut(class) else {
                return false;
            };
            let Some(plan) =
                plan.get_or_insert_with(|| self.structure_plan(mm, header.class, facts))
            else {
                return false;
            };
            let base = header.slots as usize;
            for (position, rule) in plan.rules.iter().enumerate() {
                let read = self.structure_read(mm, rule, base);
                let many = rule.upper != 1;
                let targets: &[Slot] = match (rule.kind, read) {
                    (FeatureRuleKind::Derived, Read::Empty) => continue,
                    (FeatureRuleKind::Derived, _) => return false,
                    // A many-valued feature must hold a list; none is accepted
                    // here for attributes.
                    (FeatureRuleKind::Text, _) if many => return false,
                    (FeatureRuleKind::Text, Read::Empty) => {
                        if !within(0, rule.lower, rule.upper) {
                            return false;
                        }
                        continue;
                    }
                    (FeatureRuleKind::Text, Read::Text) => {
                        if !within(1, rule.lower, rule.upper) {
                            return false;
                        }
                        continue;
                    }
                    (FeatureRuleKind::Text, _) => return false,
                    (FeatureRuleKind::Reference { .. }, Read::Empty) if !many => {
                        if !within(0, rule.lower, rule.upper) {
                            return false;
                        }
                        continue;
                    }
                    (FeatureRuleKind::Reference { .. }, Read::Target) if !many => {
                        if !within(1, rule.lower, rule.upper) {
                            return false;
                        }
                        match rule.slot.and_then(|slot| data.slots.get(base + slot)) {
                            Some(slot) => std::slice::from_ref(slot),
                            None => return false,
                        }
                    }
                    (FeatureRuleKind::Reference { .. }, Read::Targets(items)) if many => {
                        if !within(items.len(), rule.lower, rule.upper) {
                            return false;
                        }
                        items
                    }
                    (FeatureRuleKind::Reference { .. }, _) => return false,
                };
                let FeatureRuleKind::Reference { containment } = rule.kind else {
                    return false;
                };
                if rule.unique && targets.len() > 1 {
                    // Live objects have distinct identities, so equal atoms
                    // are equal rows.
                    distinct.clear();
                    for item in targets {
                        let Slot::Ref(target) = item else {
                            return false;
                        };
                        distinct.push(target.0);
                    }
                    distinct.sort_unstable();
                    if distinct.windows(2).any(|pair| pair[0] == pair[1]) {
                        return false;
                    }
                }
                for item in targets {
                    let Slot::Ref(target) = *item else {
                        return false;
                    };
                    let Some(target_header) = data.headers.get(target.index()) else {
                        return false;
                    };
                    // The audit resolves the target by identity: it must be
                    // this live object, not a pending spelling.
                    if !live(target) || data.strings.resolve(target_header.id).starts_with('?') {
                        return false;
                    }
                    let target_class = target_header.class.index();
                    let accepted = match conforms.get(&(class, position, target_class)) {
                        Some(accepted) => *accepted,
                        None => {
                            let accepted = facts.conforms(
                                &plan.class,
                                &rule.name,
                                mm.class_key(target_header.class),
                            );
                            conforms.insert((class, position, target_class), accepted);
                            accepted
                        }
                    };
                    if !accepted {
                        return false;
                    }
                    if containment {
                        let Some(owner) = container.get_mut(target.index()) else {
                            return false;
                        };
                        if *owner != NONE {
                            return false;
                        }
                        *owner = index as u32;
                    }
                }
            }
        }
        // Resource roots are exactly the live objects no containment holds,
        // each in the resource that declares it.
        let mut declared = 0usize;
        let rooted = facts.roots(&mut |id, uri| {
            declared += 1;
            let Some(row) = self.object(id) else {
                return false;
            };
            let Some(header) = data.headers.get(row.index()) else {
                return false;
            };
            container.get(row.index()) == Some(&NONE)
                && data
                    .resources
                    .get(header.resource as usize)
                    .is_some_and(|resource| data.strings.resolve(*resource) == uri)
        });
        if !rooted {
            return false;
        }
        let uncontained = (0..count)
            .filter(|index| live(ObjId(*index as u32)) && container[*index] == NONE)
            .count();
        if uncontained != declared {
            return false;
        }
        // Containment is acyclic: every chain of containers ends at a root.
        let mut state = vec![0u8; count];
        let mut path: Vec<u32> = Vec::new();
        for start in 0..count {
            let mut current = start as u32;
            path.clear();
            loop {
                match state[current as usize] {
                    2 => break,
                    1 => return false,
                    _ => {}
                }
                state[current as usize] = 1;
                path.push(current);
                let owner = container[current as usize];
                if owner == NONE {
                    break;
                }
                current = owner;
            }
            for visited in &path {
                state[*visited as usize] = 2;
            }
        }
        true
    }
}
