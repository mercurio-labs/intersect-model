//! The two README schema choices, exercising the same small model.

mod dynamic {
    use intersect_model::{
        definitions::dynamic::{ScalarType, SchemaBuilder},
        dynamic::DynamicModel,
    };

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let mut builder = SchemaBuilder::new("example", "urn:example");
        let item = builder.class("Item")?;
        let name = item.attribute("name", ScalarType::String).required()?;
        let schema = builder.build()?; // DynamicSchema
        let mut model = DynamicModel::new(schema)?;
        model.transaction(|tx| {
            let mut resource = tx.create_resource("memory:items")?;
            let object = resource.create(&item, "item-1")?;
            resource.set(&object, &name, "First item")?;
            Ok(())
        })?;
        let saved = model.to_json()?;
        println!("Saved a valid model ({} bytes)", saved.len());
        Ok(())
    }
}

mod rcore {
    use intersect_model::{api::ModelEdit, compile_rcore};

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let package = compile_rcore(
            r#"
        rcore 1;
        package example version "1";
        metamodel "urn:example" prefix example {
            class Item { attr name: String [1,1]; }
        }
    "#,
        )?;

        let mut model = package.new_model()?;
        model.create("item-1", "Item", "memory:items")?;
        model.set("item-1", "name", serde_json::json!("First item"))?;
        model.validate()?;
        let saved = package.save(&model)?;
        println!("Saved a valid model ({} bytes)", saved.len());
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    dynamic::run()?;
    rcore::run()?;
    Ok(())
}
