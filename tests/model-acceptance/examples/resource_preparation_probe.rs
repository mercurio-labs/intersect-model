//! Verification-only consumer: extraction/indexing is not Ecore semantic support.
use intersect_model::definitions::{
    bundle::DefinitionBundle,
    conversion::{InvocationContext, InvocationObserver, ProviderId},
    ecore::{ObservedImportError, ParseLimits},
    resources::{DeclarationId, ResourceSet, ResourceSetPreparation},
};
use std::rc::Rc;
struct Cancel;
impl InvocationObserver for Cancel {
    fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
        Err("Unexpected provider".into())
    }
    fn checkpoint(&self) -> Result<(), String> {
        Err("resource probe cancelled".into())
    }
}
fn prepare(bundle: &DefinitionBundle) -> Result<(ResourceSet, usize), String> {
    let context = InvocationContext::default();
    let mut preparation = ResourceSetPreparation::begin(bundle, ParseLimits::default(), &context)
        .map_err(|e| e.to_string())?;
    let mut advances = 0;
    loop {
        advances += 1;
        if preparation.advance(&context).map_err(|e| e.to_string())? {
            break;
        }
    }
    Ok((preparation.finish().map_err(|e| e.to_string())?, advances))
}
fn verify() -> Result<u32, String> {
    let bundle: DefinitionBundle = serde_json::from_str(include_str!(
        "../../../fixtures/ecore-generics/resource-preparation/bundle.json"
    ))
    .map_err(|e| e.to_string())?;
    let (resources, advances) = prepare(&bundle)?;
    let legacy = ResourceSet::new(&bundle)?;
    if advances <= 80 || format!("{resources:?}") != format!("{legacy:?}") {
        return Err("Complete resource metadata changed".into());
    }
    let a = &bundle.sources[1].id;
    let b = &bundle.sources[0].id;
    for (from, href, source, path) in [
        (a, "urn:a:inner#//Node", a, vec![0, 0, 0]),
        (b, "a.ecore#innerNode", a, vec![0, 0, 0]),
        (a, "other.ecore#//Node", b, vec![0]),
        (a, "#foreign", a, vec![0, 2]),
    ] {
        if resources.resolve(from, href)?
            != (DeclarationId {
                source: source.clone(),
                path,
            })
        {
            return Err("Exact source/fragment identity changed".into());
        }
    }
    if resources.document(a)?.source != bundle.sources[1] || resources.resolve(a, "#//Node").is_ok()
    {
        return Err("Multi-root ambiguity or source retention changed".into());
    }
    let context = InvocationContext::default();
    for boundary in 0..=advances {
        let mut preparation =
            ResourceSetPreparation::begin(&bundle, ParseLimits::default(), &context)
                .map_err(|e| e.to_string())?;
        for _ in 0..boundary {
            preparation.advance(&context).map_err(|e| e.to_string())?;
        }
        let expected = ObservedImportError::Observation("resource probe cancelled".into());
        if preparation.advance(&InvocationContext::new(Rc::new(Cancel))) != Err(expected.clone())
            || preparation.advance(&context) != Err(expected.clone())
            || !matches!(preparation.finish(),Err(actual) if actual==expected)
        {
            return Err("Terminal resource cancellation changed".into());
        }
    }
    let mut bad = bundle.clone();
    bad.namespaces[0].namespace = "urn:absent".into();
    let expected = ResourceSet::new_diagnostic(&bad)
        .err()
        .ok_or("Unknown namespace accepted")?;
    let mut preparation = ResourceSetPreparation::begin(&bad, ParseLimits::default(), &context)
        .map_err(|e| e.to_string())?;
    loop {
        match preparation.advance(&context) {
            Ok(false) => {}
            Ok(true) => return Err("Unknown namespace published".into()),
            Err(ObservedImportError::Import(actual)) if actual == expected => break,
            Err(other) => return Err(other.to_string()),
        }
    }
    Ok(4)
}
#[unsafe(no_mangle)]
pub extern "C" fn verify_resource_preparation() -> u32 {
    verify().unwrap_or(0)
}
fn main() -> Result<(), String> {
    let groups = verify()?;
    println!("{groups} retained resource groups passed");
    Ok(())
}
