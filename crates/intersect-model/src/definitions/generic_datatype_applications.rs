//! Source-checked closed datatype applications. Provider execution is a separate contract.
use super::*;
use crate::definitions::generic_types::{ClosedType, Type};

/// Checked against one immutable resolver context. Wire data must be checked again.
/// This establishes datatype kind, argument identity, arity and bounds, not a codec.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CheckedDatatypeApplication {
    application: ClosedType,
    declaration_span: SourceSpan,
    parameters: Vec<ResolvedParameter>,
    bounds: Vec<BoundCheck>,
}
impl CheckedDatatypeApplication {
    pub fn application(&self) -> &ClosedType {
        &self.application
    }
    pub fn declaration_span(&self) -> &SourceSpan {
        &self.declaration_span
    }
    pub fn parameters(&self) -> &[ResolvedParameter] {
        &self.parameters
    }
    pub fn bounds(&self) -> &[BoundCheck] {
        &self.bounds
    }
}
impl Resolver {
    /// Recheck every exact argument identity and bound using the shared engine.
    /// Raw/open/wildcard runtime identities are not admitted by ClosedType.
    pub fn instantiate_datatype(
        &self,
        application: &ClosedType,
    ) -> Result<CheckedDatatypeApplication, ImportError> {
        let Type::Classifier { declaration, .. } = application.expression() else {
            return Err(error(
                &SourceSpan {
                    source: self.standard_id.clone(),
                    start: 0,
                    end: 0,
                },
                "Requested datatype must be a closed classifier",
            ));
        };
        let declaration_span = self.node(declaration)?.span.clone();
        if self.classifier(declaration)? != TargetKind::Datatype {
            return Err(error(
                &declaration_span,
                "Requested datatype must be an EDataType",
            ));
        }
        let parameters = self
            .formals(declaration)?
            .iter()
            .map(|formal| self.resolve_parameter(&formal.declaration))
            .collect::<Result<Vec<_>, _>>()?;
        let bounds = self.check_expression_bounds(application.expression(), &declaration_span)?;
        if let Some(failed) = bounds.iter().find(|check| !check.satisfied) {
            return Err(error(
                &failed.declared_bound.span,
                "Datatype application violates generic bounds",
            ));
        }
        Ok(CheckedDatatypeApplication {
            application: application.clone(),
            declaration_span,
            parameters,
            bounds,
        })
    }
}
