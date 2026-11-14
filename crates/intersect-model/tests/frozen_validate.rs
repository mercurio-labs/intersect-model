//! Rcore `bind validate` on the frozen engine: read-only functions that return
//! `Sequence<Issue>` for every object of a class, run over a validated model.
use intersect_model::{
    core::{
        frozen::{ById, FrozenStore, INTERNAL_VALIDATION, Metamodel, Model, ObjectIssue, Severity},
        storage::{self, ModelStore},
    },
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        compiler::compile_structure_bundle,
        ecore::ParseLimits,
        rcore_behavior::ir_v2::Limits,
        rcore_scope::{
            BindingKind, VerifiedScope, compile_binding_kinds_with_source_limits,
            compile_validate_with_source_limits, load_bindings, save_bindings,
            source_validate_bindings,
        },
        schema::Schema,
    },
    frozen_scopes::{Execution, RcoreScopes},
};
use std::sync::Arc;

const SOURCE: &str = r##"rcore 1;
package example.frozenValidate version "1";
metamodel "urn:frozen-validate" prefix m {
  class Node { attr name: String [0,1]; ref members: Node [0,*] containment; }
  class Special extends Node { }
}
behavior {
  import model "urn:frozen-validate" as m;
  bind validate m.Node = named;
  bind validate m.Special = small;
  bind validate m.Node = deep;
  fn named(n:m.Node)->Sequence<Issue> = match n.name {
    Some(_) => [],
    None => [issue("warning", "test.named", "A node should have a name")]
  };
  fn small(n:m.Special)->Sequence<Issue> =
    if size(n.members) > 1 {
      [issue_rule("error", "test.small", "A special node has at most one member", "members",
        "Test.small")]
    } else { [] };
  // Visits every member twice: cheap, but more than a tiny step budget.
  fn deep(n:m.Node)->Sequence<Issue> =
    n.members.flatMap(|c| c.members.map(|g| issue("info", "test.deep", "grandchild")));
}
"##;

fn err(e: impl std::fmt::Debug) -> String {
    format!("{e:?}")
}

fn bundle(uri: &str, text: &str) -> DefinitionBundle {
    DefinitionBundle {
        version: 1,
        sources: vec![DefinitionSource::new(uri, SourceKind::Rcore, text)],
        namespaces: vec![],
    }
}

fn class_key(schema: &Schema, name: &str) -> Result<String, String> {
    schema
        .declarations
        .iter()
        .find(|(k, d)| d.name == name && schema.classes.contains_key(*k))
        .map(|(k, _)| k.clone())
        .ok_or_else(|| format!("Missing class {name}"))
}

struct Fixture {
    bundle: DefinitionBundle,
    schema: Schema,
    validations: Vec<VerifiedScope>,
    metamodel: Arc<Metamodel>,
}

fn fixture() -> Result<Fixture, String> {
    let bundle = bundle("memory:frozen-validate.rcore", SOURCE);
    let entry = bundle.sources[0].id.clone();
    let schema = compile_structure_bundle(&bundle, &entry, "urn:frozen-validate").map_err(err)?;
    let declarations: Vec<_> = source_validate_bindings(&bundle.sources[0], ParseLimits::default())
        .map_err(err)?
        .iter()
        .map(|b| (b.declaration, BindingKind::Validate))
        .collect();
    assert_eq!(declarations.len(), 3);
    let validations = compile_binding_kinds_with_source_limits(
        &bundle,
        &entry,
        &declarations,
        &Limits::default(),
        ParseLimits::default(),
    )
    .map_err(err)?;
    assert!(validations.iter().all(VerifiedScope::is_validate));
    let metamodel = Arc::new(Metamodel::freeze(&schema).map_err(err)?);
    Ok(Fixture {
        bundle,
        schema,
        validations,
        metamodel,
    })
}

/// `plain` (unnamed Node), `special` (unnamed Special with two members, one of
/// which has a member) and its named members.
fn model(f: &Fixture) -> Result<Model, String> {
    let node = class_key(&f.schema, "Node")?;
    let special = class_key(&f.schema, "Special")?;
    let schema = &f.schema;
    let mut store = FrozenStore::new(f.metamodel.clone());
    storage::atomic(&mut store, |s| {
        storage::bind_schema(s, schema)?;
        s.create_element(schema, "plain", &node, "memory:r", 0, 4)?;
        s.create_element(schema, "special", &special, "memory:r", 5, 20)?;
        s.create_element(schema, "a", &node, "memory:r", 6, 8)?;
        s.create_element(schema, "b", &node, "memory:r", 9, 12)?;
        s.create_element(schema, "c", &node, "memory:r", 10, 11)?;
        for named in ["a", "b", "c"] {
            s.set_value(named, "name", Some(serde_json::json!(named)))?;
        }
        s.set_value("special", "members", Some(serde_json::json!(["a", "b"])))?;
        s.set_value("b", "members", Some(serde_json::json!(["c"])))
    })?;
    store
        .into_builder()
        .map_err(err)?
        .link(&ById)
        .map_err(err)?
        .validate()
        .map_err(err)
}

