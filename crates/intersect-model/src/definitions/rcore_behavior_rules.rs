//! Bounded typed provider lowering into the shared model-rule representation.
//! The package compiler type-checks exact sources before calling this module.
use crate::definitions::{
    rcore_behavior::ast::{
        Declaration, DerivedContract, Document, Expr, ExprId, Literal, Path, Type,
    },
    schema::{
        ClassInitialization, Derivation, DerivedArgument, Diagnostic, Expression, Schema,
        Semantics, Validation,
    },
};

pub(crate) fn lower(doc: &Document, schema: &Schema) -> Result<Semantics, Vec<Diagnostic>> {
    lower_scoped(doc, schema, false, false)
}
pub(crate) fn lower_with_imported_models(
    doc: &Document,
    schema: &Schema,
) -> Result<Semantics, Vec<Diagnostic>> {
    lower_scoped(doc, schema, true, false)
}
pub(crate) fn lower_getter_selectors(
    doc: &Document,
    schema: &Schema,
) -> Result<Semantics, Vec<Diagnostic>> {
    lower_scoped(doc, schema, true, true)
}
fn lower_scoped(
    doc: &Document,
    schema: &Schema,
    imported_models: bool,
    derive_only: bool,
) -> Result<Semantics, Vec<Diagnostic>> {
    let mut rules = Semantics {
        scope_bindings: vec![],
        rule_conversions: vec![],
        version: 1,
        scope: "unique_names".into(),
        derived: vec![],
        initializers: vec![],
        validations: vec![],
    };
    let mut aliases = std::collections::BTreeMap::new();
    for declaration in &doc.declarations {
        if let Declaration::ModelImport { namespace, alias } = &declaration.value {
            if namespace.value != schema.uri
                && (!imported_models
                    || !schema
                        .declarations
                        .values()
                        .any(|declaration| declaration.namespace == namespace.value))
            {
                return Err(vec![Diagnostic::new(
                    "rcore_behavior",
                    declaration.span.start,
                    declaration.span.end,
                    "Typed rule activation requires the selected model namespace",
                )]);
            }
            aliases.insert(alias.value.clone(), namespace.value.clone());
        }
    }
    let resolve_receiver = |alias: &str, name: &str| -> Result<String, String> {
        if !imported_models {
            return schema.language_class_key(name).map(str::to_owned);
        }
        let namespace = aliases
            .get(alias)
            .ok_or("Provider target requires an exact model import")?;
        let mut candidates = schema.declarations.iter().filter(|(key, declaration)| {
            declaration.namespace == *namespace
                && declaration.name == name
                && schema.classes.contains_key(*key)
        });
        let key = candidates
            .next()
            .map(|(key, _)| key.clone())
            .ok_or("Provider receiver is not in the selected source world")?;
        if candidates.next().is_some() {
            return Err("Provider receiver requires one exact runtime class".into());
        }
        Ok(key)
    };
    let class = |path: &Path| -> Result<String, String> {
        if path.len() != 3 || !aliases.contains_key(&path[0].value) {
            return Err("Provider target requires a selected model import".into());
        }
        resolve_receiver(&path[0].value, &path[1].value)
    };
    for (ordinal, declaration) in doc.declarations.iter().enumerate() {
        // Getter selector inspection does not install or lower unrelated scope,
        // operation, validation or construction services. Keep original ordinals.
        if derive_only
            && !matches!(
                declaration.value,
                Declaration::ModelImport { .. }
                    | Declaration::Function { .. }
                    | Declaration::Derive { .. }
            )
        {
            continue;
        }
        let result = (|| -> Result<(), String> {
            match &declaration.value {
                Declaration::ScopeBinding { .. } => {
                    rules.scope_bindings.push(crate::definitions::rcore_scope::ScopeBindingDescriptor {
                        source: doc.source.id.clone(),
                        declaration: ordinal,
                    });
                },
                Declaration::DefaultBinding { .. } => {
                    return Err("Unsupported: `bind default` runs only on the frozen engine; packages do not install it".into());
                },
                Declaration::ImplicitBinding { .. } => {
                    return Err("Unsupported: `bind implicit` runs only on the frozen engine; packages do not install it".into());
                },
                Declaration::ValidateBinding { .. } => {
                    return Err("Unsupported: `bind validate` runs only on the frozen engine; packages do not install it".into());
                },
                Declaration::ModelImport { .. } | Declaration::Function { .. } => {},
                Declaration::Initialize { target, provider, version, members } => {
                    if target.len() != 2 || !aliases.contains_key(&target[0].value) {
                        return Err("Initializer target requires an imported model class".into());
                    }
                    let class = resolve_receiver(&target[0].value, &target[1].value)?;
                    let mut values = std::collections::BTreeMap::new();
                    let mut names = std::collections::BTreeSet::new();
                    for (name, value) in members {
                        if !names.insert(name.value.clone()) { return Err("Duplicate initializer member".into()); }
                        if let Some(value) = value {
                            values.insert(name.value.clone(), constant(doc, *value, 0)?);
                        }
                    }
                    let constant_provider = provider.value == "intersect.constant-attributes";
                    rules.initializers.push(ClassInitialization {
                        class,
                        provider: crate::definitions::conversion::ProviderId { name: provider.value.clone(), version: version.value.clone() },
                        members: members.iter().map(|(name, _)| name.value.clone()).collect(),
                        values: (constant_provider || !values.is_empty()).then_some(values),
                    });
                },
                Declaration::Projection { target, input, writable } => {
                    if target.len() != 3 || !aliases.contains_key(&target[0].value) {
                        return Err("Projection target requires a selected model import".into());
                    }
                    // One generic declaration supplies the same contract to each
                    // explicitly materialized application. Their feature domains
                    // are still checked separately by check_semantics.
                    let namespace = aliases.get(&target[0].value).ok_or("Projection requires an exact imported model")?;
                    let declarations = schema.declarations.iter().filter(|(key, declaration)| {
                        schema.classes.contains_key(*key)
                            && declaration.namespace == *namespace
                            && declaration.name == target[1].value
                    }).collect::<Vec<_>>();
                    let Some((_, identity)) = declarations.first() else {
                        return Err(format!("Unknown projection class {}", target[1].value));
                    };
                    if declarations.iter().any(|(_, declaration)| declaration.source != identity.source || declaration.path != identity.path) {
                        return Err("Ambiguous projection declaration".into());
                    }
                    for (class, _) in declarations {
                        rules.derived.push(Derivation { class: class.clone(), feature: target[2].value.clone(), expression: Expression::Stored { feature: input.value.clone(), writable: *writable } });
                    }
                },
                Declaration::RuleConversion(binding) => rules.rule_conversions.push(binding.clone()),
                Declaration::DerivedContract { target, contract, .. } => {
                    let expression = match contract {
                        DerivedContract::Count(input) => Expression::Count { feature: input.value.clone() },
                        DerivedContract::NoIncoming(input) => Expression::NoIncoming { class: class(input)?, feature: input[2].value.clone() },
                    };
                    rules.derived.push(Derivation { class: class(target)?, feature: target[2].value.clone(), expression });
                },
                Declaration::Derive { target, receiver, body, .. } => {
                    let expression = if let Some(value) = call(doc, *body, "size") {
                        Expression::Count { feature: member(doc, value, &receiver.value).ok_or("Count requires a direct stored receiver feature")?.into() }
                    } else if let Some(value) = call(doc, *body, "isEmpty") {
                        let (source, owner, feature) = incoming(doc, value, &aliases, &resolve_receiver)?;
                        if !is_name(doc, owner, &receiver.value) { return Err("Availability requires incoming references to the receiver".into()); }
                        Expression::NoIncoming { class: source, feature }
                    } else if source_call(doc, *body, &receiver.value) {
                        Expression::Source { source: doc.source.id.clone(), declaration: ordinal }
                    } else { external_call(doc, *body, &receiver.value, &class(target)?, schema)? };
                    rules.derived.push(Derivation { class: class(target)?, feature: target[2].value.clone(), expression });
                },
                Declaration::Validate { target, receiver, requirements, .. } => {
                    let owner = class(target)?;
                    for requirement in requirements {
                        let (condition, each) = validation_condition(doc, requirement.value.condition, &receiver.value)?;
                        let Expr::Binary { operator, left, right } = expr(doc, condition)? else { return Err("Validation requires an incoming size upper bound".into()); };
                        if operator.value != "<=" { return Err("Validation requires <=".into()); }
                        let counted = call(doc, *left, "size").ok_or("Validation requires size(incoming(...))")?;
                        let (source, value, feature) = incoming(doc, counted, &aliases, &resolve_receiver)?;
                        if each.is_some() && !schema.features(&owner)?.get(&feature).ok_or("Unknown validation reference")?.unique {
                            return Err("Validation collection activation requires a unique reference; duplicate-edge counting differs from incoming source counting".into());
                        }
                        let selects_reference = match each {
                            Some(each) => each.feature == feature && is_name(doc, value, each.parameter),
                            None => member(doc, value, &receiver.value) == Some(feature.as_str()),
                        };
                        if source != owner || !selects_reference { return Err("Validation must count the declared class and receiver reference".into()); }
                        let Expr::Literal(Literal::Number(limit)) = expr(doc, *right)? else { return Err("Validation upper bound must be an unsigned integer literal".into()); };
                        let Expr::Literal(Literal::String(message)) = expr(doc, requirement.value.message)? else { return Err("Validation message must be a string literal".into()); };
                        rules.validations.push(Validation { class: owner.clone(), feature, max_incoming: limit.parse().map_err(|_| "Invalid validation upper bound")?, message: message.clone() });
                    }
                },
                _ => return Err("Sources type checked successfully; Typed package function activation is not implemented; standalone functions use the function compiler".into()),
            }
            Ok(())
        })();
        if let Err(message) = result {
            return Err(vec![Diagnostic::new(
                "rcore_behavior",
                declaration.span.start,
                declaration.span.end,
                message,
            )]);
        }
    }
    Ok(rules)
}

