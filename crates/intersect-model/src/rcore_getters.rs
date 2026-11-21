//! Source getter dispatch for ordinary and prepared language consumers.
//! Retained selectors and schemas remain data; activation checks their sources.
use crate::{
    core::{query::QueryBudget, storage::ModelRead},
    definitions::{
        bundle::{DefinitionBundle, SourceId},
        ecore::ParseLimits,
        model_definition::{CheckedStructure, ModelDefinitionView, StructureSelection},
        preparation::PreparedModelDefinition,
        rcore_behavior::ir_v2::Limits,
        rcore_getter::source_getter_rules,
        schema::{Derivation, Expression, Package, Schema},
    },
    rcore_behavior_functions::{ExecutionContext, ExecutionLimits},
    scope_runtime::{self, ScopeRead},
    semantic_bindings::BorrowedBindingWork,
    source_getters::{SourceGetters, getter_result},
    source_model_reads::SourceModelReads,
};
use serde_json::Value;

/// Freshly compiled getter images for one immutable schema/source selection.
/// No Deserialize or unchecked constructor installs an executable image.
pub struct RcoreGetters<'a> {
    schema: &'a Schema,
    bundle: &'a DefinitionBundle,
    entry: &'a SourceId,
    getters: SourceGetters,
    limits: ExecutionLimits,
}

fn check_selectors(expected: &[Derivation], cached: &[Derivation]) -> Result<(), String> {
    if serde_json::to_value(expected).map_err(|e| e.to_string())?
        != serde_json::to_value(cached).map_err(|e| e.to_string())?
    {
        return Err(
            "Cached source getter selectors differ from the selected authored entry".into(),
        );
    }
    Ok(())
}

impl<'a> RcoreGetters<'a> {
    /// Activate an ordinary retained package. Source structure is rematerialized
    /// before cached schema data can select receivers, ancestry or value codecs.
    pub fn from_package(
        package: &'a Package,
        verification: &Limits,
        limits: ExecutionLimits,
        source_limits: ParseLimits,
    ) -> Result<Self, String> {
        if package.version != crate::definitions::schema::MODEL_PACKAGE_VERSION {
            return Err("Unsupported model metadata package version".into());
        }
        let recipe = package
            .compilation
            .as_ref()
            .ok_or("Source getter activation requires retained compilation sources")?;
        let checked = CheckedStructure::prepare(
            StructureSelection {
                bundle: &recipe.bundle,
                entry: &recipe.structure,
                namespace: &recipe.namespace,
                materialized_types: &recipe.materialized_types,
            },
            source_limits,
        )?;
        Self::from_checked_structure(
            package.model_definition(),
            checked,
            &recipe.behavior,
            verification,
            limits,
            source_limits,
        )
    }

    /// Activate source getters only after the model's own structural compiler
    /// establishes correspondence. The text frontend chooses any canonical
    /// reference context before calling this shared verifier and executor.
    pub fn from_checked_structure(
        definition: ModelDefinitionView<'a>,
        checked: CheckedStructure<'a>,
        behavior: &'a SourceId,
        verification: &Limits,
        limits: ExecutionLimits,
        source_limits: ParseLimits,
    ) -> Result<Self, String> {
        checked.require_schema(definition.schema())?;
        let cached = definition
            .semantics()
            .derived
            .iter()
            .filter(|rule| matches!(rule.expression, Expression::Source { .. }))
            .cloned()
            .collect::<Vec<_>>();
        Self::activate(
            definition.schema(),
            checked.sources(),
            behavior,
            &cached,
            verification,
            limits,
            source_limits,
        )
    }

    /// A prepared definition retains its private completed compiler ownership.
    /// This adds getter execution without converting it to a mutable Package.
    pub fn from_prepared(
        definition: &'a PreparedModelDefinition<'_>,
        verification: &Limits,
        limits: ExecutionLimits,
        source_limits: ParseLimits,
    ) -> Result<Self, String> {
        let cached = definition
            .source_getter_rules()
            .cloned()
            .collect::<Vec<_>>();
        Self::activate(
            definition.schema(),
            definition.sources(),
            definition.behavior_source(),
            &cached,
            verification,
            limits,
            source_limits,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn activate(
        schema: &'a Schema,
        bundle: &'a DefinitionBundle,
        entry: &'a SourceId,
        cached: &[Derivation],
        verification: &Limits,
        limits: ExecutionLimits,
        source_limits: ParseLimits,
    ) -> Result<Self, String> {
        bundle.validate()?;
        let mut sources = bundle.sources.iter().filter(|source| source.id == *entry);
        let source = sources.next().ok_or("Missing selected getter source")?;
        if sources.next().is_some() {
            return Err("Duplicate selected getter source".into());
        }
        let expected = source_getter_rules(source, schema, source_limits)
            .map_err(|errors| format!("Getter selector replay failed: {errors:?}"))?;
        check_selectors(&expected, cached)?;
        if expected.is_empty() {
            return Err("Selected Rcore entry declares no source getters".into());
        }
        let getters =
            SourceGetters::compile_rules(schema, &expected, bundle, verification, source_limits)
                .map_err(|error| error.to_string())?;
        Ok(Self {
            schema,
            bundle,
            entry,
            getters,
            limits,
        })
    }

    pub fn schema(&self) -> &'a Schema {
        self.schema
    }
    pub fn sources(&self) -> &'a DefinitionBundle {
        self.bundle
    }
    pub fn entry(&self) -> &'a SourceId {
        self.entry
    }

    /// Use the same narrow read protocol as reference scopes. A missing entry or
    /// dependency error is final; this entry never delegates to a native getter.
    pub fn read_with(
        &self,
        reads: &dyn ScopeRead,
        owner: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> Result<Value, String> {
        let work = BorrowedBindingWork::bind(budget);
        let observation = work.observer();
        reads.check(&observation)?;
        let actual = reads.class(owner, &observation)?;
        let getter = self.getters.selected(&actual, feature)?;
        let result = scope_runtime::evaluate_getter(
            getter,
            &ExecutionContext::new(self.bundle),
            self.schema,
            reads,
            owner,
            &self.limits,
            &work,
        )
        .map_err(|e| e.to_string())?;
        observation.work(1)?;
        let member = self.schema.feature_by_id(&actual, getter.member())?;
        let result = getter_result(result, member.feature, &observation)?;
        observation.checkpoint()?;
        Ok(result)
    }

    /// Ordinary models use the exact same compiled images and borrowed runner.
    pub fn read_model(
        &self,
        model: &dyn ModelRead,
        owner: &str,
        feature: &str,
        budget: &mut QueryBudget,
    ) -> Result<Value, String> {
        self.read_with(
            &SourceModelReads::for_invocation(model, self.schema),
            owner,
            feature,
            budget,
        )
    }
}
