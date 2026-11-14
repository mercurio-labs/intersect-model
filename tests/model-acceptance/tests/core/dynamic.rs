use intersect_model::core::{
    api::{BasicModel, ModelEdit, VersionedModel},
    dynamic::DynamicModel,
    storage::{ModelRead, ObjectHandle, Query},
};
use intersect_model::definitions::dynamic::{
    DynamicSchema, EClass, EFeature, ScalarType, SchemaBuilder,
};
use serde_json::json;

struct Fixture {
    schema: DynamicSchema,
    named: EClass,
    book: EClass,
    library: EClass,
    name: EFeature,
    edition: EFeature,
    books: EFeature,
    owner: EFeature,
}
fn fixture() -> Fixture {
    let mut builder = SchemaBuilder::new("library", "urn:example:library");
    let named = builder.class("Named").unwrap();
    named.abstract_type().unwrap();
    let name = named
        .attribute("name", ScalarType::String)
        .required()
        .unwrap();
    let book = builder.class("Book").unwrap();
    book.extends(&named).unwrap();
    let edition = book
        .attribute("edition", ScalarType::Integer)
        .default(1)
        .unwrap();
    let library = builder.class("Library").unwrap();
    library.extends(&named).unwrap();
    let books = library
        .reference("books", &book)
        .many()
        .containment()
        .unwrap();
    let owner = book.reference("library", &library).optional().unwrap();
    builder.opposites(&books, &owner).unwrap();
    Fixture {
        schema: builder.build().unwrap(),
        named,
        book,
        library,
        name,
        edition,
        books,
        owner,
    }
}
fn seed(model: &mut DynamicModel, f: &Fixture) -> (ObjectHandle, ObjectHandle) {
    model
        .transaction(|tx| {
            let mut r = tx.create_resource("urn:catalog")?;
            let l = r.create(&f.library, "library")?;
            r.set(&l, &f.name, "Central")?;
            let b = r.create(&f.book, "book")?;
            r.set(&b, &f.name, "Dune")?;
            r.add(&l, &f.books, &b)?;
            Ok((l, b))
        })
        .unwrap()
}

#[test]
fn inherited_features_defaults_opposites_and_edits_use_shared_services() {
    let f = fixture();
    let mut model = DynamicModel::new(f.schema.clone()).unwrap();
    let (l, b) = seed(&mut model, &f);
    assert_eq!(model.get(&b, &f.name).unwrap().as_str().unwrap(), "Dune");
    assert_eq!(model.get(&b, &f.edition).unwrap().as_i32().unwrap(), 1);
    assert_eq!(
        model.get(&b, &f.owner).unwrap().as_object().unwrap(),
        Some(l.clone())
    );
    assert_eq!(
        model.get(&l, &f.books).unwrap().as_objects().unwrap(),
        vec![b.clone()]
    );
    assert!(model.get(&b, &f.owner).unwrap().as_str().is_err());
    assert_eq!(model.class(&b).unwrap(), f.book);
    model
        .transaction(|tx| {
            tx.set(&b, &f.edition, 2)?;
            assert_eq!(tx.get(&b, &f.edition)?.as_i32()?, 2);
            Ok(())
        })
        .unwrap();
    assert!(model.get(&l, &f.edition).is_err());
    assert!(
        model
            .transaction(|tx| tx.create(&f.named, "abstract", "urn:catalog"))
            .is_err()
    );
    assert!(
        model
            .transaction(|tx| tx.create(&f.book, "book", "urn:other"))
            .is_err()
    );
}

#[test]
fn checked_references_keep_question_mark_identities_through_reload() -> Result<(), String> {
    let f = fixture();
    let mut model = DynamicModel::new(f.schema.clone())?;
    let old_book = model.transaction(|tx| {
        let mut resource = tx.create_resource("urn:catalog")?;
        let library = resource.create(&f.library, "?catalog")?;
        let book = resource.create(&f.book, "?book")?;
        resource.set(&library, &f.name, "Central")?;
        resource.set(&book, &f.name, "Dune")?;
        resource.add(&library, &f.books, &book)?;
        Ok(book)
    })?;

    let mut wire = model.to_model()?;
    wire.edit(
        f.schema.as_schema(),
        &[("?book".into(), "edition".into(), json!(2))],
    )?;
    let reopened = DynamicModel::from_model(f.schema, wire)?;
    let book = reopened.object("?book")?;
    let library = reopened.object("?catalog")?;
    assert!(reopened.class(&old_book).is_err());
    assert_eq!(reopened.get(&book, &f.edition)?.as_i32()?, 2);
    assert_eq!(
        reopened.get(&book, &f.owner)?.as_object()?,
        Some(library.clone())
    );
    assert_eq!(reopened.get(&library, &f.books)?.as_objects()?, [book]);
    Ok(())
}

