//! Language-owned sample converter proving the shared custom conversion path.
use intersect_model::definitions::conversion::{ConversionContract, ConversionProvider};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[path = "collection_constructor.rs"]
mod constructors;
use constructors::{CollectionConstructor, CollectionDomain};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rgb {
    red: u8,
    green: u8,
    blue: u8,
}

/// Exact RGB payload and #RRGGBB lexical conversion, with no host resources.
pub struct RgbConverter {
    contract: ConversionContract,
}
impl RgbConverter {
    pub fn new(contract: ConversionContract) -> Self {
        Self { contract }
    }
}
impl ConversionProvider for RgbConverter {
    fn contract(&self) -> &ConversionContract {
        &self.contract
    }
    fn classifier_relation(
        &self,
        target: &ConversionContract,
    ) -> Result<Option<intersect_model::definitions::conversion::ClassifierRelation>, String> {
        let supported = |contract: &ConversionContract| {
            contract.provider.name == "example.rgb"
                && contract.provider.version == "1"
                && contract.serializable
                && contract.declarations.is_empty()
                && contract.members.is_empty()
        };
        Ok(if supported(&self.contract) && supported(target) {
            Some(intersect_model::definitions::conversion::ClassifierRelation::Equivalent)
        } else {
            None
        })
    }
    fn parse(&self, text: &str) -> Result<Value, String> {
        let hex = text.strip_prefix('#').ok_or("RGB text requires #RRGGBB")?;
        if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("RGB text requires six ASCII hexadecimal digits".into());
        }
        let bits = u32::from_str_radix(hex, 16).map_err(|error| error.to_string())?;
        let bytes = bits.to_be_bytes();
        serde_json::to_value(Rgb {
            red: bytes[1],
            green: bytes[2],
            blue: bytes[3],
        })
        .map_err(|error| error.to_string())
    }
    fn format(&self, value: &Value) -> Result<String, String> {
        let rgb: Rgb = serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
        Ok(format!("#{:02X}{:02X}{:02X}", rgb.red, rgb.green, rgb.blue))
    }
    fn validate(&self, value: &Value) -> Result<(), String> {
        serde_json::from_value::<Rgb>(value.clone())
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
    fn default_value(&self) -> Result<Option<Value>, String> {
        Ok(None)
    }
}

/// Build only installed native implementations, selected by exact retained source identities.
/// The callback borrows a checked registry; no submitted contract is executable code.
pub fn with_catalog<T>(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    contracts: &[ConversionContract],
    callback: impl FnOnce(
        &intersect_model::definitions::conversion::ConversionRegistry<'_>,
    ) -> Result<T, String>,
) -> Result<T, String> {
    with_catalog_context(bundle, contracts, &[], callback)
}

pub fn with_catalog_context<T>(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    contracts: &[ConversionContract],
    applications: &[intersect_model::definitions::conversion::AppliedConversionContract],
    callback: impl FnOnce(
        &intersect_model::definitions::conversion::ConversionRegistry<'_>,
    ) -> Result<T, String>,
) -> Result<T, String> {
    with_catalog_policies(bundle, contracts, applications, &[], callback)
}

/// Explicit installed constructor selections are built before concrete applications.
pub fn with_catalog_policies<T>(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    contracts: &[ConversionContract],
    applications: &[intersect_model::definitions::conversion::AppliedConversionContract],
    constructors: &[intersect_model::definitions::conversion::ConstructorContract],
    callback: impl FnOnce(
        &intersect_model::definitions::conversion::ConversionRegistry<'_>,
    ) -> Result<T, String>,
) -> Result<T, String> {
    let registry = build_catalog_policies(bundle, contracts, applications, constructors)?;
    callback(&registry)
}

/// Own checked installed native implementations without a self-referential registry.
/// Building and registration happen once; later borrows do not rebuild providers.
pub fn build_catalog_policies(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    contracts: &[ConversionContract],
    applications: &[intersect_model::definitions::conversion::AppliedConversionContract],
    constructors: &[intersect_model::definitions::conversion::ConstructorContract],
) -> Result<intersect_model::definitions::conversion::ConversionRegistry<'static>, String> {
    let mut remaining = CatalogBudget::default();
    build_catalog_scope(
        bundle,
        CatalogSelections {
            contracts,
            applications,
            constructors,
        },
        &mut remaining,
        0,
    )
}