// Normalize the singular rule and its collection form into the same shared
// validation. Only a direct receiver feature and the exact lambda parameter
// qualify; arbitrary callbacks are not executable rule implementations.
struct CollectionValidation<'a> {
    feature: &'a str,
    parameter: &'a str,
}

fn validation_condition<'a>(
    doc: &'a Document,
    condition: ExprId,
    receiver: &str,
) -> Result<(ExprId, Option<CollectionValidation<'a>>), String> {
    let Expr::Call {
        callee,
        types,
        arguments,
    } = expr(doc, condition)?
    else {
        return Ok((condition, None));
    };
    if !is_name(doc, *callee, "forAll") || !types.is_empty() || arguments.len() != 2 {
        return Err(
            "Validation collection form requires forAll(receiver.feature, |item| ...)".into(),
        );
    }
    let selected = member(doc, arguments[0], receiver)
        .ok_or("Validation collection requires a direct receiver feature")?;
    let Expr::Lambda { parameters, body } = expr(doc, arguments[1])? else {
        return Err("Validation collection requires a direct lambda".into());
    };
    if parameters.len() != 1 {
        return Err("Validation collection requires one lambda parameter".into());
    }
    Ok((
        *body,
        Some(CollectionValidation {
            feature: selected,
            parameter: parameters[0].name.value.as_str(),
        }),
    ))
}

