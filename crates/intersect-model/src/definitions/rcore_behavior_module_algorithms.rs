//! Shared source/interface/import algorithm, retained without resource callbacks.
use super::*;
use crate::definitions::{bundle::DefinitionSource, continuation::pause};
pub(super) struct ModuleInputs<'b> {
    pub(super) sources: BTreeMap<SourceId, &'b DefinitionSource>,
    pub(super) selected: Vec<&'b DefinitionSource>,
}
pub(super) async fn discover_sources(
    bundle: &DefinitionBundle,
    entry: SourceId,
) -> Result<ModuleInputs<'_>, Vec<SyntaxDiagnostic>> {
    let entry_span = SourceSpan {
        source: entry.clone(),
        start: 0,
        end: 0,
    };
    let mut sources = BTreeMap::new();
    for source in &bundle.sources {
        sources.insert(source.id.clone(), source);
        pause().await;
    }
    if sources
        .get(&entry)
        .is_none_or(|s| !matches!(s.kind, SourceKind::RcoreBehavior | SourceKind::Rcore))
    {
        return Err(vec![diagnostic(
            &entry_span,
            "Entry must select an exact Rcore source",
        )]);
    }
    let mut selected = BTreeSet::new();
    let mut pending = vec![entry.clone()];
    while let Some(id) = pending.pop() {
        pause().await;
        if !selected.insert(id.clone()) {
            continue;
        }
        let source = sources
            .get(&id)
            .ok_or_else(|| vec![diagnostic(&entry_span, "Missing exact module dependency")])?;
        for dependency in &source.dependencies {
            pause().await;
            if sources
                .get(dependency)
                .is_some_and(|s| matches!(s.kind, SourceKind::RcoreBehavior | SourceKind::Rcore))
            {
                pending.push(dependency.clone());
            }
        }
    }
    let mut ordered = Vec::new();
    for id in selected {
        ordered.push(
            *sources
                .get(&id)
                .ok_or_else(|| vec![diagnostic(&entry_span, "Missing selected source")])?,
        );
        pause().await;
    }
    Ok(ModuleInputs {
        sources,
        selected: ordered,
    })
}
pub(super) async fn finish_interfaces(
    bundle: &DefinitionBundle,
    entry: SourceId,
    inputs: ModuleInputs<'_>,
    resources: ResourceSet,
    documents: Vec<(SourceId, Document)>,
) -> Result<(ModuleIndex, ResourceSet), Vec<SyntaxDiagnostic>> {
    let entry_span = SourceSpan {
        source: entry.clone(),
        start: 0,
        end: 0,
    };
    let sources = inputs.sources;
    let mut modules = Vec::new();
    let mut errors = Vec::new();
    for (id, document) in documents {
        pause().await;
        if !document.diagnostics.is_empty() {
            errors.extend(document.diagnostics.clone());
            continue;
        }
        let Some(header) = &document.module else {
            errors.push(diagnostic(&entry_span, "Missing module header"));
            continue;
        };
        if header.value.version.value.trim().is_empty() {
            errors.push(diagnostic(
                &header.value.version.span,
                "Module version must be nonempty",
            ));
        }
        let mut names = BTreeMap::new();
        let mut symbols = Vec::new();
        for (declaration, item) in document.declarations.iter().enumerate() {
            pause().await;
            let (name, kind, public, span) = match &item.value {
                Declaration::Function { name, public, .. } => (
                    vec![name.value.clone()],
                    SymbolKind::Function,
                    *public,
                    name.span.clone(),
                ),
                Declaration::Derive { target, public, .. }
                | Declaration::DerivedContract { target, public, .. } => (
                    path(target),
                    SymbolKind::Derivation,
                    *public,
                    item.span.clone(),
                ),
                Declaration::Validate { target, public, .. } => (
                    path(target),
                    SymbolKind::Validation,
                    *public,
                    item.span.clone(),
                ),
                _ => continue,
            };
            if names
                .get(&name)
                .is_some_and(|prior| *prior != SymbolKind::Function || kind != SymbolKind::Function)
            {
                errors.push(diagnostic(
                    &span,
                    format!("Duplicate declaration {}", name.join(".")),
                ));
            }
            names.insert(name.clone(), kind.clone());
            symbols.push(Symbol {
                id: SymbolId {
                    source: id.clone(),
                    declaration,
                },
                name,
                kind,
                public,
                span,
            });
        }
        modules.push(ModuleInterface {
            path: path(&header.value.name),
            version: header.value.version.value.clone(),
            document,
            symbols,
            imports: BTreeMap::new(),
        });
    }
    if !errors.is_empty() {
        return Err(sorted(errors));
    }
    // All interfaces exist now, including members of import cycles.
    for index in 0..modules.len() {
        pause().await;
        let origin = &modules[index];
        let mut imports = BTreeMap::new();
        let mut aliases = BTreeSet::new();
        for item in &origin.document.declarations {
            pause().await;
            let (alias, binding) = match &item.value {
                Declaration::ModuleImport {
                    path: imported,
                    version,
                    alias,
                } => {
                    let imported_path = path(imported);
                    let mut candidates = Vec::new();
                    for candidate in &modules {
                        pause().await;
                        let mut dependency = false;
                        for selected in &origin.document.source.dependencies {
                            pause().await;
                            if *selected == candidate.document.source.id {
                                dependency = true;
                                break;
                            }
                        }
                        if dependency
                            && candidate.path == imported_path
                            && candidate.version == version.value
                        {
                            candidates.push(candidate);
                        }
                    }
                    let binding = match candidates.as_slice() {
                        [target] => Some(ImportBinding::Module(target.document.source.id.clone())),
                        [] => {
                            errors.push(diagnostic(
                                &alias.span,
                                "No exact dependency matches module path/version",
                            ));
                            None
                        }
                        _ => {
                            errors.push(diagnostic(
                                &alias.span,
                                "Ambiguous module path/version among selected dependencies",
                            ));
                            None
                        }
                    };
                    (alias, binding)
                }
                Declaration::ModelImport { namespace, alias } => {
                    let mut explicit = None;
                    for binding in &bundle.namespaces {
                        pause().await;
                        if binding.namespace == namespace.value {
                            explicit = Some(binding);
                            break;
                        }
                    }
                    let mut candidates = Vec::new();
                    for dependency in origin.document.source.dependencies.iter().chain(
                        (origin.document.source.kind == SourceKind::Rcore)
                            .then_some(&origin.document.source.id),
                    ) {
                        pause().await;
                        if sources.get(dependency).is_none_or(|s| {
                            !matches!(s.kind, SourceKind::Ecore | SourceKind::Rcore)
                        }) {
                            continue;
                        }
                        if explicit.is_some_and(|binding| binding.source != *dependency) {
                            continue;
                        }
                        let packages = resources
                            .packages(dependency)
                            .map_err(|message| vec![diagnostic(&namespace.span, message)])?;
                        if let Some(package) = packages.get(&namespace.value) {
                            candidates.push(package.clone());
                        }
                    }
                    let binding = match candidates.as_slice() {
                        [package] => Some(ImportBinding::Model(package.clone())),
                        [] => {
                            errors.push(diagnostic(
                                &namespace.span,
                                "No exact Ecore dependency declares the imported namespace",
                            ));
                            None
                        }
                        _ => {
                            errors.push(diagnostic(
                                &namespace.span,
                                "Ambiguous model namespace among selected dependencies",
                            ));
                            None
                        }
                    };
                    (alias, binding)
                }
                _ => continue,
            };
            let duplicate_alias = !aliases.insert(alias.value.clone());
            let mut duplicate_symbol = false;
            if !duplicate_alias {
                for symbol in &origin.symbols {
                    pause().await;
                    if symbol.name == [alias.value.clone()] {
                        duplicate_symbol = true;
                        break;
                    }
                }
            }
            if duplicate_alias || duplicate_symbol {
                errors.push(diagnostic(
                    &alias.span,
                    format!("Duplicate import/declaration name {}", alias.value),
                ));
            }
            if let Some(binding) = binding {
                imports.insert(alias.value.clone(), binding);
            }
        }
        modules[index].imports = imports;
    }
    if errors.is_empty() {
        Ok((
            ModuleIndex {
                entry: entry.clone(),
                modules,
            },
            resources,
        ))
    } else {
        Err(sorted(errors))
    }
}
