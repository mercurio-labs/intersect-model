//! MOF property redefinition and Rcore `bind default` on the frozen engine.
//! The structure declares redefinitions with the same `redefines` annotations
//! that Ecore migration emits; `bind default` fills an empty reference.
use intersect_model::{
    core::frozen::{ById, Metamodel, ModelBuilder, Violation},
    definitions::{
        bundle::{DefinitionBundle, DefinitionSource, SourceKind},
        compiler::compile_structure_bundle,
        ecore::ParseLimits,
        rcore::feature_redefinitions,
        rcore_behavior::ir_v2::Limits,
        rcore_scope::{compile_default_with_source_limits, source_default_bindings},
        schema::Schema,
    },
    frozen_scopes::RcoreScopes,
};
use std::sync::Arc;

// Classifier 1 is Relation; its features 0 and 1 are general and specific.
const SOURCE: &str = r##"rcore 1;
package example.frozenDefaults version "1";
metamodel "urn:frozen-defaults" prefix m {
  class Owner { ref parts: Relation [0,*] containment; }
  class Relation {
    ref general: Owner [1,1];
    ref specific: Owner [1,1];
  }
  class Typing extends Relation {
    ref type: Owner [1,1] annotation "redefines" { reference "#//@eClassifiers.1/@eStructuralFeatures.0"; };
    ref typed: Owner [1,1] annotation "redefines" { reference "#//@eClassifiers.1/@eStructuralFeatures.1"; };
  }
}
behavior {
  import model "urn:frozen-defaults" as m;
  bind default m.Typing.typed = owner_of;
  fn owner_of(t:m.Typing)->Sequence<m.Owner> =
    match first(ancestors::<m.Owner>(t)) { Some(o) => Sequence([o]), None => [] };
}
"##;

fn err(e: impl std::fmt::Debug) -> String {
    format!("{e:?}")
}

fn setup() -> Result<(DefinitionBundle, Schema, Arc<Metamodel>), String> {
    let source = DefinitionSource::new("memory:frozen-defaults.rcore", SourceKind::Rcore, SOURCE);
    let entry = source.id.clone();
    let redefinitions = feature_redefinitions(&source, ParseLimits::default()).map_err(err)?;
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let schema = compile_structure_bundle(&bundle, &entry, "urn:frozen-defaults").map_err(err)?;
    let mm = Metamodel::freeze(&schema)
        .and_then(|mm| mm.with_redefinitions(&redefinitions))
        .map_err(err)?;
    Ok((bundle, schema, Arc::new(mm)))
}
fn class_key(schema: &Schema, name: &str) -> Result<String, String> {
    schema
        .declarations
        .iter()
        .find(|(k, d)| d.name == name && schema.classes.contains_key(*k))
        .map(|(k, _)| k.clone())
        .ok_or_else(|| format!("Missing class {name}"))
}

#[test]
fn redefines_annotations_name_the_schema_features() -> Result<(), String> {
    let (_, schema, mm) = setup()?;
    let class = |name| {
        mm.class(&class_key(&schema, name)?)
            .ok_or("class".to_owned())
    };
    let (relation, typing) = (class("Relation")?, class("Typing")?);
    let feature = |c, name: &str| mm.feature(c, name).ok_or(format!("feature {name}"));
    let general = feature(relation, "general")?;
    let specific = feature(relation, "specific")?;
    assert_eq!(
        mm.redefined_by(typing, general),
        Some(feature(typing, "type")?)
    );
    assert_eq!(
        mm.redefined_by(typing, specific),
        Some(feature(typing, "typed")?)
    );
    // Redefinition applies only where the redefining feature exists.
    assert_eq!(mm.redefined_by(relation, general), None);
    Ok(())
}

