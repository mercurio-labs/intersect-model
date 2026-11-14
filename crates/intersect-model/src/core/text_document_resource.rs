//! Explicit source/resource association. Neither this table row nor a document
//! supplies model, semantic-validation, mutation or publication authority.
use super::*;
use crate::core::query::QueryBudget;

/// Native explicit correspondence, separate from the unchanged document/node
/// holders. There is no URI normalization, Deserialize, or model capability.
pub struct DocumentResourceAssociation {
    source: SourceId,
    resource: String,
    retained_bytes: usize,
}
const _: () = assert!(std::mem::size_of::<DocumentResourceAssociation>() <= 80);
impl DocumentResourceAssociation {
    /// Portable logical admission header, independent of target pointer width.
    pub const LOGICAL_HEADER_BYTES: usize = 80;
    pub fn prepare(
        source: (&str, &str),
        resource: &str,
        remaining_bytes: usize,
        budget: &mut QueryBudget,
    ) -> Result<Self, String> {
        budget.charge()?;
        validate_resource_uri(resource)?;
        if source.0.is_empty()
            || source.1.len() != 64
            || !source.1.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("Explicit document resource requires an exact source identity".into());
        }
        let retained_bytes = Self::LOGICAL_HEADER_BYTES
            .checked_add(source.0.len())
            .and_then(|n| n.checked_add(source.1.len()))
            .and_then(|n| n.checked_add(resource.len()))
            .ok_or("Document resource association storage overflow")?;
        budget.charge()?;
        if retained_bytes > remaining_bytes {
            return Err("Document resource association storage budget exceeded".into());
        }
        // Admission precedes all new string copies. Logical units only; this
        // does not bound allocator capacity, Unicode hashing, or process RSS.
        for _ in 0..retained_bytes.div_ceil(64) {
            budget.charge()?;
        }
        Ok(Self {
            source: SourceId {
                uri: source.0.to_owned(),
                sha256: source.1.to_owned(),
            },
            resource: resource.to_owned(),
            retained_bytes,
        })
    }
    /// Shared paid row selection for text and checked physical project reads.
    /// The caller supplies its original work adapter; no allowance is created.
    pub fn select_with_work<'a>(
        rows: &'a [Self],
        source: &SourceId,
        mut charge: impl FnMut() -> Result<(), String>,
    ) -> Result<Option<&'a Self>, String> {
        let mut found = None;
        for row in rows {
            charge()?;
            if row.source().uri != source.uri {
                continue;
            }
            if row.source() != source || found.is_some() {
                return Err("Closed source/resource association source identity mismatch".into());
            }
            found = Some(row);
        }
        Ok(found)
    }
    pub fn source(&self) -> &SourceId {
        &self.source
    }
    pub fn resource_uri(&self) -> &str {
        &self.resource
    }
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }
    fn check_document(&self, document: &TextDocument) -> Result<(), String> {
        if document.source() != &self.source {
            return Err("Document association belongs to another exact raw source".into());
        }
        Ok(())
    }
    pub(in crate::core) fn check_record(&self, record: &ResourceRecord) -> Result<(), String> {
        if record.uri != self.resource
            || record
                .source
                .as_ref()
                .is_none_or(|s| s.uri != self.source.uri || s.revision != self.source.sha256)
        {
            return Err("Associated document resource lacks exact source correspondence".into());
        }
        Ok(())
    }
}

impl DocumentBuilder {
    /// Explicit raw source and independently validated logical resource. The
    /// same initializer/hash/tree worker runs; ordinary APIs keep their checks.
    pub fn from_associated_source(
        association: &DocumentResourceAssociation,
        text: &str,
        revision: u64,
        limits: DocumentLimits,
    ) -> Result<Self, String> {
        let uri = &association.source.uri;
        if text.len() > limits.source_bytes {
            return Err("Document source byte budget exceeded".into());
        }
        let bytes = text
            .len()
            .checked_add(uri.len())
            .and_then(|n| n.checked_add(64))
            .and_then(|n| n.checked_add(logical_debit::BUILDER))
            .and_then(|n| n.checked_add(2 * logical_debit::WORD))
            .ok_or("Document storage size overflow")?;
        if bytes > limits.logical_storage_bytes {
            return Err("Document logical storage budget exceeded".into());
        }
        Self::new_with_association(
            uri.clone(),
            text.to_owned(),
            revision,
            limits,
            Some(association),
        )
    }
}
impl TextDocument {
    pub fn bind_associated_resource(
        mut self,
        association: &DocumentResourceAssociation,
        resource: &ResourceRecord,
    ) -> Result<Self, String> {
        association.check_document(&self)?;
        association.check_record(resource)?;
        self.resource_revision = resource.content_revision;
        Ok(self)
    }
    pub fn check_associated_resource(
        &self,
        association: &DocumentResourceAssociation,
        resource: &ResourceRecord,
    ) -> Result<(), String> {
        association.check_document(self)?;
        association.check_record(resource)?;
        if self.resource_revision != resource.content_revision {
            return Err("Document links require the exact associated resource revision".into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "text_document_resource_tests.rs"]
mod tests;
