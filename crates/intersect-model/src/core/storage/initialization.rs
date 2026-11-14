//! Atomic per-class initialization through the existing stored mutation authority.
use super::*;
use crate::core::query::QueryBudget;
use intersect_model::definitions::{
    bundle::DefinitionSource,
    initialization::{ClassInitializationContract, InitializationReceipt},
    schema::MemberIdentity,
};

pub struct InitialValue {
    pub feature: MemberIdentity,
    pub value: Value,
}

/// Language-owned constructor behavior. Plans read the current store, honor the
/// explicit work budget, and have no I/O or external side effects. Only declared
/// fields on the new object may be initialized; opposites use shared core rules.
pub trait ClassInitializer {
    fn contract(&self) -> &ClassInitializationContract;
    fn plan(
        &self,
        store: &dyn ModelRead,
        id: &str,
        budget: &mut QueryBudget,
    ) -> StorageResult<Vec<InitialValue>>;
}

pub struct Construction<'a> {
    pub id: &'a str,
    pub resource: &'a str,
    /// Applied after constructor values through ordinary checked setters. These
    /// override writable defaults, but cannot author a readonly feature.
    pub authored: &'a [(String, Value)],
}

/// Borrowed constructor dependencies for checked facades. The facade supplies
/// its own schema; callers cannot substitute another schema for an owned model.
pub struct InitializationBinding<'a> {
    pub expected: &'a ClassInitializationContract,
    pub sources: &'a [DefinitionSource],
    pub provider: Option<&'a dyn ClassInitializer>,
    pub conversions: Option<&'a ConversionRegistry<'a>>,
}

/// Language-neutral selection borrowed for one checked source-bound creation.
/// Returning None retains schema-default creation. Implementations use the same
/// caller budget; neither selectors nor providers are retained by the store.
pub trait ClassInitializationSelector {
    fn select<'a>(
        &'a self,
        class: &str,
        budget: &mut QueryBudget,
    ) -> StorageResult<Option<InitializationBinding<'a>>>;
}

/// Select the current source-owned constructor for one genuinely fresh snapshot
/// row. The codec invokes its unchanged checked plan before any serialized
/// authored field is installed; metadata alone never authorizes readonly writes.
pub trait SnapshotInitializationSelector {
    fn select<'a>(
        &'a self,
        fresh_store: &dyn ModelRead,
        schema: &Schema,
        object: &str,
        budget: &mut QueryBudget,
    ) -> StorageResult<Option<InitializationBinding<'a>>>;
    /// Rebind current exact source/class/provider metadata for an opaque checked
    /// decode proof. This never executes a plan on populated or retained rows.
    fn revalidate<'a>(
        &'a self,
        _store: &dyn ModelRead,
        _schema: &Schema,
        _object: &str,
        _budget: &mut QueryBudget,
    ) -> StorageResult<Option<InitializationBinding<'a>>> {
        Err("Snapshot initializer publication revalidation is unsupported".into())
    }
}

#[cfg(feature = "xmi")]
#[derive(Clone, Copy)]
pub(crate) struct FreshReadonlySnapshot<'a> {
    pub object: &'a str,
    pub values: &'a [(String, Value)],
    pub receipt: &'a InitializationReceipt,
}

/// Explicit caller-selected initialization authority. Providers and source text
/// are borrowed only during construction, never hidden in a store or snapshot.
pub struct InitializationContext<'a> {
    pub schema: &'a Schema,
    pub expected: &'a ClassInitializationContract,
    pub sources: &'a [DefinitionSource],
    pub provider: Option<&'a dyn ClassInitializer>,
    pub conversions: Option<&'a ConversionRegistry<'a>>,
}
impl<'initialization> InitializationContext<'initialization> {
    pub fn create(
        &self,
        store: &mut impl ModelStore,
        request: Construction<'_>,
        budget: &mut QueryBudget,
    ) -> StorageResult<InitializationReceipt> {
        self.create_with_reference_policy(store, request, budget, DraftReferencePolicy::Resolved)
    }

