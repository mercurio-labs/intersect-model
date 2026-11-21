# Local view-engine M10 changes

The checked SourceModelFunction batch constructors preserve schema, source,
signature, effect and execution checks while sharing one retained source bundle,
compiler preparation and schema proof across the selected entries. Existing single
constructors delegate to that path. Invocation contracts remain unchanged.

Local working-tree qualification: 928 Model tests passed, 21 ignored; strict
all-target clippy and WASM library check passed. Reports 20261117-205757-recovered
and 20261117-205244 are recorded in intersect-views/docs/evidence/m10.md.

After the replacement KerML fixtures and Mercurio diagrams/General View browser
flows passed, the legacy contracts crate was removed from this workspace.
The visualization client was already absent from this develop-based checkout;
its older copy remains untouched in the preserved specification branch. The authoritative implementation now belongs to the local
intersect-views repository. No remote publication or consumer release is implied.
Final exact-pair consumer qualification remains recorded by the view-engine repo.
