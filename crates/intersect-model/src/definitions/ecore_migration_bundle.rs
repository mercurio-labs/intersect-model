//! Explicit whole-resource-set migration, including exact dependency and reference rewriting.
use super::*;
use crate::definitions::{
    bundle::{NamespaceBinding, SourceId},
    resources::ResourceSet,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleMigrationReport {
    pub version: u32,
    pub original: DefinitionBundle,
    pub converted: Option<DefinitionBundle>,
    pub reports: Vec<MigrationReport>,
}

/// Successful output owned directly by the canonical native bundle migrator.
/// This is immutable in-process provenance, not model or provider authority.
/// It cannot be reconstructed from a report, serialized data or a source hash.
///
/// ```compile_fail
/// use intersect_model::migration::{BundleMigrationReport, CompletedBundleMigration};
/// fn forged(report: BundleMigrationReport) -> CompletedBundleMigration {
///     CompletedBundleMigration { report }
/// }
/// ```
pub struct CompletedBundleMigration {
    report: BundleMigrationReport,
}
impl CompletedBundleMigration {
    pub fn report(&self) -> &BundleMigrationReport {
        &self.report
    }
    pub fn remap_model_compilation(
        &self,
        original: &crate::definitions::model_definition::ModelCompilation,
    ) -> Result<crate::definitions::model_definition::ModelCompilation, String> {
        self.report.remap_model_compilation(original)
    }
    /// Consume native provenance when an ordinary mutable report is needed.
    /// That report must use the legacy replay boundary if bound again.
    pub fn into_report(self) -> BundleMigrationReport {
        self.report
    }
}

/// Run the SAME native migration and seal only a successful converted bundle.
/// Legacy report-returning migration and deserialized report validation retain
/// their existing behavior. No migration/structural check is skipped here.
pub fn migrate_ecore_bundle_completed_with_limits(
    bundle: DefinitionBundle,
    targets: &BTreeMap<SourceId, String>,
    limits: ParseLimits,
) -> Result<CompletedBundleMigration, ImportError> {
    let report = migrate_ecore_bundle_with_limits(bundle, targets, limits)?;
    if report.converted.is_none() {
        if let Some(issue) = report
            .reports
            .iter()
            .flat_map(|r| &r.issues)
            .find(|issue| issue.blocks_conversion)
        {
            return Err(ImportError::new(
                ErrorKind::InvalidDefinition,
                &issue.span,
                format!(
                    "Native bundle migration did not complete: {}",
                    issue.message
                ),
            ));
        }
        if let Some(source) = report.original.sources.first() {
            return Err(fail(
                source,
                "Native bundle migration has no converted bundle",
            ));
        }
        return Err(ImportError::new(
            ErrorKind::InvalidDefinition,
            &SourceSpan {
                source: SourceId::for_text("migration:empty-bundle", ""),
                start: 0,
                end: 0,
            },
            "Native bundle migration has no converted bundle",
        ));
    }
    Ok(CompletedBundleMigration { report })
}

/// Read-only source/declaration rewriting over retained migration data.
/// This is not native completion provenance, replay verification or activation authority.
/// Model and language recipe adapters share this one source/type remapping implementation.
pub struct BundleSelectionRemapping<'report> {
    report: &'report BundleMigrationReport,
    converted: &'report DefinitionBundle,
}
impl<'report> BundleSelectionRemapping<'report> {
    pub fn converted(&self) -> &'report DefinitionBundle {
        self.converted
    }
    pub fn source_id(&self, id: &SourceId) -> Result<SourceId, String> {
        self.report.original.source(id)?;
        if let Some(report) = self
            .report
            .reports
            .iter()
            .find(|report| report.original.id == *id)
        {
            return report
                .converted
                .as_ref()
                .map(|source| source.id.clone())
                .ok_or_else(|| "Missing converted migration source".into());
        }
        self.converted.source(id)?;
        Ok(id.clone())
    }
    pub fn materialized_types(
        &self,
        original: &[crate::definitions::generic_types::RuntimeType],
    ) -> Result<Vec<crate::definitions::generic_types::RuntimeType>, String> {
        use crate::definitions::generic_types::{RuntimeType, Type};
        let declarations = self
            .report
            .reports
            .iter()
            .flat_map(|report| &report.correspondence)
            .map(|row| (&row.original, &row.converted))
            .collect::<BTreeMap<_, _>>();
        fn remap(
            ty: &Type,
            declarations: &BTreeMap<&DeclarationId, &DeclarationId>,
            migrated: &BTreeSet<&SourceId>,
        ) -> Result<Type, String> {
            let Type::Classifier {
                declaration,
                arguments,
                raw,
            } = ty
            else {
                return Err("Runtime migration selection requires classifier applications".into());
            };
            let selected = match declarations.get(declaration) {
                Some(converted) => (*converted).clone(),
                None if migrated.contains(&declaration.source) => {
                    return Err("Missing exact runtime declaration correspondence".into());
                }
                None => declaration.clone(),
            };
            Ok(Type::Classifier {
                declaration: selected,
                raw: *raw,
                arguments: arguments
                    .iter()
                    .map(|ty| remap(ty, declarations, migrated))
                    .collect::<Result<_, _>>()?,
            })
        }
        let migrated = self
            .report
            .reports
            .iter()
            .map(|report| &report.original.id)
            .collect();
        original
            .iter()
            .map(|ty| RuntimeType::new(remap(ty.expression(), &declarations, &migrated)?))
            .collect()
    }
}
impl BundleMigrationReport {
    /// Check exact original/converted worlds before adapting retained selectors.
    /// Serialized correspondence remains data; use native completion or replay for origin authority.
    pub fn selection_remapping(
        &self,
        original: &DefinitionBundle,
    ) -> Result<BundleSelectionRemapping<'_>, String> {
        if self.version != 1 {
            return Err("Unsupported bundle migration report version".into());
        }
        self.original.validate()?;
        if original != &self.original {
            return Err("Compilation does not identify the migration's original bundle".into());
        }
        let converted = self
            .converted
            .as_ref()
            .ok_or("Migration has blocking issues")?;
        converted.validate()?;
        Ok(BundleSelectionRemapping {
            report: self,
            converted,
        })
    }
    /// Carry model structure, behavior and closed type selections across a migration.
    /// This compiles neither behavior nor syntax and installs no providers.
    pub fn remap_model_compilation(
        &self,
        original: &crate::definitions::model_definition::ModelCompilation,
    ) -> Result<crate::definitions::model_definition::ModelCompilation, String> {
        let mapping = self.selection_remapping(&original.bundle)?;
        let materialized_types = mapping.materialized_types(&original.materialized_types)?;
        Ok(crate::definitions::model_definition::ModelCompilation {
            bundle: mapping.converted().clone(),
            structure: mapping.source_id(&original.structure)?,
            behavior: mapping.source_id(&original.behavior)?,
            namespace: original.namespace.clone(),
            materialized_types,
        })
    }
}