#[test]
fn defaults_fill_empty_references_and_redefined_features_read_through() -> Result<(), String> {
    let (bundle, schema, mm) = setup()?;
    let entry = bundle.sources[0].id.clone();
    let defaults = source_default_bindings(&bundle.sources[0], ParseLimits::default())
        .map_err(err)?
        .iter()
        .map(|b| {
            compile_default_with_source_limits(
                &bundle,
                &entry,
                b.declaration,
                &Limits::default(),
                ParseLimits::default(),
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;
    assert_eq!(defaults.len(), 1);
    assert!(defaults[0].is_default());
    let scopes = RcoreScopes::new(&mm, &bundle, &defaults).map_err(err)?;

    let class = |name| {
        mm.class(&class_key(&schema, name)?)
            .ok_or("class".to_owned())
    };
    let (owner, relation, typing) = (class("Owner")?, class("Relation")?, class("Typing")?);
    let feature = |c, name: &str| mm.feature(c, name).ok_or(format!("feature {name}"));
    let parts = feature(owner, "parts")?;
    let mut b = ModelBuilder::new(mm.clone());
    let a = b.create("a", owner, "memory:r").map_err(err)?;
    let t = b.create("t", typing, "memory:r").map_err(err)?;
    let target = b.create("target", owner, "memory:r").map_err(err)?;
    b.add_ref(a, parts, t).map_err(err)?;
    b.add_ref(t, feature(typing, "type")?, target)
        .map_err(err)?;

    let linked = b.link(&ById).map_err(err)?;
    let model = linked
        .complete_defaults(&scopes)
        .map_err(err)?
        .validate()
        .map_err(err)?;
    let t = model.lookup("t").ok_or("t")?;
    let ids = |feature| t.refs(feature).map(|o| o.identity()).collect::<Vec<_>>();
    assert_eq!(ids(feature(typing, "typed")?), vec!["a"]);
    // The redefined Relation features read as the redefining Typing features.
    assert_eq!(ids(feature(relation, "specific")?), vec!["a"]);
    assert_eq!(ids(feature(relation, "general")?), vec!["target"]);
    // Export omits the empty redefined slots.
    let exported = model.to_model().map_err(err)?;
    let element = exported
        .elements
        .iter()
        .find(|e| e.id == "t")
        .ok_or("exported t")?;
    assert!(!element.values.contains_key("general"));
    assert!(!element.values.contains_key("specific"));
    assert!(element.values.contains_key("typed"));
    Ok(())
}

#[test]
fn a_plain_relation_still_needs_its_own_values() -> Result<(), String> {
    let (_, schema, mm) = setup()?;
    let relation = mm
        .class(&class_key(&schema, "Relation")?)
        .ok_or("Relation")?;
    let mut b = ModelBuilder::new(mm.clone());
    b.create("r", relation, "memory:r").map_err(err)?;
    let failures = match b.link(&ById).map_err(err)?.validate() {
        Ok(_) => return Err("an empty Relation validated".into()),
        Err(diagnostics) => diagnostics.0,
    };
    let missing: Vec<_> = failures
        .iter()
        .filter_map(|v| match v {
            Violation::Multiplicity { feature, .. } => Some(feature.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(missing, vec!["general", "specific"]);
    Ok(())
}

const ATTRIBUTES: &str = r##"rcore 1;
package example.frozenAttributeDefaults version "1";
metamodel "urn:frozen-attribute-defaults" prefix m {
  enum Mode { Day = 1; Night = 2; }
  class Item { attr id: String [1,1]; attr mode: Mode [0,1]; }
}
behavior {
  import model "urn:frozen-attribute-defaults" as m;
  bind default m.Item.id = item_id;
  bind default m.Item.mode = no_mode;
  fn item_id(i:m.Item)->Sequence<String> = Sequence([resource(i) + ":" + hex(identity(i))]);
  fn no_mode(i:m.Item)->Sequence<m.Mode> = [];
}
"##;

/// Attribute defaults computed with `resource`, `identity` and `hex`; an
/// empty default replaces an unassigned schema default with null, but not an
/// explicit assignment of the same value.
#[test]
fn attribute_defaults_replace_only_unassigned_values() -> Result<(), String> {
    use intersect_model::core::{
        frozen::FrozenStore,
        storage::{self, ModelRead, ModelStore},
    };
    let source = DefinitionSource::new(
        "memory:frozen-attribute-defaults.rcore",
        SourceKind::Rcore,
        ATTRIBUTES,
    );
    let entry = source.id.clone();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let schema =
        compile_structure_bundle(&bundle, &entry, "urn:frozen-attribute-defaults").map_err(err)?;
    let defaults = source_default_bindings(&bundle.sources[0], ParseLimits::default())
        .map_err(err)?
        .iter()
        .map(|b| {
            compile_default_with_source_limits(
                &bundle,
                &entry,
                b.declaration,
                &Limits::default(),
                ParseLimits::default(),
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;
    assert_eq!(defaults.len(), 2);
    let mm = Arc::new(Metamodel::freeze(&schema).map_err(err)?);
    let scopes = RcoreScopes::new(&mm, &bundle, &defaults).map_err(err)?;

    let item = class_key(&schema, "Item")?;
    let mut store = FrozenStore::new(mm.clone());
    storage::atomic(&mut store, |s| {
        storage::bind_schema(s, &schema)?;
        s.create_element(&schema, "a", &item, "memory:r", 0, 0)?;
        s.create_element(&schema, "b", &item, "memory:r", 0, 0)?;
        // An explicit assignment of the schema default (Day).
        let day = s.value("b", "mode")?.ok_or("default mode")?;
        s.set_value("b", "mode", Some(day))
    })?;
    let day = store.value("b", "mode")?.ok_or("mode")?;
    let model = store
        .into_builder()
        .map_err(err)?
        .link(&ById)
        .map_err(err)?
        .complete_defaults(&scopes)
        .map_err(err)?
        .validate()
        .map_err(err)?
        .to_model()
        .map_err(err)?;
    let values = |id: &str| {
        model
            .elements
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.values.clone())
            .ok_or(format!("exported {id}"))
    };
    let a = values("a")?;
    assert_eq!(a.get("id"), Some(&serde_json::json!("memory:r:61")));
    assert_eq!(a.get("mode"), Some(&serde_json::Value::Null));
    let b = values("b")?;
    assert_eq!(b.get("id"), Some(&serde_json::json!("memory:r:62")));
    assert_eq!(b.get("mode"), Some(&day));
    Ok(())
}

const ENUM_DEFAULT: &str = r##"rcore 1;
package example.frozenEnumDefaults version "1";
metamodel "urn:frozen-enum-defaults" prefix m {
  enum Mode { Day = 1; Night = 2; }
  class Item { attr mode: Mode [0,1]; }
}
behavior {
  import model "urn:frozen-enum-defaults" as m;
  bind default m.Item.mode = night;
  fn night(i:m.Item)->Sequence<m.Mode> = Sequence([m.Mode.Night]);
}
"##;

/// An enumeration default runs on the direct executor and stores the literal
/// in the form the parser stores it; an explicit assignment stays.
#[test]
fn enumeration_defaults_store_the_literal() -> Result<(), String> {
    use intersect_model::core::{
        frozen::FrozenStore,
        storage::{self, ModelRead, ModelStore},
    };
    let source = DefinitionSource::new(
        "memory:frozen-enum-defaults.rcore",
        SourceKind::Rcore,
        ENUM_DEFAULT,
    );
    let entry = source.id.clone();
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source],
        namespaces: vec![],
    };
    let schema =
        compile_structure_bundle(&bundle, &entry, "urn:frozen-enum-defaults").map_err(err)?;
    let defaults = source_default_bindings(&bundle.sources[0], ParseLimits::default())
        .map_err(err)?
        .iter()
        .map(|b| {
            compile_default_with_source_limits(
                &bundle,
                &entry,
                b.declaration,
                &Limits::default(),
                ParseLimits::default(),
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;
    let mm = Arc::new(Metamodel::freeze(&schema).map_err(err)?);
    let scopes = RcoreScopes::new(&mm, &bundle, &defaults).map_err(err)?;
    let enumeration = schema.enumerations.values().next().ok_or("Mode")?;
    let night = serde_json::to_value(enumeration.parse("Night").map_err(err)?).map_err(err)?;

    let item = class_key(&schema, "Item")?;
    let mut store = FrozenStore::new(mm.clone());
    storage::atomic(&mut store, |s| {
        storage::bind_schema(s, &schema)?;
        s.create_element(&schema, "a", &item, "memory:r", 0, 0)?;
        s.create_element(&schema, "b", &item, "memory:r", 0, 0)?;
        let day = s.value("b", "mode")?.ok_or("default mode")?;
        s.set_value("b", "mode", Some(day))
    })?;
    let day = store.value("b", "mode")?.ok_or("mode")?;
    let model = store
        .into_builder()
        .map_err(err)?
        .link(&ById)
        .map_err(err)?
        .complete_defaults(&scopes)
        .map_err(err)?
        .validate()
        .map_err(err)?
        .to_model()
        .map_err(err)?;
    let mode = |id: &str| {
        model
            .elements
            .iter()
            .find(|e| e.id == id)
            .and_then(|e| e.values.get("mode").cloned())
            .ok_or(format!("exported {id}"))
    };
    assert_eq!(mode("a")?, night);
    assert_eq!(mode("b")?, day);
    Ok(())
}
