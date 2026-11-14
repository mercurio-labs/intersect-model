//! Source-backed verification adapters; no production API or observation policy.
use super::*;
// Verification-only owner adapters. Appended in the isolated pre-edit snapshot;
// no existing production function or observation policy is modified.
#[cfg(test)]
impl Resolver {
    pub(crate) fn legacy_owner_capture(
        &self,
        mode: &str,
        receiver: &RuntimeType,
        owner: &DeclarationId,
        environment: &Substitution,
        span: &SourceSpan,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<serde_json::Value, ImportError> {
        let json =
            |value| serde_json::to_value(value).map_err(|failure| error(span, failure.to_string()));
        match mode {
            "parents" => {
                json(self.direct_parent_types_observed(receiver.expression(), span, observation)?)
            }
            "signatures" => serde_json::to_value(self.specialize_signatures_observed(
                owner,
                environment,
                observation,
            )?)
            .map_err(|e| error(span, e.to_string())),
            "compiled_signatures" => {
                let output =
                    self.compiled_feature_signature_inputs(owner, environment, observation)?;
                Ok(
                    serde_json::json!({"kind":if matches!(output,ErasedSignatures::Shared(_)){"shared"}else{"owned"},"rows":serde_json::to_value(output).map_err(|e|error(span,e.to_string()))?}),
                )
            }
            "sources_owned" | "sources_views" => {
                let report = self.project_member_sources_observed(
                    receiver.expression(),
                    span,
                    true,
                    mode == "sources_views",
                    observation,
                )?;
                let kind = if matches!(report, SourceMembers::Views(_)) {
                    "views"
                } else {
                    "owned"
                };
                let mut members = Vec::new();
                for index in 0..report.len() {
                    let row = report
                        .get(index)
                        .ok_or_else(|| error(span, "Missing captured source row"))?;
                    members
                        .push(serde_json::to_value(row).map_err(|e| error(span, e.to_string()))?);
                }
                Ok(serde_json::json!({"kind":kind,"members":members}))
            }
            "runtime_owned" => {
                serde_json::to_value(self.runtime_members_observed(receiver, observation)?)
                    .map_err(|e| error(span, e.to_string()))
            }
            "runtime_materialization" => serde_json::to_value(
                self.runtime_members_for_materialization_observed(receiver, observation)?,
            )
            .map_err(|e| error(span, e.to_string())),
            _ => Err(error(span, "Unknown verification-only owner mode")),
        }
    }
    pub(crate) fn legacy_owner_cache_capture(&self) -> serde_json::Value {
        serde_json::json!({
            "erasure":format!("{:?}",self.prepared_erased_signatures.get().map(|v|v.borrow())),
            "ancestry":format!("{:?}",self.prepared_ancestry.get().map(|v|v.borrow())),
            "feature_signatures":format!("{:?}",self.prepared_feature_signatures.borrow())
        })
    }
}
