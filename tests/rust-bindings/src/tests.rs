use crate::{
    library::{self, BookCopy, Library, Loan, Member, Named},
    shapes::{self, Base, Thing},
};
use intersect_model::core::{
    api::{BasicModel, ModelEdit, TransactionalModel, VersionedModel},
    query::QueryBudget,
    storage::{self, ModelRead},
};
use serde_json::json;

fn populate(edit: &mut impl ModelEdit) -> Result<(Library, BookCopy), String> {
    let catalog = Library::create(edit, "catalog", "catalog.library")?;
    catalog.set_name(edit, "Catalog")?;
    let copy = BookCopy::create(edit, "dune", "catalog.library")?;
    copy.set_name(edit, "dune")?;
    copy.set_title(edit, Some("Dune"))?;
    catalog.add_copies(edit, &copy)?;
    Ok((catalog, copy))
}

#[test]
fn preserves_exact_import_and_generated_source() -> Result<(), String> {
    let source = include_str!("../../../fixtures/library/library-bindings.rcore");
    assert_eq!(library::RCORE_SOURCE, Some(source));
    assert_eq!(library::schema()?, rcore_schema(source)?);
    assert_eq!(library::dynamic_schema()?.as_schema(), &library::schema()?);
    assert_eq!(
        intersect_model::codegen::generate_rcore(source)?,
        include_str!(concat!(env!("OUT_DIR"), "/library.rs"))
    );
    let mut model = BasicModel::new(library::schema()?)?;
    let (catalog, copy) = populate(&mut model)?;
    model.validate()?;
    assert_eq!(copy.edition(&model)?, Some(1));
    assert_eq!(copy.as_named().name(&model)?, "dune");
    assert_eq!(copy.library(&model)?, Some(catalog.clone()));
    assert_eq!(catalog.copies(&model)?, vec![copy.clone()]);
    assert!(Member::bind(&model, copy.id()).is_err());
    assert_eq!(Named::bind(&model, copy.id())?, copy.as_named());

    let mut reflected = BasicModel::new(rcore_schema(source)?)?;
    reflected.create("catalog", "Library", "catalog.library")?;
    reflected.set("catalog", "name", json!("Catalog"))?;
    reflected.create("dune", "BookCopy", "catalog.library")?;
    reflected.set("dune", "name", json!("dune"))?;
    reflected.set("dune", "title", json!("Dune"))?;
    reflected.set("catalog", "copies", json!(["dune"]))?;
    assert_eq!(storage::export(&model)?, storage::export(&reflected)?);
    Ok(())
}

#[test]
fn checked_transactions_and_domain_rejection_roll_back() -> Result<(), String> {
    let mut model = TransactionalModel::new(BasicModel::new(library::schema()?)?);
    let (catalog, copy) = model.transaction(|edit| populate(edit))?;
    let before = storage::export(&model)?;
    assert!(
        model
            .transaction(|edit| {
                copy.set_title(edit, Some("Uncommitted"))?;
                // Reflective replacement rejects duplicate containment.
                catalog.set_copies(edit, &[copy.clone(), copy.clone()])
            })
            .is_err()
    );
    assert_eq!(storage::export(&model)?, before);
    model.transaction(|edit| {
        catalog.add_copies(edit, &copy)?; // Duplicate collection addition is a no-op.
        copy.set_title(edit, Some("Accepted"))
    })?;
    assert_eq!(copy.title(&model)?, Some("Accepted".into()));
    assert!(
        model
            .transaction(|edit| {
                let missing = BookCopy::create(edit, "incomplete", "catalog.library")?;
                assert!(missing.name(edit).is_err());
                Ok(())
            })
            .is_err()
    );
    assert!(BookCopy::bind(&model, "incomplete").is_err());
    assert!(
        model
            .transaction_with(
                |edit| copy.set_title(edit, Some("Rejected")),
                |_| Err("Domain rejection".into()),
            )
            .is_err()
    );
    assert_eq!(copy.title(&model)?, Some("Accepted".into()));
    model.transaction(|edit| {
        let other = Library::create(edit, "other", "other.library")?;
        other.set_name(edit, "Other")?;
        catalog.clear_copies(edit)?;
        other.add_copies(edit, &copy)?;
        assert_eq!(copy.library(edit)?, Some(other));
        Ok(())
    })?;
    assert!(catalog.copies(&model)?.is_empty());
    Ok(())
}