#[test]
fn failed_commit_and_callback_error_restore_graph_indexes_and_handle_lifetimes() {
    let f = fixture();
    let mut model = DynamicModel::new(f.schema.clone()).unwrap();
    let (l, b) = seed(&mut model, &f);
    let baseline = model.to_json().unwrap();
    let mut leaked = None;
    assert!(
        model
            .transaction(|tx| {
                let bad = tx.create(&f.book, "bad", "urn:catalog")?;
                tx.add(&l, &f.books, &bad)?;
                leaked = Some(bad);
                tx.set(&b, &f.edition, 3)?;
                Ok(())
            })
            .is_err()
    );
    assert_eq!(model.to_json().unwrap(), baseline);
    let stale = leaked.unwrap();
    assert!(model.class(&stale).is_err());
    let replacement = model
        .transaction(|tx| {
            let b = tx.create(&f.book, "bad", "urn:catalog")?;
            tx.set(&b, &f.name, "Replacement")?;
            Ok(b)
        })
        .unwrap();
    assert_ne!(replacement, stale);
    assert!(model.class(&stale).is_err());
    let baseline = model.to_json().unwrap();
    assert!(
        model
            .transaction(|tx| {
                tx.clear(&l, &f.books)?;
                Err::<(), _>("cancel".into())
            })
            .is_err()
    );
    assert_eq!(model.to_json().unwrap(), baseline);
    for invalid in [json!(true), json!(2147483648_i64), json!([1])] {
        assert!(
            model
                .transaction(|tx| tx.set(&b, &f.edition, invalid))
                .is_err()
        );
        assert_eq!(model.to_json().unwrap(), baseline);
    }
    assert!(model.transaction(|tx| tx.add(&l, &f.books, &b)).is_err());
    assert_eq!(model.to_json().unwrap(), baseline);
}

#[test]
fn foreign_schema_features_objects_and_reference_strings_are_rejected() {
    let f = fixture();
    let other = fixture();
    let mut model = DynamicModel::new(f.schema.clone()).unwrap();
    let (_, b) = seed(&mut model, &f);
    let mut other_model = DynamicModel::new(f.schema.clone()).unwrap();
    let (foreign_l, foreign_b) = seed(&mut other_model, &f);
    assert!(
        model
            .transaction(|tx| tx.create(&other.book, "other", "urn:catalog"))
            .is_err()
    );
    assert!(model.get(&b, &other.name).is_err());
    assert!(model.get(&foreign_b, &f.name).is_err());
    assert!(
        model
            .transaction(|tx| tx.set(&b, &f.owner, &foreign_l))
            .is_err()
    );
    assert!(
        model
            .transaction(|tx| tx.set(&b, &f.owner, "library"))
            .is_err()
    );
    assert!(model.transaction(|tx| tx.set(&b, &f.name, &b)).is_err());
}

#[test]
fn cross_resource_references_move_atomically_and_snapshots_validate_on_reload() {
    let f = fixture();
    let mut model = DynamicModel::new(f.schema.clone()).unwrap();
    let (l, b) = seed(&mut model, &f);
    let other = model
        .transaction(|tx| {
            let other = tx
                .create_resource("urn:other")?
                .create(&f.library, "other")?;
            tx.set(&other, &f.name, "Other")?;
            tx.clear(&l, &f.books)?;
            tx.add(&other, &f.books, &b)?;
            Ok(other)
        })
        .unwrap();
    assert_eq!(
        model.get(&b, &f.owner).unwrap().as_object().unwrap(),
        Some(other)
    );
    assert!(
        model
            .get(&l, &f.books)
            .unwrap()
            .as_objects()
            .unwrap()
            .is_empty()
    );
    let bytes = model.to_json().unwrap();
    let restored = DynamicModel::from_json(&bytes).unwrap();
    assert_eq!(restored.to_json().unwrap(), bytes);
    let rb = restored.object("book").unwrap();
    let book = restored.schema().class("Book").unwrap();
    assert_eq!(
        restored
            .get(&rb, &book.feature("name").unwrap())
            .unwrap()
            .as_str()
            .unwrap(),
        "Dune"
    );
    assert!(restored.get(&rb, &f.name).is_err());
    assert!(restored.class(&b).is_err());
    let mut corrupt: serde_json::Value = serde_json::from_str(&bytes).unwrap();
    corrupt["model"]["elements"][0]["values"]["name"] = json!(42);
    assert!(DynamicModel::from_json(&corrupt.to_string()).is_err());
    let mut corrupt: serde_json::Value = serde_json::from_str(&bytes).unwrap();
    corrupt["schema"]["classes"]["Book"]["features"]["edition"]["default"] = json!(12);
    assert!(DynamicModel::from_json(&corrupt.to_string()).is_err());
}

