//! Source-expression operation projections retain shared types and formals.
use crate::definitions::{
    conversion::InvocationContext,
    ecore::{ErrorKind, ImportError},
    generic_resolution::{AppliedOperation, Resolver, SpecializedMember},
    generic_types::ClosedType,
    operations::{DeclaredOperation, Operation, TypeTarget},
    resources::DeclarationId,
    schema::{PreparationError, reserve_preparation, reserve_preparation_value},
};

fn target(
    member: &SpecializedMember,
    owner: &DeclarationId,
    observation: &InvocationContext<'_>,
) -> Result<Option<TypeTarget>, PreparationError> {
    let mut selected = None;
    let mut repeated = false;
    for signature in &member.signatures {
        observation.work(1).map_err(PreparationError::Observation)?;
        if &signature.owner == owner && signature.feature == "eType" {
            if selected.is_some() {
                repeated = true;
                break;
            }
            selected = Some(signature);
        }
    }
    let signature = selected.ok_or("Missing operation template type signature")?;
    if repeated || signature.specialized.len() > 1 {
        return Err("Ambiguous operation template type signature".into());
    }
    if let Some(expression) = signature.specialized.first() {
        reserve_preparation_value(expression, observation)?;
        Ok(Some(TypeTarget::Expression(expression.clone())))
    } else {
        Ok(None)
    }
}

pub(crate) fn project(operation: &mut Operation, member: &SpecializedMember) -> Result<(), String> {
    project_observed(operation, member, &InvocationContext::default())
        .map_err(|error| error.to_string())
}
pub(crate) fn project_observed(
    operation: &mut Operation,
    member: &SpecializedMember,
    observation: &InvocationContext<'_>,
) -> Result<(), PreparationError> {
    operation.result.target = target(member, &operation.declaration, observation)?;
    for parameter in &mut operation.parameters {
        observation.work(1).map_err(PreparationError::Observation)?;
        parameter.value.target = target(member, &parameter.declaration, observation)?;
    }
    operation.generic = true;
    Ok(())
}

fn append_expressions<'a>(
    pending: &mut Vec<&'a crate::definitions::generic_types::Type>,
    values: impl IntoIterator<Item = &'a crate::definitions::generic_types::Type>,
    observation: &InvocationContext<'_>,
) -> Result<(), PreparationError> {
    for expression in values {
        observation.work(1).map_err(PreparationError::Observation)?;
        reserve_preparation(observation, 32, &[])?;
        pending.push(expression);
    }
    Ok(())
}

