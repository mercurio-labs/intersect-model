//! Declared-erasure override comparison; selection and invocation are separate.
use super::*;
use crate::definitions::generic_types::Type;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationOverride {
    pub operation: DeclarationId,
    pub base: DeclarationId,
    pub parameters: Vec<RawSignature>,
    pub base_parameters: Vec<RawSignature>,
    pub overrides: bool,
}
impl Resolver {
    fn operation_raw_parameters(
        &self,
        operation: &DeclarationId,
    ) -> Result<Vec<RawSignature>, ImportError> {
        self.operation(operation)?;
        let node = self.node(operation)?;
        let mut result = Vec::new();
        for (index, child) in node.children.iter().enumerate() {
            if !named(child, "eParameters") {
                continue;
            }
            if result.len() >= 1024 {
                return Err(ImportError::new(
                    ErrorKind::ResourceLimit,
                    &child.span,
                    "Operation parameter comparison budget exceeded (1024)",
                ));
            }
            let mut id = operation.clone();
            id.path.push(index);
            let mut signatures = self.check_legacy(&id)?;
            let signature = signatures
                .pop()
                .ok_or_else(|| error(&child.span, "Missing parameter signature"))?;
            if signature.feature != "eType" || signature.raw_types.len() != 1 {
                return Err(error(
                    &child.span,
                    "Operation parameter requires exactly one raw type",
                ));
            }
            result.push(signature);
        }
        Ok(result)
    }

