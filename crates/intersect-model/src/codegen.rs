//! Handwritten deterministic Rust generator. Generated bindings share core services.
//! File I/O belongs to the CLI/build script. This library also builds for WASM.
use intersect_model::definitions::{
    dynamic::DynamicSchema,
    schema::{Feature, Schema},
};
use std::collections::BTreeSet;

pub const GENERATOR_VERSION: &str = "7";
#[path = "codegen_semantic_bindings.rs"]
pub mod semantic_bindings;
pub type GenerationResult<T> = Result<T, String>;

/// Generate bindings from the canonical Rcore frontend, retaining authored source.
pub fn generate_rcore(source: &str) -> GenerationResult<String> {
    let source = intersect_model::definitions::bundle::DefinitionSource::new(
        "memory:model.rcore",
        intersect_model::definitions::bundle::SourceKind::Rcore,
        source,
    );
    let bundle = intersect_model::definitions::bundle::DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    let structure = intersect_model::definitions::rcore::compile_structure(&bundle, &source.id)
        .map_err(|e| format!("{e:?}"))?;
    generate_inner(&structure.schema, Some(&source.text))
}

/// Generate from validated IR without changing its namespace or revision.
/// No source text is invented; RCORE_SOURCE is None for this entry point.
pub fn generate(schema: &Schema) -> GenerationResult<String> {
    generate_inner(schema, None)
}

fn literal(value: &str) -> String {
    format!("{value:?}")
}

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while", "abstract", "become", "box", "do", "final", "gen", "macro",
    "override", "priv", "try", "typeof", "unsized", "virtual", "yield",
];

fn identifier(name: &str) -> GenerationResult<()> {
    if name.is_empty()
        || name == "_"
        || name.starts_with("__")
        || !name
            .bytes()
            .enumerate()
            .all(|(i, c)| c.is_ascii_alphabetic() || c == b'_' || (i > 0 && c.is_ascii_digit()))
    {
        return Err(format!("Unsupported Rust identifier {name:?}"));
    }
    Ok(())
}

fn snake(name: &str) -> GenerationResult<String> {
    identifier(name)?;
    let chars: Vec<_> = name.chars().collect();
    let mut result = String::new();
    for (i, c) in chars.iter().copied().enumerate() {
        if c.is_ascii_uppercase()
            && i > 0
            && chars[i - 1] != '_'
            && (chars[i - 1].is_ascii_lowercase()
                || chars[i - 1].is_ascii_digit()
                || chars.get(i + 1).is_some_and(char::is_ascii_lowercase))
        {
            result.push('_');
        }
        result.push(c.to_ascii_lowercase());
    }
    if KEYWORDS.contains(&result.as_str()) {
        result.push('_');
    }
    Ok(result)
}

