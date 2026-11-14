use super::*;
use crate::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    rcore_behavior::ir_v2::Limits,
};
use std::collections::BTreeMap;

const SOURCE: &str = r#"rcore 1;
package example.scopeRuntime version "1";
metamodel "urn:scope-runtime" prefix m {
  class Namespace { ref types: Type [0,*] containment; }
  class Type { attr name: String [1,1]; }
  class Property {
    ref owner: Namespace [0,1];
    ref targetType: Type [0,1];
  }
}
behavior {
  import model "urn:scope-runtime" as m;
  bind scope m.Property.targetType = visible_types;
  fn visible_types(owner:m.Property, spelling:String)->Sequence<m.Type> =
    match owner.owner {
      Some(ns) => Sequence(filter(ns.types, |t| t.name == spelling)),
      None => []
    };
}
"#;

fn fixture() -> Result<(DefinitionBundle, VerifiedScope, Schema), String> {
    fixture_source(SOURCE)
}
fn fixture_source(text: &str) -> Result<(DefinitionBundle, VerifiedScope, Schema), String> {
    let source = DefinitionSource::new("memory:scope-runtime.rcore", SourceKind::Rcore, text);
    let entry = source.id.clone();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let scope =
        crate::definitions::rcore_scope::compile_scope(&bundle, &entry, 1, &Limits::default())
            .map_err(|e| format!("{e:?}"))?;
    let schema = crate::definitions::compiler::compile_structure_bundle(
        &bundle,
        &entry,
        "urn:scope-runtime",
    )
    .map_err(|e| format!("{e:?}"))?;
    Ok((bundle, scope, schema))
}

