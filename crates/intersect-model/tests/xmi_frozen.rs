#![cfg(feature = "xmi")]
//! Handwritten tests (not generated): XMI of frozen models whose classes
//! redefine lower-bound features (MOF property redefinition), as SysML's
//! `OwningMembership` redefines `Membership.memberElement`.
use intersect_model::{
    core::{
        frozen::{ById, Metamodel, Model, ModelBuilder},
        interchange::{ModelFormat, Resource},
    },
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        compiler::compile_structure_bundle,
        ecore::ParseLimits,
        rcore::feature_redefinitions,
        schema::Schema,
    },
    xmi::XmiFormat,
};
use std::sync::Arc;

// Classifier 1 is Relation; its features 0 and 1 are general and specific,
// both [1,1], which Typing redefines by its own type and typed; its
// attribute label (feature 2) Typing redefines by its stored tag, as
// RequirementDefinition.reqId redefines declaredShortName.
const SOURCE: &str = r##"rcore 1;
package example.xmiFrozen version "1";
metamodel "urn:xmi-frozen" prefix m {
  class Owner { attr name: String [0,1]; ref parts: Relation [0,*] containment; }
  class Relation {
    ref general: Owner [1,1];
    ref specific: Owner [1,1];
    attr label: String [0,1];
  }
  class Typing extends Relation {
    ref type: Owner [1,1] annotation "redefines" { reference "#//@eClassifiers.1/@eStructuralFeatures.0"; };
    ref typed: Owner [1,1] annotation "redefines" { reference "#//@eClassifiers.1/@eStructuralFeatures.1"; };
    attr tag: String [0,1] annotation "redefines" { reference "#//@eClassifiers.1/@eStructuralFeatures.2"; };
  }
}
"##;
const USER: &str = "memory:/user/model.xmi";
const LIBRARY: &str = "memory:/library/base.xmi";

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn metamodel() -> Result<Arc<Metamodel>, String> {
    let source = DefinitionSource::new("memory:xmi-frozen.rcore", SourceKind::Rcore, SOURCE);
    let entry = source.id.clone();
    let redefinitions = feature_redefinitions(&source, ParseLimits::default()).map_err(err)?;
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let schema = compile_structure_bundle(&bundle, &entry, "urn:xmi-frozen").map_err(err)?;
    let metamodel = Metamodel::freeze(&schema)
        .and_then(|mm| mm.with_redefinitions(&redefinitions))
        .map_err(err)?;
    Ok(Arc::new(metamodel))
}

fn class_key(schema: &Schema, name: &str) -> Result<String, String> {
    schema
        .declarations
        .iter()
        .find(|(k, d)| d.name == name && schema.classes.contains_key(*k))
        .map(|(k, _)| k.clone())
        .ok_or_else(|| format!("Missing class {name}"))
}

/// A library resource with one owner, `base`, and (with `user`) a user
/// resource whose owner `a` contains a typing of `a` by the library's `base`
/// and one of `a` by itself; `general` also sets the redefined feature.
fn model(mm: &Arc<Metamodel>, user: bool, general: bool) -> Result<Model, String> {
    let schema = mm.schema();
    let class = |name| {
        mm.class(&class_key(schema, name)?)
            .ok_or(format!("Missing class {name}"))
    };
    let (owner, relation, typing) = (class("Owner")?, class("Relation")?, class("Typing")?);
    let feature = |c, name: &str| mm.feature(c, name).ok_or(format!("feature {name}"));
    let mut b = ModelBuilder::new(mm.clone());
    let base = b.create("base", owner, LIBRARY).map_err(err)?;
    b.set_str(base, feature(owner, "name")?, "Base")
        .map_err(err)?;
    if user {
        let a = b.create("a", owner, USER).map_err(err)?;
        let parts = feature(owner, "parts")?;
        for (id, target) in [("t1", base), ("t2", a)] {
            let t = b.create(id, typing, USER).map_err(err)?;
            b.add_ref(a, parts, t).map_err(err)?;
            b.add_ref(t, feature(typing, "type")?, target)
                .map_err(err)?;
            b.add_ref(t, feature(typing, "typed")?, a).map_err(err)?;
            if general {
                b.add_ref(t, feature(relation, "general")?, target)
                    .map_err(err)?;
            }
        }
    }
    b.link(&ById).map_err(err)?.validate().map_err(err)
}

fn text(resources: &[Resource], uri: &str) -> Result<String, String> {
    let resource = resources
        .iter()
        .find(|r| r.uri == uri)
        .ok_or(format!("no {uri}"))?;
    String::from_utf8(resource.bytes.clone()).map_err(err)
}

