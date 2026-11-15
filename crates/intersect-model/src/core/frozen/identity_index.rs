//! Lookup-only identity-symbol index. Dense interned symbols need one indexed
//! load rather than a second hash lookup. Sparse symbols retain the hash map;
//! the index is rebuilt on import and never changes the snapshot wire format.
use super::{
    intern::{FxHashMap, Sym},
    model::ObjId,
};

const ABSENT: ObjId = ObjId(u32::MAX);
const SMALL: usize = 64;
const DENSITY: usize = 4;

#[derive(Clone)]
pub(super) enum IdentityIndex {
    Dense { slots: Vec<ObjId>, len: usize },
    Sparse(FxHashMap<Sym, ObjId>),
}

impl Default for IdentityIndex {
    fn default() -> Self {
        Self::Dense {
            slots: Vec::new(),
            len: 0,
        }
    }
}

impl IdentityIndex {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Dense { len, .. } => *len,
            Self::Sparse(map) => map.len(),
        }
    }

    pub(super) fn reserve(&mut self, additional: usize) {
        match self {
            Self::Dense { slots, .. } => slots.reserve(additional),
            Self::Sparse(map) => map.reserve(additional),
        }
    }

    pub(super) fn get(&self, symbol: &Sym) -> Option<&ObjId> {
        match self {
            Self::Dense { slots, .. } => slots.get(symbol.0 as usize).filter(|&&row| row != ABSENT),
            Self::Sparse(map) => map.get(symbol),
        }
    }

    pub(super) fn contains_key(&self, symbol: &Sym) -> bool {
        self.get(symbol).is_some()
    }

    pub(super) fn insert(&mut self, symbol: Sym, row: ObjId) -> Option<ObjId> {
        let index = symbol.0 as usize;
        // Bound sparse-key expansion before allocating. Once sparse, remain so
        // to avoid repeatedly rebuilding an index when a model changes shape.
        if let Self::Dense { slots, len } = self
            && index >= slots.len()
            && index >= SMALL.max(len.saturating_add(1).saturating_mul(DENSITY))
        {
            let map = slots
                .iter()
                .copied()
                .enumerate()
                .filter(|&(_, value)| value != ABSENT)
                .map(|(index, value)| (Sym(index as u32), value))
                .collect();
            *self = Self::Sparse(map);
        }
        match self {
            Self::Sparse(map) => map.insert(symbol, row),
            Self::Dense { slots, len } => {
                if index >= slots.len() {
                    slots.resize(index + 1, ABSENT);
                }
                let previous = std::mem::replace(&mut slots[index], row);
                if previous == ABSENT {
                    *len += 1;
                    None
                } else {
                    Some(previous)
                }
            }
        }
    }

    pub(super) fn remove(&mut self, symbol: &Sym) -> Option<ObjId> {
        match self {
            Self::Sparse(map) => map.remove(symbol),
            Self::Dense { slots, len } => {
                let value = slots.get_mut(symbol.0 as usize)?;
                if *value == ABSENT {
                    return None;
                }
                let previous = std::mem::replace(value, ABSENT);
                *len -= 1;
                Some(previous)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_holes_replacement_deletion_and_reuse_match_the_hash_index() {
        let mut index = IdentityIndex::default();
        let mut expected = FxHashMap::default();
        for round in 0..8 {
            for key in 0..1024 {
                let symbol = Sym(key * 3);
                let row = ObjId(key + round);
                assert_eq!(index.insert(symbol, row), expected.insert(symbol, row));
            }
            for key in (0..1024).step_by(2) {
                let symbol = Sym(key * 3);
                assert_eq!(index.remove(&symbol), expected.remove(&symbol));
                assert_eq!(index.remove(&symbol), None);
            }
            let cloned = index.clone();
            for key in 0..3072 {
                assert_eq!(index.get(&Sym(key)), expected.get(&Sym(key)));
                assert_eq!(cloned.get(&Sym(key)), expected.get(&Sym(key)));
            }
            assert_eq!(index.len(), expected.len());
        }
        assert!(matches!(index, IdentityIndex::Dense { .. }));
    }

    #[test]
    fn sparse_symbols_fall_back_before_expanding_and_keep_existing_rows() {
        let mut index = IdentityIndex::default();
        index.reserve(3);
        index.insert(Sym(0), ObjId(0));
        index.insert(Sym(2), ObjId(1));
        assert_eq!(index.insert(Sym(u32::MAX), ObjId(2)), None);
        let IdentityIndex::Sparse(map) = &index else {
            panic!("sparse symbol expanded the direct table")
        };
        assert_eq!(map.len(), 3);
        assert_eq!(index.get(&Sym(0)), Some(&ObjId(0)));
        assert_eq!(index.get(&Sym(1)), None);
        assert_eq!(index.insert(Sym(2), ObjId(3)), Some(ObjId(1)));
        assert_eq!(index.remove(&Sym(u32::MAX)), Some(ObjId(2)));
        assert_eq!(index.len(), 2);
        assert!(index.contains_key(&Sym(2)));
        assert!(matches!(index.clone(), IdentityIndex::Sparse(_)));
    }
}
