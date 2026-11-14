//! Frozen metamodel: every schema fact the model path needs, compiled once into
//! dense tables. Holding a `&Metamodel` means `freeze` succeeded; nothing here
//! is re-validated on use.
use super::{
    FrozenError,
    intern::{FxHashMap, Interner, Sym},
};
use intersect_model::definitions::{
    resources::DeclarationId,
    schema::{Feature, MemberIdentity, Schema},
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClassId(pub(super) u32);
/// Global feature identity: one per (declaring class, name), shared by every
/// class that inherits it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FeatureId(pub(super) u32);
impl ClassId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}
impl FeatureId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FeatureKind {
    Attribute,
    Reference,
    Containment,
}
/// Reference conformance is a bit test: row `target` of `conformance`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Data(Sym),
    Classes(u32),
}

#[derive(Clone, Debug)]
pub struct FeatureInfo {
    pub name: Sym,
    pub owner: ClassId,
    pub kind: FeatureKind,
    pub target: Target,
    pub opposite: Option<FeatureId>,
    pub lower: u32,
    /// None is unbounded.
    pub upper: Option<u32>,
    pub many: bool,
    pub unique: bool,
    pub ordered: bool,
    pub derived: bool,
    /// Index into `Metamodel::definitions`; the original schema payload.
    pub(super) definition: u32,
}

#[derive(Clone, Debug)]
pub struct ClassInfo {
    pub key: Sym,
    pub identity: Arc<str>,
    pub abstract_type: bool,
    pub(super) has_unsettable: bool,
    pub(super) has_containment_defaults: bool,
    /// Stored (non-derived) features in canonical name order. Slot i of an
    /// object of this class holds `stored[i]`.
    pub stored: Box<[FeatureId]>,
    /// All effective features, including derived ones, in name order.
    pub features: Box<[FeatureId]>,
}

const ABSENT: u16 = u16::MAX;

pub struct Metamodel {
    pub(super) revision: Arc<str>,
    pub(super) names: Interner,
    pub(super) classes: Box<[ClassInfo]>,
    pub(super) features: Box<[FeatureInfo]>,
    pub(super) definitions: Box<[Feature]>,
    /// Dense `[class][feature]` -> stored slot.
    slot_of: Box<[u16]>,
    /// Dense `[class][feature]` presence including derived features.
    member_of: Box<[bool]>,
    /// `[class][name]` -> feature for reflective access by name.
    by_name: FxHashMap<(u32, Sym), FeatureId>,
    class_by_key: FxHashMap<Sym, ClassId>,
    /// Source declaration of each feature, for Rcore member reads.
    by_declaration: BTreeMap<DeclarationId, FeatureId>,
    /// Rows: distinct reference targets. Bit c set when class c conforms.
    conformance: Box<[Box<[u64]>]>,
    /// Row per class: bit c set when the class is c or inherits from c.
    supertypes: Box<[Box<[u64]>]>,
    words: usize,
    /// Dense `[class][feature]` -> the feature that redefines it in that class
    /// (MOF property redefinition), or `NO_REDEFINITION`.
    redefined_by: Box<[u32]>,
    /// Retained so attribute values use the canonical datatype validators.
    pub(super) schema: Arc<Schema>,
}

const NO_REDEFINITION: u32 = u32::MAX;