struct Read<'a> {
    schema: &'a Schema,
    objects: BTreeMap<&'static str, (&'static str, Value)>,
    owner_state: u8,
    page_state: u8,
    ownership_state: u8,
}
impl<'a> Read<'a> {
    fn new(schema: &'a Schema) -> Self {
        Self {
            schema,
            owner_state: 0,
            page_state: 0,
            ownership_state: 0,
            objects: BTreeMap::from([
                (
                    "property",
                    ("Property", serde_json::json!({"owner":"local"})),
                ),
                (
                    "local",
                    (
                        "Namespace",
                        serde_json::json!({"types":["voltage","current"]}),
                    ),
                ),
                (
                    "other",
                    ("Namespace", serde_json::json!({"types":["foreign"]})),
                ),
                ("voltage", ("Type", serde_json::json!({"name":"Voltage"}))),
                ("current", ("Type", serde_json::json!({"name":"Current"}))),
                ("foreign", ("Type", serde_json::json!({"name":"Voltage"}))),
            ]),
        }
    }
    fn field(&self, object: &str, member: &DeclarationId) -> Result<String, String> {
        let class = self.class(object, &InvocationContext::default())?;
        let field = self.schema.feature_by_id(
            &class,
            &MemberIdentity::Source {
                declaration: member.clone(),
            },
        )?;
        Ok(field.feature.name.clone())
    }
}
impl ScopeRead for Read<'_> {
    fn check(&self, observation: &InvocationContext<'_>) -> Result<(), String> {
        observation.checkpoint()
    }
    fn class(&self, object: &str, observation: &InvocationContext<'_>) -> Result<String, String> {
        observation.work(1)?;
        let name = self.objects.get(object).ok_or("Missing test object")?.0;
        self.schema
            .declarations
            .iter()
            .find(|(key, declaration)| {
                declaration.namespace == "urn:scope-runtime"
                    && declaration.name == name
                    && self.schema.classes.contains_key(*key)
            })
            .map(|(key, _)| key.clone())
            .ok_or_else(|| "Missing test class".into())
    }
    fn singular(
        &self,
        object: &str,
        member: &DeclarationId,
        observation: &InvocationContext<'_>,
    ) -> Result<ScopeValue, String> {
        observation.work(1)?;
        let field = self.field(object, member)?;
        if object == "property" && field == "owner" {
            match self.owner_state {
                1 => return Ok(ScopeValue::Pending),
                2 => return Ok(ScopeValue::Unavailable),
                3 => return Ok(ScopeValue::Absent),
                _ => {}
            }
        }
        Ok(
            match self
                .objects
                .get(object)
                .ok_or("Missing test object")?
                .1
                .get(field.as_str())
            {
                Some(value) => ScopeValue::Value(value.clone()),
                None => ScopeValue::Absent,
            },
        )
    }
    fn page(
        &self,
        object: &str,
        member: &DeclarationId,
        cursor: usize,
        observation: &InvocationContext<'_>,
    ) -> Result<ScopePage, String> {
        observation.work(1)?;
        match self.page_state {
            1 => return Ok(ScopePage::Pending),
            2 => return Ok(ScopePage::Unavailable),
            _ => {}
        }
        let field = self.field(object, member)?;
        let value = self
            .objects
            .get(object)
            .ok_or("Missing test object")?
            .1
            .get(field.as_str())
            .and_then(Value::as_array)
            .and_then(|values| values.get(cursor));
        Ok(match value {
            Some(value) => ScopePage::Value(value.clone()),
            None => ScopePage::End,
        })
    }
    fn ownership(
        &self,
        _: &str,
        _: OwnershipOp,
        _: &RuntimeType,
        _: usize,
        _: &ScopeSchema<'_>,
        observation: &InvocationContext<'_>,
    ) -> Result<ScopePage, String> {
        observation.work(1)?;
        Ok(match self.ownership_state {
            1 => ScopePage::Pending,
            2 => ScopePage::Unavailable,
            _ => ScopePage::End,
        })
    }
}
fn run(
    read: &Read<'_>,
    bundle: &DefinitionBundle,
    scope: &VerifiedScope,
    budget: usize,
) -> Result<ScopeCandidates, ScopeError> {
    run_with_work(read, bundle, scope, budget, 0).map(|(result, _)| result)
}
fn run_with_work(
    read: &Read<'_>,
    bundle: &DefinitionBundle,
    scope: &VerifiedScope,
    budget: usize,
    preconsumed: usize,
) -> Result<(ScopeCandidates, usize), ScopeError> {
    let mut caller = crate::core::query::QueryBudget::new(budget);
    for _ in 0..preconsumed {
        caller.charge().map_err(ScopeError::InvalidInput)?;
    }
    let work = BorrowedBindingWork::bind(&mut caller);
    let result = evaluate(
        scope,
        &ExecutionContext::new(bundle),
        read.schema,
        read,
        "property",
        "Voltage",
        &ExecutionLimits::default(),
        &work,
    );
    let used = usize::try_from(work.used())
        .map_err(|_| ScopeError::InvalidInput("Test work overflow".into()))?;
    assert_eq!(caller.reads, preconsumed + used);
    result.map(|result| (result, used))
}
#[test]
fn source_scope_uses_real_reference_collection_and_scalar_instructions() -> Result<(), String> {
    let (bundle, scope, schema) = fixture()?;
    let read = Read::new(&schema);
    let before = read.objects.clone();
    let result = run(&read, &bundle, &scope, 1_000_000).map_err(|e| e.to_string())?;
    assert_eq!(result.identities(), &["voltage"]);
    assert_eq!(read.objects, before);
    Ok(())
}
#[test]
fn source_scope_uses_shared_session_context_admission() -> Result<(), String> {
    let (mut bundle, scope, schema) = fixture()?;
    let read = Read::new(&schema);
    let before = read.objects.clone();
    bundle
        .sources
        .first_mut()
        .ok_or("Missing fixture source")?
        .text
        .push_str("\n// changed after scope preparation\n");
    assert!(run(&read, &bundle, &scope, 1_000_000).is_err());
    assert_eq!(read.objects, before);
    Ok(())
}
#[test]
fn source_scope_keeps_pending_unavailable_and_absent_distinct() -> Result<(), String> {
    let (bundle, scope, schema) = fixture()?;
    let mut read = Read::new(&schema);
    read.owner_state = 1;
    assert!(matches!(
        run(&read, &bundle, &scope, 1_000_000),
        Err(ScopeError::Pending { .. })
    ));
    read.owner_state = 2;
    assert!(matches!(
        run(&read, &bundle, &scope, 1_000_000),
        Err(ScopeError::Unavailable { .. })
    ));
    read.owner_state = 3;
    assert!(
        run(&read, &bundle, &scope, 1_000_000)
            .map_err(|e| e.to_string())?
            .identities()
            .is_empty()
    );
    Ok(())
}
#[test]
fn source_scope_rejects_wrong_reference_targets_and_original_budget_denial() -> Result<(), String> {
    let (bundle, scope, schema) = fixture()?;
    let mut read = Read::new(&schema);
    assert!(run(&read, &bundle, &scope, 0).is_err());
    read.objects
        .insert("voltage", ("Property", serde_json::json!({})));
    assert!(run(&read, &bundle, &scope, 1_000_000).is_err());
    Ok(())
}

