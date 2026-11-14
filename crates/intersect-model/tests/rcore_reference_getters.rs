#[path = "../../../fixtures/rcore-getter-service/references.rs"]
mod references;

#[test]
fn live_reference_getters_preserve_contracts_and_original_caller_work() -> Result<(), String> {
    references::verify()
}

#[test]
fn reference_getter_signatures_keep_multiplicity_order_and_uniqueness() {
    for replacement in [
        "m.Node[0,2]",
        "m.Node[0,3] unordered",
        "m.Node[0,3] nonunique",
        "m.Node[3,2]",
        "m.Node[0,2147483648]",
        "m.Node[0,3] unique nonunique",
    ] {
        let source = references::SOURCE.replace("-> m.Node[0,3]", &format!("-> {replacement}"));
        assert!(
            intersect_model::compile_rcore_at("memory:reference-getters.rcore", &source).is_err(),
            "{replacement}"
        );
    }
}

#[test]
fn reference_getters_reject_pending_unavailable_and_foreign_nominal_targets() -> Result<(), String>
{
    use intersect_model::{
        core::query::QueryBudget,
        definitions::{
            conversion::InvocationContext,
            rcore_behavior::ir_v2::Limits,
            rcore_getter::{compile_getter, source_getter_rules},
            resources::DeclarationId,
            schema::Expression,
        },
        rcore_behavior_functions::{ExecutionContext, ExecutionLimits},
        scope_runtime::{ScopePage, ScopeRead, ScopeValue},
        semantic_bindings::BorrowedBindingWork,
    };
    enum Mode {
        Absent,
        Pending,
        Unavailable,
        Foreign,
        Missing,
    }
    struct Reads<'a> {
        node: &'a str,
        other: &'a str,
        mode: Mode,
    }
    impl ScopeRead for Reads<'_> {
        fn check(&self, observation: &InvocationContext<'_>) -> Result<(), String> {
            observation.work(1)
        }
        fn class(
            &self,
            object: &str,
            observation: &InvocationContext<'_>,
        ) -> Result<String, String> {
            observation.work(1)?;
            match object {
                "owner" => Ok(self.node.into()),
                "target" if matches!(self.mode, Mode::Foreign) => Ok(self.other.into()),
                _ => Err("Missing source target".into()),
            }
        }
        fn singular(
            &self,
            _: &str,
            _: &DeclarationId,
            observation: &InvocationContext<'_>,
        ) -> Result<ScopeValue, String> {
            observation.work(1)?;
            Ok(match self.mode {
                Mode::Absent => ScopeValue::Absent,
                Mode::Pending => ScopeValue::Pending,
                Mode::Unavailable => ScopeValue::Unavailable,
                Mode::Foreign | Mode::Missing => ScopeValue::Value(serde_json::json!("target")),
            })
        }
        fn page(
            &self,
            _: &str,
            _: &DeclarationId,
            _: usize,
            _: &InvocationContext<'_>,
        ) -> Result<ScopePage, String> {
            Err("Unexpected collection read".into())
        }
    }
    let source = references::SOURCE.replace(
        "class Child extends Node {}",
        "class Child extends Node {} class Other {}",
    );
    let package = intersect_model::compile_rcore_at("memory:reference-getters.rcore", &source)
        .map_err(|e| e.to_string())?;
    let entry = package
        .sources()
        .sources
        .iter()
        .find(|s| s.id == *package.entry())
        .ok_or("Missing source")?;
    let rules = source_getter_rules(entry, package.schema(), Default::default())
        .map_err(|e| format!("{e:?}"))?;
    let rule = rules
        .iter()
        .find(|r| r.feature == "optional")
        .ok_or("Missing optional getter")?;
    let Expression::Source {
        source,
        declaration,
    } = &rule.expression
    else {
        return Err("Missing source selector".into());
    };
    let getter = compile_getter(
        package.sources(),
        source,
        *declaration,
        &Limits::default(),
        Default::default(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let key = |name: &str| {
        package
            .schema()
            .declarations
            .iter()
            .find(|(_, d)| d.name == name)
            .map(|(key, _)| key.as_str())
            .ok_or("Missing class")
    };
    for mode in [
        Mode::Absent,
        Mode::Pending,
        Mode::Unavailable,
        Mode::Foreign,
        Mode::Missing,
    ] {
        let absent = matches!(mode, Mode::Absent);
        let reads = Reads {
            node: key("Node")?,
            other: key("Other")?,
            mode,
        };
        let mut budget = QueryBudget::new(10_000_000);
        let work = BorrowedBindingWork::bind(&mut budget);
        let result = intersect_model::scope_runtime::evaluate_getter(
            &getter,
            &ExecutionContext::new(package.sources()),
            package.schema(),
            &reads,
            "owner",
            &ExecutionLimits::default(),
            &work,
        );
        if absent {
            assert!(matches!(
                result,
                Ok(intersect_model::rcore::ValueWire {
                    data: intersect_model::rcore::DataWire::None,
                    ..
                })
            ));
        } else {
            assert!(result.is_err());
        }
    }
    Ok(())
}
