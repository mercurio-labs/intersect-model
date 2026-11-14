//! Handwritten model-only prepared metadata and inspection. No text admission.
#[path = "prepared_member_inputs.rs"]
mod read_members;
use super::CompletedCompiledSources;
use crate::definitions::{
    bundle::{DefinitionBundle, SourceId},
    ecore_migration::BundleMigrationReport,
    model_definition::StructureSelection,
    resources::DeclarationId,
    schema::{Class, EffectiveFeature, Feature, MemberIdentity, Schema, Semantics},
};
use std::{borrow::Cow, collections::btree_map};

/// A selected rule is not an installed language provider or semantic proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinRuleCoverage {
    Missing,
    CheckedBuiltinDeclaration,
    ExternalDeclarationUninstalled,
    SourceDeclarationUninstalled,
    StoredMetadataOnly,
}

/// Every row remains language-semantically unverified, including non-derived
/// members. Original flags and structural storage metadata are never rewritten.
#[derive(Debug)]
pub enum SemanticRequirement<'a> {
    ClassBehaviorUnverified {
        receiver: &'a str,
        original: Option<&'a DeclarationId>,
        converted: Option<DeclarationId>,
        declared_operations: usize,
        declared_initializers: usize,
    },
    MemberBehaviorUnverified {
        receiver: &'a str,
        owner: &'a str,
        original: Option<&'a DeclarationId>,
        converted: MemberIdentity,
        feature: &'a Feature,
        rule_coverage: BuiltinRuleCoverage,
    },
}

/// Checked producer-owned model metadata and exact sources. This is an
/// inspection view: it installs no behavior and grants no grammar admission,
/// model validity or executable Package conversion. Constructors stay private.
///
/// ```compile_fail
/// use intersect_model::definitions::preparation::PreparedModelDefinition;
/// fn load(input: &str) -> Result<PreparedModelDefinition<'_>, serde_json::Error> {
///     serde_json::from_str(input)
/// }
/// ```
/// ```compile_fail
/// use intersect_model::definitions::{preparation::PreparedModelDefinition, schema::Schema};
/// fn replace(prepared: &mut PreparedModelDefinition<'_>, schema: Schema) {
///     prepared.schema = schema.into();
/// }
/// ```
pub struct PreparedModelDefinition<'inputs> {
    selection: StructureSelection<'inputs>,
    behavior: &'inputs SourceId,
    origin: Option<&'inputs BundleMigrationReport>,
    schema: Cow<'inputs, Schema>,
    semantics: Cow<'inputs, Semantics>,
    pub(super) compiled_sources: Option<CompletedCompiledSources<'inputs>>,
}
impl<'inputs> PreparedModelDefinition<'inputs> {
    /// Copy this host's prepared model into ordinary cache data. Restoring it
    /// uses the explicit native-host trust boundary, not fresh source evidence.
    pub fn save_host_cache(&self) -> crate::definitions::model_definition::SavedModelDefinition {
        crate::definitions::model_definition::SavedModelDefinition::from_prepared(
            self.selection,
            self.behavior,
            &self.schema,
            &self.semantics,
        )
    }
    pub(crate) fn from_bound_origin(
        owner: &'inputs crate::definitions::model_definition::OwnedModelDefinition,
        origin: &'inputs crate::definitions::model_definition::BoundModelOrigin,
    ) -> Result<Self, String> {
        if !origin.belongs_to(owner) {
            return Err("Bound model origin belongs to another source allocation".into());
        }
        let mut prepared = Self::from_owned_model(owner);
        prepared.origin = Some(origin.report());
        Ok(prepared)
    }

    pub(crate) fn from_compiled_result(
        model: crate::definitions::model_definition::CompiledModelDefinition<'inputs>,
    ) -> Self {
        let selection = model.preparation_selection();
        let behavior = model.preparation_behavior();
        let metadata = model.into_metadata();
        Self::from_parts(
            selection,
            behavior,
            None,
            Cow::Owned(metadata.schema),
            Cow::Owned(metadata.semantics),
            None,
        )
    }
    /// Attach only an exact-world origin checked by native migration replay.
    /// No raw report, cached mapping or approval flag supplies this proof.
    pub fn with_origin(mut self, origin: CheckedModelOrigin<'inputs>) -> Result<Self, String> {
        if !std::ptr::eq(self.selection.bundle, origin.bundle) {
            return Err("Checked model origin belongs to another source allocation".into());
        }
        self.origin = Some(origin.origin);
        Ok(self)
    }