fn summary(
    model: &Model,
    issues: &[ObjectIssue],
) -> Vec<(String, Severity, String, Option<String>)> {
    issues
        .iter()
        .map(|found| {
            (
                model
                    .obj(found.object)
                    .map(|o| o.identity().to_owned())
                    .unwrap_or_default(),
                found.issue.severity,
                found.issue.code.clone(),
                found.issue.feature.clone(),
            )
        })
        .collect()
}

/// Issues come in object order, then binding declaration order; a binding
/// applies to its class and its subclasses; both executors agree.
#[test]
fn validations_report_issues_in_object_then_declaration_order() -> Result<(), String> {
    let f = fixture()?;
    let model = model(&f)?;
    let expected = vec![
        (
            "plain".to_owned(),
            Severity::Warning,
            "test.named".to_owned(),
            None,
        ),
        (
            "special".to_owned(),
            Severity::Warning,
            "test.named".to_owned(),
            None,
        ),
        (
            "special".to_owned(),
            Severity::Error,
            "test.small".to_owned(),
            Some("members".to_owned()),
        ),
        (
            "special".to_owned(),
            Severity::Info,
            "test.deep".to_owned(),
            None,
        ),
    ];
    for execution in [Execution::Fast, Execution::Shadow, Execution::Vm] {
        let scopes = RcoreScopes::new(&f.metamodel, &f.bundle, &f.validations)
            .map_err(err)?
            .with_execution(execution);
        let issues = model.check(&scopes);
        assert_eq!(summary(&model, &issues), expected, "{execution:?}");
        let stats = scopes.stats();
        if execution == Execution::Vm {
            assert_eq!(stats.fast, 0);
        } else {
            assert_eq!(stats.deferred, 0, "{execution:?}: {stats:?}");
            assert!(stats.fast > 0);
        }
    }
    let scopes = RcoreScopes::new(&f.metamodel, &f.bundle, &f.validations).map_err(err)?;
    let issues = model.check(&scopes);
    assert_eq!(
        issues[2].issue.message,
        "A special node has at most one member"
    );
    assert_eq!(issues[2].issue.rule.as_deref(), Some("Test.small"));
    assert_eq!(issues[0].issue.rule, None);
    assert_eq!(issues, model.check(&scopes), "deterministic");
    Ok(())
}

/// A validation whose budget runs out is reported as an internal error issue
/// on the object, not a panic, and the other objects are still checked.
#[test]
fn exhausted_budget_is_an_internal_issue() -> Result<(), String> {
    let f = fixture()?;
    let model = model(&f)?;
    let mut limits = RcoreScopes::default_limits();
    limits.scalar.max_steps = 12;
    let scopes = RcoreScopes::new(&f.metamodel, &f.bundle, &f.validations)
        .map_err(err)?
        .with_limits(limits)
        .with_budget(12);
    let issues = model.check(&scopes);
    let internal: Vec<_> = issues
        .iter()
        .filter(|found| found.issue.code == INTERNAL_VALIDATION)
        .collect();
    assert!(!internal.is_empty(), "{issues:?}");
    assert!(
        internal
            .iter()
            .all(|found| found.issue.severity == Severity::Error)
    );
    // Objects whose validations fit the budget still report their issues.
    assert!(
        issues.iter().any(|found| found.issue.code == "test.named"),
        "{issues:?}"
    );
    Ok(())
}

