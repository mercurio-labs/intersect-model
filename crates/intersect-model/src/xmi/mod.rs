//! Handwritten native XMI 2.0 codec for the bounded Intersect Ecore profile.
//! No I/O, generated code, or Java runtime dependency. Explicit contexts borrow
//! language-owned lexical providers; the codec owns no conversion algorithm.
mod xml;
use intersect_model::core::{
    interchange::{DecodedModel, FormatContext, ModelFormat, Resource},
    model::{Element, Model, values},
    resources::{self, ResourceRecord},
};
use intersect_model::definitions::{
    scalars::Builtin,
    schema::{Feature, Schema},
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use xml::{IX, Node, XMI, XSI};

pub struct XmiFormat;
impl ModelFormat for XmiFormat {
    fn id(&self) -> &str {
        "xmi"
    }
    fn extensions(&self) -> &[&str] {
        &["xmi"]
    }
    fn media_type(&self) -> &str {
        "application/xmi+xml"
    }
    fn decode(&self, schema: &Schema, resources: &[Resource]) -> Result<Model, String> {
        decode(schema, resources, &FormatContext::default())
    }
    fn encode(&self, schema: &Schema, model: &Model) -> Result<Vec<Resource>, String> {
        encode(schema, model, None, &FormatContext::default())
    }
    fn decode_with_context(
        &self,
        schema: &Schema,
        resources: &[Resource],
        retained: &Model,
    ) -> Result<Model, String> {
        decode_context(schema, resources, retained, &FormatContext::default())
    }
    fn encode_selected(
        &self,
        schema: &Schema,
        model: &Model,
        selected: &[String],
    ) -> Result<Vec<Resource>, String> {
        encode(schema, model, Some(selected), &FormatContext::default())
    }
    fn decode_with_format_context(
        &self,
        schema: &Schema,
        resources: &[Resource],
        retained: Option<&Model>,
        context: &FormatContext<'_>,
    ) -> Result<Model, String> {
        match retained {
            Some(retained) => decode_context(schema, resources, retained, context),
            None => decode(schema, resources, context),
        }
    }
    fn decode_with_format_context_and_proof(
        &self,
        schema: &Schema,
        resources: &[Resource],
        retained: Option<&Model>,
        context: &FormatContext<'_>,
    ) -> Result<DecodedModel, String> {
        match retained {
            Some(retained) => decode_context_with_proof(schema, resources, retained, context),
            None => decode_context_with_proof(
                schema,
                resources,
                &Model {
                    schema_revision: schema.revision.clone(),
                    ..Model::default()
                },
                context,
            ),
        }
    }
    fn encode_with_format_context(
        &self,
        schema: &Schema,
        model: &Model,
        selected: Option<&[String]>,
        context: &FormatContext<'_>,
    ) -> Result<Vec<Resource>, String> {
        encode(schema, model, selected, context)
    }
}

/// XMI of models on the frozen engine (`core::frozen`).
///
/// A frozen model is validated by the frozen validator, whose rules differ
/// from the shared persistent validation (`FormatContext::validate`) in one
/// respect: a feature redefined in an object's class (MOF property
/// redefinition, `frozen::Metamodel::with_redefinitions`) is read through
/// its redefining feature, and its own value may stay empty although its
/// lower bound is not zero (`OwningMembership` redefines
/// `Membership.memberElement` by `ownedMemberElement`). The shared
/// validation, which knows no redefinitions, would reject such a model, so
/// these entry points validate with the frozen rules instead.
///
/// What is serialized follows EMF, where the redefining class's `eIsSet` of a
/// redefined feature is false: a redefined feature is never written; its
/// value travels in the redefining feature when that one is stored, and is
/// computed by the language when that one is derived. A value held in a
/// redefined attribute's own slot is written under its stored redefining
/// attribute (EMF's one field: `RequirementDefinition.reqId` is its
/// `declaredShortName`) and comes back in the redefining slot, where frozen
/// reads of either feature look. Any other non-default value in a redefined
/// feature cannot be carried and is an explicit error, as is an XMI document
/// that writes a redefined feature. Everything else is as for
/// [`XmiFormat::encode_selected`] and [`ModelFormat::decode_with_context`],
/// with no format context (no conversion or structural-value providers).
impl XmiFormat {
    /// Encode the resources `selected` of `model` (every resource when
    /// `None`), one XMI document per resource named by its URI when
    /// selected. References into resources that are not selected are hrefs
    /// to those resources' URIs.
    pub fn encode_frozen(
        &self,
        model: &crate::core::frozen::Model,
        selected: Option<&[String]>,
    ) -> Result<Vec<Resource>, String> {
        let metamodel = model.metamodel();
        // A frozen model exists only validated (its constructors validate,
        // or load the host's own validated snapshot): it is the evidence the
        // frozen rules held.
        let materialized = model.to_model().map_err(|error| error.to_string())?;
        encode_with_rules(
            metamodel.schema(),
            &materialized,
            selected,
            &FormatContext::default(),
            Rules::Frozen(metamodel),
        )
    }
    /// Decode `resources` on top of `retained` (whose resources the XMI may
    /// reference by href, and which it must not replace) into one frozen
    /// model of `metamodel`, validated by the frozen validator.
    pub fn decode_frozen(
        &self,
        metamodel: &std::sync::Arc<crate::core::frozen::Metamodel>,
        resources: &[Resource],
        retained: Option<&crate::core::frozen::Model>,
    ) -> Result<crate::core::frozen::Model, String> {
        let schema = metamodel.schema();
        let retained = match retained {
            Some(retained) => {
                if retained.metamodel().revision() != metamodel.revision() {
                    return Err("Retained frozen model belongs to another metamodel".into());
                }
                retained.to_model().map_err(|error| error.to_string())?
            }
            None => Model {
                schema_revision: schema.revision.clone(),
                ..Model::default()
            },
        };
        let decoded = decode_with_rules(
            schema,
            resources,
            &retained,
            &FormatContext::default(),
            Rules::Frozen(metamodel),
        )?
        .into_model();
        crate::core::frozen::Model::from_model(metamodel.clone(), &decoded)
            .map_err(|error| format!("Invalid frozen XMI model: {error}"))
    }
}

/// Which validation rules hold for the model a codec call encodes or decodes.
#[derive(Clone, Copy)]
enum Rules<'a> {
    /// The shared persistent validation (`FormatContext::validate`).
    Shared,
    /// The frozen validator's, with this metamodel's redefinitions. The
    /// caller validates (an encoded model is a materialized frozen model; a
    /// decoded one is imported into the frozen engine).
    Frozen(&'a crate::core::frozen::Metamodel),
}
impl Rules<'_> {
    /// The feature that redefines `feature` in `class`, when the frozen
    /// rules hold and one does. XMI never carries the redefined feature.
    fn redefining(&self, class: &str, feature: &str) -> Option<&str> {
        let Rules::Frozen(metamodel) = self else {
            return None;
        };
        let class = metamodel.class(class)?;
        let feature = metamodel.feature(class, feature)?;
        metamodel
            .redefined_by(class, feature)
            .map(|redefining| metamodel.feature_name(redefining))
    }
}
/// The effective features of one class (`Schema::features`) and, under the
/// frozen rules, its redefined features by name with their redefining ones.
struct ClassFeatures {
    fields: BTreeMap<String, Feature>,
    redefined: BTreeMap<String, String>,
}
/// Per-call memo of [`ClassFeatures`]: a model has many objects of few
/// classes, and the feature selection is costly to repeat per object (a
/// user file on the SysML library touches 110 000 objects).
struct Classes<'a> {
    schema: &'a Schema,
    rules: Rules<'a>,
    memo: std::cell::RefCell<BTreeMap<String, std::rc::Rc<ClassFeatures>>>,
}
impl<'a> Classes<'a> {
    fn new(schema: &'a Schema, rules: Rules<'a>) -> Self {
        Self {
            schema,
            rules,
            memo: Default::default(),
        }
    }
    fn get(&self, class: &str) -> Result<std::rc::Rc<ClassFeatures>, String> {
        if let Some(found) = self.memo.borrow().get(class) {
            return Ok(found.clone());
        }
        let fields = self.schema.features(class)?;
        let redefined = fields
            .keys()
            .filter_map(|name| {
                self.rules
                    .redefining(class, name)
                    .map(|redefining| (name.clone(), redefining.to_owned()))
            })
            .collect();
        let entry = std::rc::Rc::new(ClassFeatures { fields, redefined });
        self.memo
            .borrow_mut()
            .insert(class.to_owned(), entry.clone());
        Ok(entry)
    }
}
/// The values XMI writes under `e`'s redefining attributes for redefined
/// attributes that hold a value. In EMF a redefined attribute and a stored
/// attribute that redefines it are one field written under the redefining
/// name (`RequirementDefinition.reqId` is its `declaredShortName`), so a
/// value in the redefined feature is written as the redefining one when
/// that is a stored attribute of the same multiplicity holding nothing else.
/// Any other value in a redefined feature cannot be carried: an error.
fn carried<'e>(
    e: &'e Element,
    class: &ClassFeatures,
) -> Result<BTreeMap<String, &'e Value>, String> {
    let mut carried = BTreeMap::new();
    for (name, redefining) in &class.redefined {
        let Some(feature) = class.fields.get(name) else {
            continue;
        };
        let Some(value) = e
            .values
            .get(name)
            .filter(|value| !omittable(feature, Some(value)))
        else {
            continue;
        };
        let target = class.fields.get(redefining).filter(|target| {
            !target.derived
                && !target.transient
                && !target.reference
                && !feature.reference
                && target.upper == feature.upper
                && target.target == feature.target
        });
        let free = target.is_some_and(|target| {
            let current = e.values.get(redefining);
            omittable(target, current) || current == Some(value)
        });
        if !free {
            return Err(format!(
                "XMI cannot carry the value of {}.{name} in {}: it is redefined by {redefining}",
                e.class, e.id
            ));
        }
        if carried
            .insert(redefining.clone(), value)
            .is_some_and(|previous| previous != value)
        {
            return Err(format!(
                "XMI cannot carry two values for {}.{redefining} in {}",
                e.class, e.id
            ));
        }
    }
    Ok(carried)
}
/// A redefined feature's value is omitted from XMI; only an empty value or
/// the feature's default (what an import gives back) may be omitted.
fn omittable(feature: &Feature, value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::Array(items)) if items.is_empty() => true,
        Some(value) => *value == feature.default,
    }
}

