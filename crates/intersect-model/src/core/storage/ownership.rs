//! Ownership evidence produced by the authoritative structural validator.
use super::*;

/// Validated stored containment parents for one immutable model view.
/// This index does not own that view: discard it after any mutation. Unknown IDs
/// and roots both have no entry; callers must establish object existence first.
/// No constructor or deserializer accepts unverified ownership assertions.
pub struct OwnershipIndex {
    pub(super) parents: BTreeMap<String, String>,
    children: BTreeMap<String, Vec<String>>,
    derived_owners: BTreeSet<String>,
    bytes: usize,
    retain_children: bool,
}
impl OwnershipIndex {
    pub(super) fn check_cycles_observed(
        &self,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<()> {
        for start in self.parents.keys() {
            observation.work(1)?;
            super::structural_preparation::reserve_cycle_pair(observation, start, start)?;
            let mut current = start.clone();
            let mut seen = BTreeSet::new();
            while let Some(parent) = self.cycle_next(&current, &mut seen, observation)? {
                current = parent;
            }
            while !seen.is_empty() {
                observation.work(1)?;
                seen.pop_first();
            }
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn page_observed(
        &self,
        object: &str,
        operation: ContainmentNavigation,
        expected: &str,
        cursor: usize,
        schema: &crate::definitions::schema::ScopeSchema<'_>,
        observation: &InvocationContext<'_>,
        mut class: impl FnMut(&str, &InvocationContext<'_>) -> StorageResult<String>,
    ) -> StorageResult<Option<String>> {
        let mut matched = 0usize;
        let mut current = object;
        let mut child = 0usize;
        loop {
            observation.work(1)?;
            let candidate = match operation {
                ContainmentNavigation::Container if child != 0 => None,
                ContainmentNavigation::Container | ContainmentNavigation::Ancestors => {
                    self.parent(current)
                }
                ContainmentNavigation::Contents => {
                    self.children(object).get(child).map(String::as_str)
                }
                ContainmentNavigation::Roots => {
                    return Err("Roots navigation needs the object inventory".into());
                }
            };
            let Some(candidate) = candidate else {
                return Ok(None);
            };
            let actual = class(candidate, observation)?;
            if schema.conforms_observed(&actual, expected, observation)? {
                if matched == cursor {
                    observation.retain_storage(candidate.len())?;
                    return Ok(Some(candidate.to_owned()));
                }
                matched = matched
                    .checked_add(1)
                    .ok_or("Containment result cursor overflow")?;
            }
            current = candidate;
            child = child
                .checked_add(1)
                .ok_or("Containment occurrence cursor overflow")?;
        }
    }
    /// The `cursor`-th uncontained object of `objects` (identity order) that
    /// conforms to `expected`.
    pub(super) fn page_roots_observed<'o>(
        &self,
        objects: impl IntoIterator<Item = &'o String>,
        expected: &str,
        cursor: usize,
        schema: &crate::definitions::schema::ScopeSchema<'_>,
        observation: &InvocationContext<'_>,
        mut class: impl FnMut(&str, &InvocationContext<'_>) -> StorageResult<String>,
    ) -> StorageResult<Option<String>> {
        let mut matched = 0usize;
        for candidate in objects {
            observation.work(1)?;
            if self.parent(candidate).is_some() {
                continue;
            }
            let actual = class(candidate, observation)?;
            if schema.conforms_observed(&actual, expected, observation)? {
                if matched == cursor {
                    observation.retain_storage(candidate.len())?;
                    return Ok(Some(candidate.clone()));
                }
                matched = matched
                    .checked_add(1)
                    .ok_or("Roots result cursor overflow")?;
            }
        }
        Ok(None)
    }
    pub(super) fn new(retain_children: bool) -> Self {
        Self {
            parents: BTreeMap::new(),
            children: BTreeMap::new(),
            derived_owners: BTreeSet::new(),
            bytes: 0,
            retain_children,
        }
    }
    pub fn parent(&self, object: &str) -> Option<&str> {
        self.parents.get(object).map(String::as_str)
    }
    /// Direct children in effective feature order, then stored feature order.
    pub fn children(&self, object: &str) -> &[String] {
        self.children.get(object).map_or(&[], Vec::as_slice)
    }
    /// Derived containment needs a language provider before complete ownership
    /// discovery. Only instantiated owners contribute these unresolved edges.
    pub fn has_derived_owners(&self) -> bool {
        !self.derived_owners.is_empty()
    }
    pub fn has_derived_contents(&self, object: &str) -> bool {
        self.derived_owners.contains(object)
    }
    pub(super) fn derived(
        &mut self,
        owner: &str,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<()> {
        if self.retain_children && !self.derived_owners.contains(owner) {
            let bytes = 64usize
                .checked_add(owner.len())
                .ok_or("Ownership index storage overflow")?;
            let next = self
                .bytes
                .checked_add(bytes)
                .ok_or("Ownership index storage overflow")?;
            observation.retain_storage(bytes)?;
            self.bytes = next;
            self.derived_owners.insert(owner.into());
        }
        Ok(())
    }
    /// Logical retained storage, using a host-independent per-edge allowance.
    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }
    /// Shared single edge of the existing containment-cycle audit. The caller
    /// owns the current path and preserves its own continuation/cleanup order.
    pub(super) fn cycle_next(
        &self,
        current: &str,
        seen: &mut BTreeSet<String>,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<Option<String>> {
        let Some(owner) = self.parents.get(current) else {
            return Ok(None);
        };
        observation.work(1)?;
        if seen.contains(current) {
            return Err("Containment cycle".into());
        }
        super::structural_preparation::reserve_cycle_pair(observation, current, owner)?;
        seen.insert(current.into());
        Ok(Some(owner.clone()))
    }
    pub(super) fn insert(
        &mut self,
        child: &str,
        parent: &str,
        observation: &InvocationContext<'_>,
    ) -> StorageResult<()> {
        if self.parents.contains_key(child) {
            return Err("Multiple containment owners".into());
        }
        let bytes = (if self.retain_children {
            96usize
        } else {
            64usize
        })
        .checked_add(child.len())
        .and_then(|n| n.checked_add(parent.len()))
        .and_then(|n| n.checked_add(if self.retain_children { child.len() } else { 0 }))
        .ok_or("Ownership index storage overflow")?;
        let bytes = if self.retain_children && !self.children.contains_key(parent) {
            bytes
                .checked_add(64)
                .and_then(|n| n.checked_add(parent.len()))
                .ok_or("Ownership index storage overflow")?
        } else {
            bytes
        };
        let next = self
            .bytes
            .checked_add(bytes)
            .ok_or("Ownership index storage overflow")?;
        // Reserve the complete edge and any new parent bucket before owning
        // either identity or extending the child vector. Rejected reservations
        // leave this private index unchanged.
        observation.retain_storage(bytes)?;
        self.bytes = next;
        if self.retain_children {
            if !self.children.contains_key(parent) {
                self.children.insert(parent.into(), Vec::new());
            }
            let children = self
                .children
                .get_mut(parent)
                .ok_or("Missing ownership child bucket")?;
            children
                .try_reserve(1)
                .map_err(|_| "Ownership child allocation failed")?;
            children.push(child.into());
        }
        self.parents.insert(child.into(), parent.into());
        Ok(())
    }
}

/// Validate the full graph and retain the same parent map used by validation.
/// All previous structural/provider obligations remain mandatory.
pub fn validate_ownership_with_conversions(
    store: &(impl ModelRead + ?Sized),
    schema: &intersect_model::definitions::schema::SchemaRepresentation<
        impl intersect_model::definitions::schema::RuntimeAlternative,
    >,
    conversions: &ConversionRegistry<'_>,
) -> StorageResult<OwnershipIndex> {
    validate_structure(store, schema, Some(conversions), None, true, true)
}
/// Observed counterpart; retaining ownership does not revalidate the graph or
/// execute providers a second time. Observation is not cooperative suspension.
pub fn validate_ownership_with_conversions_observed(
    store: &(impl ModelRead + ?Sized),
    schema: &intersect_model::definitions::schema::SchemaRepresentation<
        impl intersect_model::definitions::schema::RuntimeAlternative,
    >,
    conversions: &ConversionRegistry<'_>,
    observation: &InvocationContext<'_>,
) -> StorageResult<OwnershipIndex> {
    validate_structure(
        store,
        schema,
        Some(conversions),
        Some(observation),
        true,
        true,
    )
}

#[cfg(test)]
mod storage_tests {
    use super::*;
    use intersect_model::definitions::conversion::{InvocationObserver, ProviderId};
    use std::{cell::Cell, rc::Rc};
    struct Budget {
        bytes: Cell<usize>,
        calls: Cell<usize>,
        limit: usize,
    }
    impl InvocationObserver for Budget {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            panic!("no provider invocation")
        }
        fn retain_storage(&self, bytes: usize) -> Result<(), String> {
            self.calls.set(self.calls.get() + 1);
            let next = self
                .bytes
                .get()
                .checked_add(bytes)
                .filter(|next| *next <= self.limit)
                .ok_or("ownership storage interrupted")?;
            self.bytes.set(next);
            Ok(())
        }
    }
    fn budget(limit: usize) -> Rc<Budget> {
        Rc::new(Budget {
            bytes: Cell::new(0),
            calls: Cell::new(0),
            limit,
        })
    }
    #[test]
    fn edge_and_marker_reservations_precede_owned_index_mutation() {
        // First bucket/edge 164, same-parent edge 99, UTF-8 new-parent edge
        // 168, derived-owner marker 65: 496 logical bytes in total.
        for limit in [0, 163, 164, 262, 263, 430, 431, 495, 496, 497] {
            let budget = budget(limit);
            let observation = InvocationContext::new(budget.clone());
            let mut index = OwnershipIndex::new(true);
            let mut rejected = false;
            for (child, parent) in [("c", "A"), ("d", "A"), ("é", "r2"), ("", "A")] {
                let before = (
                    index.parents.clone(),
                    index.children.clone(),
                    index.derived_owners.clone(),
                    index.bytes,
                );
                let result = if child.is_empty() {
                    index.derived(parent, &observation)
                } else {
                    index.insert(child, parent, &observation)
                };
                if let Err(error) = result {
                    assert_eq!(error, "ownership storage interrupted");
                    assert_eq!(
                        (
                            index.parents.clone(),
                            index.children.clone(),
                            index.derived_owners.clone(),
                            index.bytes
                        ),
                        before
                    );
                    assert_eq!(index.retained_bytes(), budget.bytes.get());
                    rejected = true;
                    break;
                }
                assert_eq!(index.retained_bytes(), budget.bytes.get());
            }
            assert_eq!(rejected, limit < 496);
            if !rejected {
                assert_eq!(index.retained_bytes(), 496);
                assert_eq!(index.children("A"), ["c", "d"]);
                assert_eq!(index.children("r2"), ["é"]);
                let calls = budget.calls.get();
                index.derived("A", &observation).unwrap();
                assert_eq!(
                    index.insert("c", "A", &observation).unwrap_err(),
                    "Multiple containment owners"
                );
                assert_eq!(budget.calls.get(), calls);
                assert_eq!(budget.bytes.get(), 496);
            }
        }
    }
    #[test]
    fn parent_only_storage_and_overflow_preserve_private_state() {
        let denied = budget(65);
        let mut index = OwnershipIndex::new(false);
        let observation = InvocationContext::new(denied.clone());
        index.derived("A", &observation).unwrap();
        assert_eq!(denied.calls.get(), 0);
        assert!(index.insert("c", "A", &observation).is_err());
        assert_eq!(index.retained_bytes(), 0);
        assert!(index.parents.is_empty());
        let exact = budget(66);
        index
            .insert("c", "A", &InvocationContext::new(exact.clone()))
            .unwrap();
        assert_eq!(index.retained_bytes(), 66);
        assert_eq!(exact.bytes.get(), 66);
        assert_eq!(index.parent("c"), Some("A"));
        assert!(index.children("A").is_empty());
        assert!(!index.has_derived_owners());
        let fresh = budget(usize::MAX);
        index.bytes = usize::MAX;
        let observation = InvocationContext::new(fresh.clone());
        assert_eq!(
            index.insert("d", "A", &observation).unwrap_err(),
            "Ownership index storage overflow"
        );
        assert_eq!(fresh.calls.get(), 0);
        assert_eq!(index.parents.len(), 1);
        assert_eq!(index.bytes, usize::MAX);
        let mut derived = OwnershipIndex::new(true);
        derived.bytes = usize::MAX;
        assert_eq!(
            derived.derived("A", &observation).unwrap_err(),
            "Ownership index storage overflow"
        );
        assert_eq!(fresh.calls.get(), 0);
        assert!(!derived.has_derived_owners());
    }
}