#[test]
fn programmatic_ir_and_escaped_defaults_compile_and_round_trip() -> Result<(), String> {
    use crate::programmatic::{self, record};
    assert_eq!(programmatic::RCORE_SOURCE, None);
    assert!(programmatic::SCHEMA_REVISION.starts_with("dynamic-v1:"));
    let mut model = BasicModel::new(programmatic::schema()?)?;
    let object = record::create(&mut model, "record", "records")?;
    assert_eq!(
        object.message(&model)?,
        Some("quote\"\n\\λ\0\u{2028}".into())
    );
    object.set_async_(&mut model, Some(true))?;
    assert_eq!(object.async_(&model)?, Some(true));
    Ok(())
}

#[test]
fn rejects_foreign_revision_and_missing_or_wrong_class_identities() -> Result<(), String> {
    let mut model = BasicModel::new(library::schema()?)?;
    let (_, copy) = populate(&mut model)?;
    let changed = format!("{}\n", library::RCORE_SOURCE.ok_or("Missing source")?);
    let mut foreign = BasicModel::new(rcore_schema(&changed)?)?;
    assert!(BookCopy::create(&mut foreign, "copy", "catalog").is_err());
    assert!(BookCopy::bind(&foreign, "dune").is_err());
    assert!(
        copy.set_title(&mut foreign, Some("wrong revision"))
            .is_err()
    );
    assert!(BookCopy::field_title(foreign.schema()).is_err());
    let mut tampered = library::schema()?;
    tampered
        .classes
        .get_mut("BookCopy")
        .ok_or("Missing class")?
        .features
        .remove("title");
    assert!(BookCopy::query_binding(&tampered).is_err());
    assert!(BookCopy::bind(&model, "missing").is_err());
    assert!(Member::bind(&model, "dune").is_err());
    // External identities deliberately resolve in the explicitly supplied model.
    let mut alternate = BasicModel::new(library::schema()?)?;
    let member = Member::create(&mut alternate, "dune", "catalog")?;
    member.set_name(&mut alternate, "Different class")?;
    assert!(copy.title(&alternate).is_err());
    let mut empty = BasicModel::new(library::schema()?)?;
    let loan = Loan::create(&mut empty, "loan", "catalog")?;
    loan.set_name(&mut empty, "loan")?;
    assert!(loan.set_copy(&mut empty, &copy).is_err());
    assert_eq!(empty.value("loan", "copy")?, Some(json!(null)));
    Ok(())
}

#[test]
fn scalar_collections_optional_references_and_inherited_conversions() -> Result<(), String> {
    let mut model = BasicModel::new(shapes::schema()?)?;
    let object = Thing::create(&mut model, "thing", "shapes")?;
    assert!(!object.ready(&model)?);
    object.set_ready(&mut model, true)?;
    object.set_number(&mut model, i32::MAX)?;
    object.set_type_(&mut model, Some("quoted \"text\"\nλ"))?;
    assert_eq!(
        object.as_base().type_(&model)?,
        Some("quoted \"text\"\nλ".into())
    );
    assert_eq!(object.as_middle().as_base(), object.as_base());
    object.set_url_value(&mut model, Some("urn:test"))?;
    object.add_tags(&mut model, "first")?;
    object.add_tags(&mut model, "second")?;
    assert!(object.add_tags(&mut model, "third").is_err());
    object.add_tags(&mut model, "first")?; // Existing unique item is a no-op, even at capacity.
    assert_eq!(object.tags(&model)?, vec!["first", "second"]);
    object.set_tags(&mut model, &["replacement".into()])?;
    object.clear_tags(&mut model)?;
    object.set_scores(&mut model, &[1, 1, i32::MIN])?;
    object.add_scores(&mut model, 2)?;
    assert_eq!(object.scores(&model)?, vec![1, 1, i32::MIN, 2]);
    object.set_switches(&mut model, &[false])?;
    object.add_switches(&mut model, true)?;
    assert_eq!(object.switches(&model)?, vec![false, true]);
    assert_eq!(object.peer(&model)?, None);
    object.set_peer(&mut model, Some(&object.as_base()))?;
    assert_eq!(object.peer(&model)?, Some(Base::bind(&model, "thing")?));
    object.clear_peer(&mut model)?;
    object.set_peers(&mut model, &[object.as_base()])?;
    assert_eq!(object.peers(&model)?, vec![object.as_base()]);
    object.clear_peers(&mut model)?;
    object.add_peers(&mut model, &object.as_base())?;
    object.clear_type_(&mut model)?;
    assert_eq!(object.type_(&model)?, None);
    assert_eq!(object.number(&model)?, i32::MAX);
    model.validate()?;
    let dynamic =
        intersect_model::core::dynamic::DynamicModel::from_basic(shapes::dynamic_schema()?, model)?;
    assert!(object.ready(dynamic.as_read())?);
    Ok(())
}

