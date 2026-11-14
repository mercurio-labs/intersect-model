//! Verification-only complete pre-change compiler observations, never proof input.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, SourceId},
    rcore_behavior::{
        checking, function_lowering,
        ir_v2::{EntrySelection, Limits},
        modules,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[derive(Deserialize)]
struct Capture {
    cases: Vec<Case>,
    module_successes: usize,
    type_successes: usize,
    image_successes: usize,
}
#[derive(Deserialize)]
struct Case {
    label: String,
    bundle: DefinitionBundle,
    entry: SourceId,
    selection: EntrySelection,
    limits: Limits,
    modules_ok: bool,
    types_ok: bool,
    image_ok: bool,
    modules_sha256: String,
    types_sha256: String,
    image_sha256: String,
}
fn digest(value: &impl Serialize) -> Result<String, String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(|e| e.to_string())?)
    ))
}
pub fn verify(expected: &str) -> Result<(), String> {
    if format!("{:x}", Sha256::digest(expected.as_bytes()))
        != "f622c3319359b0eacd9e48913a657729715d5763b438070ef49e097ec33115ee"
    {
        return Err("Frozen pre-change compiler observations changed".into());
    }
    let capture: Capture = serde_json::from_str(expected).map_err(|e| e.to_string())?;
    if capture.cases.len() != 51
        || capture.module_successes != 47
        || capture.type_successes != 37
        || capture.image_successes != 17
    {
        return Err("Incomplete compiler observation inventory".into());
    }
    for case in capture.cases {
        let modules = modules::index_modules(&case.bundle, &case.entry);
        let types = checking::check(&case.bundle, &case.entry);
        let image = function_lowering::compile_function(
            &case.bundle,
            &case.entry,
            &case.selection,
            &case.limits,
        );
        let image_ok = image.is_ok();
        let image: Value = match image {
            Ok(image) => {
                json!({"Ok":{"program":image.program(),"effects":image.effects(),"complete_debug":format!("{image:?}")}})
            }
            Err(errors) => json!({"Err":errors}),
        };
        if modules.is_ok() != case.modules_ok
            || types.is_ok() != case.types_ok
            || image_ok != case.image_ok
            || digest(&modules)? != case.modules_sha256
            || digest(&types)? != case.types_sha256
            || digest(&image)? != case.image_sha256
        {
            return Err(format!(
                "Complete module/type/image compatibility changed for {}",
                case.label
            ));
        }
    }
    Ok(())
}
