//! Handwritten quick controls using a language-neutral test schema.
use intersect_model::{
    core::{
        frozen::{ById, FrozenStore, Metamodel, Model as FrozenModel, ModelBuilder, SnapshotTrust},
        model::Model,
        package_load::{PackageLoadError, PackagePayload, load_against_lock},
        project_usages::{
            DependencyLock, PackageCatalog, PackageIdentity, PackageManifest, Usage, UsageResolver,
        },
        resource_set::ResourceSet,
        resources::{resource_package, resources_of_package},
        storage,
    },
    definitions::bundle::SourceId,
};
use std::{collections::BTreeMap, sync::Arc};

fn metamodel() -> Arc<Metamodel> {
    let package = intersect_model::compile_rcore(
        r#"rcore 1;
package sample version "1";
metamodel "urn:package-test" prefix t { class Node { attr name: String [0,1]; } }
"#,
    )
    .unwrap();
    Arc::new(Metamodel::freeze(package.schema()).unwrap())
}

fn identity(content: &str) -> PackageIdentity {
    PackageIdentity {
        content_digest: content.into(),
        engine: "engine-1".into(),
        schema: "schema-1".into(),
        dependency_lock_digest: DependencyLock::empty().digest().unwrap(),
    }
}

fn model(mm: &Arc<Metamodel>, id: &str, uri: &str) -> FrozenModel {
    let class = mm
        .schema()
        .declarations
        .iter()
        .find(|(key, declaration)| {
            declaration.name == "Node" && mm.schema().classes.contains_key(*key)
        })
        .and_then(|(key, _)| mm.class(key))
        .unwrap();
    let mut builder = ModelBuilder::new(mm.clone());
    let object = builder.create(id, class, uri).unwrap();
    builder
        .set_str(object, mm.feature(class, "name").unwrap(), id)
        .unwrap();
    builder.link(&ById).unwrap().validate().unwrap()
}

fn empty_project(mm: &Metamodel) -> Model {
    Model {
        schema_revision: mm.revision().into(),
        elements: vec![],
        resources: BTreeMap::new(),
    }
}

struct Catalog(Vec<PackageManifest>);
impl PackageCatalog for Catalog {
    fn candidates(&self, uri: &str, _: Option<&str>) -> Vec<PackageManifest> {
        self.0
            .iter()
            .filter(|manifest| manifest.uri == uri)
            .cloned()
            .collect()
    }
}
fn lock(entries: &[(&str, &PackageIdentity)]) -> DependencyLock {
    let catalog = Catalog(
        entries
            .iter()
            .map(|(uri, identity)| PackageManifest {
                uri: (*uri).into(),
                version: "1".into(),
                identity: (*identity).clone(),
                usages: vec![],
                provenance: SourceId::for_text(format!("source:{uri}"), uri),
                dependency_lock: DependencyLock::empty(),
            })
            .collect(),
    );
    UsageResolver::resolve(
        &entries
            .iter()
            .map(|(uri, _)| Usage {
                resource_uri: (*uri).into(),
                version_constraint: None,
            })
            .collect::<Vec<_>>(),
        &catalog,
    )
    .unwrap()
}

#[test]
fn provenance_survives_snapshot_and_materialized_round_trips() {
    let mm = metamodel();
    let identity = identity("content");
    let tagged = model(&mm, "a", "memory:/a.xmi")
        .with_package_identity(&identity)
        .unwrap();
    let bytes = tagged.to_snapshot(&[7; 32]).unwrap();
    let restored =
        FrozenModel::from_snapshot(mm.clone(), &bytes, &[7; 32], SnapshotTrust::Revalidate)
            .unwrap();
    assert_eq!(restored.resource_package("memory:/a.xmi"), Some(&identity));
    assert_eq!(restored.to_model().unwrap(), tagged.to_model().unwrap());
    let wire = serde_json::to_vec(&restored.to_model().unwrap()).unwrap();
    let materialized: Model = serde_json::from_slice(&wire).unwrap();
    let store = FrozenStore::from_materialized(mm.schema(), &materialized).unwrap();
    assert_eq!(
        resource_package(&store, "memory:/a.xmi").unwrap(),
        Some(identity.clone())
    );
    assert_eq!(
        resources_of_package(&store, &identity).unwrap(),
        ["memory:/a.xmi"]
    );
    let mut legacy = serde_json::to_value(materialized.resources["memory:/a.xmi"].clone()).unwrap();
    legacy.as_object_mut().unwrap().remove("package");
    let record: intersect_model::core::resources::ResourceRecord =
        serde_json::from_value(legacy).unwrap();
    assert!(record.package.is_none());
}

