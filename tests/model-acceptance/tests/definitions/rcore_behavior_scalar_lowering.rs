use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        ir::{self, InstructionKind, Terminator, VerificationLimits},
        lowering::compile_scalar,
        modules::SymbolId,
    },
};
fn source(body: &str) -> (DefinitionBundle, SymbolId) {
    let source = DefinitionSource::new(
        "memory:scalar-behavior.rcore",
        SourceKind::RcoreBehavior,
        format!("rcore 1; package scalar version \"1\"; {body}"),
    );
    let symbol = SymbolId {
        source: source.id.clone(),
        declaration: 0,
    };
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        symbol,
    )
}
#[test]
fn checked_source_lowers_signed_minimum_and_nested_control_flow() {
    for body in [
        "fn main()->Int8 = -(128);",
        "fn main(a:Bool,b:Int)->Int = if a {b + 1} else {if b > 2 {b * 3} else {b - 1}};",
        "fn main(a:Bool,b:Int)->Bool = a && (b / 0 == 1) || !a;",
        "fn main(x:Int)->Int = match x {0 => 1, remaining => remaining + 2};",
        "fn main(x:Bool)->Unit = match x {true => (), false => ()};",
    ] {
        let (bundle, symbol) = source(body);
        let compiled = compile_scalar(
            &bundle,
            &symbol.source,
            &symbol,
            &VerificationLimits::default(),
        )
        .unwrap_or_else(|e| panic!("{body}: {e:?}"));
        ir::verify(
            compiled.program().clone(),
            &bundle,
            &VerificationLimits::default(),
        )
        .unwrap();
        if body.contains("&&") {
            assert!(
                compiled
                    .program()
                    .blocks
                    .iter()
                    .any(|b| matches!(b.terminator, Terminator::Branch { .. }))
            );
        }
        if body.contains("128") {
            assert_eq!(compiled.program().blocks[0].instructions.len(), 1);
            let InstructionKind::Constant(value) =
                &compiled.program().blocks[0].instructions[0].kind
            else {
                panic!("Signed minimum must be a checked constant")
            };
            assert_eq!(value.payload, serde_json::json!(-128));
        }
    }
}
#[test]
fn all_scalar_entry_domains_have_real_identity_consumers() {
    for scalar in [
        "Bool",
        "String",
        "Bytes",
        "Char16",
        "DateInstant",
        "Unit",
        "Int8",
        "Int16",
        "Int32",
        "Int64",
        "UInt8",
        "UInt16",
        "UInt32",
        "UInt64",
        "BigInt",
        "Decimal",
        "Float32",
        "Float64",
    ] {
        let (bundle, symbol) = source(&format!("fn main(x:{scalar})->{scalar}=x;"));
        let compiled = compile_scalar(
            &bundle,
            &symbol.source,
            &symbol,
            &VerificationLimits::default(),
        )
        .unwrap_or_else(|e| panic!("{scalar}: {e:?}"));
        assert_eq!(
            compiled.program().parameters,
            vec![compiled.program().result.clone()]
        );
    }
}
#[test]
fn unsupported_bodies_and_stale_selection_never_become_executable() {
    for body in [
        "fn main()->Int = {let x=1; x};",
        "fn main()->Int = helper(); fn helper()->Int=1;",
        "fn main<T>(x:T)->T=x;",
        "fn main()->Int = if true {1} else {{let x=2; x}};",
    ] {
        let (bundle, symbol) = source(body);
        let errors = compile_scalar(
            &bundle,
            &symbol.source,
            &symbol,
            &VerificationLimits::default(),
        )
        .unwrap_err();
        assert!(!errors.is_empty());
        assert!(
            errors
                .iter()
                .all(|e| e.span.source == symbol.source && e.span.end > e.span.start)
        );
    }
    let (bundle, mut symbol) = source("fn main()->Int=1;");
    let entry = symbol.source.clone();
    symbol.declaration = 9;
    assert!(compile_scalar(&bundle, &entry, &symbol, &VerificationLimits::default()).is_err());
    symbol.source.sha256 = "0".repeat(64);
    assert!(compile_scalar(&bundle, &entry, &symbol, &VerificationLimits::default()).is_err());
}
#[test]
fn exact_enum_literals_and_matches_survive_source_replay() {
    let model = intersect_model_acceptance::migration_fixture::migrate_source_fixture(DefinitionSource::new(
        "memory:scalar.ecore",
        SourceKind::Ecore,
        r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:scalar"><eClassifiers x:type="e:EEnum" name="Mode"><eLiterals name="First" value="0"/><eLiterals name="Alias" value="0"/></eClassifiers></e:EPackage>"##,
    )).0;
    let mut entry = DefinitionSource::new(
        "memory:enum-behavior.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1; package enums version \"1\"; import model \"urn:scalar\" as m; fn main(x:m.Mode)->m.Mode=match x {m.Mode.First=>m.Mode.Alias,m.Mode.Alias=>m.Mode.First};",
    );
    entry.dependencies.push(model.id.clone());
    let symbol = SymbolId {
        source: entry.id.clone(),
        declaration: 1,
    };
    let mut bundle = DefinitionBundle {
        version: 1,
        sources: vec![entry, model],
        namespaces: vec![],
    };
    let compiled = compile_scalar(
        &bundle,
        &symbol.source,
        &symbol,
        &VerificationLimits::default(),
    )
    .unwrap();
    bundle.sources.reverse();
    let replayed: DefinitionBundle =
        serde_json::from_value(serde_json::to_value(&bundle).unwrap()).unwrap();
    let fresh = compile_scalar(
        &replayed,
        &symbol.source,
        &symbol,
        &VerificationLimits::default(),
    )
    .unwrap();
    assert_eq!(compiled.program(), fresh.program());
}
#[test]
fn lowering_budget_failure_is_located_and_fresh_request_recovers() {
    let (bundle, symbol) = source("fn main(x:Int)->Int = if x > 0 {x+1} else {x-1};");
    let limits = VerificationLimits {
        max_instructions: 1,
        ..VerificationLimits::default()
    };
    let errors = compile_scalar(&bundle, &symbol.source, &symbol, &limits).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("budget") && e.span.end > e.span.start)
    );
    compile_scalar(
        &bundle,
        &symbol.source,
        &symbol,
        &VerificationLimits::default(),
    )
    .unwrap();
}

