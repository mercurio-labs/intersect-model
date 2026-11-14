//! Exact feature/type resolution for adapters; not model feature-map semantics.
use crate::definitions::{
    bundle::{DefinitionBundle, DefinitionSource},
    compiler::standard_datatype_source,
    ecore::{ECORE, Node, XSI},
    resources::{DeclarationId, ResourceSet},
    scalars::Builtin,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeatureTarget {
    Builtin(Builtin),
    Datatype(DeclarationId),
    Enumeration(DeclarationId),
    Class(DeclarationId),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedFeature {
    pub declaration: DeclarationId,
    pub owner: DeclarationId,
    pub target: FeatureTarget,
    pub reference: bool,
}
pub struct FeatureResolver {
    resources: ResourceSet,
    generics: Option<crate::definitions::generic_resolution::Resolver>,
}
fn attribute<'a>(
    node: &'a Node,
    namespace: Option<&str>,
    local: &str,
) -> Option<&'a crate::definitions::ecore::Attribute> {
    node.attributes
        .iter()
        .find(|a| a.name.namespace.as_deref() == namespace && a.name.local == local)
}
fn kind(node: &Node) -> Result<&str, String> {
    let name = attribute(node, Some(XSI), "type")
        .and_then(|a| a.type_name.as_ref())
        .ok_or("Missing resolved Ecore declaration kind")?;
    if name.namespace.as_deref() != Some(ECORE) {
        return Err("Declaration kind is not in the Ecore namespace".into());
    }
    Ok(&name.local)
}
fn parent(id: &DeclarationId) -> Result<DeclarationId, String> {
    let mut result = id.clone();
    result
        .path
        .pop()
        .ok_or("Document root is not a feature or classifier")?;
    Ok(result)
}
fn classifier(resources: &ResourceSet, id: &DeclarationId) -> Result<String, String> {
    let node = resources.node(id)?;
    if node.name.namespace.is_some() || node.name.local != "eClassifiers" {
        return Err("Feature type does not identify a classifier declaration".into());
    }
    let container = resources.node(&parent(id)?)?;
    if !((container.name.namespace.as_deref() == Some(ECORE) && container.name.local == "EPackage")
        || (container.name.namespace.is_none() && container.name.local == "eSubpackages"))
    {
        return Err("Classifier is not declared directly in an EPackage".into());
    }
    Ok(kind(node)?.into())
}
impl FeatureResolver {
    pub fn new(bundle: &DefinitionBundle) -> Result<Self, String> {
        let resources = ResourceSet::new(bundle)?;
        let has_generic = bundle.sources.iter().any(|s| {
            resources.document(&s.id).is_ok_and(|d| {
                crate::definitions::compiler::generic_schema::contains_generic(&d.root)
            })
        });
        let generics = if has_generic {
            Some(
                crate::definitions::generic_resolution::Resolver::new(bundle)
                    .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };
        Ok(Self {
            resources,
            generics,
        })
    }
    pub fn resolve(&self, id: &DeclarationId) -> Result<ResolvedFeature, String> {
        let node = self.resources.node(id)?;
        if node.name.namespace.is_some() || node.name.local != "eStructuralFeatures" {
            return Err("Selection is not an EStructuralFeature declaration".into());
        }
        let reference = match kind(node)? {
            "EAttribute" => false,
            "EReference" => true,
            _ => return Err("Selection must be an EAttribute or EReference".into()),
        };
        let owner = parent(id)?;
        if classifier(&self.resources, &owner)? != "EClass" {
            return Err("Structural feature owner is not an EClass".into());
        }
        if node
            .children
            .iter()
            .any(|child| child.name.namespace.is_none() && child.name.local == "eGenericType")
        {
            let target_id = self
                .generics
                .as_ref()
                .ok_or("Missing generic resolution context")?
                .concrete_feature_target(id)
                .map_err(|e| e.to_string())?;
            let standard_source = standard_datatype_source();
            let target = if target_id.source == standard_source.id {
                let standard = ResourceSet::new(&DefinitionBundle {
                    version: 1,
                    sources: vec![standard_source],
                    namespaces: vec![],
                })?;
                let node = standard.node(&target_id)?;
                let name =
                    attribute(node, None, "name").ok_or("Missing standard classifier name")?;
                match Builtin::named(&name.value) {
                    Some(builtin) => FeatureTarget::Builtin(builtin),
                    None => target_for(&classifier(&standard, &target_id)?, target_id)?,
                }
            } else {
                target_for(&classifier(&self.resources, &target_id)?, target_id)?
            };
            return Ok(ResolvedFeature {
                declaration: id.clone(),
                owner,
                target,
                reference,
            });
        }
        let ty = attribute(node, None, "eType").ok_or("Feature requires an explicit eType")?;
        let words: Vec<_> = ty.value.split_whitespace().collect();
        let (hint, href) = match words.as_slice() {
            [href] => (None, *href),
            [_, href] => {
                let hint = ty
                    .type_name
                    .as_ref()
                    .ok_or("Missing resolved eType QName")?;
                if hint.namespace.as_deref() != Some(ECORE)
                    || !matches!(hint.local.as_str(), "EClass" | "EDataType" | "EEnum")
                {
                    return Err("Unsupported feature type QName".into());
                }
                (Some(hint.local.as_str()), *href)
            }
            _ => return Err("Malformed feature type reference".into()),
        };
        let (target, actual_kind) =
            if let Some(name) = href.strip_prefix("http://www.eclipse.org/emf/2002/Ecore#//") {
                let source: DefinitionSource = standard_datatype_source();
                let standard = ResourceSet::new(&DefinitionBundle {
                    version: 1,
                    sources: vec![source.clone()],
                    namespaces: vec![],
                })?;
                let target_id = standard.resolve(&source.id, &format!("#//{name}"))?;
                let actual = classifier(&standard, &target_id)?;
                let target = match Builtin::named(name) {
                    Some(codec) => FeatureTarget::Builtin(codec),
                    None => target_for(&actual, target_id)?,
                };
                (target, actual)
            } else {
                let target_id = self.resources.resolve(&id.source, href)?;
                let actual = classifier(&self.resources, &target_id)?;
                (target_for(&actual, target_id)?, actual)
            };
        if hint.is_some_and(|hint| {
            hint != actual_kind && !(hint == "EDataType" && actual_kind == "EEnum")
        }) {
            return Err("Feature eType QName disagrees with resolved classifier kind".into());
        }
        if reference != matches!(target, FeatureTarget::Class(_)) {
            return Err("Attribute/reference kind disagrees with feature target".into());
        }
        Ok(ResolvedFeature {
            declaration: id.clone(),
            owner,
            target,
            reference,
        })
    }
}
fn target_for(kind: &str, id: DeclarationId) -> Result<FeatureTarget, String> {
    match kind {
        "EClass" => Ok(FeatureTarget::Class(id)),
        "EDataType" => Ok(FeatureTarget::Datatype(id)),
        "EEnum" => Ok(FeatureTarget::Enumeration(id)),
        _ => Err("Unsupported feature classifier kind".into()),
    }
}
