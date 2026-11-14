//! Handwritten runtime definitions and instances; no Ecore, grammar or codegen.
use intersect_model::core::dynamic::DynamicModel;
use intersect_model::definitions::dynamic::{ScalarType, SchemaBuilder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut schema = SchemaBuilder::new("library", "urn:example:library");
    let library = schema.class("Library")?;
    let library_name = library.attribute("name", ScalarType::String).required()?;
    let book = schema.class("Book")?;
    let title = book.attribute("title", ScalarType::String).required()?;
    let edition = book.attribute("edition", ScalarType::Integer).default(1)?;
    let books = library.reference("books", &book).many().containment()?;
    let owning_library = book.reference("library", &library).optional()?;
    schema.opposites(&books, &owning_library)?;

    let schema = schema.build()?;
    let mut model = DynamicModel::new(schema)?;
    let (catalog, dune) = model.transaction(|tx| {
        let mut resource = tx.create_resource("urn:example:catalog")?;
        let catalog = resource.create(&library, "library-1")?;
        resource.set(&catalog, &library_name, "Central Library")?;
        let dune = resource.create(&book, "book-1")?;
        resource.set(&dune, &title, "Dune")?;
        resource.add(&catalog, &books, &dune)?;
        Ok((catalog, dune))
    })?;

    assert_eq!(model.class(&dune)?, book);
    assert_eq!(model.get(&dune, &title)?.as_str()?, "Dune");
    assert_eq!(model.get(&dune, &edition)?.as_i32()?, 1);
    assert_eq!(
        model.get(&dune, &owning_library)?.as_object()?,
        Some(catalog)
    );
    model.transaction(|tx| tx.set(&dune, &edition, 2))?;

    let discovered_book = model.schema().class("Book")?;
    assert_eq!(discovered_book, book);
    assert_eq!(
        model
            .get(&dune, &discovered_book.feature("title")?)?
            .as_str()?,
        "Dune"
    );

    // A host can save these bytes; the shared core performs no filesystem I/O.
    let saved = model.to_json()?;
    let reloaded = DynamicModel::from_json(&saved)?;
    let reloaded_book = reloaded.schema().class("Book")?;
    assert_eq!(
        reloaded
            .get(
                &reloaded.object("book-1")?,
                &reloaded_book.feature("edition")?
            )?
            .as_i32()?,
        2
    );
    println!("Created Dune, updated edition to 2, and reloaded schema and model successfully.");
    Ok(())
}
