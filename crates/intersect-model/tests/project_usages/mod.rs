use intersect_model::{
    compile_rcore,
    definitions::bundle::SourceId,
    project_usages::{
        DependencyLock, PackageCatalog, PackageIdentity, PackageManifest, ResolutionError,
        ResolutionLimits, Usage, UsagePath, UsageResolver,
    },
    resource_set::ResourceContext,
};

#[derive(Default)]
struct Catalog(Vec<PackageManifest>);

impl PackageCatalog for Catalog {
    fn candidates(&self, uri: &str, constraint: Option<&str>) -> Vec<PackageManifest> {
        self.0
            .iter()
            .filter(|manifest| {
                manifest.uri == uri && constraint.is_none_or(|version| manifest.version == version)
            })
            .cloned()
            .collect()
    }
}

impl Catalog {
    fn add(&mut self, uri: &str, version: &str, usages: Vec<Usage>) {
        let mut manifest = candidate(uri, version, usages);
        manifest.dependency_lock = UsageResolver::resolve(&manifest.usages, self).unwrap();
        manifest.identity.dependency_lock_digest = manifest.dependency_lock.digest().unwrap();
        self.0.push(manifest);
    }
}

fn candidate(uri: &str, version: &str, usages: Vec<Usage>) -> PackageManifest {
    let provenance =
        SourceId::for_text(format!("test:{uri}/{version}"), &format!("{uri}@{version}"));
    PackageManifest {
        uri: uri.into(),
        version: version.into(),
        identity: PackageIdentity {
            content_digest: provenance.sha256.clone(),
            dependency_lock_digest: String::new(),
            engine: "engine-1".into(),
            schema: "schema-1".into(),
        },
        provenance,
        usages,
        dependency_lock: DependencyLock::empty(),
    }
}

fn usage(uri: &str, constraint: Option<&str>) -> Usage {
    Usage {
        resource_uri: uri.into(),
        version_constraint: constraint.map(str::to_owned),
    }
}

fn v1(uri: &str) -> Usage {
    usage(uri, Some("1"))
}

fn diamond(controls_version: &str) -> Catalog {
    let mut catalog = Catalog::default();
    catalog.add("CommonTypes", "1", vec![]);
    catalog.add("CommonTypes", "2", vec![]);
    catalog.add("Powertrain", "1", vec![v1("CommonTypes")]);
    catalog.add(
        "Controls",
        "1",
        vec![usage("CommonTypes", Some(controls_version))],
    );
    catalog
}

#[test]
fn shares_identical_diamond_and_records_both_paths() {
    let mut catalog = diamond("1");
    let mut mirror = catalog.0[0].clone();
    mirror.provenance.uri = "mirror:CommonTypes".into();
    catalog.0.push(mirror);
    let lock = UsageResolver::resolve(&[v1("Powertrain"), v1("Controls")], &catalog).unwrap();
    assert_eq!(lock.entries().len(), 3);
    let common = &lock.entries()["CommonTypes"];
    assert_eq!(common.identity, catalog.0[0].identity);
    assert_eq!(
        common.requested_by,
        [
            UsagePath(vec![v1("Controls"), v1("CommonTypes")]),
            UsagePath(vec![v1("Powertrain"), v1("CommonTypes")]),
        ]
    );
    assert!(!common.direct);
    assert!(lock.entries()["Controls"].direct);
    assert!(lock.entries()["Powertrain"].direct);
}

#[test]
fn reports_every_conflicting_request_including_a_later_path() {
    let mut catalog = diamond("2");
    catalog.add("Vehicle", "1", vec![v1("Powertrain")]);
    let roots = [v1("Vehicle"), v1("Controls"), v1("Powertrain")];
    assert_eq!(
        UsageResolver::resolve(&roots, &catalog),
        Err(ResolutionError::Conflict {
            uri: "CommonTypes".into(),
            paths: vec![
                UsagePath(vec![v1("Controls"), usage("CommonTypes", Some("2"))]),
                UsagePath(vec![v1("Powertrain"), v1("CommonTypes")]),
                UsagePath(vec![v1("Vehicle"), v1("Powertrain"), v1("CommonTypes")]),
            ],
        })
    );
}