#[test]
fn scalar_match_domains_and_nested_live_values_have_verified_source_paths() {
    for body in [
        "fn main(x:Float64)->Float64=match x {0=>1,remaining=>remaining};",
        "fn main(x:Float32)->Bool=match x {0=>true,_=>false};",
        "fn main(x:Decimal)->Bool=match x {1.00=>true,_=>false};",
        "fn main(flag:Bool,a:Int,b:Int)->Int=10+(if flag {a}else{if a>0 {b}else{a+b}})*2;",
        "fn main(x:Int,flag:Bool)->Int=match x {0=>1,outer=>outer+(if flag {outer}else{match flag {true=>2,false=>outer+3}})};",
    ] {
        let (bundle, symbol) = source(body);
        let compiled = compile_scalar(
            &bundle,
            &symbol.source,
            &symbol,
            &VerificationLimits::default(),
        )
        .unwrap_or_else(|errors| panic!("{body}: {errors:?}"));
        ir::verify(
            compiled.program().clone(),
            &bundle,
            &VerificationLimits::default(),
        )
        .unwrap();
    }
    for body in [
        "fn main(x:Float64)->Int=match x {0=>1};",
        "fn main(x:Decimal)->Int=match x {1.0=>1,1.00=>2,_=>3};",
        "fn main()->Int=match true {true=>1,false=>{let x=2;x}};",
        "fn main()->Bool=false && ({let x=true;x});",
    ] {
        let (bundle, symbol) = source(body);
        let errors = compile_scalar(
            &bundle,
            &symbol.source,
            &symbol,
            &VerificationLimits::default(),
        )
        .unwrap_err();
        assert!(
            errors.iter().all(
                |error| error.span.source == symbol.source && error.span.end > error.span.start
            ),
            "{body}: {errors:?}"
        );
    }
}

#[test]
fn source_metadata_and_external_selectors_are_budgeted_before_checking() {
    let (mut bundle, symbol) = source("fn main()->Int=1;");
    let limits = VerificationLimits {
        max_source_bytes: 1024,
        ..VerificationLimits::default()
    };
    bundle.sources[0].id.uri = "x".repeat(4096);
    let errors = compile_scalar(&bundle, &symbol.source, &symbol, &limits).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.to_lowercase().contains("budget")
                || error.message.to_lowercase().contains("limit"))
    );
    let (bundle, mut hostile) = source("fn main()->Int=1;");
    hostile.source.uri = "x".repeat(4096);
    let errors = compile_scalar(&bundle, &hostile.source, &hostile, &limits).unwrap_err();
    assert!(errors[0].message.contains("selector metadata budget"));
    assert!(errors[0].span.source.uri.is_empty());
}
