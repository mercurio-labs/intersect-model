//! Handwritten typed continuation of DefinitionSource's fixed serialization shape.
//! Counting rules and fees remain shared with the metadata serializer.
use super::{DefinitionSource, SourceId, input_chunk_end};
use crate::definitions::{
    conversion::InvocationContext, metadata_limits::TraversalBudget, retained_storage,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Preflight,
    Reserve,
    Dependencies,
    Allocate,
    Copy,
    Complete,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Uri,
    Digest,
    Text,
    DependencyUri,
    DependencyDigest,
}

#[cfg(test)]
#[path = "bundle_source_copy_tests.rs"]
mod tests;
pub(crate) struct AdmittedSourceCopy(DefinitionSource);
impl AdmittedSourceCopy {
    pub(crate) fn into_source(self) -> DefinitionSource {
        self.0
    }
}
pub(crate) struct SourceCopyPreparation<'source> {
    source: &'source DefinitionSource,
    phase: Phase,
    observed: bool,
    budget: TraversalBudget,
    task: usize,
    dependency: usize,
    field: Field,
    offset: usize,
    copied: Option<DefinitionSource>,
    failed: bool,
}
impl<'source> SourceCopyPreparation<'source> {
    pub(crate) fn new(source: &'source DefinitionSource, observed: bool) -> Self {
        Self {
            source,
            phase: if observed {
                Phase::Preflight
            } else {
                Phase::Reserve
            },
            observed,
            budget: TraversalBudget::new(65_536, 64, usize::MAX),
            task: 0,
            dependency: 0,
            field: Field::Uri,
            offset: 0,
            copied: None,
            failed: false,
        }
    }
    pub(crate) fn advance(&mut self, context: &InvocationContext<'_>) -> Result<bool, String> {
        if self.failed {
            return Err("Source-copy preparation has failed".into());
        }
        let result = self.advance_inner(context);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner(&mut self, context: &InvocationContext<'_>) -> Result<bool, String> {
        context.checkpoint()?;
        match self.phase {
            Phase::Preflight => {
                let work = |units| context.work(units);
                match self.task {
                    0 => self.budget.enter_container(4, 0, &work)?,
                    1 => self.budget.field("id", 0, &work)?,
                    2 => self.budget.inspect_at_depth(&self.source.id, 1, &work)?,
                    3 => self.budget.field("kind", 0, &work)?,
                    4 => self.budget.inspect_at_depth(&self.source.kind, 1, &work)?,
                    5 => self.budget.field("text", 0, &work)?,
                    6 => self.budget.inspect_at_depth(&self.source.text, 1, &work)?,
                    7 => self.budget.field("dependencies", 0, &work)?,
                    8 => self
                        .budget
                        .enter_container(self.source.dependencies.len(), 1, &work)?,
                    _ => {
                        if let Some(dependency) = self.source.dependencies.get(self.dependency) {
                            self.budget.inspect_at_depth(dependency, 2, &work)?;
                            self.dependency += 1;
                        } else {
                            self.phase = Phase::Reserve;
                        }
                        return Ok(false);
                    }
                }
                self.task += 1;
            }
            Phase::Reserve => {
                if self.observed {
                    let bytes = retained_storage::bytes_for_budget(&self.budget)?;
                    context.retain_storage(bytes)?;
                    context.work(bytes.div_ceil(256))?;
                }
                self.copied = Some(DefinitionSource {
                    id: SourceId {
                        uri: String::new(),
                        sha256: String::new(),
                    },
                    kind: self.source.kind,
                    text: String::new(),
                    dependencies: Vec::new(),
                });
                self.dependency = 0;
                self.phase = Phase::Dependencies;
            }
            Phase::Dependencies => {
                self.copied
                    .as_mut()
                    .ok_or("Missing owned source")?
                    .dependencies
                    .try_reserve_exact(self.source.dependencies.len())
                    .map_err(|_| "Source dependency allocation failed")?;
                self.phase = Phase::Allocate;
            }
            Phase::Allocate => {
                let length = self.input()?.len();
                self.output()?
                    .try_reserve_exact(length)
                    .map_err(|_| "Source string allocation failed")?;
                self.offset = 0;
                self.phase = Phase::Copy;
            }
            Phase::Copy => {
                let input = self.input()?;
                let end = input_chunk_end(input, self.offset);
                let start = self.offset;
                self.output()?.push_str(&input[start..end]);
                self.offset = end;
                if end == input.len() {
                    self.field = match self.field {
                        Field::Uri => Field::Digest,
                        Field::Digest => Field::Text,
                        Field::Text => {
                            if self.source.dependencies.is_empty() {
                                self.phase = Phase::Complete;
                                return Ok(true);
                            }
                            self.copied
                                .as_mut()
                                .ok_or("Missing owned source")?
                                .dependencies
                                .push(SourceId {
                                    uri: String::new(),
                                    sha256: String::new(),
                                });
                            Field::DependencyUri
                        }
                        Field::DependencyUri => Field::DependencyDigest,
                        Field::DependencyDigest => {
                            self.dependency += 1;
                            if self.dependency == self.source.dependencies.len() {
                                self.phase = Phase::Complete;
                                return Ok(true);
                            }
                            self.copied
                                .as_mut()
                                .ok_or("Missing owned source")?
                                .dependencies
                                .push(SourceId {
                                    uri: String::new(),
                                    sha256: String::new(),
                                });
                            Field::DependencyUri
                        }
                    };
                    self.phase = Phase::Allocate;
                }
            }
            Phase::Complete => return Ok(true),
        }
        Ok(false)
    }
    pub(crate) fn copied_text_bytes(&self) -> usize {
        self.copied.as_ref().map_or(0, |source| source.text.len())
    }
    fn input(&self) -> Result<&'source str, String> {
        Ok(match self.field {
            Field::Uri => &self.source.id.uri,
            Field::Digest => &self.source.id.sha256,
            Field::Text => &self.source.text,
            Field::DependencyUri => {
                &self
                    .source
                    .dependencies
                    .get(self.dependency)
                    .ok_or("Missing source dependency")?
                    .uri
            }
            Field::DependencyDigest => {
                &self
                    .source
                    .dependencies
                    .get(self.dependency)
                    .ok_or("Missing source dependency")?
                    .sha256
            }
        })
    }
    fn output(&mut self) -> Result<&mut String, String> {
        let output = self.copied.as_mut().ok_or("Missing owned source")?;
        Ok(match self.field {
            Field::Uri => &mut output.id.uri,
            Field::Digest => &mut output.id.sha256,
            Field::Text => &mut output.text,
            Field::DependencyUri => {
                &mut output
                    .dependencies
                    .get_mut(self.dependency)
                    .ok_or("Missing owned dependency")?
                    .uri
            }
            Field::DependencyDigest => {
                &mut output
                    .dependencies
                    .get_mut(self.dependency)
                    .ok_or("Missing owned dependency")?
                    .sha256
            }
        })
    }
    pub(crate) fn finish(self) -> Result<AdmittedSourceCopy, String> {
        if self.failed || self.phase != Phase::Complete {
            return Err("Source-copy preparation is incomplete or failed".into());
        }
        self.copied
            .map(AdmittedSourceCopy)
            .ok_or_else(|| "Missing completed source".into())
    }
}
