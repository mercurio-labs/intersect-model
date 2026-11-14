//! Relations for the existing native scalar bindings; no ambient Java loading.
use super::*;
use crate::definitions::scalars::Builtin;

#[derive(Clone, Copy, PartialEq, Eq)]
enum NativeClass {
    Scalar(Builtin),
    Object,
}
fn relation_attr<'a>(
    node: &'a Node,
    name: &str,
    observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
) -> Result<Option<&'a str>, ImportError> {
    for attribute in &node.attributes {
        observation.work(1, &node.span.source)?;
        if attribute.name.namespace.is_none() && attribute.name.local == name {
            return Ok(Some(&attribute.value));
        }
    }
    Ok(None)
}
impl Resolver {
    fn native_scalar_class_observed(
        &self,
        declaration: &DeclarationId,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Option<NativeClass>, ImportError> {
        observation.work(1, &declaration.source)?;
        if self.classifier_observed(declaration, observation)? != TargetKind::Datatype
            || !self.formals_observed(declaration, observation)?.is_empty()
        {
            return Ok(None);
        }
        let node = self.node_observed(declaration, observation)?;
        let class = relation_attr(node, "instanceClassName", observation)?;
        let ty = relation_attr(node, "instanceTypeName", observation)?;
        for metadata in [class, ty].into_iter().flatten() {
            observation.work(metadata.len().div_ceil(256), &declaration.source)?;
        }
        if matches!((class, ty), (Some(class), Some(ty)) if class != ty) {
            return Ok(None);
        }
        if class.or(ty) == Some("java.lang.Object") {
            return Ok(Some(NativeClass::Object));
        }
        let name = relation_attr(node, "name", observation)?
            .ok_or_else(|| error(&node.span, "Missing datatype name"))?;
        observation.work(name.len().div_ceil(256), &declaration.source)?;
        if name.is_empty() || declaration.path.is_empty() {
            return Err(error(
                &node.span,
                "Datatype requires a name and classifier declaration identity",
            ));
        }
        // Share the datatype constructor's exact mapping without constructing a
        // discarded owned Datatype and unsupported-binding diagnostic.
        Ok(class
            .or(ty)
            .and_then(crate::definitions::datatypes::native_binding)
            .map(NativeClass::Scalar))
    }

    /// Type-name equivalence and boundedness are distinct. Neither changes IDs.
    #[cfg(test)]
    pub(super) fn native_scalar_relation(
        &self,
        actual: &DeclarationId,
        bound: &DeclarationId,
        equivalent: bool,
    ) -> Result<Option<bool>, ImportError> {
        self.native_scalar_relation_observed(
            actual,
            bound,
            equivalent,
            &crate::definitions::ecore::ParserObservation::new(
                &crate::definitions::conversion::InvocationContext::default(),
            ),
        )
    }
    pub(super) fn native_scalar_relation_observed(
        &self,
        actual: &DeclarationId,
        bound: &DeclarationId,
        equivalent: bool,
        observation: &crate::definitions::ecore::ParserObservation<'_, '_>,
    ) -> Result<Option<bool>, ImportError> {
        observation.work(1, &actual.source)?;
        let actual_class = self.native_scalar_class_observed(actual, observation)?;
        let bound_class = self.native_scalar_class_observed(bound, observation)?;
        let (Some(actual_class), Some(bound_class)) = (actual_class, bound_class) else {
            use crate::definitions::conversion::ClassifierRelation as R;
            for ((candidate_actual, candidate_bound), relation) in &self.provider_relations {
                observation.work(1, &actual.source)?;
                if candidate_actual == actual && candidate_bound == bound {
                    Self::observe_relation_consumption(
                        &self.consumed_provider_relations,
                        actual,
                        observation,
                    )?;
                    Self::observe_relation_consumption(
                        &self.consumed_provider_relations,
                        bound,
                        observation,
                    )?;
                    return Ok(Some(
                        *relation == R::Equivalent || (!equivalent && *relation == R::Subtype),
                    ));
                }
            }
            return Ok(None);
        };
        Ok(Some(if equivalent || actual_class == bound_class {
            actual_class == bound_class
        } else {
            match (actual_class, bound_class) {
                // Nullable scalars represent reference classes/arrays. Primitive
                // scalars neither widen nor box in Class assignability.
                (NativeClass::Scalar(codec), NativeClass::Object) => codec.nullable(),
                _ => false,
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scalar_relation_matrix_keeps_primitives_wrappers_and_object_distinct() {
        let resolver = Resolver::new(&DefinitionBundle {
            version: 1,
            sources: vec![],
            namespaces: vec![],
        })
        .unwrap();
        let types = [
            ("EBoolean", false),
            ("EByte", false),
            ("EChar", false),
            ("EShort", false),
            ("EInt", false),
            ("ELong", false),
            ("EFloat", false),
            ("EDouble", false),
            ("EBooleanObject", true),
            ("EByteObject", true),
            ("ECharacterObject", true),
            ("EShortObject", true),
            ("EIntegerObject", true),
            ("ELongObject", true),
            ("EFloatObject", true),
            ("EDoubleObject", true),
            ("EBigDecimal", true),
            ("EBigInteger", true),
            ("EString", true),
            ("EDate", true),
            ("EByteArray", true),
            ("EJavaObject", true),
        ];
        for (actual, reference) in types {
            for (bound, _) in types {
                let a = resolver
                    .standard
                    .resolve(&resolver.standard_id, &format!("#//{actual}"))
                    .unwrap();
                let b = resolver
                    .standard
                    .resolve(&resolver.standard_id, &format!("#//{bound}"))
                    .unwrap();
                assert_eq!(
                    resolver.native_scalar_relation(&a, &b, true).unwrap(),
                    Some(actual == bound)
                );
                assert_eq!(
                    resolver.native_scalar_relation(&a, &b, false).unwrap(),
                    Some(actual == bound || (bound == "EJavaObject" && reference)),
                    "{actual} -> {bound}"
                );
            }
        }
    }
}
