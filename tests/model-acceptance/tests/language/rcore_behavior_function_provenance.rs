//! Independent receipts for actual checked provider invocation, never payload inference.
use intersect_model::core::model::Model;
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    compiler::materialize_runtime_bundle,
    conversion::{
        AppliedConversionContract, ConversionContract, ConversionRegistry, MemberContract,
        MemberType, ProviderId, ReferenceContext,
    },
    generic_types::{ClosedType, RuntimeType, Type},
    rcore_behavior::{
        function_lowering::compile_function_with_conversions,
        ir_v2::{self, DataWire, EntrySelection, Limits, ValueWire, VerifiedExecutable},
        modules::SymbolId,
    },
    resources::DeclarationId,
};

use intersect_model::{
    conversion::with_catalog_policies,
    model_reference::ReferenceDomain,
    rcore_behavior_functions::{ExecutionContext, ExecutionLimits, evaluate},
};
use serde_json::{Value, json};
fn provider(name: &str) -> ProviderId {
    ProviderId {
        name: name.into(),
        version: "1".into(),
    }
}
struct Fixture {
    model: DefinitionSource,
    ordinary: Vec<ConversionContract>,
    applications: Vec<AppliedConversionContract>,
}
impl Fixture {
    fn new() -> Self {
        let model = intersect_model_acceptance::migration_fixture::migrate_source_fixture(DefinitionSource::new(
            "memory:provenance.ecore",
            SourceKind::Ecore,
            r##"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="p" nsURI="urn:provenance"><eClassifiers x:type="e:EDataType" name="RGB" instanceClassName="example.RGB"/><eClassifiers x:type="e:EDataType" name="Seq" instanceClassName="intersect.Sequence"><eTypeParameters name="T"/></eClassifiers><eClassifiers x:type="e:EClass" name="Box"><eStructuralFeatures x:type="e:EAttribute" name="color" lowerBound="1" eType="#//RGB"/></eClassifiers><eClassifiers x:type="e:EDataType" name="List" instanceClassName="org.eclipse.emf.common.util.EList" serializable="false"><eTypeParameters name="T"/></eClassifiers></e:EPackage>"##,
        )).0;
        let id = |n| {
            intersect_model_acceptance::migration_fixture::named_declaration_fixture(
                &model,
                ["RGB", "Seq", "Box", "List"][n],
            )
        };
        let rgb = ConversionContract {
            datatype: id(0),
            provider: provider("example.rgb"),
            serializable: true,
            declarations: vec![],
            members: vec![],
        };
        let atom = Type::Classifier {
            declaration: id(0),
            arguments: vec![],
            raw: false,
        };
        let make = |argument, member| AppliedConversionContract {
            application: ClosedType::new(Type::Classifier {
                declaration: id(1),
                arguments: vec![argument],
                raw: false,
            })
            .unwrap(),
            conversion: ConversionContract {
                datatype: id(1),
                provider: provider("intersect.sequence"),
                serializable: true,
                declarations: vec![],
                members: vec![MemberContract {
                    nullable: false,
                    value: member,
                }],
            },
        };
        let sequence = make(
            atom,
            MemberType::Converted {
                contract: Box::new(rgb.clone()),
            },
        );
        let nested = make(
            sequence.application.expression().clone(),
            MemberType::Applied {
                contract: Box::new(sequence.clone()),
            },
        );
        Self {
            model,
            ordinary: vec![rgb],
            applications: vec![sequence, nested],
        }
    }
    fn id(&self, n: usize) -> DeclarationId {
        intersect_model_acceptance::migration_fixture::named_declaration_fixture(
            &self.model,
            ["RGB", "Seq", "Box", "List", "Entry", "Map"][n],
        )
    }
    fn bundle(&self, body: &str) -> (DefinitionBundle, DefinitionSource) {
        let mut source = DefinitionSource::new(
            "memory:provenance.rcore_behavior",
            SourceKind::RcoreBehavior,
            format!(
                "rcore 1;package provenance version \"1\";import model \"urn:provenance\" as m;{body}"
            ),
        );
        source.dependencies.push(self.model.id.clone());
        (
            DefinitionBundle {
                version: 1,
                sources: vec![self.model.clone(), source.clone()],
                namespaces: vec![],
            },
            source,
        )
    }
    fn reference(&self) -> ReferenceContext {
        let bundle = DefinitionBundle {
            version: 1,
            sources: vec![self.model.clone()],
            namespaces: vec![],
        };
        let target = RuntimeType::new(Type::Classifier {
            declaration: self.id(2),
            arguments: vec![],
            raw: false,
        })
        .unwrap();
        let snapshot = with_catalog_policies(&bundle, &self.ordinary, &[], &[], |registry| {
            let schema = materialize_runtime_bundle(
                &bundle,
                &self.model.id,
                "urn:provenance",
                std::slice::from_ref(&target),
                Some(registry),
            )
            .map_err(|e| e.to_string())?;
            let mut model = Model::default();
            model.create(
                &schema,
                "box",
                schema.class_key_for_type(&target)?,
                "memory:instance",
                0,
                1,
            )?;
            let color = serde_json::to_value(registry.parse(&self.id(0), "#010203")?)
                .map_err(|e| e.to_string())?;
            model.edit_with_conversions(
                &schema,
                &[("box".into(), "color".into(), color)],
                registry,
            )?;
            serde_json::to_value(model).map_err(|e| e.to_string())
        })
        .unwrap();
        ReferenceContext {
            bundle,
            entry: self.model.id.clone(),
            namespace: "urn:provenance".into(),
            conversions: self.ordinary.clone(),
            materialized_types: vec![target],
            applied_conversions: vec![],
            constructors: vec![],
            model: snapshot,
        }
    }
}
fn compile(
    bundle: &DefinitionBundle,
    source: &DefinitionSource,
    registry: &ConversionRegistry<'_>,
) -> VerifiedExecutable {
    compile_function_with_conversions(
        bundle,
        &source.id,
        &EntrySelection {
            symbol: SymbolId {
                source: source.id.clone(),
                declaration: 1,
            },
            types: vec![],
            arguments: vec![],
        },
        registry,
        &Limits::default(),
    )
    .unwrap_or_else(|e| panic!("{e:?}"))
}
fn input(image: &VerifiedExecutable, payload: Value) -> ValueWire {
    ValueWire {
        ty: image.program().functions[image.program().entry.0 as usize].parameters[0],
        data: DataWire::Datatype(payload),
    }
}