    /// Construct during a caller-owned linking transaction. This only changes
    /// checks for existing canonical pending references; constructor writes,
    /// provenance, receipts and all other structural checks remain authoritative.
    /// Callers selecting a provisional policy must link and validate before use.
    pub fn create_with_reference_policy(
        &self,
        store: &mut impl ModelStore,
        request: Construction<'_>,
        budget: &mut QueryBudget,
        references: DraftReferencePolicy,
    ) -> StorageResult<InitializationReceipt> {
        let mut prepared = self.prepare_new(request.authored, budget)?;
        atomic(store, |store| {
            bind_schema(store, self.schema)?;
            store.create_element(
                self.schema,
                request.id,
                &self.expected.class,
                request.resource,
                0,
                0,
            )?;
            let receipt = prepared.apply_to_new(store, &request, self.conversions, budget)?;
            let mut validation = StructuralPreparation::draft(references);
            while !validation.advance(store, self.schema, self.conversions, None)? {}
            validation.finish()?;
            prepared.check_after_execution()?;
            Ok(receipt)
        })
    }

    #[cfg(feature = "xmi")]
    pub(crate) fn verify_fresh_readonly(
        &self,
        store: &dyn ModelRead,
        request: FreshReadonlySnapshot<'_>,
        budget: &mut QueryBudget,
    ) -> StorageResult<()> {
        let id = request.object;
        let restored = request.values;
        let receipt = request.receipt;
        let prepared = self.prepare_new(&[], budget)?;
        let fields = self.schema.features(&self.expected.class)?;
        let mut found = false;
        store.inspect(id, &mut |object| {
            found = true;
            budget.charge()?;
            if object.class() != self.expected.class
                || object.type_identity() != self.expected.class_identity
            {
                return Err("Fresh snapshot initializer actual class mismatch".into());
            }
            if object.initialization().is_some() {
                return Err("Fresh snapshot initializer cannot inherit an imported receipt".into());
            }
            if receipt.object != id || receipt.contract != *self.expected {
                return Err("Fresh readonly snapshot initialization receipt mismatch".into());
            }
            for member in &self.expected.members {
                budget.charge()?;
                let name = prepared
                    .available
                    .get(member)
                    .ok_or("Unknown snapshot initialization member")?;
                let field = fields
                    .get(name)
                    .ok_or("Unknown snapshot initialization field")?;
                if !object
                    .value(name)
                    .is_some_and(|value| value.equals_json(&field.default))
                {
                    return Err(
                        "Snapshot initializer must see genuinely fresh default member state".into(),
                    );
                }
            }
            for member in &receipt.authored_members {
                budget.charge()?;
                let field = self.schema.feature_by_id(object.class(), member)?;
                if field.feature.derived || !field.feature.changeable {
                    return Err(
                        "Readonly snapshot receipt cannot claim readonly authored members".into(),
                    );
                }
            }
            Ok(())
        })?;
        if !found {
            return Err("Missing fresh snapshot initialization receiver".into());
        }
        let plan = prepared.checked_plan(store, id, budget)?;
        let mut seen = BTreeSet::new();
        for (name, value) in restored {
            budget.charge()?;
            if !seen.insert(name) {
                return Err("Duplicate readonly snapshot member".into());
            }
            let field = fields.get(name).ok_or("Unknown readonly snapshot member")?;
            if field.changeable
                || field.derived
                || field.reference
                || field.containment
                || field.upper != 1
            {
                return Err("Unsupported readonly snapshot member".into());
            }
            let member = self
                .schema
                .effective_feature(&self.expected.class, name)?
                .identity;
            if !prepared.members.contains(&member) {
                return Err(
                    "Readonly snapshot member is not declared by current initializer".into(),
                );
            }
            let expected = plan
                .iter()
                .find(|item| item.feature == member)
                .ok_or("Missing readonly snapshot verification value")?;
            if expected.value != *value {
                return Err(
                    "Readonly snapshot value does not match fresh current initializer plan".into(),
                );
            }
        }
        prepared.check_after_execution()
    }

