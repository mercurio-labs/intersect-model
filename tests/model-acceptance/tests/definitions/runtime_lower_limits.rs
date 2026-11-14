use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    ecore::ErrorKind,
    generic_resolution::{Resolver, RuntimeFeatureDomain},
    generic_types::{RuntimeType, Type},
    resources::DeclarationId,
};
struct Fixture {
    bundle: DefinitionBundle,
    source: DefinitionSource,
}
impl Fixture {
    fn new(parameters: &str, extra: &str, extra_feature: &str) -> Self {
        let text = format!(
            r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="limits" nsURI="urn:lower-limits">
<eClassifiers x:type="e:EClass" name="Base"/>
<eClassifiers x:type="e:EClass" name="Other"/>
<eClassifiers x:type="e:EClass" name="Box"><eTypeParameters name="E"/></eClassifiers>
<eClassifiers x:type="e:EClass" name="Owner">{parameters}<eStructuralFeatures x:type="e:EReference" name="value"><eGenericType eClassifier="#//Box"><eTypeArguments><eLowerBound eTypeParameter="#//Owner/T0"/></eTypeArguments></eGenericType></eStructuralFeatures>{extra_feature}</eClassifiers>{extra}</e:EPackage>"##
        );
        let source = DefinitionSource::new("memory:lower-limits.ecore", SourceKind::Ecore, text);
        Self {
            bundle: DefinitionBundle {
                version: 1,
                sources: vec![source.clone()],
                namespaces: vec![],
            },
            source,
        }
    }
    fn ty(&self, n: usize, arguments: Vec<Type>, raw: bool) -> RuntimeType {
        RuntimeType::new(Type::Classifier {
            declaration: DeclarationId {
                source: self.source.id.clone(),
                path: vec![n],
            },
            arguments,
            raw,
        })
        .unwrap()
    }
    fn boxed(&self, argument: &RuntimeType) -> RuntimeType {
        self.ty(2, vec![argument.expression().clone()], false)
    }
    fn domain(
        &self,
        resolver: &Resolver,
        receiver: &RuntimeType,
        name: &str,
    ) -> RuntimeFeatureDomain {
        resolver
            .runtime_feature_domains(receiver)
            .unwrap()
            .into_iter()
            .find(|domain| domain.projection.source.name == name)
            .unwrap()
    }
}
fn parameter(name: &str, bounds: &str) -> String {
    format!("<eTypeParameters name=\"{name}\">{bounds}</eTypeParameters>")
}
fn bound(name: &str) -> String {
    format!("<eBounds eTypeParameter=\"#//Owner/{name}\"/>")
}
const BASE: &str = r##"<eBounds eClassifier="#//Base"/>"##;
fn diamond(height: usize) -> Fixture {
    // Direct bounds may reference only earlier source formals.
    let mut parameters = String::new();
    for level in (1..=height).rev() {
        let bounds = if level == height {
            BASE.to_owned()
        } else {
            bound(&format!("A{}", level + 1)) + bound(&format!("B{}", level + 1)).as_str()
        };
        parameters.push_str(&parameter(&format!("A{level}"), &bounds));
        parameters.push_str(&parameter(&format!("B{level}"), &bounds));
    }
    parameters.push_str(&parameter("T0", &(bound("A1") + bound("B1").as_str())));
    Fixture::new(&parameters, "", "")
}
#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn lower_any_diamond_uses_one_comparison_budget_and_recovers() {
    for (height, over) in [(12, false), (13, true)] {
        let f = diamond(height);
        let resolver = Resolver::new(&f.bundle).unwrap();
        let raw = f.ty(3, vec![], true);
        let domain = f.domain(&resolver, &raw, "value");
        let candidate = f.boxed(&f.ty(1, vec![], false));
        // Prove source extraction and candidate checking succeed before invoking
        // the lower matcher. The graph has only 1 + 2*height source parameters.
        resolver.runtime_members(&candidate).unwrap();
        let result = resolver.runtime_feature_accepts(&domain, &candidate);
        if over {
            let error = result.unwrap_err();
            assert_eq!(error.kind, ErrorKind::ResourceLimit);
            assert_eq!(error.message, "Generic conformance budget exceeded");
            assert_eq!(error.span.source, f.source.id);
        } else {
            assert!(!result.unwrap());
        }
        // A true first path and a second identical request prove failed ANY
        // exploration did not leak active obligations or consume later budgets.
        let accepted = f.boxed(&f.ty(0, vec![], false));
        assert!(
            resolver
                .runtime_feature_accepts(&domain, &accepted)
                .unwrap()
        );
        assert!(
            resolver
                .runtime_feature_accepts(&domain, &accepted)
                .unwrap()
        );
    }
}
#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn lower_parameter_chain_depth_limit_is_distinct_from_comparison_exhaustion() {
    for (count, over) in [(126, false), (127, true)] {
        let parameters = (0..count)
            .rev()
            .map(|index| {
                parameter(
                    &format!("T{index}"),
                    &if index + 1 == count {
                        BASE.to_owned()
                    } else {
                        bound(&format!("T{}", index + 1))
                    },
                )
            })
            .collect::<String>();
        let f = Fixture::new(&parameters, "", "");
        let resolver = Resolver::new(&f.bundle).unwrap();
        let domain = f.domain(&resolver, &f.ty(3, vec![], true), "value");
        let candidate = f.boxed(&f.ty(1, vec![], false));
        resolver.runtime_members(&candidate).unwrap();
        let result = resolver.runtime_feature_accepts(&domain, &candidate);
        if over {
            let error = result.unwrap_err();
            assert_eq!(error.kind, ErrorKind::ResourceLimit);
            assert_eq!(error.message, "Generic conformance budget exceeded");
        } else {
            assert!(!result.unwrap());
        }
        // Independent, source-checked concrete owner context has no raw chain.
        let owner = f.ty(
            3,
            vec![f.ty(0, vec![], false).expression().clone(); count],
            false,
        );
        let concrete = f.domain(&resolver, &owner, "value");
        assert!(
            resolver
                .runtime_feature_accepts(&concrete, &f.boxed(&f.ty(0, vec![], false)))
                .unwrap()
        );
    }
}
#[test]
fn direct_parameter_bound_cycles_reject_before_runtime_recursion() {
    let f = Fixture::new(
        &(parameter("T0", &bound("T1")) + parameter("T1", &bound("T0")).as_str()),
        "",
        "",
    );
    let resolver = Resolver::new(&f.bundle).unwrap();
    let raw = f.ty(3, vec![], true);
    let domain = f.domain(&resolver, &raw, "value");
    let candidate = f.boxed(&f.ty(0, vec![], false));
    let error = resolver
        .runtime_feature_accepts(&domain, &candidate)
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidDefinition);
    // This is the established source-bound validation guard, not evidence that
    // an invalid direct cycle reached the runtime active-obligation algorithm.
    assert_eq!(
        error.message,
        "Direct parameter bound cannot reference itself or a later parameter"
    );
    let repeated = resolver
        .runtime_feature_accepts(&domain, &candidate)
        .unwrap_err();
    assert_eq!(repeated, error);
}
#[test]
fn guarded_recursive_bounds_do_not_replace_lower_invariant_arguments() {
    let parameters = parameter(
        "T0",
        r##"<eBounds eClassifier="#//Box"><eTypeArguments eTypeParameter="#//Owner/T0"/></eBounds>"##,
    );
    let f = Fixture::new(
        &parameters,
        r##"<eClassifiers x:type="e:EClass" name="Self"><eGenericSuperTypes eClassifier="#//Box"><eTypeArguments eClassifier="#//Self"/></eGenericSuperTypes></eClassifiers>"##,
        r##"<eStructuralFeatures x:type="e:EReference" name="guarded"><eGenericType eTypeParameter="#//Owner/T0"/></eStructuralFeatures>"##,
    );
    let resolver = Resolver::new(&f.bundle).unwrap();
    let raw = f.ty(3, vec![], true);
    let self_type = f.ty(4, vec![], false);
    let guarded = f.domain(&resolver, &raw, "guarded");
    assert!(
        resolver
            .runtime_feature_accepts(&guarded, &self_type)
            .unwrap()
    );
    let lower = f.domain(&resolver, &raw, "value");
    let candidate = f.boxed(&f.boxed(&self_type));
    assert!(
        !resolver
            .runtime_feature_accepts(&lower, &candidate)
            .unwrap()
    );
    let concrete = f.ty(3, vec![self_type.expression().clone()], false);
    assert!(
        resolver
            .runtime_feature_accepts(&f.domain(&resolver, &concrete, "value"), &candidate)
            .unwrap()
    );
}