fn profile(schema: &Schema, context: &FormatContext<'_>) -> Result<(), String> {
    if schema.uri.is_empty() || [XMI, XSI, IX].contains(&schema.uri.as_str()) {
        return Err("Unsupported EPackage namespace".into());
    }
    for (name, class) in &schema.classes {
        if !xml::valid_name(name) {
            return Err(format!("Unsupported XML class name: {name}"));
        }
        if context.conversions.is_some()
            && schema.declarations.get(name).is_some_and(|declaration| {
                declaration.namespace != schema.uri || declaration.name != *name
            })
        {
            return Err(format!(
                "XMI requires an authored class name in the selected namespace: {name}"
            ));
        }
        for (name, feature) in &class.features {
            let supported_attribute = if schema.enumerations.contains_key(&feature.target) {
                enumeration(schema, feature).is_some()
            } else if feature.target == "EDouble" {
                true
            } else if !feature.reference
                && !feature.derived
                && context.conversions.is_some()
                && schema.datatypes.contains_key(&feature.target)
            {
                converted_datatype(schema, feature, context)?;
                true
            } else {
                ["EString", "EInt", "EBoolean"].contains(&feature.target.as_str())
            };
            // Derived values are evaluated by the language and never serialized.
            // Decode rejects attempts to supply them, so no payload codec is needed.
            if !xml::valid_name(name)
                || (!feature.reference && !feature.derived && !supported_attribute)
            {
                return Err(format!("Unsupported XMI feature: {name}"));
            }
        }
    }
    Ok(())
}
fn assigned(element: &Element, feature: &Feature) -> bool {
    !feature.unsettable
        || element
            .assignment_state
            .as_ref()
            .is_some_and(|state| state.explicitly_set(&feature.name))
}
fn mark_assignment(element: &mut Element, feature: &Feature) -> Result<(), String> {
    if feature.unsettable {
        element.assignment_state = Some(
            element
                .assignment_state
                .as_ref()
                .ok_or("Missing XMI assignment state")?
                .with_assignment(&feature.name, true)?,
        );
    }
    Ok(())
}
fn enumeration<'a>(
    schema: &'a Schema,
    feature: &Feature,
) -> Option<&'a intersect_model::definitions::enums::Enumeration> {
    schema
        .enumerations
        .get(&feature.target)
        .filter(|enumeration| {
            feature.datatype.as_ref() == Some(enumeration.declaration())
                && enumeration.serializable()
        })
}
fn converted_datatype<'a>(
    schema: &'a Schema,
    feature: &Feature,
    context: &FormatContext<'_>,
) -> Result<&'a intersect_model::definitions::datatypes::Datatype, String> {
    let datatype = schema
        .datatypes
        .get(&feature.target)
        .ok_or("Missing XMI datatype")?;
    if feature.datatype.as_ref() != Some(datatype.declaration()) {
        return Err("XMI feature datatype identity mismatch".into());
    }
    let conversions = context
        .conversions
        .ok_or("XMI datatype requires explicit conversion providers")?;
    if datatype.application().is_some()
        || !matches!(
            datatype.binding(),
            intersect_model::definitions::datatypes::Binding::Unsupported { .. }
        )
        || !datatype.serializable()
    {
        return Err("Datatype is outside the serializable XMI conversion profile".into());
    }
    schema.require_attribute_conversion(feature, Some(conversions))?;
    Ok(datatype)
}
fn scalar_text(
    schema: &Schema,
    feature: &Feature,
    value: &Value,
    context: &FormatContext<'_>,
) -> Result<String, String> {
    if let Some(enumeration) = enumeration(schema, feature) {
        let value = serde_json::from_value(value.clone())
            .map_err(|e| format!("Invalid XMI enum value: {e}"))?;
        return enumeration.format(&value).map(str::to_owned);
    }
    if feature.target == "EDouble" {
        return Builtin::EDouble
            .format(value)?
            .ok_or_else(|| "EDouble requires a present scalar value".into());
    }
    if context.conversions.is_some() && schema.datatypes.contains_key(&feature.target) {
        let datatype = converted_datatype(schema, feature, context)?;
        let conversions = context
            .conversions
            .ok_or("Missing XMI conversion providers")?;
        let value = serde_json::from_value(value.clone())
            .map_err(|error| format!("Invalid XMI converted value: {error}"))?;
        conversions.validate_for(datatype, &value)?;
        return conversions.format(&value);
    }
    Ok(value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned))
}
fn scalar(
    schema: &Schema,
    feature: &Feature,
    text: &str,
    context: &FormatContext<'_>,
) -> Result<Value, String> {
    if let Some(enumeration) = enumeration(schema, feature) {
        return serde_json::to_value(enumeration.parse(text)?).map_err(|e| e.to_string());
    }
    if feature.target == "EDouble" {
        return Builtin::EDouble.parse(text);
    }
    if context.conversions.is_some() && schema.datatypes.contains_key(&feature.target) {
        let datatype = converted_datatype(schema, feature, context)?;
        let conversions = context
            .conversions
            .ok_or("Missing XMI conversion providers")?;
        let value = conversions.parse(datatype.declaration(), text)?;
        conversions.validate_for(datatype, &value)?;
        return serde_json::to_value(value).map_err(|error| error.to_string());
    }
    match feature.target.as_str() {
        "EString" => Ok(Value::String(text.into())),
        "EInt" => text
            .trim()
            .parse::<i32>()
            .map(Value::from)
            .map_err(|_| format!("Invalid EInt: {text}")),
        "EBoolean" => match text.trim() {
            "true" | "1" => Ok(Value::Bool(true)),
            "false" | "0" => Ok(Value::Bool(false)),
            _ => Err(format!("Invalid EBoolean: {text}")),
        },
        _ => Err(format!("Unsupported datatype: {}", feature.target)),
    }
}
fn version(node: &Node) -> Result<(), String> {
    if node.attr(XMI, "version").is_some_and(|v| v != "2.0") {
        return Err("Only the XMI 2.0 profile is supported".into());
    }
    Ok(())
}
fn resource_uri(uri: &str) -> Result<(), String> {
    resources::validate_resource_uri(uri)
}
// Shared lexical resolution never fetches resources or inspects host paths.
fn resolve_resource(base: &str, reference: &str) -> Result<String, String> {
    resources::resolve_resource_uri(base, reference)
}

