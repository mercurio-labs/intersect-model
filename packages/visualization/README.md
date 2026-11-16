# Visualization client

Language-neutral SVG/ELK scene renderer for Mercurio. Source interpretation and
edit validation belong to the language provider; this package only emits tool
intents and stores separate presentation geometry.

The retained renderer/protocol sources came from the installed, immutable
`@intersect/visualization` package at Intersect revision
`17554be4fa73bdf0fecda5bee5d01ed3f49e8830`. This restores their maintained public
home alongside `intersect-views`; no archived Mercurio implementation is used.

`parameters.placement` selects a canvas gesture without introducing language
knowledge: `node` (default), `border`, or `connection`. Place tools support native
palette drag/drop and click-to-place. Connection tools support dragging between
visible endpoints with an orthogonal preview and Escape cancellation. The
provider receives endpoint occurrence IDs and validates ownership and paths.

Saved node geometry includes size. Port geometry uses its existing stable symbol
key in `positions`, with a local center on its parent's border and a 12×12 size.
The canvas snaps saved centers back to the border after resizing. Moving nodes,
ports, resizing, panning and zooming never modify model text.

Run `npm ci`, `npm run check` and `npm test` at the repository root. Real-engine
gesture acceptance runs in Mercurio's `general-view.spec.ts`.