struct CatalogSelections<'a> {
    contracts: &'a [ConversionContract],
    applications: &'a [intersect_model::definitions::conversion::AppliedConversionContract],
    constructors: &'a [intersect_model::definitions::conversion::ConstructorContract],
}
fn with_catalog_scope<T>(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    selections: CatalogSelections<'_>,
    remaining: &mut CatalogBudget,
    depth: usize,
    callback: impl FnOnce(
        &intersect_model::definitions::conversion::ConversionRegistry<'_>,
    ) -> Result<T, String>,
) -> Result<T, String> {
    let registry = build_catalog_scope(bundle, selections, remaining, depth)?;
    callback(&registry)
}
fn build_catalog_scope(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    selections: CatalogSelections<'_>,
    remaining: &mut CatalogBudget,
    depth: usize,
) -> Result<intersect_model::definitions::conversion::ConversionRegistry<'static>, String> {
    let CatalogSelections {
        contracts,
        applications,
        constructors,
    } = selections;
    if contracts
        .len()
        .saturating_add(applications.len())
        .saturating_add(constructors.len())
        > 128
    {
        return Err("Conversion provider selection budget exceeded".into());
    }
    for contract in constructors {
        remaining.nodes = remaining
            .nodes
            .checked_sub(1)
            .ok_or("Constructor provider budget exceeded")?;
        // Count into a bounded sink before cloning/building; do not allocate JSON.
        let bytes = intersect_model::definitions::metadata_limits::serialized_size(
            contract,
            remaining.context_bytes,
        )
        .map_err(|_| "Constructor context byte budget exceeded (16 MiB)")?;
        remaining.context_bytes -= bytes;
        remaining
            .constructors
            .push(std::rc::Rc::new(CollectionConstructor::new(
                contract.clone(),
            )?));
    }
    let implementations = build_catalog(bundle, contracts, remaining, depth)?;
    let mut registry = intersect_model::definitions::conversion::ConversionRegistry::default();
    for (datatype, implementation) in implementations {
        registry.register_shared(&datatype, std::rc::Rc::from(implementation))?;
    }
    let specialized = applications
        .iter()
        .map(|contract| build_application_provider(bundle, contract, remaining, depth))
        .collect::<Result<Vec<_>, _>>()?;
    for provider in specialized {
        let provider = std::rc::Rc::new(provider);
        registry.register_application_shared(
            &provider.datatype,
            &provider.checked,
            provider.clone(),
        )?;
    }
    for constructor in &remaining.constructors {
        registry.register_constructor_shared(bundle, constructor.clone())?;
    }
    for contract in remaining.constructor_dependencies.values() {
        registry.retain_constructor_dependency(contract)?;
    }
    for dependency in &remaining.dependencies {
        registry.retain_compilation_dependency(dependency)?;
    }
    Ok(registry)
}

struct CatalogBudget {
    // Keys retain resolution metadata, not another copy of source text. Each hit
    // revalidates submitted source digests before trusting these identities.
    resources: Vec<(
        intersect_model::definitions::bundle::DefinitionBundle,
        std::sync::Arc<intersect_model::definitions::resources::ResourceSet>,
    )>,
    constructors: Vec<std::rc::Rc<CollectionConstructor>>,
    constructor_dependencies: std::collections::BTreeMap<
        intersect_model::definitions::resources::DeclarationId,
        intersect_model::definitions::conversion::ConstructorContract,
    >,
    nodes: usize,
    context_bytes: usize,
    resolver: Option<intersect_model::definitions::generic_resolution::Resolver>,
    dependencies:
        std::collections::BTreeSet<intersect_model::definitions::resources::DeclarationId>,
}
impl Default for CatalogBudget {
    fn default() -> Self {
        Self {
            resources: vec![],
            constructors: vec![],
            constructor_dependencies: Default::default(),
            nodes: 128,
            context_bytes: 16 * 1024 * 1024,
            resolver: None,
            dependencies: Default::default(),
        }
    }
}

impl CatalogBudget {
    fn resources(
        &mut self,
        bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    ) -> Result<std::sync::Arc<intersect_model::definitions::resources::ResourceSet>, String> {
        use intersect_model::definitions::{
            bundle::{DefinitionBundle, DefinitionSource},
            resources::ResourceSet,
        };
        bundle.validate()?;
        let key = DefinitionBundle {
            version: bundle.version,
            sources: bundle
                .sources
                .iter()
                .map(|source| DefinitionSource {
                    id: source.id.clone(),
                    kind: source.kind,
                    text: String::new(),
                    dependencies: source.dependencies.clone(),
                })
                .collect(),
            namespaces: bundle.namespaces.clone(),
        };
        if let Some((_, resources)) = self.resources.iter().find(|(existing, _)| existing == &key) {
            return Ok(resources.clone());
        }
        let resources = std::sync::Arc::new(ResourceSet::new(bundle)?);
        self.resources.push((key, resources.clone()));
        Ok(resources)
    }
}

