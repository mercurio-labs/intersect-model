//! Collection typing participates in ordinary module and lexical authority.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::{
        checking::check,
        function_lowering::compile_function,
        ir_v2::{EntrySelection, Limits},
        modules::SymbolId,
    },
};
fn source(name: &str, body: &str) -> DefinitionSource {
    DefinitionSource::new(
        format!("memory:{name}-behavior.rcore"),
        SourceKind::RcoreBehavior,
        format!("rcore 1;package {name} version \"1\";{body}"),
    )
}
const NAMES: &[&str] = &[
    "groupBy",
    "sortedWith",
    "sortedBy",
    "map",
    "filter",
    "flatMap",
    "fold",
    "exists",
    "forAll",
    "one",
    "find",
    "union",
    "intersection",
    "difference",
    "concat",
    "distinct",
    "contains",
    "excludes",
    "count",
    "includesAll",
    "includesAny",
    "flatten",
    "first",
    "last",
    "at",
    "size",
    "isEmpty",
    "Sequence",
    "Set",
    "OrderedSet",
    "Bag",
];
#[test]
fn every_collection_name_obeys_local_module_and_import_binding() {
    for name in NAMES {
        for local in [false, true] {
            let body = if local {
                format!("fn run()->Int8={{let {name}=|x:Int8|x;{name}(42)}};")
            } else {
                format!("fn run()->Int8={name}(42);fn {name}(x:Int8)->Int8=x;")
            };
            let entry = source("names", &body);
            let bundle = DefinitionBundle {
                version: 1,
                sources: vec![entry.clone()],
                namespaces: vec![],
            };
            compile_function(
                &bundle,
                &entry.id,
                &EntrySelection {
                    symbol: SymbolId {
                        source: entry.id.clone(),
                        declaration: 0,
                    },
                    types: vec![],
                    arguments: vec![],
                },
                &Limits::default(),
            )
            .unwrap_or_else(|e| panic!("{name} {local}: {e:?}"));
        }
        let helper = source("helper", &format!("pub fn {name}(x:Int8)->Int8=x;"));
        let mut entry = source(
            "names",
            &format!("import package helper version \"1\" as h;fn run()->Int8=h.{name}(42);"),
        );
        entry.dependencies.push(helper.id.clone());
        let mut bundle = DefinitionBundle {
            version: 1,
            sources: vec![entry.clone(), helper.clone()],
            namespaces: vec![],
        };
        let selection = EntrySelection {
            symbol: SymbolId {
                source: entry.id.clone(),
                declaration: 1,
            },
            types: vec![],
            arguments: vec![],
        };
        let image = compile_function(&bundle, &entry.id, &selection, &Limits::default())
            .unwrap_or_else(|e| panic!("import {name}: {e:?}"));
        assert!(image.program().functions.iter().any(|f|matches!(&f.identity,intersect_model::definitions::rcore_behavior::ir_v2::FunctionIdentity::Named {symbol,..} if symbol.source==helper.id)));
        bundle.sources.reverse();
        assert_eq!(
            image.program(),
            compile_function(&bundle, &entry.id, &selection, &Limits::default())
                .unwrap()
                .program()
        );
    }
}
#[test]
fn collection_errors_in_unused_bodies_keep_source_identity() {
    for body in [
        "fn run()->Int8=42;fn unused()->Sequence<Int> =map([],|x:Int|true);",
        "fn run()->Int8=42;fn unused<T>(xs:Sequence<T>)->Sequence<Int> =map(xs,|x:T|x);",
        "fn run()->Int8=42;fn unused()->Sequence<Int> =[for x in [] where false yield x];",
        "fn run()->Int8=42;fn unused<T:Bool>(xs:Sequence<T>)->Sequence<Int> =map(xs,keep);fn keep<U:Int>(x:U)->U=x;",
        "fn run()->Int8=42;fn unused()->Int=if true {0} else {size(filter([],|x:Int|1))};",
        "fn run()->Int8=42;fn unused()->Sequence<Option<Int>> =[for x in [1] where false yield Some(true)];",
    ] {
        let entry = source("errors", body);
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![entry.clone()],
            namespaces: vec![],
        };
        let errors = check(&bundle, &entry.id).unwrap_err();
        assert!(!errors.is_empty());
        for error in errors {
            assert_eq!(error.span.source, entry.id);
            assert!(
                error.span.start < error.span.end && error.span.end <= entry.text.len(),
                "{error:?}"
            );
        }
    }
}