#[test]
fn containment_cycles_and_multiple_owners_roll_back() {
    let mut builder = SchemaBuilder::new("tree", "urn:tree");
    let node = builder.class("Node").unwrap();
    let children = node
        .reference("children", &node)
        .many()
        .containment()
        .unwrap();
    let mut model = DynamicModel::new(builder.build().unwrap()).unwrap();
    let (a, b, c) = model
        .transaction(|tx| {
            let mut r = tx.create_resource("urn:tree:instance")?;
            Ok((
                r.create(&node, "a")?,
                r.create(&node, "b")?,
                r.create(&node, "c")?,
            ))
        })
        .unwrap();
    model.transaction(|tx| tx.add(&a, &children, &b)).unwrap();
    let baseline = model.to_json().unwrap();
    assert!(model.transaction(|tx| tx.add(&b, &children, &a)).is_err());
    assert_eq!(model.to_json().unwrap(), baseline);
    assert!(model.transaction(|tx| tx.add(&c, &children, &b)).is_err());
    assert_eq!(model.to_json().unwrap(), baseline);
}

#[test]
fn unwind_rolls_back_existing_and_new_objects() {
    let f = fixture();
    let mut model = DynamicModel::new(f.schema.clone()).unwrap();
    let (_, b) = seed(&mut model, &f);
    let baseline = model.to_json().unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<(), String> = model.transaction(|tx| {
            tx.set(&b, &f.edition, 9)?;
            tx.create(&f.book, "aborted", "urn:catalog")?;
            panic!("host failed")
        });
    }));
    assert!(result.is_err());
    assert_eq!(model.to_json().unwrap(), baseline);
}

#[test]
fn bounded_collections_preserve_order_duplicates_and_subtype_references() {
    let mut builder = SchemaBuilder::new("collection", "urn:collection");
    let base = builder.class("Base").unwrap();
    base.abstract_type().unwrap();
    let child = builder.class("Child").unwrap();
    child.extends(&base).unwrap();
    let holder = builder.class("Holder").unwrap();
    let entries = holder
        .reference("entries", &base)
        .bounds(1, 2)
        .unique(false)
        .build()
        .unwrap();
    let labels = holder
        .attribute("labels", ScalarType::String)
        .many()
        .unique(false)
        .build()
        .unwrap();
    let flag = holder
        .attribute("flag", ScalarType::Boolean)
        .many()
        .bounds(0, 1)
        .build()
        .unwrap();
    let mut model = DynamicModel::new(builder.build().unwrap()).unwrap();
    let (h, c) = model
        .transaction(|tx| {
            let h = tx.create(&holder, "h", "urn:instances")?;
            let c = tx.create(&child, "c", "urn:instances")?;
            tx.set(&h, &entries, vec![c.clone(), c.clone()])?;
            tx.set(&h, &labels, json!(["second", "first", "second"]))?;
            Ok((h, c))
        })
        .unwrap();
    assert_eq!(
        model.get(&h, &entries).unwrap().as_objects().unwrap(),
        vec![c.clone(), c.clone()]
    );
    assert_eq!(
        model.get(&h, &labels).unwrap().to_json(),
        json!(["second", "first", "second"])
    );
    assert!(!model.get(&h, &flag).unwrap().as_bool().unwrap());
    let baseline = model.to_json().unwrap();
    assert!(model.transaction(|tx| tx.add(&h, &entries, &c)).is_err());
    assert!(model.transaction(|tx| tx.clear(&h, &entries)).is_err());
    assert!(
        model
            .transaction(|tx| tx.set(&h, &entries, vec![h.clone()]))
            .is_err()
    );
    assert_eq!(model.to_json().unwrap(), baseline);
}