#[test]
fn versioned_and_xmi_consumers_keep_the_same_bindings() -> Result<(), String> {
    let mut model = BasicModel::new(library::schema()?)?;
    let (_, copy) = populate(&mut model)?;
    let mut versions = VersionedModel::new(model)?;
    let original = versions.snapshot()?;
    let mut fork = versions.fork(&original)?;
    copy.set_title(&mut fork, Some("Fork"))?;
    assert_eq!(copy.title(&original)?, Some("Dune".into()));
    assert_eq!(copy.title(&fork)?, Some("Fork".into()));
    versions.commit(fork)?;
    assert_eq!(copy.title(&versions.snapshot()?)?, Some("Fork".into()));
    let mut formats = intersect_model::core::interchange::FormatRegistry::default();
    formats.register(intersect_model::xmi::XmiFormat)?;
    let schema = library::schema()?;
    let resources = formats.export("xmi", &schema, &storage::export(&original)?)?;
    let restored = formats.import("xmi", &schema, &resources)?;
    let store = intersect_model::core::frozen::FrozenStore::from_materialized(&schema, &restored)?;
    let loaded = BasicModel::from_store(schema, store)?;
    assert_eq!(copy.title(&loaded)?, Some("Dune".into()));
    assert_eq!(
        copy.library(&loaded)?.ok_or("Missing owner")?.id(),
        "catalog"
    );
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn sqlite_uses_the_same_generated_accessors() -> Result<(), String> {
    let store = intersect_model::sqlite::SqliteStore::in_memory()?;
    let mut model = TransactionalModel::new(BasicModel::from_store(library::schema()?, store)?);
    let (catalog, copy) = model.transaction(|edit| populate(edit))?;
    assert_eq!(catalog.copies(&model)?, vec![copy.clone()]);
    assert_eq!(copy.library(&model)?, Some(catalog));
    assert!(
        model
            .transaction(|edit| {
                copy.set_title(edit, Some("Discarded"))?;
                Err::<(), _>("Rollback".into())
            })
            .is_err()
    );
    assert_eq!(copy.title(&model)?, Some("Dune".into()));
    Ok(())
}

struct StateSettings<'a>(&'a str);
impl intersect_model::core::query::DerivedValues for StateSettings<'_> {
    fn schema_revision(&self) -> Option<&str> {
        Some(self.0)
    }
    fn explicit_set_state(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> Result<bool, String> {
        budget.charge()?;
        storage::is_explicitly_set(store, &crate::state::schema()?, id, state_backing(feature)?)
    }
    fn supports(&self, class: &str, feature: &str) -> bool {
        class == "Item" && matches!(feature, "computedHistory" | "computedBag" | "computedLinks")
    }
    fn read(
        &self,
        store: &dyn ModelRead,
        id: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> Result<serde_json::Value, String> {
        budget.charge()?;
        store
            .value(id, state_backing(feature)?)?
            .ok_or("Missing backing".into())
    }
}
fn state_backing(feature: &str) -> Result<&str, String> {
    match feature {
        "computedHistory" => Ok("history"),
        "computedBag" => Ok("bag"),
        "computedLinks" => Ok("links"),
        _ => Err("Unknown computed setting".into()),
    }
}
impl storage::SettingValues for StateSettings<'_> {
    fn plan(
        &self,
        _: &dyn ModelRead,
        id: &str,
        feature: &str,
        edit: &storage::ComputedEdit,
        budget: &mut QueryBudget,
    ) -> Result<Vec<storage::SettingEdit>, String> {
        budget.charge()?;
        let feature = state_backing(feature)?.to_owned();
        Ok(vec![match edit {
            storage::ComputedEdit::Set(value) => storage::SettingEdit::Set {
                id: id.into(),
                feature,
                value: value.clone(),
            },
            storage::ComputedEdit::Append(_) => {
                return Err("Append unsupported by this scalar test provider".into());
            }
            storage::ComputedEdit::Unset => storage::SettingEdit::Unset {
                id: id.into(),
                feature,
            },
        }])
    }
}

