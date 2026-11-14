use intersect_model::core::model::Model;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::{compile_structure_bundle_with_conversions, standard_datatype},
    conversion::{
        ConversionContract, ConversionProvider, MemberContract, MemberType, ProviderId,
        ReferenceContext,
    },
    resources::DeclarationId,
};

use intersect_model::{
    collection_conversion::Member,
    conversion::with_catalog,
    feature_map_conversion::{FeatureMapConverter, FeatureMapKind},
    model_reference::ReferenceDomain,
};
use serde_json::{Value, json};
const TYPES: &str = r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" name="types" nsURI="urn:reference-types">
<eClassifiers xsi:type="e:EDataType" name="RGB" instanceClassName="example.RGB"/>
<eClassifiers xsi:type="e:EClass" name="Base"><eStructuralFeatures xsi:type="e:EAttribute" name="name" lowerBound="1" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/><eStructuralFeatures xsi:type="e:EAttribute" name="color" eType="#//RGB" defaultValueLiteral="#010203"/><eStructuralFeatures xsi:type="e:EReference" name="peer" eType="#//Base"/></eClassifiers>
<eClassifiers xsi:type="e:EClass" name="Child" eSuperTypes="#//Base"/>
<eClassifiers xsi:type="e:EClass" name="Other"/>
</e:EPackage>"##;
fn setup() -> (ReferenceContext, DeclarationId) {
    let source = DefinitionSource::new("file:///contexts/types.ecore", SourceKind::Ecore, TYPES);
    let (source, declarations) =
        intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let class = declarations[&vec![1]].clone();
    let conversion = ConversionContract {
        declarations: vec![],
        members: vec![],
        datatype: declarations[&vec![0]].clone(),
        provider: ProviderId {
            name: "example.rgb".into(),
            version: "1".into(),
        },
        serializable: true,
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let model = with_catalog(&bundle, std::slice::from_ref(&conversion), |registry| {
        let schema = compile_structure_bundle_with_conversions(
            &bundle,
            &source.id,
            "urn:reference-types",
            registry,
        )
        .map_err(|e| e.to_string())?;
        let mut model = Model::default();
        for (id, class) in [("base", "Base"), ("child", "Child"), ("other", "Other")] {
            let index = model.create(&schema, id, class, "memory:objects", 0, 0)?;
            if class != "Other" {
                model.elements[index]
                    .values
                    .insert("name".into(), json!(id));
            }
        }
        model.elements[0]
            .values
            .insert("peer".into(), json!("child"));
        model.elements[1]
            .values
            .insert("peer".into(), json!("base"));
        model.validate_with_conversions(&schema, registry)?;
        Ok(serde_json::to_value(model).unwrap())
    })
    .unwrap();
    (
        ReferenceContext {
            materialized_types: vec![],
            applied_conversions: vec![],
            constructors: vec![],
            bundle,
            entry: source.id,
            namespace: "urn:reference-types".into(),
            conversions: vec![conversion],
            model,
        },
        class,
    )
}
fn domain(context: &ReferenceContext, class: &DeclarationId) -> Result<ReferenceDomain, String> {
    with_catalog(&context.bundle, &context.conversions, |registry| {
        ReferenceDomain::new(class.clone(), context.clone(), registry)
    })
}
#[test]
fn immutable_references_resolve_real_exact_and_subtype_objects() {
    let (context, class) = setup();
    let domain = domain(&context, &class).unwrap();
    for id in ["base", "child"] {
        let wire = domain.bind(id).unwrap();
        assert_eq!(domain.resolve(&wire).unwrap().id, id);
        assert_eq!(wire["context"], context.digest().unwrap());
    }
    assert!(domain.bind("other").is_err());
    assert!(domain.bind("missing").is_err());
    let member = Member::reference(false, domain);
    assert!(member.validate(&Value::Null).is_err());
    let nullable = Member::reference(
        true,
        crate::language_model_reference::domain(&context, &class).unwrap(),
    );
    nullable.validate(&Value::Null).unwrap();
    let encoded = serde_json::to_value(nullable.contract()).unwrap();
    assert_eq!(
        serde_json::from_value::<MemberContract>(encoded).unwrap(),
        *nullable.contract()
    );
}
#[test]
fn malformed_contexts_and_forged_model_objects_reject() {
    let (context, class) = setup();
    for mutation in 0..7 {
        let mut bad = context.clone();
        match mutation {
            0 => bad.model["schema_revision"] = json!("stale"),
            1 => bad.model["elements"][0]["values"]["name"] = json!(7),
            2 => bad.model["elements"][0]["type_identity"] = json!("forged"),
            3 => bad.model["elements"][1]["id"] = json!("base"),
            4 => bad.bundle.sources[0].text.push(' '),
            5 => bad.conversions[0].provider.version = "2".into(),
            _ => bad.model["unexpected"] = json!(true),
        }
        assert!(domain(&bad, &class).is_err(), "mutation {mutation}");
    }
    let mut wrong = intersect_model_acceptance::migration_fixture::named_declaration_fixture(
        &context.bundle.sources[0],
        "RGB",
    );
    assert!(domain(&context, &wrong).is_err());
    wrong = class.clone();
    wrong.source.sha256 = "0".repeat(64);
    assert!(domain(&context, &wrong).is_err());
}
#[test]
fn changed_valid_snapshots_and_wire_class_assertions_cannot_retarget_references() {
    let (context, class) = setup();
    let first = domain(&context, &class).unwrap();
    let reference = first.bind("child").unwrap();
    let mut revised = context.clone();
    revised.model["elements"][1]["values"]["name"] = json!("changed");
    let second = domain(&revised, &class).unwrap();
    assert!(second.resolve(&reference).is_err());
    assert_ne!(context.digest().unwrap(), revised.digest().unwrap());
    let mut forged = reference.clone();
    forged["element"] = json!("other");
    assert!(first.resolve(&forged).is_err());
    let mut extra = reference;
    extra["class"] = json!(class);
    assert!(first.resolve(&extra).is_err());
    assert_eq!(
        first.resolve(&first.bind("child").unwrap()).unwrap().values["name"],
        "child"
    );
}
fn outer(
    context: &ReferenceContext,
    class: &DeclarationId,
) -> (DefinitionBundle, ConversionContract) {
    let mut source = DefinitionSource::new(
        "file:///contexts/outer.rcore",
        SourceKind::Rcore,
        r#"rcore 1; package outer version "1";
metamodel "urn:outer" prefix outer {
 class Holder { ref peer: external "types.ecore.rcore#//Base" [0,1]; }
}"#,
    );
    source.dependencies.push(context.entry.clone());
    let feature = DeclarationId {
        source: source.id.clone(),
        path: vec![0, 0],
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source, context.bundle.sources[0].clone()],
        namespaces: vec![],
    };
    let contract = ConversionContract {
        datatype: standard_datatype("EFeatureMapEntry")
            .unwrap()
            .declaration()
            .clone(),
        provider: ProviderId {
            name: "ecore.feature-entry".into(),
            version: "1".into(),
        },
        serializable: false,
        declarations: vec![feature],
        members: vec![MemberContract {
            nullable: false,
            value: MemberType::Reference {
                class: class.clone(),
                context: Box::new(context.clone()),
            },
        }],
    };
    (bundle, contract)
}
#[test]
fn feature_entries_and_catalog_validate_reference_contexts_and_declared_targets() {
    let (context, class) = setup();
    let reference = domain(&context, &class).unwrap().bind("child").unwrap();
    let (bundle, contract) = outer(&context, &class);
    let member = Member::reference(false, domain(&context, &class).unwrap());
    let adapter = FeatureMapConverter::new(
        contract.clone(),
        FeatureMapKind::Entry,
        &bundle,
        vec![member],
    )
    .unwrap();
    assert_eq!(
        adapter
            .bind_reference(&contract.declarations[0], "child")
            .unwrap(),
        reference
    );
    assert_eq!(
        adapter
            .resolve_reference(&contract.declarations[0], &reference)
            .unwrap()
            .class,
        "Child"
    );
    assert!(
        adapter
            .bind_reference(&contract.declarations[0], "other")
            .is_err()
    );
    let mut unknown = contract.declarations[0].clone();
    unknown.path = vec![9];
    assert!(adapter.bind_reference(&unknown, "child").is_err());
    assert!(adapter.resolve_reference(&unknown, &reference).is_err());
    let entry = json!({"feature":contract.declarations[0],"value":reference});
    adapter.validate(&entry).unwrap();
    with_catalog(&bundle, std::slice::from_ref(&contract), |registry| {
        let converted = registry.bind(&contract.datatype, entry.clone())?;
        registry.validate(&converted)?;
        Ok(())
    })
    .unwrap();
    let wrong = intersect_model_acceptance::migration_fixture::named_declaration_fixture(
        &context.bundle.sources[0],
        "Other",
    );
    let member = Member::reference(false, domain(&context, &wrong).unwrap());
    let mut bad = contract.clone();
    bad.members = vec![member.contract().clone()];
    assert!(FeatureMapConverter::new(bad, FeatureMapKind::Entry, &bundle, vec![member]).is_err());
    let mut oversized = contract;
    let MemberType::Reference { context, .. } = &mut oversized.members[0].value else {
        panic!("reference expected")
    };
    context.model = json!("x".repeat(16 * 1024 * 1024));
    assert!(
        with_catalog(&bundle, &[oversized], |_| Ok(()))
            .unwrap_err()
            .contains("byte budget")
    );
}

