//! Verification-only canonical/unsupported model contexts for pre-change captures.
use crate::definitions::{
    bundle::{DefinitionBundle, DefinitionSource, SourceKind},
    generic_types::Type,
    resources::DeclarationId,
};
pub(crate) fn fixture(case: usize) -> (DefinitionBundle, Vec<Type>, &'static str) {
    let classes = match case {
        0 | 9 | 11 => "class Node {attr label:String [0,1];ref next:Node [0,*];}",
        1 => {
            "class Root {attr label:String [0,1];}class Left extends Root {}class Right extends Root {}class Leaf extends Left, Right {}"
        }
        2..=5 => {
            "class Value {}class Other {}class Box<T> {}class Use {ref value:Box<Value> [0,1];ref other:Box<Other> [0,1];}"
        }
        6 => "class Root extends Child {}class Child extends Root {}",
        7 => {
            "class Root {attr label:String [0,1];}class Child extends Root {attr label:String [0,1];}"
        }
        8 => "",
        10 => "class Node {attr count:Int32 [1,1] default \"bad\";}",
        _ => unreachable!(),
    };
    let source = if case == 8 {
        DefinitionSource::new(
            "memory:runtime-schema.ecore",
            SourceKind::Ecore,
            "<e:EPackage xmlns:e='http://www.eclipse.org/emf/2002/Ecore' xmlns:x='http://www.w3.org/2001/XMLSchema-instance' name='p' nsURI='urn:runtime-schema'><eClassifiers x:type='e:EClass' name='Node'><eStructuralFeatures x:type='e:EAttribute' name='label' eType='e:EDataType http://www.eclipse.org/emf/2002/Ecore#//EString'/></eClassifiers></e:EPackage>",
        )
    } else {
        DefinitionSource::new(
            "memory:runtime-schema.rcore",
            SourceKind::Rcore,
            format!(
                "rcore 1;package p version \"1\";metamodel \"urn:runtime-schema\" prefix p {{{classes}}}"
            ),
        )
    };
    let plain = |index| Type::Classifier {
        declaration: DeclarationId {
            source: source.id.clone(),
            path: vec![index],
        },
        arguments: vec![],
        raw: false,
    };
    let applied = |arguments, raw| Type::Classifier {
        declaration: DeclarationId {
            source: source.id.clone(),
            path: vec![2],
        },
        arguments,
        raw,
    };
    let requested = match case {
        1 => vec![plain(3)],
        2 => vec![
            plain(3),
            applied(vec![plain(0)], false),
            applied(vec![plain(1)], false),
            plain(0),
            plain(1),
        ],
        3 => vec![plain(3)],
        4 => vec![applied(vec![], true)],
        5 => vec![applied(vec![plain(0), plain(1)], false)],
        6 | 7 => vec![plain(1)],
        9 => vec![plain(0), plain(0)],
        _ => vec![plain(0)],
    };
    let name = if case == 2 || case == 3 {
        "value"
    } else if case == 10 {
        "count"
    } else if case == 11 {
        "missing"
    } else {
        "label"
    };
    (
        DefinitionBundle {
            version: 1,
            sources: vec![source],
            namespaces: vec![],
        },
        requested,
        name,
    )
}