// Installed relation-bearing providers have no member dependencies. Build them
// first so nested applications see the same checked relation snapshot regardless
// of selection order. Collection payloads still use their existing implementations.
type BuiltProvider = (
    intersect_model::definitions::datatypes::Datatype,
    Box<dyn ConversionProvider>,
);
fn build_catalog(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    contracts: &[ConversionContract],
    remaining: &mut CatalogBudget,
    depth: usize,
) -> Result<Vec<BuiltProvider>, String> {
    use intersect_model::definitions::{
        conversion::ConversionRegistry, generic_resolution::Resolver,
    };
    remaining.resolver = None;
    let mut implementations = Vec::new();
    for contract in contracts.iter().filter(|c| c.members.is_empty()) {
        implementations.push(build_provider(bundle, contract, remaining, depth)?);
    }
    {
        let mut registry = ConversionRegistry::default();
        for constructor in &remaining.constructors {
            registry.register_constructor_shared(bundle, constructor.clone())?;
        }
        for (datatype, provider) in &implementations {
            registry.register(datatype, provider.as_ref())?;
        }
        remaining.resolver = Some(
            Resolver::new(bundle)
                .map_err(|e| e.to_string())?
                .with_conversions(&registry)
                .map_err(|e| e.to_string())?,
        );
    }
    for contract in contracts.iter().filter(|c| !c.members.is_empty()) {
        implementations.push(build_provider(bundle, contract, remaining, depth)?);
    }
    let mut registry = ConversionRegistry::default();
    for constructor in &remaining.constructors {
        registry.register_constructor_shared(bundle, constructor.clone())?;
    }
    for (datatype, provider) in &implementations {
        registry.register(datatype, provider.as_ref())?;
    }
    remaining.resolver = Some(
        Resolver::new(bundle)
            .map_err(|e| e.to_string())?
            .with_conversions(&registry)
            .map_err(|e| e.to_string())?,
    );
    drop(registry);
    Ok(implementations)
}

fn build_provider(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    contract: &ConversionContract,
    remaining: &mut CatalogBudget,
    depth: usize,
) -> Result<
    (
        intersect_model::definitions::datatypes::Datatype,
        Box<dyn ConversionProvider>,
    ),
    String,