#[derive(Clone)]
struct Provenance {
    /// Added by the language rather than written in source (`Element::implicit`).
    implicit: bool,
    absent_stored: BTreeSet<String>,
    initialization: Option<intersect_model::definitions::initialization::InitializationReceipt>,
    id: String,
    resource: String,
    start: usize,
    end: usize,
    order: usize,
}
fn metadata(
    node: &Node,
    schema: &Schema,
) -> Result<(BTreeMap<String, Provenance>, Option<ResourceRecord>), String> {
    node.attributes(&[("", "extender")])?;
    node.whitespace()?;
    if node.attr("", "extender") != Some(IX) || node.children.len() != 1 {
        return Err("Unsupported XMI extension".into());
    }
    let model = &node.children[0];
    if model.name != (IX.into(), "model".into()) {
        return Err("Unsupported Intersect XMI extension".into());
    }
    model.attributes(&[("", "schemaRevision"), ("", "resourceRecord")])?;
    model.whitespace()?;
    if model.attr("", "schemaRevision") != Some(schema.revision.as_str()) {
        return Err("XMI schema revision mismatch".into());
    }
    let mut result = BTreeMap::new();
    for e in &model.children {
        if e.name != (IX.into(), "element".into()) || !e.children.is_empty() {
            return Err("Unsupported XMI provenance entry".into());
        }
        e.whitespace()?;
        e.attributes(&[
            ("", "xmiId"),
            ("", "id"),
            ("", "resource"),
            ("", "start"),
            ("", "end"),
            ("", "order"),
            ("", "initialization"),
            ("", "absentStored"),
            ("", "implicit"),
        ])?;
        let attr = |key| {
            e.attr("", key)
                .ok_or_else(|| format!("Missing provenance {key}"))
        };
        let number = |key| {
            attr(key)?
                .parse::<usize>()
                .map_err(|_| format!("Invalid provenance {key}"))
        };
        let absent: Vec<String> = e
            .attr("", "absentStored")
            .map(serde_json::from_str)
            .transpose()
            .map_err(|error| format!("Invalid absent stored provenance: {error}"))?
            .unwrap_or_default();
        let absent_stored: BTreeSet<String> = absent.iter().cloned().collect();
        if absent_stored.len() != absent.len() || absent_stored.iter().any(String::is_empty) {
            return Err("Duplicate or empty absent stored provenance".into());
        }
        let entry = Provenance {
            implicit: match e.attr("", "implicit") {
                None => false,
                Some("true") => true,
                Some(_) => return Err("Invalid implicit provenance".into()),
            },
            absent_stored,
            initialization: e
                .attr("", "initialization")
                .map(serde_json::from_str)
                .transpose()
                .map_err(|error| format!("Invalid initialization receipt: {error}"))?,
            id: attr("id")?.into(),
            resource: attr("resource")?.into(),
            start: number("start")?,
            end: number("end")?,
            order: number("order")?,
        };
        resource_uri(&entry.resource)?;
        if entry.id.is_empty()
            || entry.start > entry.end
            || result.insert(attr("xmiId")?.into(), entry).is_some()
        {
            return Err("Invalid or duplicate XMI provenance".into());
        }
    }
    let record = model
        .attr("", "resourceRecord")
        .map(serde_json::from_str::<ResourceRecord>)
        .transpose()
        .map_err(|error| format!("Invalid XMI resource record: {error}"))?;
    Ok((result, record))
}
// Parse only the bounded readonly scalar wire cells; they stay outside the
// actual fresh row until a current source-owned initializer verifies them.
fn readonly_wire_values(
    node: &Node,
    schema: &Schema,
    fields: &BTreeMap<String, Feature>,
    context: &FormatContext<'_>,
) -> Result<Vec<(String, Value)>, String> {
    let mut values = BTreeMap::new();
    let supported = |feature: &Feature| -> Result<(), String> {
        if context.snapshot_initializers.is_none()
            || context.structural_values.is_none()
            || feature.derived
            || feature.reference
            || feature.containment
            || feature.upper != 1
        {
            return Err(format!(
                "XMI assignment to non-changeable feature is unsupported: {}",
                feature.name
            ));
        }
        Ok(())
    };
    for ((ns, name), text) in &node.attrs {
        if ns.is_empty()
            && let Some(feature) = fields.get(name).filter(|feature| !feature.changeable)
        {
            supported(feature)?;
            values.insert(name.clone(), scalar(schema, feature, text, context)?);
        }
    }
    for child in &node.children {
        if child.name.0.is_empty() || child.name.0 == schema.uri {
            let name = &child.name.1;
            if let Some(feature) = fields.get(name).filter(|feature| !feature.changeable) {
                supported(feature)?;
                child.attributes(&[(XSI, "nil")])?;
                if !child.children.is_empty() {
                    return Err("Nested readonly scalar content is unsupported".into());
                }
                let value = match child.attr(XSI, "nil") {
                    Some("true" | "1") if child.text.is_empty() => Value::Null,
                    Some(_) => return Err("Unsupported readonly xsi:nil value".into()),
                    None => scalar(schema, feature, &child.text, context)?,
                };
                if values.insert(name.clone(), value).is_some() {
                    return Err("Duplicate readonly snapshot assignment".into());
                }
            }
        }
    }
    Ok(values.into_iter().collect())
}

