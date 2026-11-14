//! Frozen canonical wire limits and authority controls.
use super::*;
use crate::{
    core::{
        frozen::FrozenStore,
        query::QueryBudget,
        resources::SourceCorrespondence,
        storage::pending_source::tests::{Meter, definition, inputs},
        text_document::DocumentResourceAssociation,
    },
    definitions::bundle::SourceId,
};

#[test]
fn retained_wire_round_trips_frozen_rows_and_strips_source_receipts() -> StorageResult<()> {
    const RESOURCE: &str = "memory:retained-model";
    let (input, migration) = inputs()?;
    let definition = definition(&input, &migration)?;
    let schema = definition.schema();
    let mut store = FrozenStore::for_schema(schema).map_err(|error| error.to_string())?;
    let closed = [SourceId::for_text(RESOURCE, "retained model source")];
    let mut work = QueryBudget::new(100_000);
    let rows = [DocumentResourceAssociation::prepare(
        (&closed[0].uri, &closed[0].sha256),
        RESOURCE,
        1_048_576,
        &mut work,
    )?];
    atomic(&mut store, |store| {
        store.create_element(schema, "a", "A", RESOURCE, 0, 1)?;
        store.create_element(schema, "b", "A", RESOURCE, 1, 2)?;
        set_with_opposite(store, schema, "a", "next", serde_json::json!("b"))?;
        let mut record = store
            .resource_record(RESOURCE)?
            .ok_or("Missing retained resource")?;
        record.source = Some(SourceCorrespondence {
            uri: closed[0].uri.clone(),
            revision: closed[0].sha256.clone(),
        });
        store.set_resource_record(record)
    })?;
    let baseline = export(&store)?;
    let observation = InvocationContext::new(Meter::new(200_000));
    let limits = PhysicalPersistenceLimits {
        objects: 10,
        logical_bytes: 1_048_576,
        wire_bytes: 65_536,
    };
    let save = |reader: &dyn ModelRead, limits| {
        RetainedModelWire::save(reader, &definition, &closed, &rows, limits, &observation)
    };
    let wire = save(&store, limits)?;
    assert_eq!(
        wire.json()["model"],
        serde_json::to_value(&baseline).map_err(|e| e.to_string())?
    );
    let decoded = wire.restore_model(&definition, &closed, &rows, limits, &observation)?;
    assert_eq!(decoded.elements, baseline.elements);
    assert!(
        decoded
            .resources
            .get(RESOURCE)
            .is_some_and(|r| r.source.is_none())
    );
    let restored = FrozenStore::from_materialized(schema, &decoded)?;
    assert_eq!(export(&restored)?, decoded);

    let too_few = PhysicalPersistenceLimits {
        objects: 1,
        ..limits
    };
    assert!(save(&store, too_few).unwrap_err().contains("object limit"));
    assert!(
        wire.restore_model(&definition, &closed, &rows, too_few, &observation)
            .unwrap_err()
            .contains("object limit")
    );
    let exhausted = InvocationContext::new(Meter::new(0));
    assert!(
        RetainedModelWire::save(&store, &definition, &closed, &rows, limits, &exhausted).is_err()
    );
    Ok(())
}
