//! Source-backed S04 typing; these checks do not execute Rcore or qualify SysML.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    generic_types::Type as ModelType,
    rcore_behavior::{
        checking::{TypeReport, check},
        types::Type,
    },
    resources::DeclarationId,
};

fn model() -> DefinitionSource {
    let original = DefinitionSource::new(
        "memory:structural-model.ecore",
        SourceKind::Ecore,
        r##"
<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="model" nsURI="urn:structural-model">
 <eClassifiers x:type="e:EClass" name="Value"/>
 <eClassifiers x:type="e:EClass" name="Base">
  <eStructuralFeatures x:type="e:EReference" name="sequence" eType="#//Value" lowerBound="2" upperBound="4" ordered="true" unique="false"/>
  <eStructuralFeatures x:type="e:EReference" name="set" eType="#//Value" upperBound="-1" ordered="false" unique="true"/>
  <eStructuralFeatures x:type="e:EReference" name="orderedSet" eType="#//Value" upperBound="-1" ordered="true" unique="true"/>
  <eStructuralFeatures x:type="e:EReference" name="bag" eType="#//Value" upperBound="-1" ordered="false" unique="false"/>
  <eStructuralFeatures x:type="e:EReference" name="required" eType="#//Value" lowerBound="2" upperBound="-1" ordered="true" unique="false" derived="true"/>
  <eStructuralFeatures x:type="e:EReference" name="bounded" eType="#//Value" upperBound="4" ordered="true" unique="false" derived="true"/>
 </eClassifiers>
 <eClassifiers x:type="e:EClass" name="Child" eSuperTypes="#//Base"/>
 <eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="T"/>
  <eStructuralFeatures x:type="e:EReference" name="items" lowerBound="1" upperBound="3" ordered="true" unique="false"><eGenericType eTypeParameter="#//Box/T"/></eStructuralFeatures>
 </eClassifiers>
 <eClassifiers x:type="e:EClass" name="Applied"><eGenericSuperTypes eClassifier="#//Box"><eTypeArguments eClassifier="#//Value"/></eGenericSuperTypes></eClassifiers>
</e:EPackage>"##,
    );
    intersect_model_acceptance::migration_fixture::migrate_source_fixture(original).0
}

fn input(model: &DefinitionSource, body: &str) -> (DefinitionBundle, DefinitionSource) {
    let mut source = DefinitionSource::new(
        "memory:structural-model-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!(
            "rcore 1; package structural version \"1\"; import model \"urn:structural-model\" as m; {body}"
        ),
    );
    source.dependencies.push(model.id.clone());
    (
        DefinitionBundle {
            version: 1,
            sources: vec![model.clone(), source.clone()],
            namespaces: vec![],
        },
        source,
    )
}

fn good(model: &DefinitionSource, body: &str) -> TypeReport {
    let (bundle, source) = input(model, body);
    check(&bundle, &source.id).unwrap_or_else(|errors| panic!("{body}: {errors:?}"))
}

#[test]
fn inherited_feature_reads_cover_all_four_collection_kinds_without_erasing_bounds() {
    let model = model();
    let cases = [
        ("sequence", "Sequence", true, false, 2, 4),
        ("set", "Set", false, true, 0, -1),
        ("orderedSet", "OrderedSet", true, true, 0, -1),
        ("bag", "Bag", false, false, 0, -1),
    ];
    for (position, (field, kind, ordered, unique, lower, upper)) in cases.iter().enumerate() {
        let body = format!("fn read(x:m.Child)->{kind}<m.Value> =x.{field};");
        let report = good(&model, &body);
        let feature = report
            .modules
            .iter()
            .flat_map(|m| m.members.values())
            .next()
            .unwrap();
        assert_eq!(
            feature.declaration,
            DeclarationId {
                source: model.id.clone(),
                path: vec![1, position]
            }
        );
        assert_eq!(
            feature.owner.expression(),
            &ModelType::Classifier {
                declaration: DeclarationId {
                    source: model.id.clone(),
                    path: vec![1]
                },
                arguments: vec![],
                raw: false
            }
        );
        assert_eq!(
            feature.receiver.expression(),
            &ModelType::Classifier {
                declaration: DeclarationId {
                    source: model.id.clone(),
                    path: vec![2]
                },
                arguments: vec![],
                raw: false
            }
        );
        assert!(!feature.schema_revision.is_empty());
        assert!(
            matches!(report.signatures.types.get(feature.value).unwrap(), Type::Many { lower: l, upper: u, ordered: o, unique: q, .. } if (l,u,o,q)==(lower,upper,ordered,unique))
        );
        let (mut bundle, source) = input(&model, &body);
        bundle.sources.reverse();
        assert_eq!(
            serde_json::to_value(&report).unwrap(),
            serde_json::to_value(check(&bundle, &source.id).unwrap()).unwrap()
        );
        for (_, other, _, _, _, _) in &cases {
            if other == kind {
                continue;
            }
            let (bundle, source) = input(
                &model,
                &format!("fn bad(x:m.Child)->{other}<m.Value> =x.{field};"),
            );
            let errors = check(&bundle, &source.id).unwrap_err();
            assert!(errors.iter().any(|e| e.span.source == source.id));
        }
    }
}

#[test]
fn collection_annotations_cannot_certify_required_or_bounded_derived_features() {
    let model = model();
    for field in ["required", "bounded"] {
        let (bundle, source) = input(
            &model,
            &format!("derive m.Child.{field}(self)->Sequence<m.Value> =[];"),
        );
        let errors = check(&bundle, &source.id).unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("multiplicity")),
            "{errors:?}"
        );
    }
}

#[test]
fn applied_inherited_feature_reports_preserve_exact_owner_and_source_revision() {
    let original = model();
    let body = "fn read(x:m.Applied)->Sequence<m.Value> =x.items; fn direct(x:m.Box<m.Value>)->Sequence<m.Value> =x.items;";
    let report = good(&original, body);
    let features: Vec<_> = report
        .modules
        .iter()
        .flat_map(|m| m.members.values())
        .collect();
    assert_eq!(features.len(), 2);
    let owner = ModelType::Classifier {
        declaration: DeclarationId {
            source: original.id.clone(),
            path: vec![3],
        },
        arguments: vec![ModelType::Classifier {
            declaration: DeclarationId {
                source: original.id.clone(),
                path: vec![0],
            },
            arguments: vec![],
            raw: false,
        }],
        raw: false,
    };
    for feature in &features {
        assert_eq!(feature.owner.expression(), &owner);
        assert_eq!(
            feature.declaration,
            DeclarationId {
                source: original.id.clone(),
                path: vec![3, 0]
            }
        );
        assert!(feature.assignment_exact);
        assert!(matches!(
            report.signatures.types.get(feature.value).unwrap(),
            Type::Many {
                lower: 1,
                upper: 3,
                ordered: true,
                unique: false,
                ..
            }
        ));
    }
    assert_ne!(features[0].receiver, features[1].receiver);
    let revised = DefinitionSource::new(
        original.id.uri.clone(),
        SourceKind::Rcore,
        original.text.replace("[1,3]", "[1,5]"),
    );
    let updated = good(&revised, body);
    for feature in updated.modules.iter().flat_map(|m| m.members.values()) {
        assert_eq!(feature.declaration.source, revised.id);
        assert_ne!(feature.schema_revision, features[0].schema_revision);
        assert!(matches!(
            updated.signatures.types.get(feature.value).unwrap(),
            Type::Many { upper: 5, .. }
        ));
    }
    let (mut stale, source) = input(&original, body);
    stale.sources[0] = revised;
    assert!(check(&stale, &source.id).is_err());
}