/// Migrate Ecore sources in an explicit resource set. Every Ecore input needs an
/// output URI. Other source bytes/IDs are retained, with dependency edges updated;
/// this does not claim that their grammar or behavior has been compiled.
pub fn migrate_ecore_bundle(
    bundle: DefinitionBundle,
    targets: &BTreeMap<SourceId, String>,
) -> Result<BundleMigrationReport, ImportError> {
    migrate_ecore_bundle_with_limits(bundle, targets, ParseLimits::default())
}

/// Migrate a complete bundle under the same explicit per-source size policy.
pub fn migrate_ecore_bundle_with_limits(
    bundle: DefinitionBundle,
    targets: &BTreeMap<SourceId, String>,
    limits: ParseLimits,
) -> Result<BundleMigrationReport, ImportError> {
    let resources = ResourceSet::new_with_limits(&bundle, limits)?;
    for id in targets.keys() {
        if !bundle
            .sources
            .iter()
            .any(|s| s.id == *id && s.kind == SourceKind::Ecore)
        {
            return Err(ImportError::new(
                ErrorKind::InvalidDefinition,
                &SourceSpan {
                    source: id.clone(),
                    start: 0,
                    end: 0,
                },
                "Migration target must identify an exact Ecore input",
            ));
        }
    }
    let mut used = BTreeSet::new();
    let mut output = BundleMigrationReport {
        version: 1,
        original: bundle.clone(),
        converted: None,
        reports: vec![],
    };
    let mut converted = Vec::new();
    for source in &bundle.sources {
        if source.kind != SourceKind::Ecore {
            converted.push(source.clone());
            continue;
        }
        let document = resources
            .document(&source.id)
            .map_err(|e| fail(source, e))?;
        let target = targets
            .get(&source.id)
            .ok_or_else(|| fail(source, "Missing explicit output URI"))?;
        if target == &source.id.uri
            || !used.insert(target)
            || bundle.sources.iter().any(|s| &s.id.uri == target)
        {
            return Err(fail(
                source,
                "Migration requires Ecore inputs and distinct new output URIs",
            ));
        }
        let mut writer = Writer {
            source,
            issues: vec![],
            references: BTreeMap::new(),
        };
        rewrite_references(
            &document.root,
            source,
            &resources,
            targets,
            &mut writer.references,
        )?;
        if !supported_root(&document.root) {
            writer.unsupported(&document.root);
        }
        let draft = writer.package(&document.root, true);
        converted.push(DefinitionSource::new(target, SourceKind::Rcore, &draft));
        output.reports.push(MigrationReport {
            version: 1,
            original: source.clone(),
            draft,
            converted: None,
            correspondence: vec![],
            issues: writer.issues,
        });
    }
    let ids: BTreeMap<_, _> = bundle
        .sources
        .iter()
        .zip(&converted)
        .map(|(a, b)| (a.id.clone(), b.id.clone()))
        .collect();
    for (a, b) in bundle.sources.iter().zip(&mut converted) {
        b.dependencies = a
            .dependencies
            .iter()
            .map(|id| {
                ids.get(id)
                    .cloned()
                    .ok_or_else(|| fail(a, "Missing migrated dependency"))
            })
            .collect::<Result<_, _>>()?;
    }
    let namespaces = bundle
        .namespaces
        .iter()
        .map(|n| {
            Ok(NamespaceBinding {
                namespace: n.namespace.clone(),
                source: ids.get(&n.source).cloned().ok_or_else(|| {
                    ImportError::new(
                        ErrorKind::InvalidDefinition,
                        &SourceSpan {
                            source: n.source.clone(),
                            start: 0,
                            end: 0,
                        },
                        "Missing migrated namespace source",
                    )
                })?,
            })
        })
        .collect::<Result<_, ImportError>>()?;
    let target_bundle = DefinitionBundle {
        version: 1,
        sources: converted,
        namespaces,
    };
    if output
        .reports
        .iter()
        .any(|r| r.issues.iter().any(|i| i.blocks_conversion))
    {
        return Ok(output);
    }
    let target_resources = ResourceSet::new_with_limits(&target_bundle, limits)?;
    for report in &mut output.reports {
        let target = target_bundle
            .sources
            .iter()
            .find(|s| ids.get(&report.original.id) == Some(&s.id))
            .ok_or_else(|| fail(&report.original, "Missing converted source"))?;
        let original = resources
            .document(&report.original.id)
            .map_err(|e| fail(&report.original, e))?;
        let namespace = value(&original.root, "nsURI").unwrap_or("");
        if !validate_definitions(
            &bundle,
            &report.original.id,
            namespace,
            "invalid_original",
            &mut report.issues,
            limits,
        ) || !validate_definitions(
            &target_bundle,
            &target.id,
            namespace,
            "target_rejected",
            &mut report.issues,
            limits,
        ) {
            continue;
        }
        let target_doc = target_resources
            .document(&target.id)
            .map_err(|e| fail(target, e))?;
        mappings(
            &original.root,
            &target_doc.root,
            vec![],
            vec![],
            &mut report.correspondence,
        );
        report.converted = Some(target.clone());
    }
    if output.reports.iter().all(|r| r.converted.is_some()) {
        output.converted = Some(target_bundle);
    }
    Ok(output)
}

