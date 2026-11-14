//! Shared native qualification fixture.
use crate::FixtureRequired;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    generic_types::{RuntimeType, Type},
    resources::DeclarationId,
};
pub struct Fixture {
    pub bundle: DefinitionBundle,
    #[allow(dead_code)] // Language fixtures also need the selected source.
    pub source: DefinitionSource,
    declarations: std::collections::BTreeMap<Vec<usize>, DeclarationId>,
}
impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}
impl Fixture {
    pub fn new() -> Self {
        let source = DefinitionSource::new(
            "memory:runtime-lower.ecore",
            SourceKind::Ecore,
            include_str!("../../../fixtures/ecore-generics/runtime-lower-bounds.ecore"),
        );
        let (source, declarations) = crate::migration_fixture::migrate_source_fixture(source);
        Self {
            bundle: DefinitionBundle {
                version: 1,
                sources: vec![source.clone()],
                namespaces: vec![],
            },
            source,
            declarations,
        }
    }
    pub fn ty(&self, n: usize, args: Vec<Type>, raw: bool) -> RuntimeType {
        RuntimeType::new(Type::Classifier {
            declaration: self.declarations[&vec![n]].clone(),
            arguments: args,
            raw,
        })
        .required()
    }
    pub fn atom(&self, n: usize) -> RuntimeType {
        self.ty(n, vec![], false)
    }
    pub fn pair(&self, a: &RuntimeType, b: &RuntimeType) -> RuntimeType {
        self.ty(
            3,
            vec![a.expression().clone(), b.expression().clone()],
            false,
        )
    }
}
