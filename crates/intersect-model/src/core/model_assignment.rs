//! Versioned explicit assignment markers, independent of effective stored values.
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeSet;

/// Preserved assignment evidence. Schema-aware services must validate feature
/// identity and applicability before interpreting these markers as Ecore state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AssignmentState {
    version: u8,
    set_features: BTreeSet<String>,
}
impl AssignmentState {
    pub fn new(features: impl IntoIterator<Item = impl Into<String>>) -> Result<Self, String> {
        let mut set_features = BTreeSet::new();
        for feature in features {
            let feature = feature.into();
            if feature.is_empty() {
                return Err("Assignment-state feature name must be nonempty".into());
            }
            if !set_features.insert(feature) {
                return Err("Duplicate assignment-state feature".into());
            }
        }
        Ok(Self {
            version: 1,
            set_features,
        })
    }
    /// Return updated raw evidence; feature identity/applicability remains the
    /// schema-aware caller's responsibility. Effective values are unchanged.
    pub fn with_assignment(&self, feature: &str, assigned: bool) -> Result<Self, String> {
        if feature.is_empty() {
            return Err("Assignment-state feature name must be nonempty".into());
        }
        let mut next = self.clone();
        if assigned {
            next.set_features.insert(feature.into());
        } else {
            next.set_features.remove(feature);
        }
        Ok(next)
    }
    pub fn explicitly_set(&self, feature: &str) -> bool {
        self.set_features.contains(feature)
    }
    pub(crate) fn next_set_feature(&self, after: Option<&str>) -> Option<&str> {
        use std::ops::Bound::{Excluded, Unbounded};
        match after {
            Some(last) => self
                .set_features
                .range::<str, _>((Excluded(last), Unbounded))
                .next(),
            None => self.set_features.iter().next(),
        }
        .map(String::as_str)
    }
    pub fn set_features(&self) -> impl ExactSizeIterator<Item = &str> {
        self.set_features.iter().map(String::as_str)
    }
}
impl<'de> Deserialize<'de> for AssignmentState {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            version: u8,
            set_features: Vec<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        if wire.version != 1 {
            return Err(serde::de::Error::custom(
                "Unsupported assignment-state version",
            ));
        }
        Self::new(wire.set_features).map_err(serde::de::Error::custom)
    }
}
