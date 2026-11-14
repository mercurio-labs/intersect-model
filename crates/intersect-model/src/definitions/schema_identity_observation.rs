//! Canonical identity formatting and observed allocation-free comparisons.
use super::*;
use crate::definitions::conversion::InvocationContext;
use std::fmt::Write;
fn text_equal(a: &str, b: &str, observation: &InvocationContext<'_>) -> Result<bool, String> {
    observation.work(1 + a.len().max(b.len()).div_ceil(64))?;
    Ok(a == b)
}
pub(crate) fn member_identity_equal_observed(
    a: &MemberIdentity,
    b: &MemberIdentity,
    observation: &InvocationContext<'_>,
) -> Result<bool, String> {
    observation.work(1)?;
    Ok(match (a, b) {
        (MemberIdentity::Source { declaration: a }, MemberIdentity::Source { declaration: b }) => {
            text_equal(&a.source.uri, &b.source.uri, observation)?
                && text_equal(&a.source.sha256, &b.source.sha256, observation)?
                && {
                    observation.work(1)?;
                    let mut equal = a.path.len() == b.path.len();
                    if equal {
                        for (a, b) in a.path.iter().zip(&b.path) {
                            observation.work(1)?;
                            if a != b {
                                equal = false;
                                break;
                            }
                        }
                    }
                    equal
                }
        }
        (
            MemberIdentity::Programmatic {
                revision: a,
                namespace: b,
                owner: c,
                name: d,
            },
            MemberIdentity::Programmatic {
                revision: e,
                namespace: f,
                owner: g,
                name: h,
            },
        ) => {
            text_equal(a, e, observation)?
                && text_equal(b, f, observation)?
                && text_equal(c, g, observation)?
                && text_equal(d, h, observation)?
        }
        _ => false,
    })
}
/// Precharge both the helper's owner lookup and the subsequent canonical
/// owner/name resolution. No selection or identity authority is created here.
pub(crate) fn admit_feature_lookup_observed<A: RuntimeAlternative>(
    schema: &SchemaRepresentation<A>,
    owner: &str,
    name: &str,
    observation: &InvocationContext<'_>,
) -> Result<(), String> {
    let owners = (usize::BITS - schema.classes.len().leading_zeros()) as usize;
    observation.work(1 + 2 * owners * (1 + owner.len().div_ceil(64)))?;
    if let Some(class) = schema.classes.get(owner) {
        observation.work(
            1 + (usize::BITS - class.features.len().leading_zeros()) as usize
                * (1 + name.len().div_ceil(64)),
        )?;
    }
    Ok(())
}
struct IdentityDisplay<'a, A: RuntimeAlternative>(&'a SchemaRepresentation<A>, &'a str);
impl<A: RuntimeAlternative> std::fmt::Display for IdentityDisplay<'_, A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let schema = self.0;
        if let Some(i) = schema.instantiations.get(self.1) {
            return f.write_str(i.identity());
        }
        if let Some(d) = schema.declarations.get(self.1) {
            write!(f, "{}@{}#", d.source.uri, d.source.sha256)?;
            for (index, step) in d.path.iter().enumerate() {
                if index != 0 {
                    f.write_str("/")?;
                }
                write!(f, "{step}")?;
            }
            Ok(())
        } else {
            write!(f, "{}#//{}", schema.uri, self.1)
        }
    }
}
struct Comparison<'a, 'scope> {
    expected: &'a str,
    position: usize,
    equal: bool,
    observation: &'a InvocationContext<'scope>,
    error: Option<String>,
}
impl std::fmt::Write for Comparison<'_, '_> {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        if let Err(error) = self.observation.work(1 + text.len().div_ceil(64)) {
            self.error = Some(error);
            return Err(std::fmt::Error);
        }
        let end = self
            .position
            .checked_add(text.len())
            .ok_or(std::fmt::Error)?;
        self.equal &= self.expected.as_bytes().get(self.position..end) == Some(text.as_bytes());
        self.position = end;
        Ok(())
    }
}
impl<A: RuntimeAlternative> SchemaRepresentation<A> {
    pub(super) fn formatted_identity(&self, class: &str) -> String {
        IdentityDisplay(self, class).to_string()
    }
    /// Compare canonical identity without allocating an identity string or path vector.
    pub(crate) fn identity_matches_observed(
        &self,
        class: &str,
        expected: &str,
        observation: &InvocationContext<'_>,
    ) -> Result<bool, String> {
        observation.work(
            1 + (1 + class.len().div_ceil(64))
                * ((usize::BITS - self.instantiations.len().leading_zeros()) as usize
                    + (usize::BITS - self.declarations.len().leading_zeros()) as usize),
        )?;
        let mut comparison = Comparison {
            expected,
            position: 0,
            equal: true,
            observation,
            error: None,
        };
        let formatted = write!(&mut comparison, "{}", IdentityDisplay(self, class));
        if let Some(error) = comparison.error {
            return Err(error);
        }
        formatted.map_err(|_| "Schema identity length overflow".to_string())?;
        Ok(comparison.equal && comparison.position == expected.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::conversion::{InvocationObserver, ProviderId};
    use std::{cell::Cell, rc::Rc};
    struct Meter(Cell<usize>);
    impl InvocationObserver for Meter {
        fn before_invoke(&self, _: &ProviderId) -> Result<(), String> {
            Err("No provider".into())
        }
        fn work(&self, units: usize) -> Result<(), String> {
            let left = self
                .0
                .get()
                .checked_sub(units)
                .ok_or("Identity work exhausted")?;
            self.0.set(left);
            Ok(())
        }
        fn retain_storage(&self, _: usize) -> Result<(), String> {
            Err("Comparison must not allocate retained identities".into())
        }
    }
    #[test]
    fn canonical_identity_streaming_preserves_source_paths_unicode_and_denial() -> Result<(), String>
    {
        let mut schema: Schema = serde_json::from_value(
            serde_json::json!({"revision":"r","uri":"memory:ÃŽÂ´","name":"schema","classes":{}}),
        )
        .map_err(|e| e.to_string())?;
        let meter = Rc::new(Meter(Cell::new(10_000)));
        let observation = InvocationContext::new(meter.clone());
        assert_eq!(schema.identity("ÃŽÂ©"), "memory:ÃŽÂ´#//ÃŽÂ©");
        assert!(schema.identity_matches_observed("ÃŽÂ©", "memory:ÃŽÂ´#//ÃŽÂ©", &observation)?);
        let source =
            crate::definitions::bundle::SourceId::for_text("memory:ÃŽÂ´.ecore", "canonical source");
        schema.declarations.insert(
            "A".into(),
            ClassifierDeclaration {
                source: source.clone(),
                path: vec![0, 21, usize::MAX],
                namespace: "n".into(),
                name: "A".into(),
            },
        );
        let expected = format!("{}@{}#0/21/{}", source.uri, source.sha256, usize::MAX);
        assert_eq!(schema.identity("A"), expected);
        assert!(schema.identity_matches_observed("A", &expected, &observation)?);
        assert!(!schema.identity_matches_observed("A", &(expected.clone() + "x"), &observation)?);
        assert!(!schema.identity_matches_observed("A", "wrong", &observation)?);
        meter.0.set(0);
        assert!(
            schema
                .identity_matches_observed("A", &expected, &observation)
                .is_err()
        );
        Ok(())
    }
    #[test]
    fn member_identity_comparison_charges_each_source_component_exactly_before_use()
    -> Result<(), String> {
        let identity = MemberIdentity::Programmatic {
            revision: "revision".into(),
            namespace: "namespace".into(),
            owner: "owner".into(),
            name: "name".into(),
        };
        let meter = Rc::new(Meter(Cell::new(1000)));
        let observation = InvocationContext::new(meter.clone());
        assert!(member_identity_equal_observed(
            &identity,
            &identity,
            &observation
        )?);
        let exact = 1000 - meter.0.get();
        meter.0.set(exact);
        assert!(member_identity_equal_observed(
            &identity,
            &identity,
            &observation
        )?);
        assert_eq!(meter.0.get(), 0);
        meter.0.set(exact - 1);
        assert!(member_identity_equal_observed(&identity, &identity, &observation).is_err());
        let mut different = identity.clone();
        if let MemberIdentity::Programmatic { name, .. } = &mut different {
            *name = "other".into();
        }
        meter.0.set(1000);
        assert!(!member_identity_equal_observed(
            &identity,
            &different,
            &observation
        )?);
        Ok(())
    }
}
