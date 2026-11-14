//! Staged behavior-compilation verification. This does not close its full contract.
use intersect_model::definitions::{
    bundle::{DefinitionSource, SourceKind},
    rcore_behavior::{
        self,
        ast::{Declaration, Expr, Literal, Type},
    },
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
fn check(ok: bool, message: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(message.into()) }
}
fn compatibility() -> Result<(), String> {
    #[derive(Deserialize)]
    struct Capture {
        cases: Vec<Case>,
        diagnostic_free_documents: usize,
    }
    #[derive(Deserialize)]
    struct Case {
        source: DefinitionSource,
        label: String,
        expected_sha256: String,
        diagnostic_count: usize,
    }
    let bytes = include_bytes!(
        "../../../crates/intersect-model/assets/fixtures/rcore-behavior-ast-baseline.json"
    );
    check(
        format!("{:x}", Sha256::digest(bytes))
            == "9bf809471ae37ddbde47868d1d61f7e4f1ad47928a08a51234ad694799f48082",
        "Frozen behavior AST capture changed",
    )?;
    let capture: Capture = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    check(
        capture.cases.len() == 9197 && capture.diagnostic_free_documents == 821,
        "Incomplete behavior capture",
    )?;
    for case in capture.cases {
        let pointer = case.source.text.as_ptr();
        let deps = case.source.dependencies.as_ptr();
        let result = rcore_behavior::parse(case.source);
        check(
            result.source.text.as_ptr() == pointer && result.source.dependencies.as_ptr() == deps,
            "Ordinary behavior parse changed original source allocations",
        )?;
        let encoded = serde_json::to_vec(&result).map_err(|e| e.to_string())?;
        if format!("{:x}", Sha256::digest(encoded)) != case.expected_sha256
            || result.diagnostics.len() != case.diagnostic_count
        {
            return Err(format!(
                "Complete behavior AST/diagnostics changed for {}",
                case.label
            ));
        }
    }
    Ok(())
}
fn literals() -> Result<(), String> {
    let text = "rcore 1;package p version \"1\";fn run()->Int32=1+2;";
    let source = DefinitionSource::new(
        "memory:literal-behavior.rcore",
        SourceKind::RcoreBehavior,
        text,
    );
    let doc = rcore_behavior::parse(source.clone());
    check(
        doc.source == source
            && doc.diagnostics.is_empty()
            && doc.declarations.len() == 1
            && doc.types.len() == 1
            && doc.expressions.len() == 3
            && doc.patterns.is_empty(),
        "Literal behavior document/arena shape changed",
    )?;
    let Declaration::Function {
        name, result, body, ..
    } = &doc.declarations[0].value
    else {
        return Err("Literal function missing".into());
    };
    check(
        name.value == "run"
            && result.0 == 0
            && body.0 == 2
            && doc.declarations[0].span.start == 30
            && doc.declarations[0].span.end == 50,
        "Literal function arena IDs/span changed",
    )?;
    check(
        matches!(&doc.types[0].value,Type::Named{path,arguments} if path.len()==1 && path[0].value=="Int32" && arguments.is_empty()),
        "Literal named result type changed",
    )?;
    check(
        matches!(&doc.expressions[0].value,Expr::Literal(Literal::Number(n)) if n=="1")
            && matches!(&doc.expressions[1].value,Expr::Literal(Literal::Number(n)) if n=="2")
            && matches!(&doc.expressions[2].value,Expr::Binary{operator,left,right} if operator.value=="+" && left.0==0 && right.0==1),
        "Literal Pratt arena order changed",
    )?;
    check(
        doc.expressions[2].span.start == 46
            && doc.expressions[2].span.end == 49
            && doc.expressions.iter().all(|e| e.span.source == source.id),
        "Literal byte spans/source identity changed",
    )?;
    let source = DefinitionSource::new(
        "memory:recovery.rcore",
        SourceKind::RcoreBehavior,
        "rcore 1;package p version \"1\";fn bad( -> Int32 = ;fn good()->Int32=7;",
    );
    let recovered = rcore_behavior::parse(source.clone());
    check(
        recovered.diagnostics.len() == 1
            && recovered.diagnostics[0].message == "Expected an identifier"
            && recovered.diagnostics[0].span.source == source.id
            && recovered.declarations.len() == 2
            && matches!(recovered.declarations[0].value, Declaration::Error)
            && matches!(&recovered.declarations[1].value,Declaration::Function{name,..} if name.value=="good"),
        "Literal declaration recovery changed",
    )?;
    Ok(())
}
fn verify() -> Result<u32, String> {
    compatibility()?;
    literals()?;
    Ok(2)
}
#[cfg_attr(not(test), unsafe(no_mangle))]
pub extern "C" fn verify_rcore_behavior_compilation() -> u32 {
    verify().unwrap_or(0)
}
fn main() -> Result<(), String> {
    println!(
        "{} staged behavior frontend groups passed; full compilation contract remains open",
        verify()?
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_literal_arenas_spans_and_recovery() {
        literals().unwrap();
    }
}