    pub(in crate::definitions) fn from_parts(
        selection: StructureSelection<'inputs>,
        behavior: &'inputs SourceId,
        origin: Option<&'inputs BundleMigrationReport>,
        schema: Cow<'inputs, Schema>,
        semantics: Cow<'inputs, Semantics>,
        compiled_sources: Option<CompletedCompiledSources<'inputs>>,
    ) -> Self {
        Self {
            selection,
            behavior,
            origin,
            schema,
            semantics,
            compiled_sources,
        }
    }
    // The private RcorePackage calls this only with its canonical compiled data.
    // No optional optimization receipt is asserted for this cold metadata path.
    pub(crate) fn from_rcore_parts(
        selection: StructureSelection<'inputs>,
        schema: &'inputs Schema,
        semantics: &'inputs Semantics,
    ) -> Self {
        Self::from_parts(
            selection,
            selection.entry,
            None,
            Cow::Borrowed(schema),
            Cow::Borrowed(semantics),
            None,
        )
    }
    #[cfg(test)]
    pub(crate) fn without_completed_compiler_field_data_for_test(mut self) -> Self {
        if let Some(completed) = &mut self.compiled_sources {
            completed.physical_inputs = None;
        }
        self
    }
    #[cfg(test)]
    pub(crate) fn inspection_for_test(
        selection: StructureSelection<'inputs>,
        behavior: &'inputs SourceId,
        origin: &'inputs BundleMigrationReport,
        schema: Schema,
        semantics: Semantics,
    ) -> Self {
        Self::from_parts(
            selection,
            behavior,
            Some(origin),
            Cow::Owned(schema),
            Cow::Owned(semantics),
            None,
        )
    }
    pub(crate) fn from_owned_model(
        owner: &'inputs crate::definitions::model_definition::OwnedModelDefinition,
    ) -> Self {
        Self::from_parts(
            owner.structure_selection(),
            owner.behavior_source(),
            None,
            Cow::Borrowed(owner.schema()),
            Cow::Borrowed(owner.semantics()),
            Some(CompletedCompiledSources::from_owned_model(owner)),
        )
    }
    pub fn structure_selection(&self) -> StructureSelection<'inputs> {
        self.selection
    }
    pub fn behavior_source(&self) -> &'inputs SourceId {
        self.behavior
    }
    pub fn sources(&self) -> &'inputs DefinitionBundle {
        self.selection.bundle
    }
    pub fn original_sources(&self) -> &'inputs DefinitionBundle {
        self.origin
            .map_or(self.selection.bundle, |origin| &origin.original)
    }
    pub fn migration(&self) -> Option<&'inputs BundleMigrationReport> {
        self.origin
    }
    // Private immutable DATA access from this actual prepared owner, before
    // physical metadata admission. No model/provider/constructor grant follows.
    pub(crate) fn compiled_sources(&self) -> Option<&CompletedCompiledSources<'inputs>> {
        self.compiled_sources.as_ref()
    }
    pub fn schema(&self) -> &Schema {
        &self.schema
    }
    /// Declaration metadata only; this does not install source scope execution.
    pub fn has_source_scope_bindings(&self) -> bool {
        !self.semantics.scope_bindings.is_empty()
    }
    /// Cached selector data only; fresh checking must install executable getters.
    pub fn source_getter_rules(
        &self,
    ) -> impl Iterator<Item = &crate::definitions::schema::Derivation> {
        self.semantics.derived.iter().filter(|rule| {
            matches!(
                rule.expression,
                crate::definitions::schema::Expression::Source { .. }
            )
        })
    }
    /// Explicit immutable SOURCE metadata admission. Work belongs to this
    /// operation's observer; consumers must not reset an exhausted request to
    /// prepare it. This installs no providers and validates no model values.
    pub fn prepare_physical_metadata<'definition>(
        &'definition self,
        observation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<PreparedPhysicalMetadata<'definition>, crate::definitions::schema::PreparationError>
    {
        let checked = if self.compiled_sources.is_some() {
            // Reuse only this compiler-owned metadata's actual canonical inputs.
            crate::definitions::schema::ValidatedRuntimeTypes::prepare_with_physical_inputs_observed(
                self.schema(),
                observation,
            )?
        } else {
            crate::definitions::schema::ValidatedRuntimeTypes::prepare_observed(
                self.schema(),
                observation,
            )?
        };
        Ok(PreparedPhysicalMetadata {
            definition: self,
            checked,
        })
    }

    /// Fallible per-receiver inspection over existing effective-member services.
    /// No expanded whole-schema requirements table is retained. Existing legacy
    /// preparation/traversal costs remain unqualified parent-S09 obligations.
    pub fn requirements(&self) -> SemanticRequirements<'_> {
        SemanticRequirements {
            definition: self,
            classes: self.schema.classes.iter(),
            current: None,
        }
    }
    /// Canonical fallible class stream. Every class header validates its full
    /// effective inventory; consumers may omit only infallible nonemitting rows.
    /// This grants no model/read/provider or semantic-validation authority.
    pub fn requirement_classes(&self) -> SemanticRequirementClasses<'_> {
        SemanticRequirementClasses {
            definition: self,
            classes: self.schema.classes.iter(),
        }
    }
    fn requirement_member<'a>(
        &'a self,
        receiver: &'a str,
        member: EffectiveFeature<'a>,
    ) -> SemanticRequirement<'a> {
        let original = match &member.identity {
            MemberIdentity::Source { declaration } => self.original_declaration(declaration),
            MemberIdentity::Programmatic { .. } => None,
        };
        let rule_coverage = if !member.feature.derived {
            BuiltinRuleCoverage::StoredMetadataOnly
        } else {
            match self.semantics.derived.iter().find(|rule| {
                rule.feature == member.feature.name && self.schema.conforms(receiver, &rule.class)
            }) {
                None => BuiltinRuleCoverage::Missing,
                Some(rule)
                    if matches!(
                        rule.expression,
                        crate::definitions::schema::Expression::Source { .. }
                    ) =>
                {
                    BuiltinRuleCoverage::SourceDeclarationUninstalled
                }
                Some(rule)
                    if matches!(
                        rule.expression,
                        crate::definitions::schema::Expression::External { .. }
                    ) =>
                {
                    BuiltinRuleCoverage::ExternalDeclarationUninstalled
                }
                Some(_) => BuiltinRuleCoverage::CheckedBuiltinDeclaration,
            }
        };
        SemanticRequirement::MemberBehaviorUnverified {
            receiver,
            owner: member.owner,
            original,
            converted: member.identity,
            feature: member.feature,
            rule_coverage,
        }
    }
    fn requirement_class<'a>(
        &'a self,
        receiver: &'a str,
        class: &'a Class,
    ) -> Result<(SemanticRequirement<'a>, Vec<EffectiveFeature<'a>>), String> {
        let members = self.schema.effective_features(receiver)?;
        let converted = self
            .schema
            .declarations
            .get(receiver)
            .map(|declaration| DeclarationId {
                source: declaration.source.clone(),
                path: declaration.path.clone(),
            });
        let original = converted
            .as_ref()
            .and_then(|id| self.original_declaration(id));
        let header = SemanticRequirement::ClassBehaviorUnverified {
            receiver,
            original,
            converted,
            declared_operations: class.operations.len(),
            declared_initializers: self
                .semantics
                .initializers
                .iter()
                .filter(|rule| self.schema.conforms(receiver, &rule.class))
                .count(),
        };
        Ok((header, members))
    }
    fn original_declaration(&self, converted: &DeclarationId) -> Option<&DeclarationId> {
        self.origin?
            .reports
            .iter()
            .flat_map(|report| &report.correspondence)
            .find(|row| row.converted == *converted)
            .map(|row| &row.original)
    }
}