struct Import<'a, 'services> {
    context: FormatContext<'services>,
    classes: Classes<'a>,
    schema: &'a Schema,
    model: Model,
    aliases: BTreeMap<(String, String), String>,
    pending: Vec<(usize, String, Vec<String>)>,
    explicit: BTreeSet<(usize, String)>,
    orders: BTreeMap<String, usize>,
    retained_count: usize,
    readonly: Vec<crate::core::storage::VerifiedReadonlyInitialization>,
}
impl Import<'_, '_> {
    /// XMI never carries a feature redefined in the object's class (EMF's
    /// `eIsSet` of it is false there); its value is the redefining one's.
    fn writable(&self, class: &str, name: &str) -> Result<(), String> {
        match self.classes.get(class)?.redefined.get(name) {
            Some(redefining) => Err(format!(
                "Unsupported XMI assignment to {class}.{name}, redefined by {redefining}"
            )),
            None => Ok(()),
        }
    }
    fn alias(&mut self, uri: &str, fragment: &str, id: &str) -> Result<(), String> {
        if let Some(previous) = self
            .aliases
            .insert((uri.into(), fragment.into()), id.into())
            && previous != id
        {
            return Err(format!("Duplicate XMI identity: {uri}#{fragment}"));
        }
        Ok(())
    }
    fn object(
        &mut self,
        node: &Node,
        uri: &str,
        path: &str,
        declared: Option<&str>,
        provenance: &mut BTreeMap<String, Provenance>,
    ) -> Result<String, String> {
        node.whitespace()?;
        version(node)?;
        let class = if let Some(kind) = node.attr(XSI, "type").or_else(|| node.attr(XMI, "type")) {
            let (ns, kind) = node.qname(kind)?;
            if ns != self.schema.uri {
                return Err("XMI type namespace does not match metamodel".into());
            }
            kind
        } else if let Some(kind) = declared {
            kind.into()
        } else {
            if node.name.0 != self.schema.uri {
                return Err("XMI root namespace does not match metamodel".into());
            }
            node.name.1.clone()
        };
        if declared.is_some_and(|target| !self.schema.conforms(&class, target)) {
            return Err("XMI containment type mismatch".into());
        }
        if declared.is_none()
            && (node.name.0 != self.schema.uri || !self.schema.conforms(&class, &node.name.1))
        {
            return Err("XMI root type mismatch".into());
        }
        if node.attr(XSI, "type").is_some() && node.attr(XMI, "type").is_some() {
            return Err("Duplicate XMI type declaration".into());
        }
        let fragment = node.attr(XMI, "id").unwrap_or(path);
        if node.attr(XMI, "id").is_some_and(|id| !xml::valid_name(id)) {
            return Err("Unsupported xmi:id (ASCII NCName required)".into());
        }
        let saved = provenance.remove(fragment);
        let id = saved
            .as_ref()
            .map_or_else(|| format!("{uri}#{fragment}"), |p| p.id.clone());
        let resource = saved.as_ref().map_or(uri, |p| p.resource.as_str());
        let start = saved.as_ref().map_or(node.start, |p| p.start);
        let end = saved.as_ref().map_or(node.end, |p| p.end);
        let class_features = self.classes.get(&class)?;
        let features = &class_features.fields;
        let absent = saved
            .as_ref()
            .map(|saved| saved.absent_stored.clone())
            .unwrap_or_default();
        if !absent.is_empty() && self.context.structural_values.is_none() {
            return Err(
                "Absent stored XMI provenance requires explicit structural-value providers".into(),
            );
        }
        for name in &absent {
            let field = features
                .get(name)
                .ok_or("Unknown absent stored XMI member")?;
            if field.derived || field.containment {
                return Err("Unsupported absent stored XMI member".into());
            }
        }
        let element = Element::new(self.schema, &id, &class, resource, start, end)?;
        let initialization = saved
            .as_ref()
            .and_then(|saved| saved.initialization.clone());
        let implicit = saved.as_ref().is_some_and(|saved| saved.implicit);
        if let Some(saved) = saved {
            self.orders.insert(id.clone(), saved.order);
        }
        let readonly = readonly_wire_values(node, self.schema, features, &self.context)?;
        self.alias(uri, fragment, &id)?;
        self.alias(uri, path, &id)?;
        let index = self.model.elements.len();
        self.model.elements.push(element);
        if !readonly.is_empty() {
            let authority = self
                .context
                .structural_values
                .ok_or("Missing fresh snapshot operation context")?;
            let selector = self
                .context
                .snapshot_initializers
                .ok_or("Missing fresh snapshot initializer selector")?;
            let receipt = initialization
                .as_ref()
                .ok_or("Fresh readonly snapshot requires a matching initialization receipt")?;
            let verified = authority.verify_fresh_readonly(
                &self.model,
                self.schema,
                crate::core::storage::FreshReadonlySnapshot {
                    object: &id,
                    values: &readonly,
                    receipt,
                },
                selector,
            )?;
            self.readonly.push(verified);
        }
        // Metadata and wire values acquire no authority before the independent
        // current plan check. This is a new decoder row, never a retained object.
        self.model.elements[index].initialization = initialization;
        self.model.elements[index].implicit = implicit;
        for name in &absent {
            self.model.elements[index].values.remove(name);
        }
        for ((ns, name), text) in &node.attrs {
            if (ns == XMI && ["id", "version", "type"].contains(&name.as_str()))
                || (ns == XSI && name == "type")
            {
                continue;
            }
            if !ns.is_empty() {
                return Err(format!("Unsupported XMI attribute {{{ns}}}{name}"));
            }
            let feature = features
                .get(name)
                .ok_or_else(|| format!("Unknown feature {class}.{name}"))?;
            self.writable(&class, name)?;
            if absent.contains(name) {
                return Err(
                    "Authored XMI assignment conflicts with absent stored provenance".into(),
                );
            }
            if !feature.changeable {
                let (_, value) = readonly
                    .iter()
                    .find(|(field, _)| field == name)
                    .ok_or("Unsupported readonly snapshot assignment")?;
                self.model.elements[index]
                    .values
                    .insert(name.clone(), value.clone());
                mark_assignment(&mut self.model.elements[index], feature)?;
                self.explicit.insert((index, name.clone()));
                continue;
            }
            let explicit_empty = feature.unsettable && feature.upper != 1 && text.is_empty();
            if feature.derived || (feature.containment && !explicit_empty) {
                return Err(format!("Unsupported serialized feature {class}.{name}"));
            }
            self.explicit.insert((index, name.clone()));
            mark_assignment(&mut self.model.elements[index], feature)?;
            if explicit_empty {
                self.model.elements[index]
                    .values
                    .insert(name.clone(), Value::Array(vec![]));
            } else if feature.reference {
                self.pending.push((
                    index,
                    name.clone(),
                    text.split_whitespace().map(str::to_owned).collect(),
                ));
            } else if feature.upper == 1 {
                self.model.elements[index].values.insert(
                    name.clone(),
                    scalar(self.schema, feature, text, &self.context)?,
                );
            } else {
                return Err("Use repeated XML elements for multi-valued attributes".into());
            }
        }
        let mut counts = BTreeMap::<String, usize>::new();
        for child in &node.children {
            if declared.is_none() && child.name == (XMI.into(), "Extension".into()) {
                continue; // Validated and collected before object construction.
            }
            let name = &child.name.1;
            if !child.name.0.is_empty() && child.name.0 != self.schema.uri {
                return Err(format!("Unsupported namespaced feature: {:?}", child.name));
            }
            let feature = features
                .get(name)
                .ok_or_else(|| format!("Unknown feature {class}.{name}"))?;
            self.writable(&class, name)?;
            if absent.contains(name) {
                return Err(
                    "Authored XMI assignment conflicts with absent stored provenance".into(),
                );
            }
            if !feature.changeable {
                let (_, value) = readonly
                    .iter()
                    .find(|(field, _)| field == name)
                    .ok_or("Unsupported readonly snapshot assignment")?;
                self.model.elements[index]
                    .values
                    .insert(name.clone(), value.clone());
                mark_assignment(&mut self.model.elements[index], feature)?;
                self.explicit.insert((index, name.clone()));
                continue;
            }
            if feature.derived {
                return Err(format!("Cannot import derived feature {name}"));
            }
            let count = counts.entry(name.clone()).or_default();
            if *count == 0 && self.explicit.contains(&(index, name.clone()))
                || *count > 0 && feature.upper == 1
            {
                return Err(format!("Duplicate feature assignment: {name}"));
            }
            let value = if let Some(nil) = child.attr(XSI, "nil") {
                child.attributes(&[(XSI, "nil")])?;
                if !matches!(nil, "true" | "1")
                    || feature.upper != 1
                    || !child.text.is_empty()
                    || !child.children.is_empty()
                {
                    return Err("Unsupported xsi:nil value or collection null".into());
                }
                Value::Null
            } else if feature.containment {
                let suffix = if feature.upper == 1 {
                    format!("@{name}")
                } else {
                    format!("@{name}.{count}")
                };
                Value::String(self.object(
                    child,
                    uri,
                    &format!("{path}/{suffix}"),
                    Some(&feature.target),
                    provenance,
                )?)
            } else if feature.reference {
                child.attributes(&[("", "href")])?;
                child.whitespace()?;
                if !child.children.is_empty() {
                    return Err("Nested reference content is unsupported".into());
                }
                let href = child
                    .attr("", "href")
                    .ok_or("Reference element requires href")?;
                if href.is_empty() || href.chars().any(char::is_whitespace) {
                    return Err("Invalid reference href".into());
                }
                if let Some((_, _, refs)) = self
                    .pending
                    .iter_mut()
                    .find(|(i, f, _)| *i == index && f == name)
                {
                    refs.push(href.into());
                } else {
                    self.pending.push((index, name.clone(), vec![href.into()]));
                }
                Value::Null
            } else {
                child.attributes(&[(XSI, "nil")])?;
                if !child.children.is_empty() {
                    return Err("Nested scalar content is unsupported".into());
                }
                match child.attr(XSI, "nil") {
                    Some("true" | "1") if child.text.is_empty() && feature.upper == 1 => {
                        Value::Null
                    }
                    Some(_) => return Err("Unsupported xsi:nil value or collection null".into()),
                    None => scalar(self.schema, feature, &child.text, &self.context)?,
                }
            };
            if !feature.reference || feature.containment || child.attr(XSI, "nil").is_some() {
                let values = &mut self.model.elements[index].values;
                if feature.upper == 1 {
                    values.insert(name.clone(), value);
                } else {
                    if *count == 0 {
                        values.insert(name.clone(), Value::Array(vec![]));
                    }
                    values
                        .get_mut(name)
                        .and_then(Value::as_array_mut)
                        .ok_or("Invalid collection")?
                        .push(value);
                }
            }
            mark_assignment(&mut self.model.elements[index], feature)?;
            *count += 1;
            self.explicit.insert((index, name.clone()));
        }
        Ok(id)
    }
    fn finish(mut self, origins: &[String]) -> Result<DecodedModel, String> {
        for (index, name, references) in self.pending {
            let feature = self
                .classes
                .get(&self.model.elements[index].class)?
                .fields
                .get(&name)
                .cloned()
                .ok_or("Unknown reference")?;
            let mut targets = Vec::new();
            for reference in references {
                let (resource, fragment) = reference.split_once('#').unwrap_or(("", &reference));
                let uri = resolve_resource(&origins[index], resource)?;
                let target = self
                    .aliases
                    .get(&(uri.clone(), fragment.into()))
                    .ok_or_else(|| format!("Unresolved XMI reference: {uri}#{fragment}"))?;
                targets.push(Value::String(target.clone()));
            }
            let value = if feature.upper == 1 {
                if targets.len() > 1 {
                    return Err(format!("Multiple targets for singular reference {name}"));
                }
                targets.pop().unwrap_or(Value::Null)
            } else {
                Value::Array(targets)
            };
            self.model.elements[index].values.insert(name, value);
        }
        let ids: BTreeMap<_, _> = self
            .model
            .elements
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id.clone(), i))
            .collect();
        if ids.len() != self.model.elements.len() {
            return Err("Duplicate model identity".into());
        }
        // EMF omits container opposites and can serialize only one direction.
        // Fill only absent assignments; never repair contradictory authored data.
        // A retained frozen model is validated, opposites included, and the
        // frozen import validates the result again: only new objects fill.
        let first = match self.classes.rules {
            Rules::Frozen(_) => self.retained_count,
            Rules::Shared => 0,
        };
        for index in first..self.model.elements.len() {
            let class_features = self.classes.get(&self.model.elements[index].class)?;
            let mut opposites = class_features
                .fields
                .iter()
                .filter_map(|(name, feature)| {
                    let opposite = feature.opposite.as_ref().filter(|_| !feature.derived)?;
                    let set = self.model.elements[index]
                        .values
                        .get(name)
                        .is_some_and(|v| {
                            !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty())
                        });
                    set.then_some((name, feature, opposite))
                })
                .peekable();
            if opposites.peek().is_none() {
                continue;
            }
            let opposites: Vec<_> = opposites.collect();
            let element = self.model.elements[index].clone();
            for (name, feature, opposite) in opposites {
                let (_, back) = opposite
                    .split_once('/')
                    .ok_or("Invalid opposite declaration")?;
                for target in values(
                    element.values.get(name).unwrap_or(&Value::Null),
                    feature.upper,
                )? {
                    let target = *ids
                        .get(target.as_str().ok_or("Invalid reference identity")?)
                        .ok_or("Missing reference target")?;
                    let back_feature = self
                        .classes
                        .get(&self.model.elements[target].class)?
                        .fields
                        .get(back)
                        .cloned()
                        .ok_or("Unknown opposite")?;
                    let back_value = self.model.elements[target]
                        .values
                        .get_mut(back)
                        .ok_or("Missing opposite value")?;
                    if intersect_model::core::model::contains(Some(back_value), &element.id) {
                        continue;
                    }
                    // Retained objects are explicit in every feature.
                    if target < self.retained_count
                        || self.explicit.contains(&(target, back.into()))
                    {
                        return Err(format!("Conflicting explicit opposite: {back}"));
                    }
                    if back_feature.upper == 1 {
                        if !back_value.is_null() {
                            return Err("Multiple owners of singular opposite".into());
                        }
                        *back_value = Value::String(element.id.clone());
                    } else {
                        back_value
                            .as_array_mut()
                            .ok_or("Invalid opposite collection")?
                            .push(Value::String(element.id.clone()));
                    }
                    mark_assignment(&mut self.model.elements[target], &back_feature)?;
                }
            }
        }
        if !self.orders.is_empty() {
            let unique: BTreeSet<_> = self.orders.values().collect();
            if self.orders.len() != self.model.elements.len() - self.retained_count
                || unique.len() != self.orders.len()
            {
                return Err("Incomplete or duplicate provenance ordering".into());
            }
            let mut incoming = self.model.elements.split_off(self.retained_count);
            incoming.sort_by_key(|e| self.orders[&e.id]);
            self.model.elements.extend(incoming);
        }
        for element in &mut self.model.elements {
            for feature in self.classes.get(&element.class)?.fields.values() {
                if feature.unsettable && self.schema.is_container_feature(feature)? {
                    let assigned = element
                        .values
                        .get(&feature.name)
                        .is_some_and(|v| !v.is_null());
                    element.assignment_state = Some(
                        element
                            .assignment_state
                            .as_ref()
                            .ok_or("Missing XMI assignment state")?
                            .with_assignment(&feature.name, assigned)?,
                    );
                }
            }
        }
        // The frozen rules are checked by the caller's frozen import.
        if matches!(self.classes.rules, Rules::Shared) {
            self.context.validate(self.schema, &self.model)?;
        }
        DecodedModel::checked(self.model, self.readonly, self.schema, &self.context)
    }
}
fn decode(
    schema: &Schema,
    resources: &[Resource],
    context: &FormatContext<'_>,
) -> Result<Model, String> {
    decode_context(
        schema,
        resources,
        &Model {
            schema_revision: schema.revision.clone(),
            ..Model::default()
        },
        context,
    )
}
fn decode_context(
    schema: &Schema,
    resources: &[Resource],
    retained: &Model,
    context: &FormatContext<'_>,
) -> Result<Model, String> {
    decode_context_with_proof(schema, resources, retained, context).map(DecodedModel::into_model)
}
fn decode_context_with_proof(
    schema: &Schema,
    resources: &[Resource],
    retained: &Model,
    context: &FormatContext<'_>,
) -> Result<DecodedModel, String> {
    decode_with_rules(schema, resources, retained, context, Rules::Shared)
}
fn decode_with_rules(
    schema: &Schema,
    resources: &[Resource],
    retained: &Model,
    context: &FormatContext<'_>,
    rules: Rules<'_>,
) -> Result<DecodedModel, String> {
    profile(schema, context)?;
    if retained.schema_revision != schema.revision {
        return Err("Retained model schema revision mismatch".into());
    }
    let mut import = Import {
        context: *context,
        classes: Classes::new(schema, rules),
        schema,
        model: retained.clone(),
        aliases: BTreeMap::new(),
        pending: vec![],
        explicit: BTreeSet::new(),
        orders: BTreeMap::new(),
        retained_count: retained.elements.len(),
        readonly: Vec::new(),
    };
    // Retained aliases come only from the explicitly supplied model context.
    for element in &retained.elements {
        import.alias(
            &element.resource,
            &stable_fragment(&element.id),
            &element.id,
        )?;
        if let Some((uri, fragment)) = element.id.rsplit_once('#')
            && uri == element.resource
        {
            import.alias(uri, fragment, &element.id)?;
        }
    }
    let retained_by_id: BTreeMap<_, _> = retained
        .elements
        .iter()
        .map(|element| (element.id.as_str(), element))
        .collect();
    let mut visited = BTreeSet::new();
    for record in retained.resources.values() {
        let mut pending = Vec::new();
        for (index, id) in record.roots.iter().enumerate() {
            import.alias(&record.uri, &format!("/{index}"), id)?;
            let path = if record.roots.len() == 1 {
                "/".to_string()
            } else {
                format!("/{index}")
            };
            pending.push((id.as_str(), path));
        }
        while let Some((id, path)) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            let Some(element) = retained_by_id.get(id) else {
                continue;
            };
            import.alias(&element.resource, &path, id)?;
            for (name, feature) in &import.classes.get(&element.class)?.fields {
                if !feature.containment || feature.derived {
                    continue;
                }
                for (index, value) in values(
                    element.values.get(name).unwrap_or(&Value::Null),
                    feature.upper,
                )?
                .into_iter()
                .enumerate()
                {
                    let target = value
                        .as_str()
                        .ok_or("Invalid retained containment identity")?;
                    let suffix = if feature.upper == 1 {
                        format!("@{name}")
                    } else {
                        format!("@{name}.{index}")
                    };
                    pending.push((target, format!("{path}/{suffix}")));
                }
            }
        }
    }
    let mut seen = BTreeSet::new();
    let mut origins: Vec<_> = retained
        .elements
        .iter()
        .map(|e| e.resource.clone())
        .collect();
    for resource in resources {
        resource_uri(&resource.uri)?;
        if !seen.insert(resource.uri.clone()) {
            return Err("Duplicate resource URI".into());
        }
        let document = xml::parse(std::str::from_utf8(&resource.bytes).map_err(|e| e.to_string())?)
            .map_err(|e| format!("{}: {e}", resource.uri))?;
        let mut provenance = BTreeMap::new();
        let mut record = None;
        let mut extension_seen = false;
        let roots = if document.name == (XMI.into(), "XMI".into()) {
            document.attributes(&[(XMI, "version")])?;
            document.whitespace()?;
            version(&document)?;
            let mut roots = Vec::new();
            for child in &document.children {
                if child.name == (XMI.into(), "Extension".into()) {
                    if extension_seen {
                        return Err("Duplicate XMI extension".into());
                    }
                    extension_seen = true;
                    (provenance, record) = metadata(child, schema)?;
                } else {
                    roots.push(child);
                }
            }
            roots
        } else {
            vec![&document]
        };
        for root in &roots {
            for child in &root.children {
                if child.name == (XMI.into(), "Extension".into()) {
                    if extension_seen {
                        return Err("Duplicate XMI extension".into());
                    }
                    extension_seen = true;
                    (provenance, record) = metadata(child, schema)?;
                }
            }
        }
        let provenance_count = provenance.len();
        let before = import.model.elements.len();
        let mut root_ids = Vec::new();
        for (index, root) in roots.iter().enumerate() {
            let path = if roots.len() == 1 {
                "/".into()
            } else {
                format!("/{index}")
            };
            let id = import.object(root, &resource.uri, &path, None, &mut provenance)?;
            import.alias(&resource.uri, &format!("/{index}"), &id)?;
            root_ids.push(id);
        }
        let inferred = resources::records_for_elements(schema, &import.model.elements[before..])?;
        if let Some(record) = record {
            resource_uri(&record.uri)?;
            if record.roots.as_ref() != &root_ids || inferred.keys().any(|uri| uri != &record.uri) {
                return Err(
                    "XMI resource roots or membership disagree with resource metadata".into(),
                );
            }
            if import
                .model
                .resources
                .insert(record.uri.clone(), record)
                .is_some()
            {
                return Err("Resource replaces a retained resource without selection".into());
            }
        } else if !inferred.is_empty() {
            for (uri, mut record) in inferred {
                record.roots = std::sync::Arc::new(root_ids.clone());
                if import.model.resources.insert(uri, record).is_some() {
                    return Err("Duplicate logical XMI resource URI".into());
                }
            }
        } else {
            let record = ResourceRecord::new(resource.uri.clone())?;
            if import
                .model
                .resources
                .insert(resource.uri.clone(), record)
                .is_some()
            {
                return Err("Duplicate logical XMI resource URI".into());
            }
        }
        if !provenance.is_empty() {
            return Err("Provenance refers to missing XMI elements".into());
        }
        if extension_seen && provenance_count != import.model.elements.len() - before {
            return Err("Incomplete XMI provenance".into());
        }
        origins.extend(std::iter::repeat_n(
            resource.uri.clone(),
            import.model.elements.len() - before,
        ));
    }
    import.finish(&origins)
}

