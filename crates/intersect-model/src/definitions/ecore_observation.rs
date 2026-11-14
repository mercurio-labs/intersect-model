//! Logical parser allocation preflight; errors retain their original category.
use super::{ErrorKind, ImportError};
use crate::definitions::{
    bundle::{SourceId, SourceSpan},
    conversion::InvocationContext,
};
use std::cell::RefCell;
pub(super) const RECORD_BYTES: usize = crate::definitions::retained_storage::RETAINED_NODE_BYTES;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObservedImportError {
    Import(ImportError),
    Observation(String),
}
impl std::fmt::Display for ObservedImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Import(error) => error.fmt(f),
            Self::Observation(error) => f.write_str(error),
        }
    }
}
impl std::error::Error for ObservedImportError {}

// The parser keeps its existing ImportError control flow. This operation-local
// receipt records observer failure separately; no diagnostic string is inspected
// to recover a failure category. Error construction/cleanup is not metered here.
pub(crate) struct ParserObservation<'a, 'b> {
    context: &'a InvocationContext<'b>,
    failure: RefCell<Option<String>>,
}
impl<'a, 'b> ParserObservation<'a, 'b> {
    pub(crate) fn new(context: &'a InvocationContext<'b>) -> Self {
        Self {
            context,
            failure: RefCell::new(None),
        }
    }
    pub(crate) fn checked(
        &self,
        result: Result<(), String>,
        id: &SourceId,
    ) -> Result<(), ImportError> {
        result.map_err(|message| self.observer_error(message, id))
    }
    fn observer_error(&self, message: String, id: &SourceId) -> ImportError {
        *self.failure.borrow_mut() = Some(message.clone());
        ImportError::new(
            ErrorKind::ResourceLimit,
            &SourceSpan {
                source: id.clone(),
                start: 0,
                end: 0,
            },
            message,
        )
    }
    pub(crate) fn work(&self, units: usize, id: &SourceId) -> Result<(), ImportError> {
        self.checked(self.context.work(units), id)
    }
    // Schema and observer failures remain distinct until the outer importer
    // publishes its typed result. No diagnostic text is used to classify them.
    pub(crate) fn preparation<T>(
        &self,
        id: &SourceId,
        run: impl FnOnce(
            &InvocationContext<'_>,
        ) -> Result<T, crate::definitions::schema::PreparationError>,
    ) -> Result<T, ImportError> {
        match run(self.context) {
            Ok(value) => Ok(value),
            Err(crate::definitions::schema::PreparationError::Observation(message)) => {
                Err(self.observer_error(message, id))
            }
            Err(crate::definitions::schema::PreparationError::Schema(message)) => {
                Err(ImportError::new(
                    ErrorKind::InvalidDefinition,
                    &SourceSpan {
                        source: id.clone(),
                        start: 0,
                        end: 0,
                    },
                    message,
                ))
            }
        }
    }
    // Bridge a nested retained importer without classifying diagnostic text.
    pub(crate) fn imported_result<T>(
        &self,
        source: &SourceId,
        result: Result<T, ObservedImportError>,
    ) -> Result<T, ImportError> {
        match result {
            Ok(value) => Ok(value),
            Err(ObservedImportError::Import(error)) => Err(error),
            Err(ObservedImportError::Observation(message)) => {
                Err(self.observer_error(message, source))
            }
        }
    }
    pub(crate) fn imported<T>(
        &self,
        source: &SourceId,
        run: impl FnOnce(&InvocationContext<'_>) -> Result<T, ObservedImportError>,
    ) -> Result<T, ImportError> {
        self.imported_result(source, run(self.context))
    }
    pub(crate) fn reserve(&self, bytes: usize, id: &SourceId) -> Result<(), ImportError> {
        self.checked(self.context.retain_storage(bytes), id)
    }
    pub(crate) fn preparation_at<T>(
        &self,
        span: &SourceSpan,
        run: impl FnOnce(
            &InvocationContext<'_>,
        ) -> Result<T, crate::definitions::schema::PreparationError>,
    ) -> Result<T, ImportError> {
        match run(self.context) {
            Ok(value) => Ok(value),
            Err(crate::definitions::schema::PreparationError::Observation(message)) => {
                Err(self.observer_error(message, &span.source))
            }
            Err(crate::definitions::schema::PreparationError::Schema(message)) => Err(
                ImportError::new(ErrorKind::InvalidDefinition, span, message),
            ),
        }
    }
    pub(crate) fn string(&self, bytes: usize, id: &SourceId) -> Result<(), ImportError> {
        let bytes = self.size(bytes.checked_add(64), id)?;
        self.reserve(bytes, id)
    }
    pub(crate) fn size(&self, bytes: Option<usize>, id: &SourceId) -> Result<usize, ImportError> {
        match bytes {
            Some(bytes) => Ok(bytes),
            None => {
                self.checked(Err("XML parser storage size overflow".into()), id)?;
                Ok(0)
            }
        }
    }
    pub(super) fn scratch(&self, bytes: usize, id: &SourceId) -> Result<(), ImportError> {
        let bytes = self.size(
            bytes
                .checked_add(64)
                .and_then(|n| n.checked_mul(2))
                .and_then(|n| n.checked_add(RECORD_BYTES + 128)),
            id,
        )?;
        self.reserve(bytes, id)
    }
    pub(super) fn reserve_span(&self, id: &SourceId) -> Result<(), ImportError> {
        // One record and two string slots, plus UTF-8 source identity bytes.
        let bytes = self.size(
            id.uri
                .len()
                .checked_add(id.sha256.len())
                .and_then(|n| n.checked_add(RECORD_BYTES + 128)),
            id,
        )?;
        self.reserve(bytes, id)
    }
    pub(crate) fn span(
        &self,
        id: &SourceId,
        start: usize,
        end: usize,
    ) -> Result<SourceSpan, ImportError> {
        self.reserve_span(id)?;
        Ok(SourceSpan {
            source: id.clone(),
            start,
            end,
        })
    }
    pub(crate) fn finish<T>(
        &self,
        result: Result<T, ImportError>,
    ) -> Result<T, ObservedImportError> {
        match self.failure.borrow_mut().take() {
            Some(message) => Err(ObservedImportError::Observation(message)),
            None => result.map_err(ObservedImportError::Import),
        }
    }
    pub(crate) fn serialized<T: ?Sized + serde::Serialize>(
        &self,
        value: &T,
        id: &SourceId,
    ) -> Result<(), ImportError> {
        if self.context.has_observer() {
            let result =
                crate::definitions::retained_storage::reserve_serialized(value, self.context)
                    .and_then(|bytes| self.context.work(bytes.div_ceil(256)));
            self.checked(result, id)?;
        }
        Ok(())
    }
    pub(crate) fn empty_span(&self) -> Result<SourceSpan, ImportError> {
        let result = self.context.retain_storage(256 + 13 + 64);
        let span = SourceSpan {
            source: SourceId {
                uri: "memory:bundle".into(),
                sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
            },
            start: 0,
            end: 0,
        };
        self.checked(result, &span.source)?;
        Ok(span)
    }
    pub(crate) fn profile_name(
        &self,
        name: &super::ExpandedName,
        id: &SourceId,
    ) -> Result<String, ImportError> {
        use std::fmt::Write;
        struct Count(usize);
        impl Write for Count {
            fn write_str(&mut self, value: &str) -> std::fmt::Result {
                self.0 = self.0.checked_add(value.len()).ok_or(std::fmt::Error)?;
                Ok(())
            }
        }
        let mut count = Count(0);
        self.checked(
            write!(&mut count, "{}", name.profile())
                .map_err(|_| "XML profile name size overflow".into()),
            id,
        )?;
        self.work(count.0.div_ceil(256).max(1), id)?;
        self.string(count.0, id)?;
        Ok(name.profile_name())
    }
    pub(crate) fn static_source(
        &self,
        uri: &str,
        text: &str,
    ) -> Result<crate::definitions::bundle::DefinitionSource, ImportError> {
        let bytes = uri
            .len()
            .checked_add(text.len())
            .and_then(|n| n.checked_add(128 + 3 * 64 + 64));
        let result = match bytes {
            Some(bytes) => self
                .context
                .work(1 + 2 * text.len().div_ceil(256))
                .and_then(|()| self.context.retain_storage(bytes)),
            None => Err("Static definition storage size overflow".into()),
        };
        if let Err(message) = result {
            // This diagnostic identity is created only after a rejected guard.
            return Err(self.observer_error(
                message,
                &SourceId {
                    uri: uri.into(),
                    sha256: String::new(),
                },
            ));
        }
        Ok(crate::definitions::bundle::DefinitionSource::new(
            uri,
            crate::definitions::bundle::SourceKind::Ecore,
            text,
        ))
    }
    pub(crate) fn formatted(
        &self,
        value: &impl std::fmt::Display,
        id: &SourceId,
    ) -> Result<String, ImportError> {
        use std::fmt::Write;
        struct Count(usize);
        impl Write for Count {
            fn write_str(&mut self, text: &str) -> std::fmt::Result {
                self.0 = self.0.checked_add(text.len()).ok_or(std::fmt::Error)?;
                Ok(())
            }
        }
        let mut count = Count(0);
        self.checked(
            write!(&mut count, "{value}").map_err(|_| "Resource URI size overflow".into()),
            id,
        )?;
        let work = self.size(count.0.div_ceil(256).max(1).checked_mul(2), id)?;
        self.work(work, id)?;
        self.string(count.0, id)?;
        Ok(value.to_string())
    }
    pub(crate) fn admit_rcore_document(
        &self,
        source: &crate::definitions::bundle::DefinitionSource,
    ) -> Result<(), ImportError> {
        self.work(source.text.len().max(1), &source.id)?;
        // Existing conservative frontend charge, before retained or direct parsing.
        let bytes = self.size(source.text.len().checked_mul(256), &source.id)?;
        self.reserve(bytes, &source.id)
    }
    pub(crate) fn document_with_limits(
        &self,
        source: &crate::definitions::bundle::DefinitionSource,
        limits: super::ParseLimits,
    ) -> Result<super::Document, ImportError> {
        if source.kind == crate::definitions::bundle::SourceKind::Rcore {
            self.admit_rcore_document(source)?;
            return crate::definitions::rcore::document(source, limits);
        }
        match super::Document::parse_observed_with_limits(source, self.context, limits) {
            Ok(document) => Ok(document),
            Err(ObservedImportError::Import(error)) => Err(error),
            Err(ObservedImportError::Observation(message)) => {
                Err(self.observer_error(message, &source.id))
            }
        }
    }
}