#[test]
fn source_scope_respects_exact_short_and_preconsumed_original_allowances() -> Result<(), String> {
    let (bundle, scope, schema) = fixture()?;
    let read = Read::new(&schema);
    let (_, used) =
        run_with_work(&read, &bundle, &scope, 1_000_000, 0).map_err(|e| e.to_string())?;
    assert!(used > 1);
    assert_eq!(
        run_with_work(&read, &bundle, &scope, used, 0)
            .map_err(|e| e.to_string())?
            .0
            .identities(),
        &["voltage"]
    );
    assert!(matches!(
        run_with_work(&read, &bundle, &scope, used - 1, 0),
        Err(ScopeError::Boundary(_))
    ));
    assert!(run_with_work(&read, &bundle, &scope, used + 5, 5).is_ok());
    assert!(run_with_work(&read, &bundle, &scope, used + 4, 5).is_err());
    Ok(())
}

#[test]
fn source_scope_keeps_collection_and_ownership_dependencies_explicit() -> Result<(), String> {
    let (bundle, scope, schema) = fixture()?;
    let mut read = Read::new(&schema);
    read.page_state = 1;
    assert!(matches!(
        run(&read, &bundle, &scope, 1_000_000),
        Err(ScopeError::Pending { .. })
    ));
    read.page_state = 2;
    assert!(matches!(
        run(&read, &bundle, &scope, 1_000_000),
        Err(ScopeError::Unavailable { .. })
    ));
    let text = SOURCE.replace(
        "match owner.owner {\n      Some(ns) => Sequence(filter(ns.types, |t| t.name == spelling)),\n      None => []\n    }",
        "Sequence(contents::<m.Type>(owner))");
    assert_ne!(text, SOURCE);
    let (bundle, scope, schema) = fixture_source(&text)?;
    let mut read = Read::new(&schema);
    read.ownership_state = 1;
    assert!(matches!(
        run(&read, &bundle, &scope, 1_000_000),
        Err(ScopeError::PendingOwnership { .. })
    ));
    read.ownership_state = 2;
    assert!(matches!(
        run(&read, &bundle, &scope, 1_000_000),
        Err(ScopeError::UnavailableOwnership { .. })
    ));
    Ok(())
}

#[test]
fn source_scope_deduplicates_identities_after_checking_each_candidate() -> Result<(), String> {
    let text = SOURCE.replace(
        "Sequence(filter(ns.types, |t| t.name == spelling))",
        "Sequence(flatMap(ns.types, |t| [t,t]))",
    );
    assert_ne!(text, SOURCE);
    let (bundle, scope, schema) = fixture_source(&text)?;
    let mut read = Read::new(&schema);
    assert_eq!(
        run(&read, &bundle, &scope, 1_000_000)
            .map_err(|e| e.to_string())?
            .identities(),
        &["voltage", "current"]
    );
    read.objects
        .insert("current", ("Property", serde_json::json!({})));
    assert!(run(&read, &bundle, &scope, 1_000_000).is_err());
    Ok(())
}

#[test]
fn source_scope_rejects_duplicate_declared_unique_input_without_normalizing() -> Result<(), String>
{
    let (bundle, scope, schema) = fixture()?;
    let mut read = Read::new(&schema);
    read.objects.insert(
        "local",
        (
            "Namespace",
            serde_json::json!({"types":["voltage","voltage"]}),
        ),
    );
    let result = run(&read, &bundle, &scope, 1_000_000);
    assert!(
        matches!(&result, Err(ScopeError::Execution(error)) if error.message.contains("declared-unique")),
        "duplicate stored inputs must reject rather than deduplicate into a valid scope"
    );
    Ok(())
}