fn constant(doc: &Document, id: ExprId, depth: usize) -> Result<serde_json::Value, String> {
    use serde_json::Value;
    if depth >= 64 {
        return Err("Initializer constant nesting exceeds 64 levels".into());
    }
    Ok(match expr(doc, id)? {
        Expr::Literal(Literal::String(value)) => Value::String(value.clone()),
        Expr::Literal(Literal::Boolean(value)) => Value::Bool(*value),
        Expr::Literal(Literal::None) => Value::Null,
        Expr::Literal(Literal::Number(value)) => {
            Value::Number(value.parse().map_err(|_| "Invalid initializer number")?)
        }
        Expr::Unary { operator, operand } if operator.value == "-" => {
            let Expr::Literal(Literal::Number(value)) = expr(doc, *operand)? else {
                return Err("Initializer negation requires a numeric literal".into());
            };
            Value::Number(
                format!("-{value}")
                    .parse()
                    .map_err(|_| "Invalid initializer number")?,
            )
        }
        Expr::List(values) => Value::Array(
            values
                .iter()
                .map(|id| constant(doc, *id, depth + 1))
                .collect::<Result<_, _>>()?,
        ),
        Expr::Record(fields) => {
            let mut result = serde_json::Map::new();
            for (name, value) in fields {
                if result
                    .insert(name.value.clone(), constant(doc, *value, depth + 1)?)
                    .is_some()
                {
                    return Err("Duplicate initializer record field".into());
                }
            }
            Value::Object(result)
        }
        Expr::Group(id) => constant(doc, *id, depth + 1)?,
        _ => {
            return Err(
                "Initializer values must be constants; provider execution belongs to the host"
                    .into(),
            );
        }
    })
}

