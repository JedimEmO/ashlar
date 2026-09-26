# ADR 0001: Rust building descriptions separate from geometry backends

Status: accepted, 2026-09-13.

## Context

Games should be able to describe buildings as reusable Rust recipes without
baking one architectural style into the engine. A macro language is premature
before its underlying types are useful. An earlier external-authoring prototype
was retired.

## Decision

`ashlar` is a Bevy-free domain crate. It depends only on shared math, Serde and
typed errors. It describes local solids, reusable parts, instances, material
slots and attachment frames. Rust functions and builders author the same
editable data that Serde loads.

Finalization validates a recipe and returns an immutable description. Backends
consume that description; the domain has no IO or renderer dependencies.
All dimensions and placements are local `f64` metres in right-handed Y-up space.
Style choices live in recipe geometry and supplied material bindings, with
instance overrides. There is no mandatory grid or fixed rotation increment.

## Consequences

The domain can be tested without a GPU. A future macro is a thin frontend over
ordinary types. Mesh generation, physics export and visual preview remain
separate implementation steps. The serialized recipe is experimental, not a
runtime asset format.

## Mesh adapter

`GeometryMesher`, `TriangleMesh` and `MeshError` define the renderer-independent
output boundary in the domain. `ashlar-manifold` evaluates expressions via
Manifold's Rust bindings, preserving shared part meshes and instance metadata.
The native C++ dependency lives entirely in that adapter, outside the domain and
outside any headless runtime graph that consumes fitted results.
The kernel can be replaced by another implementation of the port. No
architectural style, palette, grid policy or storey dimension is introduced into
the backend.

Manifold was chosen to evaluate nested arbitrary solid subtraction without
introducing a bespoke boolean kernel. Its native toolchain and first-build
source download are explicit tradeoffs; see the recipe workflow for
prerequisites.

## Expression vocabulary

`Shape` grew union, array and mirror nodes beside the primitives and
subtraction. They express repetition and symmetry as geometry rather than as a
loop that produces one element per copy, so the kernel fuses the copies and the
cut slots keep working across them: a union creates no cut faces of its own,
every copy of an array carries the provenance of its source, and a reflection
keeps the provenance of the faces it mirrors. `Shape` is `#[non_exhaustive]`,
and a backend rejects by path what it does not implement, so the vocabulary can
grow without breaking one.

## Material and physical detail

The domain also describes opaque PBR material libraries independently of
building recipes. Texture keys remain opaque; the render adapter handles PNG
loading, color spaces, repeat sampling and tangent generation. Per-material
metre scale and pattern offsets preserve shared geometry across finish variants.
Optional element cut slots use native surface provenance to split outer and cut
batches; no architectural role or material type is baked into that mechanism.

Texture projection is per element: planar by default, box mapping for repeats
that line up across coplanar elements of separate modules, and cylindrical for a
metre-true wrap with one seam. The mode is applied after evaluation, so the
mesher port is unchanged by it. Meshes are indexed, with vertices shared where
position, normal and UV agree, which keeps creases and UV seams split and
nothing else.

Physical chamfers are an explicit box primitive, not an implicit global
smoothing policy. General edge modifiers can be added independently. The example
content lives in `ashlar-showcase` together with the generator for its study
textures and material RON, and demonstrates shared modules in two structures.

## Sockets and collision

See [ADR 0002](0002-attachment-and-derived-collision.md): sockets derive an
instance's pose, and elements declare a conservative convex collision proxy that
the mesher derives from the evaluated solid.

## Terrain fitting

Ground fitting is a pure calculation over a supplied triangulated height patch.
The domain owns footprint, entrance, policy and fitted convex-solid data. It does
not sample a world, select a style or create renderer/physics handles. Triangle
clipping checks the full support and approach surface, including extrema between
corners. The caller says which way its own cells split, because a patch read
along the other diagonal is a different surface from the one its players walk
on. World sampling and convex collision conversion belong to the embedding
game.

`ashlar-showcase` is renderer-free content. `ashlar-bevy` shares mesh and PBR
upload without a native kernel dependency, so a consumer can draw fitted results
without compiling C++. This is not yet a building streaming system.