#[test]
fn opposite_displacement_and_caught_duplicate_leave_transaction_usable() {
    let f = fixture();
    let mut model = DynamicModel::new(f.schema.clone()).unwrap();
    let (library, book) = seed(&mut model, &f);
    let other = model
        .transaction(|tx| {
            let other = tx.create(&f.library, "other", "urn:other")?;
            tx.set(&other, &f.name, "Other")?;
            tx.add(&other, &f.books, &book)?;
            assert_eq!(tx.get(&book, &f.owner)?.as_object()?, Some(other.clone()));
            assert!(tx.add(&other, &f.books, &book).is_err());
            tx.set(&book, &f.edition, 5)?;
            Ok(other)
        })
        .unwrap();
    assert_eq!(
        model.get(&book, &f.owner).unwrap().as_object().unwrap(),
        Some(other.clone())
    );
    assert!(
        model
            .get(&library, &f.books)
            .unwrap()
            .as_objects()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        model.get(&other, &f.books).unwrap().as_objects().unwrap(),
        vec![book.clone()]
    );
    assert_eq!(model.get(&book, &f.edition).unwrap().as_i32().unwrap(), 5);
}

#[test]
fn caught_structural_errors_preserve_prior_edits_and_allow_commit() {
    let f = fixture();
    let mut model = DynamicModel::new(f.schema.clone()).unwrap();
    let (library, book) = seed(&mut model, &f);
    model
        .transaction(|tx| {
            tx.set(&book, &f.edition, 4)?;
            assert!(tx.set(&book, &f.edition, "invalid").is_err());
            assert_eq!(tx.get(&book, &f.edition)?.as_i32()?, 4);
            assert!(tx.add(&library, &f.books, &book).is_err());
            assert_eq!(
                tx.get(&library, &f.books)?.as_objects()?,
                vec![book.clone()]
            );
            // Required fields can be incomplete between checked edits.
            tx.clear(&book, &f.name)?;
            tx.set(&book, &f.name, "Revised")?;
            Ok(())
        })
        .unwrap();
    assert_eq!(model.get(&book, &f.edition).unwrap().as_i32().unwrap(), 4);
    assert_eq!(
        model.get(&book, &f.name).unwrap().as_str().unwrap(),
        "Revised"
    );
}

#[test]
fn domain_rejection_uses_the_shared_transaction_boundary() {
    let f = fixture();
    let mut model = DynamicModel::new(f.schema.clone()).unwrap();
    let (_, book) = seed(&mut model, &f);
    let baseline = model.to_model().unwrap();
    let outcome = model.transaction_with(
        |tx| tx.set(&book, &f.edition, 9),
        |staged| {
            assert_eq!(staged.value("book", "edition")?, Some(json!(9)));
            Err("Edition rejected by domain policy".into())
        },
    );
    assert_eq!(outcome.unwrap_err(), "Edition rejected by domain policy");
    assert_eq!(model.to_model().unwrap(), baseline);
    assert_eq!(model.object("book").unwrap(), book);
    model
        .transaction(|tx| tx.set(&book, &f.edition, 2))
        .unwrap();
}

#[test]
fn dynamic_basic_queries_and_versioned_forks_share_one_model() {
    let f = fixture();
    let mut model = DynamicModel::new(f.schema.clone()).unwrap();
    let (_, book) = seed(&mut model, &f);
    assert_eq!(
        model.as_read().query(Query::Class("Book")).unwrap(),
        vec!["book"]
    );
    let mut basic = model.into_basic();
    basic.set("book", "edition", json!(3)).unwrap();
    let model = DynamicModel::from_basic(f.schema.clone(), basic).unwrap();
    assert_eq!(model.object("book").unwrap(), book);
    assert_eq!(model.get(&book, &f.edition).unwrap().as_i32().unwrap(), 3);

    let mut versions = VersionedModel::new(model.into_basic()).unwrap();
    let before = versions.snapshot().unwrap();
    let mut draft = versions.fork(&before).unwrap();
    draft.set("book", "edition", json!(4)).unwrap();
    assert_eq!(before.value("book", "edition").unwrap(), Some(json!(3)));
    versions.commit(draft).unwrap();
    let after = versions.snapshot().unwrap();
    assert_eq!(after.value("book", "edition").unwrap(), Some(json!(4)));
    assert_eq!(before.value("book", "edition").unwrap(), Some(json!(3)));
    assert_eq!(after.query(Query::Class("Book")).unwrap(), vec!["book"]);
}

#[test]
fn basic_adoption_rejects_incomplete_models_and_mismatched_definitions() {
    let f = fixture();
    let mut draft = BasicModel::new(f.schema.as_schema().clone()).unwrap();
    draft.create("incomplete", "Book", "urn:catalog").unwrap();
    assert!(DynamicModel::from_basic(f.schema.clone(), draft).is_err());

    let mut different = f.schema.as_schema().clone();
    different
        .classes
        .get_mut("Book")
        .unwrap()
        .features
        .get_mut("edition")
        .unwrap()
        .default = json!(7);
    // Matching revision strings alone do not authorize a different definition.
    let basic = BasicModel::new(different).unwrap();
    assert!(DynamicModel::from_basic(f.schema, basic).is_err());
}

