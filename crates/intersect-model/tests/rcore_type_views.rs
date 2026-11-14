#[path = "../../../fixtures/rcore-getter-service/type_views.rs"]
mod portable;
#[test]
fn source_subtype_branches_share_live_membership_and_original_caller_work() -> Result<(), String> {
    portable::verify()
}
#[test]
fn type_views_reject_wrong_type_arity_and_nonmodel_operands() {
    for replacement in [
        "asType(node)",
        "asType::<String>(node)",
        "asType::<m.Leaf>()",
        "asType::<m.Leaf>(true)",
        "asType::<m.Leaf, m.Node>(node)",
    ] {
        let source = portable::SOURCE.replace("asType::<m.Leaf>(node)", replacement);
        assert!(
            intersect_model::compile_rcore_at("memory:type-views.rcore", &source).is_err(),
            "{replacement}"
        );
    }
}

#[test]
fn bare_none_is_a_constructor_pattern_and_cannot_hide_missing_cases() -> Result<(), String> {
    use intersect_model::{
        core::query::QueryBudget,
        definitions::{
            bundle::{DefinitionBundle, DefinitionSource, SourceKind},
            rcore_behavior::{ir_v2::EntrySelection, modules::SymbolId},
        },
        source_scalar_functions::SourceScalarFunction,
    };
    let compile = |body: &str| {
        let source = DefinitionSource::new(
            "memory:none-pattern.rcore",
            SourceKind::RcoreBehavior,
            format!("rcore 1; package pattern.control version \"1\"; {body}"),
        );
        let entry = source.id.clone();
        SourceScalarFunction::compile(
            DefinitionBundle {
                version: 1,
                sources: vec![source],
                namespaces: vec![],
            },
            &entry,
            &EntrySelection {
                symbol: SymbolId {
                    source: entry.clone(),
                    declaration: 0,
                },
                types: vec![],
                arguments: vec![],
            },
        )
    };
    let program = compile(
        "fn present(value:Option<String>)->Bool = match value { None => false, Some(_) => true };",
    )?;
    for (input, expected) in [
        (serde_json::Value::Null, false),
        (serde_json::json!(""), true),
    ] {
        assert_eq!(
            program.invoke(&[input], &mut QueryBudget::new(100_000))?,
            serde_json::Value::Bool(expected)
        );
    }
    for body in [
        "fn incomplete(value:Option<String>)->Bool = match value { None => false };",
        "fn wrong(value:Bool)->Bool = match value { None => false };",
        "fn repeated(value:Option<String>)->Bool = match value { None => false, None => true, Some(_) => true };",
    ] {
        assert!(compile(body).is_err(), "{body}");
    }
    Ok(())
}

#[test]
fn failed_or_unavailable_model_reads_never_become_a_type_mismatch() -> Result<(), String> {
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
        Match,
        NonMatch,
        Absent,
        Pending,
        Unavailable,
        Failed,
        Missing,
        Foreign,
    }
    struct Reads<'a> {
        mode: Mode,
        root: &'a str,
        leaf: &'a str,
        sibling: &'a str,
        next: &'a DeclarationId,
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
                "owner" => Ok(self.root.into()),
                "target" => match self.mode {
                    Mode::Match => Ok(self.leaf.into()),
                    Mode::NonMatch => Ok(self.sibling.into()),
                    Mode::Failed => Err("Deliberate classification read failure".into()),
                    Mode::Foreign => Ok("foreign-class".into()),
                    _ => Err("Missing classification target".into()),
                },
                _ => Err("Missing source object".into()),
            }
        }
        fn singular(
            &self,
            object: &str,
            member: &DeclarationId,
            observation: &InvocationContext<'_>,
        ) -> Result<ScopeValue, String> {
            observation.work(1)?;
            if object == "target" {
                return Ok(ScopeValue::Value(serde_json::json!("chosen")));
            }
            if member != self.next {
                return Err("Unexpected view fixture dependency".into());
            }
            Ok(match self.mode {
                Mode::Absent => ScopeValue::Absent,
                Mode::Pending => ScopeValue::Pending,
                Mode::Unavailable => ScopeValue::Unavailable,
                _ => ScopeValue::Value(serde_json::json!("target")),
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
        // Intentionally no ownership adapter: classification uses the shared
        // source authority and must not require physical containment navigation.
    }
    let package = intersect_model::compile_rcore_at("memory:type-views.rcore", portable::SOURCE)
        .map_err(|error| error.to_string())?;
    let source = package
        .sources()
        .sources
        .iter()
        .find(|source| source.id == *package.entry())
        .ok_or("Missing source")?;
    let rules = source_getter_rules(source, package.schema(), Default::default())
        .map_err(|errors| format!("{errors:?}"))?;
    let rule = rules
        .iter()
        .find(|rule| rule.feature == "nextLabel")
        .ok_or("Missing source getter")?;
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
    .map_err(|errors| format!("{errors:?}"))?;
    assert_eq!(
        getter
            .executable()
            .function_effect(getter.executable().program().entry)
            .map_err(|error| error.to_string())?,
        intersect_model::definitions::rcore_behavior::ir_v2::Effect::Query
    );
    let key = |name: &str| {
        package
            .schema()
            .declarations
            .iter()
            .find(|(_, declaration)| declaration.name == name)
            .map(|(key, _)| key.as_str())
            .ok_or("Missing source class")
    };
    let root = key("Node")?;
    let member = package.schema().effective_feature(root, "next")?;
    let intersect_model::definitions::schema::MemberIdentity::Source { declaration: next } =
        member.identity
    else {
        return Err("Missing source member".into());
    };
    for mode in [
        Mode::Match,
        Mode::NonMatch,
        Mode::Absent,
        Mode::Pending,
        Mode::Unavailable,
        Mode::Failed,
        Mode::Missing,
        Mode::Foreign,
    ] {
        let success = matches!(mode, Mode::Match | Mode::NonMatch | Mode::Absent);
        let absent = matches!(mode, Mode::NonMatch | Mode::Absent);
        let failed = matches!(mode, Mode::Failed);
        let reads = Reads {
            mode,
            root,
            leaf: key("Leaf")?,
            sibling: key("Sibling")?,
            next: &next,
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
        assert_eq!(result.is_ok(), success);
        if absent {
            assert!(matches!(
                result,
                Ok(intersect_model::rcore::ValueWire {
                    data: intersect_model::rcore::DataWire::None,
                    ..
                })
            ));
        } else if failed {
            assert!(
                result
                    .err()
                    .ok_or("Failure was hidden")?
                    .to_string()
                    .contains("Deliberate classification read failure")
            );
        }
    }
    Ok(())
}