#[test]
fn redefined_lower_bound_features_export_and_round_trip() -> Result<(), String> {
    let mm = metamodel()?;
    let model = model(&mm, true, false)?;
    let materialized = model.to_model().map_err(err)?;

    // The shared validation knows no redefinitions: the frozen model fails it.
    let shared = XmiFormat.encode_selected(mm.schema(), &materialized, &[USER.to_owned()]);
    match shared {
        Err(error) => assert!(error.contains("Multiplicity violation"), "{error}"),
        Ok(_) => return Err("the shared path accepted empty redefined features".into()),
    }

    let selected = [USER.to_owned()];
    let resources = XmiFormat.encode_frozen(&model, Some(&selected))?;
    assert_eq!(resources.len(), 1);
    let xmi = text(&resources, USER)?;
    // As EMF writes it: the redefining features, never the redefined ones,
    // and a reference into the library as an href to its resource.
    assert!(xmi.contains(&format!("<type href=\"{LIBRARY}#")), "{xmi}");
    assert!(xmi.contains("<typed href=\"#"), "{xmi}");
    assert!(
        !xmi.contains("<general") && !xmi.contains("<specific"),
        "{xmi}"
    );

    let library = self::model(&mm, false, false)?;
    let decoded = XmiFormat.decode_frozen(&mm, &resources, Some(&library))?;
    assert_eq!(decoded.to_model().map_err(err)?, materialized);

    // Every resource at once, without a retained library.
    let all = XmiFormat.encode_frozen(&model, None)?;
    assert_eq!(all.len(), 2);
    let decoded = XmiFormat.decode_frozen(&mm, &all, None)?;
    let mut again = decoded.to_model().map_err(err)?;
    // Unselected export names resources resource-NNNN.xmi; the identities and
    // resource records come back from the provenance extension.
    again.elements.sort_by(|a, b| a.id.cmp(&b.id));
    let mut expected = materialized.clone();
    expected.elements.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(again, expected);
    Ok(())
}

#[test]
fn a_value_in_a_redefined_feature_is_an_explicit_error() -> Result<(), String> {
    let mm = metamodel()?;
    let selected = [USER.to_owned()];
    // Export: XMI cannot carry it (EMF never writes a redefined feature).
    let model = model(&mm, true, true)?;
    match XmiFormat.encode_frozen(&model, Some(&selected)) {
        Err(error) => assert!(
            error.contains("general") && error.contains("redefined by type"),
            "{error}"
        ),
        Ok(_) => return Err("a redefined feature's value was dropped".into()),
    }
    // Import: a document that writes the redefined feature is refused.
    let model = self::model(&mm, true, false)?;
    let mut resources = XmiFormat.encode_frozen(&model, Some(&selected))?;
    let xmi = text(&resources, USER)?.replacen("<typed href", "<specific href", 1);
    resources[0].bytes = xmi.into_bytes();
    let library = self::model(&mm, false, false)?;
    match XmiFormat.decode_frozen(&mm, &resources, Some(&library)) {
        Err(error) => assert!(
            error.contains("specific") && error.contains("redefined by typed"),
            "{error}"
        ),
        Ok(_) => return Err("a redefined feature was imported".into()),
    }
    Ok(())
}

/// A value in a redefined attribute's own slot (as `FrozenStore::set_value`
/// leaves it) is written under the stored redefining attribute, as EMF
/// writes `reqId` for a requirement definition's short name; it comes back
/// in the redefining attribute's slot, where frozen reads of either look.
#[test]
fn a_redefined_attribute_value_is_written_as_the_redefining_attribute() -> Result<(), String> {
    use intersect_model::core::{
        frozen::FrozenStore,
        storage::{self, ModelStore},
    };
    let mm = metamodel()?;
    let selected = [USER.to_owned()];
    let mut store = FrozenStore::from_model(model(&mm, true, false)?);
    storage::atomic(&mut store, |s| s.set_value("t1", "label", Some("L".into())))?;
    let model = store
        .into_builder()
        .map_err(err)?
        .link(&ById)
        .map_err(err)?
        .validate()
        .map_err(err)?;
    let t1 = |m: &Model| -> Result<_, String> {
        let element = m
            .to_model()
            .map_err(err)?
            .elements
            .into_iter()
            .find(|e| e.id == "t1")
            .ok_or("t1")?;
        Ok((
            element.values.get("label").cloned(),
            element.values.get("tag").cloned(),
        ))
    };
    assert_eq!(
        t1(&model)?,
        (Some("L".into()), Some(serde_json::Value::Null))
    );
    let resources = XmiFormat.encode_frozen(&model, Some(&selected))?;
    let xmi = text(&resources, USER)?;
    assert!(xmi.contains(" tag=\"L\""), "{xmi}");
    assert!(!xmi.contains(" label="), "{xmi}");
    let library = self::model(&mm, false, false)?;
    let decoded = XmiFormat.decode_frozen(&mm, &resources, Some(&library))?;
    assert_eq!(t1(&decoded)?, (None, Some("L".into())));
    let relation = mm
        .class(&class_key(mm.schema(), "Relation")?)
        .ok_or("Relation")?;
    let label = mm.feature(relation, "label").ok_or("label")?;
    let read = decoded.lookup("t1").ok_or("t1")?.str(label);
    assert_eq!(read, Some("L"));
    Ok(())
}
