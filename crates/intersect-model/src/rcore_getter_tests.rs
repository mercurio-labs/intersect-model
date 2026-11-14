//! Source feature ABI and borrowed getter execution; no language qualification.
use crate::{
    core::query::QueryBudget,
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        compiler::compile_structure_bundle,
        conversion::InvocationContext,
        rcore_behavior::ir_v2::{DataWire, Limits},
        rcore_getter::{VerifiedGetter, compile_getter},
        resources::DeclarationId,
        schema::Schema,
    },
    rcore_behavior_functions::{ExecutionContext, ExecutionLimits},
    scope_runtime::*,
    semantic_bindings::BorrowedBindingWork,
};
use serde_json::{Value, json};

const SOURCE: &str = r#"rcore 1;
package getter.controls version "1";
metamodel "urn:getter-controls" prefix m {
  class Base { attr declaredName: String [0,1]; }
  class Element extends Base { attr name: String [0,1] derived; }
}
behavior {
  import model "urn:getter-controls" as m;
  derive m.Element.name(self) -> Option<String> = effective_name(self);
  fn effective_name(self:m.Element) -> Option<String> =
    match self.declaredName { Some(name) => if name == "P" { Some("mapped") } else { Some(name) }, None => None };
}
"#;
fn fixture(text: &str) -> Result<(DefinitionBundle, VerifiedGetter, Schema), String> {
    let source = DefinitionSource::new("memory:getter.rcore", SourceKind::Rcore, text);
    let entry = source.id.clone();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let getter = compile_getter(&bundle, &entry, 1, &Limits::default(), Default::default())
        .map_err(|e| format!("{e:?}"))?;
    let schema = compile_structure_bundle(&bundle, &entry, "urn:getter-controls")
        .map_err(|e| format!("{e:?}"))?;
    Ok((bundle, getter, schema))
}
struct Read<'a> {
    schema: &'a Schema,
    input: Value,
    pending: bool,
}
impl ScopeRead for Read<'_> {
    fn check(&self, observation: &InvocationContext<'_>) -> Result<(), String> {
        observation.checkpoint()
    }
    fn class(&self, object: &str, observation: &InvocationContext<'_>) -> Result<String, String> {
        observation.work(1)?;
        if object != "receiver" {
            return Err("Unadmitted test receiver".into());
        }
        self.schema.language_class_key("Element").map(str::to_owned)
    }
    fn singular(
        &self,
        object: &str,
        member: &DeclarationId,
        observation: &InvocationContext<'_>,
    ) -> Result<ScopeValue, String> {
        self.class(object, observation)?;
        observation.work(1)?;
        let wanted = self
            .schema
            .effective_feature("Element", "declaredName")?
            .identity;
        if wanted
            != (crate::definitions::schema::MemberIdentity::Source {
                declaration: member.clone(),
            })
        {
            return Err("Unadmitted dependency".into());
        }
        if self.pending {
            Ok(ScopeValue::Pending)
        } else {
            Ok(ScopeValue::Value(self.input.clone()))
        }
    }
    fn page(
        &self,
        _: &str,
        _: &DeclarationId,
        _: usize,
        _: &InvocationContext<'_>,
    ) -> Result<ScopePage, String> {
        Err("No collection dependency installed".into())
    }
}
fn value(
    bundle: &DefinitionBundle,
    getter: &VerifiedGetter,
    read: &Read<'_>,
    budget: &mut QueryBudget,
) -> Result<Value, String> {
    let result = evaluate_getter(
        getter,
        &ExecutionContext::new(bundle),
        read.schema,
        read,
        "receiver",
        &ExecutionLimits::default(),
        &BorrowedBindingWork::bind(budget),
    )
    .map_err(|e| e.to_string())?;
    match result.data {
        DataWire::None => Ok(Value::Null),
        DataWire::Some(value) => match value.data {
            DataWire::Scalar(value) => Ok(value.payload),
            _ => Err("Expected scalar".into()),
        },
        _ => Err("Expected optional scalar".into()),
    }
}
#[test]
fn source_getter_binds_feature_and_executes_body_on_current_dependency() -> Result<(), String> {
    let (bundle, getter, schema) = fixture(SOURCE)?;
    assert_eq!(
        getter.member(),
        &schema.effective_feature("Element", "name")?.identity
    );
    for (input, expected) in [
        (json!("P"), json!("mapped")),
        (Value::Null, Value::Null),
        (json!("renamed"), json!("renamed")),
    ] {
        assert_eq!(
            value(
                &bundle,
                &getter,
                &Read {
                    schema: &schema,
                    input,
                    pending: false
                },
                &mut QueryBudget::new(1_000_000)
            )?,
            expected
        );
    }
    let changed = SOURCE.replace("Some(\"mapped\")", "None");
    let (changed_bundle, changed_getter, changed_schema) = fixture(&changed)?;
    assert_ne!(getter.span().source, changed_getter.span().source);
    assert_eq!(
        value(
            &changed_bundle,
            &changed_getter,
            &Read {
                schema: &changed_schema,
                input: json!("P"),
                pending: false
            },
            &mut QueryBudget::new(1_000_000)
        )?,
        Value::Null
    );
    assert!(
        value(
            &changed_bundle,
            &getter,
            &Read {
                schema: &schema,
                input: json!("P"),
                pending: false
            },
            &mut QueryBudget::new(1_000_000)
        )
        .is_err()
    );
    Ok(())
}
#[test]
fn source_getter_rejects_wrong_feature_signature_and_external_implementation() {
    let marker = SOURCE
        .find("  fn effective_name")
        .expect("fixture function");
    let external = format!(
        "{}  extern fn effective_name(self:m.Element) -> Option<String>;\n}}\n",
        &SOURCE[..marker]
    );
    for source in [
        SOURCE.replace("self:m.Element", "self:m.Base"),
        SOURCE.replace("String [0,1] derived", "String [1,1] derived"),
        SOURCE.replace(" derived;", ";"),
        SOURCE.replace("effective_name(self);", "effective_name(self, self);"),
        external,
        SOURCE.replace("fn effective_name(", "fn effective_name<T>("),
    ] {
        assert!(fixture(&source).is_err(), "accepted {source}");
    }
}
#[test]
fn source_getter_preserves_pending_and_original_exact_short_budgets() -> Result<(), String> {
    let (bundle, getter, schema) = fixture(SOURCE)?;
    let read = Read {
        schema: &schema,
        input: json!("P"),
        pending: false,
    };
    let mut measured = QueryBudget::new(1_000_000);
    assert_eq!(
        value(&bundle, &getter, &read, &mut measured)?,
        json!("mapped")
    );
    let prior = 7;
    let mut exact = QueryBudget::new(prior + measured.reads);
    exact.charge_many(prior)?;
    assert_eq!(value(&bundle, &getter, &read, &mut exact)?, json!("mapped"));
    assert_eq!(exact.remaining(), 0);
    for limit in [0, measured.reads - 1] {
        let mut short = QueryBudget::new(limit);
        assert!(value(&bundle, &getter, &read, &mut short).is_err());
        assert_eq!(short.remaining(), 0);
    }
    let pending = Read {
        pending: true,
        ..read
    };
    assert!(
        value(&bundle, &getter, &pending, &mut QueryBudget::new(1_000_000))
            .err()
            .is_some_and(|message| message.contains("pending"))
    );
    Ok(())
}
