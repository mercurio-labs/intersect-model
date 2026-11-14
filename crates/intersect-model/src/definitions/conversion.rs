//! Checked custom conversion contracts; algorithms are supplied by language packages.
use crate::definitions::{datatypes::Datatype, resources::DeclarationId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{borrow::Borrow, collections::BTreeMap};
#[path = "conversion_validation_preparation.rs"]
mod validation_preparation;
pub use validation_preparation::{
    ConversionValidationAdapter, ConversionValidationCursor, ConversionValidationInput,
    ConversionValidationJob, ConversionValidationPreparation, ConversionValidationProgress,
    ConversionValidationStep, OwnedConversionValidationPreparation,
};

/// Category of a required provider lookup that could not resolve an exact identity.
/// This reports dispatch failure, not a provider's rejection of a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderDependencyKind {
    Conversion,
    AppliedConversion,
    Constructor,
}
/// Evaluation-owned receipt sink. Metadata inspection is never invocation.
pub trait InvocationObserver {
    /// Called after dispatch checks and immediately before the semantic method.
    /// An error prevents invocation; implementations must preflight before copying.
    fn before_invoke(&self, provider: &ProviderId) -> Result<(), String>;
    /// A required dispatch lookup failed before invocation. This does not create
    /// an invocation receipt. Existing observers may ignore the additional signal.
    fn dependency_unavailable(&self, _kind: ProviderDependencyKind) {}
    /// An observed registry operation failed. Default paths add no charge or result change.
    /// This is not an invocation receipt; observers may latch the failure.
    fn failed_result(&self, _error: &str) {}
    /// Observe cooperative control inside a provider without recording invocation.
    fn checkpoint(&self) -> Result<(), String> {
        Ok(())
    }
    /// Precharge explicit provider work before executing it. Observers without a
    /// work budget retain cooperative cancellation through the same checkpoint.
    fn work(&self, _units: usize) -> Result<(), String> {
        self.checkpoint()
    }
    /// Reserve cumulative logical retained storage before making an owned copy.
    /// This is separate from traversal work and semantic invocation evidence.
    /// Unbounded observers still provide the same cancellation checkpoint.
    fn retain_storage(&self, _bytes: usize) -> Result<(), String> {
        self.checkpoint()
    }
}
#[derive(Clone, Default)]
pub struct InvocationContext<'a>(Option<std::rc::Rc<dyn InvocationObserver + 'a>>);
impl<'a> InvocationContext<'a> {
    /// Whether an operation observer owns resource/cancellation policy.
    pub fn has_observer(&self) -> bool {
        self.0.is_some()
    }
    pub fn new(observer: std::rc::Rc<dyn InvocationObserver + 'a>) -> Self {
        Self(Some(observer))
    }
    pub(crate) fn dependency_unavailable(&self, kind: ProviderDependencyKind) {
        if let Some(observer) = &self.0 {
            observer.dependency_unavailable(kind);
        }
    }
    /// Report a failed operation to the original observer without invoking a
    /// provider or resetting its work, storage or sticky-failure policy.
    pub fn failed_result(&self, error: &str) {
        if let Some(observer) = &self.0 {
            observer.failed_result(error);
        }
    }
    fn complete_result<T>(&self, result: Result<T, String>) -> Result<T, String> {
        if let Err(error) = &result {
            self.failed_result(error);
        }
        result
    }
    pub fn checkpoint(&self) -> Result<(), String> {
        match &self.0 {
            Some(observer) => observer.checkpoint(),
            None => Ok(()),
        }
    }
    pub fn work(&self, units: usize) -> Result<(), String> {
        match &self.0 {
            Some(observer) => observer.work(units),
            None => Ok(()),
        }
    }
    pub fn retain_storage(&self, bytes: usize) -> Result<(), String> {
        match &self.0 {
            Some(observer) => observer.retain_storage(bytes),
            None => Ok(()),
        }
    }
    pub(crate) fn invoke(&self, provider: &ProviderId) -> Result<(), String> {
        match &self.0 {
            Some(observer) => observer.before_invoke(provider),
            None => Ok(()),
        }
    }
}
#[path = "constructor_conversion.rs"]
mod constructors;
pub use constructors::{ConstructorContract, ConstructorProvider, ConstructorRegistry};
#[path = "application_conversion.rs"]
mod applications;
pub use applications::{
    AppliedConversionContract, AppliedConversionProvider, AppliedConversionRegistry,
    AppliedConvertedValue,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderId {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionContract {
    /// Exact provider dependencies; ordered canonically, never executable code.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declarations: Vec<DeclarationId>,
    /// Ordered element/key/value policies, including nested converter identity.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<MemberContract>,
    pub datatype: DeclarationId,
    pub provider: ProviderId,
    pub serializable: bool,
}

/// Collection dependencies are definition data; validators belong to language packages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberContract {
    pub nullable: bool,
    pub value: MemberType,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemberType {
    Builtin {
        codec: crate::definitions::scalars::Builtin,
    },
    Datatype {
        declaration: DeclarationId,
    },
    Enumeration {
        declaration: DeclarationId,
    },
    Converted {
        contract: Box<ConversionContract>,
    },
    Applied {
        contract: Box<AppliedConversionContract>,
    },
    Reference {
        class: DeclarationId,
        context: Box<ReferenceContext>,
    },
    RuntimeReference {
        target: crate::definitions::generic_types::RuntimeType,
        context: Box<ReferenceContext>,
    },
}

/// Retained input for a checked immutable model-reference domain. Not executable code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceContext {
    pub bundle: crate::definitions::bundle::DefinitionBundle,
    pub entry: crate::definitions::bundle::SourceId,
    pub namespace: String,
    pub conversions: Vec<ConversionContract>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub materialized_types: Vec<crate::definitions::generic_types::RuntimeType>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applied_conversions: Vec<AppliedConversionContract>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constructors: Vec<ConstructorContract>,
    pub model: Value,
}
impl ReferenceContext {
    pub fn digest(&self) -> Result<String, String> {
        self.digest_with_work(&|_| Ok(()))
    }
    /// Hash the exact existing wire identity without retaining encoded JSON.
    /// The callback precharges emitted bytes before hashing. This is synchronous,
    /// not a retained serialization continuation, and records no provider call.
    pub fn digest_with_work(
        &self,
        work: &dyn Fn(usize) -> Result<(), String>,
    ) -> Result<String, String> {
        crate::definitions::json_digest::digest_with_work(self, work)
    }
}

mod payload {
    pub trait Sealed {}
    impl Sealed for serde_json::Value {}
    impl Sealed for &serde_json::Value {}
}

/// Canonical payloads with a stable immutable Value identity for the lifetime
/// of a validation job. Arbitrary Borrow implementations cannot establish that
/// authority: they can return a different Value between polling transitions.
/// The official generic wire schema supports the owned and borrowed forms.
pub trait ConversionPayload: Borrow<Value> + payload::Sealed {}
impl ConversionPayload for Value {}
impl ConversionPayload for &Value {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConvertedValue<Payload = Value> {
    Converted {
        datatype: DeclarationId,
        provider: ProviderId,
        value: Payload,
    },
}

/// Relationship between native instance domains, never a conversion of nominal values.
/// `Subtype` means every instance of the source is accepted by the target domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassifierRelation {
    Equivalent,
    Subtype,
    Supertype,
    Unrelated,
}
impl ClassifierRelation {
    fn reverse(self) -> Self {
        match self {
            Self::Equivalent => Self::Equivalent,
            Self::Subtype => Self::Supertype,
            Self::Supertype => Self::Subtype,
            Self::Unrelated => Self::Unrelated,
        }
    }
}

/// Implementations must be deterministic and receive host resources explicitly.
/// Registration is not proof that the provider implements a language's semantics.
pub trait ConversionProvider {
    fn contract(&self) -> &ConversionContract;
    /// Explicit provider-owned cooperative validation. Observed hooks alone do
    /// not supply a continuation. The default keeps existing atomic validation.
    fn retained_validation(&self) -> Option<&dyn ConversionValidationAdapter> {
        None
    }

    /// Optional native instance-domain relation to an exact registered contract.
    /// No relation is inferred from provider names, JSON shape or lexical codecs.
    /// Implementations must inspect all relevant versioned contract dependencies.
    /// This describes non-parameterized classifier domains; generic argument
    /// substitution requires a separate checked consumer.
    fn classifier_relation(
        &self,
        _target: &ConversionContract,
    ) -> Result<Option<ClassifierRelation>, String> {
        Ok(None)
    }
    fn parse(&self, text: &str) -> Result<Value, String>;
    fn format(&self, value: &Value) -> Result<String, String>;
    fn validate(&self, value: &Value) -> Result<(), String>;
    fn default_value(&self) -> Result<Option<Value>, String>;
    fn validate_observed(&self, value: &Value, _context: &InvocationContext) -> Result<(), String> {
        self.validate(value)
    }
    fn parse_observed(&self, text: &str, _context: &InvocationContext) -> Result<Value, String> {
        self.parse(text)
    }
    fn format_observed(
        &self,
        value: &Value,
        _context: &InvocationContext,
    ) -> Result<String, String> {
        self.format(value)
    }
    fn default_value_observed(
        &self,
        _context: &InvocationContext,
    ) -> Result<Option<Value>, String> {
        self.default_value()
    }
}

/// Shared provider authority preserves the existing borrowed registration API.
/// Neither form is deserializable or creates an implementation from metadata.
enum ProviderRef<'a, T: ?Sized> {
    Borrowed(&'a T),
    Shared(std::rc::Rc<T>),
}
impl<T: ?Sized> Clone for ProviderRef<'_, T> {
    fn clone(&self) -> Self {
        match self {
            Self::Borrowed(value) => Self::Borrowed(value),
            Self::Shared(value) => Self::Shared(value.clone()),
        }
    }
}
impl<T: ?Sized> std::ops::Deref for ProviderRef<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        match self {
            Self::Borrowed(value) => value,
            Self::Shared(value) => value.as_ref(),
        }
    }
}
#[derive(Clone)]
enum ConversionImplementation<'a> {
    Direct(ProviderRef<'a, dyn ConversionProvider + 'a>),
    Applied(ProviderRef<'a, dyn AppliedConversionProvider + 'a>),
}
impl ConversionImplementation<'_> {
    fn get(&self) -> &dyn ConversionProvider {
        match self {
            Self::Direct(value) => &**value,
            Self::Applied(value) => value.implementation(),
        }
    }
}

