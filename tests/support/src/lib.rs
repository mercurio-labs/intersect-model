//! Model-only qualification infrastructure, not a runtime language provider.
pub mod generic_schema;
pub mod migration_fixture;
pub mod runtime_lower_bounds;
pub mod stores;
pub mod bindings {
    pub mod checked_contract;
    pub mod contract;
    pub mod native;
    pub mod typed;
}
pub use bindings::{checked_contract, contract, native, typed};

pub fn migrate_model_fixture(text: &str) -> Result<String, String> {
    use intersect_model::definitions::bundle::{DefinitionSource, SourceKind};
    let report = intersect_model::migration::migrate_ecore(
        DefinitionSource::new("memory:oracle.ecore", SourceKind::Ecore, text),
        "memory:fixture.rcore",
    )
    .map_err(|e| e.to_string())?;
    report
        .converted
        .map(|s| s.text)
        .ok_or_else(|| format!("Fixture migration failed: {:?}", report.issues))
}

trait FixtureRequired<T> {
    fn required(self) -> T;
    fn required_with(self, message: &str) -> T;
}
impl<T, E: std::fmt::Debug> FixtureRequired<T> for Result<T, E> {
    fn required(self) -> T {
        self.required_with("Invalid qualification fixture")
    }
    fn required_with(self, message: &str) -> T {
        match self {
            Ok(value) => value,
            Err(error) => panic!("{message}: {error:?}"),
        }
    }
}
impl<T> FixtureRequired<T> for Option<T> {
    fn required(self) -> T {
        self.required_with("Missing qualification fixture value")
    }
    fn required_with(self, message: &str) -> T {
        match self {
            Some(value) => value,
            None => panic!("{message}"),
        }
    }
}