#[test]
fn attribute_members_cannot_bind_or_resolve_object_references() {
    let member = Member::builtin(
        false,
        intersect_model::definitions::scalars::Builtin::EString,
    );
    assert!(member.bind_reference("child").is_err());
    assert!(
        member
            .resolve_reference(&json!({"kind":"model_reference","context":"x","element":"child"}))
            .is_err()
    );
}

#[test]
fn observed_domain_construction_stops_at_every_segment_and_recovers() {
    use intersect_model::definitions::{
        conversion::{InvocationContext, InvocationObserver},
        generic_types::{RuntimeType, Type},
    };
    use std::{cell::Cell, rc::Rc};
    struct Probe {
        count: Cell<usize>,
        stop: usize,
        calls: Cell<usize>,
        rejected_units: Cell<usize>,
    }
    impl InvocationObserver for Probe {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            self.calls.set(self.calls.get() + 1);
            self.work(1)
        }
        fn work(&self, units: usize) -> Result<(), String> {
            if self.count.get() + units > self.stop {
                self.rejected_units.set(units);
                return Err("domain interrupted".into());
            }
            self.count.set(self.count.get() + units);
            Ok(())
        }
    }
    let (context, class) = setup();
    let snapshot = context.model.clone();
    let target = RuntimeType::new(Type::Classifier {
        declaration: class,
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    with_catalog(&context.bundle, &context.conversions, |registry| {
        let probe = Rc::new(Probe {
            count: Cell::new(0),
            stop: usize::MAX,
            calls: Cell::new(0),
            rejected_units: Cell::new(0),
        });
        let reference = ReferenceDomain::new_runtime_observed(
            target.clone(),
            context.clone(),
            registry,
            &InvocationContext::new(probe.clone()),
        )?;
        assert_eq!(reference.resolve(&reference.bind("child")?)?.id, "child");
        let total = probe.count.get();
        assert!(total > 50);
        assert_eq!(probe.calls.get(), 2);
        for stop in 0..total {
            let probe = Rc::new(Probe {
                count: Cell::new(0),
                stop,
                calls: Cell::new(0),
                rejected_units: Cell::new(0),
            });
            let result = ReferenceDomain::new_runtime_observed(
                target.clone(),
                context.clone(),
                registry,
                &InvocationContext::new(probe.clone()),
            );
            let error = result.err().expect("must interrupt");
            assert!(error.contains("domain interrupted"), "{stop}: {error}");
            // Hashing precharges a whole emitted byte chunk. A rejected chunk
            // leaves the counter below the cap rather than charging a prefix.
            assert!(probe.count.get() <= stop);
            assert!(probe.count.get() + probe.rejected_units.get() > stop);
            assert_eq!(context.model, snapshot);
        }
        let exact = Rc::new(Probe {
            count: Cell::new(0),
            stop: total,
            calls: Cell::new(0),
            rejected_units: Cell::new(0),
        });
        let recovered = ReferenceDomain::new_runtime_observed(
            target.clone(),
            context.clone(),
            registry,
            &InvocationContext::new(exact.clone()),
        )?;
        assert_eq!(exact.count.get(), total);
        assert_eq!(reference.bind("base")?, recovered.bind("base")?);
        assert!(recovered.bind("other").is_err());
        Ok(())
    })
    .unwrap();
}

#[test]
fn admitted_reference_domain_projects_stored_members_through_shared_read_indexes() {
    use intersect_model::core::storage::{ModelRead, Query};
    let (context, class) = setup();
    let checked = domain(&context, &class).unwrap();
    let child = checked.bind("child").unwrap();
    let member = checked
        .schema()
        .effective_feature("Base", "name")
        .unwrap()
        .identity;
    let name = checked.stored_feature(&child, "Base", &member).unwrap();
    assert_eq!(name.singular().unwrap(), json!("child"));
    let peer = checked
        .schema()
        .effective_feature("Base", "peer")
        .unwrap()
        .identity;
    assert_eq!(
        checked
            .stored_feature(&child, "Base", &peer)
            .unwrap()
            .singular()
            .unwrap(),
        json!("base")
    );
    let incoming = checked
        .read()
        .query_candidates(Query::Value("peer", &json!("child")), None, 1)
        .unwrap();
    assert_eq!(incoming.rows, [(0, "base".into())]);
    assert!(incoming.exact && incoming.exhausted);
    let mut foreign = child.clone();
    foreign["context"] = json!("another-snapshot");
    assert!(checked.stored_feature(&foreign, "Base", &member).is_err());
    let mut absent = child.clone();
    absent["element"] = json!("missing");
    assert!(checked.stored_feature(&absent, "Base", &member).is_err());
    assert!(checked.stored_feature(&child, "Other", &member).is_err());
    // Failed projection does not poison subsequent reads or nominal binding.
    assert_eq!(
        checked
            .stored_feature(&child, "Base", &member)
            .unwrap()
            .singular()
            .unwrap(),
        json!("child")
    );
    assert_eq!(checked.resolve(&child).unwrap().id, "child");
}

#[test]
fn retargeted_domains_share_admission_but_keep_exact_membership_and_observation() {
    use intersect_model::definitions::{
        conversion::{InvocationContext, InvocationObserver},
        generic_types::{RuntimeType, Type},
    };
    use std::{cell::Cell, rc::Rc};
    struct Probe {
        work: Cell<usize>,
        stop: usize,
    }
    impl InvocationObserver for Probe {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            panic!("retarget must not repeat model provider validation")
        }
        fn work(&self, count: usize) -> Result<(), String> {
            if self.work.get() + count > self.stop {
                return Err("retarget interrupted".into());
            }
            self.work.set(self.work.get() + count);
            Ok(())
        }
    }
    let (context, class) = setup();
    let original = domain(&context, &class).unwrap();
    let target = |name| {
        RuntimeType::new(Type::Classifier {
            declaration: intersect_model_acceptance::migration_fixture::named_declaration_fixture(
                &context.bundle.sources[0],
                name,
            ),
            arguments: vec![],
            raw: false,
        })
        .unwrap()
    };
    let child = original
        .retarget_runtime_observed(target("Child"), &InvocationContext::default())
        .unwrap();
    assert!(std::ptr::eq(original.schema(), child.schema()));
    assert!(std::ptr::eq(original.read(), child.read()));
    assert!(std::ptr::eq(original.context(), child.context()));
    assert_eq!(
        original.bind("child").unwrap(),
        child.bind("child").unwrap()
    );
    assert!(child.bind("base").is_err());
    assert!(child.bind("other").is_err());
    let other = original
        .retarget_runtime_observed(target("Other"), &InvocationContext::default())
        .unwrap();
    assert!(other.bind("other").is_ok());
    assert!(other.resolve(&child.bind("child").unwrap()).is_err());
    let baseline_probe = Rc::new(Probe {
        work: Cell::new(0),
        stop: usize::MAX,
    });
    original
        .retarget_runtime_observed(
            target("Child"),
            &InvocationContext::new(baseline_probe.clone()),
        )
        .unwrap();
    let required = baseline_probe.work.get();
    assert!(required > 4);
    for stop in 0..=required {
        let probe = Rc::new(Probe {
            work: Cell::new(0),
            stop,
        });
        let result = original
            .retarget_runtime_observed(target("Child"), &InvocationContext::new(probe.clone()));
        assert_eq!(result.is_ok(), stop == required);
        assert_eq!(probe.work.get(), stop);
        assert!(original.bind("base").is_ok());
    }
    let foreign = RuntimeType::new(Type::Classifier {
        declaration: DeclarationId {
            source: intersect_model::definitions::bundle::SourceId {
                uri: class.source.uri.clone(),
                sha256: "0".repeat(64),
            },
            path: vec![2],
        },
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    assert!(
        original
            .retarget_runtime_observed(foreign, &InvocationContext::default())
            .is_err()
    );
    let bound = child.bind("child").unwrap();
    drop(original);
    assert_eq!(child.resolve(&bound).unwrap().id, "child");
}

#[test]
fn membership_preparation_yields_schema_and_ancestry_without_publishing_partial_domains() {
    use intersect_model::definitions::{
        conversion::{InvocationContext, InvocationObserver},
        generic_types::{RuntimeType, Type},
    };
    use std::{cell::Cell, rc::Rc};
    struct Probe {
        work: Cell<usize>,
        stop: usize,
    }
    impl InvocationObserver for Probe {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            panic!("no repeated providers")
        }
        fn work(&self, count: usize) -> Result<(), String> {
            assert_eq!(count, 1);
            if self.work.get() == self.stop {
                return Err("cancelled".into());
            }
            self.work.set(self.work.get() + 1);
            Ok(())
        }
    }
    let (context, class) = setup();
    let original = domain(&context, &class).unwrap();
    let target = RuntimeType::new(Type::Classifier {
        declaration: intersect_model_acceptance::migration_fixture::named_declaration_fixture(
            &context.bundle.sources[0],
            "Child",
        ),
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    let baseline_probe = Rc::new(Probe {
        work: Cell::new(0),
        stop: usize::MAX,
    });
    let baseline_observation = InvocationContext::new(baseline_probe.clone());
    let mut baseline = original
        .begin_retarget_runtime(target.clone(), &baseline_observation)
        .unwrap();
    let mut slices = 0;
    while !baseline.advance(&baseline_observation).unwrap() {
        slices += 1;
        assert!(slices < 10_000);
    }
    let required = baseline_probe.work.get();
    // Extra serialized visits: declaration 10 + classifier Type 18 + schema
    // Feature payloads 23 (name), 62 (converted RGB color), 23 (peer) = 136.
    assert_eq!(required, slices + 2 + 136);
    assert!(required > 4);
    baseline.finish(&baseline_observation).unwrap();
    for stop in 0..required {
        let probe = Rc::new(Probe {
            work: Cell::new(0),
            stop,
        });
        let observation = InvocationContext::new(probe.clone());
        let pending = original.begin_retarget_runtime(target.clone(), &observation);
        let mut pending = match pending {
            Ok(value) => value,
            Err(error) => {
                assert_eq!(error, "cancelled");
                assert_eq!(probe.work.get(), stop);
                assert!(original.bind("base").is_ok());
                continue;
            }
        };
        loop {
            match pending.advance(&observation) {
                Ok(false) => {}
                Ok(true) => panic!("insufficient work published membership"),
                Err(error) => {
                    assert_eq!(error, "cancelled");
                    break;
                }
            }
        }
        assert_eq!(probe.work.get(), stop);
        assert!(pending.advance(&InvocationContext::default()).is_err());
        assert!(pending.finish(&InvocationContext::default()).is_err());
        assert!(original.bind("base").is_ok());
    }
    for steps in [0, 1, slices / 2, slices - 1] {
        let mut pending = original
            .begin_retarget_runtime(target.clone(), &InvocationContext::default())
            .unwrap();
        for _ in 0..steps {
            assert!(!pending.advance(&InvocationContext::default()).unwrap());
        }
        assert!(pending.finish(&InvocationContext::default()).is_err());
    }
    let probe = Rc::new(Probe {
        work: Cell::new(0),
        stop: required,
    });
    let observation = InvocationContext::new(probe.clone());
    let mut pending = original
        .begin_retarget_runtime(target, &observation)
        .unwrap();
    assert_eq!(probe.work.get(), 29); // Initial unit + 10/18 input visits.
    for index in 0..=slices {
        let before = probe.work.get();
        assert_eq!(pending.advance(&observation).unwrap(), index == slices);
        assert!([1, 24, 63].contains(&(probe.work.get() - before)));
    }
    assert!(pending.advance(&observation).unwrap());
    assert_eq!(probe.work.get(), required);
    let finished = pending.finish(&observation).unwrap();
    assert!(std::ptr::eq(original.read(), finished.read()));
    assert!(finished.bind("child").is_ok());
    assert!(finished.bind("base").is_err());
}

#[test]
fn staged_admission_publishes_only_after_validation_and_membership() {
    use intersect_model::definitions::{
        conversion::InvocationContext,
        generic_types::{RuntimeType, Type},
    };
    let (context, class) = setup();
    let target = RuntimeType::new(Type::Classifier {
        declaration: class.clone(),
        arguments: vec![],
        raw: false,
    })
    .unwrap();
    with_catalog(&context.bundle, &context.conversions, |registry| {
        let observation = InvocationContext::default();
        let pending = ReferenceDomain::begin_runtime_observed(
            target.clone(),
            context.clone(),
            registry,
            &observation,
        )
        .unwrap();
        assert!(pending.finish(&observation).is_err());
        let mut pending = ReferenceDomain::begin_runtime_observed(
            target.clone(),
            context.clone(),
            registry,
            &observation,
        )
        .unwrap();
        let mut slices = 0;
        while !pending.advance(registry, &observation).unwrap() {
            slices += 1;
            assert!(slices < 1000);
        }
        assert!(slices > context.model["elements"].as_array().unwrap().len());
        let admitted = pending.finish(&observation).unwrap();
        assert_eq!(
            admitted.bind("child").unwrap(),
            domain(&context, &class).unwrap().bind("child").unwrap()
        );
        let mut invalid = context.clone();
        invalid.model["elements"][0]["type_identity"] = serde_json::json!("forged");
        let mut pending =
            ReferenceDomain::begin_runtime_observed(target, invalid, registry, &observation)
                .unwrap();
        loop {
            match pending.advance(registry, &observation) {
                Err(_) => break,
                Ok(false) => {}
                Ok(true) => panic!("invalid graph admitted"),
            }
        }
        assert!(pending.advance(registry, &observation).is_err());
        assert!(pending.finish(&observation).is_err());
        Ok(())
    })
    .unwrap();
}

#[test]
fn deep_reference_membership_retains_ancestry_and_exact_snapshot_selection() {
    use intersect_model::definitions::{
        conversion::{ConversionRegistry, InvocationContext, InvocationObserver},
        generic_types::{RuntimeType, Type},
    };
    use std::{cell::Cell, rc::Rc};
    struct Probe {
        work: Cell<usize>,
        stop: usize,
    }
    impl InvocationObserver for Probe {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            panic!("no providers in this fixture")
        }
        fn work(&self, count: usize) -> Result<(), String> {
            assert_eq!(count, 1);
            if self.work.get() == self.stop {
                return Err("cancelled".into());
            }
            self.work.set(self.work.get() + 1);
            Ok(())
        }
    }
    let classes = (0..64)
        .map(|n| {
            let parent = if n == 0 {
                String::new()
            } else {
                format!(r##" eSuperTypes="#//C{:02}""##, n - 1)
            };
            format!(r#"<eClassifiers x:type="e:EClass" name="C{n:02}"{parent}/>"#)
        })
        .collect::<String>();
    let source = intersect_model::definitions::bundle::DefinitionSource::new(
        "memory:deep-reference.ecore",
        intersect_model::definitions::bundle::SourceKind::Ecore,
        format!(
            r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:deep-reference">{classes}<eClassifiers x:type="e:EClass" name="Other"/></e:EPackage>"#
        ),
    );
    let (source, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(source);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let schema = compile_structure_bundle_with_conversions(
        &bundle,
        &source.id,
        "urn:deep-reference",
        &ConversionRegistry::default(),
    )
    .unwrap();
    let mut model = Model::default();
    for (id, class) in [("deep", "C63"), ("base", "C00"), ("other", "Other")] {
        model.create(&schema, id, class, "r", 0, 0).unwrap();
    }
    let runtime = |n| {
        RuntimeType::new(Type::Classifier {
            declaration: DeclarationId {
                source: source.id.clone(),
                path: vec![n],
            },
            arguments: vec![],
            raw: false,
        })
        .unwrap()
    };
    let context = ReferenceContext {
        bundle,
        entry: source.id.clone(),
        namespace: "urn:deep-reference".into(),
        conversions: vec![],
        materialized_types: vec![],
        applied_conversions: vec![],
        constructors: vec![],
        model: serde_json::to_value(model).unwrap(),
    };
    let original =
        ReferenceDomain::new_runtime(runtime(0), context, &ConversionRegistry::default()).unwrap();
    let probe = Rc::new(Probe {
        work: Cell::new(0),
        stop: usize::MAX,
    });
    let observation = InvocationContext::new(probe.clone());
    let mut pending = original
        .begin_retarget_runtime(runtime(32), &observation)
        .unwrap();
    while !pending.advance(&observation).unwrap() {
        assert!(probe.work.get() < 10_000);
    }
    let required = probe.work.get();
    assert!(required >= 64 * 4);
    let selected = pending.finish(&observation).unwrap();
    assert!(std::ptr::eq(original.read(), selected.read()));
    assert!(selected.bind("deep").is_ok());
    assert!(selected.bind("base").is_err());
    assert!(selected.bind("other").is_err());
    assert!(original.bind("base").is_ok());
    for stop in [1, 64, required / 2, required - 1] {
        let probe = Rc::new(Probe {
            work: Cell::new(0),
            stop,
        });
        let observation = InvocationContext::new(probe.clone());
        let mut pending = match original.begin_retarget_runtime(runtime(32), &observation) {
            Ok(value) => value,
            Err(error) => {
                assert_eq!(error, "cancelled");
                assert_eq!(probe.work.get(), stop);
                assert!(original.bind("base").is_ok());
                continue;
            }
        };
        loop {
            match pending.advance(&observation) {
                Ok(false) => {}
                Ok(true) => panic!("insufficient budget published membership"),
                Err(error) => {
                    assert_eq!(error, "cancelled");
                    break;
                }
            }
        }
        assert_eq!(probe.work.get(), stop);
        assert!(pending.advance(&InvocationContext::default()).is_err());
        assert!(pending.finish(&InvocationContext::default()).is_err());
        assert!(original.bind("base").is_ok());
    }
}

#[test]
fn target_selection_precedes_snapshot_conversion_and_empty_domains_require_completion() {
    use intersect_model::definitions::{
        conversion::InvocationContext,
        generic_types::{RuntimeType, Type},
    };
    let (context, class) = setup();
    let runtime = |declaration| {
        RuntimeType::new(Type::Classifier {
            declaration,
            arguments: vec![],
            raw: false,
        })
        .unwrap()
    };
    let mut foreign = class.clone();
    foreign.source.sha256 = "0".repeat(64);
    let mut malformed = context.clone();
    malformed.model["unexpected"] = json!(true);
    with_catalog(&context.bundle, &context.conversions, |registry| {
        let observation = InvocationContext::default();
        let mut pending = ReferenceDomain::begin_runtime_observed(
            runtime(foreign.clone()),
            malformed.clone(),
            registry,
            &observation,
        )
        .unwrap();
        let mut slices = 0;
        let error = loop {
            slices += 1;
            assert!(slices < 10_000);
            match pending.advance(registry, &observation) {
                Ok(false) => {}
                Ok(true) => panic!("foreign target admitted"),
                Err(error) => break error,
            }
        };
        assert!(slices > 1);
        assert_eq!(error, "Unknown closed class declaration");
        assert!(pending.advance(registry, &observation).is_err());
        assert!(pending.finish(&observation).is_err());
        assert_eq!(
            ReferenceDomain::new(foreign.clone(), malformed.clone(), registry)
                .err()
                .unwrap(),
            "Reference target is not an exact selected EClass"
        );
        assert_eq!(
            ReferenceDomain::new_runtime(runtime(class.clone()), malformed.clone(), registry)
                .err()
                .unwrap(),
            "Reference model snapshot requires exact explicit fields"
        );

        let mut empty = context.clone();
        empty.model["elements"] = json!([]);
        for record in empty.model["resources"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            record["roots"] = json!([]);
        }
        let original =
            ReferenceDomain::new_runtime(runtime(class.clone()), empty, registry).unwrap();
        let pending = original
            .begin_retarget_runtime(runtime(class.clone()), &observation)
            .unwrap();
        assert!(pending.finish(&observation).is_err());
        let mut pending = original
            .begin_retarget_runtime(runtime(class.clone()), &observation)
            .unwrap();
        let mut steps = 0;
        while !pending.advance(&observation).unwrap() {
            steps += 1;
            assert!(steps < 10_000);
        }
        assert!(steps > 1);
        assert!(pending.advance(&observation).unwrap());
        let domain = pending.finish(&observation).unwrap();
        assert!(std::ptr::eq(original.read(), domain.read()));
        assert!(domain.bind("missing").is_err());
        let mut pending = original
            .begin_retarget_runtime(runtime(foreign.clone()), &observation)
            .unwrap();
        loop {
            match pending.advance(&observation) {
                Ok(false) => {}
                Ok(true) => panic!("empty model bypassed target qualification"),
                Err(error) => {
                    assert_eq!(error, "Unknown closed class declaration");
                    break;
                }
            }
        }
        assert!(pending.finish(&observation).is_err());
        Ok(())
    })
    .unwrap();
}
