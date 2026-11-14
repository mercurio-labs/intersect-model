use intersect_model::{
    core::query::QueryBudget,
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        rcore_behavior::{ir_v2::EntrySelection, modules::SymbolId},
    },
    source_scalar_reads::SourceScalarRead,
};
use serde_json::json;
#[path = "../../../fixtures/rcore-getter-service/conjugated_name.rs"]
mod portable;

#[test]
fn authored_language_body_has_shared_native_wasm_controls() -> Result<(), String> {
    portable::verify()
}

fn compile(prefix: &str) -> Result<SourceScalarRead, String> {
    let source = DefinitionSource::new(
        "memory:concat.rcore",
        SourceKind::RcoreBehavior,
        format!(
            "rcore 1;package concat.control version \"1\";fn label(name:Option<String>)->Option<String> = match name {{ Some(name) => Some(\"{prefix}\" + name), None => None }};"
        ),
    );
    let entry = source.id.clone();
    SourceScalarRead::compile(
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
}

#[test]
fn source_string_add_preserves_unicode_null_and_original_caller_work() -> Result<(), String> {
    let program = compile("~")?;
    for text in [String::new(), "π & 'quoted'".into(), "é😀終".repeat(1000)] {
        let mut measured = QueryBudget::new(10_000_000);
        let expected = json!(format!("~{text}"));
        assert_eq!(program.invoke(json!(text), &mut measured)?, expected);
        let mut exact = QueryBudget::new(7 + measured.reads);
        exact.charge_many(7)?;
        assert_eq!(program.invoke(json!(text), &mut exact)?, expected);
        assert_eq!(exact.remaining(), 0);
        let mut short = QueryBudget::new(measured.reads - 1);
        assert!(program.invoke(json!(text), &mut short).is_err());
        assert!(program.invoke(json!(text), &mut short).is_err());
    }
    assert_eq!(
        program.invoke(json!(null), &mut QueryBudget::new(1_000_000))?,
        json!(null)
    );
    let changed = compile("=")?;
    assert_ne!(program.source(), changed.source());
    assert_eq!(
        changed.invoke(json!("P"), &mut QueryBudget::new(1_000_000))?,
        json!("=P")
    );
    assert!(
        program
            .invoke(json!(42), &mut QueryBudget::new(1_000_000))
            .is_err()
    );
    Ok(())
}