/// Only objects added after a prepared model are checked.
#[test]
fn prepared_objects_are_not_checked_again() -> Result<(), String> {
    let f = fixture()?;
    let prepared = model(&f)?;
    let node = class_key(&f.schema, "Node")?;
    let mut store = FrozenStore::from_model(prepared);
    let schema = &f.schema;
    storage::atomic(&mut store, |s| {
        s.create_element(schema, "later", &node, "memory:s", 0, 5)
    })?;
    let model = store
        .into_builder()
        .map_err(err)?
        .link(&ById)
        .map_err(err)?
        .validate()
        .map_err(err)?;
    assert_eq!(model.prepared(), 5);
    let scopes = RcoreScopes::new(&f.metamodel, &f.bundle, &f.validations).map_err(err)?;
    assert_eq!(
        summary(&model, &model.check(&scopes)),
        vec![(
            "later".to_owned(),
            Severity::Warning,
            "test.named".to_owned(),
            None
        )]
    );
    Ok(())
}

/// Saved validations load with their kind and run like the compiled ones.
#[test]
fn saved_validations_round_trip() -> Result<(), String> {
    let f = fixture()?;
    let saved = save_bindings(&f.validations, "compiler-a")?;
    let loaded = load_bindings(&saved, &f.bundle, "compiler-a")?;
    assert!(loaded.iter().all(|s| s.kind() == BindingKind::Validate));
    let image = |s: &VerifiedScope| format!("{:?}\n{:?}", s.binding(), s.executable());
    assert_eq!(
        loaded.iter().map(image).collect::<Vec<_>>(),
        f.validations.iter().map(image).collect::<Vec<_>>()
    );
    let model = model(&f)?;
    let compiled = RcoreScopes::new(&f.metamodel, &f.bundle, &f.validations).map_err(err)?;
    let reloaded = RcoreScopes::new(&f.metamodel, &f.bundle, &loaded).map_err(err)?;
    assert_eq!(model.check(&compiled), model.check(&reloaded));
    Ok(())
}

fn refused(body: &str, expected: &str) -> Result<(), String> {
    let text = format!(
        "rcore 1;\npackage example.refused version \"1\";\n\
         metamodel \"urn:refused\" prefix m {{ class Node {{ attr name: String [0,1]; }} }}\n\
         behavior {{\n  import model \"urn:refused\" as m;\n{body}\n}}\n"
    );
    let bundle = bundle("memory:refused.rcore", &text);
    let entry = bundle.sources[0].id.clone();
    let declaration = source_validate_bindings(&bundle.sources[0], ParseLimits::default())
        .map_err(err)?
        .first()
        .map(|b| b.declaration)
        .ok_or("validate binding")?;
    let error = compile_validate_with_source_limits(
        &bundle,
        &entry,
        declaration,
        &Limits::default(),
        ParseLimits::default(),
    )
    .err()
    .ok_or_else(|| format!("must not compile: {body}"))?;
    assert!(
        error.iter().any(|d| d.message.contains(expected)),
        "{expected}: {error:?}"
    );
    Ok(())
}

/// The contract: one receiver parameter, `Sequence<Issue>`, read-only.
#[test]
fn validations_with_another_contract_are_refused() -> Result<(), String> {
    refused(
        "bind validate m.Node = f;\nfn f(n:m.Node)->Sequence<String> = [];",
        "must return Sequence<Issue>",
    )?;
    refused(
        "bind validate m.Node = f;\n\
         fn f(n:m.Node)->Sequence<{severity: String, rule: String}> = [];",
        "must return Sequence<Issue>",
    )?;
    refused(
        "bind validate m.Node = f;\nfn f(n:m.Node)->Issue = issue(\"error\", \"c\", \"m\");",
        "must return Sequence<Issue>",
    )?;
    refused(
        "bind validate m.Node = f;\nfn f(n:m.Node, s:String)->Sequence<Issue> = [];",
        "exactly the receiver model parameter",
    )?;
    refused(
        "bind validate m.Node = f;\n\
         fn f(n:m.Node)->Sequence<Issue> = match Some(new m.Node { name: \"x\" }) {\n\
           Some(_) => [], None => [] };",
        "`new` is allowed only",
    )?;
    Ok(())
}

const INT32_SOURCE: &str = r##"rcore 1;
package example.frozenInt32 version "1";
metamodel "urn:frozen-int32" prefix m {
  class Node { attr size: Int32 [0,1]; attr sizes: Int32 [0,*]; }
}
behavior {
  import model "urn:frozen-int32" as m;
  bind validate m.Node = sized;
  bind validate m.Node = squared;
  // Int32 stored values, literals, comparisons, negation and arithmetic.
  fn sized(n:m.Node)->Sequence<Issue> = match n.size {
    Some(s) => {
      let limit:Int32 = 10;
      let found = Sequence(n.sizes).exists(|x| x == s + 1);
      if s > limit || -s > 0 || s - 1 >= limit || found {
        [issue("warning", "test.size", "size out of range")]
      } else { [] }
    },
    None => []
  };
  // Overflows Int32 for sizes above 46340: the VM reports the error.
  fn squared(n:m.Node)->Sequence<Issue> = match n.size {
    Some(s) => if s * s > 2000000000 { [issue("info", "test.square", "large")] } else { [] },
    None => []
  };
}
"##;