fn names(schema: &Schema) -> GenerationResult<()> {
    let reserved = [
        "String",
        "Vec",
        "Option",
        "Result",
        "Some",
        "None",
        "Ok",
        "Err",
        "schema",
        "dynamic_schema",
        "SCHEMA_URI",
        "SCHEMA_REVISION",
        "SCHEMA_JSON",
        "RCORE_SOURCE",
        "GENERATOR_VERSION",
        "std",
        "serde_json",
        "intersect_model::core",
        "intersect_model::definitions",
        "i32",
        "i64",
        "bool",
        "str",
    ];
    for name in schema.classes.keys() {
        identifier(name)?;
        if KEYWORDS.contains(&name.as_str()) || reserved.contains(&name.as_str()) {
            return Err(format!("Reserved Rust class name {name:?}"));
        }
        let mut methods: BTreeSet<String> = [
            "bind",
            "create",
            "id",
            "check",
            "query_binding",
            "delete",
            "reparent",
            "clone",
            "clone_from",
            "eq",
            "ne",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let mut ancestor = schema.classes[name].parents.first().map(String::as_str);
        while let Some(parent) = ancestor {
            let method = format!("as_{}", snake(parent)?);
            if !methods.insert(method.clone()) {
                return Err(format!(
                    "Rust ancestor conversion collision in {name}: {method}"
                ));
            }
            ancestor = schema.classes[parent].parents.first().map(String::as_str);
        }
        for feature in schema.features(name)?.values() {
            let field = snake(&feature.name)?;
            let mut generated = vec![
                field.clone(),
                format!("field_{field}"),
                format!("is_set_{field}"),
            ];
            if feature.derived && feature.changeable {
                generated.push(format!("set_{field}"));
                generated.push(format!("unset_{field}"));
            }
            if !feature.derived {
                generated.push(format!("set_{field}"));
                generated.push(format!("unset_{field}"));
                if feature.upper != 1 {
                    generated.push(format!("add_{field}"));
                    generated.push(format!("remove_{field}"));
                    if feature.ordered {
                        for operation in ["insert", "replace", "remove_at", "move"] {
                            generated.push(format!("{operation}_{field}"));
                        }
                    }
                }
                if feature.upper != 1 || feature.lower == 0 {
                    generated.push(format!("clear_{field}"));
                }
            }
            for method in generated {
                if !methods.insert(method.clone()) {
                    return Err(format!("Rust method collision in {name}: {method}"));
                }
            }
        }
    }
    Ok(())
}

fn base_type(feature: &Feature, query: bool) -> GenerationResult<String> {
    if feature.reference {
        if query {
            Ok(format!("__ReferenceId<{}>", feature.target))
        } else {
            Ok(feature.target.clone())
        }
    } else {
        Ok(match feature.target.as_str() {
            "EString" => "String".into(),
            "EInt" if query => "i64".into(),
            "EInt" => "i32".into(),
            "EBoolean" => "bool".into(),
            other => return Err(format!("Unsupported Rust scalar type {other}")),
        })
    }
}

fn value_type(feature: &Feature, query: bool) -> GenerationResult<String> {
    let base = base_type(feature, query)?;
    Ok(if feature.upper != 1 {
        format!("Vec<{base}>")
    } else if feature.lower == 0 {
        format!("Option<{base}>")
    } else {
        base
    })
}

fn input_base(feature: &Feature) -> GenerationResult<String> {
    if feature.reference {
        Ok(format!("&{}", feature.target))
    } else if feature.target == "EString" {
        Ok("&str".into())
    } else {
        base_type(feature, false)
    }
}

fn input_type(feature: &Feature) -> GenerationResult<String> {
    Ok(if feature.upper != 1 {
        format!("&[{}]", base_type(feature, false)?)
    } else if feature.lower == 0 {
        format!("Option<{}>", input_base(feature)?)
    } else {
        input_base(feature)?
    })
}

fn generate_inner(schema: &Schema, source: Option<&str>) -> GenerationResult<String> {
    // Import capability is broader than this generator's executable binding profile.
    // Retaining metadata in SCHEMA_JSON must not imply emitted APIs for it.
    if !schema.enumerations.is_empty()
        || !schema.datatypes.is_empty()
        || schema.generic_signatures.is_some()
        || !schema.instantiations.is_empty()
    {
        return Err(
            "Rust bindings do not yet support enums, declared datatypes or generic schemas".into(),
        );
    }
    for class in schema.classes.values() {
        if class.interface_type || class.parents.len() > 1 || !class.operations.is_empty() {
            return Err(format!(
                "Rust bindings do not yet support interfaces, multiple inheritance or operations: {}",
                class.name
            ));
        }
    }
    DynamicSchema::from_schema(schema.clone())?;
    names(schema)?;
    let json = serde_json::to_string(schema).map_err(|e| e.to_string())?;
    let mut out = String::from(
        "// GENERATED by intersect-codegen. Do not edit; keep handwritten providers separate.\n",
    );
    out += &format!("pub const GENERATOR_VERSION: &str = {GENERATOR_VERSION:?};\n");
    out += &format!("pub const SCHEMA_URI: &str = {};\n", literal(&schema.uri));
    out += &format!(
        "pub const SCHEMA_REVISION: &str = {};\n",
        literal(&schema.revision)
    );
    out += &format!("pub const SCHEMA_JSON: &str = {};\n", literal(&json));
    let source = source.map_or("None".into(), |s| format!("Some({})", literal(s)));
    out += &format!("pub const RCORE_SOURCE: Option<&str> = {source};\n");
    out.push_str(r#"
#[allow(unused_imports)]
use intersect_model::core::{
    api::ModelEdit as __ModelEdit,
    storage::{ModelRead as __ModelRead, StorageResult as __Result},
    query::{Binding as __Binding, Entity as __Entity, Field as __Field,
        ReferenceId as __ReferenceId, DerivedValues as __DerivedValues, QueryBudget as __QueryBudget},
};
use intersect_model::definitions::schema::Schema as __Schema;

pub fn schema() -> __Result<__Schema> {
    let schema: __Schema = serde_json::from_str(SCHEMA_JSON).map_err(|e| e.to_string())?;
    schema.validate()?;
    Ok(schema)
}
pub fn dynamic_schema() -> __Result<intersect_model::definitions::dynamic::DynamicSchema> {
    intersect_model::definitions::dynamic::DynamicSchema::from_schema(schema()?)
}
fn __check_model(model: &impl __ModelRead) -> __Result<()> {
    if model.revision()? != SCHEMA_REVISION {
        return Err("Generated binding schema revision mismatch".into());
    }
    Ok(())
}
fn __check_schema(schema: &__Schema) -> __Result<()> {
    if schema.uri != SCHEMA_URI || schema.revision != SCHEMA_REVISION || schema != &self::schema()? {
        return Err("Generated binding schema mismatch".into());
    }
    Ok(())
}
"#);
    for class in schema.classes.values() {
        let name = &class.name;
        let class_literal = literal(name);
        out += &format!(
            r#"
/// Typed external identity, resolved against the supplied model on every access.
#[allow(non_camel_case_types)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct {name} {{ id: String }}
impl __Entity for {name} {{
    const CLASS: &'static str = {class_literal};
    const SCHEMA_URI: &'static str = SCHEMA_URI;
}}
impl {name} {{
    pub fn id(&self) -> &str {{ &self.id }}
    pub fn bind(model: &impl __ModelRead, id: &str) -> __Result<Self> {{
        let object = Self {{ id: id.into() }};
        object.check(model)?;
        Ok(object)
    }}
    fn check(&self, model: &impl __ModelRead) -> __Result<()> {{
        __check_model(model)?;
        let actual = model.class(&self.id)?.ok_or_else(|| format!("Missing object {{}}", self.id))?;
"#
        );
        let conforming = schema
            .classes
            .keys()
            .filter(|c| schema.conforms(c, name))
            .map(|c| literal(c))
            .collect::<Vec<_>>()
            .join(" | ");
        out += &format!(
            r#"        if !matches!(actual.as_str(), {conforming}) {{
            return Err(format!("Object {{}} has class {{actual}}, expected {{}}", self.id, {class_literal}));
        }}
        Ok(())
    }}
    /// Delete this external identity (or its subtree) through shared core services.
    pub fn delete(&self, model: &mut impl __ModelEdit,
        selection: intersect_model::core::storage::DeleteSelection,
        incoming: intersect_model::core::storage::IncomingReferences,
        conversions: Option<&intersect_model::definitions::conversion::ConversionRegistry<'_>>,
        budget: &mut __QueryBudget) -> __Result<Vec<String>> {{
        self.check(model)?;
        model.delete_objects(intersect_model::core::storage::DeleteRequest {{ roots: &[&self.id], selection, incoming }}, conversions, budget)
    }}
    /// Move or detach this external identity without changing source provenance.
    pub fn reparent(&self, model: &mut impl __ModelEdit,
        destination: Option<intersect_model::core::storage::ContainmentDestination<'_>>,
        conversions: Option<&intersect_model::definitions::conversion::ConversionRegistry<'_>>,
        budget: &mut __QueryBudget) -> __Result<bool> {{
        self.check(model)?;
        model.reparent(intersect_model::core::storage::ReparentRequest {{ object: &self.id, destination }}, conversions, budget)
    }}
    pub fn query_binding(schema: &__Schema) -> __Result<__Binding<'_, Self>> {{
        __check_schema(schema)?;
        __Binding::new(schema)
    }}
"#
        );
        if !class.abstract_type {
            out += &format!(
                r#"    /// Create a draft; fill required features before validation or commit.
    pub fn create(model: &mut impl __ModelEdit, id: &str, resource: &str) -> __Result<Self> {{
        __check_model(model)?;
        __check_schema(model.schema())?;
        model.create(id, {class_literal}, resource)?;
        Ok(Self {{ id: id.into() }})
    }}
"#
            );
        }
        let mut ancestor = class.parents.first().map(String::as_str);
        while let Some(parent) = ancestor {
            out += &format!(
                "    pub fn as_{}(&self) -> {parent} {{ {parent} {{ id: self.id.clone() }} }}\n",
                snake(parent)?
            );
            ancestor = schema.classes[parent].parents.first().map(String::as_str);
        }
        for feature in schema.features(name)?.values() {
            emit_feature(&mut out, feature)?;
        }
        out.push_str("}\n");
    }
    Ok(out)
}