#[test]
fn reports_missing_recorded_transitive_usage() {
    let missing = v1("Missing");
    let catalog = Catalog(vec![candidate("Parent", "1", vec![missing.clone()])]);
    assert!(matches!(
        UsageResolver::resolve(&[v1("Parent")], &catalog),
        Err(ResolutionError::LockMismatch { package, uri, path, recorded: None, .. })
            if package == "Parent" && uri == "Missing"
                && path == UsagePath(vec![v1("Parent"), missing])
    ));
}

#[test]
fn reports_cycle_members_without_the_prefix() {
    let mut catalog = Catalog(vec![
        candidate("Root", "1", vec![v1("B")]),
        candidate("B", "1", vec![v1("A")]),
        candidate("A", "1", vec![v1("B")]),
    ]);
    // A cycle cannot have a valid finite content-addressed build lock. Keep
    // direct records so the resolver can diagnose the unsupported cycle
    // before checking the deliberately invalid digests.
    let identities: std::collections::BTreeMap<_, _> = catalog
        .0
        .iter()
        .map(|m| (m.uri.clone(), m.identity.clone()))
        .collect();
    for manifest in &mut catalog.0 {
        let entries: std::collections::BTreeMap<_, _> = manifest
            .usages
            .iter()
            .map(|u| {
                (
                    u.resource_uri.clone(),
                    serde_json::json!({
                        "identity": identities[&u.resource_uri], "direct": true,
                        "requested_by": [[u]],
                    }),
                )
            })
            .collect();
        manifest.dependency_lock = serde_json::from_value(serde_json::json!({
            "format": "intersect.dependency-lock.v1",
            "root_usages": manifest.usages,
            "entries": entries,
        }))
        .unwrap();
    }
    assert_eq!(
        UsageResolver::resolve(&[v1("Root")], &catalog),
        Err(ResolutionError::Cycle {
            members: vec!["A".into(), "B".into()]
        })
    );
}

#[test]
fn canonical_lock_survives_shuffled_input_and_round_trip() {
    let mut catalog = diamond("1");
    catalog.add("Vehicle", "1", vec![v1("Powertrain"), v1("Controls")]);
    let roots = [v1("Vehicle"), v1("CommonTypes")];
    let lock = UsageResolver::resolve(&roots, &catalog).unwrap();
    let json = serde_json::to_vec(&lock).unwrap();

    catalog.0.rotate_left(2);
    for manifest in &mut catalog.0 {
        manifest.usages.reverse();
    }
    let shuffled = UsageResolver::resolve(
        &[roots[1].clone(), roots[0].clone(), roots[0].clone()],
        &catalog,
    )
    .unwrap();
    assert_eq!(serde_json::to_vec(&shuffled).unwrap(), json);
    let restored: DependencyLock = serde_json::from_slice(&json).unwrap();
    assert_eq!(restored, lock);
    assert_eq!(restored.digest().unwrap(), lock.digest().unwrap());
    assert_eq!(restored.root_usages(), [v1("CommonTypes"), v1("Vehicle")]);
    assert!(restored.entries()["CommonTypes"].direct);
    assert_eq!(restored.entries()["CommonTypes"].requested_by.len(), 3);
    assert!(!restored.entries()["Controls"].direct);
}

#[test]
fn selects_only_the_identity_common_to_all_requirements() {
    let catalog = diamond("1");
    let unpinned = usage("CommonTypes", None);
    assert_eq!(
        UsageResolver::resolve(std::slice::from_ref(&unpinned), &catalog),
        Err(ResolutionError::Ambiguous {
            usage: unpinned.clone(),
            candidates: {
                let mut ids = vec![catalog.0[0].identity.clone(), catalog.0[1].identity.clone()];
                ids.sort();
                ids
            },
        })
    );
    // The narrower usage is discovered through another package.
    let lock = UsageResolver::resolve(&[unpinned, v1("Powertrain")], &catalog).unwrap();
    assert_eq!(
        lock.entries()["CommonTypes"].identity,
        catalog.0[0].identity
    );
}