#[derive(Clone)]
struct Registration<'a> {
    contract: ConversionContract,
    implementation: ConversionImplementation<'a>,
}

#[derive(Clone, Default)]
pub struct ConversionRegistry<'a> {
    observation: InvocationContext<'a>,
    constructors: ConstructorRegistry<'a>,
    constructor_dependencies: BTreeMap<DeclarationId, ConstructorContract>,
    applied: AppliedConversionRegistry<'a>,
    compilation_dependencies: std::collections::BTreeSet<DeclarationId>,
    providers: BTreeMap<DeclarationId, Registration<'a>>,
}
/// Counts encoded owned registration metadata before an observed registry clone.
/// Provider implementations and the observation context are borrowed/shared, not copied.
struct ObservationCopyBudget {
    shape: crate::definitions::metadata_limits::TraversalBudget,
    remaining: usize,
    used: usize,
}
impl ObservationCopyBudget {
    fn add(&mut self, value: &impl Serialize) -> Result<(), String> {
        // Bound recursion/work before recursive JSON serialization or any clone.
        self.shape.inspect(value)?;
        serde_json::to_writer(&mut *self, value)
            .map_err(|_| "Provider observation metadata budget exceeded".to_owned())
    }
}
impl std::io::Write for ObservationCopyBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(std::io::Error::other(
                "Provider observation metadata budget exceeded",
            ));
        }
        self.remaining -= bytes.len();
        self.used += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> ConversionRegistry<'a> {
    pub fn observation_copy_bytes(
        &self,
        max: usize,
        nodes: usize,
        depth: usize,
    ) -> Result<usize, String> {
        let result = self.observation_copy_bytes_unreported(max, nodes, depth);
        self.observation.complete_result(result)
    }
    fn observation_copy_bytes_unreported(
        &self,
        max: usize,
        nodes: usize,
        depth: usize,
    ) -> Result<usize, String> {
        let mut budget = ObservationCopyBudget {
            shape: crate::definitions::metadata_limits::TraversalBudget::new(
                nodes,
                depth,
                usize::MAX,
            ),
            remaining: max,
            used: 0,
        };
        self.measure_observation_copy(&mut budget)?;
        Ok(budget.used)
    }
    fn measure_observation_copy(&self, budget: &mut ObservationCopyBudget) -> Result<(), String> {
        self.constructors.measure_observation_copy(budget)?;
        for (key, contract) in &self.constructor_dependencies {
            budget.add(key)?;
            budget.add(contract)?;
        }
        for key in &self.compilation_dependencies {
            budget.add(key)?;
        }
        for (key, registration) in &self.providers {
            budget.add(key)?;
            budget.add(&registration.contract)?;
        }
        self.applied.measure_observation_copy(budget)
    }

    pub fn with_observation(mut self, context: InvocationContext<'a>) -> Self {
        self.observation = context.clone();
        self.applied.set_observation(context.clone());
        self.constructors.set_observation(context);
        self
    }
    pub fn constructors(&self) -> &ConstructorRegistry<'a> {
        &self.constructors
    }
    pub fn register_constructor(
        &mut self,
        bundle: &crate::definitions::bundle::DefinitionBundle,
        provider: &'a dyn ConstructorProvider,
    ) -> Result<(), String> {
        let result = self.register_constructor_unreported(bundle, provider);
        self.observation.complete_result(result)
    }
    fn register_constructor_unreported(
        &mut self,
        bundle: &crate::definitions::bundle::DefinitionBundle,
        provider: &'a dyn ConstructorProvider,
    ) -> Result<(), String> {
        self.constructors.register(bundle, provider)
    }
    pub fn register_constructor_shared(
        &mut self,
        bundle: &crate::definitions::bundle::DefinitionBundle,
        provider: std::rc::Rc<dyn ConstructorProvider + 'a>,
    ) -> Result<(), String> {
        let result = self.register_constructor_shared_unreported(bundle, provider);
        self.observation.complete_result(result)
    }
    fn register_constructor_shared_unreported(
        &mut self,
        bundle: &crate::definitions::bundle::DefinitionBundle,
        provider: std::rc::Rc<dyn ConstructorProvider + 'a>,
    ) -> Result<(), String> {
        self.constructors.register_shared(bundle, provider)
    }
    pub fn retain_constructor_dependency(
        &mut self,
        contract: &ConstructorContract,
    ) -> Result<(), String> {
        let result = self.retain_constructor_dependency_unreported(contract);
        self.observation.complete_result(result)
    }
    fn retain_constructor_dependency_unreported(
        &mut self,
        contract: &ConstructorContract,
    ) -> Result<(), String> {
        if !self.constructors.registered_contracts()?.contains(contract) {
            return Err(
                "Consumed constructor policy is not registered with this exact contract".into(),
            );
        }
        self.constructor_dependencies
            .insert(contract.datatype.declaration().clone(), contract.clone());
        Ok(())
    }
    pub(crate) fn constructor_dependencies(
        &self,
    ) -> Result<impl Iterator<Item = &ConstructorContract> + '_, String> {
        // Validate all registrations first, including unconsumed policies, as
        // the former owning snapshot did. Keep the dependency selection borrowed.
        for registered in self.constructors.registered_contract_refs() {
            registered?;
        }
        for contract in self.constructor_dependencies.values() {
            if !self.constructors.contains_contract(contract)? {
                return Err(
                    "Consumed constructor policy no longer matches its registration".into(),
                );
            }
        }
        Ok(self.constructor_dependencies.values())
    }

    /// Retain a selected provider consumed by source checks during catalog construction.
    /// The caller owns those checks; this does not install a provider or a relation.
    pub fn retain_compilation_dependency(
        &mut self,
        declaration: &DeclarationId,
    ) -> Result<(), String> {
        let result = self.retain_compilation_dependency_unreported(declaration);
        self.observation.complete_result(result)
    }
    fn retain_compilation_dependency_unreported(
        &mut self,
        declaration: &DeclarationId,
    ) -> Result<(), String> {
        self.provider(declaration)?;
        self.compilation_dependencies.insert(declaration.clone());
        Ok(())
    }
    pub(crate) fn compilation_dependencies(
        &self,
    ) -> Result<impl Iterator<Item = Result<&ConversionContract, String>> + '_, String> {
        // Preserve complete registration validation before the consumer retains
        // any selection, then recheck each borrowed provider snapshot on lookup.
        for id in &self.compilation_dependencies {
            self.provider(id)?;
        }
        Ok(self
            .compilation_dependencies
            .iter()
            .map(|id| self.provider(id).map(|provider| provider.contract())))
    }
    pub fn applied(&self) -> &AppliedConversionRegistry<'a> {
        &self.applied
    }
    pub fn register_application(
        &mut self,
        datatype: &Datatype,
        checked: &crate::definitions::generic_resolution::CheckedDatatypeApplication,
        provider: &'a dyn AppliedConversionProvider,
    ) -> Result<(), String> {
        let result = self.register_application_unreported(datatype, checked, provider);
        self.observation.complete_result(result)
    }
    fn register_application_unreported(
        &mut self,
        datatype: &Datatype,
        checked: &crate::definitions::generic_resolution::CheckedDatatypeApplication,
        provider: &'a dyn AppliedConversionProvider,
    ) -> Result<(), String> {
        self.applied.register(datatype, checked, provider)
    }
    pub fn register_application_shared(
        &mut self,
        datatype: &Datatype,
        checked: &crate::definitions::generic_resolution::CheckedDatatypeApplication,
        provider: std::rc::Rc<dyn AppliedConversionProvider + 'a>,
    ) -> Result<(), String> {
        let result = self.register_application_shared_unreported(datatype, checked, provider);
        self.observation.complete_result(result)
    }
    fn register_application_shared_unreported(
        &mut self,
        datatype: &Datatype,
        checked: &crate::definitions::generic_resolution::CheckedDatatypeApplication,
        provider: std::rc::Rc<dyn AppliedConversionProvider + 'a>,
    ) -> Result<(), String> {
        self.applied.register_shared(datatype, checked, provider)
    }
    pub fn require_datatype(&self, datatype: &Datatype) -> Result<(), String> {
        let result = self.require_datatype_unreported(datatype);
        self.observation.complete_result(result)
    }
    fn require_datatype_unreported(&self, datatype: &Datatype) -> Result<(), String> {
        if let Some(application) = datatype.application() {
            let contract = self.applied.require(application)?;
            if contract.conversion.datatype != *datatype.declaration()
                || contract.conversion.serializable != datatype.serializable()
            {
                return Err("Applied conversion differs from destination datatype".into());
            }
            Ok(())
        } else {
            self.require_unreported(datatype).map(|_| ())
        }
    }
    pub fn parse_datatype(&self, datatype: &Datatype, text: &str) -> Result<Value, String> {
        let result = self.parse_datatype_unreported(datatype, text);
        self.observation.complete_result(result)
    }
    /// Format a JSON value against its destination declaration/application,
    /// using the same checked provider round trip as the typed formatting API.
    pub fn format_datatype(&self, datatype: &Datatype, value: &Value) -> Result<String, String> {
        let result = (|| {
            self.require_datatype_unreported(datatype)?;
            if let Some(application) = datatype.application() {
                let value: AppliedConvertedValue =
                    serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
                self.applied.validate_for(application, &value)?;
                self.applied.format(&value)
            } else {
                let value: ConvertedValue =
                    serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
                self.validate_for_unreported(datatype, &value)?;
                self.format_unreported(&value)
            }
        })();
        self.observation.complete_result(result)
    }
    fn parse_datatype_unreported(&self, datatype: &Datatype, text: &str) -> Result<Value, String> {
        self.require_datatype_unreported(datatype)?;
        match datatype.application() {
            Some(application) => serde_json::to_value(self.applied.parse(application, text)?),
            None => serde_json::to_value(self.parse_unreported(datatype.declaration(), text)?),
        }
        .map_err(|e| e.to_string())
    }
    pub fn datatype_default(&self, datatype: &Datatype) -> Result<Value, String> {
        let result = self.datatype_default_unreported(datatype);
        self.observation.complete_result(result)
    }
    fn datatype_default_unreported(&self, datatype: &Datatype) -> Result<Value, String> {
        self.require_datatype_unreported(datatype)?;
        match datatype.application() {
            Some(application) => serde_json::to_value(self.applied.default_value(application)?),
            None => serde_json::to_value(self.default_value_unreported(datatype.declaration())?),
        }
        .map_err(|e| e.to_string())
    }
    pub fn register(
        &mut self,
        datatype: &Datatype,
        provider: &'a dyn ConversionProvider,
    ) -> Result<(), String> {
        let result = self.register_unreported(datatype, provider);
        self.observation.complete_result(result)
    }
    fn register_unreported(
        &mut self,
        datatype: &Datatype,
        provider: &'a dyn ConversionProvider,
    ) -> Result<(), String> {
        if !datatype.parameters().is_empty() {
            return Err("Generic datatype requires a specialized application provider".into());
        }
        self.register_specialized(
            datatype,
            ConversionImplementation::Direct(ProviderRef::Borrowed(provider)),
        )
    }
    /// Retain a native implementation for the registry's lifetime.
    pub fn register_shared(
        &mut self,
        datatype: &Datatype,
        provider: std::rc::Rc<dyn ConversionProvider + 'a>,
    ) -> Result<(), String> {
        let result = self.register_shared_unreported(datatype, provider);
        self.observation.complete_result(result)
    }
    fn register_shared_unreported(
        &mut self,
        datatype: &Datatype,
        provider: std::rc::Rc<dyn ConversionProvider + 'a>,
    ) -> Result<(), String> {
        if !datatype.parameters().is_empty() {
            return Err("Generic datatype requires a specialized application provider".into());
        }
        self.register_specialized(
            datatype,
            ConversionImplementation::Direct(ProviderRef::Shared(provider)),
        )
    }
    // Only the application registry may bypass the uninstantiated declaration guard.
    fn register_specialized(
        &mut self,
        datatype: &Datatype,
        provider: ConversionImplementation<'a>,
    ) -> Result<(), String> {
        let contract = provider.get().contract();
        if contract.datatype != *datatype.declaration()
            || contract.serializable != datatype.serializable()
        {
            return Err(
                "Provider contract does not match datatype identity or serializability".into(),
            );
        }
        if contract.provider.name.trim().is_empty() || contract.provider.version.trim().is_empty() {
            return Err("Provider requires a nonempty name and version".into());
        }
        if contract
            .declarations
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err("Conversion declaration scope must be sorted and unique".into());
        }
        if self.providers.contains_key(&contract.datatype) {
            return Err(
                "A conversion provider is already registered for this exact datatype".into(),
            );
        }
        self.providers.insert(
            contract.datatype.clone(),
            Registration {
                contract: contract.clone(),
                implementation: provider,
            },
        );
        Ok(())
    }
    fn provider(&self, datatype: &DeclarationId) -> Result<&dyn ConversionProvider, String> {
        self.provider_observed(datatype, &self.observation)
    }
    fn provider_observed(
        &self,
        datatype: &DeclarationId,
        observation: &InvocationContext<'_>,
    ) -> Result<&dyn ConversionProvider, String> {
        let registration = self.providers.get(datatype).ok_or_else(|| {
            observation.dependency_unavailable(
                crate::definitions::conversion::ProviderDependencyKind::Conversion,
            );
            "No conversion provider for this exact datatype revision"
        })?;
        if registration.implementation.get().contract() != &registration.contract {
            return Err("Conversion provider changed its registered contract".into());
        }
        Ok(registration.implementation.get())
    }

    /// Check one exact registered policy without cloning the registry manifest.
    /// This validates its provider snapshot but does not invoke semantic work.
    pub fn contains_contract(&self, contract: &ConversionContract) -> Result<bool, String> {
        let result = self.contains_contract_unreported(contract);
        self.observation.complete_result(result)
    }
    fn contains_contract_unreported(&self, contract: &ConversionContract) -> Result<bool, String> {
        if !self.providers.contains_key(&contract.datatype) {
            return Ok(false);
        }
        Ok(self.provider(&contract.datatype)?.contract() == contract)
    }

    /// Return exact registered contracts after checking provider snapshots.
    pub fn registered_contracts(&self) -> Result<Vec<ConversionContract>, String> {
        let result = self.registered_contracts_unreported();
        self.observation.complete_result(result)
    }
    fn registered_contracts_unreported(&self) -> Result<Vec<ConversionContract>, String> {
        self.registered_contract_refs()
            .map(|value| value.cloned())
            .collect()
    }
    pub(crate) fn registered_contract_refs(
        &self,
    ) -> impl Iterator<Item = Result<&ConversionContract, String>> + '_ {
        self.providers
            .keys()
            .map(|id| self.provider(id).map(|provider| provider.contract()))
    }

    /// Query both registered implementations and reject conflicting assertions.
    /// Unknown is distinct from unrelated. This never retags a converted value.
    pub fn classifier_relation(
        &self,
        source: &DeclarationId,
        target: &DeclarationId,
    ) -> Result<Option<ClassifierRelation>, String> {
        let result = self.classifier_relation_unreported(source, target);
        self.observation.complete_result(result)
    }
    fn classifier_relation_unreported(
        &self,
        source: &DeclarationId,
        target: &DeclarationId,
    ) -> Result<Option<ClassifierRelation>, String> {
        let source_provider = self.provider(source)?;
        let target_provider = self.provider(target)?;
        if source == target {
            return Ok(Some(ClassifierRelation::Equivalent));
        }
        self.observation
            .invoke(&source_provider.contract().provider)?;
        let forward = source_provider.classifier_relation(target_provider.contract());
        self.provider(source)?;
        self.provider(target)?;
        let forward = forward?;
        self.observation
            .invoke(&target_provider.contract().provider)?;
        let reverse = target_provider.classifier_relation(source_provider.contract());
        self.provider(source)?;
        self.provider(target)?;
        let reverse = reverse?;
        match (forward, reverse) {
            (Some(forward), Some(reverse)) if forward.reverse() == reverse => Ok(Some(forward)),
            (Some(_), Some(_)) => {
                Err("Conversion providers disagree on classifier relation".into())
            }
            _ => Ok(None),
        }
    }

    /// Require the destination contract even when its current value is absent.
    pub fn require(&self, datatype: &Datatype) -> Result<&ConversionContract, String> {
        let result = self.require_unreported(datatype);
        self.observation.complete_result(result)
    }
    fn require_unreported(&self, datatype: &Datatype) -> Result<&ConversionContract, String> {
        let contract = self.provider(datatype.declaration())?.contract();
        if contract.serializable != datatype.serializable() {
            return Err("Provider serializability differs from destination datatype".into());
        }
        Ok(contract)
    }
    fn wrap(
        &self,
        provider: &dyn ConversionProvider,
        value: Value,
    ) -> Result<ConvertedValue, String> {
        if value.is_null() {
            return Err("Null is absence, not a present converted value".into());
        }
        self.observation.invoke(&provider.contract().provider)?;
        validation_preparation::validate_provider_observed(provider, &value, &self.observation)?;
        let contract = provider.contract();
        Ok(ConvertedValue::Converted {
            datatype: contract.datatype.clone(),
            provider: contract.provider.clone(),
            value,
        })
    }
    pub fn bind(&self, datatype: &DeclarationId, value: Value) -> Result<ConvertedValue, String> {
        let result = self.bind_unreported(datatype, value);
        self.observation.complete_result(result)
    }
    fn bind_unreported(
        &self,
        datatype: &DeclarationId,
        value: Value,
    ) -> Result<ConvertedValue, String> {
        let result = self.wrap(self.provider(datatype)?, value)?;
        self.provider(datatype)?;
        Ok(result)
    }
    /// Validate a value against its destination datatype, not only its own tag.
    pub fn validate_for<'v>(
        &self,
        datatype: &Datatype,
        value: &'v ConvertedValue,
    ) -> Result<&'v Value, String> {
        let result = self.validate_for_unreported(datatype, value);
        self.observation.complete_result(result)
    }
    fn validate_for_unreported<'v>(
        &self,
        datatype: &Datatype,
        value: &'v ConvertedValue,
    ) -> Result<&'v Value, String> {
        self.validate_for_observed_unreported(datatype, value, &self.observation)
    }
    /// Validate with an operation-scoped observer, without copying the registry.
    pub fn validate_for_observed<'v>(
        &self,
        datatype: &Datatype,
        value: &'v ConvertedValue,
        observation: &InvocationContext<'_>,
    ) -> Result<&'v Value, String> {
        let result = self.validate_for_observed_unreported(datatype, value, observation);
        observation.complete_result(result)
    }
    fn validate_for_observed_unreported<'v>(
        &self,
        datatype: &Datatype,
        value: &'v ConvertedValue,
        observation: &InvocationContext<'_>,
    ) -> Result<&'v Value, String> {
        self.validate_payload_for_observed_unreported(datatype, value, observation)
    }
    /// Validate either canonical payload representation against its actual
    /// destination, preserving the owned facade's contract/error ordering.
    pub fn validate_payload_for_observed<'v, Payload: ConversionPayload>(
        &self,
        datatype: &Datatype,
        value: &'v ConvertedValue<Payload>,
        observation: &InvocationContext<'_>,
    ) -> Result<&'v Value, String> {
        let result = self.validate_payload_for_observed_unreported(datatype, value, observation);
        observation.complete_result(result)
    }
    fn validate_payload_for_observed_unreported<'v, Payload: ConversionPayload>(
        &self,
        datatype: &Datatype,
        value: &'v ConvertedValue<Payload>,
        observation: &InvocationContext<'_>,
    ) -> Result<&'v Value, String> {
        let ConvertedValue::Converted {
            datatype: actual, ..
        } = value;
        self.check_payload_destination(datatype, actual, observation)?;
        self.validate_payload_observed_unreported(value, observation)
    }
    fn check_payload_destination(
        &self,
        datatype: &Datatype,
        actual: &DeclarationId,
        observation: &InvocationContext<'_>,
    ) -> Result<(), String> {
        if actual != datatype.declaration() {
            return Err("Converted value does not belong to the destination datatype".into());
        }
        let provider = self.provider_observed(actual, observation)?;
        if provider.contract().serializable != datatype.serializable() {
            return Err("Provider serializability differs from destination datatype".into());
        }
        Ok(())
    }
    /// Admit the same destination before retaining official owned metadata.
    /// The caller must fund the complete inline job and any metadata buffers.
    pub fn prepare_owned_header_validation_for_observed<'v>(
        &'v self,
        datatype: &Datatype,
        value: ConvertedValue<&'v Value>,
        observation: &InvocationContext<'_>,
    ) -> Result<OwnedConversionValidationPreparation<'v, 'a>, String> {
        let result = self.prepare_owned_header_validation_for_observed_unreported(
            datatype,
            value,
            observation,
        );
        observation.complete_result(result)
    }
    fn prepare_owned_header_validation_for_observed_unreported<'v>(
        &'v self,
        datatype: &Datatype,
        value: ConvertedValue<&'v Value>,
        observation: &InvocationContext<'_>,
    ) -> Result<OwnedConversionValidationPreparation<'v, 'a>, String> {
        let ConvertedValue::Converted {
            datatype: actual, ..
        } = &value;
        self.check_payload_destination(datatype, actual, observation)?;
        Ok(self.prepare_owned_header_validation(value))
    }
    pub fn validate<'v>(&self, value: &'v ConvertedValue) -> Result<&'v Value, String> {
        let result = self.validate_unreported(value);
        self.observation.complete_result(result)
    }
    fn validate_unreported<'v>(&self, value: &'v ConvertedValue) -> Result<&'v Value, String> {
        self.validate_observed_unreported(value, &self.observation)
    }
    /// Prepare validation for one exact immutable converted value. No semantic
    /// method or adapter factory runs until the first resume.
    pub fn prepare_validation<'v, Payload: ConversionPayload>(
        &'v self,
        value: &'v ConvertedValue<Payload>,
    ) -> ConversionValidationPreparation<'v, 'a, Payload> {
        ConversionValidationPreparation::new(self, value)
    }
    /// Move admitted official parsed metadata into a job whose provider cursor
    /// borrows only the exact external payload. No provider/factory runs here.
    pub fn prepare_owned_header_validation<'v>(
        &'v self,
        value: ConvertedValue<&'v Value>,
    ) -> OwnedConversionValidationPreparation<'v, 'a> {
        ConversionValidationJob::new(self, value)
    }
    pub fn validate_observed<'v>(
        &self,
        value: &'v ConvertedValue,
        observation: &InvocationContext<'_>,
    ) -> Result<&'v Value, String> {
        let result = self.validate_observed_unreported(value, observation);
        observation.complete_result(result)
    }
    fn validate_observed_unreported<'v>(
        &self,
        value: &'v ConvertedValue,
        observation: &InvocationContext<'_>,
    ) -> Result<&'v Value, String> {
        self.validate_payload_observed_unreported(value, observation)
    }
    /// Drain the same validation job for an owned or borrowed payload. This
    /// explicit facade leaves existing owned-wire type inference unchanged.
    pub fn validate_payload_observed<'v, Payload: ConversionPayload>(
        &self,
        value: &'v ConvertedValue<Payload>,
        observation: &InvocationContext<'_>,
    ) -> Result<&'v Value, String> {
        let result = self.validate_payload_observed_unreported(value, observation);
        observation.complete_result(result)
    }
    fn validate_payload_observed_unreported<'v, Payload: ConversionPayload>(
        &self,
        value: &'v ConvertedValue<Payload>,
        observation: &InvocationContext<'_>,
    ) -> Result<&'v Value, String> {
        let mut preparation = self.prepare_validation(value);
        loop {
            match preparation.resume_unreported(std::num::NonZeroUsize::MIN, observation) {
                ConversionValidationProgress::Pending => {}
                ConversionValidationProgress::Complete(result) => {
                    result?;
                    let ConvertedValue::Converted { value, .. } = value;
                    return Ok(value.borrow());
                }
                ConversionValidationProgress::Finished => {
                    return Err("Conversion validation finished without an outcome".into());
                }
            }
        }
    }
    fn validation_implementation(
        &self,
        datatype: &DeclarationId,
        provider: &ProviderId,
        value: &Value,
        observation: &InvocationContext<'_>,
    ) -> Result<&dyn ConversionProvider, String> {
        let implementation = self.provider_observed(datatype, observation)?;
        if implementation.contract().provider != *provider {
            return Err("Converted value belongs to another provider or version".into());
        }
        if value.is_null() {
            return Err("Null is absence, not a present converted value".into());
        }
        Ok(implementation)
    }
    pub fn default_value(
        &self,
        datatype: &DeclarationId,
    ) -> Result<Option<ConvertedValue>, String> {
        let result = self.default_value_unreported(datatype);
        self.observation.complete_result(result)
    }
    fn default_value_unreported(
        &self,
        datatype: &DeclarationId,
    ) -> Result<Option<ConvertedValue>, String> {
        let provider = self.provider(datatype)?;
        self.observation.invoke(&provider.contract().provider)?;
        let result = provider
            .default_value_observed(&self.observation)?
            .map(|value| self.wrap(provider, value))
            .transpose()?;
        self.provider(datatype)?;
        Ok(result)
    }

    pub fn parse(&self, datatype: &DeclarationId, text: &str) -> Result<ConvertedValue, String> {
        let result = self.parse_unreported(datatype, text);
        self.observation.complete_result(result)
    }
    fn parse_unreported(
        &self,
        datatype: &DeclarationId,
        text: &str,
    ) -> Result<ConvertedValue, String> {
        let provider = self.provider(datatype)?;
        if !provider.contract().serializable {
            return Err("Datatype is not serializable".into());
        }
        self.observation.invoke(&provider.contract().provider)?;
        let result = self.wrap(provider, provider.parse_observed(text, &self.observation)?)?;
        self.provider(datatype)?;
        Ok(result)
    }
    pub fn format(&self, value: &ConvertedValue) -> Result<String, String> {
        let result = self.format_unreported(value);
        self.observation.complete_result(result)
    }
    fn format_unreported(&self, value: &ConvertedValue) -> Result<String, String> {
        let ConvertedValue::Converted { datatype, .. } = value;
        let provider = self.provider(datatype)?;
        if !provider.contract().serializable {
            return Err("Datatype is not serializable".into());
        }
        let payload = self.validate_unreported(value)?;
        self.observation.invoke(&provider.contract().provider)?;
        let text = provider.format_observed(payload, &self.observation)?;
        self.observation.invoke(&provider.contract().provider)?;
        let restored = provider.parse_observed(&text, &self.observation)?;
        self.observation.invoke(&provider.contract().provider)?;
        validation_preparation::validate_provider_observed(provider, &restored, &self.observation)?;
        if &restored != payload {
            return Err("Conversion provider formatting loses value information".into());
        }
        self.provider(datatype)?;
        Ok(text)
    }
}

/// Read the official converted wire while borrowing its exact immutable JSON
/// payload. The derived schema owns metadata parsing; canonical comparison uses
/// the same work hook as the owned facade. No provider validation or authority
/// follows from parsing. Header Serde preparation and canonical comparison are
/// synchronous and retain their explicit atomic boundaries.
pub fn borrowed_value_with_work<'a>(
    payload: &'a Value,
    work: &dyn Fn(usize) -> Result<(), String>,
) -> Result<ConvertedValue<&'a Value>, String> {
    let parsed = ConvertedValue::<serde::de::IgnoredAny>::deserialize(payload)
        .map_err(|error| error.to_string())?;
    let ConvertedValue::Converted {
        datatype, provider, ..
    } = parsed;
    let value = payload
        .get("value")
        .ok_or("Checked converted payload unavailable")?;
    let view = ConvertedValue::Converted {
        datatype,
        provider,
        value,
    };
    if !crate::definitions::json_exact::matches_with_work(&view, payload, work)? {
        return Err("Nominal value requires exact canonical fields".into());
    }
    Ok(view)
}
