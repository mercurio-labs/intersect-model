//! Handwritten exact provider-policy selection for reference admission.
//! Inventories remain immutable; one full contract equality is an atomic step.
use intersect_model::definitions::conversion::ReferenceContext;

const MISMATCH: &str = "Reference context converter selections differ from compiled schema";
enum SelectionStage {
    Header,
    Previous,
    Consumed,
    Next,
    Done,
}
struct SelectionPreparation {
    stage: SelectionStage,
    row: usize,
    offset: usize,
    failed: bool,
}
impl SelectionPreparation {
    fn new() -> Self {
        Self {
            stage: SelectionStage::Header,
            row: 0,
            offset: 0,
            failed: false,
        }
    }
    fn advance<T: PartialEq>(&mut self, selected: &[T], consumed: &[T]) -> Result<bool, String> {
        if self.failed {
            return Err("Reference policy selection has failed".into());
        }
        let result = self.advance_inner(selected, consumed);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn advance_inner<T: PartialEq>(
        &mut self,
        selected: &[T],
        consumed: &[T],
    ) -> Result<bool, String> {
        match self.stage {
            SelectionStage::Header => {
                if selected.len() != consumed.len() {
                    return Err(MISMATCH.into());
                }
                self.stage = if selected.is_empty() {
                    SelectionStage::Done
                } else {
                    SelectionStage::Previous
                };
            }
            SelectionStage::Previous => {
                if self.offset < self.row {
                    let value = selected.get(self.row).ok_or(MISMATCH)?;
                    if selected.get(self.offset).ok_or(MISMATCH)? == value {
                        return Err(MISMATCH.into());
                    }
                    self.offset += 1;
                } else {
                    self.offset = 0;
                    self.stage = SelectionStage::Consumed;
                }
            }
            SelectionStage::Consumed => {
                let actual = consumed.get(self.offset).ok_or(MISMATCH)?;
                if actual == selected.get(self.row).ok_or(MISMATCH)? {
                    self.stage = SelectionStage::Next;
                } else {
                    self.offset += 1;
                }
            }
            SelectionStage::Next => {
                self.row += 1;
                self.offset = 0;
                self.stage = if self.row == selected.len() {
                    SelectionStage::Done
                } else {
                    SelectionStage::Previous
                };
            }
            SelectionStage::Done => return Ok(true),
        }
        Ok(matches!(self.stage, SelectionStage::Done))
    }
    fn finish(self) -> Result<(), String> {
        if self.failed || !matches!(self.stage, SelectionStage::Done) {
            return Err("Reference policy selection is not complete".into());
        }
        Ok(())
    }
}
enum PolicyKind {
    Ordinary,
    Applied,
    Constructors,
    Done,
}
pub(super) struct PolicyPreparation {
    kind: PolicyKind,
    selection: Option<SelectionPreparation>,
}
impl PolicyPreparation {
    pub(super) fn new() -> Self {
        Self {
            kind: PolicyKind::Ordinary,
            selection: Some(SelectionPreparation::new()),
        }
    }
    pub(super) fn advance(
        &mut self,
        context: &ReferenceContext,
        schema: &intersect_model::definitions::schema::SchemaRepresentation<
            impl intersect_model::definitions::schema::RuntimeAlternative,
        >,
    ) -> Result<bool, String> {
        if matches!(self.kind, PolicyKind::Done) {
            return Ok(true);
        }
        let selection = self
            .selection
            .as_mut()
            .ok_or("Missing reference policy selection")?;
        let done = match self.kind {
            PolicyKind::Ordinary => {
                selection.advance(&context.conversions, &schema.conversion_contracts)?
            }
            PolicyKind::Applied => selection.advance(
                &context.applied_conversions,
                &schema.applied_conversion_contracts,
            )?,
            PolicyKind::Constructors => {
                selection.advance(&context.constructors, &schema.constructor_contracts)?
            }
            PolicyKind::Done => return Ok(true),
        };
        if done {
            self.selection
                .take()
                .ok_or("Missing reference policy selection")?
                .finish()?;
            self.kind = match self.kind {
                PolicyKind::Ordinary => PolicyKind::Applied,
                PolicyKind::Applied => PolicyKind::Constructors,
                PolicyKind::Constructors | PolicyKind::Done => PolicyKind::Done,
            };
            if !matches!(self.kind, PolicyKind::Done) {
                self.selection = Some(SelectionPreparation::new());
            }
        }
        Ok(matches!(self.kind, PolicyKind::Done))
    }
    pub(super) fn finish(self) -> Result<(), String> {
        if !matches!(self.kind, PolicyKind::Done) {
            return Err("Reference policy preparation is not complete".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use intersect_model::definitions::{
        bundle::{DefinitionBundle, SourceId},
        compiler,
        conversion::{
            AppliedConversionContract, ConstructorContract, ConversionContract, MemberContract,
            MemberType, ProviderId,
        },
        generic_types::{ClosedType, Type},
    };
    use serde_json::json;

    fn native_datatype(name: &str) -> intersect_model::definitions::datatypes::Datatype {
        let source = compiler::standard_datatype_source();
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![source.clone()],
            namespaces: vec![],
        };
        let id = intersect_model::definitions::resources::ResourceSet::new_diagnostic(&bundle)
            .unwrap()
            .resolve(&source.id, &format!("#//{name}"))
            .unwrap();
        compiler::import_datatype_bundle(&bundle, &id).unwrap()
    }

    #[test]
    fn exact_selection_yields_wide_permutations_and_never_finishes_partial_or_failed() {
        let selected: Vec<_> = (0..64).collect();
        let consumed: Vec<_> = (0..64).rev().collect();
        let mut pending = SelectionPreparation::new();
        let mut steps = 0;
        loop {
            steps += 1;
            if pending.advance(&selected, &consumed).unwrap() {
                break;
            }
            assert!(steps < 10000);
        }
        assert!(steps > 64 * 64);
        assert!(pending.advance(&selected, &consumed).unwrap());
        pending.finish().unwrap();
        for stop in [0, 1, 64, steps / 2, steps - 1] {
            let mut pending = SelectionPreparation::new();
            for _ in 0..stop {
                assert!(!pending.advance(&selected, &consumed).unwrap());
            }
            assert!(pending.finish().is_err());
        }
        for (selected, consumed) in [
            (vec![1], vec![]),
            (vec![1, 1], vec![1, 2]),
            (vec![1, 3], vec![1, 2]),
        ] {
            let mut pending = SelectionPreparation::new();
            let error = loop {
                match pending.advance(&selected, &consumed) {
                    Ok(false) => {}
                    Ok(true) => panic!("invalid selection accepted"),
                    Err(error) => break error,
                }
            };
            assert_eq!(error, MISMATCH);
            assert_eq!(
                pending.advance::<usize>(&[], &[]).unwrap_err(),
                "Reference policy selection has failed"
            );
            assert!(pending.finish().is_err());
        }
        let mut empty = SelectionPreparation::new();
        assert!(empty.advance::<usize>(&[], &[]).unwrap());
        empty.finish().unwrap();
    }

    #[test]
    fn all_policy_families_compare_full_versions_dependencies_and_application_identity() {
        let mut schema = compiler::inspect_ecore_schema(r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="p" nsURI="urn:policy-comparison"><eClassifiers x:type="e:EClass" name="C"/></e:EPackage>"#).unwrap();
        let datatype = native_datatype("EString");
        let conversion = ConversionContract {
            datatype: datatype.declaration().clone(),
            provider: ProviderId {
                name: "test.policy".into(),
                version: "1".into(),
            },
            serializable: true,
            declarations: vec![],
            members: vec![],
        };
        let application = ClosedType::new(Type::Classifier {
            declaration: datatype.declaration().clone(),
            arguments: vec![],
            raw: false,
        })
        .unwrap();
        schema.conversion_contracts = vec![conversion.clone()];
        schema.applied_conversion_contracts = vec![AppliedConversionContract {
            application,
            conversion,
        }];
        schema.constructor_contracts = vec![ConstructorContract {
            datatype,
            provider: ProviderId {
                name: "test.constructor".into(),
                version: "1".into(),
            },
        }];
        // Deliberate immutable table fixtures exercise equality/scheduling only;
        // they do not establish source-valid registration of these policies.
        let context = ReferenceContext {
            bundle: DefinitionBundle {
                version: 1,
                sources: vec![],
                namespaces: vec![],
            },
            entry: SourceId {
                uri: "memory:policies".into(),
                sha256: "0".repeat(64),
            },
            namespace: "urn:policy-comparison".into(),
            conversions: schema.conversion_contracts.clone(),
            applied_conversions: schema.applied_conversion_contracts.clone(),
            constructors: schema.constructor_contracts.clone(),
            materialized_types: vec![],
            model: json!(null),
        };
        let mut pending = PolicyPreparation::new();
        while !pending.advance(&context, &schema).unwrap() {}
        assert!(pending.advance(&context, &schema).unwrap());
        pending.finish().unwrap();
        for mutation in 0..7 {
            let mut changed = context.clone();
            match mutation {
                0 => changed.conversions[0].provider.version = "2".into(),
                1 => {
                    let dependency = changed.conversions[0].datatype.clone();
                    changed.conversions[0].declarations.push(dependency);
                }
                2 => changed.conversions[0].members.push(MemberContract {
                    nullable: true,
                    value: MemberType::Builtin {
                        codec: intersect_model::definitions::scalars::Builtin::EInt,
                    },
                }),
                3 => {
                    changed.applied_conversions[0].application = ClosedType::new(Type::Classifier {
                        declaration: native_datatype("EInt").declaration().clone(),
                        arguments: vec![],
                        raw: false,
                    })
                    .unwrap()
                }
                4 => changed.applied_conversions[0].conversion.provider.version = "2".into(),
                5 => changed.constructors[0].provider.version = "2".into(),
                _ => changed.constructors[0].datatype = native_datatype("EInt"),
            }
            let mut pending = PolicyPreparation::new();
            let error = loop {
                match pending.advance(&changed, &schema) {
                    Ok(false) => {}
                    Ok(true) => panic!("mutated policy accepted: {mutation}"),
                    Err(error) => break error,
                }
            };
            assert_eq!(error, MISMATCH);
            assert!(pending.finish().is_err());
        }
    }
}