#[test]
fn structural_handles_transfer_delete_rollback_and_recreation() -> Result<(), String> {
    use intersect_model::core::{
        dynamic::DynamicContainmentDestination,
        query::QueryBudget,
        storage::{DeleteSelection, IncomingReferences},
    };
    let f = fixture();
    let mut model = DynamicModel::new(f.schema.clone())?;
    let (library, book) = seed(&mut model, &f);
    let branch = model.transaction(|tx| {
        let branch = tx.create(&f.library, "branch", "urn:branch")?;
        tx.set(&branch, &f.name, "Branch")?;
        assert!(tx.reparent(
            &book,
            Some(DynamicContainmentDestination {
                owner: &branch,
                feature: &f.books,
                position: Some(0),
            }),
            &mut QueryBudget::new(100_000)
        )?);
        assert!(!tx.reparent(
            &book,
            Some(DynamicContainmentDestination {
                owner: &branch,
                feature: &f.books,
                position: None,
            }),
            &mut QueryBudget::new(100_000)
        )?);
        Ok(branch)
    })?;
    assert!(model.get(&library, &f.books)?.as_objects()?.is_empty());
    assert_eq!(
        model.get(&book, &f.owner)?.as_object()?,
        Some(branch.clone())
    );
    let before = model.to_model()?;
    assert!(
        model
            .transaction(|tx| {
                tx.delete_objects(
                    std::slice::from_ref(&branch),
                    DeleteSelection::Subtrees,
                    IncomingReferences::Reject,
                    &mut QueryBudget::new(100_000),
                )?;
                assert!(tx.get(&book, &f.name).is_err());
                Err::<(), _>("domain veto".into())
            })
            .is_err()
    );
    assert_eq!(model.to_model()?, before);
    assert_eq!(model.get(&book, &f.name)?.as_str()?, "Dune");
    model.transaction(|tx| {
        assert!(
            tx.delete_objects(
                std::slice::from_ref(&branch),
                DeleteSelection::Objects,
                IncomingReferences::Reject,
                &mut QueryBudget::new(100_000)
            )
            .is_err()
        );
        assert!(tx.reparent(&book, None, &mut QueryBudget::new(0)).is_err());
        assert_eq!(
            tx.delete_objects(
                &[branch],
                DeleteSelection::Subtrees,
                IncomingReferences::Reject,
                &mut QueryBudget::new(100_000)
            )?,
            vec!["book", "branch"]
        );
        Ok(())
    })?;
    assert!(model.class(&book).is_err());
    let replacement = model.transaction(|tx| {
        let replacement = tx.create(&f.book, "book", "urn:new")?;
        tx.set(&replacement, &f.name, "Replacement")?;
        Ok(replacement)
    })?;
    assert_ne!(book, replacement);
    assert!(model.class(&book).is_err());
    assert_eq!(model.get(&replacement, &f.name)?.as_str()?, "Replacement");
    Ok(())
}

#[test]
fn structural_dynamic_endpoints_reject_foreign_contexts_and_features() -> Result<(), String> {
    use intersect_model::core::{
        dynamic::DynamicContainmentDestination,
        query::QueryBudget,
        storage::{DeleteSelection, IncomingReferences},
    };
    let f = fixture();
    let foreign_schema = fixture();
    let mut model = DynamicModel::new(f.schema.clone())?;
    let (library, book) = seed(&mut model, &f);
    let mut other = DynamicModel::new(f.schema.clone())?;
    let (foreign, _) = seed(&mut other, &f);
    let before = model.to_model()?;
    model.transaction(|tx| {
        assert!(
            tx.delete_objects(
                std::slice::from_ref(&foreign),
                DeleteSelection::Subtrees,
                IncomingReferences::Detach,
                &mut QueryBudget::new(100_000)
            )
            .is_err()
        );
        for (owner, feature) in [
            (&foreign, &f.books),
            (&library, &foreign_schema.books),
            (&library, &f.name),
        ] {
            assert!(
                tx.reparent(
                    &book,
                    Some(DynamicContainmentDestination {
                        owner,
                        feature,
                        position: None
                    }),
                    &mut QueryBudget::new(100_000)
                )
                .is_err()
            );
        }
        assert!(
            tx.reparent(&foreign, None, &mut QueryBudget::new(100_000))
                .is_err()
        );
        Ok(())
    })?;
    assert_eq!(model.to_model()?, before);
    Ok(())
}