/// Int32 values run on the direct executor with the VM's results; an Int32
/// overflow defers to the VM, which reports it.
#[test]
fn int32_validations_run_on_the_direct_executor() -> Result<(), String> {
    let bundle = bundle("memory:frozen-int32.rcore", INT32_SOURCE);
    let entry = bundle.sources[0].id.clone();
    let schema = compile_structure_bundle(&bundle, &entry, "urn:frozen-int32").map_err(err)?;
    let declarations: Vec<_> = source_validate_bindings(&bundle.sources[0], ParseLimits::default())
        .map_err(err)?
        .iter()
        .map(|b| (b.declaration, BindingKind::Validate))
        .collect();
    let validations = compile_binding_kinds_with_source_limits(
        &bundle,
        &entry,
        &declarations,
        &Limits::default(),
        ParseLimits::default(),
    )
    .map_err(err)?;
    let metamodel = Arc::new(Metamodel::freeze(&schema).map_err(err)?);
    let node = class_key(&schema, "Node")?;
    let build = |values: &[(&str, Option<i64>, &[i64])]| -> Result<Model, String> {
        let mut store = FrozenStore::new(metamodel.clone());
        storage::atomic(&mut store, |s| {
            storage::bind_schema(s, &schema)?;
            for (index, (id, size, sizes)) in values.iter().enumerate() {
                s.create_element(&schema, id, &node, "memory:r", index * 2, index * 2 + 1)?;
                if let Some(size) = size {
                    s.set_value(id, "size", Some(serde_json::json!(size)))?;
                }
                s.set_value(id, "sizes", Some(serde_json::json!(sizes)))?;
            }
            Ok(())
        })?;
        store
            .into_builder()
            .map_err(err)?
            .link(&ById)
            .map_err(err)?
            .validate()
            .map_err(err)
    };
    let in_range = build(&[
        ("none", None, &[]),
        ("small", Some(3), &[1, 2]),
        ("large", Some(11), &[]),
        ("negative", Some(-2), &[]),
        ("next", Some(5), &[6]),
        ("square", Some(46000), &[]),
        ("lowest", Some(i64::from(i32::MIN) + 1), &[]),
    ])?;
    let expected = [
        ("large", "test.size"),
        ("negative", "test.size"),
        ("next", "test.size"),
        ("square", "test.size"),
        ("square", "test.square"),
    ];
    let codes = |model: &Model, issues: &[ObjectIssue]| -> Vec<(String, String)> {
        summary(model, issues)
            .into_iter()
            .map(|(id, _, code, _)| (id, code))
            .collect()
    };
    let mut results = Vec::new();
    for execution in [Execution::Fast, Execution::Shadow, Execution::Vm] {
        let scopes = RcoreScopes::new(&metamodel, &bundle, &validations)
            .map_err(err)?
            .with_execution(execution);
        let issues = in_range.check(&scopes);
        let found = codes(&in_range, &issues);
        let wanted: Vec<_> = expected
            .iter()
            .map(|(id, code)| ((*id).to_owned(), (*code).to_owned()))
            .collect();
        // `lowest` squared overflows: an internal issue from the VM.
        let lowest: Vec<_> = issues
            .iter()
            .filter(|f| in_range.obj(f.object).map(|o| o.identity()) == Some("lowest"))
            .collect();
        assert!(
            lowest.len() == 1 && lowest[0].issue.code == INTERNAL_VALIDATION,
            "{execution:?}: {lowest:?}"
        );
        assert_eq!(
            found
                .into_iter()
                .filter(|(id, _)| id != "lowest")
                .collect::<Vec<_>>(),
            wanted,
            "{execution:?}"
        );
        let stats = scopes.stats();
        if execution == Execution::Vm {
            assert_eq!(stats.fast, 0);
        } else {
            // Only the overflow defers.
            assert_eq!(
                stats.deferred_by_reason,
                vec![("integer overflow or division", 1)],
                "{execution:?}: {stats:?}"
            );
        }
        results.push(issues);
    }
    assert_eq!(results[0], results[2], "direct and VM issues agree");
    Ok(())
}
