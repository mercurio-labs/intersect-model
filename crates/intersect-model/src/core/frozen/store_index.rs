//! Mutable postings. Only row order is observable, so every posting is sorted.
use super::super::{
    ClassId,
    intern::{FxHashMap, Sym},
    model::ObjId,
};
use crate::core::storage::{IndexPolicy, Query, query_key};
use serde_json::Value;
use std::{collections::BTreeSet, hash::Hash, sync::OnceLock};

#[derive(Clone, Default)]
pub(super) struct Postings {
    pub classes: FxHashMap<ClassId, BTreeSet<ObjId>>,
    pub resources: FxHashMap<Sym, BTreeSet<ObjId>>,
    pub values: FxHashMap<(String, String), BTreeSet<ObjId>>,
}

impl Postings {
    pub fn update_row(&mut self, row: ObjId, class: ClassId, resource: Sym, add: bool) {
        update_posting(&mut self.classes, class, row, add);
        update_posting(&mut self.resources, resource, row, add);
    }
}

/// The postings of a store. They may be deferred: nothing is indexed until
/// a query reads them, and the first read indexes every row as it then is.
/// Until then row updates are dropped, which that first read makes up for.
#[derive(Clone)]
pub(super) struct Indexes {
    policy: IndexPolicy,
    postings: OnceLock<Postings>,
}

impl Indexes {
    pub fn new(policy: IndexPolicy) -> Self {
        Self {
            policy,
            postings: OnceLock::from(Postings::default()),
        }
    }

    /// Leave the rows now in the store unindexed until the first query.
    /// Only for a store that indexes no values: the deferred indexing
    /// covers class and resource membership.
    pub fn defer(&mut self) {
        if !self.has_values() {
            self.postings = OnceLock::new();
        }
    }

    /// The postings; `index` fills them when they were deferred.
    pub fn postings(&self, index: impl FnOnce(&mut Postings)) -> &Postings {
        self.postings.get_or_init(|| {
            let mut postings = Postings::default();
            index(&mut postings);
            postings
        })
    }

    pub fn empty(&self) -> Self {
        Self::new(self.policy.clone())
    }

    pub fn has_values(&self) -> bool {
        match &self.policy {
            IndexPolicy::AllValues => true,
            IndexPolicy::SelectedValues(fields) => !fields.is_empty(),
        }
    }

    pub fn indexes(&self, field: &str) -> bool {
        self.policy.contains(field)
    }

    /// Whether the postings are kept (not deferred) and took the update.
    pub fn update_row(&mut self, row: ObjId, class: ClassId, resource: Sym, add: bool) -> bool {
        let Some(postings) = self.postings.get_mut() else {
            return false;
        };
        postings.update_row(row, class, resource, add);
        true
    }

    pub fn update_value(&mut self, row: ObjId, field: &str, value: &Value, add: bool) {
        for member in value_members(value) {
            self.update_member(row, field, member, add);
        }
    }

    /// One member of a collection, including an atomic nested JSON value.
    pub fn update_member(&mut self, row: ObjId, field: &str, member: &Value, add: bool) {
        if member.is_null() {
            return;
        }
        let Some(postings) = self.postings.get_mut() else {
            return;
        };
        update_posting(
            &mut postings.values,
            query_key(Query::Value(field, member)),
            row,
            add,
        );
    }
}

pub(super) fn value_members(value: &Value) -> impl Iterator<Item = &Value> {
    value
        .as_array()
        .map_or_else(|| std::slice::from_ref(value), Vec::as_slice)
        .iter()
        .filter(|item| !item.is_null())
}

fn update_posting<K: Eq + Hash>(
    index: &mut FxHashMap<K, BTreeSet<ObjId>>,
    key: K,
    row: ObjId,
    add: bool,
) {
    if add {
        index.entry(key).or_default().insert(row);
    } else if let Some(posting) = index.get_mut(&key) {
        posting.remove(&row);
        if posting.is_empty() {
            index.remove(&key);
        }
    }
}
