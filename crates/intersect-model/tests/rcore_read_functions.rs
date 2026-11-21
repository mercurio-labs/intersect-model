#[path = "../../../fixtures/rcore-model-rule-service/read_function.rs"]
mod portable;
#[test]
fn supplemental_read_services_keep_structure_sources_and_caller_work() -> Result<(), String> {
    portable::verify()
}

#[test]
fn supplementary_sources_cannot_replace_structure_or_weaken_read_contracts() -> Result<(), String> {
    use intersect_model::{
        compile_rcore_at,
        definitions::{bundle::SourceKind, rcore_behavior::ir_v2::Limits},
        rcore::ExecutionLimits,
        source_model_functions::SourceModelFunction,
    };
    let package = compile_rcore_at("memory:standalone-model.rcore", portable::MODEL)
        .map_err(|error| error.to_string())?;
    let compile = |source: intersect_model::definitions::bundle::DefinitionSource| {
        let selection = portable::selection(&source);
        SourceModelFunction::from_rcore(
            &package,
            vec![source],
            &selection,
            &Limits::default(),
            ExecutionLimits::default(),
            Default::default(),
        )
    };
    let mut wrong_kind = portable::supplement(&package, portable::BODY);
    wrong_kind.kind = SourceKind::Rcore;
    assert!(compile(wrong_kind).is_err());
    let mut replacement = portable::supplement(&package, portable::BODY);
    replacement.id = intersect_model::definitions::bundle::SourceId::for_text(
        package.entry().uri.clone(),
        &replacement.text,
    );
    assert!(compile(replacement).is_err());
    let mut stale = portable::supplement(&package, portable::BODY);
    stale.text.push(' ');
    assert!(compile(stale).is_err());
    for body in [
        "fn inspect(self:Bool)->Bool = self;",
        "fn inspect(self:m.Node, other:m.Node)->m.Node = other;",
        "extern fn inspect(self:m.Node)->Option<String>;",
        "fn inspect(self:m.Node)->(Bool)->Bool = |value:Bool| value;",
    ] {
        let text = format!(
            "rcore 1; package bad.read version \"1\"; import model \"urn:standalone-model-reads\" as m; {body}"
        );
        assert!(
            compile(portable::supplement(&package, &text)).is_err(),
            "{body}"
        );
    }
    let source = portable::supplement(&package, portable::BODY);
    let limits = intersect_model::definitions::ecore::ParseLimits { source_bytes: 1 };
    assert!(
        SourceModelFunction::from_rcore(
            &package,
            vec![source.clone()],
            &portable::selection(&source),
            &Limits::default(),
            ExecutionLimits::default(),
            limits
        )
        .is_err()
    );
    Ok(())
}

