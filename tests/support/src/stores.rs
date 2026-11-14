//! Qualification-only construction seam for acceptance and resource-lifecycle.
//! Runtime crates do not depend on this module.
//! Construction leaves schema binding/revision changes to the shared test body,
//! while using the schema to choose the arena layout.
// Each suite uses a different subset of the shared fixture constructors.
#![allow(dead_code)]
use intersect_model::core::{
    api::BasicModel,
    frozen::FrozenStore,
    model::Model,
    resource_set::{ResourceContext, ResourceSet},
    storage::{ModelStore, StorageResult},
};
use intersect_model::definitions::schema::Schema;

pub trait StoreFixture: ModelStore + Sized {
    fn for_schema(schema: &Schema) -> StorageResult<Self>;
    fn from_model(schema: &Schema, model: &Model) -> StorageResult<Self>;
    const NAME: &'static str;
}

impl StoreFixture for FrozenStore {
    const NAME: &'static str = "frozen";
    fn for_schema(schema: &Schema) -> StorageResult<Self> {
        FrozenStore::for_schema(schema).map_err(|error| error.to_string())
    }
    fn from_model(schema: &Schema, model: &Model) -> StorageResult<Self> {
        FrozenStore::from_materialized(schema, model)
    }
}

pub fn store<S: StoreFixture>(schema: &Schema) -> StorageResult<S> {
    S::for_schema(schema)
}

pub fn from_model<S: StoreFixture>(schema: &Schema, model: &Model) -> StorageResult<S> {
    S::from_model(schema, model)
}

pub fn basic<S: StoreFixture>(schema: Schema) -> StorageResult<BasicModel<S>> {
    let store = store::<S>(&schema)?;
    BasicModel::from_store(schema, store)
}

pub fn resources<S: StoreFixture>(
    schema: Schema,
    context: ResourceContext,
) -> StorageResult<ResourceSet<S>> {
    let store = store::<S>(&schema)?;
    ResourceSet::from_store(schema, context, store)
}

pub fn resources_from_model<S: StoreFixture>(
    schema: Schema,
    context: ResourceContext,
    model: &Model,
) -> StorageResult<ResourceSet<S>> {
    let store = from_model::<S>(&schema, model)?;
    ResourceSet::from_store(schema, context, store)
}

pub fn empty_schema() -> StorageResult<Schema> {
    intersect_model::compile_rcore(
        "rcore 1; package fixture version \"1\"; metamodel \"urn:fixture\" prefix f { class Item {} }",
    )
    .map(|package| package.schema().clone())
    .map_err(|error| error.to_string())
}

pub fn verify_frozen_store_starts_empty_with_the_requested_schema() -> Result<(), String> {
    use intersect_model::core::storage::{self, ModelRead};
    let package = intersect_model::compile_rcore(
        "rcore 1; package fixture version \"1\"; metamodel \"urn:fixture\" prefix f { class Item {} }",
    )
    .map_err(|error| error.to_string())?;
    let schema = package.schema();
    let frozen = store::<FrozenStore>(schema)?;
    assert_eq!(storage::export(&frozen)?, Model::default());
    assert_eq!(frozen.metamodel().schema(), schema);
    assert_eq!(frozen.revision()?, "");
    Ok(())
}
