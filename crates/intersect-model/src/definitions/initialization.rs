//! Per-class initialization identity, distinct from datatype constructor policies.
//! A contract is data. Executable consumption requires the selected language provider.
use crate::definitions::{bundle::SourceId, conversion::ProviderId, schema::MemberIdentity};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassInitializationContract {
    pub version: u32,
    pub schema_revision: String,
    pub class: String,
    pub class_identity: String,
    pub provider: ProviderId,
    /// Exact sources establishing this initialization policy, separately retained
    /// from the Ecore literals and datatype conversion provider dependencies.
    pub sources: Vec<SourceId>,
    /// Exactly the stored members initialized by this provider, including inherited
    /// declaration identities. Plans cannot silently omit or add members.
    pub members: Vec<MemberIdentity>,
}

/// Returned only after successful atomic construction. Deserialized receipts are
/// retained evidence, not permission to skip source or provider validation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitializationReceipt {
    pub contract: ClassInitializationContract,
    pub object: String,
    pub authored_members: Vec<MemberIdentity>,
}