#[test]
fn rejects_a_package_locked_against_a_different_dependency_snapshot() {
    let mut catalog = diamond("1");
    catalog.0[0].identity.engine = "engine-2".into();
    let parent = v1("Powertrain");
    assert!(matches!(
        UsageResolver::resolve(std::slice::from_ref(&parent), &catalog),
        Err(ResolutionError::LockMismatch { package, uri, path, .. })
            if package == "Powertrain" && uri == "CommonTypes"
                && path == UsagePath(vec![parent, v1("CommonTypes")])
    ));
}

#[test]
fn resource_context_from_lock_uses_exact_direct_and_transitive_identities() {
    let package = compile_rcore(
        r#"rcore 1; package example version "1";
metamodel "urn:example" prefix e { class Item {} }"#,
    )
    .unwrap();
    let schema = package.schema();
    let mut catalog = diamond("1");
    let roots = [v1("Powertrain")];
    let lock = UsageResolver::resolve(&roots, &catalog).unwrap();
    let context = ResourceContext::from_lock(schema, &lock).unwrap();
    assert_eq!(
        context
            .dependencies()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["CommonTypes", "Powertrain"]
    );
    assert_eq!(
        context,
        ResourceContext::new(schema, context.dependencies().clone()).unwrap()
    );

    // Only the parent's own dependency lock changes. Its content, version,
    // engine and schema all remain identical.
    catalog.0[0].identity.schema = "schema-2".into();
    catalog.0.retain(|manifest| manifest.uri != "Powertrain");
    catalog.add("Powertrain", "1", vec![v1("CommonTypes")]);
    let changed_lock = UsageResolver::resolve(&roots, &catalog).unwrap();
    let changed = ResourceContext::from_lock(schema, &changed_lock).unwrap();
    assert_ne!(
        changed.dependencies()["Powertrain"],
        context.dependencies()["Powertrain"]
    );
    assert_ne!(
        changed.dependencies()["CommonTypes"],
        context.dependencies()["CommonTypes"]
    );
}

#[test]
fn a_registered_unconstrained_consumer_survives_new_catalog_versions() {
    let mut catalog = Catalog::default();
    catalog.add("CommonTypes", "1", vec![]);
    catalog.add("Consumer", "1", vec![usage("CommonTypes", None)]);
    let roots = [v1("Consumer")];
    let first = UsageResolver::resolve(&roots, &catalog).unwrap();
    catalog.add("CommonTypes", "2", vec![]);
    let second = UsageResolver::resolve(&roots, &catalog).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        second.entries()["CommonTypes"].identity,
        catalog.0[0].identity
    );
    let manifest = &catalog.0[1];
    let restored: PackageManifest =
        serde_json::from_slice(&serde_json::to_vec(manifest).unwrap()).unwrap();
    assert_eq!(restored, *manifest);
}

#[test]
fn two_recorded_unconstrained_versions_report_both_requesting_paths() {
    let mut one = Catalog::default();
    one.add("CommonTypes", "1", vec![]);
    one.add("ConsumerOne", "1", vec![usage("CommonTypes", None)]);
    let mut two = Catalog::default();
    two.add("CommonTypes", "2", vec![]);
    two.add("ConsumerTwo", "1", vec![usage("CommonTypes", None)]);
    one.0.extend(two.0);
    assert_eq!(
        UsageResolver::resolve(&[v1("ConsumerOne"), v1("ConsumerTwo")], &one),
        Err(ResolutionError::Conflict {
            uri: "CommonTypes".into(),
            paths: vec![
                UsagePath(vec![v1("ConsumerOne"), usage("CommonTypes", None)]),
                UsagePath(vec![v1("ConsumerTwo"), usage("CommonTypes", None)]),
            ],
        })
    );
}

#[test]
fn a_recorded_identity_must_satisfy_its_declared_constraint() {
    let mut catalog = Catalog::default();
    catalog.add("CommonTypes", "1", vec![]);
    catalog.add("Consumer", "1", vec![v1("CommonTypes")]);
    catalog.add("CommonTypes", "2", vec![]);
    catalog.0[1].usages[0].version_constraint = Some("2".into());
    assert!(
        matches!(UsageResolver::resolve(&[v1("Consumer")], &catalog),
        Err(ResolutionError::LockMismatch { package, uri, recorded: Some(_), reason, .. })
            if package == "Consumer" && uri == "CommonTypes" && reason.contains("violates"))
    );
}