    pub(crate) fn validate_snapshot_binding(
        &self,
        store: &dyn ModelRead,
        object: &str,
        budget: &mut QueryBudget,
    ) -> StorageResult<()> {
        let prepared = self.prepare_new(&[], budget)?;
        let mut found = false;
        store.inspect(object, &mut |row| {
            budget.charge()?;
            found = true;
            if row.class() != self.expected.class
                || row.type_identity() != self.expected.class_identity
            {
                return Err("Current snapshot initializer actual class mismatch".into());
            }
            Ok(())
        })?;
        if !found {
            return Err("Missing current snapshot initializer receiver".into());
        }
        prepared.check_after_execution()
    }

    // One shared source/member binding for ordinary and draft fresh creation.
    pub(super) fn prepare_new(
        &self,
        authored: &[(String, Value)],
        budget: &mut QueryBudget,
    ) -> StorageResult<PreparedInitialization<'initialization>> {
        budget.charge()?;
        let contract = self.expected;
        if contract.version != 1
            || contract.schema_revision != self.schema.revision
            || !self.schema.classes.contains_key(&contract.class)
            || contract.class_identity != self.schema.identity(&contract.class)
            || contract.provider.name.trim().is_empty()
            || contract.provider.version.trim().is_empty()
        {
            return Err("Invalid or stale class initialization contract".into());
        }
        if contract.sources.is_empty() {
            return Err("Class initialization requires explicit source provenance".into());
        }
        let mut required_sources = BTreeSet::new();
        for source in &contract.sources {
            budget.charge()?;
            if !required_sources.insert(source) {
                return Err("Duplicate initialization source dependency".into());
            }
        }
        let mut sources = BTreeSet::new();
        for source in self.sources {
            budget.charge()?;
            for _ in 0..source.text.len().div_ceil(1024) {
                budget.charge()?;
            }
            source.validate()?;
            if !sources.insert(&source.id) {
                return Err("Duplicate initialization source".into());
            }
        }
        if !required_sources.is_subset(&sources) {
            return Err("Missing or stale class initialization source".into());
        }
        let fields = self.schema.features(&contract.class)?;
        let mut available = BTreeMap::new();
        for (name, field) in &fields {
            budget.charge()?;
            if !field.derived {
                available.insert(
                    self.schema
                        .effective_feature(&contract.class, name)?
                        .identity,
                    name.clone(),
                );
            }
        }
        let mut members = BTreeSet::new();
        for member in &contract.members {
            budget.charge()?;
            if !available.contains_key(member) || !members.insert(member) {
                return Err("Invalid or duplicate class initialization member".into());
            }
        }
        let mut authored_members = Vec::new();
        let mut authored_names = BTreeSet::new();
        for (name, _) in authored {
            budget.charge()?;
            if !authored_names.insert(name) {
                return Err("Duplicate authored initialization member".into());
            }
            authored_members.push(
                self.schema
                    .effective_feature(&contract.class, name)?
                    .identity,
            );
        }
        let provider = self
            .provider
            .ok_or("Required class initializer is not installed")?;
        if provider.contract() != contract {
            return Err("Class initializer provider contract mismatch".into());
        }
        Ok(PreparedInitialization {
            schema: self.schema,
            contract,
            provider,
            available,
            members,
            authored_members,
        })
    }
}