/// Completed shared runtime metadata validation over one live prepared source.
/// This authority cannot be deserialized, asserted from a hash, moved to a raw
/// schema, or converted to executable Package/model/provider authority.
///
/// ```compile_fail
/// use intersect_model::definitions::{preparation::PreparedPhysicalMetadata, schema::Schema};
/// fn asserted<'a>(schema: &'a Schema) -> PreparedPhysicalMetadata<'a> { schema.into() }
/// ```
pub struct PreparedPhysicalMetadata<'definition> {
    definition: &'definition PreparedModelDefinition<'definition>,
    checked: crate::definitions::schema::ValidatedRuntimeTypes<'definition, String>,
}
impl<'definition> PreparedPhysicalMetadata<'definition> {
    pub fn definition(&self) -> &'definition PreparedModelDefinition<'definition> {
        self.definition
    }
    pub(crate) fn compiled_sources(&self) -> Option<&CompletedCompiledSources<'definition>> {
        self.definition.compiled_sources()
    }
    pub(crate) fn frozen_feature_inputs(
        &self,
        schema: &Schema,
        receiver: &str,
        observation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Option<std::sync::Arc<crate::definitions::schema::PreparedFeatureInputs>>, String>
    {
        if !self.matches_schema(schema) {
            return Err("Frozen physical metadata/schema owner changed".into());
        }
        // The final runtime-type validation above remains mandatory. This is
        // reuse of pure SOURCE-identity map DATA already completed by the same
        // ordinary compiler owner; it grants no reference/provider semantics.
        if let Some(completed) = self.compiled_sources()
            && let Some(fields) =
                completed.physical_fields_observed(schema, receiver, observation)?
        {
            return Ok(Some(fields));
        }
        self.checked
            .physical_fields_observed(schema, receiver, observation)
    }
    pub(crate) fn frozen_feature_input_cache(
        &self,
        schema: &Schema,
        observation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Option<crate::definitions::schema::SharedFeatureInputCache>, String> {
        if !self.matches_schema(schema) {
            return Err("Frozen physical metadata/schema owner changed".into());
        }
        if let Some(completed) = self.compiled_sources()
            && let Some(cache) = completed.physical_cache_observed(schema, observation)?
        {
            return Ok(Some(cache));
        }
        self.checked.physical_cache_observed(schema, observation)
    }
    // Pure canonical DATA: no getter/model/source authority follows. Optional
    // rows are already complete; missing DATA keeps the literal selector.
    pub(crate) fn semantic_feature_observed<'schema>(
        &self,
        schema: &'schema Schema,
        receiver: &str,
        name: &str,
        observation: &crate::definitions::conversion::InvocationContext<'_>,
    ) -> Result<Option<crate::definitions::schema::EffectiveFeature<'schema>>, String> {
        read_members::named(self, schema, receiver, name, observation)
    }
    pub(crate) fn matches_schema(&self, schema: &Schema) -> bool {
        self.checked.belongs_to(schema) && std::ptr::eq(self.definition.schema(), schema)
    }
}

