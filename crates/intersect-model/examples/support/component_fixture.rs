//! Handwritten small fixture shared by the example and its integration controls.
use intersect_model::{
    components::{ComponentImage, MountedModel, SharedSchema},
    frozen::{ById, ModelBuilder},
    storage::StorageResult,
};
use std::sync::Arc;

pub const URI: &str = "urn:example:node-library";
pub const SOURCE: &str = r#"rcore 1;
package components version "1";
metamodel "urn:example:components" prefix c {
  class Node {
    attr name: String [0,1];
    ref target: Node [0,1];
    ref out: Node [0,*] opposite Node.incoming;
    ref incoming: Node [0,*] opposite Node.out;
    ref partner: Node [0,1] opposite Node.partneredBy;
    ref partneredBy: Node [0,1] opposite Node.partner;
    ref children: Node [0,*] containment opposite Node.parent;
    ref parent: Node [0,1] opposite Node.children;
  }
  class Other { attr name: String [0,1]; }
}"#;

pub struct Fixture {
    pub context: Arc<SharedSchema>,
    pub library: Arc<ComponentImage>,
    pub node: String,
    pub other: String,
}

impl Fixture {
    pub fn new() -> StorageResult<Self> {
        let package = intersect_model::compile_rcore(SOURCE).map_err(|e| e.to_string())?;
        let context = SharedSchema::new(package.schema())?;
        let key = |name: &str| {
            package
                .schema()
                .declarations
                .iter()
                .find(|(key, declaration)| {
                    declaration.name == name && package.schema().classes.contains_key(*key)
                })
                .map(|(key, _)| key.clone())
                .ok_or_else(|| format!("Missing class {name}"))
        };
        let node = key("Node")?;
        let other = key("Other")?;
        let library = prepare(context.clone(), &node, URI, "Library definition")?;
        Ok(Self {
            context,
            library,
            node,
            other,
        })
    }

    pub fn project(&self, id: &str) -> StorageResult<MountedModel> {
        let mut project = MountedModel::new(self.context.clone(), vec![self.library.clone()])?;
        project.create(id, &self.node, "memory:/project.nodes")?;
        Ok(project)
    }
}

pub fn prepare(
    context: Arc<SharedSchema>,
    class: &str,
    uri: &str,
    name: &str,
) -> StorageResult<Arc<ComponentImage>> {
    let mm = context.metamodel();
    let class = mm.class(class).ok_or("Missing Node class")?;
    let name_feature = mm.feature(class, "name").ok_or("Missing name feature")?;
    let out = mm.feature(class, "out").ok_or("Missing out feature")?;
    let mut builder = ModelBuilder::new(mm);
    let shared = builder
        .create("shared", class, "memory:/library.nodes")
        .map_err(|e| e.to_string())?;
    let internal = builder
        .create("internal", class, "memory:/library.nodes")
        .map_err(|e| e.to_string())?;
    builder
        .set_str(shared, name_feature, name)
        .map_err(|e| e.to_string())?;
    builder
        .set_str(internal, name_feature, "Internal definition")
        .map_err(|e| e.to_string())?;
    builder
        .add_ref(shared, out, internal)
        .map_err(|e| e.to_string())?;
    let model = builder
        .link(&ById)
        .map_err(|e| format!("{e:?}"))?
        .validate()
        .map_err(|e| format!("{e:?}"))?;
    ComponentImage::prepare(context, uri, model)
}
