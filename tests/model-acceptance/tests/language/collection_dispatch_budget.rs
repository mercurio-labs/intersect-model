use intersect_model::conversion::{
    applied_collection_provider_with_conversions, with_catalog_context,
};
use intersect_model::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    conversion::{
        AppliedConversionContract, ConversionContract, MemberContract, MemberType, ProviderId,
    },
    generic_types::{ClosedType, Type},
    resources::DeclarationId,
};
use serde_json::json;

#[test]
fn selected_collection_tree_is_built_once_at_catalog_node_boundary() {
    let source = DefinitionSource::new(
        "memory:collection-budget",
        SourceKind::Ecore,
        r#"<e:EPackage xmlns:e="http://www.eclipse.org/emf/2002/Ecore" xmlns:x="http://www.w3.org/2001/XMLSchema-instance" name="budget" nsURI="urn:budget">
    <eClassifiers x:type="e:EDataType" name="Text" instanceClassName="java.lang.String"/>
    <eClassifiers x:type="e:EDataType" name="Map" instanceClassName="java.util.Map" serializable="false"><eTypeParameters name="K"/><eTypeParameters name="V"/></eClassifiers>
    <eClassifiers x:type="e:EDataType" name="List" instanceClassName="org.eclipse.emf.common.util.EList" serializable="false"><eTypeParameters name="T"/></eClassifiers>
    </e:EPackage>"#,
    );
    let id = |n| DeclarationId {
        source: source.id.clone(),
        path: vec![n],
    };
    let ty = |n, arguments| Type::Classifier {
        declaration: id(n),
        arguments,
        raw: false,
    };
    let mut tree = AppliedConversionContract {
        application: ClosedType::new(ty(1, vec![ty(0, vec![]), ty(0, vec![])])).unwrap(),
        conversion: ConversionContract {
            datatype: id(1),
            provider: ProviderId {
                name: "ecore.map".into(),
                version: "1".into(),
            },
            serializable: false,
            declarations: vec![],
            members: vec![
                MemberContract {
                    nullable: false,
                    value: MemberType::Datatype { declaration: id(0) }
                };
                2
            ],
        },
    };
    // Seven complete map levels consume 127 nodes. A list wrapper consumes the
    // final node. Dispatch must reuse that checked implementation, not rebuild it.
    for _ in 0..6 {
        tree = AppliedConversionContract {
            application: ClosedType::new(ty(1, vec![tree.application.expression().clone(); 2]))
                .unwrap(),
            conversion: ConversionContract {
                members: vec![
                    MemberContract {
                        nullable: false,
                        value: MemberType::Applied {
                            contract: Box::new(tree.clone())
                        }
                    };
                    2
                ],
                ..tree.conversion.clone()
            },
        };
    }
    let selected = AppliedConversionContract {
        application: ClosedType::new(ty(2, vec![tree.application.expression().clone()])).unwrap(),
        conversion: ConversionContract {
            datatype: id(2),
            provider: ProviderId {
                name: "ecore.list".into(),
                version: "1".into(),
            },
            serializable: false,
            declarations: vec![],
            members: vec![MemberContract {
                nullable: false,
                value: MemberType::Applied {
                    contract: Box::new(tree),
                },
            }],
        },
    };
    let bundle = DefinitionBundle {
        version: 1,
        sources: vec![source.clone()],
        namespaces: vec![],
    };
    with_catalog_context(&bundle, &[], std::slice::from_ref(&selected), |registry| {
        let implementation =
            applied_collection_provider_with_conversions(&bundle, &selected, registry)?;
        let mut value = implementation.empty()?;
        assert_eq!(value, json!({"items":[]}));
        implementation
            .insert(
                &mut value,
                0,
                json!(
                    registry
                        .applied()
                        .bind(&selected.application, json!({"items":[]}))?
                ),
            )
            .unwrap_err();
        assert_eq!(value, json!({"items":[]}));
        registry.applied().bind(&selected.application, value)?;
        Ok(())
    })
    .unwrap();
    let exceeded = AppliedConversionContract {
        application: ClosedType::new(ty(2, vec![selected.application.expression().clone()]))
            .unwrap(),
        conversion: ConversionContract {
            members: vec![MemberContract {
                nullable: false,
                value: MemberType::Applied {
                    contract: Box::new(selected.clone()),
                },
            }],
            ..selected.conversion
        },
    };
    assert!(
        with_catalog_context(&bundle, &[], &[exceeded], |_| Ok(()))
            .unwrap_err()
            .contains("budget")
    );
}