pub struct SemanticRequirements<'a> {
    definition: &'a PreparedModelDefinition<'a>,
    classes: btree_map::Iter<'a, String, Class>,
    current: Option<(&'a str, std::vec::IntoIter<EffectiveFeature<'a>>)>,
}
impl<'a> Iterator for SemanticRequirements<'a> {
    type Item = Result<SemanticRequirement<'a>, String>;
    fn next(&mut self) -> Option<Self::Item> {
        if let Some((receiver, members)) = &mut self.current
            && let Some(member) = members.next()
        {
            return Some(Ok(self.definition.requirement_member(receiver, member)));
        }
        let (receiver, class) = self.classes.next()?;
        match self.definition.requirement_class(receiver, class) {
            Ok((header, members)) => {
                self.current = Some((receiver, members.into_iter()));
                Some(Ok(header))
            }
            Err(error) => {
                self.current = None;
                Some(Err(error))
            }
        }
    }
}

/// One fully checked effective class inventory from the canonical source path.
/// Header/member order matches SemanticRequirements; no expanded table is held.
pub struct SemanticRequirementClass<'a> {
    definition: &'a PreparedModelDefinition<'a>,
    receiver: &'a str,
    header: SemanticRequirement<'a>,
    members: std::vec::IntoIter<EffectiveFeature<'a>>,
}
impl<'a> SemanticRequirementClass<'a> {
    pub fn receiver(&self) -> &'a str {
        self.receiver
    }
    pub fn header(&self) -> &SemanticRequirement<'a> {
        &self.header
    }
    pub fn next_member(&mut self) -> Option<SemanticRequirement<'a>> {
        self.members
            .next()
            .map(|member| self.definition.requirement_member(self.receiver, member))
    }
}
pub struct SemanticRequirementClasses<'a> {
    definition: &'a PreparedModelDefinition<'a>,
    classes: btree_map::Iter<'a, String, Class>,
}
impl<'a> Iterator for SemanticRequirementClasses<'a> {
    type Item = Result<SemanticRequirementClass<'a>, String>;
    fn next(&mut self) -> Option<Self::Item> {
        let (receiver, class) = self.classes.next()?;
        Some(
            self.definition
                .requirement_class(receiver, class)
                .map(|(header, members)| SemanticRequirementClass {
                    definition: self.definition,
                    receiver,
                    header,
                    members: members.into_iter(),
                }),
        )
    }
}