    /// Native override policy: declaration erasure and exact classifier identities,
    /// with explicit checked datatype equivalence. No receiver substitution occurs.
    pub fn operation_override(
        &self,
        operation: &DeclarationId,
        base: &DeclarationId,
    ) -> Result<OperationOverride, ImportError> {
        let parameters = self.operation_raw_parameters(operation)?;
        let base_parameters = self.operation_raw_parameters(base)?;
        let node = self.node(operation)?;
        let base_node = self.node(base)?;
        let name = attr(node, "name")
            .filter(|name| !name.is_empty())
            .ok_or_else(|| error(&node.span, "Operation name is empty"))?;
        let base_name = attr(base_node, "name")
            .filter(|name| !name.is_empty())
            .ok_or_else(|| error(&base_node.span, "Operation name is empty"))?;
        let owner = parent(operation)?;
        let base_owner = parent(base)?;
        let ty = Type::Classifier {
            declaration: owner.clone(),
            arguments: vec![],
            raw: !self.formals(&owner)?.is_empty(),
        };
        let ancestors = self.ancestry_expression(&ty, &node.span)?;
        let related = owner == base_owner || ancestors.iter().any(|ancestor| matches!(&ancestor.expression, Type::Classifier { declaration, .. } if declaration == &base_owner));
        let overrides = related
            && name == base_name
            && self.operation_parameters_equivalent(&parameters, &base_parameters, &node.span)?;
        Ok(OperationOverride {
            operation: operation.clone(),
            base: base.clone(),
            parameters,
            base_parameters,
            overrides,
        })
    }
    /// Concrete nominal signatures do not acquire equivalence merely from sharing
    /// a native scalar codec. Actual generic erasure retains its checked domain rule.
    fn parameter_requires_erasure(&self, parameter: &RawSignature) -> Result<bool, ImportError> {
        for usage in &parameter.generic {
            match self.type_expression(usage)?.expression {
                Type::Classifier {
                    arguments,
                    raw: false,
                    ..
                } if arguments.is_empty() => {}
                _ => return Ok(true),
            }
        }
        for classifier in &parameter.raw_types {
            if !self.formals(classifier)?.is_empty() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn selected_operation_equivalence(&self, a: &DeclarationId, b: &DeclarationId) -> Option<bool> {
        use crate::definitions::conversion::ClassifierRelation;
        if let Some(relation) = self.constructor_relations.get(&(a.clone(), b.clone())) {
            let mut consumed = self.consumed_constructor_relations.borrow_mut();
            consumed.insert(a.clone());
            consumed.insert(b.clone());
            return Some(*relation == ClassifierRelation::Equivalent);
        }
        self.provider_relations
            .get(&(a.clone(), b.clone()))
            .map(|relation| {
                let mut consumed = self.consumed_provider_relations.borrow_mut();
                consumed.insert(a.clone());
                consumed.insert(b.clone());
                *relation == ClassifierRelation::Equivalent
            })
    }

    fn operation_parameters_equivalent(
        &self,
        parameters: &[RawSignature],
        base_parameters: &[RawSignature],
        span: &SourceSpan,
    ) -> Result<bool, ImportError> {
        if parameters.len() != base_parameters.len() {
            return Ok(false);
        }
        for (parameter, base_parameter) in parameters.iter().zip(base_parameters) {
            let a = &parameter.raw_types[0];
            let b = &base_parameter.raw_types[0];
            if a == b {
                continue;
            }
            let relation = if let Some(selected) = self.selected_operation_equivalence(a, b) {
                Some(selected)
            } else if self.parameter_requires_erasure(parameter)?
                || self.parameter_requires_erasure(base_parameter)?
            {
                self.classifier_domain_relation(a, b, true)?
            } else {
                // Native nominal concrete signature identity, including equivalent
                // argument-free eType and eGenericType source encodings.
                Some(false)
            };
            match relation {
                Some(true) => {}
                Some(false) => return Ok(false),
                None if self.classifier(a)? == TargetKind::Datatype
                    && self.classifier(b)? == TargetKind::Datatype =>
                {
                    return Err(ImportError::new(
                        ErrorKind::Unsupported,
                        span,
                        "Operation parameter equivalence requires an explicit provider contract",
                    ));
                }
                None => return Ok(false),
            }
        }
        Ok(true)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenericOperation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<RuntimeMemberProjection>,
    pub member: SpecializedMember,
    pub raw_parameters: Vec<RawSignature>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenericOperationResolution {
    pub selected: GenericOperation,
    /// Other owner projections of the same winning source declaration.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_projections: Vec<GenericOperation>,
    pub overridden: Vec<GenericOperation>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenericOperationSet {
    pub receiver: crate::definitions::generic_types::RuntimeType,
    pub bounds: Vec<BoundCheck>,
    pub operations: Vec<GenericOperationResolution>,
}
struct Candidate {
    operation: GenericOperation,
    owner: DeclarationId,
    ancestors: BTreeSet<DeclarationId>,
}
fn selection_step(remaining: &mut usize, span: &SourceSpan) -> Result<(), ImportError> {
    *remaining = remaining.checked_sub(1).ok_or_else(|| {
        ImportError::new(
            ErrorKind::ResourceLimit,
            span,
            "Generic operation selection comparison budget exceeded (16384)",
        )
    })?;
    Ok(())
}
impl Resolver {
    /// Resolve declaration-erasure overload groups for a checked closed receiver.
    /// Retained generic formals still require explicit arguments before invocation.
    pub fn resolved_generic_operations(
        &self,
        receiver: &crate::definitions::generic_types::ClosedType,
    ) -> Result<GenericOperationSet, ImportError> {
        self.resolved_runtime_operations(&receiver.into())
    }
    pub fn resolved_runtime_operations(
        &self,
        receiver: &crate::definitions::generic_types::RuntimeType,
    ) -> Result<GenericOperationSet, ImportError> {
        // Without conversion relations or rigid parameters the set is a pure
        // function of the receiver over this resolver's fixed sources, so a
        // context's materializations resolve each receiver once.
        let cache = self.runtime_operations.as_ref().filter(|_| {
            self.rigid_parameters.is_empty()
                && self.provider_relations.is_empty()
                && self.constructor_relations.is_empty()
        });
        let Some(cache) = cache else {
            return self.resolve_runtime_operations(receiver);
        };
        if let Some(set) = cache.borrow().get(receiver.expression()) {
            return Ok(set.clone());
        }
        let set = self.resolve_runtime_operations(receiver)?;
        cache
            .borrow_mut()
            .insert(receiver.expression().clone(), set.clone());
        Ok(set)
    }
    fn resolve_runtime_operations(
        &self,
        receiver: &crate::definitions::generic_types::RuntimeType,
    ) -> Result<GenericOperationSet, ImportError> {
        let report = self.runtime_members(receiver)?;
        let raw_context = receiver.has_raw()
            || report.source_members.iter().any(|member| {
                crate::definitions::generic_types::RuntimeType::new(member.owner.clone())
                    .map_or(true, |owner| owner.has_raw())
            });
        let mut seen = BTreeSet::new();
        let mut groups: Vec<Vec<Candidate>> = Vec::new();
        let mut remaining = 16_384;
        for projection in self.runtime_member_projections(receiver)? {
            let member = projection.source.clone();
            if !named(self.node(&member.declaration)?, "eOperations") {
                continue;
            }
            if seen.contains(&(member.declaration.clone(), member.owner.clone())) {
                continue;
            }
            if seen.len() >= 128 {
                return Err(ImportError::new(
                    ErrorKind::ResourceLimit,
                    &member.span,
                    "Generic operation selection candidate budget exceeded (128)",
                ));
            }
            seen.insert((member.declaration.clone(), member.owner.clone()));
            if member.name.is_empty() {
                return Err(error(&member.span, "Operation name is empty"));
            }
            let parameters = self.operation_raw_parameters(&member.declaration)?;
            let mut matching = None;
            for (index, group) in groups.iter().enumerate() {
                let mut equivalent = None;
                for prior in group {
                    selection_step(&mut remaining, &member.span)?;
                    let same = member.name == prior.operation.member.name
                        && self.operation_parameters_equivalent(
                            &parameters,
                            &prior.operation.raw_parameters,
                            &member.span,
                        )?;
                    if equivalent.is_some_and(|previous| previous != same) {
                        return Err(error(
                            &member.span,
                            "Inconsistent operation signature equivalence",
                        ));
                    }
                    equivalent = Some(same);
                }
                if equivalent == Some(true) {
                    if matching.is_some() {
                        return Err(error(
                            &member.span,
                            "Inconsistent operation signature equivalence groups",
                        ));
                    }
                    matching = Some(index);
                }
            }
            let owner = parent(&member.declaration)?;
            let mut ancestors = BTreeSet::from([owner.clone()]);
            for ancestor in self.ancestry_expression(&member.owner, &member.span)? {
                if let Type::Classifier { declaration, .. } = ancestor.expression {
                    ancestors.insert(declaration);
                }
            }
            let candidate = Candidate {
                owner,
                ancestors,
                operation: GenericOperation {
                    projection: raw_context.then_some(projection),
                    member,
                    raw_parameters: parameters,
                },
            };
            if let Some(index) = matching {
                if groups[index].iter().any(|prior| {
                    prior.owner == candidate.owner
                        && prior.operation.member.declaration
                            != candidate.operation.member.declaration
                }) {
                    return Err(error(
                        &candidate.operation.member.span,
                        "Duplicate local generic operation signature",
                    ));
                }
                groups[index].push(candidate);
            } else {
                groups.push(vec![candidate]);
            }
        }
        let mut operations = Vec::new();
        for group in groups {
            let mut winners = Vec::new();
            for (index, candidate) in group.iter().enumerate() {
                let mut dominates = true;
                for other in &group {
                    selection_step(&mut remaining, &candidate.operation.member.span)?;
                    if !candidate.ancestors.contains(&other.owner) {
                        dominates = false;
                        break;
                    }
                }
                if dominates {
                    winners.push(index);
                }
            }
            let winner = *winners.first().ok_or_else(|| {
                error(
                    &report.declaration_span,
                    "Ambiguous inherited generic operation",
                )
            })?;
            let declaration = &group[winner].operation.member.declaration;
            if winners
                .iter()
                .any(|index| &group[*index].operation.member.declaration != declaration)
            {
                return Err(error(
                    &report.declaration_span,
                    "Ambiguous inherited generic operation",
                ));
            }
            let selected = group[winner].operation.clone();
            let selected_projections = winners
                .iter()
                .copied()
                .filter(|index| *index != winner)
                .map(|index| group[index].operation.clone())
                .collect();
            let overridden = group
                .into_iter()
                .enumerate()
                .filter_map(|(index, candidate)| {
                    (!winners.contains(&index)).then_some(candidate.operation)
                })
                .collect();
            operations.push(GenericOperationResolution {
                selected,
                selected_projections,
                overridden,
            });
        }
        Ok(GenericOperationSet {
            receiver: receiver.clone(),
            bounds: report.bounds,
            operations,
        })
    }
}