impl Metamodel {
    /// The single validation point for static schema facts. It runs the same
    /// instantiation validator the store runs when binding a schema; datatype
    /// conversions are checked per value by `validate`, as in the store.
    pub fn freeze(schema: &Schema) -> Result<Self, FrozenError> {
        schema
            .validate_instantiations()
            .map_err(FrozenError::Schema)?;
        let mut names = Interner::default();
        let class_count = schema.classes.len();
        let class_u32 =
            |i: usize| u32::try_from(i).map_err(|_| FrozenError::Capacity("metamodel classes"));
        let mut class_by_key = FxHashMap::default();
        for (index, key) in schema.classes.keys().enumerate() {
            class_by_key.insert(names.intern(key)?, ClassId(class_u32(index)?));
        }

        // Global features keyed by declaring owner and name.
        let mut feature_ids: FxHashMap<(u32, Sym), FeatureId> = FxHashMap::default();
        let mut features: Vec<FeatureInfo> = Vec::new();
        let mut definitions: Vec<Feature> = Vec::new();
        let mut opposite_names: Vec<Option<String>> = Vec::new();
        let mut targets: Vec<String> = Vec::new();
        let mut target_rows: FxHashMap<String, u32> = FxHashMap::default();
        let mut class_features: Vec<Vec<FeatureId>> = Vec::with_capacity(class_count);
        let mut by_declaration = BTreeMap::new();
        for key in schema.classes.keys() {
            let mut members = Vec::new();
            for member in schema
                .effective_features(key)
                .map_err(FrozenError::Schema)?
            {
                let owner_sym = names.intern(member.owner)?;
                let owner = *class_by_key.get(&owner_sym).ok_or_else(|| {
                    FrozenError::Schema(format!("Unknown owner {}", member.owner))
                })?;
                let name = names.intern(&member.feature.name)?;
                let id = match feature_ids.get(&(owner.0, name)) {
                    Some(id) => *id,
                    None => {
                        let id = FeatureId(
                            u32::try_from(features.len())
                                .map_err(|_| FrozenError::Capacity("metamodel features"))?,
                        );
                        let f = member.feature;
                        let kind = match (f.reference, f.containment) {
                            (_, true) => FeatureKind::Containment,
                            (true, false) => FeatureKind::Reference,
                            (false, false) => FeatureKind::Attribute,
                        };
                        let target = if kind == FeatureKind::Attribute {
                            Target::Data(names.intern(&f.target)?)
                        } else {
                            let next = u32::try_from(targets.len())
                                .map_err(|_| FrozenError::Capacity("reference targets"))?;
                            let row = *target_rows.entry(f.target.clone()).or_insert_with(|| {
                                targets.push(f.target.clone());
                                next
                            });
                            Target::Classes(row)
                        };
                        let upper = u32::try_from(f.upper).ok();
                        features.push(FeatureInfo {
                            name,
                            owner,
                            kind,
                            target,
                            opposite: None,
                            lower: u32::try_from(f.lower)
                                .map_err(|_| FrozenError::Capacity("lower bound"))?,
                            upper,
                            many: upper != Some(1),
                            unique: f.unique,
                            ordered: f.ordered,
                            derived: f.derived,
                            definition: u32::try_from(definitions.len())
                                .map_err(|_| FrozenError::Capacity("feature definitions"))?,
                        });
                        definitions.push(f.clone());
                        if let MemberIdentity::Source { declaration } = &member.identity {
                            by_declaration.insert(declaration.clone(), id);
                        }
                        opposite_names.push(f.opposite.clone());
                        feature_ids.insert((owner.0, name), id);
                        id
                    }
                };
                members.push((names.resolve(name).to_owned(), id));
            }
            members.sort_by(|a, b| a.0.cmp(&b.0));
            class_features.push(members.into_iter().map(|(_, id)| id).collect());
        }

        let feature_count = features.len();
        let cells = class_count
            .checked_mul(feature_count)
            .ok_or(FrozenError::Capacity("slot table"))?;
        let mut slot_of = vec![ABSENT; cells].into_boxed_slice();
        let mut member_of = vec![false; cells].into_boxed_slice();
        let mut by_name = FxHashMap::default();
        let mut classes = Vec::with_capacity(class_count);
        for (index, (key, class)) in schema.classes.iter().enumerate() {
            let members = class_features
                .get(index)
                .ok_or(FrozenError::Capacity("class features"))?;
            let mut stored = Vec::new();
            let mut has_unsettable = false;
            let mut has_containment_defaults = false;
            for id in members {
                let info = &features[id.index()];
                let cell = index * feature_count + id.index();
                member_of[cell] = true;
                by_name.insert((class_u32(index)?, info.name), *id);
                if !info.derived {
                    let definition = definitions
                        .get(info.definition as usize)
                        .ok_or(FrozenError::Capacity("feature definition"))?;
                    has_unsettable |= definition.unsettable;
                    has_containment_defaults |=
                        crate::core::resources::has_populated_containment_default(definition);
                    slot_of[cell] = u16::try_from(stored.len())
                        .ok()
                        .filter(|s| *s != ABSENT)
                        .ok_or(FrozenError::Capacity("stored features per class"))?;
                    stored.push(*id);
                }
            }
            classes.push(ClassInfo {
                key: names.intern(key)?,
                identity: Arc::from(schema.identity(key)),
                abstract_type: class.abstract_type,
                has_unsettable,
                has_containment_defaults,
                stored: stored.into_boxed_slice(),
                features: members.clone().into_boxed_slice(),
            });
        }

        // Opposites are `owner-key/name` or a bare name in the target class.
        let class_named = |key: &str| {
            names
                .get(key)
                .and_then(|sym| class_by_key.get(&sym))
                .copied()
        };
        for (index, opposite) in opposite_names.iter().enumerate() {
            let Some(opposite) = opposite else { continue };
            let definition = &definitions[features[index].definition as usize];
            let (target, member) = match opposite.rsplit_once('/') {
                Some((owner, member)) if class_named(owner).is_some() => {
                    (class_named(owner), member)
                }
                _ => (class_named(&definition.target), opposite.as_str()),
            };
            let target = target.ok_or_else(|| {
                FrozenError::Schema(format!(
                    "Opposite target {} is not a class",
                    definition.target
                ))
            })?;
            let name = names
                .get(member)
                .ok_or_else(|| FrozenError::Schema(format!("Unknown opposite {opposite}")))?;
            let found = by_name
                .get(&(target.0, name))
                .copied()
                .ok_or_else(|| FrozenError::Schema(format!("Unknown opposite {opposite}")))?;
            features[index].opposite = Some(found);
        }

        // Conformance rows use the canonical schema relation once per pair kind:
        // ancestry for class targets, `Schema::conforms` for anything else.
        let words = class_count.div_ceil(64);
        let mut conformance = Vec::with_capacity(targets.len());
        let ancestry: Vec<Vec<&str>> = schema
            .classes
            .keys()
            .map(|key| schema.ancestry(key).map_err(FrozenError::Schema))
            .collect::<Result<_, _>>()?;
        for target in &targets {
            let mut row = vec![0u64; words].into_boxed_slice();
            for (index, key) in schema.classes.keys().enumerate() {
                let conforms = if schema.classes.contains_key(target) {
                    key == target || ancestry[index].iter().any(|a| a == target)
                } else {
                    schema.conforms(key, target)
                };
                if conforms {
                    row[index / 64] |= 1 << (index % 64);
                }
            }
            conformance.push(row);
        }
        let mut supertypes = Vec::with_capacity(class_count);
        for (index, ancestors) in ancestry.iter().enumerate() {
            let mut row = vec![0u64; words].into_boxed_slice();
            row[index / 64] |= 1 << (index % 64);
            for ancestor in ancestors {
                if let Some(id) = names.get(ancestor).and_then(|sym| class_by_key.get(&sym)) {
                    row[id.index() / 64] |= 1 << (id.index() % 64);
                }
            }
            supertypes.push(row);
        }

        Ok(Self {
            revision: Arc::from(schema.revision.as_str()),
            names,
            classes: classes.into_boxed_slice(),
            features: features.into_boxed_slice(),
            definitions: definitions.into_boxed_slice(),
            slot_of,
            member_of,
            by_name,
            class_by_key,
            by_declaration,
            conformance: conformance.into_boxed_slice(),
            supertypes: supertypes.into_boxed_slice(),
            words,
            redefined_by: vec![NO_REDEFINITION; class_count * feature_count].into_boxed_slice(),
            schema: Arc::new(schema.clone()),
        })
    }

