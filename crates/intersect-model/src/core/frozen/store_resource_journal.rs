//! Compact undo for the shared resource touch operation. Arbitrary resource
//! replacement still uses a full snapshot, promoted to the frame's first state.
use super::{Frame, FrozenStore, ResourceRecord, StorageResult};
use crate::core::{
    frozen::intern::{FxHashMap, Interner, Sym},
    resources::{ResourceLoadState, SourceCorrespondence, touch_resource_compat},
};

struct Metadata {
    source: Option<SourceCorrespondence>,
    diagnostics: Vec<String>,
}

pub(super) struct Undo {
    revision: u64,
    metadata: Option<Box<Metadata>>,
}

impl Undo {
    fn capture(record: &ResourceRecord) -> Self {
        Self {
            revision: record.content_revision,
            metadata: (record.source.is_some() || !record.diagnostics.is_empty()).then(|| {
                Box::new(Metadata {
                    source: record.source.clone(),
                    diagnostics: record.diagnostics.clone(),
                })
            }),
        }
    }

    pub(super) fn restore(self, record: &mut ResourceRecord) {
        record.content_revision = self.revision;
        if let Some(metadata) = self.metadata {
            record.source = metadata.source;
            record.diagnostics = metadata.diagnostics;
        } else {
            record.source = None;
            record.diagnostics.clear();
        }
    }
}

/// Most parser checkpoints touch just one resource. Keep that entry inline,
/// without allocating a map or copying an already interned resource URI.
#[derive(Default)]
pub(super) struct ResourceTouches {
    first: Option<(Sym, Undo)>,
    more: FxHashMap<Sym, Undo>,
}

impl ResourceTouches {
    fn contains(&self, uri: Sym) -> bool {
        self.first.as_ref().is_some_and(|entry| entry.0 == uri) || self.more.contains_key(&uri)
    }

    fn take(&mut self, uri: Sym) -> Option<Undo> {
        if self.first.as_ref().is_some_and(|entry| entry.0 == uri) {
            self.first.take().map(|entry| entry.1)
        } else {
            self.more.remove(&uri)
        }
    }

    fn insert(&mut self, uri: Sym, undo: Undo) {
        if self.contains(uri) {
            return;
        }
        if self.first.is_none() {
            self.first = Some((uri, undo));
        } else {
            self.more.insert(uri, undo);
        }
    }

    pub(super) fn into_iter(self) -> impl Iterator<Item = (Sym, Undo)> {
        self.first.into_iter().chain(self.more)
    }
}

impl FrozenStore {
    pub(super) fn remember_resource(&mut self, uri: &str) {
        if let Some(frame) = self.frames.last_mut()
            && !frame.records.contains_key(uri)
        {
            let mut record = self.records.get(uri).cloned();
            if let Some(undo) = self
                .builder
                .data
                .strings
                .get(uri)
                .and_then(|sym| frame.touches.take(sym))
                && let Some(record) = &mut record
            {
                undo.restore(record);
            }
            frame.records.insert(uri.to_owned(), record);
        }
    }

    pub(super) fn touch_record(&mut self, uri: &str) -> StorageResult<()> {
        let sym = self.builder.data.strings.get(uri);
        // Preserve the compatibility path's URI admission and error order,
        // including reads/revision checks before the write-transaction check.
        let (Some(frame), Some(record), Some(sym)) =
            (self.frames.last_mut(), self.records.get_mut(uri), sym)
        else {
            return touch_resource_compat(self, uri);
        };
        if !self.validated_resource_uris.contains(uri) {
            return touch_resource_compat(self, uri);
        }
        if record.load_state != ResourceLoadState::Loaded {
            return Err(format!("Resource is not loaded: {uri}"));
        }
        record.next_content_revision()?;
        if !frame.records.contains_key(uri) && !frame.touches.contains(sym) {
            frame.touches.insert(sym, Undo::capture(record));
        }
        record.touch()
    }
}

pub(super) fn merge(
    strings: &Interner,
    parent: &mut Frame,
    records: FxHashMap<String, Option<ResourceRecord>>,
    touches: ResourceTouches,
) {
    for (uri, mut record) in records {
        if parent.records.contains_key(&uri) {
            continue;
        }
        if let Some(undo) = strings.get(&uri).and_then(|sym| parent.touches.take(sym))
            && let Some(record) = &mut record
        {
            // A parent touch implies an existing record. Any later removal
            // first journals that existing record in its own frame.
            undo.restore(record);
        }
        parent.records.insert(uri, record);
    }
    for (sym, undo) in touches.into_iter() {
        if !parent.records.contains_key(strings.resolve(sym)) {
            parent.touches.insert(sym, undo);
        }
    }
}

#[cfg(test)]
#[path = "store_resource_journal_tests.rs"]
mod tests;
