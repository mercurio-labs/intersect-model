//! Native Model qualification helpers; no text compiler or language policy.
use crate::FixtureRequired;
use intersect_model::definitions::schema::Diagnostic;

pub fn compile_migrated_schema_fixture(
    ecore: &str,
) -> std::result::Result<intersect_model::definitions::schema::Schema, String> {
    use intersect_model::definitions::bundle::{DefinitionBundle, DefinitionSource, SourceKind};
    let source = DefinitionSource::new(
        "memory:fixture.rcore",
        SourceKind::Rcore,
        crate::migrate_model_fixture(ecore)?,
    );
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    intersect_model::definitions::rcore::compile_structure(&bundle, &source.id)
        .map(|structure| structure.schema)
        .map_err(|errors| {
            errors
                .into_iter()
                .map(|e| e.message)
                .collect::<Vec<_>>()
                .join("; ")
        })
}

pub fn canonical_behavior_fixture(
    behavior: &str,
    namespace: &str,
) -> std::result::Result<String, Vec<Diagnostic>> {
    use intersect_model::definitions::schema::{Expression, Semantics};
    if !behavior.trim_start().starts_with('{') {
        return Ok(behavior.to_owned());
    }
    let error = |message| {
        vec![Diagnostic::new(
            "rcore_behavior",
            0,
            behavior.len(),
            message,
        )]
    };
    let rules: Semantics = serde_json::from_str(behavior).map_err(|e| error(e.to_string()))?;
    if rules.version != 1 || rules.scope != "unique_names" {
        return Err(error("Unsupported fixture rule version/scope".into()));
    }
    let name = |value: &str| format!("`{}`", value.replace('`', "``"));
    let quote = |value: &str| serde_json::Value::String(value.into()).to_string();
    let mut text = "rcore 1; package fixtureProviders version \"1\";\n".to_owned();
    if !rules.derived.is_empty() || !rules.validations.is_empty() || !rules.initializers.is_empty()
    {
        text.push_str(&format!("import model {} as m;\n", quote(namespace)));
    }
    for rule in rules.derived {
        let target = format!("m.{}.{}", name(&rule.class), name(&rule.feature));
        text.push_str(&match rule.expression {
            Expression::Source { .. } => {
                return Err(error(
                    "Source getter contracts require exact authored source fixtures".into(),
                ));
            }
            Expression::External { .. } => {
                return Err(error(
                    "External binding contracts require authored source fixtures".into(),
                ));
            }
            Expression::Stored { feature, writable } => format!(
                "projection {target} from {}{};\n",
                name(&feature),
                if writable { " writable" } else { "" }
            ),
            Expression::Count { feature } => {
                format!("derive {target} using count({});\n", name(&feature))
            }
            Expression::NoIncoming { class, feature } => format!(
                "derive {target} using noIncoming(m.{}.{});\n",
                name(&class),
                name(&feature)
            ),
        });
    }
    for (index, rule) in rules.validations.iter().enumerate() {
        let owner = format!("m.{}", name(&rule.class));
        text.push_str(&format!("validate {owner}.rule{index}(self) {{ require size(incoming::<{owner}>(self.{}, {})) <= {} code \"rule{index}\" message {}; }}\n", name(&rule.feature), quote(&rule.feature), rule.max_incoming, quote(&rule.message)));
    }
    for rule in rules.initializers {
        text.push_str(&format!(
            "initialize m.{} using {} version {} {{\n",
            name(&rule.class),
            quote(&rule.provider.name),
            quote(&rule.provider.version)
        ));
        if let Some(values) = &rule.values
            && values.keys().any(|key| !rule.members.contains(key))
        {
            return Err(error(
                "Initializer values must match declared members".into(),
            ));
        }
        for member in &rule.members {
            let assignment = rule
                .values
                .as_ref()
                .and_then(|values| values.get(member))
                .map(|value| format!(" = {}", constant_fixture(value)))
                .unwrap_or_default();
            text.push_str(&format!("{}{};\n", name(member), assignment));
        }
        text.push_str("}\n");
    }
    for rule in rules.rule_conversions {
        text.push_str(&format!(
            "convert {} rule {} from {} sha256 {} span {} {} to {} using {} version {} {{\n",
            quote(&rule.context),
            quote(&rule.rule),
            quote(&rule.declaration.source.uri),
            quote(&rule.declaration.source.sha256),
            rule.declaration.start,
            rule.declaration.end,
            quote(&rule.datatype),
            quote(&rule.provider.name),
            quote(&rule.provider.version)
        ));
        for (keyword, value) in rule.keywords {
            text.push_str(&format!("{} = {value};\n", quote(&keyword)));
        }
        text.push_str("}\n");
    }
    Ok(text)
}

fn constant_fixture(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "none".into(),
        serde_json::Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(constant_fixture)
                .collect::<Vec<_>>()
                .join(",")
        ),
        serde_json::Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!(
                    "`{}`: {}",
                    key.replace('`', "``"),
                    constant_fixture(value)
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
        _ => value.to_string(),
    }
}

pub fn migrate_source_fixture(
    source: intersect_model::definitions::bundle::DefinitionSource,
) -> (
    intersect_model::definitions::bundle::DefinitionSource,
    std::collections::BTreeMap<Vec<usize>, intersect_model::definitions::resources::DeclarationId>,
) {
    let target = format!("{}.rcore", source.id.uri);
    let report = intersect_model::migration::migrate_ecore(source, &target).required();
    let converted = report
        .converted
        .unwrap_or_else(|| panic!("{:?}", report.issues));
    let declarations = report
        .correspondence
        .into_iter()
        .map(|row| (row.original.path, row.converted))
        .collect();
    (converted, declarations)
}