#[test]
fn loads_aliases_once_and_keeps_project_resources_untagged() {
    let mm = metamodel();
    let identity = identity("content");
    let payload = model(&mm, "a", "memory:/package.xmi")
        .with_package_identity(&identity)
        .unwrap();
    let lock = lock(&[("urn:one", &identity), ("urn:alias", &identity)]);
    let project = model(&mm, "p", "memory:/project.xmi").to_model().unwrap();
    let loaded = load_against_lock(
        mm.clone(),
        &lock,
        &[PackagePayload {
            identity: &identity,
            model: &payload,
        }],
        &project,
    )
    .unwrap();
    assert_eq!(loaded.context.dependencies().len(), 2);
    let assembled = storage::export(&loaded.store).unwrap();
    assert_eq!(assembled.schema_revision, mm.revision());
    assert_eq!(assembled.elements.len(), 2);
    assert_eq!(
        resources_of_package(&loaded.store, &identity).unwrap(),
        ["memory:/package.xmi"]
    );
    assert_eq!(
        resource_package(&loaded.store, "memory:/project.xmi").unwrap(),
        None
    );
    assert_eq!(
        payload.resources_of_package(&identity),
        ["memory:/package.xmi"]
    );
    assert_eq!(
        resource_package(&loaded.store, "memory:/missing.xmi").unwrap(),
        None
    );
}

#[test]
fn rejects_missing_payload() {
    let mm = metamodel();
    let identity = identity("a");
    let lock = lock(&[("urn:a", &identity)]);
    assert!(
        matches!(load_against_lock(mm.clone(), &lock, &[], &empty_project(&mm)),
        Err(PackageLoadError::MissingPayload { uris, .. }) if uris == ["urn:a"])
    );
}

#[test]
fn an_empty_package_resource_still_carries_provenance() {
    let mm = metamodel();
    let identity = identity("empty");
    let mut wire = empty_project(&mm);
    wire.resources.insert(
        "memory:/empty.xmi".into(),
        intersect_model::core::resources::ResourceRecord::new("memory:/empty.xmi").unwrap(),
    );
    let tagged = FrozenModel::from_model(mm.clone(), &wire)
        .unwrap()
        .with_package_identity(&identity)
        .unwrap();
    let bytes = tagged.to_snapshot(&[8; 32]).unwrap();
    let restored =
        FrozenModel::from_snapshot(mm.clone(), &bytes, &[8; 32], SnapshotTrust::Revalidate)
            .unwrap();
    let lock = lock(&[("urn:empty", &identity)]);
    let loaded = load_against_lock(
        mm.clone(),
        &lock,
        &[PackagePayload {
            identity: &identity,
            model: &restored,
        }],
        &empty_project(&mm),
    )
    .unwrap();
    assert_eq!(
        resources_of_package(&loaded.store, &identity).unwrap(),
        ["memory:/empty.xmi"]
    );
}

#[test]
fn rejects_a_payload_with_another_resource_identity() {
    let mm = metamodel();
    let identity = identity("a");
    let other = self::identity("b");
    let lock = lock(&[("urn:a", &identity)]);
    let payload = model(&mm, "a", "memory:/a.xmi")
        .with_package_identity(&other)
        .unwrap();
    assert!(matches!(
        load_against_lock(
            mm.clone(),
            &lock,
            &[PackagePayload {
                identity: &identity,
                model: &payload
            }],
            &empty_project(&mm)
        ),
        Err(PackageLoadError::IdentityMismatch { .. })
    ));
}

#[test]
fn rejects_a_tagged_resource_outside_the_lock() {
    let mm = metamodel();
    let identity = identity("a");
    let project = model(&mm, "a", "memory:/a.xmi")
        .with_package_identity(&identity)
        .unwrap()
        .to_model()
        .unwrap();
    assert!(matches!(
        load_against_lock(mm, &DependencyLock::empty(), &[], &project),
        Err(PackageLoadError::TaggedNotInLock { .. })
    ));
}