> {
    let datatype = provider_datatype(bundle, contract, remaining, depth)?;
    let implementation: Box<dyn ConversionProvider> = match contract.provider.name.as_str() {
        "intersect.sequence" => {
            return Err("Sequence requires an exact datatype application".into());
        }
        "ecore.feature-map" | "ecore.feature-entry" => Box::new(build_feature_map(
            bundle, contract, &datatype, remaining, depth,
        )?),
        "ecore.list" | "ecore.map" | "java.list" | "java.collection" => Box::new(build_collection(
            bundle, contract, &datatype, remaining, depth,
        )?),
        "ecore.definition-resource" | "ecore.definition-resource-set" => {
            use crate::resource_conversion::{ResourceConverter, ResourceKind};
            let (class, kind) = if contract.provider.name == "ecore.definition-resource" {
                (
                    "org.eclipse.emf.ecore.resource.Resource",
                    ResourceKind::Resource,
                )
            } else {
                (
                    "org.eclipse.emf.ecore.resource.ResourceSet",
                    ResourceKind::Set,
                )
            };
            if datatype.instance_class_name() != Some(class)
                || datatype
                    .instance_type_name()
                    .is_some_and(|name| name != class)
            {
                return Err(
                    "Resource provider requires the corresponding resource datatype binding".into(),
                );
            }
            Box::new(ResourceConverter::new_with_resources(
                contract.clone(),
                kind,
                || remaining.resources(bundle),
            )?)
        }
        "ecore.definition-tree-iterator" => {
            let class = "org.eclipse.emf.common.util.TreeIterator";
            if datatype.instance_class_name() != Some(class)
                || datatype
                    .instance_type_name()
                    .is_some_and(|name| name != class)
            {
                return Err(
                    "Tree iterator provider requires a TreeIterator datatype binding".into(),
                );
            }
            Box::new(
                crate::tree_iterator_conversion::TreeIteratorConverter::new_with_resources(
                    contract.clone(),
                    || remaining.resources(bundle),
                )?,
            )
        }
        "ecore.diagnostic-chain" => {
            let class = "org.eclipse.emf.common.util.DiagnosticChain";
            if datatype.instance_class_name() != Some(class)
                || datatype
                    .instance_type_name()
                    .is_some_and(|name| name != class)
            {
                return Err(
                    "Diagnostic chain provider requires a DiagnosticChain datatype binding".into(),
                );
            }
            Box::new(
                crate::diagnostic_chain_conversion::DiagnosticChainConverter::new(
                    contract.clone(),
                )?,
            )
        }
        "ecore.invocation-error" => {
            let class = "java.lang.reflect.InvocationTargetException";
            if datatype.instance_class_name() != Some(class)
                || datatype
                    .instance_type_name()
                    .is_some_and(|name| name != class)
            {
                return Err("Invocation error provider requires an InvocationTargetException datatype binding".into());
            }
            Box::new(
                crate::invocation_error_conversion::InvocationErrorConverter::new(
                    contract.clone(),
                )?,
            )
        }
        "ecore.enumerator" => {
            if datatype.instance_class_name() != Some("org.eclipse.emf.common.util.Enumerator")
                || datatype
                    .instance_type_name()
                    .is_some_and(|name| name != "org.eclipse.emf.common.util.Enumerator")
            {
                return Err("Enumerator provider requires an Enumerator datatype binding".into());
            }
            let enumerations = contract
                .declarations
                .iter()
                .map(|id| {
                    intersect_model::definitions::compiler::import_enum_bundle(bundle, id)
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            Box::new(crate::enumerator_conversion::EnumeratorConverter::new(
                contract.clone(),
                enumerations,
            )?)
        }
        "ecore.class" => {
            if datatype.instance_class_name() != Some("java.lang.Class")
                || datatype
                    .instance_type_name()
                    .is_some_and(|name| name != "java.lang.Class")
            {
                return Err("Class provider requires a java.lang.Class datatype binding".into());
            }
            Box::new(crate::class_conversion::ClassConverter::portable_core(
                contract.clone(),
            )?)
        }
        _ => Box::new(RgbConverter::new(contract.clone())),
    };

    let mut registry = intersect_model::definitions::conversion::ConversionRegistry::default();
    registry.register(&datatype, implementation.as_ref())?;
    if implementation.contract() != contract {
        return Err("Installed provider changed selected contract".into());
    }
    drop(registry);
    Ok((datatype, implementation))
}

fn build_member(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    member: &intersect_model::definitions::conversion::MemberContract,
    remaining: &mut CatalogBudget,
    depth: usize,
) -> Result<crate::collection_conversion::Member, String> {
    use crate::collection_conversion::Member;
    use intersect_model::definitions::{compiler, conversion::MemberType};
    match &member.value {
        MemberType::Builtin { codec } => Ok(Member::builtin(member.nullable, *codec)),
        MemberType::Datatype { declaration } => Member::datatype(
            member.nullable,
            compiler::import_datatype_bundle(bundle, declaration)
                .map_err(|error| error.to_string())?,
        ),
        MemberType::Enumeration { declaration } => Ok(Member::enumeration(
            member.nullable,
            compiler::import_enum_bundle(bundle, declaration).map_err(|error| error.to_string())?,
        )),
        MemberType::Reference { class, context } => {
            with_reference_scope(context, remaining, depth, |registry| {
                let domain = crate::model_reference::ReferenceDomain::new(
                    class.clone(),
                    context.as_ref().clone(),
                    registry,
                )?;
                Ok(Member::reference(member.nullable, domain))
            })
        }
        MemberType::RuntimeReference { target, context } => {
            with_reference_scope(context, remaining, depth, |registry| {
                let domain = crate::model_reference::ReferenceDomain::new_runtime(
                    target.clone(),
                    context.as_ref().clone(),
                    registry,
                )?;
                Ok(Member::reference(member.nullable, domain))
            })
        }
        MemberType::Applied { contract } => {
            let provider = build_application_provider(bundle, contract, remaining, depth + 1)?;
            Member::applied(member.nullable, provider)
        }
        MemberType::Converted { contract } => {
            let (datatype, provider) = build_provider(bundle, contract, remaining, depth + 1)?;
            Member::converted(member.nullable, datatype, provider)
        }
    }
}

fn with_reference_scope<T>(
    context: &intersect_model::definitions::conversion::ReferenceContext,
    remaining: &mut CatalogBudget,
    depth: usize,
    callback: impl FnOnce(
        &intersect_model::definitions::conversion::ConversionRegistry<'_>,
    ) -> Result<T, String>,
) -> Result<T, String> {
    if depth + 1 >= 16 || remaining.nodes == 0 {
        return Err("Reference context node/depth budget exceeded".into());
    }
    remaining.nodes -= 1;
    // Count into a bounded sink before cloning/building; do not allocate JSON.
    let bytes = intersect_model::definitions::metadata_limits::serialized_size(
        context,
        remaining.context_bytes,
    )
    .map_err(|_| "Reference context byte budget exceeded (16 MiB)")?;
    remaining.context_bytes -= bytes;
    // Independent policy scope, shared remaining aggregate counters. The outer
    // resolver and consumed dependencies are never borrowed by an inner context.
    let mut inner = CatalogBudget {
        resources: std::mem::take(&mut remaining.resources),
        nodes: remaining.nodes,
        context_bytes: remaining.context_bytes,
        constructors: vec![],
        constructor_dependencies: Default::default(),
        resolver: None,
        dependencies: Default::default(),
    };
    let result = with_catalog_scope(
        &context.bundle,
        CatalogSelections {
            contracts: &context.conversions,
            applications: &context.applied_conversions,
            constructors: &context.constructors,
        },
        &mut inner,
        depth + 1,
        callback,
    );
    remaining.resources = inner.resources;
    remaining.nodes = inner.nodes;
    remaining.context_bytes = inner.context_bytes;
    result
}

fn provider_datatype(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    contract: &ConversionContract,
    remaining: &mut CatalogBudget,
    depth: usize,
) -> Result<intersect_model::definitions::datatypes::Datatype, String> {
    if depth >= 16 || remaining.nodes == 0 {
        return Err("Nested conversion provider budget exceeded (128 providers, depth 16)".into());
    }
    remaining.nodes -= 1;
    if !matches!(
        contract.provider.name.as_str(),
        "intersect.sequence"
            | "ecore.list"
            | "ecore.map"
            | "java.list"
            | "java.collection"
            | "ecore.feature-map"
            | "ecore.feature-entry"
    ) && !contract.members.is_empty()
    {
        return Err("Selected conversion provider does not consume member contracts".into());
    }
    if !matches!(
        contract.provider.name.as_str(),
        "intersect.sequence"
            | "example.rgb"
            | "ecore.feature-map"
            | "ecore.feature-entry"
            | "ecore.list"
            | "ecore.map"
            | "java.list"
            | "java.collection"
            | "ecore.class"
            | "ecore.enumerator"
            | "ecore.invocation-error"
            | "ecore.diagnostic-chain"
            | "ecore.definition-resource"
            | "ecore.definition-resource-set"
            | "ecore.definition-tree-iterator"
    ) || contract.provider.version != "1"
    {
        return Err("Unknown native conversion provider or version".into());
    }
    if contract.declarations.len() > 256 {
        return Err("Conversion declaration scope budget exceeded (256)".into());
    }
    if !matches!(
        contract.provider.name.as_str(),
        "ecore.enumerator"
            | "ecore.feature-map"
            | "ecore.feature-entry"
            | "ecore.definition-resource"
            | "ecore.definition-resource-set"
            | "ecore.definition-tree-iterator"
    ) && !contract.declarations.is_empty()
    {
        return Err("Selected conversion provider does not consume declaration scope".into());
    }
    let datatype = if contract.datatype.source
        == intersect_model::definitions::compiler::standard_datatype_source().id
    {
        let datatype =
            intersect_model::definitions::compiler::standard_datatype_by_id(&contract.datatype)?;
        if (datatype.name() == "EFeatureMap" && contract.provider.name != "ecore.feature-map")
            || (datatype.name() == "EFeatureMapEntry"
                && contract.provider.name != "ecore.feature-entry")
        {
            return Err(
                "Pinned feature-map datatype requires its corresponding installed adapter".into(),
            );
        }
        if (datatype.name() == "EEList" && contract.provider.name != "ecore.list")
            || (datatype.name() == "EMap" && contract.provider.name != "ecore.map")
        {
            return Err(
                "Pinned collection datatype requires its corresponding installed adapter".into(),
            );
        }
        if datatype.name() == "ETreeIterator"
            && contract.provider.name != "ecore.definition-tree-iterator"
        {
            return Err("Pinned ETreeIterator requires the installed tree iterator adapter".into());
        }
        if (datatype.name() == "EResource" && contract.provider.name != "ecore.definition-resource")
            || (datatype.name() == "EResourceSet"
                && contract.provider.name != "ecore.definition-resource-set")
        {
            return Err("Pinned resource datatype requires its installed resource adapter".into());
        }
        if datatype.name() == "EDiagnosticChain"
            && contract.provider.name != "ecore.diagnostic-chain"
        {
            return Err(
                "Pinned EDiagnosticChain requires the installed diagnostic chain provider".into(),
            );
        }
        if datatype.name() == "EInvocationTargetException"
            && contract.provider.name != "ecore.invocation-error"
        {
            return Err("Pinned EInvocationTargetException requires the installed invocation error provider".into());
        }
        if datatype.name() == "EEnumerator" && contract.provider.name != "ecore.enumerator" {
            return Err("Pinned EEnumerator requires the installed enum adapter provider".into());
        }
        if datatype.name() == "EJavaClass" && contract.provider.name != "ecore.class" {
            return Err(
                "Pinned EJavaClass requires the installed class descriptor provider".into(),
            );
        }
        datatype
    } else {
        intersect_model::definitions::compiler::import_datatype_bundle(bundle, &contract.datatype)
            .map_err(|error| error.to_string())?
    };
    if datatype.codec().is_ok() {
        return Err("Custom provider cannot override a builtin datatype codec".into());
    }
    Ok(datatype)
}

fn build_collection(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    contract: &ConversionContract,
    datatype: &intersect_model::definitions::datatypes::Datatype,
    remaining: &mut CatalogBudget,
    depth: usize,
) -> Result<crate::collection_conversion::CollectionConverter, String> {
    if !matches!(
        contract.provider.name.as_str(),
        "ecore.list" | "ecore.map" | "java.list" | "java.collection"
    ) {
        return Err("Collection operations require an installed collection provider".into());
    }
    use crate::collection_conversion::CollectionConverter;
    let (class, kind, count) = CollectionDomain::provider(&contract.provider.name)?.metadata();
    if datatype.instance_class_name() != Some(class)
        || datatype
            .instance_type_name()
            .is_some_and(|name| name != class)
    {
        return Err("Collection provider requires its corresponding datatype binding".into());
    }
    if contract.members.len() != count {
        return Err("Wrong collection member contract count".into());
    }
    let members = contract
        .members
        .iter()
        .map(|member| build_member(bundle, member, remaining, depth))
        .collect::<Result<Vec<_>, _>>()?;
    CollectionConverter::new(contract.clone(), kind, members)
}

fn operation_context(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    conversions: Option<&intersect_model::definitions::conversion::ConversionRegistry<'_>>,
) -> Result<CatalogBudget, String> {
    let mut context = CatalogBudget::default();
    if let Some(registry) = conversions {
        context.resolver = Some(
            intersect_model::definitions::generic_resolution::Resolver::new(bundle)
                .map_err(|e| e.to_string())?
                .with_conversions(registry)
                .map_err(|e| e.to_string())?,
        );
    }
    Ok(context)
}

/// Reconstruct a typed collection adapter through the same checked catalog path.
pub fn collection_provider(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    contract: &ConversionContract,
) -> Result<crate::collection_conversion::CollectionConverter, String> {
    collection_provider_context(bundle, contract, None)
}

/// Reconstruct an operation adapter with the package's selected bound providers.
pub fn collection_provider_with_conversions(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    contract: &ConversionContract,
    conversions: &intersect_model::definitions::conversion::ConversionRegistry<'_>,
) -> Result<crate::collection_conversion::CollectionConverter, String> {
    collection_provider_context(bundle, contract, Some(conversions))
}
/// Recheck an exact retained application and reuse its installed collection adapter.
pub fn applied_collection_provider_with_conversions(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    contract: &intersect_model::definitions::conversion::AppliedConversionContract,
    conversions: &intersect_model::definitions::conversion::ConversionRegistry<'_>,
) -> Result<crate::collection_conversion::CollectionConverter, String> {
    if conversions.applied().require(&contract.application)? != contract {
        return Err("Applied operation provider differs from selected contract".into());
    }
    let mut context = operation_context(bundle, Some(conversions))?;
    let provider = build_application_provider(bundle, contract, &mut context, 0)?;
    match provider.implementation {
        ApplicationImplementation::Collection(implementation) => Ok(*implementation),
        ApplicationImplementation::Sequence(_) => {
            Err("Collection operations require an installed collection provider".into())
        }
    }
}

fn collection_provider_context(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    contract: &ConversionContract,
    conversions: Option<&intersect_model::definitions::conversion::ConversionRegistry<'_>>,
) -> Result<crate::collection_conversion::CollectionConverter, String> {
    let mut remaining = operation_context(bundle, conversions)?;
    let datatype = provider_datatype(bundle, contract, &mut remaining, 0)?;
    let provider = build_collection(bundle, contract, &datatype, &mut remaining, 0)?;
    let mut registry = intersect_model::definitions::conversion::ConversionRegistry::default();
    registry.register(&datatype, &provider)?;
    drop(registry);
    Ok(provider)
}

fn build_feature_map(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    contract: &ConversionContract,
    datatype: &intersect_model::definitions::datatypes::Datatype,
    remaining: &mut CatalogBudget,
    depth: usize,
) -> Result<crate::feature_map_conversion::FeatureMapConverter, String> {
    use crate::feature_map_conversion::{FeatureMapConverter, FeatureMapKind};
    let (class, kind) = match contract.provider.name.as_str() {
        "ecore.feature-map" => ("org.eclipse.emf.ecore.util.FeatureMap", FeatureMapKind::Map),
        "ecore.feature-entry" => (
            "org.eclipse.emf.ecore.util.FeatureMap$Entry",
            FeatureMapKind::Entry,
        ),
        _ => return Err("Feature-map operations require an installed feature-map provider".into()),
    };
    if datatype.instance_class_name() != Some(class)
        || datatype
            .instance_type_name()
            .is_some_and(|name| name != class)
    {
        return Err("Feature-map provider requires its corresponding datatype binding".into());
    }
    if contract.members.len() != contract.declarations.len() {
        return Err("Feature-map scope requires one member policy per feature".into());
    }
    let members = contract
        .members
        .iter()
        .map(|member| build_member(bundle, member, remaining, depth))
        .collect::<Result<Vec<_>, _>>()?;
    FeatureMapConverter::new(contract.clone(), kind, bundle, members)
}

/// Reconstruct the typed feature-map adapter from exact retained source policies.
pub fn feature_map_provider(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    contract: &ConversionContract,
) -> Result<crate::feature_map_conversion::FeatureMapConverter, String> {
    feature_map_provider_context(bundle, contract, None)
}

/// Reconstruct an operation adapter with the package's selected bound providers.
pub fn feature_map_provider_with_conversions(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    contract: &ConversionContract,
    conversions: &intersect_model::definitions::conversion::ConversionRegistry<'_>,
) -> Result<crate::feature_map_conversion::FeatureMapConverter, String> {
    feature_map_provider_context(bundle, contract, Some(conversions))
}
fn feature_map_provider_context(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    contract: &ConversionContract,
    conversions: Option<&intersect_model::definitions::conversion::ConversionRegistry<'_>>,
) -> Result<crate::feature_map_conversion::FeatureMapConverter, String> {
    let mut remaining = operation_context(bundle, conversions)?;
    let datatype = provider_datatype(bundle, contract, &mut remaining, 0)?;
    let provider = build_feature_map(bundle, contract, &datatype, &mut remaining, 0)?;
    let mut registry = intersect_model::definitions::conversion::ConversionRegistry::default();
    registry.register(&datatype, &provider)?;
    drop(registry);
    Ok(provider)
}

/// Source-checked installed specialization; payload algorithms stay in language-owned codecs.
pub struct ApplicationProvider {
    contract: intersect_model::definitions::conversion::AppliedConversionContract,
    datatype: intersect_model::definitions::datatypes::Datatype,
    checked: intersect_model::definitions::generic_resolution::CheckedDatatypeApplication,
    implementation: ApplicationImplementation,
}
enum ApplicationImplementation {
    Collection(Box<crate::collection_conversion::CollectionConverter>),
    Sequence(Box<crate::sequence_conversion::SequenceConverter>),
}
impl intersect_model::definitions::conversion::AppliedConversionProvider for ApplicationProvider {
    fn contract(&self) -> &intersect_model::definitions::conversion::AppliedConversionContract {
        &self.contract
    }
    fn implementation(&self) -> &dyn ConversionProvider {
        match &self.implementation {
            ApplicationImplementation::Collection(implementation) => implementation.as_ref(),
            ApplicationImplementation::Sequence(implementation) => implementation.as_ref(),
        }
    }
}
impl ApplicationProvider {
    pub(crate) fn with_registry<T>(
        &self,
        callback: impl FnOnce(
            &intersect_model::definitions::conversion::AppliedConversionRegistry<'_>,
        ) -> Result<T, String>,
    ) -> Result<T, String> {
        self.with_registry_observed(
            &intersect_model::definitions::conversion::InvocationContext::default(),
            callback,
        )
    }
    pub(crate) fn with_registry_observed<T>(
        &self,
        observation: &intersect_model::definitions::conversion::InvocationContext,
        callback: impl FnOnce(
            &intersect_model::definitions::conversion::AppliedConversionRegistry<'_>,
        ) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut registry =
            intersect_model::definitions::conversion::AppliedConversionRegistry::default()
                .with_observation(observation.clone());
        registry.register(&self.datatype, &self.checked, self)?;
        callback(&registry)
    }
}

/// Source-check and rebuild only installed application providers, including nested members.
pub fn with_application_catalog<T>(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    contracts: &[intersect_model::definitions::conversion::AppliedConversionContract],
    callback: impl FnOnce(
        &intersect_model::definitions::conversion::AppliedConversionRegistry<'_>,
    ) -> Result<T, String>,
) -> Result<T, String> {
    with_catalog_context(bundle, &[], contracts, |registry| {
        callback(registry.applied())
    })
}

fn build_application_provider(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    contract: &intersect_model::definitions::conversion::AppliedConversionContract,
    remaining: &mut CatalogBudget,
    depth: usize,
) -> Result<ApplicationProvider, String> {
    use intersect_model::definitions::{
        compiler, conversion::MemberType, generic_resolution::Resolver, generic_types::Type,
        resources::ResourceSet, scalars::Builtin,
    };
    if !matches!(
        contract.conversion.provider.name.as_str(),
        "ecore.list" | "ecore.map" | "java.list" | "java.collection" | "intersect.sequence"
    ) {
        return Err("No installed specialization for this application provider".into());
    }
    // Enforce the same version, scope and recursive budget policy as ordinary providers.
    provider_datatype(bundle, &contract.conversion, remaining, depth)?;
    let fallback;
    let resolver = if let Some(resolver) = &remaining.resolver {
        resolver
    } else {
        fallback = Resolver::new(bundle).map_err(|e| e.to_string())?;
        &fallback
    };
    let checked = resolver
        .instantiate_datatype(&contract.application)
        .map_err(|e| e.to_string())?;
    remaining
        .dependencies
        .extend(resolver.consumed_relation_datatypes());
    for contract in resolver.consumed_constructor_contracts() {
        remaining
            .constructor_dependencies
            .insert(contract.datatype.declaration().clone(), contract);
    }
    let Type::Classifier {
        declaration,
        arguments,
        ..
    } = contract.application.expression()
    else {
        return Err("Expected closed datatype application".into());
    };
    if declaration != &contract.conversion.datatype {
        return Err("Application and conversion declaration differ".into());
    }
    let mut sources = bundle.clone();
    let standard = compiler::standard_datatype_source();
    if !sources
        .sources
        .iter()
        .any(|source| source.id == standard.id)
    {
        sources.sources.push(standard.clone());
    }
    let datatype =
        compiler::import_datatype_bundle(&sources, declaration).map_err(|e| e.to_string())?;
    if arguments.len() != contract.conversion.members.len() || arguments.is_empty() {
        return Err("Application arguments must match ordered member contracts".into());
    }
    let resources = ResourceSet::new_diagnostic(&sources).map_err(|e| e.to_string())?;
    for (argument, member) in arguments.iter().zip(&contract.conversion.members) {
        let Type::Classifier {
            declaration,
            arguments,
            ..
        } = argument
        else {
            return Err("Expected closed member argument".into());
        };
        let matches = match &member.value {
            MemberType::Applied { contract } => contract.application.expression() == argument,
            MemberType::Builtin { codec }
                if declaration.source == standard.id && arguments.is_empty() =>
            {
                let node = resources.node(declaration)?;
                let name = node
                    .attributes
                    .iter()
                    .find(|a| a.name.namespace.is_none() && a.name.local == "name")
                    .map(|a| a.value.as_str());
                name.and_then(Builtin::named) == Some(*codec)
            }
            MemberType::Datatype {
                declaration: actual,
            }
            | MemberType::Enumeration {
                declaration: actual,
            } => arguments.is_empty() && actual == declaration,
            MemberType::Converted { contract } => {
                arguments.is_empty() && &contract.datatype == declaration
            }
            MemberType::Reference { class, .. } => arguments.is_empty() && class == declaration,
            MemberType::RuntimeReference { target, .. } => target.expression() == argument,
            _ => false,
        };
        if !matches {
            return Err("Member domain differs from its exact application argument".into());
        }
    }
    let implementation = if contract.conversion.provider.name == "intersect.sequence" {
        if datatype.instance_class_name() != Some("intersect.Sequence")
            || datatype
                .instance_type_name()
                .is_some_and(|name| name != "intersect.Sequence")
            || datatype.parameters().len() != 1
        {
            return Err("Sequence provider requires intersect.Sequence<T> metadata".into());
        }
        let member = contract
            .conversion
            .members
            .first()
            .ok_or("Sequence requires one member")?;
        if matches!(
            member.value,
            MemberType::Reference { .. } | MemberType::RuntimeReference { .. }
        ) {
            return Err("Sequence reference members have no lexical identity codec".into());
        }
        ApplicationImplementation::Sequence(Box::new(
            crate::sequence_conversion::SequenceConverter::new(
                contract.conversion.clone(),
                build_member(&sources, member, remaining, depth + 1)?,
            )?,
        ))
    } else {
        ApplicationImplementation::Collection(Box::new(build_collection(
            &sources,
            &contract.conversion,
            &datatype,
            remaining,
            depth,
        )?))
    };
    let provider = ApplicationProvider {
        contract: contract.clone(),
        datatype,
        checked,
        implementation,
    };
    provider.with_registry(|_| Ok(()))?;
    Ok(provider)
}

/// Build a retained selection from exact source metadata for an installed policy.
/// This does not construct a value provider or establish application conformance.
pub fn constructor_contract(
    bundle: &intersect_model::definitions::bundle::DefinitionBundle,
    declaration: &intersect_model::definitions::resources::DeclarationId,
    provider: intersect_model::definitions::conversion::ProviderId,
) -> Result<intersect_model::definitions::conversion::ConstructorContract, String> {
    let contract = intersect_model::definitions::conversion::ConstructorContract {
        datatype: intersect_model::definitions::compiler::import_datatype_bundle(
            bundle,
            declaration,
        )
        .map_err(|e| e.to_string())?,
        provider,
    };
    let implementation = CollectionConstructor::new(contract.clone())?;
    let mut registry = intersect_model::definitions::conversion::ConstructorRegistry::default();
    registry.register(bundle, &implementation)?;
    Ok(contract)
}

#[cfg(test)]
#[path = "conversion_resource_tests.rs"]
mod resource_sharing_tests;
