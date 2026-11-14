//! Host-only inspection example; parsed source is not an executable schema.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    ecore::{Document, ParseLimits},
    generic_resolution::Resolver,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("Usage: inspect_ecore_document PATH [SOURCE_BYTES] [--resolve|--schema]")?;
    let limits = ParseLimits {
        source_bytes: args
            .next()
            .map(|value| value.parse::<usize>())
            .transpose()?
            .unwrap_or(ParseLimits::default().source_bytes),
    };
    let mode = args.next();
    if !matches!(mode.as_deref(), None | Some("--resolve" | "--schema")) {
        return Err("Unexpected argument".into());
    }
    if args.next().is_some() {
        return Err("Unexpected argument".into());
    }
    let source = DefinitionSource::new(
        path.clone(),
        SourceKind::Ecore,
        std::fs::read_to_string(&path)?,
    );
    let document = Document::parse_with_limits(source, limits)?;
    let mut output = serde_json::json!({"source":document.source.id,"bytes":document.source.text.len(),"root":document.root.lexical_name,"classifiers":document.root.children.iter().filter(|node|node.name.local=="eClassifiers").count(),"semantically_verified":false});
    if mode.is_some() {
        let namespace = document
            .root
            .attributes
            .iter()
            .find(|attribute| attribute.name.namespace.is_none() && attribute.name.local == "nsURI")
            .map(|attribute| attribute.value.clone())
            .ok_or("Missing package namespace")?;
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![document.source],
            namespaces: vec![],
        };
        let inventory = Resolver::new_with_limits(&bundle, limits)?.resolve_all()?;
        if mode.as_deref() == Some("--schema") {
            let original = bundle.sources[0].clone();
            let report = intersect_model::migration::migrate_ecore_with_limits(
                original.clone(),
                &format!("{}.rcore", original.id.uri),
                limits,
            )?;
            let converted = report
                .converted
                .ok_or_else(|| format!("Migration requires manual work: {:?}", report.issues))?;
            let canonical = DefinitionBundle {
                version: 1,
                sources: vec![converted.clone()],
                namespaces: vec![],
            };
            let schema =
                intersect_model::definitions::compiler::compile_structure_bundle_with_limits(
                    &canonical,
                    &converted.id,
                    &namespace,
                    limits,
                )?;
            output["migration"] = serde_json::json!({"original": original.id, "converted": converted.id, "correspondence": report.correspondence.len()});
            output["compiled_structure"] = serde_json::json!({"classes":schema.classes.len(),"revision":schema.revision,"domain_semantics_verified":false});
        }
        output["resolved_inventory"] = serde_json::json!({"sources":inventory.sources,"generic_types":inventory.types.len(),"parameters":inventory.parameters.len(),"raw_signatures":inventory.raw_signatures.len(),"retained_payloads":inventory.retained_payloads.len(),"executable_schema":false});
    }
    println!("{output}");
    Ok(())
}
