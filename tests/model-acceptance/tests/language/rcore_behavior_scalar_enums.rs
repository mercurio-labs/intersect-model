use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    enums::EnumValue,
    rcore_behavior::{
        ir::{Value, ValueType, VerificationLimits},
        lowering::compile_scalar,
        modules::SymbolId,
        types::Scalar,
    },
    resources::DeclarationId,
};
use intersect_model::rcore_behavior_execution::{ExecutionErrorKind, ExecutionLimits, evaluate};

fn fixture() -> (DefinitionBundle, SymbolId, DeclarationId) {
    let model = intersect_model_acceptance::migration_fixture::migrate_source_fixture(DefinitionSource::new(
        "memory:runtime-enums.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:runtime-enums"><eClassifiers x:type="e:EEnum" name="Mode"><eLiterals name="First" value="7" literal="first"/><eLiterals name="Alias" value="7" literal="alias"/></eClassifiers><eClassifiers x:type="e:EEnum" name="Other"><eLiterals name="First" value="7" literal="first"/></eClassifiers></e:EPackage>"##,
    )).0;
    let enumeration = DeclarationId {
        source: model.id.clone(),
        path: vec![0],
    };
    let mut source = DefinitionSource::new(
        "memory:runtime-enums-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1; package runtime_enums version \"1\"; import model \"urn:runtime-enums\" as m; fn flip(x:m.Mode)->m.Mode=match x {m.Mode.First=>m.Mode.Alias,m.Mode.Alias=>m.Mode.First}; fn equal(a:m.Mode,b:m.Mode)->Bool=a==b; fn bind(x:m.Mode)->m.Mode=match x {m.Mode.First=>m.Mode.First,remaining=>remaining};",
    );
    source.dependencies.push(model.id.clone());
    let symbol = SymbolId {
        source: source.id.clone(),
        declaration: 1,
    };
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source, model],
            namespaces: vec![],
        },
        symbol,
        enumeration,
    )
}
fn member(enumeration: &DeclarationId, index: usize) -> Value {
    let mut path = enumeration.path.clone();
    path.push(index);
    Value {
        ty: ValueType::Enum(enumeration.clone()),
        payload: serde_json::to_value(EnumValue::Enum {
            enumeration: enumeration.clone(),
            literal: DeclarationId {
                source: enumeration.source.clone(),
                path,
            },
        })
        .unwrap(),
    }
}
#[test]
fn source_enum_matches_and_equality_preserve_alias_identity_through_replay() {
    let (mut bundle, mut symbol, enumeration) = fixture();
    let first = member(&enumeration, 0);
    let alias = member(&enumeration, 1);
    assert_ne!(first, alias);
    let compiled = compile_scalar(
        &bundle,
        &symbol.source,
        &symbol,
        &VerificationLimits::default(),
    )
    .unwrap();
    let flipped = evaluate(
        &compiled,
        std::slice::from_ref(&first),
        &ExecutionLimits::default(),
    )
    .unwrap();
    assert_eq!(flipped.value, alias);
    assert_eq!(
        evaluate(
            &compiled,
            std::slice::from_ref(&alias),
            &ExecutionLimits::default()
        )
        .unwrap()
        .value,
        first
    );
    bundle.sources.reverse();
    let replayed: DefinitionBundle =
        serde_json::from_value(serde_json::to_value(&bundle).unwrap()).unwrap();
    let rebuilt = compile_scalar(
        &replayed,
        &symbol.source,
        &symbol,
        &VerificationLimits::default(),
    )
    .unwrap();
    assert_eq!(compiled.program(), rebuilt.program());
    assert_eq!(
        serde_json::to_value(&flipped).unwrap(),
        serde_json::to_value(
            evaluate(
                &rebuilt,
                std::slice::from_ref(&first),
                &ExecutionLimits::default()
            )
            .unwrap()
        )
        .unwrap()
    );
    symbol.declaration = 2;
    let equality = compile_scalar(
        &bundle,
        &symbol.source,
        &symbol,
        &VerificationLimits::default(),
    )
    .unwrap();
    assert_eq!(
        evaluate(
            &equality,
            &[first.clone(), alias.clone()],
            &ExecutionLimits::default()
        )
        .unwrap()
        .value
        .payload,
        serde_json::json!(false)
    );
    assert_eq!(
        evaluate(
            &equality,
            &[alias.clone(), alias.clone()],
            &ExecutionLimits::default()
        )
        .unwrap()
        .value
        .payload,
        serde_json::json!(true)
    );
    symbol.declaration = 3;
    let binding = compile_scalar(
        &bundle,
        &symbol.source,
        &symbol,
        &VerificationLimits::default(),
    )
    .unwrap();
    assert_eq!(
        evaluate(
            &binding,
            std::slice::from_ref(&alias),
            &ExecutionLimits::default()
        )
        .unwrap()
        .value,
        alias
    );
}
#[test]
fn enum_execution_rejects_stale_wrong_type_and_forged_literal_arguments() {
    let (bundle, symbol, enumeration) = fixture();
    let compiled = compile_scalar(
        &bundle,
        &symbol.source,
        &symbol,
        &VerificationLimits::default(),
    )
    .unwrap();
    let valid = member(&enumeration, 0);
    let mut stale_id = enumeration.clone();
    stale_id.source.sha256 = "0".repeat(64);
    let mut other = enumeration.clone();
    other.path = vec![1];
    let mut stale_payload = valid.clone();
    stale_payload.payload = member(&stale_id, 0).payload;
    let mut wrong_owner = valid.clone();
    wrong_owner.payload = member(&other, 0).payload;
    let mut forged = valid.clone();
    forged.payload["literal"]["path"] = serde_json::json!([0, 99]);
    let mut extra = valid.clone();
    extra.payload["untrusted"] = serde_json::json!(true);
    let mut extra_enum_id = valid.clone();
    extra_enum_id.payload["enumeration"]["untrusted"] = serde_json::json!(true);
    let mut extra_literal_id = valid.clone();
    extra_literal_id.payload["literal"]["untrusted"] = serde_json::json!(true);
    for invalid in [
        Value {
            ty: ValueType::Scalar(Scalar::Int32),
            payload: serde_json::json!(7),
        },
        member(&stale_id, 0),
        member(&other, 0),
        stale_payload,
        wrong_owner,
        forged,
        extra,
        extra_enum_id,
        extra_literal_id,
    ] {
        let error = evaluate(&compiled, &[invalid], &ExecutionLimits::default()).unwrap_err();
        assert_eq!(error.kind, ExecutionErrorKind::Arguments, "{error:?}");
        assert_eq!(error.symbol, symbol);
    }
    assert_eq!(
        evaluate(&compiled, &[valid], &ExecutionLimits::default())
            .unwrap()
            .value,
        member(&enumeration, 1)
    );
}
