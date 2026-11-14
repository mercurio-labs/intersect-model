//! Explicit collection interface policies; no ambient Java class lookup.
use crate::collection_conversion::CollectionKind;
use intersect_model::definitions::conversion::{
    ClassifierRelation, ConstructorContract, ConstructorProvider,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CollectionDomain {
    EList,
    List,
    Collection,
    Map,
    Sequence,
}
impl CollectionDomain {
    pub(super) fn provider(name: &str) -> Result<Self, String> {
        match name {
            "ecore.list" => Ok(Self::EList),
            "java.list" => Ok(Self::List),
            "java.collection" => Ok(Self::Collection),
            "ecore.map" => Ok(Self::Map),
            _ => Err("No installed collection domain for this provider".into()),
        }
    }
    pub(super) fn metadata(self) -> (&'static str, CollectionKind, usize) {
        match self {
            Self::EList => ("org.eclipse.emf.common.util.EList", CollectionKind::List, 1),
            Self::List => ("java.util.List", CollectionKind::List, 1),
            Self::Collection => ("java.util.Collection", CollectionKind::List, 1),
            Self::Map => ("java.util.Map", CollectionKind::Map, 2),
            Self::Sequence => ("intersect.Sequence", CollectionKind::List, 1),
        }
    }
    fn relation(self, target: Self) -> ClassifierRelation {
        use ClassifierRelation as R;
        use CollectionDomain as D;
        if self == target {
            return R::Equivalent;
        }
        match (self, target) {
            (D::EList, D::List | D::Collection) | (D::List, D::Collection) => R::Subtype,
            (D::List | D::Collection, D::EList) | (D::Collection, D::List) => R::Supertype,
            _ => R::Unrelated,
        }
    }
}

pub(super) struct CollectionConstructor {
    contract: ConstructorContract,
}
impl CollectionConstructor {
    pub(super) fn new(contract: ConstructorContract) -> Result<Self, String> {
        Self::domain(&contract)?;
        Ok(Self { contract })
    }
    fn domain(contract: &ConstructorContract) -> Result<CollectionDomain, String> {
        let name = contract
            .provider
            .name
            .strip_suffix(".constructor")
            .ok_or("No installed constructor policy for this provider")?;
        // Sequence is a distinct serializable domain. It does not opt the
        // nonserializable collection value factory into lexical sequence values.
        let domain = if name == "intersect.sequence" {
            CollectionDomain::Sequence
        } else {
            CollectionDomain::provider(name)?
        };
        let (class, _, arity) = domain.metadata();
        let datatype = &contract.datatype;
        if contract.provider.version != "1" {
            return Err("Unsupported constructor provider version".into());
        }
        if datatype.serializable() != (domain == CollectionDomain::Sequence)
            || datatype.parameters().len() != arity
            || datatype.application().is_some()
            || datatype.instance_class_name() != Some(class)
            || datatype
                .instance_type_name()
                .is_some_and(|name| name != class)
        {
            return Err(
                "Constructor policy requires its exact collection datatype metadata".into(),
            );
        }
        Ok(domain)
    }
}
impl ConstructorProvider for CollectionConstructor {
    fn contract(&self) -> &ConstructorContract {
        &self.contract
    }
    fn constructor_relation(
        &self,
        target: &ConstructorContract,
    ) -> Result<Option<ClassifierRelation>, String> {
        Ok(Some(
            Self::domain(&self.contract)?.relation(Self::domain(target)?),
        ))
    }
}
