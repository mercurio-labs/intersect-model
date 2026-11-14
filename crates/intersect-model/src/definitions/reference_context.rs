//! Handwritten, model-owned canonical reference-context transactions.
//! A frontend can select EObject context; it cannot edit checked schema data.
use super::{CheckedModelCompilation, CheckedStructure, CompiledStructure};
use crate::definitions::{resources::DeclarationId, schema::Schema};
use std::ops::Deref;

mod sealed {
    pub trait Sealed {}
}

/// Structural metadata that permits only canonical reference-context selection.
/// This interface grants no grammar admission, behavior or model execution.
pub trait StructureContext: sealed::Sealed {
    fn schema(&self) -> &Schema;
    fn reference_context(&mut self) -> ReferenceContextTransaction<'_>;
}

/// A read-only schema borrow with one canonical operation. Dropping an unfinished
/// transaction restores the exact prior reference context and revision.
///
/// ```compile_fail
/// use intersect_model::definitions::model_definition::ReferenceContextTransaction;
/// fn edit(context: &mut ReferenceContextTransaction<'_>) {
///     context.classes.clear();
/// }
/// ```
/// ```compile_fail
/// use intersect_model::definitions::model_definition::ReferenceContextTransaction;
/// fn escape<'a>(context: ReferenceContextTransaction<'a>) -> &'a mut intersect_model::definitions::schema::Schema {
///     context.schema
/// }
/// ```
pub struct ReferenceContextTransaction<'a> {
    schema: &'a mut Schema,
    previous: Option<(Option<DeclarationId>, String)>,
}
impl<'a> ReferenceContextTransaction<'a> {
    fn new(schema: &'a mut Schema) -> Self {
        let previous = Some((schema.reference_top.clone(), schema.revision.clone()));
        Self { schema, previous }
    }
    pub fn enable_eobject_references(&mut self) -> Result<(), String> {
        self.schema.prepare_eobject_reference_context()
    }
    /// Retain the canonical change. This does not admit the calling frontend's
    /// syntax or turn inspection metadata into an executable owner.
    pub fn commit(mut self) {
        self.previous = None;
    }
}
impl Deref for ReferenceContextTransaction<'_> {
    type Target = Schema;
    fn deref(&self) -> &Schema {
        self.schema
    }
}
impl Drop for ReferenceContextTransaction<'_> {
    fn drop(&mut self) {
        if let Some((reference_top, revision)) = self.previous.take() {
            self.schema.reference_top = reference_top;
            self.schema.revision = revision;
        }
    }
}
impl sealed::Sealed for Schema {}
impl StructureContext for Schema {
    fn schema(&self) -> &Schema {
        self
    }
    fn reference_context(&mut self) -> ReferenceContextTransaction<'_> {
        ReferenceContextTransaction::new(self)
    }
}
impl sealed::Sealed for CheckedStructure<'_> {}
impl StructureContext for CheckedStructure<'_> {
    fn schema(&self) -> &Schema {
        self.schema()
    }
    fn reference_context(&mut self) -> ReferenceContextTransaction<'_> {
        ReferenceContextTransaction::new(&mut self.schema)
    }
}
impl sealed::Sealed for CompiledStructure<'_> {}
impl StructureContext for CompiledStructure<'_> {
    fn schema(&self) -> &Schema {
        self.schema()
    }
    fn reference_context(&mut self) -> ReferenceContextTransaction<'_> {
        self.checked.reference_context()
    }
}
impl sealed::Sealed for CheckedModelCompilation<'_> {}
impl StructureContext for CheckedModelCompilation<'_> {
    fn schema(&self) -> &Schema {
        self.schema()
    }
    fn reference_context(&mut self) -> ReferenceContextTransaction<'_> {
        self.structure.reference_context()
    }
}