fn emit_feature(out: &mut String, feature: &Feature) -> GenerationResult<()> {
    let method = snake(&feature.name)?;
    let field = literal(&feature.name);
    let output = value_type(feature, false)?;
    let query_type = value_type(feature, true)?;
    *out += &format!(
        r#"    pub fn field_{method}(schema: &__Schema) -> __Result<__Field<Self, {query_type}>> {{
        Self::query_binding(schema)?.field({field})
    }}
"#
    );
    let provider = if feature.derived {
        ", provider: &dyn __DerivedValues, budget: &mut __QueryBudget"
    } else {
        ""
    };
    *out += &format!(
        "    pub fn {method}(&self, model: &impl __ModelRead{provider}) -> __Result<{output}> {{\n        self.check(model)?;\n"
    );
    if feature.derived {
        *out += &format!(
            r#"        let class = model.class(&self.id)?.ok_or("Missing object")?;
        if !provider.supports(&class, {field}) {{
            return Err(format!("Missing derived provider for {{class}}.{{}}", {field}));
        }}
        let value = provider.read(model, &self.id, {field}, budget)?;
"#
        );
    } else {
        *out += &format!(
            "        let value = model.value(&self.id, {field})?.unwrap_or(serde_json::Value::Null);\n"
        );
    }
    if feature.reference {
        let target = &feature.target;
        if feature.upper != 1 {
            out.push_str("        let ids: Vec<String> = serde_json::from_value(value).map_err(|e| e.to_string())?;\n");
            *out += &format!("        ids.iter().map(|id| {target}::bind(model, id)).collect()\n");
        } else if feature.lower == 0 {
            out.push_str("        let id: Option<String> = serde_json::from_value(value).map_err(|e| e.to_string())?;\n");
            *out += &format!("        id.map(|id| {target}::bind(model, &id)).transpose()\n");
        } else {
            out.push_str("        let id: String = serde_json::from_value(value).map_err(|e| e.to_string())?;\n");
            *out += &format!("        {target}::bind(model, &id)\n");
        }
    } else {
        *out += &format!(
            "        serde_json::from_value(value).map_err(|e| format!(\"Invalid {{}}: {{e}}\", {field}))\n"
        );
    }
    out.push_str("    }\n");
    *out += &format!(
        "    pub fn is_set_{method}(&self, model: &impl __ModelRead{provider}) -> __Result<bool> {{\n        self.check(model)?;\n"
    );
    if feature.derived {
        *out += &format!(
            "        intersect_model::core::storage::is_set_with_derived(model, &schema()?, &self.id, {field}, provider, budget)\n    }}\n"
        );
        if feature.changeable {
            let input = input_type(feature)?;
            *out += &format!(
                "    pub fn set_{method}(&self, model: &mut impl __ModelEdit, value: {input}, context: intersect_model::core::storage::SettingContext<'_>, budget: &mut __QueryBudget) -> __Result<()> {{\n        self.check(model)?;\n"
            );
            let encoded = encode_input(out, feature);
            *out += &format!(
                "        model.edit_computed(&self.id, {field}, intersect_model::core::storage::ComputedEdit::Set({encoded}.map_err(|e| e.to_string())?), context, budget)\n    }}\n"
            );
            *out += &format!(
                "    pub fn unset_{method}(&self, model: &mut impl __ModelEdit, context: intersect_model::core::storage::SettingContext<'_>, budget: &mut __QueryBudget) -> __Result<()> {{\n        self.check(model)?;\n        model.edit_computed(&self.id, {field}, intersect_model::core::storage::ComputedEdit::Unset, context, budget)\n    }}\n"
            );
        }
        return Ok(());
    }
    *out += &format!(
        "        intersect_model::core::storage::is_set(model, &schema()?, &self.id, {field})\n    }}\n"
    );
    *out += &format!(
        "    pub fn unset_{method}(&self, model: &mut impl __ModelEdit) -> __Result<()> {{\n        self.check(model)?;\n        model.unset(&self.id, {field})\n    }}\n"
    );
    let input = input_type(feature)?;
    *out += &format!(
        "    pub fn set_{method}(&self, model: &mut impl __ModelEdit, value: {input}) -> __Result<()> {{\n        self.check(model)?;\n"
    );
    let encoded = encode_input(out, feature);
    *out += &format!(
        "        model.set(&self.id, {field}, {encoded}.map_err(|e| e.to_string())?)\n    }}\n"
    );
    if feature.upper != 1 {
        let input = input_base(feature)?;
        *out += &format!(
            "    pub fn add_{method}(&self, model: &mut impl __ModelEdit, value: {input}) -> __Result<()> {{\n        self.check(model)?;\n"
        );
        let encoded = if feature.reference {
            out.push_str("        value.check(model)?;\n");
            "serde_json::to_value(value.id())"
        } else {
            "serde_json::to_value(value)"
        };
        *out += &format!(
            r#"        model.edit_collection(&self.id, {field}, intersect_model::core::storage::CollectionEdit::Add({encoded}.map_err(|e| e.to_string())?)).map(|_| ())
    }}
