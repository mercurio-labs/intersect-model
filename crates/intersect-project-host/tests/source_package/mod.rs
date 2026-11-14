use intersect_model::{
    definitions::bundle::SourceId,
    project_usages::{PackageCatalog, PackageManifest, Usage},
};
use intersect_project_host::{ManifestError, SourcePackage};

struct Catalog(Vec<PackageManifest>);
impl PackageCatalog for Catalog {
    fn candidates(&self, uri: &str, _: Option<&str>) -> Vec<PackageManifest> {
        self.0.iter().filter(|m| m.uri == uri).cloned().collect()
    }
}

fn source(usages: Vec<Usage>) -> SourcePackage<()> {
    SourcePackage {
        uri: "urn:example:source".into(),
        version: "1".into(),
        provenance: SourceId::for_text("memory:source", "source"),
        content_digest: "source-digest".into(),
        usages,
        payload: (),
    }
}

#[test]
fn manifest_requires_both_build_identities() {
    for (engine, schema) in [("", "schema"), ("engine", ""), (" ", "schema")] {
        assert!(matches!(
            source(vec![]).manifest(engine, schema, &Catalog(vec![])),
            Err(ManifestError::MissingBuildContext)
        ));
    }
}

#[test]
fn manifest_records_the_resolved_lock_and_its_exact_digest() {
    let dependency = source(vec![])
        .manifest("engine", "schema", &Catalog(vec![]))
        .unwrap();
    let mut package = source(vec![Usage {
        resource_uri: dependency.uri.clone(),
        version_constraint: None,
    }]);
    package.uri = "urn:example:consumer".into();
    let manifest = package
        .manifest("engine", "schema", &Catalog(vec![dependency.clone()]))
        .unwrap();
    assert_eq!(manifest.dependency_lock.root_usages(), package.usages);
    let entry = &manifest.dependency_lock.entries()[&dependency.uri];
    assert_eq!(entry.identity, dependency.identity);
    assert!(entry.direct);
    assert_eq!(
        manifest.identity.dependency_lock_digest,
        manifest.dependency_lock.digest().unwrap()
    );
    assert_eq!(manifest.identity.content_digest, package.content_digest);
}
