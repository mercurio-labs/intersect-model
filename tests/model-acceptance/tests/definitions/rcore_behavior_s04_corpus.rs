//! Handwritten S04 consumer of the pinned, generated minimal metadata projection.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{checking::check, types::Type},
};
const MODEL: &str = include_str!("../../../../fixtures/rcore-behavior-s04/model.ecore");
const SOURCE: &str = include_str!("../../../../fixtures/rcore-behavior-s04/main-behavior.rcore");
fn input(body: &str) -> (DefinitionBundle, DefinitionSource, DefinitionSource) {
    let model = DefinitionSource::new("fixture:s04/model.ecore", SourceKind::Ecore, MODEL);
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    let mut entry = DefinitionSource::new(
        "fixture:s04/main-behavior.rcore",
        SourceKind::RcoreBehavior,
        body,
    );
    entry.dependencies.push(model.id.clone());
    (
        DefinitionBundle {
            version: 1,
            sources: vec![model.clone(), entry.clone()],
            namespaces: vec![],
        },
        model,
        entry,
    )
}
#[test]
fn pinned_mercurio_shapes_are_consumed_statically_with_qualified_provenance() {
    let evidence: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/rcore-behavior-s04/mercurio-shapes.json"
    ))
    .unwrap();
    assert_eq!(
        evidence["upstream_commit"],
        "32750d7c7799970d7d494e0e8e6e0bf1daae4689"
    );
    assert_eq!(
        evidence["upstream_sha256"],
        "7c4290959d5687e5245ee7d4c6b018ab337d950d3552402e18977252541fb8dd"
    );
    assert_eq!(
        evidence["upstream_behavior"],
        "metadata only; delegates and model algorithms not executed"
    );
    let (bundle, model, entry) = input(SOURCE);
    let report = check(&bundle, &entry.id).unwrap();
    let namespace = "https://www.omg.org/spec/SysML/20250201";
    for (index, path, owner, name, target, lower, upper, ordered) in [
        (
            0,
            vec![3, 0],
            "ActionDefinition",
            "action",
            "ActionUsage",
            0,
            -1,
            true,
        ),
        (
            1,
            vec![4, 0],
            "AcceptActionUsage",
            "payloadArgument",
            "Expression",
            0,
            1,
            false,
        ),
        (
            2,
            vec![4, 1],
            "AcceptActionUsage",
            "payloadParameter",
            "ReferenceUsage",
            1,
            1,
            false,
        ),
    ] {
        let record = &evidence["features"][index];
        assert_eq!(record["id"], format!("{namespace}#//{owner}/{name}"));
        assert_eq!(record["owner"], format!("{namespace}#//{owner}"));
        assert_eq!(record["type"], format!("{namespace}#//{target}"));
        assert_eq!(record["lower_bound"], lower);
        assert_eq!(record["upper_bound"], upper);
        assert_eq!(record["ordered"], ordered);
        assert_eq!(record["unique"], true);
        for flag in ["derived", "volatile", "transient"] {
            assert_eq!(record[flag], true);
        }
        let feature = report.modules[0]
            .members
            .values()
            .find(|f| f.declaration.path == path)
            .unwrap();
        assert_eq!(feature.declaration.source, model.id);
        assert!(feature.derived);
        let ty = report.signatures.types.get(feature.value).unwrap();
        match index {
            0 => assert!(matches!(
                ty,
                Type::Many {
                    lower: 0,
                    upper: -1,
                    ordered: true,
                    unique: true,
                    ..
                }
            )),
            1 => assert!(matches!(ty, Type::Option(_))),
            2 => assert!(matches!(ty, Type::Model(_))),
            _ => unreachable!(),
        }
    }
    let mut replay: DefinitionBundle =
        serde_json::from_value(serde_json::to_value(&bundle).unwrap()).unwrap();
    replay.sources.reverse();
    assert_eq!(
        serde_json::to_value(report).unwrap(),
        serde_json::to_value(check(&replay, &entry.id).unwrap()).unwrap()
    );
}
#[test]
fn pinned_shape_wrappers_reject_kind_and_requiredness_overclaims() {
    for body in [
        "fn wrong(x:m.ActionDefinition)->Sequence<m.ActionUsage> =x.action;",
        "fn wrong(x:m.ActionDefinition)->m.ActionUsage =x.action[0];",
        "fn wrong(x:m.AcceptActionUsage)->m.Expression =x.payloadArgument;",
        "fn wrong(x:m.AcceptActionUsage)->m.ReferenceUsage? =x.payloadParameter;",
    ] {
        let text = format!(
            "rcore 1; package rejected version \"1\"; import model \"https://www.omg.org/spec/SysML/20250201\" as m; {body}"
        );
        let (bundle, _, entry) = input(&text);
        let errors = check(&bundle, &entry.id).unwrap_err();
        assert!(!errors.is_empty());
        assert!(
            errors
                .iter()
                .all(|e| e.span.source == entry.id && e.span.start < e.span.end),
            "{body}: {errors:?}"
        );
    }
}
#[test]
fn recursive_expanding_instantiation_is_distinct_from_an_impossible_result_constraint() {
    for (body, valid) in [
        ("fn recur<T>(x:T)->Int =recur([x]);", true),
        ("fn recur<T>(x:T)->T =recur([x]);", false),
    ] {
        let text = format!("rcore 1; package recursion version \"1\"; {body}");
        let (bundle, _, entry) = input(&text);
        let result = check(&bundle, &entry.id);
        if valid {
            assert!(result.is_ok(), "{body}: {result:?}");
        } else {
            let errors = result.unwrap_err();
            assert!(!errors.is_empty());
            assert!(
                errors
                    .iter()
                    .all(|e| e.span.source == entry.id && e.span.start < e.span.end),
                "{errors:?}"
            );
        }
    }
}
#[test]
fn generic_inference_rejects_two_incomparable_common_ancestors() {
    let model = DefinitionSource::new(
        "memory:ambiguous.ecore",
        SourceKind::Ecore,
        "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='ambiguous' nsURI='urn:ambiguous'><eClassifiers x:type='e:EClass' name='A'/><eClassifiers x:type='e:EClass' name='B'/><eClassifiers x:type='e:EClass' name='Left' eSuperTypes='#//A #//B'/><eClassifiers x:type='e:EClass' name='Right' eSuperTypes='#//A #//B'/></e:EPackage>",
    );
    let (model, _) = intersect_model_acceptance::migration_fixture::migrate_source_fixture(model);
    for arguments in ["left,right", "right,left"] {
        for explicit in [false, true] {
            let call = if explicit { "same::<m.A>" } else { "same" };
            let mut entry = DefinitionSource::new(
                "memory:ambiguous-behavior.rcore",
                SourceKind::RcoreBehavior,
                format!(
                    "rcore 1; package ambiguous version \"1\"; import model \"urn:ambiguous\" as m; fn same<T>(x:T,y:T)->T=x; fn test(left:m.Left,right:m.Right)->m.A={{let inferred={call}({arguments}); inferred}};"
                ),
            );
            entry.dependencies.push(model.id.clone());
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![model.clone(), entry.clone()],
                namespaces: vec![],
            };
            let result = check(&bundle, &entry.id);
            if explicit {
                assert!(result.is_ok(), "{result:?}");
            } else {
                let errors = result.unwrap_err();
                assert!(
                    errors
                        .iter()
                        .any(|e| e.message.to_lowercase().contains("ambig")),
                    "{errors:?}"
                );
                assert!(
                    errors
                        .iter()
                        .all(|e| e.span.source == entry.id && e.span.start < e.span.end)
                );
            }
        }
    }
}