#[test]
fn multiple_payloads_are_order_independent_and_reject_overlapping_resources() {
    let mm = metamodel();
    let a = identity("a");
    let b = identity("b");
    let first = model(&mm, "a", "memory:/a.xmi")
        .with_package_identity(&a)
        .unwrap();
    let second = model(&mm, "b", "memory:/b.xmi")
        .with_package_identity(&b)
        .unwrap();
    let lock = lock(&[("urn:a", &a), ("urn:b", &b)]);
    let project = empty_project(&mm);
    let load = |reverse| {
        let mut payloads = vec![
            PackagePayload {
                identity: &a,
                model: &first,
            },
            PackagePayload {
                identity: &b,
                model: &second,
            },
        ];
        if reverse {
            payloads.reverse();
        }
        let loaded = load_against_lock(mm.clone(), &lock, &payloads, &project).unwrap();
        storage::export(&loaded.store).unwrap()
    };
    assert_eq!(load(false), load(true));
    let duplicate = model(&mm, "c", "memory:/a.xmi")
        .with_package_identity(&b)
        .unwrap();
    assert!(matches!(
        load_against_lock(
            mm,
            &lock,
            &[
                PackagePayload {
                    identity: &a,
                    model: &first
                },
                PackagePayload {
                    identity: &b,
                    model: &duplicate
                }
            ],
            &project
        ),
        Err(PackageLoadError::Invalid(_))
    ));
}

#[test]
fn replacement_retains_package_origin_and_rejects_retagging_atomically() {
    let mm = metamodel();
    let identity = identity("a");
    let lock = lock(&[("urn:a", &identity)]);
    let payload = model(&mm, "a", "memory:/a.xmi")
        .with_package_identity(&identity)
        .unwrap();
    let loaded = load_against_lock(
        mm.clone(),
        &lock,
        &[PackagePayload {
            identity: &identity,
            model: &payload,
        }],
        &empty_project(&mm),
    )
    .unwrap();
    let mut set =
        ResourceSet::from_store(mm.schema().clone(), loaded.context.clone(), loaded.store).unwrap();
    let mut incoming = payload.to_model().unwrap();
    incoming.resources.get_mut("memory:/a.xmi").unwrap().package = None;
    incoming.elements[0]
        .values
        .insert("name".into(), serde_json::json!("changed"));
    set.replace(&["memory:/a.xmi".into()], incoming, &loaded.context)
        .unwrap();
    assert_eq!(
        resource_package(set.reader(), "memory:/a.xmi").unwrap(),
        Some(identity)
    );
    let before = set.snapshot().unwrap();
    let mut invalid = before.clone();
    invalid.resources.get_mut("memory:/a.xmi").unwrap().package = Some(self::identity("other"));
    assert!(
        set.replace(&["memory:/a.xmi".into()], invalid, &loaded.context)
            .is_err()
    );
    assert_eq!(set.snapshot().unwrap(), before);
}

#[cfg(feature = "xmi")]
#[test]
fn save_unload_and_reload_preserve_package_provenance() {
    use intersect_model::core::{
        interchange::{FormatRegistry, ReferenceClosure},
        resource_set::UnloadPolicy,
    };
    let mm = metamodel();
    let identity = identity("a");
    let lock = lock(&[("urn:a", &identity)]);
    let payload = model(&mm, "a", "memory:/a.xmi")
        .with_package_identity(&identity)
        .unwrap();
    let loaded = load_against_lock(
        mm.clone(),
        &lock,
        &[PackagePayload {
            identity: &identity,
            model: &payload,
        }],
        &empty_project(&mm),
    )
    .unwrap();
    let mut set =
        ResourceSet::from_store(mm.schema().clone(), loaded.context.clone(), loaded.store).unwrap();
    let mut registry = FormatRegistry::default();
    registry.register(intersect_model::xmi::XmiFormat).unwrap();
    let selected = vec!["memory:/a.xmi".into()];
    let saved = set
        .prepare_save(&registry, "xmi", &selected, ReferenceClosure::SelectedOnly)
        .unwrap();
    let bytes = saved.resources().to_vec();
    set.acknowledge_save(&registry, &saved).unwrap();
    set.unload(&registry, &selected, UnloadPolicy::RequireSaved)
        .unwrap();
    assert_eq!(
        resource_package(set.reader(), "memory:/a.xmi").unwrap(),
        Some(identity.clone())
    );
    let observation = storage::InvocationContext::default();
    let reload = set
        .prepare_reload(
            &registry,
            "xmi",
            &selected,
            &bytes,
            &loaded.context,
            &observation,
        )
        .unwrap();
    set.commit_reload(reload, |_| Ok(()), &observation).unwrap();
    assert_eq!(
        resource_package(set.reader(), "memory:/a.xmi").unwrap(),
        Some(identity)
    );
    assert_eq!(
        set.snapshot().unwrap().elements,
        payload.to_model().unwrap().elements
    );
}