fn exercise_generated_state(edit: &mut impl ModelEdit) -> Result<(), String> {
    use crate::state::Item;
    let a = Item::create(edit, "a", "model")?;
    let b = Item::create(edit, "b", "model")?;
    let original = storage::export(edit)?;
    let revision = edit.schema().revision.clone();
    let provider = StateSettings(&revision);
    let context = storage::SettingContext {
        provider: &provider,
        conversions: None,
    };
    a.set_computed_history(edit, Some("seed"), context, &mut QueryBudget::new(100))?;
    assert!(a.is_set_computed_history(edit, &provider, &mut QueryBudget::new(100))?);
    a.unset_computed_history(edit, context, &mut QueryBudget::new(100))?;
    assert!(!a.is_set_computed_history(edit, &provider, &mut QueryBudget::new(100))?);
    a.set_computed_bag(edit, &[], context, &mut QueryBudget::new(100))?;
    assert!(a.is_set_computed_bag(edit, &provider, &mut QueryBudget::new(100))?);
    a.unset_computed_bag(edit, context, &mut QueryBudget::new(100))?;
    a.set_computed_history(edit, Some("computed"), context, &mut QueryBudget::new(100))?;
    assert_eq!(a.history(edit)?, Some("computed".into()));
    assert!(a.is_set_computed_history(edit, &provider, &mut QueryBudget::new(100))?);
    a.set_computed_bag(
        edit,
        &["twice".into(), "twice".into()],
        context,
        &mut QueryBudget::new(100),
    )?;
    assert_eq!(a.bag(edit)?, vec!["twice", "twice"]);
    a.set_computed_links(
        edit,
        std::slice::from_ref(&b),
        context,
        &mut QueryBudget::new(100),
    )?;
    assert_eq!(a.links(edit)?, vec![b.clone()]);
    let prior = storage::export(edit)?;
    assert!(
        a.set_computed_links(
            edit,
            &[b.clone(), b.clone()],
            context,
            &mut QueryBudget::new(100)
        )
        .is_err()
    );
    assert!(
        a.set_computed_history(edit, Some("budget"), context, &mut QueryBudget::new(0))
            .is_err()
    );
    assert_eq!(storage::export(edit)?, prior);
    a.unset_computed_history(edit, context, &mut QueryBudget::new(100))?;
    a.unset_computed_bag(edit, context, &mut QueryBudget::new(100))?;
    a.unset_computed_links(edit, context, &mut QueryBudget::new(100))?;
    let reset = storage::export(edit)?;
    let mut expected = original.clone();
    for (uri, record) in &mut expected.resources {
        let current = &reset.resources[uri];
        assert!(current.content_revision > record.content_revision);
        record.content_revision = current.content_revision;
    }
    assert_eq!(reset, expected);

    assert!(!a.is_set_ordinary(edit)?);
    a.set_ordinary(edit, Some("seed"))?;
    assert!(!a.is_set_ordinary(edit)?);
    a.set_history(edit, Some("seed"))?;
    assert!(a.is_set_history(edit)?);
    assert!(a.as_base().is_set_history(edit)?);
    a.clear_history(edit)?;
    assert_eq!(a.history(edit)?, None);
    assert!(a.is_set_history(edit)?);
    a.unset_history(edit)?;
    assert_eq!(a.history(edit)?, Some("seed".into()));
    assert!(!a.is_set_history(edit)?);
    assert!(!a.is_set_tags(edit)?);
    a.clear_tags(edit)?;
    assert!(a.is_set_tags(edit)?);
    a.add_tags(edit, "first")?;
    a.add_tags(edit, "first")?;
    assert_eq!(a.tags(edit)?, vec!["first"]);
    assert!(!a.remove_tags(edit, "missing")?);
    assert!(a.insert_tags(edit, 0, "second")?);
    assert!(a.replace_tags(edit, 1, "third")?);
    assert!(a.move_tags(edit, 0, 1)?);
    assert_eq!(a.tags(edit)?, vec!["third", "second"]);
    let indexed_before = storage::export(edit)?;
    assert!(a.insert_tags(edit, usize::MAX, "invalid").is_err());
    assert!(a.replace_tags(edit, 1, "third").is_err());
    assert!(a.move_tags(edit, 0, usize::MAX).is_err());
    assert!(a.remove_at_tags(edit, usize::MAX).is_err());
    assert_eq!(storage::export(edit)?, indexed_before);
    assert!(a.remove_at_tags(edit, 1)?);
    assert!(a.remove_tags(edit, "third")?);
    a.add_tags(edit, "first")?;
    a.add_bag(edit, "same")?;
    a.add_bag(edit, "same")?;
    assert!(a.remove_bag(edit, "same")?);
    assert_eq!(a.bag(edit)?, vec!["same"]);
    a.unset_bag(edit)?;
    assert!(!a.is_set_bag(edit)?);
    let before = storage::export(edit)?;
    assert!(a.set_tags(edit, &["replacement".into()]).is_err());
    assert!(a.unset_tags(edit).is_err());
    assert_eq!(storage::export(edit)?, before);
    a.add_links(edit, &b)?;
    a.add_links(edit, &b)?;
    assert_eq!(a.links(edit)?, vec![b.clone()]);
    assert!(a.insert_links(edit, 0, &a)?);
    let c = Item::create(edit, "c", "model")?;
    assert!(a.replace_links(edit, 0, &c)?);
    assert!(a.move_links(edit, 0, 1)?);
    assert_eq!(a.links(edit)?, vec![b, c.clone()]);
    assert!(a.remove_at_links(edit, 0)?);
    assert!(a.remove_links(edit, &c)?);
    a.clear_links(edit)?;
    assert!(a.is_set_links(edit)?);
    a.unset_links(edit)?;
    assert!(!a.is_set_links(edit)?);
    Ok(())
}