#[test]
fn a_corrupt_recorded_lock_digest_is_a_lock_mismatch() {
    let mut catalog = Catalog::default();
    catalog.add("Leaf", "1", vec![]);
    catalog.0[0].identity.dependency_lock_digest = "wrong digest".into();
    assert!(matches!(UsageResolver::resolve(&[v1("Leaf")], &catalog),
        Err(ResolutionError::LockMismatch { package, uri, path, .. })
            if package == "Leaf" && uri == "Leaf" && path == UsagePath(vec![v1("Leaf")])));
}

#[test]
fn an_unavailable_root_is_unsatisfied() {
    let missing = v1("Missing");
    assert_eq!(
        UsageResolver::resolve(std::slice::from_ref(&missing), &Catalog::default()),
        Err(ResolutionError::Unsatisfied {
            usage: missing.clone(),
            path: UsagePath(vec![missing])
        })
    );
}

#[test]
fn layered_diamond_path_enumeration_has_a_deterministic_limit() {
    let mut catalog = Catalog::default();
    catalog.add("A0", "1", vec![]);
    catalog.add("B0", "1", vec![]);
    for level in 1..=4 {
        let previous = vec![
            v1(&format!("A{}", level - 1)),
            v1(&format!("B{}", level - 1)),
        ];
        catalog.add(&format!("A{level}"), "1", previous.clone());
        catalog.add(&format!("B{level}"), "1", previous);
    }
    let roots = [v1("A4"), v1("B4")];
    let limits = ResolutionLimits { max_paths: 32 };
    assert_eq!(
        UsageResolver::resolve_with_limits(&roots, &catalog, limits),
        Err(ResolutionError::PathLimitExceeded { limit: 32 })
    );
    catalog.0.reverse();
    assert_eq!(
        UsageResolver::resolve_with_limits(&[roots[1].clone(), roots[0].clone()], &catalog, limits),
        Err(ResolutionError::PathLimitExceeded { limit: 32 })
    );
    assert!(UsageResolver::resolve(&roots, &catalog).is_ok());
}

#[test]
fn invalid_root_constraint_is_reported_before_lookup() {
    struct CheckedCatalog;
    impl PackageCatalog for CheckedCatalog {
        fn validate_usage(&self, _usage: &Usage) -> Result<(), String> {
            Err("unsupported syntax".into())
        }
        fn candidates(&self, _uri: &str, _constraint: Option<&str>) -> Vec<PackageManifest> {
            panic!("Invalid constraints must not reach lookup")
        }
    }
    let invalid = usage("CommonTypes", Some("invalid"));
    assert_eq!(
        UsageResolver::resolve(std::slice::from_ref(&invalid), &CheckedCatalog),
        Err(ResolutionError::Constraint {
            usage: invalid,
            reason: "unsupported syntax".into()
        })
    );
}

#[test]
fn conflicts_stop_expansion_before_selecting_a_conflicting_packages_children() {
    let mut one = Catalog::default();
    one.add("Common", "1", vec![]);
    one.add("Z", "1", vec![v1("Common")]);
    one.add("P", "1", vec![v1("Z")]);
    let mut two = Catalog::default();
    two.add("Common", "2", vec![]);
    two.add("Z", "2", vec![usage("Common", Some("2"))]);
    two.add("Q", "1", vec![usage("Z", Some("2"))]);
    two.add("Long", "1", vec![v1("Q")]);
    two.add("R", "1", vec![usage("Common", Some("2"))]);
    one.0.extend(two.0);
    assert_eq!(
        UsageResolver::resolve(&[v1("P"), v1("Long"), v1("R")], &one),
        Err(ResolutionError::Conflict {
            uri: "Z".into(),
            paths: vec![
                UsagePath(vec![v1("Long"), v1("Q"), usage("Z", Some("2"))]),
                UsagePath(vec![v1("P"), v1("Z")]),
            ]
        })
    );
}