"#
        );
    }
    if feature.upper != 1 {
        let input = input_base(feature)?;
        for operation in ["remove", "insert", "replace"] {
            if operation != "remove" && !feature.ordered {
                continue;
            }
            let index = if operation == "remove" {
                ""
            } else {
                "index: usize, "
            };
            *out += &format!(
                "    pub fn {operation}_{method}(&self, model: &mut impl __ModelEdit, {index}value: {input}) -> __Result<bool> {{\n        self.check(model)?;\n"
            );
            let encoded = if feature.reference {
                out.push_str("        value.check(model)?;\n");
                "serde_json::to_value(value.id())"
            } else {
                "serde_json::to_value(value)"
            };
            *out += &format!("        let value = {encoded}.map_err(|e| e.to_string())?;\n");
            let edit = match operation {
                "remove" => "Remove(value)",
                "insert" => "Insert { index, value }",
                _ => "Replace { index, value }",
            };
            *out += &format!(
                "        model.edit_collection(&self.id, {field}, intersect_model::core::storage::CollectionEdit::{edit})\n    }}\n"
            );
        }
        if feature.ordered {
            for (operation, parameters, edit) in [
                ("remove_at", "index: usize", "RemoveAt(index)"),
                ("move", "from: usize, to: usize", "Move { from, to }"),
            ] {
                *out += &format!(
                    "    pub fn {operation}_{method}(&self, model: &mut impl __ModelEdit, {parameters}) -> __Result<bool> {{\n        self.check(model)?;\n        model.edit_collection(&self.id, {field}, intersect_model::core::storage::CollectionEdit::{edit})\n    }}\n"
                );
            }
        }
    }
    if feature.upper != 1 || feature.lower == 0 {
        let operation = if feature.upper != 1 {
            format!(
                "model.edit_collection(&self.id, {field}, intersect_model::core::storage::CollectionEdit::Clear).map(|_| ())"
            )
        } else {
            format!("model.set(&self.id, {field}, serde_json::Value::Null)")
        };
        *out += &format!(
            "    pub fn clear_{method}(&self, model: &mut impl __ModelEdit) -> __Result<()> {{\n        self.check(model)?;\n        {operation}\n    }}\n"
        );
    }
    Ok(())
}

fn encode_input(out: &mut String, feature: &Feature) -> &'static str {
    if feature.reference {
        if feature.upper != 1 {
            out.push_str("        for object in value { object.check(model)?; }\n");
            "serde_json::to_value(value.iter().map(|object| object.id()).collect::<Vec<_>>())"
        } else if feature.lower == 0 {
            out.push_str("        if let Some(object) = value { object.check(model)?; }\n");
            "serde_json::to_value(value.map(|object| object.id()))"
        } else {
            out.push_str("        value.check(model)?;\n");
            "serde_json::to_value(value.id())"
        }
    } else {
        "serde_json::to_value(value)"
    }
}