#[test]
fn generated_assignment_state_and_readonly_collections_share_facade_semantics() -> Result<(), String>
{
    let mut basic = BasicModel::new(crate::state::schema()?)?;
    exercise_generated_state(&mut basic)?;
    let expected = storage::export(&basic)?;
    let mut transactional = TransactionalModel::new(BasicModel::new(crate::state::schema()?)?);
    transactional.transaction(|edit| exercise_generated_state(edit))?;
    assert_eq!(storage::export(&transactional)?, expected);
    let mut versions = VersionedModel::new(BasicModel::new(crate::state::schema()?)?)?;
    let initial = versions.snapshot()?;
    let mut fork = versions.fork(&initial)?;
    exercise_generated_state(&mut fork)?;
    assert!(storage::export(&initial)?.elements.is_empty());
    versions.commit(fork)?;
    assert_eq!(storage::export(&versions.snapshot()?)?, expected);
    let before = storage::export(&transactional)?;
    assert!(
        transactional
            .transaction(|edit| {
                let a = crate::state::Item::bind(edit, "a")?;
                a.set_history(edit, Some("seed"))?;
                assert!(a.is_set_history(edit)?);
                Err::<(), _>("Rollback".into())
            })
            .is_err()
    );
    assert_eq!(storage::export(&transactional)?, before);
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn generated_state_uses_sqlite_assignment_storage() -> Result<(), String> {
    let store = intersect_model::sqlite::SqliteStore::in_memory()?;
    let mut model =
        TransactionalModel::new(BasicModel::from_store(crate::state::schema()?, store)?);
    model.transaction(|edit| exercise_generated_state(edit))?;
    let a = crate::state::Item::bind(&model, "a")?;
    assert!(!a.is_set_history(&model)?);
    assert!(a.is_set_tags(&model)?);
    let before = storage::export(&model)?;
    assert!(
        model
            .transaction(|edit| {
                a.set_history(edit, Some("seed"))?;
                a.clear_tags(edit)?;
                Err::<(), _>("Rollback".into())
            })
            .is_err()
    );
    assert_eq!(storage::export(&model)?, before);
    Ok(())
}

#[test]
fn generated_structural_edits_share_basic_transaction_and_fork_services() -> Result<(), String> {
    use storage::{ContainmentDestination, DeleteSelection, IncomingReferences};
    let mut basic = BasicModel::new(library::schema()?)?;
    let (catalog, copy) = populate(&mut basic)?;
    let branch = Library::create(&mut basic, "branch", "branch.library")?;
    branch.set_name(&mut basic, "Branch")?;
    assert!(copy.reparent(
        &mut basic,
        Some(ContainmentDestination {
            owner: branch.id(),
            feature: "copies",
            position: Some(0)
        }),
        None,
        &mut QueryBudget::new(100_000)
    )?);
    assert!(catalog.copies(&basic)?.is_empty());
    assert_eq!(copy.library(&basic)?, Some(branch.clone()));
    assert!(!copy.reparent(
        &mut basic,
        Some(ContainmentDestination {
            owner: branch.id(),
            feature: "copies",
            position: None
        }),
        None,
        &mut QueryBudget::new(100_000)
    )?);
    let mut transactional = TransactionalModel::new(basic);
    let before = storage::export(&transactional)?;
    assert!(
        transactional
            .transaction(|edit| {
                branch.delete(
                    edit,
                    DeleteSelection::Subtrees,
                    IncomingReferences::Reject,
                    None,
                    &mut QueryBudget::new(100_000),
                )?;
                assert!(copy.name(edit).is_err());
                Err::<(), _>("domain veto".into())
            })
            .is_err()
    );
    assert_eq!(storage::export(&transactional)?, before);
    let mut versioned = VersionedModel::new(transactional.into_basic())?;
    let snapshot = versioned.snapshot()?;
    let mut fork = versioned.fork(&snapshot)?;
    assert!(
        branch
            .delete(
                &mut fork,
                DeleteSelection::Objects,
                IncomingReferences::Reject,
                None,
                &mut QueryBudget::new(100_000)
            )
            .is_err()
    );
    assert!(
        copy.reparent(&mut fork, None, None, &mut QueryBudget::new(0))
            .is_err()
    );
    assert_eq!(storage::export(&fork)?, before);
    assert_eq!(
        branch.delete(
            &mut fork,
            DeleteSelection::Subtrees,
            IncomingReferences::Reject,
            None,
            &mut QueryBudget::new(100_000)
        )?,
        vec!["branch", "dune"]
    );
    assert!(copy.name(&fork).is_err());
    assert_eq!(copy.name(&snapshot)?, "dune");
    versioned.commit(fork)?;
    assert!(copy.name(&versioned.snapshot()?).is_err());
    Ok(())
}

#[test]
fn generated_structural_wrappers_keep_external_identity_semantics() -> Result<(), String> {
    let mut model = BasicModel::new(library::schema()?)?;
    let (_, copy) = populate(&mut model)?;
    copy.delete(
        &mut model,
        storage::DeleteSelection::Objects,
        storage::IncomingReferences::Reject,
        None,
        &mut QueryBudget::new(100_000),
    )?;
    assert!(copy.name(&model).is_err());
    let replacement = BookCopy::create(&mut model, copy.id(), "replacement.library")?;
    replacement.set_name(&mut model, "replacement")?;
    // Generated wrappers address external IDs; dynamic ObjectHandles identify incarnations.
    assert_eq!(copy.name(&model)?, "replacement");
    let mut other = BasicModel::new(shapes::schema()?)?;
    assert!(
        copy.reparent(&mut other, None, None, &mut QueryBudget::new(100_000))
            .is_err()
    );
    assert!(
        copy.delete(
            &mut other,
            storage::DeleteSelection::Subtrees,
            storage::IncomingReferences::Detach,
            None,
            &mut QueryBudget::new(100_000)
        )
        .is_err()
    );
    Ok(())
}

fn rcore_schema(text: &str) -> Result<intersect_model::definitions::schema::Schema, String> {
    use intersect_model::definitions::bundle::{DefinitionBundle, DefinitionSource, SourceKind};
    let source = DefinitionSource::new("memory:model.rcore", SourceKind::Rcore, text);
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    intersect_model::definitions::rcore::compile_structure(&bundle, &source.id)
        .map(|s| s.schema)
        .map_err(|e| format!("{e:?}"))
}