fn attribute(out: &mut String, name: &str, value: &str) -> Result<(), String> {
    out.push_str(&format!(" {name}=\"{}\"", xml::escape(value)?));
    Ok(())
}
struct Export<'a, 'services> {
    context: FormatContext<'services>,
    classes: Classes<'a>,
    schema: &'a Schema,
    elements: BTreeMap<&'a str, &'a Element>,
    locations: BTreeMap<&'a str, (String, String)>,
}
impl Export<'_, '_> {
    fn object(
        &self,
        out: &mut String,
        e: &Element,
        tag: Option<&str>,
        depth: usize,
        extension: Option<&str>,
    ) -> Result<(), String> {
        if depth > 120 {
            return Err("XMI containment depth limit (120) exceeded".into());
        }
        let indent = "  ".repeat(depth);
        let name = tag.map_or_else(|| format!("m:{}", e.class), str::to_owned);
        out.push_str(&format!("{indent}<{name}"));
        attribute(out, "xmi:id", &self.locations[&e.id.as_str()].1)?;
        if tag.is_some() {
            attribute(out, "xsi:type", &format!("m:{}", e.class))?;
        }
        let class_features = self.classes.get(&e.class)?;
        let features = &class_features.fields;
        let carried = carried(e, &class_features)?;
        let value_of = |name: &str| carried.get(name).copied().or_else(|| e.values.get(name));
        for (name, feature) in features {
            if feature.derived
                || feature.transient
                || !assigned(e, feature)
                || class_features.redefined.contains_key(name)
            {
                continue;
            }
            if feature.unsettable
                && feature.upper != 1
                && value_of(name)
                    .and_then(Value::as_array)
                    .is_some_and(Vec::is_empty)
            {
                attribute(out, name, "")?;
                continue;
            }
            if feature.reference || feature.upper != 1 {
                continue;
            }
            if let Some(value) = value_of(name).filter(|v| !v.is_null()) {
                attribute(
                    out,
                    name,
                    &scalar_text(self.schema, feature, value, &self.context)?,
                )?;
            }
        }
        out.push_str(">\n");
        for (name, feature) in features {
            if feature.derived
                || feature.transient
                || !assigned(e, feature)
                || class_features.redefined.contains_key(name)
            {
                continue;
            }
            // A container back-reference is reconstructed from containment.
            // Serializing it as a proxy can detach children in EMF loaders.
            if let Some((owner, back)) = feature.opposite.as_deref().and_then(|v| v.split_once('/'))
                && self
                    .classes
                    .get(owner)?
                    .fields
                    .get(back)
                    .is_some_and(|f| f.containment)
            {
                continue;
            }
            let Some(value) = value_of(name) else {
                // encode() already ran the shared persistent validation, which
                // rejects every unsupported absence and validates each selected
                // value's original shape/targets. Preserve that checked raw
                // absence in provenance; do not emit a null or materialized alias.
                if feature.containment || self.context.structural_values.is_none() {
                    return Err("Missing stored XMI value lacks checked absence authority".into());
                }
                continue;
            };
            if feature.upper == 1 && value.is_null() {
                if feature.unsettable || (!feature.reference && !feature.default.is_null()) {
                    out.push_str(&format!("{indent}  <{name} xsi:nil=\"true\"/>\n"));
                }
                continue;
            }
            if !feature.reference && feature.upper == 1 {
                continue;
            }
            for value in values(value, feature.upper)? {
                if feature.containment {
                    let child = self
                        .elements
                        .get(value.as_str().ok_or("Invalid containment identity")?)
                        .ok_or("Missing containment target")?;
                    if child.resource != e.resource {
                        return Err("Cross-resource containment is outside the XMI profile".into());
                    }
                    self.object(out, child, Some(name), depth + 1, None)?;
                } else if feature.reference {
                    let target = self
                        .locations
                        .get(value.as_str().ok_or("Invalid reference")?)
                        .ok_or("Reference target is omitted from persistent XMI projection")?;
                    let local = &self.locations[&e.id.as_str()].0;
                    let href = if local == &target.0 {
                        format!("#{}", target.1)
                    } else {
                        format!("{}#{}", relative_resource(local, &target.0)?, target.1)
                    };
                    out.push_str(&format!("{indent}  <{name}"));
                    attribute(out, "href", &href)?;
                    out.push_str("/>\n");
                } else {
                    let text = scalar_text(self.schema, feature, value, &self.context)?;
                    out.push_str(&format!(
                        "{indent}  <{name}>{}</{name}>\n",
                        xml::escape(&text)?
                    ));
                }
            }
        }
        if let Some(extension) = extension {
            out.push_str(extension);
        }
        out.push_str(&format!("{indent}</{name}>\n"));
        Ok(())
    }
}
fn encode(
    schema: &Schema,
    model: &Model,
    selected: Option<&[String]>,
    context: &FormatContext<'_>,
) -> Result<Vec<Resource>, String> {
    encode_with_rules(schema, model, selected, context, Rules::Shared)
}
fn encode_with_rules(
    schema: &Schema,
    model: &Model,
    selected: Option<&[String]>,
    context: &FormatContext<'_>,
    rules: Rules<'_>,
) -> Result<Vec<Resource>, String> {
    profile(schema, context)?;
    // A frozen model was validated by the frozen engine (`encode_frozen`).
    if matches!(rules, Rules::Shared) {
        context.validate(schema, model)?;
    }
    if model.schema_revision != schema.revision {
        return Err("Bind even an empty model to its schema before XMI export".into());
    }
    let resources: BTreeSet<_> = model.resources.keys().map(String::as_str).collect();
    let mut names = BTreeMap::new();
    for (i, uri) in resources.into_iter().enumerate() {
        resource_uri(uri)?;
        names.insert(
            uri,
            if selected.is_some() {
                uri.to_owned()
            } else {
                format!("resource-{i:04}.xmi")
            },
        );
    }
    if names.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(selected) = selected {
        let unique: BTreeSet<_> = selected.iter().collect();
        if unique.len() != selected.len()
            || selected
                .iter()
                .any(|uri| !model.resources.contains_key(uri))
        {
            return Err("Invalid or duplicate XMI resource selection".into());
        }
    }
    let mut export = Export {
        context: *context,
        classes: Classes::new(schema, rules),
        schema,
        elements: BTreeMap::new(),
        locations: BTreeMap::new(),
    };
    let mut contained = BTreeSet::new();
    for (i, e) in model.elements.iter().enumerate() {
        let class_features = export.classes.get(&e.class)?;
        let fields = &class_features.fields;
        carried(e, &class_features)?;
        if fields.iter().any(|(name, feature)| {
            !feature.derived
                && !class_features.redefined.contains_key(name)
                && !e.values.contains_key(name)
                && (context.structural_values.is_none() || feature.containment)
        }) || fields
            .iter()
            .any(|(name, feature)| feature.derived && e.values.contains_key(name))
        {
            return Err(format!(
                "XMI export requires all stored features and no derived entries: {}",
                e.id
            ));
        }
        if e.start > e.end {
            return Err("Invalid model source span".into());
        }
        export.elements.insert(e.id.as_str(), e);
        export.locations.insert(
            e.id.as_str(),
            (
                names[e.resource.as_str()].clone(),
                if selected.is_some() {
                    stable_fragment(&e.id)
                } else {
                    format!("e{i}")
                },
            ),
        );
        for (name, feature) in fields {
            if feature.containment && !feature.derived {
                for id in values(e.values.get(name).unwrap_or(&Value::Null), feature.upper)? {
                    contained.insert(id.as_str().ok_or("Invalid containment")?.to_owned());
                }
            }
        }
    }
    // Runtime ownership determines roots, including transient containment. Only
    // persistent containment is traversed: omitted children must not become roots.
    let mut serialized = BTreeSet::new();
    let mut pending: Vec<&str> = model
        .elements
        .iter()
        .filter(|e| !contained.contains(&e.id))
        .map(|e| e.id.as_str())
        .collect();
    while let Some(id) = pending.pop() {
        if !serialized.insert(id) {
            continue;
        }
        let element = export.elements.get(id).ok_or("Missing projection target")?;
        for (name, feature) in &export.classes.get(&element.class)?.fields {
            if feature.containment
                && !feature.derived
                && !feature.transient
                && assigned(element, feature)
            {
                for value in values(
                    element.values.get(name).unwrap_or(&Value::Null),
                    feature.upper,
                )? {
                    pending.push(value.as_str().ok_or("Invalid containment identity")?);
                }
            }
        }
    }
    export.locations.retain(|id, _| serialized.contains(id));
    let mut result = Vec::new();
    for (resource, uri) in names {
        if selected.is_some_and(|selected| !selected.iter().any(|selected| selected == resource)) {
            continue;
        }
        let mut out = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xmi:XMI xmi:version=\"2.0\" xmlns:xmi=\"{XMI}\" xmlns:xsi=\"{XSI}\" xmlns:ix=\"{IX}\""
        );
        attribute(&mut out, "xmlns:m", &schema.uri)?;
        out.push_str(">\n");
        let mut extension = String::new();
        extension.push_str(&format!(
            "  <xmi:Extension extender=\"{IX}\">\n    <ix:model"
        ));
        attribute(&mut extension, "schemaRevision", &schema.revision)?;
        if let Some(record) = model.resources.get(resource) {
            attribute(
                &mut extension,
                "resourceRecord",
                &serde_json::to_string(record).map_err(|error| error.to_string())?,
            )?;
        }
        extension.push_str(">\n");
        for (i, e) in model
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.resource == resource && serialized.contains(e.id.as_str()))
        {
            extension.push_str("      <ix:element");
            for (name, value) in [
                ("xmiId", export.locations[&e.id.as_str()].1.clone()),
                ("id", e.id.clone()),
                ("resource", e.resource.clone()),
                ("start", e.start.to_string()),
                ("end", e.end.to_string()),
                ("order", i.to_string()),
            ] {
                attribute(&mut extension, name, &value)?;
            }
            if e.implicit {
                attribute(&mut extension, "implicit", "true")?;
            }
            let class_features = export.classes.get(&e.class)?;
            let absent: Vec<_> = class_features
                .fields
                .iter()
                .filter(|(name, field)| {
                    !field.derived
                        && !e.values.contains_key(*name)
                        && !class_features.redefined.contains_key(*name)
                })
                .map(|(name, _)| name)
                .collect();
            if !absent.is_empty() {
                attribute(
                    &mut extension,
                    "absentStored",
                    &serde_json::to_string(&absent).map_err(|error| error.to_string())?,
                )?;
            }
            if let Some(receipt) = &e.initialization {
                attribute(
                    &mut extension,
                    "initialization",
                    &serde_json::to_string(receipt).map_err(|error| error.to_string())?,
                )?;
            }
            extension.push_str("/>\n");
        }
        extension.push_str("    </ix:model>\n  </xmi:Extension>\n");
        // Resource provenance belongs to the wrapper, including empty resources.
        out.push_str(&extension);
        if let Some(record) = model.resources.get(resource) {
            for id in record.roots.iter() {
                let element = export
                    .elements
                    .get(id.as_str())
                    .ok_or("Missing resource root")?;
                export.object(&mut out, element, None, 1, None)?;
            }
        }
        out.push_str("</xmi:XMI>\n");
        result.push(Resource {
            uri,
            bytes: out.into_bytes(),
        });
    }
    Ok(result)
}