    /// Apply MOF property redefinitions, as (redefining, redefined) feature
    /// declarations (see `definitions::rcore::feature_redefinitions`). In every
    /// class that has both features, the redefined one reads as the redefining
    /// one, and validation and export skip its own empty slot. Chains are
    /// followed to the last redefining feature.
    pub fn with_redefinitions(
        mut self,
        redefinitions: &[(DeclarationId, DeclarationId)],
    ) -> Result<Self, FrozenError> {
        let feature = |declaration: &DeclarationId| {
            self.feature_by_declaration(declaration).ok_or_else(|| {
                FrozenError::Schema(format!(
                    "Redefinition names a feature outside this metamodel: {declaration:?}"
                ))
            })
        };
        let pairs = redefinitions
            .iter()
            .map(|(redefining, redefined)| Ok((feature(redefining)?, feature(redefined)?)))
            .collect::<Result<Vec<_>, FrozenError>>()?;
        let mut table = std::mem::take(&mut self.redefined_by);
        for class in 0..self.classes.len() {
            let class = ClassId(class as u32);
            for (redefining, redefined) in &pairs {
                if self.has_feature(class, *redefining)
                    && self.has_feature(class, *redefined)
                    && let Some(cell) = self.cell(class, *redefined)
                {
                    table[cell] = redefining.0;
                }
            }
            // Follow chains; a cycle cannot be completed within the feature count.
            for redefined in 0..self.features.len() {
                let Some(cell) = self.cell(class, FeatureId(redefined as u32)) else {
                    continue;
                };
                let mut last = table[cell];
                let mut steps = 0;
                while let Some(next) = self
                    .cell(class, FeatureId(last))
                    .and_then(|c| table.get(c))
                    .filter(|next| **next != NO_REDEFINITION)
                {
                    last = *next;
                    steps += 1;
                    if steps > self.features.len() {
                        return Err(FrozenError::Schema(format!(
                            "Redefinition cycle at {}",
                            self.feature_name(FeatureId(redefined as u32))
                        )));
                    }
                }
                table[cell] = last;
            }
        }
        self.redefined_by = table;
        Ok(self)
    }
    /// The feature that redefines `feature` in `class`, if any.
    pub fn redefined_by(&self, class: ClassId, feature: FeatureId) -> Option<FeatureId> {
        let next = *self.redefined_by.get(self.cell(class, feature)?)?;
        (next != NO_REDEFINITION).then_some(FeatureId(next))
    }
    /// The feature a write of attribute `feature` in `class` stores: the
    /// attribute that redefines it in `class` when that one is stored
    /// (`RequirementDefinition.reqId` for `declaredShortName`), as reads go
    /// to its slot (`read_slot`); otherwise `feature` itself.
    pub(crate) fn write_feature(&self, class: ClassId, feature: FeatureId) -> FeatureId {
        match (
            self.feature_info(feature),
            self.redefined_by(class, feature),
        ) {
            (Some(info), Some(redefining))
                if info.kind == FeatureKind::Attribute
                    && self.slot(class, redefining).is_some() =>
            {
                redefining
            }
            _ => feature,
        }
    }
    /// The stored slot that a read of `feature` in `class` uses: the
    /// redefining feature's slot when it is stored, otherwise its own.
    #[inline]
    pub(crate) fn read_slot(&self, class: ClassId, feature: FeatureId) -> Option<usize> {
        self.redefined_by(class, feature)
            .and_then(|redefining| self.slot(class, redefining))
            .or_else(|| self.slot(class, feature))
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }
    pub fn schema(&self) -> &Schema {
        &self.schema
    }
    /// The schema class key of a closed type expression, as
    /// `Schema::class_key_for_type` on `schema()` answers it. `freeze`
    /// validated the schema's instantiations and the retained copy is
    /// immutable, so the lookup does not validate the schema again.
    pub fn class_key_for_type(
        &self,
        ty: &impl AsRef<intersect_model::definitions::generic_types::Type>,
    ) -> Result<&str, String> {
        self.schema.class_key_for_validated_type(ty)
    }
    pub fn class_count(&self) -> usize {
        self.classes.len()
    }
    pub(crate) fn feature_count(&self) -> usize {
        self.features.len()
    }
    pub fn class(&self, key: &str) -> Option<ClassId> {
        self.names
            .get(key)
            .and_then(|sym| self.class_by_key.get(&sym))
            .copied()
    }
    pub fn class_info(&self, class: ClassId) -> Option<&ClassInfo> {
        self.classes.get(class.index())
    }
    pub fn class_key(&self, class: ClassId) -> &str {
        self.classes
            .get(class.index())
            .map_or("", |c| self.names.resolve(c.key))
    }
    pub fn feature_info(&self, feature: FeatureId) -> Option<&FeatureInfo> {
        self.features.get(feature.index())
    }
    pub fn feature_name(&self, feature: FeatureId) -> &str {
        self.features
            .get(feature.index())
            .map_or("", |f| self.names.resolve(f.name))
    }
    pub fn definition(&self, feature: FeatureId) -> Option<&Feature> {
        self.features
            .get(feature.index())
            .and_then(|f| self.definitions.get(f.definition as usize))
    }
    /// Reflective lookup; typed and generated callers hold the FeatureId.
    pub fn feature(&self, class: ClassId, name: &str) -> Option<FeatureId> {
        let name = self.names.get(name)?;
        self.by_name.get(&(class.0, name)).copied()
    }
    /// Feature declared by an exact source declaration (Rcore member identity).
    pub fn feature_by_declaration(&self, declaration: &DeclarationId) -> Option<FeatureId> {
        self.by_declaration.get(declaration).copied()
    }
    /// O(1): is `feature` an effective feature of `class` (derived included)?
    pub(crate) fn has_feature(&self, class: ClassId, feature: FeatureId) -> bool {
        self.cell(class, feature)
            .and_then(|cell| self.member_of.get(cell))
            .copied()
            .unwrap_or(false)
    }
    /// O(1) stored slot of `feature` in objects of `class`.
    #[inline]
    pub fn slot(&self, class: ClassId, feature: FeatureId) -> Option<usize> {
        let slot = *self.slot_of.get(self.cell(class, feature)?)?;
        (slot != ABSENT).then_some(slot as usize)
    }
    /// O(1) reference conformance of an object of `actual` to `feature`'s target.
    #[inline]
    pub fn conforms(&self, actual: ClassId, feature: FeatureId) -> bool {
        let Some(Target::Classes(row)) = self.features.get(feature.index()).map(|f| f.target)
        else {
            return false;
        };
        let index = actual.index();
        self.conformance
            .get(row as usize)
            .and_then(|row| row.get(index / 64))
            .is_some_and(|word| word & (1 << (index % 64)) != 0)
    }
    /// O(1): is `actual` the class `expected` or one of its subclasses?
    #[inline]
    pub fn is_a(&self, actual: ClassId, expected: ClassId) -> bool {
        let index = expected.index();
        self.supertypes
            .get(actual.index())
            .and_then(|row| row.get(index / 64))
            .is_some_and(|word| word & (1 << (index % 64)) != 0)
    }
    /// SHA-256 over every fact a stored model's layout and reads depend on:
    /// the schema revision, each class (key, type identity, abstractness,
    /// stored and effective features in slot order), each feature (name,
    /// owner, kind, target, opposite, bounds, flags and its schema
    /// definition), the redefinition table and the conformance and supertype
    /// bitsets. Two metamodels with the same digest number classes,
    /// features and slots identically, so a model snapshot written under one
    /// reads correctly under the other. Deterministic: fields are hashed in
    /// table order with length prefixes.
    pub fn digest(&self) -> Result<[u8; 32], FrozenError> {
        use sha2::{Digest, Sha256};
        fn text(hash: &mut Sha256, value: &str) {
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value.as_bytes());
        }
        fn word(hash: &mut Sha256, value: u64) {
            hash.update(value.to_le_bytes());
        }
        let mut hash = Sha256::new();
        text(&mut hash, "intersect.frozen.metamodel-digest.v1");
        text(&mut hash, &self.revision);
        word(&mut hash, self.classes.len() as u64);
        for class in self.classes.iter() {
            text(&mut hash, self.names.resolve(class.key));
            text(&mut hash, &class.identity);
            word(&mut hash, u64::from(class.abstract_type));
            word(&mut hash, class.stored.len() as u64);
            for feature in class.stored.iter() {
                word(&mut hash, u64::from(feature.0));
            }
            word(&mut hash, class.features.len() as u64);
            for feature in class.features.iter() {
                word(&mut hash, u64::from(feature.0));
            }
        }
        word(&mut hash, self.features.len() as u64);
        for feature in self.features.iter() {
            text(&mut hash, self.names.resolve(feature.name));
            word(&mut hash, u64::from(feature.owner.0));
            word(
                &mut hash,
                match feature.kind {
                    FeatureKind::Attribute => 0,
                    FeatureKind::Reference => 1,
                    FeatureKind::Containment => 2,
                },
            );
            match feature.target {
                Target::Data(sym) => {
                    word(&mut hash, 0);
                    text(&mut hash, self.names.resolve(sym));
                }
                Target::Classes(row) => {
                    word(&mut hash, 1);
                    word(&mut hash, u64::from(row));
                }
            }
            word(
                &mut hash,
                feature.opposite.map_or(u64::MAX, |f| u64::from(f.0)),
            );
            word(&mut hash, u64::from(feature.lower));
            word(&mut hash, feature.upper.map_or(u64::MAX, u64::from));
            for flag in [
                feature.many,
                feature.unique,
                feature.ordered,
                feature.derived,
            ] {
                word(&mut hash, u64::from(flag));
            }
            let definition = self
                .definitions
                .get(feature.definition as usize)
                .ok_or(FrozenError::UnknownFeature)?;
            let definition = serde_json::to_vec(definition)
                .map_err(|error| FrozenError::Schema(error.to_string()))?;
            hash.update((definition.len() as u64).to_le_bytes());
            hash.update(&definition);
        }
        word(&mut hash, self.redefined_by.len() as u64);
        for cell in self.redefined_by.iter() {
            hash.update(cell.to_le_bytes());
        }
        for rows in [&self.conformance, &self.supertypes] {
            word(&mut hash, rows.len() as u64);
            for row in rows.iter() {
                word(&mut hash, row.len() as u64);
                for bits in row.iter() {
                    word(&mut hash, *bits);
                }
            }
        }
        Ok(hash.finalize().into())
    }
    pub fn table_bytes(&self) -> usize {
        self.slot_of.len() * 2 + self.member_of.len() + self.conformance.len() * self.words * 8
    }
    fn cell(&self, class: ClassId, feature: FeatureId) -> Option<usize> {
        let features = self.features.len();
        if class.index() >= self.classes.len() || feature.index() >= features {
            return None;
        }
        Some(class.index() * features + feature.index())
    }
}

#[cfg(test)]
#[path = "metamodel_tests.rs"]
mod tests;
