//! Source getter composition uses the existing named-function call machinery.
use super::*;
use crate::definitions::rcore_behavior::model_types::FeatureType;

impl Compiler<'_> {
    pub(super) fn source_getter(
        &mut self,
        field: &FeatureType,
        span: &SourceSpan,
    ) -> Result<(FunctionId, TypeRef)> {
        if !field.derived || !field.assignment_exact || field.unsettable {
            return Err(diagnostic(
                span,
                "Composed getter requires an exact derived value contract without assignment history",
            ));
        }
        let mut selected = None;
        for module in &self.report.modules {
            for (ordinal, provider) in &module.providers {
                self.remaining = self
                    .remaining
                    .checked_sub(1)
                    .ok_or_else(|| diagnostic(span, "Getter binding lookup work limit exceeded"))?;
                if provider
                    .feature
                    .as_ref()
                    .is_some_and(|feature| feature.declaration == field.declaration)
                {
                    if selected.is_some() {
                        return Err(diagnostic(
                            span,
                            "Derived member has multiple source bindings; virtual override dispatch is unsupported",
                        ));
                    }
                    selected = Some((module.source.clone(), *ordinal));
                }
            }
        }
        let (source, ordinal) = selected.ok_or_else(|| {
            diagnostic(
                span,
                "Derived model navigation requires an executable source getter binding",
            )
        })?;
        let getter =
            crate::definitions::rcore_getter::select_source_getter(&self.report, &source, ordinal)
                .map_err(|errors| {
                    errors
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| diagnostic(span, "Invalid source getter binding"))
                })?;
        if getter.function.parameters.len() != 1 {
            return Err(diagnostic(
                span,
                "Source getter function requires exactly its receiver",
            ));
        }
        // Normalize Model/ModelApplication representations through the existing
        // wire type graph, as top-level getter installation does. The function
        // receiver and result remain exact; inherited callers use a checked view.
        let mut nodes = Vec::new();
        let mut mapped = BTreeMap::new();
        let mut roots = Vec::new();
        for ty in [
            getter.provider.receiver,
            getter.feature.value,
            getter.function.parameters[0],
            getter.function.result,
            field.value,
        ] {
            roots.push(
                image::export_type(ty, &self.table, &mut nodes, &mut mapped, self.limits)
                    .map_err(|error| diagnostic(span, error.to_string()))?,
            );
        }
        let symbol = getter.function.symbol.clone();
        let mut canonical = TypeTable::default();
        let imported = image::import_types(&nodes, &mut canonical, &mut self.models, self.limits)
            .map_err(|error| diagnostic(span, error.to_string()))?;
        let root = |index: usize| imported.get(roots[index].0 as usize).copied();
        if root(0) != root(2) || root(1) != root(3) || root(1) != root(4) {
            return Err(diagnostic(
                span,
                "Composed getter function differs from its exact receiver or feature result contract",
            ));
        }
        self.named_instance(&symbol, &[], span)
    }
}
