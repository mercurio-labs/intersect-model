//! Versioned engine contracts shared by execution hosts.
//!
//! Each host supplies the engines it installs and validates its own model bindings.

#![no_std]

/// A behavior or request type supported by an engine contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Capability {
    StateTransitions,
    NumericalIntegration,
    ThresholdTransitions,
    TimedTokenFlow,
    ScalarRootFinding,
    ScalarAffineSolving,
    /// Override the authored scenario with events supplied in the request.
    ScenarioEvents,
    /// Override numeric parameters by case.
    NumericCaseOverrides,
}

macro_rules! declare_engines {
    ($($engine:ident => ($id:literal, [$($capability:ident),*])),+ $(,)?) => {
        /// A known, exact version of an execution-engine contract.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[non_exhaustive]
        pub enum Engine {
            $($engine),+
        }

        /// The engine contracts defined by this catalog.
        pub const ENGINES: &[Engine] = &[$(Engine::$engine),+];

        impl Engine {
            /// The exact identifier used in profiles and traces.
            pub const fn id(self) -> &'static str {
                match self {
                    $(Self::$engine => $id),+
                }
            }

            pub const fn capabilities(self) -> &'static [Capability] {
                match self {
                    $(Self::$engine => &[$(Capability::$capability),*]),+
                }
            }
        }
    };
}

declare_engines! {
    StateMachine => ("intersect.state-machine@1", [StateTransitions, ScenarioEvents]),
    Ode => ("intersect.ode@1", [NumericalIntegration, NumericCaseOverrides]),
    CoupledOde => (
        "intersect.coupled-ode@1",
        [StateTransitions, NumericalIntegration, ThresholdTransitions, NumericCaseOverrides]
    ),
    TimedTokenFlow => ("intersect.timed-token-flow@1", [TimedTokenFlow]),
    ScalarRoot => ("intersect.scalar-root@1", [ScalarRootFinding]),
    ScalarAffine => ("intersect.scalar-affine@1", [ScalarAffineSolving]),
    StateAffine => (
        "intersect.state-affine@1",
        [StateTransitions, ScalarAffineSolving, ScenarioEvents]
    ),
}

impl Engine {
    /// Resolve a known contract by its exact identifier and version.
    pub fn from_id(id: &str) -> Option<Self> {
        ENGINES.iter().copied().find(|engine| engine.id() == id)
    }

    /// Resolve a contract supported by this host.
    pub fn select(id: &str, installed: &[Self]) -> Option<Self> {
        let engine = Self::from_id(id)?;
        installed.contains(&engine).then_some(engine)
    }

    pub fn supports(self, capability: Capability) -> bool {
        self.capabilities().contains(&capability)
    }
}

/// Recognize the identifier syntax independently of support for that contract.
///
/// A name starts with an ASCII letter or digit and may contain ASCII letters,
/// digits, periods, underscores and hyphens. The version is a positive decimal
/// `u32` written without leading zeroes.
pub fn is_versioned_id(id: &str) -> bool {
    id.split_once('@').is_some_and(|(name, version)| {
        name.as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            && !version.starts_with('0')
            && version.bytes().all(|byte| byte.is_ascii_digit())
            && version.parse::<u32>().is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_ids_are_exact_and_unique() {
        assert_eq!(ENGINES.len(), 7);
        for &engine in ENGINES {
            assert!(is_versioned_id(engine.id()));
            assert_eq!(Engine::from_id(engine.id()), Some(engine));
        }
        assert_eq!(Engine::from_id("intersect.state-machine"), None);
        assert_eq!(Engine::from_id("intersect.state-machine@2"), None);
        assert_eq!(Engine::from_id("INTERSECT.state-machine@1"), None);
    }

    #[test]
    fn a_host_selects_only_the_contracts_it_installs() {
        let installed = [Engine::ScalarAffine, Engine::CoupledOde];
        assert_eq!(
            Engine::select("intersect.scalar-affine@1", &installed),
            Some(Engine::ScalarAffine)
        );
        assert_eq!(
            Engine::select("intersect.state-machine@1", &installed),
            None
        );
        assert_eq!(Engine::select("intersect.coupled-ode@2", &installed), None);
        assert_eq!(Engine::select("intersect.coupled-ode@1", &[]), None);
    }

    #[test]
    fn request_capabilities_describe_existing_contracts() {
        for &engine in ENGINES {
            assert_eq!(
                engine.supports(Capability::ScenarioEvents),
                matches!(engine, Engine::StateMachine | Engine::StateAffine)
            );
            assert_eq!(
                engine.supports(Capability::NumericCaseOverrides),
                matches!(engine, Engine::Ode | Engine::CoupledOde)
            );
        }
        assert!(Engine::CoupledOde.supports(Capability::ThresholdTransitions));
        assert!(Engine::StateAffine.supports(Capability::ScalarAffineSolving));
    }

    #[test]
    fn identifier_syntax_accepts_unknown_contracts_and_rejects_malformed_versions() {
        assert!(is_versioned_id("other.future-engine@4294967295"));
        assert_eq!(Engine::from_id("other.future-engine@4294967295"), None);
        for malformed in [
            "",
            "@1",
            "-engine@1",
            "my engine@1",
            "engine@0",
            "engine@01",
            "engine@",
            "engine@-1",
            "engine@+1",
            "engine@4294967296",
            "engine@1@2",
        ] {
            assert!(!is_versioned_id(malformed), "{malformed}");
        }
    }
}