mod batch {
    use super::portable;
    use intersect_model::{
        api::ModelEdit,
        compile_rcore_at,
        core::{query::QueryBudget, storage},
        definitions::{
            bundle::{DefinitionBundle, DefinitionSource, SourceKind},
            rcore_behavior::{
                ir_v2::{DataWire, EntrySelection, Limits},
                modules::{SymbolKind, index_modules},
            },
        },
        rcore::{ExecutionLimits, RcorePackage},
        source_model_functions::SourceModelFunction,
    };
    use serde_json::json;
    fn source(package: &RcorePackage, uri: &str, body: &str) -> DefinitionSource {
        let mut s = DefinitionSource::new(uri, SourceKind::RcoreBehavior, body);
        s.dependencies.push(package.entry().clone());
        s
    }
    fn select(
        bundle: &DefinitionBundle,
        source: &DefinitionSource,
        name: &str,
    ) -> Result<EntrySelection, String> {
        let index = index_modules(bundle, &source.id).map_err(|e| format!("{e:?}"))?;
        let symbol = index
            .module(&source.id)?
            .symbols
            .iter()
            .find(|s| s.name == [name] && s.kind == SymbolKind::Function)
            .ok_or("Missing batch entry")?;
        Ok(EntrySelection {
            symbol: symbol.id.clone(),
            types: vec![],
            arguments: vec![],
        })
    }
    const A: &str = r#"rcore 1; package batch.a version "1";
        import model "urn:standalone-model-reads" as m;
        pub fn label(e:m.Node)->String = match e.name {Some(n)=>n,None=>"?"};
        pub fn has(e:m.Node)->Bool = match e.next {Some(n)=>true,None=>false};
        pub fn make(e:m.Node)->m.Node = new m.Node {};
    "#;
    const B: &str = r#"rcore 1; package batch.b version "1";
        import model "urn:standalone-model-reads" as m;
        pub fn second(e:m.Node)->String = "second";
        pub fn invalid(e:Bool)->Bool = e;
    "#;
    #[test]
    fn ordered_batch_matches_single_reads_and_retains_runtime_construction_refusal()
    -> Result<(), String> {
        let package =
            compile_rcore_at("memory:model.rcore", portable::MODEL).map_err(|e| e.to_string())?;
        let original = package.sources().clone();
        let a = source(&package, "memory:a.rcore", A);
        let b = source(&package, "memory:b.rcore", B);
        let mut bundle = original.clone();
        bundle.sources.extend([a.clone(), b.clone()]);
        let entries = [
            select(&bundle, &b, "second")?,
            select(&bundle, &a, "label")?,
            select(&bundle, &a, "has")?,
            select(&bundle, &a, "label")?,
        ];
        let batch = SourceModelFunction::from_rcore_many(
            &package,
            vec![a.clone(), b.clone()],
            &entries,
            &Limits::default(),
            ExecutionLimits::default(),
            Default::default(),
        )?;
        assert_eq!(batch.len(), 4);
        assert!(std::ptr::eq(batch[0].sources(), batch[1].sources()));
        assert!(std::ptr::eq(batch[0].schema(), package.schema()));
        let mut model = package.new_model().map_err(|e| e.to_string())?;
        model.create("a", "Node", "memory:objects")?;
        model.create("b", "Child", "memory:objects")?;
        model.set("a", "name", json!("label-a"))?;
        model.set("a", "next", json!("b"))?;
        for ((function, entry), expected) in batch.iter().zip(entries.iter()).zip([
            json!("second"),
            json!("label-a"),
            json!(true),
            json!("label-a"),
        ]) {
            let single = SourceModelFunction::from_rcore(
                &package,
                vec![a.clone(), b.clone()],
                entry,
                &Limits::default(),
                ExecutionLimits::default(),
                Default::default(),
            )?;
            assert_eq!(
                function.executable().program(),
                single.executable().program()
            );
            let result =
                function.invoke_model(&*model, "a", vec![], &mut QueryBudget::new(1_000_000))?;
            let DataWire::Scalar(value) = result.data else {
                return Err("Non-scalar batch result".into());
            };
            assert_eq!(value.payload, expected);
        }
        let functions = SourceModelFunction::from_rcore_many(
            &package,
            vec![a.clone()],
            &[select(&bundle, &a, "make")?],
            &Limits::default(),
            ExecutionLimits::default(),
            Default::default(),
        )?;
        let before = storage::export(&*model)?;
        assert!(
            functions[0]
                .invoke_model(&*model, "a", vec![], &mut QueryBudget::new(1_000_000))
                .is_err()
        );
        assert_eq!(storage::export(&*model)?, before);
        assert_eq!(package.sources(), &original);
        Ok(())
    }
    #[test]
    fn batch_preflights_sources_limits_and_all_signatures_before_returning() -> Result<(), String> {
        let package =
            compile_rcore_at("memory:model.rcore", portable::MODEL).map_err(|e| e.to_string())?;
        let original = package.sources().clone();
        let a = source(&package, "memory:a.rcore", A);
        let b = source(&package, "memory:b.rcore", B);
        let mut bundle = original.clone();
        bundle.sources.extend([a.clone(), b.clone()]);
        let good = select(&bundle, &a, "label")?;
        let bad = select(&bundle, &b, "invalid")?;
        let compile = |additional, entries: &[EntrySelection], limits: &Limits, source_limits| {
            SourceModelFunction::from_rcore_many(
                &package,
                additional,
                entries,
                limits,
                ExecutionLimits::default(),
                source_limits,
            )
        };
        assert!(
            compile(
                vec![a.clone(), b.clone()],
                &[good.clone(), bad],
                &Limits::default(),
                Default::default()
            )
            .is_err()
        );
        assert!(compile(vec![a.clone()], &[], &Limits::default(), Default::default())?.is_empty());
        let mut replacement = a.clone();
        replacement.id.uri = package.entry().uri.clone();
        assert!(
            compile(
                vec![replacement],
                &[],
                &Limits::default(),
                Default::default()
            )
            .is_err()
        );
        let mut wrong = a.clone();
        wrong.kind = SourceKind::Rcore;
        assert!(
            compile(
                vec![wrong],
                std::slice::from_ref(&good),
                &Limits::default(),
                Default::default()
            )
            .is_err()
        );
        let mut stale = a.clone();
        stale.text.push(' ');
        assert!(
            compile(
                vec![stale],
                std::slice::from_ref(&good),
                &Limits::default(),
                Default::default()
            )
            .is_err()
        );
        let mut foreign = good.clone();
        foreign.symbol.source.sha256 = "0".repeat(64);
        assert!(
            compile(
                vec![a.clone()],
                &[foreign],
                &Limits::default(),
                Default::default()
            )
            .is_err()
        );
        let low = Limits {
            max_functions: 1,
            ..Default::default()
        };
        assert!(
            compile(
                vec![a.clone()],
                &[good.clone(), good.clone()],
                &low,
                Default::default()
            )
            .is_err()
        );
        let mut huge = good.clone();
        huge.symbol.source.uri = "x".repeat(Limits::default().scalar.max_source_bytes + 1);
        assert!(
            compile(
                vec![a.clone()],
                &[huge],
                &Limits::default(),
                Default::default()
            )
            .is_err()
        );
        assert!(
            compile(
                vec![a],
                &[good],
                &Limits::default(),
                intersect_model::definitions::ecore::ParseLimits { source_bytes: 1 }
            )
            .is_err()
        );
        assert_eq!(package.sources(), &original);
        Ok(())
    }
}