fn fail(source: &DefinitionSource, message: impl Into<String>) -> ImportError {
    ImportError::new(
        ErrorKind::InvalidDefinition,
        &SourceSpan {
            source: source.id.clone(),
            start: 0,
            end: source.text.len(),
        },
        message,
    )
}
pub(super) fn rewrite_references(
    node: &Node,
    source: &DefinitionSource,
    resources: &ResourceSet,
    targets: &BTreeMap<SourceId, String>,
    out: &mut BTreeMap<String, String>,
) -> Result<(), ImportError> {
    for attribute in &node.attributes {
        if attribute.name.namespace.is_some() {
            continue;
        }
        // Type positions may be written as Rcore names; feature and type
        // parameter references keep their exact positional fragment.
        let type_position = match attribute.name.local.as_str() {
            "eType" | "eClassifier" | "eSuperTypes" | "eExceptions" => true,
            "href" => node.name.local != "eOpposite",
            _ => false,
        };
        let refs: Vec<_> = match attribute.name.local.as_str() {
            "eType" | "eClassifier" | "eTypeParameter" | "eOpposite" => attribute
                .value
                .split_whitespace()
                .last()
                .into_iter()
                .collect(),
            "eSuperTypes" => attribute
                .value
                .split_whitespace()
                .filter(|word| !word.contains(':') || word.contains('/') || word.contains('#'))
                .collect(),
            "eExceptions" | "references" => attribute.value.split_whitespace().collect(),
            "href"
                if ["eSuperTypes", "eType", "eOpposite", "eExceptions"]
                    .contains(&node.name.local.as_str()) =>
            {
                vec![attribute.value.as_str()]
            }
            _ => vec![],
        };
        for href in refs {
            if href.starts_with(&format!("{ECORE}#")) {
                continue;
            }
            let declaration = resources
                .resolve(&source.id, href)
                .map_err(|e| ImportError::new(ErrorKind::InvalidDefinition, &attribute.span, e))?;
            let document = resources
                .document(&declaration.source)
                .map_err(|e| fail(source, e))?;
            let target_uri = match targets.get(&declaration.source) {
                Some(uri) => uri,
                None if document.source.kind == SourceKind::Rcore => &document.source.id.uri,
                None => return Err(fail(source, "Reference has no explicit migrated target")),
            };
            let fragment = target_fragment(&document.root, &declaration.path)
                .ok_or_else(|| fail(source, "Reference cannot be mapped to Rcore containment"))?;
            let uri = if declaration.source == source.id {
                ""
            } else {
                target_uri
            };
            let named = (type_position && declaration.source == source.id)
                .then(|| named_classifier(&document.root, &declaration.path))
                .flatten();
            out.insert(
                href.into(),
                named.unwrap_or_else(|| format!("{uri}{fragment}")),
            );
        }
    }
    for child in &node.children {
        rewrite_references(child, source, resources, targets, out)?;
    }
    Ok(())
}
/// The `#//package/Classifier` spelling of a same-source classifier, when the
/// path is a chain of uniquely named subpackages ending in a uniquely named
/// classifier under a single root package. Anything else keeps its positional
/// fragment, which stays exact.
fn named_classifier(root: &Node, path: &[usize]) -> Option<String> {
    if root.name.namespace.as_deref() != Some(ECORE) || root.name.local != "EPackage" {
        return None;
    }
    let (last, packages) = path.split_last()?;
    let mut node = root;
    let mut segments = Vec::with_capacity(path.len());
    for (position, &index) in packages.iter().chain([last]).enumerate() {
        let child = node.children.get(index)?;
        let wanted = if position == packages.len() {
            "eClassifiers"
        } else {
            "eSubpackages"
        };
        if child.name.namespace.is_some() || child.name.local != wanted {
            return None;
        }
        let name = value(child, "name")
            .filter(|name| !name.is_empty() && !name.contains(['/', '@', '#', '%', ' ']))?;
        let duplicates = node
            .children
            .iter()
            .filter(|n| n.name.namespace.is_none() && n.name.local == wanted)
            .filter(|n| value(n, "name") == Some(name))
            .count();
        if duplicates != 1 {
            return None;
        }
        segments.push(name);
        node = child;
    }
    Some(format!("#//{}", segments.join("/")))
}
fn target_fragment(root: &Node, path: &[usize]) -> Option<String> {
    let mut node = root;
    let mut result = String::from("#/");
    for &index in path {
        let child = node.children.get(index)?;
        if node.name.namespace.as_deref() == Some(XMI) && node.name.local == "XMI" {
            result.push_str(&index.to_string());
            node = child;
            continue;
        }
        if child.name.namespace.is_some() {
            return None;
        }
        let ordinal = if child.name.local == "eClassifiers" {
            let rank = |n: &Node| match kind(n) {
                Some("EClass") => 0,
                Some("EEnum") => 1,
                _ => 2,
            };
            node.children
                .iter()
                .enumerate()
                .filter(|(i, n)| {
                    n.name.local == "eClassifiers"
                        && (rank(n) < rank(child) || (rank(n) == rank(child) && *i < index))
                })
                .count()
        } else {
            node.children[..index]
                .iter()
                .filter(|n| n.name == child.name)
                .count()
        };
        result.push_str(&format!("/@{}.{ordinal}", child.name.local));
        node = child;
    }
    Some(result)
}
