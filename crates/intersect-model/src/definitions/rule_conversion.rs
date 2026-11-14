//! Source-identified rule-conversion declarations. Algorithms belong to language packages.
use crate::definitions::{bundle::SourceSpan, conversion::ProviderId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleConversion {
    pub context: String,
    pub rule: String,
    pub declaration: SourceSpan,
    pub datatype: String,
    pub provider: ProviderId,
    pub keywords: BTreeMap<String, bool>,
}