pub fn named_declaration_fixture(
    source: &intersect_model::definitions::bundle::DefinitionSource,
    name: &str,
) -> intersect_model::definitions::resources::DeclarationId {
    let bundle = intersect_model::definitions::bundle::DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    intersect_model::definitions::resources::ResourceSet::new_diagnostic(&bundle)
        .required()
        .resolve(&source.id, &format!("#//{name}"))
        .required()
}

pub fn migrate_resources_fixture(
    bundle: intersect_model::definitions::bundle::DefinitionBundle,
) -> intersect_model::migration::BundleMigrationReport {
    let targets = bundle
        .sources
        .iter()
        .filter(|source| source.kind == intersect_model::definitions::bundle::SourceKind::Ecore)
        .map(|source| {
            let target = if bundle
                .sources
                .iter()
                .filter(|other| other.id.uri == source.id.uri)
                .count()
                > 1
            {
                format!("{}.{}.rcore", source.id.uri, source.id.sha256)
            } else {
                format!("{}.rcore", source.id.uri)
            };
            (source.id.clone(), target)
        })
        .collect();
    intersect_model::migration::migrate_ecore_bundle(bundle, &targets).required()
}

pub fn original_schema_fixture(ecore: &str) -> intersect_model::definitions::schema::Schema {
    let input = intersect_model::definitions::bundle::DefinitionSource::new(
        "memory:regression",
        intersect_model::definitions::bundle::SourceKind::Ecore,
        ecore,
    );
    let values: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/rcore/ecore-schema-regression.json"
    ))
    .required();
    let row = values
        .as_object()
        .required()
        .values()
        .find(|row| row["input_sha256"] == input.id.sha256)
        .required_with("No captured original schema for these exact source bytes");
    // Adapt the frozen observation's representation, preserving all expected values.
    // The original capture remains byte-for-byte unchanged.
    let mut schema = row["schema"].clone();
    for class in schema["classes"].as_object_mut().required().values_mut() {
        let class = class.as_object_mut().required();
        let mut parents = Vec::new();
        if let Some(parent) = class.remove("parent").filter(|value| !value.is_null()) {
            parents.push(parent);
        }
        if let Some(serde_json::Value::Array(extra)) = class.remove("additional_parents") {
            parents.extend(extra);
        }
        class.insert("parents".into(), parents.into());
    }
    serde_json::from_value(schema).required()
}

pub fn converted_source_fixture(
    report: &intersect_model::migration::BundleMigrationReport,
    original: &intersect_model::definitions::bundle::SourceId,
) -> intersect_model::definitions::bundle::DefinitionSource {
    report
        .reports
        .iter()
        .find(|row| row.original.id == *original)
        .required()
        .converted
        .clone()
        .required()
}

pub fn migrate_provider_fixture(
    original: intersect_model::definitions::bundle::DefinitionSource,
    contracts: &[intersect_model::definitions::conversion::ConversionContract],
) -> (
    intersect_model::definitions::bundle::DefinitionBundle,
    Vec<intersect_model::definitions::conversion::ConversionContract>,
) {
    use intersect_model::definitions::{bundle::DefinitionBundle, resources::DeclarationId};
    use serde_json::Value;
    let report = intersect_model::migration::migrate_ecore(original.clone(), "memory:model.rcore")
        .required();
    let source = report
        .converted
        .unwrap_or_else(|| panic!("{:?}", report.issues));
    let ids: std::collections::BTreeMap<_, _> = report
        .correspondence
        .into_iter()
        .map(|row| (row.original, row.converted))
        .collect();
    fn rewrite(
        value: &mut Value,
        original: &Value,
        source: &Value,
        old_id: &Value,
        new_id: &Value,
        ids: &std::collections::BTreeMap<DeclarationId, DeclarationId>,
    ) {
        if value == original {
            *value = source.clone();
            return;
        }
        if value == old_id {
            *value = new_id.clone();
            return;
        }
        if let Ok(id) = serde_json::from_value::<DeclarationId>(value.clone()) {
            if let Some(mapped) = ids.get(&id) {
                *value = serde_json::to_value(mapped).required();
                return;
            }
            assert_ne!(
                serde_json::to_value(&id.source).required(),
                *old_id,
                "Missing exact declaration correspondence: {id:?}"
            );
        }
        match value {
            Value::Array(values) => {
                for child in values {
                    rewrite(child, original, source, old_id, new_id, ids);
                }
            }
            Value::Object(values) => {
                for child in values.values_mut() {
                    rewrite(child, original, source, old_id, new_id, ids);
                }
            }
            _ => {}
        }
    }
    let mut values = serde_json::to_value(contracts).required();
    rewrite(
        &mut values,
        &serde_json::to_value(&original).required(),
        &serde_json::to_value(&source).required(),
        &serde_json::to_value(&original.id).required(),
        &serde_json::to_value(&source.id).required(),
        &ids,
    );
    let mut contracts: Vec<intersect_model::definitions::conversion::ConversionContract> =
        serde_json::from_value(values).required();
    for contract in &mut contracts {
        for member in &mut contract.members {
            if let intersect_model::definitions::conversion::MemberType::Reference {
                context, ..
            } = &mut member.value
            {
                let schema = intersect_model::conversion::with_catalog(&context.bundle, &context.conversions, |registry| {
                    intersect_model::definitions::compiler::compile_structure_bundle_with_conversions(
                        &context.bundle, &context.entry, &context.namespace, registry)
                        .map_err(|e| e.to_string())
                }).required();
                context.model["schema_revision"] = schema.revision.clone().into();
                for element in context.model["elements"].as_array_mut().required() {
                    element["type_identity"] =
                        schema.identity(element["class"].as_str().required()).into();
                }
            }
        }
    }
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        contracts,
    )
}