fn evidence_size(selected: &[ProviderId], consumed: &[ProviderId]) -> usize {
    selected
        .iter()
        .chain(consumed)
        .map(|p| serde_json::to_vec(p).unwrap().len())
        .sum()
}
#[test]
fn ordinary_provider_receipts_distinguish_selection_invocation_failure_and_forged_tag() {
    let f = Fixture::new();
    let (bundle, source) = f.bundle("fn run(x:m.RGB)->m.RGB={let keep=|n:Int8|x;keep(0)};");
    with_catalog_policies(&bundle, &f.ordinary, &f.applications, &[], |registry| {
        let image = compile(&bundle, &source, registry);
        let payload = serde_json::to_value(registry.parse(&f.id(0), "#010203")?).unwrap();
        let argument = input(&image, payload.clone());
        let context = ExecutionContext {
            bundle: &bundle,
            conversions: Some(registry),
            references: &[],
        };
        let good = evaluate(
            &image,
            std::slice::from_ref(&argument),
            &context,
            &ExecutionLimits::default(),
        )
        .unwrap();
        assert_eq!(good.value, argument);
        assert_eq!(
            good.selected_providers,
            vec![provider("example.rgb"), provider("intersect.sequence")]
        );
        assert_eq!(good.consumed_providers, vec![provider("example.rgb")]);
        assert!(good.consumed.provider_invocations > 0);
        assert_eq!(
            good.consumed.provider_evidence_bytes,
            evidence_size(&good.selected_providers, &good.consumed_providers)
        );
        let mut invalid = payload.clone();
        invalid["value"]["red"] = json!(300);
        let failed = evaluate(
            &image,
            &[input(&image, invalid)],
            &context,
            &ExecutionLimits::default(),
        )
        .unwrap_err();
        assert_eq!(failed.consumed_providers, vec![provider("example.rgb")]);
        assert!(failed.consumed.provider_invocations > 0);
        assert!(!failed.provider_evidence_truncated);
        let mut forged = payload.clone();
        forged["provider"]["version"] = json!("forged");
        let failed = evaluate(
            &image,
            &[input(&image, forged)],
            &context,
            &ExecutionLimits::default(),
        )
        .unwrap_err();
        assert!(failed.consumed_providers.is_empty());
        assert_eq!(failed.consumed.provider_invocations, 0);
        assert!(!failed.provider_evidence_truncated);
        let (unused_bundle, unused_source) = f.bundle("fn run()->Int8=1;");
        let unused = compile(&unused_bundle, &unused_source, registry);
        let context = ExecutionContext {
            bundle: &unused_bundle,
            conversions: Some(registry),
            references: &[],
        };
        let unused = evaluate(&unused, &[], &context, &ExecutionLimits::default()).unwrap();
        assert_eq!(unused.selected_providers, good.selected_providers);
        assert!(unused.consumed_providers.is_empty());
        assert_eq!(unused.consumed.provider_invocations, 0);
        let mut replay: DefinitionBundle =
            serde_json::from_value(serde_json::to_value(&bundle).unwrap()).unwrap();
        replay.sources.reverse();
        let fresh = compile(&replay, &source, registry);
        assert_eq!(image.program(), fresh.program());
        let context = ExecutionContext {
            bundle: &replay,
            conversions: Some(registry),
            references: &[],
        };
        let replayed = evaluate(
            &fresh,
            std::slice::from_ref(&argument),
            &context,
            &ExecutionLimits::default(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(good).unwrap(),
            serde_json::to_value(replayed).unwrap()
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn applied_nested_validation_records_real_child_calls_and_skips_empty_members() {
    let f = Fixture::new();
    for (annotation, index, text, expected) in [
        (
            "m.Seq<m.RGB>",
            0,
            "[]",
            vec![provider("intersect.sequence")],
        ),
        (
            "m.Seq<m.RGB>",
            0,
            "[\"#010203\"]",
            vec![provider("example.rgb"), provider("intersect.sequence")],
        ),
        (
            "m.Seq<m.Seq<m.RGB>>",
            1,
            "[\"[\\\"#010203\\\"]\",\"[\\\"#040506\\\"]\"]",
            vec![provider("example.rgb"), provider("intersect.sequence")],
        ),
    ] {
        let (bundle, source) = f.bundle(&format!(
            "fn run(x:{annotation})->{annotation} ={{let keep=|n:Int8|x;keep(0)}};"
        ));
        with_catalog_policies(&bundle, &f.ordinary, &f.applications, &[], |registry| {
            let image = compile(&bundle, &source, registry);
            let payload = serde_json::to_value(
                registry
                    .applied()
                    .parse(&f.applications[index].application, text)?,
            )
            .unwrap();
            let argument = input(&image, payload.clone());
            let context = ExecutionContext {
                bundle: &bundle,
                conversions: Some(registry),
                references: &[],
            };
            let result = evaluate(
                &image,
                std::slice::from_ref(&argument),
                &context,
                &ExecutionLimits::default(),
            )
            .unwrap();
            assert_eq!(result.value, argument);
            assert_eq!(result.consumed_providers, expected);
            assert!(result.consumed.provider_invocations >= result.consumed_providers.len());
            let mut forged = payload;
            forged["converted"]["provider"]["version"] = json!("forged");
            let error = evaluate(
                &image,
                &[input(&image, forged)],
                &context,
                &ExecutionLimits::default(),
            )
            .unwrap_err();
            assert!(error.consumed_providers.is_empty(), "{error:?}");
            assert_eq!(error.consumed.provider_invocations, 0);
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn provider_evidence_and_invocation_limits_have_exact_boundaries_and_recovery() {
    let f = Fixture::new();
    let (bundle, source) = f.bundle("fn run(x:m.RGB)->m.RGB=x;");
    with_catalog_policies(&bundle, &f.ordinary, &f.applications, &[], |registry| {
        let image = compile(&bundle, &source, registry);
        let argument = input(
            &image,
            serde_json::to_value(registry.parse(&f.id(0), "#010203")?).unwrap(),
        );
        let context = ExecutionContext {
            bundle: &bundle,
            conversions: Some(registry),
            references: &[],
        };
        let baseline = evaluate(
            &image,
            std::slice::from_ref(&argument),
            &context,
            &ExecutionLimits::default(),
        )
        .unwrap();
        let exact = ExecutionLimits {
            max_provider_evidence_bytes: baseline.consumed.provider_evidence_bytes,
            max_provider_invocations: baseline.consumed.provider_invocations,
            ..ExecutionLimits::default()
        };
        assert_eq!(
            serde_json::to_value(&baseline).unwrap(),
            serde_json::to_value(
                evaluate(&image, std::slice::from_ref(&argument), &context, &exact).unwrap()
            )
            .unwrap()
        );
        for limits in [
            ExecutionLimits {
                max_provider_evidence_bytes: exact.max_provider_evidence_bytes - 1,
                ..exact.clone()
            },
            ExecutionLimits {
                max_provider_invocations: exact.max_provider_invocations - 1,
                ..exact.clone()
            },
            ExecutionLimits {
                max_provider_evidence_bytes: 0,
                ..exact.clone()
            },
        ] {
            let error =
                evaluate(&image, std::slice::from_ref(&argument), &context, &limits).unwrap_err();
            assert_eq!(
                error.kind,
                intersect_model::rcore_behavior_functions::ExecutionErrorKind::ResourceExhausted
            );
            assert!(
                error.message.to_lowercase().contains("provider"),
                "{error:?}"
            );
            assert!(error.consumed.provider_invocations <= limits.max_provider_invocations);
            assert!(
                evidence_size(&error.selected_providers, &error.consumed_providers)
                    <= limits.max_provider_evidence_bytes
            );
            assert!(evaluate(&image, std::slice::from_ref(&argument), &context, &exact).is_ok());
        }
        let mut invalid = match argument.data.clone() {
            DataWire::Datatype(value) => value,
            _ => unreachable!(),
        };
        invalid["value"]["red"] = json!(300);
        let error = evaluate(
            &image,
            &[input(&image, invalid)],
            &context,
            &ExecutionLimits {
                max_trace_bytes: 0,
                ..exact
            },
        )
        .unwrap_err();
        assert!(error.selected_providers.is_empty());
        assert!(error.consumed_providers.is_empty());
        assert!(error.provider_evidence_truncated);
        assert!(error.trace_truncated);
        Ok(())
    })
    .unwrap();
}

#[test]
fn model_domain_construction_receipts_are_deduplicated_on_cached_reference_use() {
    let f = Fixture::new();
    let reference = f.reference();
    let (bundle, source) = f.bundle("fn run(a:m.Box,b:m.Box)->m.Box={let keep=|n:Int8|a;keep(0)};");
    with_catalog_policies(&bundle, &f.ordinary, &f.applications, &[], |registry| {
        let image = compile(&bundle, &source, registry);
        let payload = ReferenceDomain::new_runtime(
            reference.materialized_types[0].clone(),
            reference.clone(),
            registry,
        )?
        .bind("box")?;
        let ty = image.program().functions[0].parameters[0];
        let argument = ValueWire {
            ty,
            data: DataWire::Model(payload.clone()),
        };
        let refs = [reference.clone()];
        let context = ExecutionContext {
            bundle: &bundle,
            conversions: Some(registry),
            references: &refs,
        };
        let result = evaluate(
            &image,
            &[argument.clone(), argument.clone()],
            &context,
            &ExecutionLimits::default(),
        )
        .unwrap();
        assert_eq!(result.value, argument);
        assert_eq!(result.consumed_providers, vec![provider("example.rgb")]);
        assert_eq!(result.consumed.model_contexts, 1);
        assert_eq!(result.consumed.reference_domains, 1);
        let (single_bundle, single_source) = f.bundle("fn run(a:m.Box)->m.Box=a;");
        let single = compile(&single_bundle, &single_source, registry);
        let argument = ValueWire {
            ty: single.program().functions[0].parameters[0],
            data: DataWire::Model(payload),
        };
        let context = ExecutionContext {
            bundle: &single_bundle,
            conversions: Some(registry),
            references: &refs,
        };
        let once = evaluate(
            &single,
            std::slice::from_ref(&argument),
            &context,
            &ExecutionLimits::default(),
        )
        .unwrap();
        assert_eq!(
            once.consumed.provider_invocations,
            result.consumed.provider_invocations
        );
        let mut forged = argument;
        if let DataWire::Model(ref mut value) = forged.data {
            value["context"] = json!("foreign");
        }
        let error =
            evaluate(&single, &[forged], &context, &ExecutionLimits::default()).unwrap_err();
        assert!(error.consumed_providers.is_empty());
        assert_eq!(error.consumed.provider_invocations, 0);
        Ok(())
    })
    .unwrap();
}

#[test]
fn eager_uncalled_checked_image_constant_records_validation_before_source_execution() {
    let f = Fixture::new();
    let (bundle, source) =
        f.bundle("fn run()->Int8={let unused=helper;1};fn helper(x:m.Seq<m.RGB>)->Int8=2;");
    with_catalog_policies(&bundle,&f.ordinary,&f.applications,&[],|registry|{
 let source_image=compile(&bundle,&source,registry);let mut program=source_image.program().clone();let helper=program.functions.iter_mut().find(|function|matches!(&function.identity,ir_v2::FunctionIdentity::Named{symbol,..} if symbol.declaration==2)).unwrap();let ty=helper.parameters[0];let span=helper.span.clone();let payload=serde_json::to_value(registry.applied().parse(&f.applications[0].application,"[\"#010203\"]")?).unwrap();
 helper.blocks[0].instructions.push(ir_v2::Instruction{ty,kind:ir_v2::InstructionKind::Constant(ValueWire{ty,data:DataWire::Datatype(payload)}),span});
 // Modified image is independently safety-verified, not claimed to reproduce its source body.
 let image=ir_v2::verify_with_conversions(program,&bundle,registry,&Limits::default()).unwrap();let context=ExecutionContext{bundle:&bundle,conversions:Some(registry),references:&[]};let result=evaluate(&image,&[],&context,&ExecutionLimits::default()).unwrap();assert_eq!(result.consumed_providers,vec![provider("example.rgb"),provider("intersect.sequence")]);assert!(result.consumed.provider_invocations>=2);
 let result=evaluate(&image,&[],&context,&ExecutionLimits{scalar:intersect_model::rcore_behavior_execution::ExecutionLimits{max_steps:0,..Default::default()},..Default::default()}).unwrap_err();assert_eq!(result.consumed.steps,0);assert_eq!(result.consumed_providers,vec![provider("example.rgb"),provider("intersect.sequence")]);
 Ok(())}).unwrap();
}

#[test]
fn nested_reference_codec_receipts_do_not_invent_catalog_construction_consumption() {
    let f = Fixture::new();
    let reference = f.reference();
    let target = reference.materialized_types[0].clone();
    let application = AppliedConversionContract {
        application: ClosedType::new(Type::Classifier {
            declaration: f.id(3),
            arguments: vec![target.expression().clone()],
            raw: false,
        })
        .unwrap(),
        conversion: ConversionContract {
            datatype: f.id(3),
            provider: provider("ecore.list"),
            serializable: false,
            declarations: vec![],
            members: vec![MemberContract {
                nullable: false,
                value: MemberType::RuntimeReference {
                    target: target.clone(),
                    context: Box::new(reference.clone()),
                },
            }],
        },
    };
    let (bundle, source) =
        f.bundle("fn run(x:m.List<m.Box>)->m.List<m.Box> ={let keep=|n:Int8|x;keep(0)};");
    let mut applications = f.applications.clone();
    applications.push(application.clone());
    with_catalog_policies(&bundle, &f.ordinary, &applications, &[], |registry| {
        let image = compile(&bundle, &source, registry);
        let reference_payload =
            ReferenceDomain::new_runtime(target, reference.clone(), registry)?.bind("box")?;
        let payload = serde_json::to_value(registry.applied().bind(
            &application.application,
            json!({"items":[reference_payload]}),
        )?)
        .unwrap();
        let argument = input(&image, payload);
        let context = ExecutionContext {
            bundle: &bundle,
            conversions: Some(registry),
            references: &[],
        };
        let result = evaluate(
            &image,
            std::slice::from_ref(&argument),
            &context,
            &ExecutionLimits::default(),
        )
        .unwrap();
        assert_eq!(result.value, argument);
        // The installed list's reference domain was built before evaluation. Its cached
        // membership lookup invokes the list validator, not an RGB codec retroactively.
        assert_eq!(result.consumed_providers, vec![provider("ecore.list")]);
        assert!(result.selected_providers.contains(&provider("example.rgb")));
        assert!(
            result
                .selected_providers
                .contains(&provider("intersect.sequence"))
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn feature_entry_and_map_dispatch_observed_member_validation() {
    let mut fixture = Fixture::new();
    let insertion = fixture.model.text.rfind('}').unwrap();
    let mut text = fixture.model.text.clone();
    text.insert_str(insertion, r#"datatype Entry instanceClass "org.eclipse.emf.ecore.util.FeatureMap$Entry" nonserializable {} datatype Map instanceClass "org.eclipse.emf.ecore.util.FeatureMap" nonserializable {}"#);
    fixture.model =
        DefinitionSource::new("memory:feature-provenance.rcore", SourceKind::Rcore, text);
    // Rebuild policies from the new exact source identity; none of the old fixture
    // contracts may certify this changed source revision.
    let rgb = ConversionContract {
        datatype: fixture.id(0),
        provider: provider("example.rgb"),
        serializable: true,
        declarations: vec![],
        members: vec![],
    };
    let feature = intersect_model_acceptance::migration_fixture::named_declaration_fixture(
        &fixture.model,
        "Box/color",
    );
    for (name, index, codec) in [
        ("Entry", 4, "ecore.feature-entry"),
        ("Map", 5, "ecore.feature-map"),
    ] {
        let contract = ConversionContract {
            datatype: fixture.id(index),
            provider: provider(codec),
            serializable: false,
            declarations: vec![feature.clone()],
            members: vec![MemberContract {
                nullable: false,
                value: MemberType::Converted {
                    contract: Box::new(rgb.clone()),
                },
            }],
        };
        let contracts = vec![rgb.clone(), contract.clone()];
        let (bundle, source) = fixture.bundle(&format!("fn run(x:m.{name})->m.{name}=x;"));
        with_catalog_policies(&bundle, &contracts, &[], &[], |registry| {
            let image = compile(&bundle, &source, registry);
            let color = serde_json::to_value(registry.parse(&fixture.id(0), "#010203")?).unwrap();
            let entry = json!({"feature":feature,"value":color});
            let payload = if name == "Entry" {
                entry.clone()
            } else {
                json!({"entries":[entry.clone(),entry]})
            };
            let tagged = serde_json::to_value(registry.bind(&fixture.id(index), payload)?).unwrap();
            let context = ExecutionContext {
                bundle: &bundle,
                conversions: Some(registry),
                references: &[],
            };
            let argument = input(&image, tagged.clone());
            let result = evaluate(
                &image,
                std::slice::from_ref(&argument),
                &context,
                &ExecutionLimits::default(),
            )
            .unwrap();
            assert_eq!(result.value, argument);
            assert_eq!(
                result.consumed_providers,
                vec![provider(codec), provider("example.rgb")]
            );
            assert!(result.consumed.provider_invocations >= if name == "Entry" { 2 } else { 3 });
            let mut bad = tagged.clone();
            let nested = if name == "Entry" {
                &mut bad["value"]["value"]
            } else {
                &mut bad["value"]["entries"][0]["value"]
            };
            nested["value"]["red"] = json!(300);
            let failure = evaluate(
                &image,
                &[input(&image, bad)],
                &context,
                &ExecutionLimits::default(),
            )
            .unwrap_err();
            assert_eq!(
                failure.consumed_providers,
                vec![provider(codec), provider("example.rgb")]
            );
            let mut forged = tagged;
            let nested = if name == "Entry" {
                &mut forged["value"]["value"]
            } else {
                &mut forged["value"]["entries"][0]["value"]
            };
            nested["provider"]["version"] = json!("forged");
            let failure = evaluate(
                &image,
                &[input(&image, forged)],
                &context,
                &ExecutionLimits::default(),
            )
            .unwrap_err();
            assert_eq!(failure.consumed_providers, vec![provider(codec)]);
            if name == "Map" {
                let empty =
                    serde_json::to_value(registry.bind(&fixture.id(index), json!({"entries":[]}))?)
                        .unwrap();
                let result = evaluate(
                    &image,
                    &[input(&image, empty)],
                    &context,
                    &ExecutionLimits::default(),
                )
                .unwrap();
                assert_eq!(result.consumed_providers, vec![provider(codec)]);
            }
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn long_provider_identity_is_bounded_before_selection_copy_or_dispatch() {
    use intersect_model::definitions::{
        compiler::import_datatype_bundle, conversion::ConversionProvider,
    };
    use std::{cell::Cell, rc::Rc};
    struct Probe {
        contract: ConversionContract,
        calls: Rc<Cell<usize>>,
    }
    impl ConversionProvider for Probe {
        fn contract(&self) -> &ConversionContract {
            &self.contract
        }
        fn parse(&self, _: &str) -> Result<Value, String> {
            Ok(json!({"red":1,"green":2,"blue":3}))
        }
        fn format(&self, value: &Value) -> Result<String, String> {
            Ok(value.to_string())
        }
        fn validate(&self, value: &Value) -> Result<(), String> {
            self.calls.set(self.calls.get() + 1);
            if value == &json!({"red":1,"green":2,"blue":3}) {
                Ok(())
            } else {
                Err("Probe payload rejected".into())
            }
        }
        fn default_value(&self) -> Result<Option<Value>, String> {
            Ok(None)
        }
    }
    let fixture = Fixture::new();
    let identity = ProviderId {
        name: format!("probe.{}", "long\"\\identity".repeat(128)),
        version: format!("1.{}", "revision".repeat(64)),
    };
    let calls = Rc::new(Cell::new(0));
    let implementation = Probe {
        contract: ConversionContract {
            datatype: fixture.id(0),
            provider: identity.clone(),
            serializable: true,
            declarations: vec![],
            members: vec![],
        },
        calls: calls.clone(),
    };
    let (bundle, source) = fixture.bundle("fn run(x:m.RGB)->m.RGB=x;");
    let datatype = import_datatype_bundle(&bundle, &fixture.id(0)).unwrap();
    let mut registry = ConversionRegistry::default();
    registry.register(&datatype, &implementation).unwrap();
    let image = compile(&bundle, &source, &registry);
    let payload = serde_json::to_value(registry.parse(&fixture.id(0), "ignored").unwrap()).unwrap();
    let argument = input(&image, payload);
    let context = ExecutionContext {
        bundle: &bundle,
        conversions: Some(&registry),
        references: &[],
    };
    let one = serde_json::to_vec(&identity).unwrap().len();
    for cap in [0, one - 1] {
        calls.set(0);
        let error = evaluate(
            &image,
            std::slice::from_ref(&argument),
            &context,
            &ExecutionLimits {
                max_provider_evidence_bytes: cap,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(
            error.kind,
            intersect_model::rcore_behavior_functions::ExecutionErrorKind::ResourceExhausted
        );
        assert!(error.selected_providers.is_empty());
        assert!(error.consumed_providers.is_empty());
        assert!(error.provider_evidence_truncated);
        assert_eq!(error.consumed.steps, 0);
        assert_eq!(error.consumed.provider_invocations, 0);
        assert_eq!(calls.get(), 0);
    }
    calls.set(0);
    let blocked = evaluate(
        &image,
        std::slice::from_ref(&argument),
        &context,
        &ExecutionLimits {
            max_provider_evidence_bytes: 2 * one - 1,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(blocked.selected_providers, vec![identity.clone()]);
    assert!(blocked.consumed_providers.is_empty());
    assert_eq!(calls.get(), 0);
    let exact = ExecutionLimits {
        max_provider_evidence_bytes: 2 * one,
        ..Default::default()
    };
    let result = evaluate(&image, std::slice::from_ref(&argument), &context, &exact).unwrap();
    assert_eq!(result.selected_providers, vec![identity.clone()]);
    assert_eq!(result.consumed_providers, vec![identity.clone()]);
    assert_eq!(result.consumed.provider_evidence_bytes, 2 * one);
    assert_eq!(result.consumed.provider_invocations, calls.get());
    assert!(calls.get() > 0);
    // A context preflight failure must still preserve the already selected exact
    // identity, without claiming that a semantic provider callback ran.
    calls.set(0);
    let error = evaluate(
        &image,
        std::slice::from_ref(&argument),
        &context,
        &ExecutionLimits {
            max_context_bytes: 0,
            ..exact
        },
    )
    .unwrap_err();
    assert_eq!(error.selected_providers, vec![identity]);
    assert!(error.consumed_providers.is_empty());
    assert_eq!(error.consumed.provider_invocations, 0);
    assert_eq!(calls.get(), 0);
}

#[test]
fn owned_catalog_survives_selection_inputs_and_original_registry_with_exact_execution() {
    let fixture = Fixture::new();
    let (bundle, source) =
        fixture.bundle("fn run(x:m.Seq<m.RGB>)->m.Seq<m.RGB> ={let keep=|n:Int8|x;keep(0)};");
    let application = fixture.applications[0].application.clone();
    let constructor = intersect_model::definitions::conversion::ConstructorContract {
        datatype: intersect_model::definitions::compiler::import_datatype_bundle(
            &bundle,
            &fixture.id(3),
        )
        .unwrap(),
        provider: provider("ecore.list.constructor"),
    };
    let registry: ConversionRegistry<'static> =
        intersect_model::conversion::build_catalog_policies(
            &bundle,
            &fixture.ordinary,
            &fixture.applications,
            std::slice::from_ref(&constructor),
        )
        .unwrap();
    let image = compile(&bundle, &source, &registry);
    let argument = input(
        &image,
        serde_json::to_value(
            registry
                .applied()
                .parse(&application, "[\"#010203\"]")
                .unwrap(),
        )
        .unwrap(),
    );
    let execute = |registry: &ConversionRegistry<'_>| {
        evaluate(
            &image,
            std::slice::from_ref(&argument),
            &ExecutionContext {
                bundle: &bundle,
                conversions: Some(registry),
                references: &[],
            },
            &ExecutionLimits::default(),
        )
        .unwrap()
    };
    let expected = execute(&registry);
    for quantum in [1, 3, 17] {
        let context = ExecutionContext {
            bundle: &bundle,
            conversions: Some(&registry),
            references: &[],
        };
        let limits = ExecutionLimits::default();
        let mut session = intersect_model::rcore_behavior_functions::EvaluationSession::new(
            &image,
            std::slice::from_ref(&argument),
            &context,
            &limits,
        )
        .unwrap();
        let mut pending = 0;
        loop {
            use intersect_model::rcore_behavior_functions::SessionProgress;
            match session.resume(std::num::NonZeroUsize::new(quantum).unwrap()) {
                SessionProgress::Pending => pending += 1,
                SessionProgress::Complete(outcome) => {
                    assert_eq!(
                        serde_json::to_value((*outcome).unwrap()).unwrap(),
                        serde_json::to_value(&expected).unwrap()
                    );
                    break;
                }
                SessionProgress::Finished => panic!("missing session result"),
            }
        }
        if quantum == 1 {
            assert!(pending > 10);
        }
        assert!(matches!(
            session.resume(std::num::NonZeroUsize::MAX),
            intersect_model::rcore_behavior_functions::SessionProgress::Finished
        ));
    }
    assert_eq!(expected.value, argument);
    assert_eq!(
        expected.consumed_providers,
        vec![provider("example.rgb"), provider("intersect.sequence")]
    );
    let owned_image = compile(&bundle, &source, &registry);
    let mut owned_session =
        intersect_model::rcore_behavior_functions::EvaluationSession::new_owned(
            owned_image,
            vec![argument.clone()],
            bundle.clone(),
            vec![],
            Some(registry.clone()),
            ExecutionLimits::default(),
        )
        .unwrap();
    assert!(matches!(
        owned_session.resume(std::num::NonZeroUsize::MIN),
        intersect_model::rcore_behavior_functions::SessionProgress::Pending
    ));
    let retained = registry.clone();
    drop(registry);
    drop(fixture);
    loop {
        use intersect_model::rcore_behavior_functions::SessionProgress;
        match owned_session.resume(std::num::NonZeroUsize::new(3).unwrap()) {
            SessionProgress::Pending => {}
            SessionProgress::Complete(outcome) => {
                assert_eq!(
                    serde_json::to_value((*outcome).unwrap()).unwrap(),
                    serde_json::to_value(&expected).unwrap()
                );
                break;
            }
            SessionProgress::Finished => panic!("owned session lost its result"),
        }
    }
    assert_eq!(
        retained.constructors().registered_contracts().unwrap(),
        vec![constructor]
    );
    assert_eq!(
        serde_json::to_value(execute(&retained)).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    let mut forged = argument.clone();
    if let DataWire::Datatype(value) = &mut forged.data {
        value["converted"]["provider"]["version"] = json!("stale");
    } else {
        panic!("datatype");
    }
    assert!(
        evaluate(
            &image,
            &[forged],
            &ExecutionContext {
                bundle: &bundle,
                conversions: Some(&retained),
                references: &[]
            },
            &ExecutionLimits::default()
        )
        .is_err()
    );
    assert_eq!(execute(&retained).value, argument);
}

#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn cancellation_preserves_selected_and_consumed_provider_evidence_at_every_checkpoint() {
    use intersect_model::rcore_behavior_functions::{
        EvaluationSession, ExecutionControl, ExecutionErrorKind, SessionProgress,
    };
    use std::{cell::Cell, num::NonZeroUsize};
    struct Control {
        polls: Cell<usize>,
        stop: usize,
    }
    impl ExecutionControl for Control {
        fn cancellation_requested(&self) -> bool {
            self.polls.set(self.polls.get() + 1);
            self.polls.get() >= self.stop
        }
    }
    let f = Fixture::new();
    for applied in [false, true] {
        let annotation = if applied { "m.Seq<m.RGB>" } else { "m.RGB" };
        let (bundle, source) = f.bundle(&format!("fn run(x:{annotation})->{annotation} = x;"));
        with_catalog_policies(&bundle, &f.ordinary, &f.applications, &[], |registry| {
            let image = compile(&bundle, &source, registry);
            let payload = if applied {
                serde_json::to_value(
                    registry
                        .applied()
                        .parse(&f.applications[0].application, "[\"#010203\"]")?,
                )
                .unwrap()
            } else {
                serde_json::to_value(registry.parse(&f.id(0), "#010203")?).unwrap()
            };
            let args = [input(&image, payload)];
            let context = ExecutionContext {
                bundle: &bundle,
                conversions: Some(registry),
                references: &[],
            };
            let limits = ExecutionLimits::default();
            let counter = Control {
                polls: Cell::new(0),
                stop: usize::MAX,
            };
            let mut baseline = EvaluationSession::new(&image, &args, &context, &limits).unwrap();
            let expected = match baseline.resume_with_control(NonZeroUsize::MAX, &counter) {
                SessionProgress::Complete(result) => result.unwrap(),
                _ => panic!("small fixture must finish"),
            };
            let mut before_provider = false;
            let mut after_provider = false;
            let mut between_parent_and_child = false;
            for stop in 1..=counter.polls.get() {
                let mut failures = Vec::new();
                for quantum in [NonZeroUsize::MIN, NonZeroUsize::MAX] {
                    let control = Control {
                        polls: Cell::new(0),
                        stop,
                    };
                    let mut session =
                        EvaluationSession::new(&image, &args, &context, &limits).unwrap();
                    let error = loop {
                        match session.resume_with_control(quantum, &control) {
                            SessionProgress::Pending => {}
                            SessionProgress::Complete(result) => break result.unwrap_err(),
                            SessionProgress::Finished => panic!("missing terminal result"),
                        }
                    };
                    assert_eq!(error.kind, ExecutionErrorKind::Cancelled);
                    assert_eq!(control.polls.get(), stop);
                    assert_eq!(error.selected_providers, expected.selected_providers);
                    if error.consumed.provider_invocations == 0 {
                        before_provider = true;
                        assert!(error.consumed_providers.is_empty());
                    } else if applied && error.consumed.provider_invocations == 1 {
                        between_parent_and_child = true;
                        assert_eq!(
                            error.consumed_providers,
                            vec![provider("intersect.sequence")]
                        );
                    } else {
                        after_provider = true;
                        assert_eq!(error.consumed_providers, expected.consumed_providers);
                    }
                    failures.push(serde_json::to_value(error).unwrap());
                }
                assert_eq!(failures[0], failures[1]);
            }
            assert!(before_provider && after_provider);
            assert_eq!(between_parent_and_child, applied);
            assert_eq!(
                serde_json::to_value(evaluate(&image, &args, &context, &limits).unwrap()).unwrap(),
                serde_json::to_value(expected).unwrap()
            );
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn recursive_nominal_captures_preserve_provider_receipts_and_aggregate_limits() {
    use intersect_model::definitions::rcore_behavior::{ir, types::Scalar};
    use intersect_model::rcore_behavior_functions::ExecutionErrorKind;
    let f = Fixture::new();
    let reference = f.reference();
    for annotation in ["m.RGB", "m.Seq<m.RGB>", "m.Box"] {
        let (bundle,source)=f.bundle(&format!("fn run(x:{annotation},n:Int8)->{annotation} ={{let keep=|unused:Int8|bounce(n,x);keep(0)}};fn bounce<T>(n:Int8,x:T)->T=if n==0 {{x}} else {{bounce::<T>(n-1,x)}};"));
        with_catalog_policies(&bundle, &f.ordinary, &f.applications, &[], |registry| {
            let image = compile(&bundle, &source, registry);
            let parameters =
                &image.program().functions[image.program().entry.0 as usize].parameters;
            let data = match annotation {
                "m.RGB" => DataWire::Datatype(
                    serde_json::to_value(registry.parse(&f.id(0), "#010203")?).unwrap(),
                ),
                "m.Seq<m.RGB>" => DataWire::Datatype(
                    serde_json::to_value(
                        registry
                            .applied()
                            .parse(&f.applications[0].application, "[\"#010203\"]")?,
                    )
                    .unwrap(),
                ),
                _ => DataWire::Model(
                    ReferenceDomain::new_runtime(
                        reference.materialized_types[0].clone(),
                        reference.clone(),
                        registry,
                    )?
                    .bind("box")?,
                ),
            };
            let value = ValueWire {
                ty: parameters[0],
                data,
            };
            let refs = [reference.clone()];
            let context = ExecutionContext {
                bundle: &bundle,
                conversions: Some(registry),
                references: &refs,
            };
            let invoke = |n, limits: &ExecutionLimits| {
                evaluate(
                    &image,
                    &[
                        value.clone(),
                        ValueWire {
                            ty: parameters[1],
                            data: DataWire::Scalar(ir::Value {
                                ty: ir::ValueType::Scalar(Scalar::Int8),
                                payload: json!(n),
                            }),
                        },
                    ],
                    &context,
                    limits,
                )
            };
            let base = invoke(0, &ExecutionLimits::default()).unwrap();
            let recursive = invoke(4, &ExecutionLimits::default()).unwrap();
            assert_eq!(recursive.value, value);
            assert_eq!(recursive.value, base.value);
            assert_eq!(recursive.selected_providers, base.selected_providers);
            assert_eq!(recursive.consumed_providers, base.consumed_providers);
            assert_eq!(
                recursive.consumed.provider_invocations,
                base.consumed.provider_invocations
            );
            assert!(recursive.consumed.steps > base.consumed.steps);
            let mut limits = ExecutionLimits {
                max_provider_invocations: base.consumed.provider_invocations,
                ..Default::default()
            };
            assert!(invoke(4, &limits).is_ok());
            assert!(limits.max_provider_invocations > 0);
            limits.max_provider_invocations -= 1;
            assert_eq!(
                invoke(4, &limits).unwrap_err().kind,
                ExecutionErrorKind::ResourceExhausted
            );
            assert_eq!(invoke(4, &ExecutionLimits::default()).unwrap().value, value);
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn absent_exact_provider_policies_have_typed_causes_without_invocation() {
    use intersect_model::definitions::rcore_behavior::ir_v2::ContextDependency;
    use intersect_model::rcore_behavior_functions::{DependencyCause, ExecutionErrorKind};
    let f = Fixture::new();
    let (bundle, source) = f.bundle("fn run()->Int8=42;");
    with_catalog_policies(&bundle, &f.ordinary, &f.applications, &[], |registry| {
        let image = compile(&bundle, &source, registry);
        let error = evaluate(
            &image,
            &[],
            &ExecutionContext::new(&bundle),
            &ExecutionLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error.kind, ExecutionErrorKind::MissingDependency);
        assert_eq!(
            error.dependency.as_deref(),
            Some(&DependencyCause::ExecutableContext(
                ContextDependency::ConversionPolicy { manifest_index: 0 }
            ))
        );
        assert_eq!(error.consumed.provider_invocations, 0);
        assert!(error.consumed_providers.is_empty());
        assert!(!error.selected_providers.is_empty());
        with_catalog_policies(&bundle, &f.ordinary, &[], &[], |ordinary| {
            let context = ExecutionContext {
                bundle: &bundle,
                conversions: Some(ordinary),
                references: &[],
            };
            let error = evaluate(&image, &[], &context, &ExecutionLimits::default()).unwrap_err();
            assert_eq!(error.kind, ExecutionErrorKind::MissingDependency);
            assert_eq!(
                error.dependency.as_deref(),
                Some(&DependencyCause::ExecutableContext(
                    ContextDependency::AppliedConversionPolicy { manifest_index: 0 }
                ))
            );
            assert_eq!(error.consumed.provider_invocations, 0);
            Ok(())
        })?;
        let context = ExecutionContext {
            bundle: &bundle,
            conversions: Some(registry),
            references: &[],
        };
        assert!(evaluate(&image, &[], &context, &ExecutionLimits::default()).is_ok());
        Ok(())
    })
    .unwrap();
}

#[test]
fn absent_constructor_policy_is_a_typed_dependency_even_for_an_unused_catalog_entry() {
    use intersect_model::definitions::rcore_behavior::ir_v2::ContextDependency;

    use intersect_model::{
        conversion::build_catalog_policies,
        rcore_behavior_functions::{DependencyCause, ExecutionErrorKind},
    };
    let f = Fixture::new();
    let (bundle, source) = f.bundle("fn run()->Int8=42;");
    let constructor = intersect_model::definitions::conversion::ConstructorContract {
        datatype: intersect_model::definitions::compiler::import_datatype_bundle(&bundle, &f.id(3))
            .unwrap(),
        provider: provider("ecore.list.constructor"),
    };
    let complete = build_catalog_policies(
        &bundle,
        &f.ordinary,
        &f.applications,
        std::slice::from_ref(&constructor),
    )
    .unwrap();
    let missing = build_catalog_policies(&bundle, &f.ordinary, &f.applications, &[]).unwrap();
    let image = compile(&bundle, &source, &complete);
    let context = ExecutionContext {
        bundle: &bundle,
        conversions: Some(&missing),
        references: &[],
    };
    let error = evaluate(&image, &[], &context, &ExecutionLimits::default()).unwrap_err();
    assert_eq!(error.kind, ExecutionErrorKind::MissingDependency);
    assert_eq!(
        error.dependency.as_deref(),
        Some(&DependencyCause::ExecutableContext(
            ContextDependency::ConstructorPolicy { manifest_index: 0 }
        ))
    );
    assert_eq!(image.program().policies.constructors[0], constructor);
    assert_eq!(error.consumed.steps, 0);
    assert_eq!(error.consumed.provider_invocations, 0);
    let context = ExecutionContext {
        bundle: &bundle,
        conversions: Some(&complete),
        references: &[],
    };
    assert!(evaluate(&image, &[], &context, &ExecutionLimits::default()).is_ok());
}

#[test]
fn nested_lookup_failures_are_typed_terminal_and_not_inferred_from_messages() {
    use intersect_model::definitions::{
        compiler::import_datatype_bundle,
        conversion::{ConversionProvider, InvocationContext, ProviderDependencyKind as K},
        datatypes::Datatype,
    };
    use intersect_model::rcore_behavior_functions::{DependencyCause, ExecutionErrorKind};
    struct Nested {
        contract: ConversionContract,
        datatype: Datatype,
        application: ClosedType,
        kind: Option<K>,
        swallow: bool,
    }
    impl ConversionProvider for Nested {
        fn contract(&self) -> &ConversionContract {
            &self.contract
        }
        fn parse(&self, _: &str) -> Result<Value, String> {
            Err("unused".into())
        }
        fn format(&self, _: &Value) -> Result<String, String> {
            Err("unused".into())
        }
        fn validate(&self, _: &Value) -> Result<(), String> {
            Ok(())
        }
        fn default_value(&self) -> Result<Option<Value>, String> {
            Ok(None)
        }
        fn validate_observed(
            &self,
            _: &Value,
            observation: &InvocationContext,
        ) -> Result<(), String> {
            let registry = ConversionRegistry::default().with_observation(observation.clone());
            let result = match self.kind {
                Some(K::Conversion) => registry.require(&self.datatype).map(|_| ()),
                Some(K::AppliedConversion) => {
                    registry.applied().require(&self.application).map(|_| ())
                }
                Some(K::Constructor) => registry
                    .constructors()
                    .constructor_relation(self.datatype.declaration(), self.datatype.declaration())
                    .map(|_| ()),
                None => Err("No conversion provider for this exact datatype revision".into()),
            };
            if self.swallow { Ok(()) } else { result }
        }
    }
    let f = Fixture::new();
    let (bundle, source) = f.bundle("fn run(x:m.RGB)->m.RGB=x;");
    let payload = with_catalog_policies(&bundle, &f.ordinary, &[], &[], |registry| {
        serde_json::to_value(registry.parse(&f.id(0), "#010203")?).map_err(|e| e.to_string())
    })
    .unwrap();
    for kind in [
        Some(K::Conversion),
        Some(K::AppliedConversion),
        Some(K::Constructor),
        None,
    ] {
        for swallow in [false, true] {
            let nested = Nested {
                contract: f.ordinary[0].clone(),
                datatype: import_datatype_bundle(&bundle, &f.id(0)).unwrap(),
                application: f.applications[0].application.clone(),
                kind,
                swallow,
            };
            let mut registry = ConversionRegistry::default();
            registry.register(&nested.datatype, &nested).unwrap();
            let image = compile(&bundle, &source, &registry);
            let argument = input(&image, payload.clone());
            let context = ExecutionContext {
                bundle: &bundle,
                conversions: Some(&registry),
                references: &[],
            };
            let result = evaluate(
                &image,
                std::slice::from_ref(&argument),
                &context,
                &ExecutionLimits::default(),
            );
            if let Some(kind) = kind {
                let error = result.unwrap_err();
                assert_eq!(error.kind, ExecutionErrorKind::MissingDependency);
                assert_eq!(
                    error.dependency.as_deref(),
                    Some(&DependencyCause::Provider(kind))
                );
                assert_eq!(error.consumed.provider_invocations, 1);
                assert_eq!(
                    error.consumed_providers,
                    vec![nested.contract.provider.clone()]
                );
            } else if swallow {
                assert!(
                    result.is_ok(),
                    "an ordinary rejection may be handled by provider code"
                );
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.kind, ExecutionErrorKind::Nominal);
                assert!(error.dependency.is_none());
            }
            // An earlier resource limit wins and never enters the provider.
            let limits = ExecutionLimits {
                max_provider_invocations: 0,
                ..ExecutionLimits::default()
            };
            let error = evaluate(&image, &[argument], &context, &limits).unwrap_err();
            assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
            assert!(error.dependency.is_none());
            assert_eq!(error.consumed.provider_invocations, 0);
        }
    }
}

#[test]
#[cfg_attr(not(feature = "milestone-tests"), ignore = "milestone")]
fn scoped_control_cancels_inside_installed_nested_sequence_validation() {
    use intersect_model::rcore_behavior_functions::{
        EvaluationSession, ExecutionControl, ExecutionErrorKind, SessionProgress,
    };
    use std::{cell::Cell, num::NonZeroUsize};
    struct Control {
        polls: Cell<usize>,
        stop: usize,
    }
    impl ExecutionControl for Control {
        fn cancellation_requested(&self) -> bool {
            self.polls.set(self.polls.get() + 1);
            self.polls.get() == self.stop
        }
    }
    let f = Fixture::new();
    let (bundle, source) = f.bundle("fn run(x:m.Seq<m.RGB>)->m.Seq<m.RGB> = x;");
    with_catalog_policies(&bundle, &f.ordinary, &f.applications, &[], |registry| {
        let image = compile(&bundle, &source, registry);
        let text = serde_json::to_string(&vec!["#010203"; 16]).unwrap();
        let argument = input(
            &image,
            serde_json::to_value(
                registry
                    .applied()
                    .parse(&f.applications[0].application, &text)?,
            )
            .unwrap(),
        );
        let context = ExecutionContext {
            bundle: &bundle,
            conversions: Some(registry),
            references: &[],
        };
        let limits = ExecutionLimits::default();
        let run = |stop, quantum| {
            let control = Control {
                polls: Cell::new(0),
                stop,
            };
            let mut session =
                EvaluationSession::new(&image, std::slice::from_ref(&argument), &context, &limits)
                    .unwrap();
            loop {
                match session.resume_with_control(NonZeroUsize::new(quantum).unwrap(), &control) {
                    SessionProgress::Pending => {}
                    SessionProgress::Complete(result) => {
                        assert!(matches!(
                            session.resume(NonZeroUsize::MIN),
                            SessionProgress::Finished
                        ));
                        break (*result, control.polls.get());
                    }
                    SessionProgress::Finished => panic!("missing terminal outcome"),
                }
            }
        };
        let (baseline, polls) = run(usize::MAX, 1);
        let baseline = baseline.unwrap();
        assert_eq!(
            serde_json::to_value(&baseline).unwrap(),
            serde_json::to_value(
                evaluate(&image, std::slice::from_ref(&argument), &context, &limits).unwrap()
            )
            .unwrap()
        );
        let mut inner = 0;
        for stop in 1..=polls {
            let mut expected = None;
            for quantum in [1, 7] {
                let (result, seen) = run(stop, quantum);
                let error = result.unwrap_err();
                assert_eq!(seen, stop);
                assert_eq!(error.kind, ExecutionErrorKind::Cancelled);
                assert!(error.dependency.is_none());
                assert!(
                    error.consumed.provider_invocations <= baseline.consumed.provider_invocations
                );
                if error.message.contains("provider work")
                    && error.consumed.provider_invocations > 1
                    && error.consumed.provider_invocations < 17
                {
                    inner += 1;
                }
                let wire = serde_json::to_value(error).unwrap();
                if let Some(expected) = &expected {
                    assert_eq!(&wire, expected);
                } else {
                    expected = Some(wire);
                }
            }
        }
        assert!(
            inner >= 16,
            "cancellation must reach partially validated sequence members"
        );
        assert_eq!(
            serde_json::to_value(run(usize::MAX, 7).0.unwrap()).unwrap(),
            serde_json::to_value(baseline).unwrap()
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn provider_cannot_swallow_scoped_cancellation() {
    use intersect_model::definitions::{
        compiler::import_datatype_bundle,
        conversion::{ConversionProvider, InvocationContext},
    };
    use intersect_model::rcore_behavior_functions::{
        EvaluationSession, ExecutionControl, ExecutionErrorKind, SessionProgress,
    };
    use std::{cell::Cell, num::NonZeroUsize};
    struct Entered {
        flag: Cell<bool>,
    }
    impl ExecutionControl for Entered {
        fn cancellation_requested(&self) -> bool {
            self.flag.get()
        }
    }
    struct Catching<'a> {
        contract: ConversionContract,
        entered: &'a Entered,
        swallow: bool,
    }
    impl ConversionProvider for Catching<'_> {
        fn contract(&self) -> &ConversionContract {
            &self.contract
        }
        fn parse(&self, _: &str) -> Result<Value, String> {
            Err("unused".into())
        }
        fn format(&self, _: &Value) -> Result<String, String> {
            Err("unused".into())
        }
        fn default_value(&self) -> Result<Option<Value>, String> {
            Ok(None)
        }
        fn validate(&self, _: &Value) -> Result<(), String> {
            Ok(())
        }
        fn validate_observed(
            &self,
            _: &Value,
            observation: &InvocationContext,
        ) -> Result<(), String> {
            self.entered.flag.set(true);
            let result = observation.checkpoint();
            self.entered.flag.set(false); // A transient request must remain terminal.
            if self.swallow { Ok(()) } else { result }
        }
    }
    let f = Fixture::new();
    let (bundle, source) = f.bundle("fn run(x:m.RGB)->m.RGB=x;");
    let payload = with_catalog_policies(&bundle, &f.ordinary, &[], &[], |registry| {
        serde_json::to_value(registry.parse(&f.id(0), "#010203")?).map_err(|e| e.to_string())
    })
    .unwrap();
    for swallow in [false, true] {
        let entered = Entered {
            flag: Cell::new(false),
        };
        let provider = Catching {
            contract: f.ordinary[0].clone(),
            entered: &entered,
            swallow,
        };
        let datatype = import_datatype_bundle(&bundle, &f.id(0)).unwrap();
        let mut registry = ConversionRegistry::default();
        registry.register(&datatype, &provider).unwrap();
        let image = compile(&bundle, &source, &registry);
        let args = [input(&image, payload.clone())];
        let context = ExecutionContext {
            bundle: &bundle,
            conversions: Some(&registry),
            references: &[],
        };
        let limits = ExecutionLimits::default();
        let mut session = EvaluationSession::new(&image, &args, &context, &limits).unwrap();
        let failure = loop {
            match session.resume_with_control(NonZeroUsize::MAX, &entered) {
                SessionProgress::Pending => {}
                SessionProgress::Complete(result) => break result.unwrap_err(),
                SessionProgress::Finished => panic!("missing cancellation"),
            }
        };
        assert_eq!(failure.kind, ExecutionErrorKind::Cancelled);
        assert_eq!(failure.consumed.provider_invocations, 1);
        assert_eq!(
            failure.consumed_providers,
            vec![provider.contract.provider.clone()]
        );
        assert!(failure.dependency.is_none());
        assert!(!entered.flag.get());
        assert!(matches!(
            session.resume(NonZeroUsize::MIN),
            SessionProgress::Finished
        ));
        assert!(evaluate(&image, &args, &context, &limits).is_ok());
    }
}

#[test]
fn provider_work_is_aggregate_and_canonical_checks_do_not_invent_invocations() {
    use intersect_model::definitions::{
        compiler::import_datatype_bundle,
        conversion::{ConversionProvider, InvocationContext},
    };
    use intersect_model::rcore_behavior_functions::ExecutionErrorKind;
    struct Work {
        contract: ConversionContract,
        swallow: u8,
    }
    impl ConversionProvider for Work {
        fn contract(&self) -> &ConversionContract {
            &self.contract
        }
        fn parse(&self, _: &str) -> Result<Value, String> {
            Err("unused".into())
        }
        fn format(&self, _: &Value) -> Result<String, String> {
            Err("unused".into())
        }
        fn default_value(&self) -> Result<Option<Value>, String> {
            Ok(None)
        }
        fn validate(&self, _: &Value) -> Result<(), String> {
            Ok(())
        }
        fn validate_observed(
            &self,
            _: &Value,
            observation: &InvocationContext,
        ) -> Result<(), String> {
            let result = (0..8).try_for_each(|_| observation.work(1));
            match self.swallow {
                0 => result,
                1 => Ok(()),
                _ => result.map_err(|_| "provider replacement error".into()),
            }
        }
    }
    let f = Fixture::new();
    let (bundle, source) = f.bundle("fn run(x:m.RGB)->m.RGB={let keep=|n:Int8|bounce(x,n);keep(2)};fn bounce(x:m.RGB,n:Int8)->m.RGB=if n==0 {x} else {bounce(x,n-1)};");
    let payload = with_catalog_policies(&bundle, &f.ordinary, &[], &[], |registry| {
        serde_json::to_value(registry.parse(&f.id(0), "#010203")?).map_err(|e| e.to_string())
    })
    .unwrap();
    let mut failures = std::collections::BTreeMap::new();
    for swallow in [0, 1, 2] {
        let provider = Work {
            contract: f.ordinary[0].clone(),
            swallow,
        };
        let datatype = import_datatype_bundle(&bundle, &f.id(0)).unwrap();
        let mut registry = ConversionRegistry::default();
        registry.register(&datatype, &provider).unwrap();
        let image = compile(&bundle, &source, &registry);
        let args = [input(&image, payload.clone())];
        let context = ExecutionContext {
            bundle: &bundle,
            conversions: Some(&registry),
            references: &[],
        };
        let baseline = evaluate(&image, &args, &context, &ExecutionLimits::default()).unwrap();
        // One exact ConvertedValue RGB wire comparison: 12 value/container
        // visits and six RGB map-key/value visits. This admission work does
        // not create a semantic provider invocation receipt.
        let canonical_visits = 18;
        assert_eq!(
            baseline.consumed.provider_work,
            baseline.consumed.provider_invocations * 9 + canonical_visits
        );
        for cap in [
            0,
            3,
            canonical_visits + 3,
            baseline.consumed.provider_work - 1,
        ] {
            let limits = ExecutionLimits {
                max_provider_work: cap,
                ..ExecutionLimits::default()
            };
            let error = evaluate(&image, &args, &context, &limits).unwrap_err();
            assert_eq!(error.kind, ExecutionErrorKind::ResourceExhausted);
            assert_eq!(&*error.message, "Provider work limit exceeded");
            let wire = serde_json::to_value(&error).unwrap();
            if let Some(expected) = failures.get(&cap) {
                assert_eq!(&wire, expected, "provider swallow mode {swallow}");
            } else {
                failures.insert(cap, wire);
            }
            assert_eq!(error.consumed.provider_work, cap);
            assert!(error.dependency.is_none());
            if cap == 3 {
                assert_eq!(error.consumed.provider_invocations, 0);
                assert!(error.consumed_providers.is_empty());
            }
            if cap == canonical_visits + 3 {
                assert_eq!(error.consumed.provider_invocations, 1);
            }
        }
        let exact = ExecutionLimits {
            max_provider_work: baseline.consumed.provider_work,
            ..ExecutionLimits::default()
        };
        assert_eq!(
            serde_json::to_value(evaluate(&image, &args, &context, &exact).unwrap()).unwrap(),
            serde_json::to_value(baseline).unwrap()
        );
        let (unused_bundle, unused_source) = f.bundle("fn run()->Int8=7;");
        let unused = compile(&unused_bundle, &unused_source, &registry);
        let value = evaluate(
            &unused,
            &[],
            &ExecutionContext {
                bundle: &unused_bundle,
                conversions: Some(&registry),
                references: &[],
            },
            &ExecutionLimits {
                max_provider_work: 0,
                ..ExecutionLimits::default()
            },
        )
        .unwrap();
        assert_eq!(value.consumed.provider_work, 0);
        assert_eq!(value.consumed.provider_invocations, 0);
        assert!(!value.selected_providers.is_empty());
    }
    let defaults = ExecutionLimits::default();
    let mut legacy = serde_json::to_value(&defaults).unwrap();
    legacy.as_object_mut().unwrap().remove("max_provider_work");
    assert_eq!(
        serde_json::from_value::<ExecutionLimits>(legacy)
            .unwrap()
            .max_provider_work,
        defaults.max_provider_work
    );
}