// Private prepared authority; callers must insert the fresh ID in the same
// transaction before calling apply_to_new. There is no public existing-object
// initializer API. The canonical plan, member checks, setters and receipt are
// shared by both creator routes.
pub(super) struct PreparedInitialization<'a> {
    schema: &'a Schema,
    contract: &'a ClassInitializationContract,
    provider: &'a dyn ClassInitializer,
    available: BTreeMap<MemberIdentity, String>,
    members: BTreeSet<&'a MemberIdentity>,
    authored_members: Vec<MemberIdentity>,
}
impl PreparedInitialization<'_> {
    fn checked_plan(
        &self,
        store: &dyn ModelRead,
        id: &str,
        budget: &mut QueryBudget,
    ) -> StorageResult<Vec<InitialValue>> {
        let plan = self.provider.plan(store, id, budget)?;
        let mut planned = BTreeSet::new();
        for value in &plan {
            budget.charge()?;
            if !self.members.contains(&value.feature) || !planned.insert(&value.feature) {
                return Err("Initializer plan contains an undeclared or duplicate member".into());
            }
        }
        if planned != self.members {
            return Err("Initializer plan omitted a required member".into());
        }
        if self.provider.contract() != self.contract {
            return Err("Class initializer contract changed during planning".into());
        }
        Ok(plan)
    }

    pub(super) fn apply_to_new(
        &mut self,
        store: &mut impl ModelStore,
        request: &Construction<'_>,
        conversions: Option<&ConversionRegistry<'_>>,
        budget: &mut QueryBudget,
    ) -> StorageResult<InitializationReceipt> {
        let plan = self.checked_plan(store, request.id, budget)?;
        for value in plan {
            budget.charge()?;
            let name = self
                .available
                .get(&value.feature)
                .ok_or("Unknown initialization member")?;
            // This private permission is reachable only after creating this ID
            // in the same transaction. It cannot mutate an existing readonly field.
            write_inner(
                store,
                self.schema,
                None,
                request.id,
                name,
                value.value,
                conversions,
                WriteMode::Initialize,
            )?;
        }
        for (name, value) in request.authored {
            budget.charge()?;
            set_inner(
                store,
                self.schema,
                request.id,
                name,
                value.clone(),
                conversions,
            )?;
        }
        let receipt = InitializationReceipt {
            contract: self.contract.clone(),
            object: request.id.into(),
            authored_members: std::mem::take(&mut self.authored_members),
        };
        store.set_initialization(request.id, Some(receipt.clone()))?;
        Ok(receipt)
    }
    pub(super) fn check_after_execution(&self) -> StorageResult<()> {
        if self.provider.contract() != self.contract {
            return Err("Class initializer contract changed during execution".into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "draft_initializer_tests.rs"]
mod draft_initializer_tests;

pub(super) trait MemberLookup {
    fn member_identity(&self, name: &str) -> Option<&MemberIdentity>;
}
impl MemberLookup for BTreeMap<String, MemberIdentity> {
    fn member_identity(&self, name: &str) -> Option<&MemberIdentity> {
        self.get(name)
    }
}

enum ReceiptStage {
    Header,
    Sources,
    SourcesClear,
    Available,
    ContractMembers,
    ContractClear,
    AuthoredMembers,
    MembersClear,
    Done,
}

/// Cooperative validation of retained initialization evidence. The object and
/// schema must remain unchanged for the lifetime of the preparation.
pub(super) struct ReceiptPreparation {
    class: String,
    stage: ReceiptStage,
    index: usize,
    last_field: Option<String>,
    available: BTreeSet<intersect_model::definitions::schema::MemberIdentity>,
    seen: BTreeSet<intersect_model::definitions::schema::MemberIdentity>,
    source_seen: BTreeSet<intersect_model::definitions::bundle::SourceId>,
}

impl ReceiptPreparation {
    pub(super) fn new(
        _schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        object: ObjectView<'_>,
    ) -> StorageResult<Self> {
        Ok(Self {
            class: object.class().into(),
            stage: if object.initialization().is_some() {
                ReceiptStage::Header
            } else {
                ReceiptStage::Done
            },
            index: 0,
            last_field: None,
            available: BTreeSet::new(),
            seen: BTreeSet::new(),
            source_seen: BTreeSet::new(),
        })
    }

    pub(super) fn advance(
        &mut self,
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
        object: ObjectView<'_>,
        fields: &(impl assignment::FieldLookup + ?Sized),
        member_identities: &(impl MemberLookup + ?Sized),
        mut charge: impl FnMut() -> StorageResult<()>,
    ) -> StorageResult<bool> {
        if matches!(self.stage, ReceiptStage::Done) {
            return Ok(true);
        }
        let receipt = object
            .initialization()
            .ok_or("Missing initialization receipt")?;
        let contract = &receipt.contract;
        match self.stage {
            ReceiptStage::Header => {
                charge()?;
                if receipt.object != object.id()
                    || contract.version != 1
                    || contract.schema_revision != schema.revision
                    || contract.class != self.class
                    || contract.class_identity != schema.identity(&self.class)
                    || contract.provider.name.trim().is_empty()
                    || contract.provider.version.trim().is_empty()
                    || contract.sources.is_empty()
                {
                    return Err("Invalid or stale initialization receipt".into());
                }
                self.stage = ReceiptStage::Sources;
                self.index = 0;
            }
            ReceiptStage::Sources => {
                if let Some(source) = contract.sources.get(self.index) {
                    charge()?;
                    if source.uri.is_empty()
                        || source.sha256.len() != 64
                        || !source.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
                        || !self.source_seen.insert(source.clone())
                    {
                        return Err("Invalid initialization receipt sources".into());
                    }
                    self.index += 1;
                } else {
                    self.stage = ReceiptStage::SourcesClear;
                    self.index = 0;
                }
            }
            ReceiptStage::SourcesClear => {
                if !self.source_seen.is_empty() {
                    charge()?;
                    self.source_seen.pop_first();
                }
                if self.source_seen.is_empty() {
                    self.stage = ReceiptStage::Available;
                }
            }
            ReceiptStage::Available => {
                let next = fields.next_field(self.last_field.as_deref());
                if let Some((name, field)) = next {
                    charge()?;
                    if !field.derived {
                        self.available.insert(
                            member_identities
                                .member_identity(name)
                                .ok_or("Missing cached initialization member identity")?
                                .clone(),
                        );
                    }
                    self.last_field = Some(name.to_owned());
                } else {
                    self.stage = ReceiptStage::ContractMembers;
                    self.index = 0;
                }
            }
            ReceiptStage::ContractMembers | ReceiptStage::AuthoredMembers => {
                let members = if matches!(self.stage, ReceiptStage::ContractMembers) {
                    &contract.members
                } else {
                    &receipt.authored_members
                };
                if let Some(member) = members.get(self.index) {
                    charge()?;
                    if !self.available.contains(member) || !self.seen.insert(member.clone()) {
                        return Err("Invalid initialization receipt members".into());
                    }
                    self.index += 1;
                } else if matches!(self.stage, ReceiptStage::ContractMembers) {
                    self.stage = ReceiptStage::ContractClear;
                    self.index = 0;
                } else {
                    self.stage = ReceiptStage::MembersClear;
                }
            }
            ReceiptStage::ContractClear => {
                if !self.seen.is_empty() {
                    charge()?;
                    self.seen.pop_first();
                }
                if self.seen.is_empty() {
                    self.stage = ReceiptStage::AuthoredMembers;
                }
            }
            ReceiptStage::MembersClear => {
                if !self.seen.is_empty() {
                    charge()?;
                    self.seen.pop_first();
                } else if !self.available.is_empty() {
                    charge()?;
                    self.available.pop_first();
                }
                if self.seen.is_empty() && self.available.is_empty() {
                    self.stage = ReceiptStage::Done;
                }
            }
            ReceiptStage::Done => {}
        }
        Ok(matches!(self.stage, ReceiptStage::Done))
    }
}

// Shared default-factory insertion. The legacy callback/checkpoint order is exact.
pub(super) fn insert_created_element<S: ModelStore + ?Sized>(
    store: &mut S,
    schema: &Schema,
    element: Element,
) -> StorageResult<()> {
    if store.class(&element.id)?.is_some() {
        return Err("Element identity must be unique and nonempty".into());
    }
    atomic(store, |store| {
        store.configure_schema(schema)?;
        store.put(element)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use intersect_model::definitions::{
        compiler,
        conversion::ProviderId,
        initialization::{ClassInitializationContract, InitializationReceipt},
    };

    #[test]
    fn receipt_validation_yields_sources_available_members_and_authored_members() {
        let schema = compiler::inspect_ecore_schema(
            r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:receipt"><eClassifiers x:type="e:EClass" name="Node"><eStructuralFeatures x:type="e:EAttribute" name="value" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/></eClassifiers></e:EPackage>"#,
        )
        .unwrap();
        let mut element =
            crate::core::model::Element::new(&schema, "n", "Node", "r", 0, 0).unwrap();
        let member = schema.effective_feature("Node", "value").unwrap().identity;
        let source = intersect_model::definitions::bundle::SourceId {
            uri: "memory:receipt".into(),
            sha256: "a".repeat(64),
        };
        element.initialization = Some(InitializationReceipt {
            object: "n".into(),
            authored_members: vec![member.clone()],
            contract: ClassInitializationContract {
                version: 1,
                schema_revision: schema.revision.clone(),
                class: "Node".into(),
                class_identity: schema.identity("Node"),
                provider: ProviderId {
                    name: "provider".into(),
                    version: "1".into(),
                },
                sources: vec![source],
                members: vec![member],
            },
        });
        let view = ObjectView::from_element(&element);
        let fields = schema.features("Node").unwrap();
        let member_identities: BTreeMap<String, MemberIdentity> = schema
            .effective_features("Node")
            .unwrap()
            .into_iter()
            .map(|member| (member.feature.name.clone(), member.identity))
            .collect();
        let mut pending = ReceiptPreparation::new(&schema, view).unwrap();
        let mut charged = 0;
        let mut slices = 0;
        loop {
            let before = charged;
            let done = pending
                .advance(&schema, view, &fields, &member_identities, || {
                    charged += 1;
                    Ok(())
                })
                .unwrap();
            assert!(charged - before <= 1);
            slices += 1;
            if done {
                break;
            }
            assert!(slices < 20);
        }
        assert_eq!(charged, 9);
        assert!(slices >= 4);
        for limit in 0..=charged {
            let mut pending = ReceiptPreparation::new(&schema, view).unwrap();
            let mut work = 0;
            let result = loop {
                match pending.advance(&schema, view, &fields, &member_identities, || {
                    if work == limit {
                        return Err("cancelled".into());
                    }
                    work += 1;
                    Ok(())
                }) {
                    Ok(false) => {}
                    other => break other,
                }
            };
            assert_eq!(work, limit);
            assert_eq!(
                result,
                if limit == charged {
                    Ok(true)
                } else {
                    Err("cancelled".into())
                }
            );
        }
        drop(pending);
        let receipt = element.initialization.as_mut().unwrap();
        receipt
            .authored_members
            .push(receipt.authored_members[0].clone());
        let view = ObjectView::from_element(&element);
        let mut pending = ReceiptPreparation::new(&schema, view).unwrap();
        loop {
            match pending.advance(&schema, view, &fields, &member_identities, || Ok(())) {
                Ok(false) => {}
                Ok(true) => panic!("duplicate authored member accepted"),
                Err(error) => {
                    assert_eq!(error, "Invalid initialization receipt members");
                    break;
                }
            }
        }
    }

    #[test]
    fn wide_inherited_receipt_uses_cached_exact_members_and_cancels_at_every_work_budget() {
        let fields = (0..64).map(|n| format!(r#"<eStructuralFeatures x:type="e:EAttribute" name="f{n:02}" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/>"#)).collect::<String>();
        let source = DefinitionSource::new(
            "memory:wide-receipt.ecore",
            intersect_model::definitions::bundle::SourceKind::Ecore,
            format!(
                r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="m" nsURI="urn:wide-receipt"><eClassifiers x:type="e:EClass" name="Base">{fields}</eClassifiers><eClassifiers x:type="e:EClass" name="Node" eSuperTypes="#//Base"/><eClassifiers x:type="e:EClass" name="Other"><eStructuralFeatures x:type="e:EAttribute" name="f63" eType="e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString"/></eClassifiers></e:EPackage>"##
            ),
        );
        let report =
            intersect_model::migration::migrate_ecore(source, "memory:wide-receipt.rcore").unwrap();
        let source = report
            .converted
            .unwrap_or_else(|| panic!("{:?}", report.issues));
        let schema = compiler::compile_structure_bundle(
            &intersect_model::definitions::bundle::DefinitionBundle {
                version: 1,
                sources: vec![source.clone()],
                namespaces: vec![],
            },
            &source.id,
            "urn:wide-receipt",
        )
        .unwrap();
        let fields = schema.features("Node").unwrap();
        let member_identities: BTreeMap<_, _> = schema
            .effective_features("Node")
            .unwrap()
            .into_iter()
            .map(|member| (member.feature.name.clone(), member.identity))
            .collect();
        let mut element =
            crate::core::model::Element::new(&schema, "n", "Node", "r", 0, 0).unwrap();
        let members: Vec<_> = member_identities.values().cloned().collect();
        element.initialization = Some(InitializationReceipt {
            object: "n".into(),
            authored_members: members.clone(),
            contract: ClassInitializationContract {
                version: 1,
                schema_revision: schema.revision.clone(),
                class: "Node".into(),
                class_identity: schema.identity("Node"),
                provider: ProviderId {
                    name: "provider".into(),
                    version: "1".into(),
                },
                sources: vec![schema.declarations["Node"].source.clone()],
                members,
            },
        });
        let view = ObjectView::from_element(&element);
        let mut pending = ReceiptPreparation::new(&schema, view).unwrap();
        let mut charged = 0;
        let mut slices = 0;
        loop {
            let before = charged;
            let done = pending
                .advance(&schema, view, &fields, &member_identities, || {
                    charged += 1;
                    Ok(())
                })
                .unwrap();
            assert!(charged - before <= 1);
            slices += 1;
            assert!(slices < 1000);
            if done {
                break;
            }
        }
        assert!(charged >= 64 * 6);
        for limit in 0..=charged {
            let mut pending = ReceiptPreparation::new(&schema, view).unwrap();
            let mut work = 0;
            let result = loop {
                match pending.advance(&schema, view, &fields, &member_identities, || {
                    if work == limit {
                        return Err("cancelled".into());
                    }
                    work += 1;
                    Ok(())
                }) {
                    Ok(false) => (),
                    other => break other,
                }
            };
            assert_eq!(work, limit);
            assert_eq!(
                result,
                if limit == charged {
                    Ok(true)
                } else {
                    Err("cancelled".into())
                }
            );
        }
        drop(pending);
        let foreign = schema.effective_feature("Other", "f63").unwrap().identity;
        element.initialization.as_mut().unwrap().contract.members[63] = foreign;
        let view = ObjectView::from_element(&element);
        let mut pending = ReceiptPreparation::new(&schema, view).unwrap();
        loop {
            match pending.advance(&schema, view, &fields, &member_identities, || Ok(())) {
                Ok(false) => (),
                Ok(true) => panic!("unrelated same-name receipt member accepted"),
                Err(error) => {
                    assert_eq!(error, "Invalid initialization receipt members");
                    break;
                }
            }
        }
    }
}