// Stable, injective ASCII NCName mapping. Runtime handles and insertion order
// never participate in persisted external fragments.
fn stable_fragment(identity: &str) -> String {
    let mut result = String::from("i");
    for byte in identity.bytes() {
        result.push_str(&format!("{byte:02x}"));
    }
    result
}
fn relative_resource(base: &str, target: &str) -> Result<String, String> {
    use iri_string::types::IriReferenceStr;
    let target_parts = IriReferenceStr::new(target)
        .map_err(|error| format!("Invalid XMI target resource URI: {error}"))?;
    let reference =
        if target_parts.scheme_str().is_some() || target_parts.path_str().starts_with('/') {
            target.to_owned()
        } else {
            let base_parts = IriReferenceStr::new(base)
                .map_err(|error| format!("Invalid XMI source resource URI: {error}"))?;
            let mut from: Vec<_> = base_parts.path_str().split('/').collect();
            from.pop();
            let mut to: Vec<_> = target_parts.path_str().split('/').collect();
            let filename = to.pop().ok_or("Missing target resource path")?;
            // Only directory components can be shared. A final target filename
            // matching a source directory still needs its own path segment.
            let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
            let mut parts = vec![".."; from.len() - common];
            parts.extend_from_slice(&to[common..]);
            parts.push(filename);
            let mut reference = parts.join("/");
            if IriReferenceStr::new(&reference)
                .map_err(|error| format!("Invalid relative XMI resource URI: {error}"))?
                .scheme_str()
                .is_some()
            {
                reference.insert_str(0, "./");
            }
            reference
        };
    // Every branch must preserve the exact logical identity. Some combinations
    // of absolute sources and relative targets require an explicit host mapping.
    if resolve_resource(base, &reference)? != target {
        return Err("Cannot represent logical URI as an XMI reference".into());
    }
    Ok(reference)
}