/// Native physical-persistence identity inputs. Extension data is observational
/// data, never grammar admission or a model/provider execution grant. A language
/// supplies its actual private context; model-only callers use the null context.
/// The prepared model remains producer-owned and cannot be built from this trait.
pub trait PreparedPersistenceContext {
    type ContextData<'a>: serde::Serialize
    where
        Self: 'a;
    fn model_preparation(&self) -> &PreparedModelDefinition<'_>;
    fn context_data(&self) -> Self::ContextData<'_>;
}
impl PreparedPersistenceContext for PreparedModelDefinition<'_> {
    type ContextData<'a>
        = Option<()>
    where
        Self: 'a;
    fn model_preparation(&self) -> &PreparedModelDefinition<'_> {
        self
    }
    fn context_data(&self) -> Self::ContextData<'_> {
        None
    }
}

/// Fresh native migration correspondence for one exact converted source world.
/// This proves source origin only; it does not admit a grammar or a model value.
///
/// ```compile_fail
/// use intersect_model::definitions::preparation::CheckedModelOrigin;
/// fn load(input: &str) -> Result<CheckedModelOrigin<'_>, serde_json::Error> {
///     serde_json::from_str(input)
/// }
/// ```
pub struct CheckedModelOrigin<'inputs> {
    bundle: &'inputs DefinitionBundle,
    origin: &'inputs BundleMigrationReport,
}
impl<'inputs> CheckedModelOrigin<'inputs> {
    pub fn check(
        bundle: &'inputs DefinitionBundle,
        origin: &'inputs BundleMigrationReport,
        limits: crate::definitions::ecore::ParseLimits,
    ) -> Result<Self, String> {
        origin.original.validate()?;
        if origin.version != 1 || origin.converted.as_ref() != Some(bundle) {
            return Err("Preparation requires the exact converted migration bundle".into());
        }
        let targets = origin
            .reports
            .iter()
            .map(|report| {
                report
                    .converted
                    .as_ref()
                    .map(|source| (report.original.id.clone(), source.id.uri.clone()))
                    .ok_or_else(|| "Migration has blocking or missing converted sources".to_owned())
            })
            .collect::<Result<std::collections::BTreeMap<_, _>, _>>()?;
        let replay = crate::definitions::ecore_migration::migrate_ecore_bundle_with_limits(
            origin.original.clone(),
            &targets,
            limits,
        )
        .map_err(|error| error.to_string())?;
        if replay != *origin {
            return Err("Migration source/correspondence replay differs".into());
        }
        Ok(Self { bundle, origin })
    }
}
