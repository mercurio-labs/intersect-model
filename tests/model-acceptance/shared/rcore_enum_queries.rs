//! Independent enum identity/alias controls through actual native/WASM consumers.
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::import_enum_bundle,
    rcore_behavior::{
        bindings::{self, Target},
        checking,
    },
    resources::DeclarationId,
};
fn require(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
const ENUM: &str = "<eClassifiers x:type='e:EEnum' name='Mode'><eAnnotations source='urn:opaque'><details key='k' value='v'/></eAnnotations><eLiterals name='First' value='7'/><eLiterals name='Alias' value='7' literal=''/></eClassifiers>";
fn inputs(inline: bool, missing: bool) -> DefinitionBundle {
    let name = if missing { "Missing" } else { "Alias" };
    let body = format!(
        "import model \"urn:enum-consumer\" as m;fn first()->m.Mode=m.Mode.First;fn alias()->m.Mode=m.Mode.{name};"
    );
    let mut sources = if inline {
        vec![DefinitionSource::new(
            "memory:enum-consumer.rcore",
            SourceKind::Rcore,
            format!(
                "rcore 1;package p version \"1\";metamodel \"urn:enum-consumer\" prefix p {{enum Mode {{First = 7;Alias = 7;}}}}behavior {{{body}}}"
            ),
        )]
    } else {
        vec![
            DefinitionSource::new(
                "memory:enum-consumer.ecore",
                SourceKind::Ecore,
                format!(
                    "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:enum-consumer'>{ENUM}</e:EPackage>"
                ),
            ),
            DefinitionSource::new(
                "memory:enum-consumer.behavior",
                SourceKind::RcoreBehavior,
                format!("rcore 1;package p version \"1\";{body}"),
            ),
        ]
    };
    if !inline {
        let model = sources[0].id.clone();
        sources[1].dependencies.push(model);
    }
    DefinitionBundle {
        version: 1,
        sources,
        namespaces: vec![],
    }
}
pub fn verify() -> Result<usize, String> {
    for inline in [false, true] {
        let bundle = inputs(inline, false);
        let original = serde_json::to_vec(&bundle).map_err(|error| error.to_string())?;
        let enumeration = DeclarationId {
            source: bundle.sources[0].id.clone(),
            path: vec![0],
        };
        let imported =
            import_enum_bundle(&bundle, &enumeration).map_err(|error| error.to_string())?;
        let offset = if inline { 0 } else { 1 };
        let first = DeclarationId {
            source: enumeration.source.clone(),
            path: vec![0, offset],
        };
        let alias = DeclarationId {
            source: enumeration.source.clone(),
            path: vec![0, offset + 1],
        };
        require(imported.literals().len() == 2, "Incomplete enum literals")?;
        require(
            imported
                .by_value(7)
                .is_some_and(|literal| literal.declaration == first),
            "Numeric alias lost first declaration",
        )?;
        require(
            imported
                .by_name("Alias")
                .is_some_and(|literal| literal.declaration == alias),
            "Alias source identity changed",
        )?;
        if !inline {
            require(
                imported
                    .by_literal("")
                    .is_some_and(|literal| literal.declaration == alias),
                "Empty literal spelling was discarded",
            )?;
            require(
                imported.source_payload() == Some(ENUM),
                "Opaque source/annotations were changed",
            )?;
        } else {
            require(
                imported
                    .source_payload()
                    .is_some_and(|text| text.contains("enum Mode")),
                "Authored Rcore source was replaced",
            )?;
        }
        let entry = &bundle.sources.last().ok_or("Missing behavior source")?.id;
        let bound = bindings::bind(&bundle, entry).map_err(|errors| format!("{errors:?}"))?;
        let mut targets = bound
            .bindings
            .iter()
            .flat_map(|module| module.expressions.values())
            .filter_map(|target| {
                if let Target::EnumLiteral {
                    enumeration,
                    literal,
                } = target
                {
                    Some((enumeration.clone(), literal.clone()))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        targets.sort();
        require(
            targets == vec![(enumeration.clone(), first), (enumeration, alias)],
            "Binder lost exact enum/literal identities",
        )?;
        checking::check(&bundle, entry).map_err(|errors| format!("{errors:?}"))?;
        require(
            serde_json::to_vec(&bundle).map_err(|error| error.to_string())? == original,
            "Compilation changed retained sources",
        )?;
    }
    for inline in [false, true] {
        let bundle = inputs(inline, true);
        let entry = &bundle.sources.last().ok_or("Missing behavior source")?.id;
        let errors = bindings::bind(&bundle, entry)
            .err()
            .ok_or("Unknown enum literal was accepted")?;
        require(
            errors
                .iter()
                .any(|error| error.message == "Unknown enum literal Missing"),
            "Unknown enum literal diagnostic changed",
        )?;
    }
    let mut missing_dependency = inputs(false, false);
    missing_dependency.sources[1].dependencies.clear();
    let entry = &missing_dependency.sources[1].id;
    let errors = bindings::bind(&missing_dependency, entry)
        .err()
        .ok_or("Undeclared model dependency was accepted")?;
    require(
        errors.iter().any(|error| {
            error.message == "No exact Ecore dependency declares the imported namespace"
        }),
        "Exact model dependency guard changed",
    )?;
    Ok(3)
}
