# Decisions

These are the architecture decision records: one page per decision that shaped ashlar,
with the problem it answered, what we chose and what it costs.
They are written when the decision is made and amended, with a date, when it moves,
so a record says what was true then and what changed since.

If you are reading the manual front to back, you do not need these to use the crates.
They are what to read when something looks surprising and you want to know why.

| Record | What it decides |
| --- | --- |
| [0001](0001-recipe-domain-and-backends.md) | A building is plain Rust data, and the kernel that meshes it is a separate crate |
| [0002](0002-attachment-and-derived-collision.md) | Parts attach by sockets, and collision is derived from the same geometry |
| [0003](0003-procedural-materials.md) | A material is a graph that bakes to textures or compiles to a shader |
| [0004](0004-strand-layers.md) | Grass, fur and moss are strands scattered from the material's own fields |
| [0005](0005-merged-geometry-and-damage.md) | Merged buildings, interiors, face provenance, and damage as data |
| [0006](0006-baked-buildings-and-lod.md) | Buildings bake to files with levels of detail, like materials do |