impl<A: crate::definitions::schema::RuntimeAlternative>
    crate::definitions::schema::SchemaRepresentation<A>
{
    pub(crate) fn validate_operation_template_observed(
        &self,
        retained: &DeclaredOperation,
        observation: &InvocationContext<'_>,
    ) -> Result<(), PreparationError> {
        let Some(template) = &retained.template else {
            self.operation_signature_observed(&retained.operation, observation)?;
            return Ok(());
        };
        let operation = &retained.operation;
        if !operation.generic
            || !template.requires_schema_template_observed(observation)?
            || template.declaration != operation.declaration
            || template.name != operation.name
            || template.source_payload != operation.source_payload
        {
            return Err("Invalid retained operation template identity".into());
        }
        let owner = if let Some(projection) = &retained.projection {
            if &projection.source != template {
                return Err("Operation template differs from runtime source projection".into());
            }
            reserve_preparation_value(&projection.storage_owner, observation)?;
            projection.storage_owner.clone()
        } else {
            reserve_preparation_value(&template.owner, observation)?;
            crate::definitions::generic_types::RuntimeType::new(template.owner.clone())?
        };
        if self.class_key_for_type_observed(&owner, observation)? != retained.owner {
            return Err("Operation template owner differs from schema projection".into());
        }
        let inventory = self
            .generic_signatures
            .as_ref()
            .ok_or("Missing operation template source inventory")?;
        let mut formals = std::collections::BTreeSet::new();
        for formal in &template.formals {
            observation.work(1).map_err(PreparationError::Observation)?;
            let id = &formal.declaration.declaration;
            reserve_preparation(observation, 128, &[])?;
            if id.source != operation.declaration.source
                || id.path.len() != operation.declaration.path.len() + 1
                || !id.path.starts_with(&operation.declaration.path)
                || !formals.insert(id)
            {
                return Err("Invalid operation template formal owner or duplicate".into());
            }
            let mut found = false;
            for parameter in &inventory.parameters {
                observation.work(1).map_err(PreparationError::Observation)?;
                if parameter.syntax == formal.declaration {
                    found = true;
                    break;
                }
            }
            if !found {
                return Err("Operation template formal differs from source inventory".into());
            }
        }
        for signature in &template.signatures {
            observation.work(1).map_err(PreparationError::Observation)?;
            let mut found = false;
            for raw in &inventory.raw_signatures {
                observation.work(1).map_err(PreparationError::Observation)?;
                if raw.owner == signature.owner && raw.feature == signature.feature {
                    found = true;
                    break;
                }
            }
            if !found {
                return Err("Operation template signature differs from source inventory".into());
            }
        }
        if let Some(projection) = &retained.projection {
            let mut pending = Vec::new();
            append_expressions(
                &mut pending,
                projection
                    .source
                    .signatures
                    .iter()
                    .flat_map(|signature| &signature.specialized)
                    .chain(
                        projection
                            .source
                            .formals
                            .iter()
                            .flat_map(|formal| &formal.bounds),
                    ),
                observation,
            )?;
            while let Some(expression) = pending.pop() {
                observation.work(1).map_err(PreparationError::Observation)?;
                match expression {
                    crate::definitions::generic_types::Type::RigidParameter { .. } => {
                        return Err(
                            "Symbolic formal cannot enter a runtime operation template".into()
                        );
                    }
                    crate::definitions::generic_types::Type::Parameter { declaration } => {
                        let mut found = false;
                        for parameter in &inventory.parameters {
                            observation.work(1).map_err(PreparationError::Observation)?;
                            if parameter.syntax.declaration == *declaration {
                                found = true;
                                break;
                            }
                        }
                        if !found {
                            return Err("Unknown runtime operation formal".into());
                        }
                        if !formals.contains(declaration) {
                            reserve_preparation(observation, 128, &[])?;
                            formals.insert(declaration);
                        }
                    }
                    crate::definitions::generic_types::Type::Classifier { arguments, .. } => {
                        append_expressions(&mut pending, arguments.iter(), observation)?;
                    }
                    crate::definitions::generic_types::Type::Wildcard { upper, lower } => {
                        append_expressions(
                            &mut pending,
                            [upper, lower]
                                .into_iter()
                                .filter_map(|bound| bound.as_deref()),
                            observation,
                        )?
                    }
                }
            }
        }
        for expression in template
            .signatures
            .iter()
            .flat_map(|signature| &signature.specialized)
            .chain(template.formals.iter().flat_map(|formal| &formal.bounds))
        {
            observation.work(1).map_err(PreparationError::Observation)?;
            // Empty substitution was used only to validate shape, then discard
            // an identical owned expression. Share its borrowed shape validator.
            crate::definitions::generic_types::shape_observed(expression, observation)?;
            let mut pending = Vec::new();
            append_expressions(&mut pending, std::iter::once(expression), observation)?;
            while let Some(ty) = pending.pop() {
                observation.work(1).map_err(PreparationError::Observation)?;
                match ty {
                    crate::definitions::generic_types::Type::RigidParameter { .. } => {
                        return Err(
                            "Symbolic formal cannot enter a runtime operation template".into()
                        );
                    }
                    crate::definitions::generic_types::Type::Parameter { declaration }
                        if !formals.contains(declaration) =>
                    {
                        return Err("Operation template references a foreign formal".into());
                    }
                    crate::definitions::generic_types::Type::Classifier { arguments, .. } => {
                        append_expressions(&mut pending, arguments.iter(), observation)?;
                    }
                    crate::definitions::generic_types::Type::Wildcard { upper, lower } => {
                        append_expressions(
                            &mut pending,
                            [upper, lower]
                                .into_iter()
                                .filter_map(|bound| bound.as_deref()),
                            observation,
                        )?
                    }
                    _ => {}
                }
            }
        }
        reserve_preparation_value(operation, observation)?;
        let mut expected = operation.clone();
        project_observed(&mut expected, template, observation)?;
        if expected != *operation {
            return Err("Operation template types differ from schema projection".into());
        }
        self.validate_operation_structure_observed(operation, true, observation)
    }

    pub(crate) fn canonical_operation_target(
        &self,
        resolver: &Resolver,
        target: &TypeTarget,
    ) -> Result<crate::definitions::generic_types::Type, String> {
        use crate::definitions::generic_types::Type;
        let declaration = match target {
            TypeTarget::Expression(expression) => return Ok(expression.clone()),
            TypeTarget::Builtin(name) => {
                self.operation_parameter_type(target)?;
                resolver.pinned_operation_builtin(name)?
            }
            TypeTarget::Class(name) => {
                self.operation_parameter_type(target)?;
                if let Some(application) = self.instantiations.get(name) {
                    return Ok(application.expression().clone());
                }
                let source = self
                    .declarations
                    .get(name)
                    .ok_or("Concrete operation class requires source identity")?;
                DeclarationId {
                    source: source.source.clone(),
                    path: source.path.clone(),
                }
            }
            TypeTarget::Datatype(name) => {
                self.operation_parameter_type(target)?;
                let datatype = self
                    .datatypes
                    .get(name)
                    .ok_or("Unknown concrete operation datatype")?;
                if let Some(application) = datatype.application() {
                    return Ok(application.expression().clone());
                }
                datatype.declaration().clone()
            }
            TypeTarget::Enumeration(name) => {
                self.operation_parameter_type(target)?;
                self.enumerations
                    .get(name)
                    .ok_or("Unknown concrete operation enum")?
                    .declaration()
                    .clone()
            }
        };
        Ok(Type::Classifier {
            declaration,
            arguments: vec![],
            raw: false,
        })
    }

    fn canonical_retained_operation(
        &self,
        resolver: &Resolver,
        operation: &Operation,
    ) -> Result<Operation, String> {
        let mut canonical = operation.clone();
        for value in std::iter::once(&mut canonical.result).chain(
            canonical
                .parameters
                .iter_mut()
                .map(|parameter| &mut parameter.value),
        ) {
            value.target = value
                .target
                .as_ref()
                .map(|target| {
                    self.canonical_operation_target(resolver, target)
                        .map(TypeTarget::Expression)
                })
                .transpose()?;
        }
        // Canonical form uses expressions; this flag does not change stored wire data.
        canonical.generic = true;
        Ok(canonical)
    }

    /// Check one retained runtime projection against current source before method application.
    pub fn instantiate_runtime_operation(
        &self,
        resolver: &Resolver,
        receiver: &crate::definitions::generic_types::RuntimeType,
        operation: &DeclarationId,
        owner: Option<&crate::definitions::generic_types::Type>,
        arguments: &[crate::definitions::generic_types::RuntimeType],
    ) -> Result<crate::definitions::generic_resolution::RuntimeAppliedOperation, ImportError> {
        let span = crate::definitions::bundle::SourceSpan {
            source: operation.source.clone(),
            start: 0,
            end: 0,
        };
        let invalid = |message| ImportError::new(ErrorKind::InvalidDefinition, &span, message);
        let kind = self.class_key_for_type(receiver).map_err(invalid)?;
        self.validate_declared_operation_inventory(kind)
            .map_err(invalid)?;
        let report = resolver
            .instantiate_runtime_operation_in_owner(receiver, operation, owner, arguments)?;
        let retained = self
            .declared_operation_selections
            .as_ref()
            .and_then(|sets| sets.get(kind))
            .ok_or_else(|| invalid("Missing runtime operation inventory".into()))?;
        let expected = resolver.canonical_operation_source(&report.projection.source)?;
        let storage_owner = self
            .class_key_for_type(&report.projection.storage_owner)
            .map_err(invalid)?;
        let mut matches = 0usize;
        for entry in retained
            .groups
            .iter()
            .flat_map(|group| &group.operations)
            .filter(|entry| &entry.operation.declaration == operation)
        {
            let identity_matches = entry.owner == storage_owner
                && if let Some(projection) = &entry.projection {
                    projection == &report.projection
                } else if let Some(template) = &entry.template {
                    template == &report.projection.source
                } else {
                    report.projection.source.formals.is_empty()
                };
            if !identity_matches {
                continue;
            }
            let canonical = self
                .canonical_retained_operation(resolver, &entry.operation)
                .map_err(invalid)?;
            if canonical != expected {
                return Err(invalid(
                    "Retained operation signature differs from checked source".into(),
                ));
            }
            matches += 1;
        }
        if matches != 1 {
            return Err(invalid(
                "Retained runtime operation projection differs from checked source".into(),
            ));
        }
        Ok(report)
    }

    /// Retained receiver-substituted template, not an executable operation.
    pub fn operation_template(
        &self,
        kind: &str,
        operation: &DeclarationId,
    ) -> Result<&SpecializedMember, String> {
        self.resolved_operations(kind)?;
        self.declared_operation_selections
            .as_ref()
            .and_then(|sets| sets.get(kind))
            .and_then(|set| {
                set.groups
                    .iter()
                    .flat_map(|group| &group.operations)
                    .find(|member| &member.operation.declaration == operation)
            })
            .and_then(|member| member.template.as_ref())
            .ok_or_else(|| "Missing operation-local generic template".into())
    }

    /// Revalidate the retained template against source and check explicit arguments
    /// through the same Resolver used by standalone generic requests. No invocation.
    pub fn instantiate_generic_operation(
        &self,
        resolver: &Resolver,
        receiver: &ClosedType,
        operation: &DeclarationId,
        arguments: &[ClosedType],
    ) -> Result<AppliedOperation, ImportError> {
        let span = crate::definitions::bundle::SourceSpan {
            source: operation.source.clone(),
            start: 0,
            end: 0,
        };
        let invalid = |message| ImportError::new(ErrorKind::InvalidDefinition, &span, message);
        let kind = self.class_key_for_type(receiver).map_err(invalid)?;
        let template = self.operation_template(kind, operation).map_err(invalid)?;
        let current = resolver.instantiate_members(receiver)?;
        let mut members = current
            .members
            .iter()
            .filter(|member| &member.declaration == operation);
        if members.next() != Some(template) || members.next().is_some() {
            return Err(ImportError::new(
                ErrorKind::InvalidDefinition,
                &template.span,
                "Retained operation template differs from checked source",
            ));
        }
        resolver.instantiate_operation(receiver, operation, arguments)
    }
}
