//! Deterministic module interfaces over exact, explicitly selected source revisions.
//! Interface indexing is separate from lexical binding, typing and execution.
use super::{
    SyntaxDiagnostic,
    ast::{Declaration, Document, Path},
};
use crate::definitions::{
    bundle::{DefinitionBundle, SourceId, SourceKind, SourceSpan},
    resources::{DeclarationId, ResourceSet},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SymbolId {
    pub source: SourceId,
    pub declaration: usize,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolKind {
    Function,
    Derivation,
    Validation,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    pub id: SymbolId,
    pub name: Vec<String>,
    pub kind: SymbolKind,
    pub public: bool,
    pub span: SourceSpan,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImportBinding {
    Module(SourceId),
    Model(DeclarationId),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModuleInterface {
    pub path: Vec<String>,
    pub version: String,
    pub document: Document,
    pub symbols: Vec<Symbol>,
    pub imports: BTreeMap<String, ImportBinding>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModuleIndex {
    pub entry: SourceId,
    pub modules: Vec<ModuleInterface>,
}

impl ModuleIndex {
    pub fn module(&self, source: &SourceId) -> Result<&ModuleInterface, String> {
        self.modules
            .iter()
            .find(|m| &m.document.source.id == source)
            .ok_or_else(|| format!("Unknown module source {}@{}", source.uri, source.sha256))
    }
    /// Resolve a qualified exported declaration through a module-local import alias.
    /// Aliases themselves are not implicitly re-exported.
    pub fn imported_symbol(
        &self,
        from: &SourceId,
        alias: &str,
        name: &[String],
    ) -> Result<&Symbol, String> {
        let symbols = self.imported_symbols(from, alias, name)?;
        match symbols.as_slice() {
            [symbol] => Ok(*symbol),
            _ => Err(format!("Ambiguous exported declaration {}", name.join("."))),
        }
    }
    /// Return every public candidate; declaration order is reporting order only.
    pub fn imported_symbols(
        &self,
        from: &SourceId,
        alias: &str,
        name: &[String],
    ) -> Result<Vec<&Symbol>, String> {
        let origin = self.module(from)?;
        let target = match origin.imports.get(alias) {
            Some(ImportBinding::Module(source)) => self.module(source)?,
            Some(ImportBinding::Model(_)) => {
                return Err("Model import requires classifier resolution".into());
            }
            None => return Err(format!("Unknown import alias {alias}")),
        };
        let matching: Vec<_> = target
            .symbols
            .iter()
            .filter(|symbol| symbol.name == name)
            .collect();
        if matching.is_empty() {
            return Err(format!("Unknown exported declaration {}", name.join(".")));
        }
        let symbols: Vec<_> = matching
            .into_iter()
            .filter(|symbol| symbol.public)
            .collect();
        if symbols.is_empty() {
            return Err(format!("Private declaration {}", name.join(".")));
        }
        Ok(symbols)
    }
}

fn path(path: &Path) -> Vec<String> {
    path.iter().map(|name| name.value.clone()).collect()
}
fn diagnostic(span: &SourceSpan, message: impl Into<String>) -> SyntaxDiagnostic {
    SyntaxDiagnostic {
        span: span.clone(),
        message: message.into(),
    }
}

/// Parse and index every reachable Rcore interface before linking any imports.
/// Cyclic dependency graphs terminate without imposing a source-order preference.
pub fn index_modules(
    bundle: &DefinitionBundle,
    entry: &SourceId,
) -> Result<ModuleIndex, Vec<SyntaxDiagnostic>> {
    index_modules_with_limits(bundle, entry, Default::default())
}

pub(crate) fn index_modules_with_limits(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<ModuleIndex, Vec<SyntaxDiagnostic>> {
    index_modules_with_resources(bundle, entry, limits).map(|(index, _)| index)
}

pub(crate) fn index_modules_with_resources(
    bundle: &DefinitionBundle,
    entry: &SourceId,
    limits: crate::definitions::ecore::ParseLimits,
) -> Result<(ModuleIndex, ResourceSet), Vec<SyntaxDiagnostic>> {
    let context = crate::definitions::conversion::InvocationContext::default();
    let error = |error| match error {
        ModulePreparationError::Diagnostics(errors) => errors,
        ModulePreparationError::Observation(message) => vec![diagnostic(
            &SourceSpan {
                source: entry.clone(),
                start: 0,
                end: 0,
            },
            message,
        )],
    };
    let mut pending =
        ModuleIndexPreparation::begin(bundle, entry, limits, &context).map_err(error)?;
    while !pending.advance(&context).map_err(error)? {}
    pending.finish_with_resources().map_err(error)
}

fn sorted(mut errors: Vec<SyntaxDiagnostic>) -> Vec<SyntaxDiagnostic> {
    errors.sort_by(|a, b| {
        (&a.span.source, a.span.start, a.span.end, &a.message).cmp(&(
            &b.span.source,
            b.span.start,
            b.span.end,
            &b.message,
        ))
    });
    errors
}

#[path = "rcore_behavior_module_algorithms.rs"]
mod algorithms;
use algorithms::{ModuleInputs, discover_sources, finish_interfaces};
#[path = "rcore_behavior_module_preparation.rs"]
mod preparation;
pub use preparation::{ModuleIndexPreparation, ModulePreparationError, ModulePreparationPhase};
