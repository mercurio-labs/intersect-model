//! Handwritten Library caller of build-time generated Rust bindings.
use intersect_model::core::api::{BasicModel, TransactionalModel};
use intersect_rust_bindings_tests::library::{self, BookCopy, Library};

fn main() -> Result<(), String> {
    let mut model = TransactionalModel::new(BasicModel::new(library::schema()?)?);
    let copy = model.transaction(|edit| {
        let catalog = Library::create(edit, "catalog", "catalog.library")?;
        catalog.set_name(edit, "Catalog")?;
        let copy = BookCopy::create(edit, "dune", "catalog.library")?;
        copy.set_name(edit, "dune")?;
        copy.set_title(edit, Some("Dune"))?;
        catalog.add_copies(edit, &copy)?;
        Ok(copy)
    })?;
    println!(
        "{}: {:?}; edition {:?}; owner {:?}",
        copy.name(&model)?,
        copy.title(&model)?,
        copy.edition(&model)?,
        copy.library(&model)?.map(|owner| owner.id().to_owned())
    );
    Ok(())
}
