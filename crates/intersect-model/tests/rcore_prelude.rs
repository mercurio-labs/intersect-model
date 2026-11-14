//! Method calls, type-switch matches and the OCL-named prelude operations,
//! executed over a live model, plus the forms the parser must reject.
use intersect_model::{
    api::ModelEdit, compile_rcore_at, core::query::QueryBudget, rcore::ExecutionLimits,
};
use serde_json::{Value, json};

const SOURCE: &str = include_str!("../../../fixtures/rcore-prelude/prelude.rcore");

#[test]
fn prelude_operations_and_type_switches_run_over_a_live_model()
-> Result<(), Box<dyn std::error::Error>> {
    let package = compile_rcore_at("memory:prelude.rcore", SOURCE)?;
    let mut model = package.new_model()?;
    for (id, class) in [
        ("a", "Node"),
        ("b", "Leaf"),
        ("c", "LeafChild"),
        ("d", "Sibling"),
    ] {
        model.create(id, class, "memory:objects")?;
        model.set(id, "raw", json!(format!("raw-{id}")))?;
    }
    model.set("b", "detail", json!("detail-b"))?;
    model.set("a", "peers", json!(["d", "c", "a", "b"]))?;
    let limits = ExecutionLimits::default();
    let read = |owner: &str, feature: &str| -> Result<Value, Box<dyn std::error::Error>> {
        Ok(package.read_derived(
            &model,
            owner,
            feature,
            &mut QueryBudget::new(10_000_000),
            &limits,
        )?)
    };
    for (owner, feature, expected) in [
        ("a", "kind", json!("node")),
        ("b", "kind", json!("detail-b")),
        ("c", "kind", json!("child")),
        ("c", "kind", json!("child")),
        ("d", "kind", json!("sibling")),
        ("a", "fallback", json!("raw-a")),
        ("b", "fallback", json!(null)),
        ("d", "fallback", json!("raw-d")),
        ("a", "leafPeers", json!(["c", "b"])),
        ("a", "firstLeaf", json!("c")),
        ("a", "nonLeafCount", int64(2)),
        ("a", "otherCount", int64(3)),
        ("a", "hasPeers", json!(true)),
        ("b", "hasPeers", json!(false)),
        ("b", "leafPeers", json!([])),
        ("b", "firstLeaf", json!(null)),
        ("a", "isLeaf", json!(false)),
        ("b", "isLeaf", json!(true)),
        ("c", "isLeaf", json!(true)),
    ] {
        assert_eq!(read(owner, feature)?, expected, "{owner}.{feature}");
    }
    Ok(())
}

const HEAD: &str = r#"rcore 1;
package prelude.syntax version "1";
metamodel "urn:prelude-syntax" prefix m {
  class Element { attr name: String [0,1]; }
  class Type extends Element { ref general: Type [0,*]; }
}
behavior {
  import model "urn:prelude-syntax" as m;
"#;

fn compile(body: &str) -> Result<(), String> {
    compile_rcore_at(
        "memory:prelude-syntax.rcore",
        &format!("{HEAD}{body}\n}}\n"),
    )
    .map(|_| ())
    .map_err(|error| format!("{error:?}"))
}

#[test]
fn admitted_forms() {
    for body in [
        // Method calls on values, builtins with type arguments and chains.
        "fn f(xs:Sequence<Int64>)->Int64 = xs.filter(|x| x > 1).size();",
        "fn f(xs:Sequence<m.Element>)->Sequence<m.Type> = xs.selectByKind::<m.Type>();",
        "fn f(s:String)->Option<String> = s.split(\"::\").first();",
        // Qualified names rooted at an import alias stay qualified.
        "fn f(e:m.Element)->Option<String> = e.name;",
        // A reject predicate may be any function value.
        "fn big(x:Int64)->Bool = x > 1;\nfn f(xs:Sequence<Int64>)->Sequence<Int64> = Sequence(reject(xs, big));",
        // Source declarations named like a prelude operation take precedence.
        "fn reject(xs:Sequence<Int64>, n:Int64)->Int64 = n;\nfn f(xs:Sequence<Int64>)->Int64 = reject(xs, 3);",
        // Expansions cannot be captured by a source function named like a builtin.
        "fn asType(x:Int64)->Int64 = x;\nfn f(e:m.Element)->Bool = match e { _: m.Type => true, _ => false };",
        // Nested type switches.
        "fn f(e:m.Element, g:m.Element)->Int64 = match e { _: m.Type => match g { _: m.Type => 2, _ => 1 }, _ => 0 };",
    ] {
        compile(body).unwrap_or_else(|error| panic!("{body}: {error}"));
    }
}

#[test]
fn rejected_forms() {
    for (body, message) in [
        (
            "fn f(e:m.Element)->Int64 = match e { _: m.Type => 1, _: m.Element => 2 };",
            "must end with a `_` or binding catch-all",
        ),
        (
            "fn f(e:Option<m.Element>)->Int64 = match e { _: m.Type => 1, None => 2, _ => 3 };",
            "cannot be mixed",
        ),
        (
            "fn f(xs:Sequence<m.Element>)->Sequence<m.Type> = xs.selectByKind();",
            "selectByKind takes 1 type argument(s) and 1 argument(s)",
        ),
        (
            "fn f(xs:Sequence<Int64>)->Bool = notEmpty(xs, xs);",
            "notEmpty takes 0 type argument(s) and 1 argument(s)",
        ),
        (
            "fn f(`#builtin.size`:Int64)->Int64 = 0;",
            "reserved for parser expansions",
        ),
        // Kind tests need model-bounded operands.
        (
            "fn f(x:Int64)->Bool = x.oclIsKindOf::<m.Type>();",
            "model-bounded",
        ),
    ] {
        let error = compile(body).expect_err(body);
        assert!(error.contains(message), "{body}: {error}");
    }
}

fn int64(value: i64) -> Value {
    json!({"kind": "int64", "value": value.to_string()})
}
