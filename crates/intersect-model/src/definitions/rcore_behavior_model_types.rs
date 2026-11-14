//! Rcore typing adapter over the shared Ecore schema, without a second graph.
use super::types::{CollectionKind, Scalar, Type, TypeRef, TypeTable};
use crate::definitions::{
    bundle::DefinitionBundle,
    resources::{DeclarationId, ResourceSet},
    schema::Schema,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
type CheckedSchema = crate::definitions::schema::CheckedRuntimeSchema;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FeatureType {
    pub declaration: DeclarationId,
    pub schema_revision: String,
    pub receiver: crate::definitions::generic_types::RuntimeType,
    pub owner: crate::definitions::generic_types::RuntimeType,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub domains: Vec<crate::definitions::schema::CheckedRuntimeFeatureDomain>,
    /// False means the read type is not a certificate for a value written back.
    pub assignment_exact: bool,
    pub value: TypeRef,
    /// A typed read is not evidence that a derived provider exists or executes.
    pub derived: bool,
    pub changeable: bool,
    pub transient: bool,
    pub volatile: bool,
    pub unsettable: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FeatureProjection {
    pub declaration: DeclarationId,
    pub receiver: crate::definitions::generic_types::RuntimeType,
    pub owner: crate::definitions::generic_types::RuntimeType,
    pub schema_revision: String,
    pub domains: Vec<crate::definitions::schema::CheckedRuntimeFeatureDomain>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModelDomainContext {
    pub schema_revision: String,
    pub domains: Vec<crate::definitions::schema::CheckedRuntimeFeatureDomain>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SymbolicFeatureType {
    pub declaration: DeclarationId,
    pub parents: Vec<crate::definitions::generic_types::Type>,
    pub bounds: Vec<crate::definitions::generic_resolution::BoundCheck>,
    pub receiver: crate::definitions::generic_types::Type,
    pub owner: crate::definitions::generic_types::Type,
    pub value: TypeRef,
    pub derived: bool,
    pub changeable: bool,
    pub transient: bool,
    pub volatile: bool,
    pub unsettable: bool,
    pub reference: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct SymbolicApplicationCheck {
    pub expression: crate::definitions::generic_types::Type,
    pub bounds: Vec<crate::definitions::generic_resolution::BoundCheck>,
}

pub(crate) type NominalInferencePair = (TypeRef, TypeRef);

#[derive(Clone)]
pub struct ModelTypes<'a> {
    source_limits: crate::definitions::ecore::ParseLimits,
    bundle: &'a DefinitionBundle,
    conversions: Option<&'a crate::definitions::conversion::ConversionRegistry<'a>>,
    resources: ResourceSet,
    symbolic_resolver: Option<crate::definitions::generic_resolution::Resolver>,
    symbolic_parameters: BTreeMap<crate::definitions::generic_types::RigidParameterId, TypeRef>,
    symbolic_checks: BTreeMap<crate::definitions::generic_types::Type, SymbolicApplicationCheck>,
    enumerations: BTreeMap<DeclarationId, crate::definitions::enums::Enumeration>,
    applied_datatypes:
        BTreeMap<crate::definitions::generic_types::Type, crate::definitions::datatypes::Datatype>,
    datatype_checks: BTreeMap<
        crate::definitions::generic_types::Type,
        crate::definitions::generic_resolution::CheckedDatatypeApplication,
    >,
    standard_datatypes: BTreeMap<DeclarationId, crate::definitions::datatypes::Datatype>,
    // Every schema here is a checked compiler output: class-key lookups use its
    // immutable validation authority instead of re-validating per lookup. The
    // `Arc` lets trial copies and shared stores reuse one owned schema.
    runtime_schemas: BTreeMap<Vec<crate::definitions::generic_types::Type>, Arc<CheckedSchema>>,
    // Runtime-schema preparation caches, each a pure function of this
    // context's fixed bundle, resources and conversions: the model closure of
    // each single requested type, and the `runtime_schemas` key materialized
    // from each exact closure. Without them every distinct type re-walked and
    // re-materialized a closure that usually spans most of the metamodel.
    runtime_closures: BTreeMap<
        crate::definitions::generic_types::Type,
        BTreeSet<crate::definitions::generic_types::Type>,
    >,
    materialized: BTreeMap<
        Vec<crate::definitions::generic_types::Type>,
        Vec<crate::definitions::generic_types::Type>,
    >,
    // `conforms_application` results by (actual, expected), likewise fixed
    // per context once both schemas are materialized.
    conformance: BTreeMap<
        (
            crate::definitions::generic_types::Type,
            crate::definitions::generic_types::Type,
        ),
        bool,
    >,
    generic_checks: BTreeMap<
        crate::definitions::generic_types::Type,
        crate::definitions::generic_resolution::RuntimeMembers,
    >,
    // Completed preparations shared with other contexts over the same sources
    // and limits. Like `bundle`, it is shared rather than owned by trial copies.
    schema_store: Option<RuntimeSchemaStore>,
    // Resolved runtime operation sets by receiver, shared by this context's
    // unconverted schema materializations (each otherwise re-resolved every
    // receiver of its closure). Pure in the context's fixed inputs.
    runtime_operations: crate::definitions::generic_resolution::RuntimeOperationCache,
}
impl<'a> ModelTypes<'a> {
    /// Share completed runtime-schema preparations with other contexts through
    /// `store`. Contexts with a conversion registry, or whose sources or limits
    /// differ from those the store is bound to, keep preparing on their own.
    /// Reports from this context are unchanged: an imported preparation adds
    /// exactly what preparing it here would have added.
    pub fn with_schema_store(mut self, store: &RuntimeSchemaStore) -> Self {
        self.attach_schema_store(Some(store));
        self
    }
    pub(crate) fn attach_schema_store(&mut self, store: Option<&RuntimeSchemaStore>) {
        self.schema_store = store
            .filter(|store| {
                self.conversions.is_none() && store.admits(self.bundle, self.source_limits)
            })
            .cloned();
    }
    /// Source spelling for inspection; runtime identity remains the exact type.
    pub(crate) fn classifier_name(
        &self,
        ty: &crate::definitions::generic_types::RuntimeType,
    ) -> Result<&str, String> {
        let crate::definitions::generic_types::Type::Classifier { declaration, .. } =
            ty.expression()
        else {
            return Err("Expected a model classifier".into());
        };
        self.resources
            .node(declaration)?
            .attributes
            .iter()
            .find(|attribute| attribute.name.namespace.is_none() && attribute.name.local == "name")
            .map(|attribute| attribute.value.as_str())
            .ok_or_else(|| "Missing model classifier name".into())
    }
    /// Select a concrete feature without granting stored or derived execution.
    pub(crate) fn navigation_feature(
        &mut self,
        table: &mut TypeTable,
        receiver: TypeRef,
        name: &str,
    ) -> Result<FeatureType, String> {
        let field = match table.get(receiver)?.clone() {
            Type::Model(class) => self.feature(table, &class, name)?,
            Type::ModelApplication(application) => {
                self.feature_application(table, &application, name)?
            }
            _ => return Err("Stored feature requires a concrete model receiver".into()),
        };
        let value = match table.get(field.value)? {
            Type::Option(inner) | Type::Many { element: inner, .. } => *inner,
            _ => field.value,
        };
        if !matches!(
            table.get(value)?,
            Type::Scalar(_)
                | Type::Enumeration(_)
                | Type::Datatype(_)
                | Type::DatatypeApplication(_)
                | Type::Model(_)
                | Type::ModelApplication(_)
        ) {
            return Err("Unsupported stored feature element type".into());
        }
        Ok(field)
    }

    /// Stored reads share exact nominal validation and paged Many transport.
    pub(crate) fn stored_feature(
        &mut self,
        table: &mut TypeTable,
        receiver: TypeRef,
        name: &str,
    ) -> Result<FeatureType, String> {
        let field = self.navigation_feature(table, receiver, name)?;
        if field.derived {
            return Err("Derived model navigation requires an executable provider".into());
        }
        Ok(field)
    }

    /// Select the exact stored reference used by a typed inverse query.
    pub(crate) fn incoming_feature(
        &mut self,
        table: &mut TypeTable,
        source: TypeRef,
        target: TypeRef,
        name: &str,
    ) -> Result<FeatureType, String> {
        let field = self.stored_feature(table, source, name)?;
        let element = match table.get(field.value)? {
            Type::Option(inner) | Type::Many { element: inner, .. } => *inner,
            _ => field.value,
        };
        if !matches!(
            table.get(element)?,
            Type::Model(_) | Type::ModelApplication(_)
        ) {
            return Err("incoming requires a stored reference member".into());
        }
        if !matches!(
            table.get(target)?,
            Type::Model(_) | Type::ModelApplication(_)
        ) || !self.assignable(table, target, element)?
        {
            return Err("incoming target does not conform to the reference domain".into());
        }
        Ok(field)
    }

    /// Charge a bounded representation of every owned cache before a trial copy.
    /// Shared bundle/provider references are immutable and are not cloned.
    pub(crate) fn trial_clone(&self, remaining: &mut usize) -> Result<Self, String> {
        super::checking::snapshot_charge(remaining, &self.resources)?;
        super::checking::snapshot_charge(remaining, &self.symbolic_resolver)?;
        super::checking::snapshot_charge(remaining, &self.symbolic_parameters)?;
        super::checking::snapshot_charge(remaining, &self.symbolic_checks)?;
        super::checking::snapshot_charge(remaining, &self.enumerations)?;
        super::checking::snapshot_charge(remaining, &self.applied_datatypes)?;
        super::checking::snapshot_charge(remaining, &self.datatype_checks)?;
        super::checking::snapshot_charge(remaining, &self.standard_datatypes)?;
        super::checking::snapshot_charge(remaining, &self.runtime_schemas)?;
        super::checking::snapshot_charge(remaining, &self.runtime_closures)?;
        super::checking::snapshot_charge(remaining, &self.materialized)?;
        super::checking::snapshot_charge(remaining, &self.conformance)?;
        super::checking::snapshot_charge(remaining, &self.generic_checks)?;
        Ok(self.clone())
    }

    pub(crate) fn configure_symbolic(
        &mut self,
        signatures: &super::signatures::Signatures,
    ) -> Result<(), crate::definitions::ecore::ImportError> {
        let entries = super::symbolic::entries(signatures)?;
        for function in &signatures.functions {
            for (parameter, generic) in function.generics.iter().enumerate() {
                let identity = crate::definitions::generic_types::RigidParameterId {
                    source: function.symbol.source.clone(),
                    declaration: function.symbol.declaration,
                    parameter,
                };
                self.symbolic_parameters.insert(identity, generic.ty);
            }
        }
        let resolver = self.fresh_resolver()?;
        let resolver = match self.conversions {
            Some(registry) => resolver.with_conversions(registry)?,
            None => resolver,
        };
        self.symbolic_resolver = Some(resolver.with_rigid_parameters(entries)?);
        Ok(())
    }
    pub fn symbolic_checks(&self) -> Vec<SymbolicApplicationCheck> {
        self.symbolic_checks.values().cloned().collect()
    }
    pub(crate) fn check_symbolic(
        &mut self,
        expression: &crate::definitions::generic_types::Type,
    ) -> Result<(), String> {
        let bounds = self
            .symbolic_resolver
            .as_ref()
            .ok_or("Missing source-owned symbolic context")?
            .symbolic_application(expression)
            .map_err(|e| e.to_string())?;
        self.symbolic_checks.insert(
            expression.clone(),
            SymbolicApplicationCheck {
                expression: expression.clone(),
                bounds,
            },
        );
        Ok(())
    }
    pub(crate) fn application_type(
        &mut self,
        table: &TypeTable,
        declaration: DeclarationId,
        arguments: Vec<TypeRef>,
        datatype: bool,
    ) -> Result<Type, String> {
        let expression = crate::definitions::generic_types::Type::Classifier {
            declaration: declaration.clone(),
            raw: false,
            arguments: arguments
                .iter()
                .map(|ty| super::symbolic::expression(table, *ty))
                .collect::<Result<_, _>>()?,
        };
        let symbolic = arguments
            .iter()
            .map(|ty| super::symbolic::is_symbolic(table, *ty))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .any(|value| value);
        if symbolic {
            self.check_symbolic(&expression)?;
            Ok(if datatype {
                Type::SymbolicDatatypeApplication {
                    declaration,
                    arguments,
                }
            } else {
                Type::SymbolicModelApplication {
                    declaration,
                    arguments,
                }
            })
        } else if datatype {
            self.applied_datatype(&expression)?;
            Ok(Type::DatatypeApplication(expression))
        } else {
            self.conforms_application(&expression, &expression)?;
            Ok(Type::ModelApplication(expression))
        }
    }
    pub(crate) fn symbolic_assignable(
        &mut self,
        table: &TypeTable,
        actual: TypeRef,
        expected: TypeRef,
    ) -> Result<bool, String> {
        let nominal = |ty: &Type| {
            matches!(
                ty,
                Type::Parameter { .. }
                    | Type::Model(_)
                    | Type::ModelApplication(_)
                    | Type::Datatype(_)
                    | Type::DatatypeApplication(_)
                    | Type::Enumeration(_)
                    | Type::SymbolicModelApplication { .. }
                    | Type::SymbolicDatatypeApplication { .. }
            )
        };
        if !nominal(table.get(actual)?) || !nominal(table.get(expected)?) {
            return Ok(false);
        }
        let a = super::symbolic::expression(table, actual)?;
        let b = super::symbolic::expression(table, expected)?;
        self.symbolic_resolver
            .as_ref()
            .ok_or("Missing source-owned symbolic context")?
            .symbolic_value_conforms(&a, &b)
            .map_err(|e| e.to_string())
    }
    pub(crate) fn type_from_expression(
        &mut self,
        table: &mut TypeTable,
        expression: &crate::definitions::generic_types::Type,
    ) -> Result<TypeRef, String> {
        self.nominal_expression_type(table, expression, false)
    }
    fn nominal_expression_type(
        &mut self,
        table: &mut TypeTable,
        expression: &crate::definitions::generic_types::Type,
        argument: bool,
    ) -> Result<TypeRef, String> {
        fn lower(
            models: &mut ModelTypes<'_>,
            table: &mut TypeTable,
            expression: &crate::definitions::generic_types::Type,
            depth: usize,
            argument: bool,
        ) -> Result<TypeRef, String> {
            if depth > 64 {
                return Err("Symbolic member type nesting exceeds 64 levels".into());
            }
            match expression {
                crate::definitions::generic_types::Type::RigidParameter { identity } => models
                    .symbolic_parameters
                    .get(identity)
                    .copied()
                    .ok_or_else(|| "Missing exact Rcore rigid parameter identity".into()),
                crate::definitions::generic_types::Type::Classifier {
                    declaration,
                    arguments,
                    raw,
                } => {
                    let standard_resources;
                    let node = if declaration.source
                        == *crate::definitions::compiler::standard_datatype_source_id()
                    {
                        standard_resources = ResourceSet::new(&DefinitionBundle {
                            version: 1,
                            sources: vec![crate::definitions::compiler::standard_datatype_source()],
                            namespaces: vec![],
                        })?;
                        standard_resources.node(declaration)?
                    } else {
                        models.resources.node(declaration)?
                    };
                    let kind = node
                        .attributes
                        .iter()
                        .find_map(|a| {
                            if a.name.local == "type" {
                                a.type_name.as_ref().map(|name| name.local.clone())
                            } else {
                                None
                            }
                        })
                        .ok_or("Missing nominal classifier kind")?;
                    let name = node
                        .attributes
                        .iter()
                        .find(|a| a.name.local == "name")
                        .map(|a| a.value.clone())
                        .ok_or("Missing classifier name")?;
                    let arguments = arguments
                        .iter()
                        .map(|arg| lower(models, table, arg, depth + 1, true))
                        .collect::<Result<Vec<_>, _>>()?;
                    if !arguments.is_empty() {
                        let ty = models.application_type(
                            table,
                            declaration.clone(),
                            arguments,
                            kind == "EDataType",
                        )?;
                        return table.intern(ty);
                    }
                    let ty = match kind.as_str() {
                        "EClass" => {
                            if *raw {
                                Type::ModelApplication(expression.clone())
                            } else {
                                Type::Model(declaration.clone())
                            }
                        }
                        "EEnum" => Type::Enumeration(declaration.clone()),
                        "EDataType" => {
                            // Builtin feature domains retain the same scalar representation as concrete reads.
                            let scalar = if !argument
                                && declaration.source
                                    == *crate::definitions::compiler::standard_datatype_source_id()
                            {
                                match name.as_str() {
                                    "EString" => Some(Scalar::String),
                                    "EByteArray" => Some(Scalar::Bytes),
                                    "EChar" | "ECharacterObject" => Some(Scalar::Char16),
                                    "EDate" => Some(Scalar::DateInstant),
                                    "EBigInteger" => Some(Scalar::BigInt),
                                    "EBigDecimal" => Some(Scalar::Decimal),
                                    "EBoolean" | "EBooleanObject" => Some(Scalar::Bool),
                                    "EByte" | "EByteObject" => Some(Scalar::Int8),
                                    "EShort" | "EShortObject" => Some(Scalar::Int16),
                                    "EInt" | "EIntegerObject" => Some(Scalar::Int32),
                                    "ELong" | "ELongObject" => Some(Scalar::Int64),
                                    "EFloat" | "EFloatObject" => Some(Scalar::Float32),
                                    "EDouble" | "EDoubleObject" => Some(Scalar::Float64),
                                    _ => None,
                                }
                            } else {
                                None
                            };
                            if let Some(scalar) = scalar {
                                Type::Scalar(scalar)
                            } else {
                                models.datatype(declaration)?;
                                Type::Datatype(declaration.clone())
                            }
                        }
                        _ => return Err("Unsupported nominal classifier kind".into()),
                    };
                    table.intern(ty)
                }
                _ => Err("Symbolic member retains an unsupported open Ecore type".into()),
            }
        }
        lower(self, table, expression, 0, argument)
    }
    pub(crate) fn symbolic_value_argument_equal(
        &self,
        table: &TypeTable,
        actual: TypeRef,
        expected: TypeRef,
    ) -> Result<bool, String> {
        let actual = super::symbolic::expression(table, actual)?;
        let expected = super::symbolic::expression(table, expected)?;
        self.symbolic_resolver
            .as_ref()
            .ok_or("Missing source-owned symbolic context")?
            .symbolic_value_argument_equal(&actual, &expected)
            .map_err(|error| error.to_string())
    }
    pub(crate) fn symbolic_inference_pairs(
        &mut self,
        table: &mut TypeTable,
        template: TypeRef,
        actual: TypeRef,
        outward: bool,
    ) -> Result<Option<Vec<NominalInferencePair>>, String> {
        if !matches!(
            table.get(template)?,
            Type::SymbolicModelApplication { .. } | Type::SymbolicDatatypeApplication { .. }
        ) {
            return Ok(None);
        }
        let template_expression = super::symbolic::expression(table, template)?;
        let actual_expression = match table.get(actual)? {
            Type::Model(_)
            | Type::ModelApplication(_)
            | Type::Datatype(_)
            | Type::DatatypeApplication(_)
            | Type::Enumeration(_)
            | Type::SymbolicModelApplication { .. }
            | Type::SymbolicDatatypeApplication { .. } => {
                super::symbolic::expression(table, actual)?
            }
            _ => return Ok(None),
        };
        let resolver = self
            .symbolic_resolver
            .as_ref()
            .ok_or("Missing source-owned symbolic context")?;
        let (projected, other) = if outward {
            let crate::definitions::generic_types::Type::Classifier { declaration, .. } =
                &actual_expression
            else {
                return Ok(None);
            };
            (
                resolver
                    .symbolic_project_as(&template_expression, declaration)
                    .map_err(|e| e.to_string())?,
                actual_expression,
            )
        } else {
            let crate::definitions::generic_types::Type::Classifier { declaration, .. } =
                &template_expression
            else {
                return Ok(None);
            };
            (
                resolver
                    .symbolic_project_as(&actual_expression, declaration)
                    .map_err(|e| e.to_string())?,
                template_expression,
            )
        };
        let Some(projected) = projected else {
            return Ok(None);
        };
        let (left, right) = if outward {
            (projected, other)
        } else {
            (other, projected)
        };
        let (
            crate::definitions::generic_types::Type::Classifier { arguments: a, .. },
            crate::definitions::generic_types::Type::Classifier { arguments: b, .. },
        ) = (left, right)
        else {
            return Ok(None);
        };
        if a.len() != b.len() {
            return Ok(None);
        }
        let mut relations = Vec::new();
        for (a, b) in a.iter().zip(&b) {
            let a = self.nominal_expression_type(table, a, true)?;
            let b = self.nominal_expression_type(table, b, true)?;
            // The inference engine tags these as nominal-invariant constraints.
            // Shared invariant matching, not value covariance or retagging,
            // validates them after exact argument inference.
            relations.push((a, b));
        }
        Ok(Some(relations))
    }
    pub(crate) fn symbolic_feature(
        &mut self,
        table: &mut TypeTable,
        receiver: TypeRef,
        name: &str,
    ) -> Result<SymbolicFeatureType, String> {
        let receiver = super::symbolic::expression(table, receiver)?;
        self.check_symbolic(&receiver)?;
        let feature = self
            .symbolic_resolver
            .as_ref()
            .ok_or("Missing source-owned symbolic context")?
            .symbolic_feature(&receiver, name)
            .map_err(|e| e.to_string())?;
        let target = feature
            .member
            .signatures
            .iter()
            .filter(|signature| matches!(signature.feature.as_str(), "eType" | "eGenericType"))
            .flat_map(|signature| signature.specialized.iter())
            .next()
            .ok_or("Missing symbolic feature target")?;
        let element = self.type_from_expression(table, target)?;
        let value = if feature.upper == 1 {
            if feature.lower == 0 {
                table.intern(Type::Option(element))?
            } else {
                element
            }
        } else {
            table.intern(Type::Many {
                element,
                lower: feature.lower,
                upper: feature.upper,
                ordered: feature.ordered,
                unique: feature.unique,
            })?
        };
        Ok(SymbolicFeatureType {
            declaration: feature.member.declaration,
            receiver: feature.receiver,
            parents: feature.parents,
            bounds: feature.bounds,
            owner: feature.member.owner,
            value,
            derived: feature.derived,
            changeable: feature.changeable,
            transient: feature.transient,
            volatile: feature.volatile,
            unsettable: feature.unsettable,
            reference: feature.reference,
        })
    }

    /// Shared projections retain exact source spans, checked bounds and raw warnings.
    pub fn generic_checks(&self) -> Vec<crate::definitions::generic_resolution::RuntimeMembers> {
        self.generic_checks.values().cloned().collect()
    }

    /// Domain alternatives are scoped to the checked materialization revision.
    pub fn domain_contexts(&self) -> Vec<ModelDomainContext> {
        self.runtime_schemas
            .values()
            .map(|schema| schema.schema())
            .filter(|schema| !schema.runtime_feature_domains.is_empty())
            .map(|schema| ModelDomainContext {
                schema_revision: schema.revision.clone(),
                domains: schema
                    .runtime_feature_domains
                    .values()
                    .flatten()
                    .cloned()
                    .collect(),
            })
            .collect()
    }
    /// Expose checked source/domain evidence even when Rcore cannot type its value.
    pub fn feature_projection(
        &mut self,
        expression: &crate::definitions::generic_types::Type,
        name: &str,
    ) -> Result<FeatureProjection, String> {
        let runtime = crate::definitions::generic_types::RuntimeType::new(expression.clone())?;
        let schema = self.runtime_schema(std::slice::from_ref(expression))?;
        project_feature(schema, &runtime, name)
    }

    pub(crate) fn runtime_schema(
        &mut self,
        expressions: &[crate::definitions::generic_types::Type],
    ) -> Result<&CheckedSchema, String> {
        let context = crate::definitions::conversion::InvocationContext::default();
        let mut preparation = RuntimeSchemaPreparation::begin(self, expressions, &context)
            .map_err(|error| error.to_string())?;
        while !preparation
            .advance(self, &context)
            .map_err(|error| error.to_string())?
        {}
        let key = preparation.finish().map_err(|error| error.to_string())?;
        self.runtime_schemas
            .get(&key)
            .map(|schema| schema.as_ref())
            .ok_or_else(|| "Missing checked model application context".into())
    }

    pub fn conforms_application(
        &mut self,
        actual: &crate::definitions::generic_types::Type,
        expected: &crate::definitions::generic_types::Type,
    ) -> Result<bool, String> {
        // A checked pair's schemas are already materialized in this context,
        // so a repeated query (every verified image of a batch relates all of
        // its types pairwise) is answered from the recorded result.
        let pair = (actual.clone(), expected.clone());
        if let Some(conforms) = self.conformance.get(&pair) {
            return Ok(*conforms);
        }
        let conforms = self.conforms_uncached(actual, expected)?;
        self.conformance.insert(pair, conforms);
        Ok(conforms)
    }

    fn conforms_uncached(
        &mut self,
        actual: &crate::definitions::generic_types::Type,
        expected: &crate::definitions::generic_types::Type,
    ) -> Result<bool, String> {
        let actual_top = reference_top(actual)?;
        let expected_top = reference_top(expected)?;
        if actual_top || expected_top {
            if !actual_top {
                self.runtime_schema(std::slice::from_ref(actual))?;
            }
            if !expected_top {
                self.runtime_schema(std::slice::from_ref(expected))?;
            }
            return Ok(expected_top);
        }
        let actual_runtime = crate::definitions::generic_types::RuntimeType::new(actual.clone())?;
        let expected_runtime =
            crate::definitions::generic_types::RuntimeType::new(expected.clone())?;
        // Every ancestor of `actual` is in its own materialized closure, so the
        // per-type schema (cached) decides conformance; materializing one per
        // pair made verification quadratic in schema closures. `expected` is
        // still materialized, so an invalid expected type is still rejected.
        self.runtime_schema(std::slice::from_ref(expected))?;
        let schema = self.runtime_schema(std::slice::from_ref(actual))?;
        let a = schema.class_key_for_type(&actual_runtime)?;
        Ok(match schema.class_key_for_type(&expected_runtime) {
            Ok(b) => schema.schema().conforms(a, b),
            // Not in the closure of `actual`, so not one of its ancestors.
            Err(_) => false,
        })
    }

    fn common_application(
        &mut self,
        left: &crate::definitions::generic_types::Type,
        right: &crate::definitions::generic_types::Type,
    ) -> Result<Type, String> {
        if reference_top(left)? || reference_top(right)? {
            self.conforms_application(left, right)?;
            return Ok(Type::Model(
                crate::definitions::schema::eobject_reference_top_declaration()?,
            ));
        }
        let left_runtime = crate::definitions::generic_types::RuntimeType::new(left.clone())?;
        let right_runtime = crate::definitions::generic_types::RuntimeType::new(right.clone())?;
        let checked = self.runtime_schema(&[left.clone(), right.clone()])?;
        let left_key = checked.class_key_for_type(&left_runtime)?;
        let right_key = checked.class_key_for_type(&right_runtime)?;
        let schema = checked.schema();
        let right_ancestors: BTreeSet<_> = schema.ancestry(right_key)?.into_iter().collect();
        let candidates: Vec<_> = schema
            .ancestry(left_key)?
            .into_iter()
            .filter(|key| right_ancestors.contains(key))
            .collect();
        let minimal: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|candidate| {
                !candidates
                    .iter()
                    .any(|other| other != candidate && schema.conforms(other, candidate))
            })
            .collect();
        match minimal.as_slice() {
            [only] => match schema.instantiations.get(*only) {
                Some(application) => Ok(Type::ModelApplication(application.expression().clone())),
                None => Ok(Type::Model(declaration(schema, only)?)),
            },
            [] => Err("Model branches have no common supertype".into()),
            _ => Err("Ambiguous common model supertype; supply an expected type".into()),
        }
    }

    pub fn datatype_checks(
        &self,
    ) -> Vec<crate::definitions::generic_resolution::CheckedDatatypeApplication> {
        self.datatype_checks.values().cloned().collect()
    }

    pub fn applied_datatype(
        &mut self,
        expression: &crate::definitions::generic_types::Type,
    ) -> Result<&crate::definitions::datatypes::Datatype, String> {
        let application = crate::definitions::generic_types::ClosedType::new(expression.clone())?;
        if !self.applied_datatypes.contains_key(expression) {
            let resolver = self.fresh_resolver().map_err(|error| error.to_string())?;
            let resolver = match self.conversions {
                Some(registry) => resolver
                    .with_conversions(registry)
                    .map_err(|error| error.to_string())?,
                None => resolver,
            };
            let checked = resolver
                .instantiate_datatype(&application)
                .map_err(|error| error.to_string())?;
            let crate::definitions::generic_types::Type::Classifier {
                declaration,
                arguments,
                ..
            } = expression
            else {
                return Err("Expected an exact datatype application".into());
            };
            if arguments.is_empty() {
                return Err("Applied datatype requires explicit arguments".into());
            }
            let datatype = crate::definitions::compiler::import_datatype_bundle_with_limits(
                self.bundle,
                declaration,
                self.source_limits,
            )
            .map_err(|error| error.to_string())?
            .specialize(&checked)?;
            self.applied_datatypes.insert(expression.clone(), datatype);
            self.datatype_checks.insert(expression.clone(), checked);
        }
        let datatype = self
            .applied_datatypes
            .get(expression)
            .ok_or("Missing checked datatype application")?;
        self.conversions
            .ok_or("Explicit applied conversion registry required")?
            .require_datatype(datatype)?;
        Ok(datatype)
    }

    pub fn datatype(
        &mut self,
        id: &DeclarationId,
    ) -> Result<&crate::definitions::datatypes::Datatype, String> {
        if !self.standard_datatypes.contains_key(id) {
            let datatype =
                if id.source == *crate::definitions::compiler::standard_datatype_source_id() {
                    crate::definitions::compiler::standard_datatype_by_id(id)?
                } else {
                    crate::definitions::compiler::import_datatype_bundle_with_limits(
                        self.bundle,
                        id,
                        self.source_limits,
                    )
                    .map_err(|error| error.to_string())?
                };
            if !datatype.parameters().is_empty() {
                return Err(
                    "Generic datatype annotations require an explicit applied datatype type".into(),
                );
            }
            if matches!(
                datatype.binding(),
                crate::definitions::datatypes::Binding::Unsupported { .. }
            ) {
                self.conversions
                    .ok_or("Explicit conversion registry required")?
                    .require_datatype(&datatype)?;
            }
            self.standard_datatypes.insert(id.clone(), datatype);
        }
        self.standard_datatypes
            .get(id)
            .ok_or_else(|| "Missing checked datatype context".into())
    }
    pub fn enumeration(
        &mut self,
        id: &DeclarationId,
    ) -> Result<&crate::definitions::enums::Enumeration, String> {
        if !self.enumerations.contains_key(id) {
            let enumeration = crate::definitions::compiler::import_enum_bundle_with_limits(
                self.bundle,
                id,
                self.source_limits,
            )
            .map_err(|error| error.to_string())?;
            self.enumerations.insert(id.clone(), enumeration);
        }
        self.enumerations
            .get(id)
            .ok_or_else(|| "Missing exact enum declaration".into())
    }
    /// Resolve within an already selected exact package, then validate its shared
    /// schema context. Names containing dots remain one classifier name.
    pub fn classifier(
        &mut self,
        package: &DeclarationId,
        name: &str,
    ) -> Result<DeclarationId, String> {
        let node = self.resources.node(package)?;
        let ordinal = node
            .children
            .iter()
            .position(|node| {
                node.name.namespace.is_none()
                    && node.name.local == "eClassifiers"
                    && node.attributes.iter().any(|a| {
                        a.name.namespace.is_none() && a.name.local == "name" && a.value == name
                    })
            })
            .ok_or_else(|| format!("Unknown model classifier {name}"))?;
        let mut id = package.clone();
        id.path.push(ordinal);
        self.runtime_schema(&[plain(&id)])?;
        Ok(id)
    }
    /// Reuse immutable parsed source inputs only. Every resolver receives fresh
    /// standard resources and fresh generic/conversion/rigid policy state.
    fn fresh_resolver(
        &self,
    ) -> Result<
        crate::definitions::generic_resolution::Resolver,
        crate::definitions::ecore::ImportError,
    > {
        crate::definitions::generic_resolution::Resolver::with_source_resources(
            self.bundle,
            self.resources.clone(),
            self.source_limits,
        )
    }
    pub fn new(bundle: &'a DefinitionBundle) -> Result<Self, String> {
        Self::with_source_limits(bundle, None, Default::default())
    }
    pub fn with_conversions(
        bundle: &'a DefinitionBundle,
        conversions: &'a crate::definitions::conversion::ConversionRegistry<'a>,
    ) -> Result<Self, String> {
        Self::with_source_limits(bundle, Some(conversions), Default::default())
    }
    pub(crate) fn with_source_limits(
        bundle: &'a DefinitionBundle,
        conversions: Option<&'a crate::definitions::conversion::ConversionRegistry<'a>>,
        source_limits: crate::definitions::ecore::ParseLimits,
    ) -> Result<Self, String> {
        let resources =
            ResourceSet::new_with_limits(bundle, source_limits).map_err(|e| e.to_string())?;
        Ok(Self::with_source_resources(
            bundle,
            conversions,
            source_limits,
            resources,
        ))
    }
    /// Move immutable parsed inputs onward; policy/resolution caches are dropped.
    pub(crate) fn into_source_resources(self) -> ResourceSet {
        self.resources
    }
    /// Internal ownership handoff from source-validated compiler preparation.
    /// Resolver policies and conversion authority are still configured fresh.
    pub(crate) fn with_source_resources(
        bundle: &'a DefinitionBundle,
        conversions: Option<&'a crate::definitions::conversion::ConversionRegistry<'a>>,
        source_limits: crate::definitions::ecore::ParseLimits,
        resources: ResourceSet,
    ) -> Self {
        Self {
            bundle,
            conversions,
            source_limits,
            resources,
            symbolic_resolver: None,
            symbolic_parameters: BTreeMap::new(),
            symbolic_checks: BTreeMap::new(),
            enumerations: BTreeMap::new(),
            applied_datatypes: BTreeMap::new(),
            datatype_checks: BTreeMap::new(),
            standard_datatypes: BTreeMap::new(),
            runtime_schemas: BTreeMap::new(),
            runtime_closures: BTreeMap::new(),
            materialized: BTreeMap::new(),
            conformance: BTreeMap::new(),
            generic_checks: BTreeMap::new(),
            schema_store: None,
            runtime_operations: Default::default(),
        }
    }
    pub fn conforms(
        &mut self,
        actual: &DeclarationId,
        expected: &DeclarationId,
    ) -> Result<bool, String> {
        self.conforms_application(&plain(actual), &plain(expected))
    }

    /// Unique most-specific shared ancestor; unrelated or ambiguous models reject.
    pub fn common_supertype(
        &mut self,
        left: &DeclarationId,
        right: &DeclarationId,
    ) -> Result<DeclarationId, String> {
        match self.common_application(&plain(left), &plain(right))? {
            Type::Model(id) => Ok(id),
            Type::ModelApplication(_) => Err(
                "Common model supertype is an application; retain its shared type expression"
                    .into(),
            ),
            _ => Err("Expected a model supertype".into()),
        }
    }

    pub fn feature(
        &mut self,
        table: &mut TypeTable,
        class: &DeclarationId,
        name: &str,
    ) -> Result<FeatureType, String> {
        self.feature_application(table, &plain(class), name)
    }

    pub fn feature_application(
        &mut self,
        table: &mut TypeTable,
        expression: &crate::definitions::generic_types::Type,
        name: &str,
    ) -> Result<FeatureType, String> {
        let runtime = crate::definitions::generic_types::RuntimeType::new(expression.clone())?;
        let checked = self.runtime_schema(std::slice::from_ref(expression))?;
        let key = checked.class_key_for_type(&runtime)?.to_owned();
        let projection = project_feature(checked, &runtime, name)?;
        let schema = checked.schema();
        let member = schema.effective_feature(&key, name)?;
        let feature = member.feature.clone();
        if projection
            .domains
            .iter()
            .any(|domain| !domain.unresolved.is_empty())
        {
            return Err("Rcore feature domain contains unresolved runtime candidates; complete static read coverage is unavailable".into());
        }
        // A domain record may preserve a concrete inherited projection. Its
        // presence alone does not make the static result inexact. Require the
        // checked source expression to be representable and the selected target
        // to agree, rather than treating the structural erasure as evidence.
        let assignment_exact = projection.domains.iter().all(|domain| {
            let Ok(exact) = crate::definitions::generic_types::RuntimeType::new(
                domain.source.expression.clone(),
            ) else {
                return false;
            };
            if feature.reference {
                checked
                    .class_key_for_type(&exact)
                    .is_ok_and(|target| target == feature.target)
            } else {
                domain.lexical_target.as_ref() == Some(&feature.target)
                    && domain.source.erasure == exact
                    && domain
                        .alternatives
                        .iter()
                        .all(|target| target == &feature.target)
            }
        });
        if !assignment_exact {
            if !feature.reference {
                return Err("Rcore constrained attribute domain requires an explicit static domain type; checked source projection is available".into());
            }
            // Widen only through the shared runtime conformance relation. This
            // read type does not certify assignment to the constrained feature.
            if projection
                .domains
                .iter()
                .flat_map(|domain| &domain.alternatives)
                .any(|actual| !schema.conforms(actual, &feature.target))
            {
                return Err(
                    "Rcore constrained reference domain has no verified erased read type".into(),
                );
            }
        }
        let element = if feature.reference {
            if feature.target == crate::definitions::schema::EOBJECT_REFERENCE_TOP {
                Type::Model(
                    schema
                        .reference_top
                        .clone()
                        .ok_or("Missing checked EObject reference domain")?,
                )
            } else {
                match schema.instantiations.get(&feature.target) {
                    Some(application) => Type::ModelApplication(application.expression().clone()),
                    None => Type::Model(declaration(schema, &feature.target)?),
                }
            }
        } else if let Some(id) = &feature.datatype {
            if let Some(datatype) = schema
                .datatypes
                .get(&feature.target)
                .filter(|datatype| datatype.declaration() == id)
            {
                // Schema construction checked the selected provider and exact application.
                match datatype.application() {
                    Some(application) => {
                        Type::DatatypeApplication(application.expression().clone())
                    }
                    None => Type::Datatype(datatype.declaration().clone()),
                }
            } else {
                let enumeration = schema
                    .enumerations
                    .get(&feature.target)
                    .filter(|enumeration| enumeration.declaration() == id)
                    .ok_or("Unknown declared feature datatype")?;
                Type::Enumeration(enumeration.declaration().clone())
            }
        } else {
            Type::Scalar(match feature.target.as_str() {
                "EString" => Scalar::String,
                "EByteArray" => Scalar::Bytes,
                "EChar" | "ECharacterObject" => Scalar::Char16,
                "EDate" => Scalar::DateInstant,
                "EBigInteger" => Scalar::BigInt,
                "EBigDecimal" => Scalar::Decimal,
                "EBoolean" | "EBooleanObject" => Scalar::Bool,
                "EByte" | "EByteObject" => Scalar::Int8,
                "EShort" | "EShortObject" => Scalar::Int16,
                "EInt" | "EIntegerObject" => Scalar::Int32,
                "ELong" | "ELongObject" => Scalar::Int64,
                "EFloat" | "EFloatObject" => Scalar::Float32,
                "EDouble" | "EDoubleObject" => Scalar::Float64,
                _ => return Err("Feature datatype requires E05/E06".into()),
            })
        };
        if let Type::DatatypeApplication(expression) = &element {
            self.applied_datatype(expression)?;
        }
        let element = table.intern(element)?;
        let value = if feature.upper == 1 {
            if feature.lower == 0 {
                table.intern(Type::Option(element))?
            } else {
                element
            }
        } else {
            table.intern(Type::Many {
                element,
                lower: feature.lower,
                upper: feature.upper,
                ordered: feature.ordered,
                unique: feature.unique,
            })?
        };
        Ok(FeatureType {
            declaration: projection.declaration,
            schema_revision: projection.schema_revision,
            receiver: projection.receiver,
            owner: projection.owner,
            assignment_exact,
            domains: projection.domains,
            value,
            derived: feature.derived,
            changeable: feature.changeable,
            transient: feature.transient,
            volatile: feature.volatile,
            unsettable: feature.unsettable,
        })
    }

    /// Least common supported branch type, preserving wrappers and multiplicity.
    /// Explicit postorder frames avoid recursion even for deeply nested wrappers.
    pub fn join(
        &mut self,
        table: &mut TypeTable,
        left: TypeRef,
        right: TypeRef,
    ) -> Result<TypeRef, String> {
        let mut pending = vec![(left, right, false)];
        let mut joined = BTreeMap::new();
        while let Some((left, right, exiting)) = pending.pop() {
            if joined.contains_key(&(left, right)) {
                continue;
            }
            // Equivalent unconstrained Many/Collection representations have a
            // canonical mixed join: Collection, independent of branch order.
            let mixed_collection = matches!(
                (table.get(left)?, table.get(right)?),
                (Type::Many { .. }, Type::Collection { .. })
                    | (Type::Collection { .. }, Type::Many { .. })
            );
            if !exiting && !mixed_collection {
                if self.assignable(table, left, right)? {
                    joined.insert((left, right), right);
                    continue;
                }
                if self.assignable(table, right, left)? {
                    joined.insert((left, right), left);
                    continue;
                }
            }
            let a = table.get(left)?.clone();
            let b = table.get(right)?.clone();
            let child = |a, b| {
                joined
                    .get(&(a, b))
                    .copied()
                    .ok_or_else(|| "Missing joined child type".to_owned())
            };
            let (ty, children) = match (a, b) {
                (Type::Model(a), Type::Model(b)) => (
                    Some(self.common_application(&plain(&a), &plain(&b))?),
                    vec![],
                ),
                (Type::ModelApplication(a), Type::ModelApplication(b)) => {
                    (Some(self.common_application(&a, &b)?), vec![])
                }
                (Type::ModelApplication(a), Type::Model(b)) => {
                    (Some(self.common_application(&a, &plain(&b))?), vec![])
                }
                (Type::Model(a), Type::ModelApplication(b)) => {
                    (Some(self.common_application(&plain(&a), &b)?), vec![])
                }
                (
                    Type::Collection {
                        kind: ak,
                        element: a,
                    },
                    Type::Collection {
                        kind: bk,
                        element: b,
                    },
                ) if ak == bk => (
                    if exiting {
                        Some(Type::Collection {
                            kind: ak,
                            element: child(a, b)?,
                        })
                    } else {
                        None
                    },
                    vec![(a, b)],
                ),
                (
                    Type::Many {
                        element: a,
                        ordered,
                        unique,
                        ..
                    },
                    Type::Collection { kind, element: b },
                )
                | (
                    Type::Collection { kind, element: a },
                    Type::Many {
                        element: b,
                        ordered,
                        unique,
                        ..
                    },
                ) if collection_kind(ordered, unique) == kind => (
                    if exiting {
                        Some(Type::Collection {
                            kind,
                            element: child(a, b)?,
                        })
                    } else {
                        None
                    },
                    vec![(a, b)],
                ),
                (Type::Tuple(a), Type::Tuple(b)) if a.len() == b.len() => (
                    if exiting {
                        Some(Type::Tuple(
                            a.iter()
                                .copied()
                                .zip(b.iter().copied())
                                .map(|(a, b)| child(a, b))
                                .collect::<Result<_, _>>()?,
                        ))
                    } else {
                        None
                    },
                    a.into_iter().zip(b).collect(),
                ),
                (Type::Record(a), Type::Record(b)) if a.keys().eq(b.keys()) => (
                    if exiting {
                        Some(Type::Record(
                            a.iter()
                                .zip(&b)
                                .map(|((name, a), (_, b))| Ok((name.clone(), child(*a, *b)?)))
                                .collect::<Result<_, String>>()?,
                        ))
                    } else {
                        None
                    },
                    a.into_values().zip(b.into_values()).collect(),
                ),
                (Type::Option(a), Type::Option(b)) => (
                    if exiting {
                        Some(Type::Option(child(a, b)?))
                    } else {
                        None
                    },
                    vec![(a, b)],
                ),
                (Type::Result { ok: a, error: ae }, Type::Result { ok: b, error: be }) => (
                    if exiting {
                        Some(Type::Result {
                            ok: child(a, b)?,
                            error: child(ae, be)?,
                        })
                    } else {
                        None
                    },
                    vec![(a, b), (ae, be)],
                ),
                (
                    Type::Function {
                        parameters: a,
                        result: ar,
                    },
                    Type::Function {
                        parameters: b,
                        result: br,
                    },
                ) if a == b => (
                    if exiting {
                        Some(Type::Function {
                            parameters: a,
                            result: child(ar, br)?,
                        })
                    } else {
                        None
                    },
                    vec![(ar, br)],
                ),
                (
                    Type::Many {
                        element: a,
                        lower: al,
                        upper: au,
                        ordered: ao,
                        unique: aq,
                    },
                    Type::Many {
                        element: b,
                        lower: bl,
                        upper: bu,
                        ordered: bo,
                        unique: bq,
                    },
                ) if ao == bo && aq == bq => (
                    if exiting {
                        Some(Type::Many {
                            element: child(a, b)?,
                            lower: al.min(bl),
                            upper: if au == -1 || bu == -1 { -1 } else { au.max(bu) },
                            ordered: ao,
                            unique: aq,
                        })
                    } else {
                        None
                    },
                    vec![(a, b)],
                ),
                _ => {
                    return Err(
                        "Branches have incompatible types; supply a compatible expected type"
                            .into(),
                    );
                }
            };
            if let Some(ty) = ty {
                joined.insert((left, right), table.intern(ty)?);
            } else {
                pending.push((left, right, true));
                pending.extend(children.into_iter().rev().map(|(a, b)| (a, b, false)));
            }
        }
        joined
            .get(&(left, right))
            .copied()
            .ok_or_else(|| "Missing branch type".into())
    }

    /// Immutable wrappers are covariant. Function inputs are contravariant and
    /// outputs covariant. No numeric promotion, optional lift or unwrapping.
    pub fn assignable(
        &mut self,
        table: &TypeTable,
        actual: TypeRef,
        expected: TypeRef,
    ) -> Result<bool, String> {
        let mut pending = vec![(actual, expected)];
        let mut checked = BTreeSet::new();
        while let Some((actual, expected)) = pending.pop() {
            if !checked.insert((actual, expected)) {
                continue;
            }
            if matches!(
                table.get(actual)?,
                Type::SymbolicModelApplication { .. } | Type::SymbolicDatatypeApplication { .. }
            ) || matches!(
                table.get(expected)?,
                Type::SymbolicModelApplication { .. } | Type::SymbolicDatatypeApplication { .. }
            ) {
                if !self.symbolic_assignable(table, actual, expected)? {
                    return Ok(false);
                }
                continue;
            }
            let actual = table.get(actual)?;
            let expected = table.get(expected)?;
            match (actual, expected) {
                (Type::Scalar(a), Type::Scalar(b)) if a == b => {}
                (Type::Enumeration(a), Type::Enumeration(b)) if a == b => {
                    self.enumeration(a)?;
                }
                (Type::Datatype(a), Type::Datatype(b)) if a == b => {
                    self.datatype(a)?;
                }
                (Type::DatatypeApplication(a), Type::DatatypeApplication(b)) if a == b => {
                    self.applied_datatype(a)?;
                }
                (Type::Model(a), Type::Model(b)) if self.conforms(a, b)? => {}
                (Type::ModelApplication(a), Type::ModelApplication(b))
                    if self.conforms_application(a, b)? => {}
                (Type::ModelApplication(a), Type::Model(b))
                    if self.conforms_application(a, &plain(b))? => {}
                (Type::Model(a), Type::ModelApplication(b))
                    if self.conforms_application(&plain(a), b)? => {}
                (
                    Type::Parameter {
                        symbol: a,
                        ordinal: ai,
                    },
                    Type::Parameter {
                        symbol: b,
                        ordinal: bi,
                    },
                ) if a == b && ai == bi => {}
                (
                    Type::Collection {
                        kind: ak,
                        element: a,
                    },
                    Type::Collection {
                        kind: bk,
                        element: b,
                    },
                ) if ak == bk => pending.push((*a, *b)),
                (
                    Type::Many {
                        element: a,
                        ordered,
                        unique,
                        ..
                    },
                    Type::Collection { kind, element: b },
                ) if collection_kind(*ordered, *unique) == *kind => pending.push((*a, *b)),
                (
                    Type::Collection { kind, element: a },
                    Type::Many {
                        element: b,
                        lower: 0,
                        upper: -1,
                        ordered,
                        unique,
                    },
                ) if collection_kind(*ordered, *unique) == *kind => pending.push((*a, *b)),
                (Type::Tuple(a), Type::Tuple(b)) if a.len() == b.len() => {
                    pending.extend(a.iter().copied().zip(b.iter().copied()))
                }
                (Type::Record(a), Type::Record(b)) if a.keys().eq(b.keys()) => {
                    pending.extend(a.values().copied().zip(b.values().copied()))
                }
                (Type::Option(a), Type::Option(b)) => pending.push((*a, *b)),
                (Type::Result { ok: a, error: ae }, Type::Result { ok: b, error: be }) => {
                    pending.extend([(*a, *b), (*ae, *be)]);
                }
                (
                    Type::Function {
                        parameters: a,
                        result: ar,
                    },
                    Type::Function {
                        parameters: b,
                        result: br,
                    },
                ) if a.len() == b.len() => {
                    pending.push((*ar, *br));
                    pending.extend(b.iter().copied().zip(a.iter().copied()));
                }
                (
                    Type::Many {
                        element: a,
                        lower: al,
                        upper: au,
                        ordered: ao,
                        unique: aq,
                    },
                    Type::Many {
                        element: b,
                        lower: bl,
                        upper: bu,
                        ordered: bo,
                        unique: bq,
                    },
                ) if al >= bl && (*bu == -1 || (*au != -1 && au <= bu)) && ao == bo && aq == bq => {
                    pending.push((*a, *b));
                }
                _ => return Ok(false),
            }
        }
        Ok(true)
    }
}
fn project_feature(
    checked: &CheckedSchema,
    receiver: &crate::definitions::generic_types::RuntimeType,
    name: &str,
) -> Result<FeatureProjection, String> {
    let key = checked.class_key_for_type(receiver)?;
    let schema = checked.schema();
    let member = schema.effective_feature(key, name)?;
    let id = match member.identity {
        crate::definitions::schema::MemberIdentity::Source { declaration } => declaration,
        crate::definitions::schema::MemberIdentity::Programmatic { .. } => {
            return Err("Missing feature source identity".into());
        }
    };
    let owner = match schema.instantiations.get(member.owner) {
        Some(owner) => owner.clone(),
        None => crate::definitions::generic_types::RuntimeType::new(plain(&declaration(
            schema,
            member.owner,
        )?))?,
    };
    Ok(FeatureProjection {
        declaration: id,
        receiver: receiver.clone(),
        owner,
        schema_revision: schema.revision.clone(),
        domains: schema
            .feature_domains(key, name)
            .into_iter()
            .cloned()
            .collect(),
    })
}

fn declaration(schema: &Schema, key: &str) -> Result<DeclarationId, String> {
    let id = schema
        .declarations
        .get(key)
        .ok_or("Missing exact classifier identity")?;
    Ok(DeclarationId {
        source: id.source.clone(),
        path: id.path.clone(),
    })
}

fn plain(id: &DeclarationId) -> crate::definitions::generic_types::Type {
    crate::definitions::generic_types::Type::Classifier {
        declaration: id.clone(),
        arguments: vec![],
        raw: false,
    }
}

/// Adapt only exact nominal Ecore identities. Rcore wrappers are not Ecore classifiers.
pub(crate) fn generic_argument(
    ty: &Type,
) -> Result<crate::definitions::generic_types::Type, String> {
    match ty {
        Type::Model(id) | Type::Enumeration(id) | Type::Datatype(id) => Ok(plain(id)),
        Type::ModelApplication(expression) | Type::DatatypeApplication(expression) => {
            Ok(expression.clone())
        }
        _ => Err("Ecore model arguments require an exact nominal classifier type".into()),
    }
}

fn collection_kind(ordered: bool, unique: bool) -> CollectionKind {
    match (ordered, unique) {
        (true, false) => CollectionKind::Sequence,
        (false, true) => CollectionKind::Set,
        (true, true) => CollectionKind::OrderedSet,
        (false, false) => CollectionKind::Bag,
    }
}

fn reference_top(expression: &crate::definitions::generic_types::Type) -> Result<bool, String> {
    Ok(expression == &plain(&crate::definitions::schema::eobject_reference_top_declaration()?))
}

#[cfg(test)]
mod prepared_resource_tests {
    use super::*;
    use crate::definitions::{
        bundle::{DefinitionSource, SourceKind},
        conversion::InvocationContext,
        rcore_behavior::{bindings, checking, modules::ModuleIndexPreparation, signatures},
    };
    #[test]
    fn exact_resource_allocations_survive_binding_signatures_and_body_checking()
    -> Result<(), String> {
        let source = DefinitionSource::new(
            "memory:prepared-model.rcore",
            SourceKind::Rcore,
            "rcore 1;package p version \"1\";metamodel \"urn:prepared\" prefix b {class Node {attr label:String [0,1];}}behavior {import model \"urn:prepared\" as m;fn run(n:m.Node)->String?=n.label;}",
        );
        let id = source.id.clone();
        let input_text = source.text.as_ptr();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        };
        let limits = Default::default();
        let context = InvocationContext::default();
        let mut pending = ModuleIndexPreparation::begin(&bundle, &id, limits, &context)
            .map_err(|e| format!("{e:?}"))?;
        while !pending.advance(&context).map_err(|e| format!("{e:?}"))? {}
        let (index, resources) = pending
            .finish_with_resources()
            .map_err(|e| format!("{e:?}"))?;
        let allocation = |resources: &ResourceSet| -> Result<_, String> {
            let doc = resources.document(&id)?;
            Ok((doc.source.text.as_ptr(), doc.root.children.as_ptr()))
        };
        let prepared = allocation(&resources)?;
        assert_ne!(prepared.0, input_text);
        let (bound, resources) = bindings::bind_prepared(&bundle, index, resources, limits)
            .map_err(|e| format!("{e:?}"))?;
        assert_eq!(allocation(&resources)?, prepared);
        let (signatures, resources) =
            signatures::index_prepared(bound, resources).map_err(|e| format!("{e:?}"))?;
        assert_eq!(allocation(&resources)?, prepared);
        let models = ModelTypes::with_source_resources(&bundle, None, limits, resources);
        assert_eq!(allocation(&models.resources)?, prepared);
        let (report, resources) =
            checking::check_prepared(&bundle, None, limits, signatures, models.resources, None)
                .map_err(|e| format!("{e:?}"))?;
        assert!(!report.modules[0].members.is_empty());
        assert_eq!(allocation(&resources)?, prepared);
        assert_eq!(bundle.sources[0].text.as_ptr(), input_text);
        Ok(())
    }
}

#[cfg(test)]
mod resource_sharing_model_tests {
    use super::*;
    use crate::definitions::bundle::{DefinitionSource, SourceKind};

    #[test]
    fn resource_sharing_model_trials_keep_input_allocations_and_exact_snapshot_limits()
    -> Result<(), String> {
        let source = DefinitionSource::new(
            "memory:shared-model.rcore",
            SourceKind::Rcore,
            "rcore 1;package p version \"1\";metamodel \"urn:shared-model\" prefix p {class Node {attr label:String [0,1];}}",
        );
        let id = source.id.clone();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        };
        let models = ModelTypes::new(&bundle)?;
        let pointer = models.resources.document(&id)? as *const _;
        let fresh = models.fresh_resolver().map_err(|error| error.to_string())?;
        assert_eq!(fresh.resource_sets().0.document(&id)? as *const _, pointer);
        let mut remaining = usize::MAX;
        let trial = models.trial_clone(&mut remaining)?;
        let charged = usize::MAX - remaining;
        assert!(charged > 0);
        assert_eq!(trial.resources.document(&id)? as *const _, pointer);
        let mut exact = charged;
        let exact_trial = models.trial_clone(&mut exact)?;
        assert_eq!(exact, 0);
        assert_eq!(exact_trial.resources.document(&id)? as *const _, pointer);
        let mut short = charged - 1;
        assert!(
            matches!(models.trial_clone(&mut short),Err(ref error) if error=="Overload snapshot byte limit exceeded")
        );
        assert_eq!(short, 0);
        assert_eq!(models.resources.document(&id)? as *const _, pointer);
        Ok(())
    }
}

#[cfg(test)]
#[path = "rcore_runtime_schema_capture_tests.rs"]
mod runtime_schema_capture_tests;

#[path = "rcore_runtime_schema_preparation.rs"]
mod runtime_schema_preparation;
#[path = "rcore_runtime_schema_store.rs"]
mod runtime_schema_store;
use runtime_schema_preparation::RuntimeSchemaPreparation;
pub(crate) use runtime_schema_preparation::{ModelFeaturePreparation, ModelPreparationError};
pub use runtime_schema_store::RuntimeSchemaStore;