fn expr(doc: &Document, id: ExprId) -> Result<&Expr, String> {
    doc.expressions
        .get(id.0)
        .map(|e| &e.value)
        .ok_or_else(|| "Missing typed rule expression".into())
}
fn is_name(doc: &Document, id: ExprId, expected: &str) -> bool {
    matches!(expr(doc, id), Ok(Expr::Name(name)) if name.value == expected)
}
fn call(doc: &Document, id: ExprId, name: &str) -> Option<ExprId> {
    let Expr::Call {
        callee,
        types,
        arguments,
    } = expr(doc, id).ok()?
    else {
        return None;
    };
    (types.is_empty() && arguments.len() == 1 && is_name(doc, *callee, name)).then(|| arguments[0])
}
fn member<'a>(doc: &'a Document, id: ExprId, receiver_name: &str) -> Option<&'a str> {
    let Expr::Member {
        receiver,
        name,
        safe: false,
    } = expr(doc, id).ok()?
    else {
        return None;
    };
    is_name(doc, *receiver, receiver_name).then_some(name.value.as_str())
}
fn incoming(
    doc: &Document,
    id: ExprId,
    aliases: &std::collections::BTreeMap<String, String>,
    resolve_receiver: &impl Fn(&str, &str) -> Result<String, String>,
) -> Result<(String, ExprId, String), String> {
    let Expr::Call {
        callee,
        types,
        arguments,
    } = expr(doc, id)?
    else {
        return Err("Expected incoming call".into());
    };
    if !is_name(doc, *callee, "incoming") || types.len() != 1 || arguments.len() != 2 {
        return Err("Expected incoming::<Model>(target, \"feature\")".into());
    }
    let Type::Named {
        path,
        arguments: actuals,
    } = &doc
        .types
        .get(types[0].0)
        .ok_or("Missing incoming type")?
        .value
    else {
        return Err("Incoming requires a concrete model class".into());
    };
    if path.len() != 2 || !actuals.is_empty() || !aliases.contains_key(&path[0].value) {
        return Err("Incoming requires the selected model import".into());
    }
    let Expr::Literal(Literal::String(feature)) = expr(doc, arguments[1])? else {
        return Err("Incoming requires a literal feature selector".into());
    };
    Ok((
        resolve_receiver(&path[0].value, &path[1].value)?,
        arguments[0],
        feature.clone(),
    ))
}

fn source_call(doc: &Document, body: ExprId, receiver: &str) -> bool {
    let Ok(Expr::Call {
        callee,
        types,
        arguments,
    }) = expr(doc, body)
    else {
        return false;
    };
    if !types.is_empty() || arguments.len() != 1 || !is_name(doc, arguments[0], receiver) {
        return false;
    }
    let Ok(Expr::Name(name)) = expr(doc, *callee) else {
        return false;
    };
    doc.declarations
        .iter()
        .filter(|declaration| {
            matches!(&declaration.value,
        Declaration::Function { external: false, name: selected, generics, .. }
            if selected.value == name.value && generics.is_empty())
        })
        .count()
        == 1
}

fn external_call(
    doc: &Document,
    body: ExprId,
    receiver: &str,
    class: &str,
    schema: &Schema,
) -> Result<Expression, String> {
    let Expr::Call {
        callee,
        types,
        arguments,
    } = expr(doc, body)?
    else {
        return Err("Derivation requires a supported builtin or a direct external call".into());
    };
    let Expr::Name(name) = expr(doc, *callee)? else {
        return Err("Derived external calls require a local function name".into());
    };
    if !types.is_empty() || doc.declarations.iter().filter(|d| matches!(&d.value,
        Declaration::Function { external: true, name: n, generics, .. } if n.value == name.value && generics.is_empty())).count() != 1 {
        return Err("Derived calls require one non-generic external declaration".into());
    }
    let fields = schema.features(class)?;
    let arguments = arguments.iter().map(|id| {
        if is_name(doc, *id, receiver) { return Ok(DerivedArgument::Receiver); }
        let feature = member(doc, *id, receiver).ok_or("External arguments require the receiver or a direct stored feature; nested calls are unsupported")?;
        let field = fields.get(feature).ok_or("Unknown external argument feature")?;
        if field.derived || field.reference || field.lower != 1 || field.upper != 1 || field.datatype.is_some()
            || !matches!(field.target.as_str(), "ELong" | "EString" | "EBoolean") {
            return Err("External arguments require a required stored Int64, String or Bool attribute".into());
        }
        Ok(DerivedArgument::Stored { feature: feature.into() })
    }).collect::<Result<Vec<_>, String>>()?;
    Ok(Expression::External {
        function: name.value.clone(),
        arguments,
    })
}
