# ADR 0006: Baked buildings and levels of detail

Status: accepted, 2026-09-24.

## Context

A production game ships baked resources and draws distant things cheaply.
Materials already do the first: `ashlar_material::export` writes KTX2 maps and
strand sets in a content step, and the featureless `ashlar-bevy` loads them
with no graph engine in the game (ADR 0003, 2026-09-24 amendment). Geometry
did not. The only way to get a building's triangles was to run
`ashlar-manifold`, which is the Manifold C++ kernel, in the game at startup,
and there was one level of detail: the building as authored, drawn at every
distance, so a metropolis of ten thousand instances drew twenty million
triangles.

## Decision

### A building is baked like a material

`ashlar::BakedBuilding` is what a mesher made of a recipe, plus coarser levels
of the same building. It holds the validated recipe, one `BakedLevel` per level
of detail (element meshes per part and merged groups, exactly the fields of a
`MeshedBuilding`), and level zero's collision proxies and portals.
`BakedBuilding::level(n)` answers a `MeshedBuilding`, so every consumer of a
freshly meshed building — `pieces()`, `colliders()`, `portals()`,
`ashlar_bevy::drawables` — reads a baked one unchanged.

It lives in `ashlar`, not in the mesher, because it names nothing a kernel
owns; that is the same reason `MeshedBuilding` does.

### The file

`BakedBuilding::write` and `read`: twelve identifying bytes shaped as KTX2's
and the strand set's, a version, a length-prefixed RON *manifest*, and a
length-prefixed binary *mesh table*. The manifest carries the recipe and every
level's structure with each mesh replaced by an index; the table carries the
meshes as little-endian arrays. The manifest is indented RON, so
structure stays readable and diffable, bulk
stays compact, and the reader refuses what it cannot account for — identifier,
version, every length, every index — by kind rather than panicking.

Positions, normals and UVs narrow to `f32` on disk. They narrow to `f32` on
their way to the GPU anyway, and building-local metres at that precision are
tens of microns across a city. Indices and face provenance are stored as
written. A read revalidates the recipe, so a file written by one version of the
rules and read by another says so.

No new dependency: `ron` was already in the workspace, and the container is a
few dozen lines beside the strand file's.

### Levels of detail are the recipe, simplified

A coarser level is not a decimated mesh. The recipe is procedural, so a
simpler building is a simpler recipe, and `ashlar::LodPolicy` says how:

- `min_feature` (metres): an element or union member thinner than this is
  left out (amended 2026-09-25, see below), and so is a cutter whose *opening* is narrower
  than this. An opening is judged by the middle of the cutter's three extents,
  because a cutter runs past both faces it opens and its depth says nothing
  about the hole: a vent's 20 cm, a door's metre. A hole that is no longer cut
  is carried by the facade texture at the distance the level draws from.
- `segment_scale`: every cylinder and revolve keeps this share of its
  segments, never fewer than `MIN_SEGMENTS` (eight) and never more than were
  authored.
- `drop_interior`: every `Side::Interior` element goes.
- `until` (metres): where the level stops drawing; `None` on the last level.

`Geometry::simplified`, `Part::simplified` and `Building::simplified` apply a
policy. A part nothing of which survives is removed, and so are the instances
that placed it; surviving instances keep their resolved poses with their
attachments cleared, and only the slot overrides their parts still use. A
baked level simply has no entry for a vanished part, and
`MeshedBuilding::pieces` already skips a part it has no meshes for.

`ashlar_manifold::bake(building, ladder, mesher)` meshes level zero exactly as
`mesh_building` does, colliders and portals included, and each later level by
meshing the simplified parts. A part whose simplification equals the previous
level's is not meshed again, and a merge group is re-meshed only when one of
its members changed. The mesher evaluates simplified parts exactly as it
evaluates authored ones, so every level is a closed, correctly shaded and
correctly mapped solid, and a level's texture lines up with the next one's.

Collision proxies and portals are level zero's at every level: physics and
visibility run at every distance.

`LodPolicy::ladder()` is a default of three levels: as authored to 60 m; no
interior, nothing under 35 cm and half the segments to 250 m; nothing under
1.5 m and a quarter of the segments beyond. Measured on the showcase
(triangles drawn, every instance counted, per level):

| Scene | Level 0 | Level 1 | Level 2 | Bake | File |
| --- | --- | --- | --- | --- | --- |
| metropolis (4 x 4 blocks) | 3,813,034 | 1,351,694 | 914,074 | 0.25 s | 9.1 MB |
| city block | 306,442 | 108,066 | 71,508 | 0.10 s | 4.8 MB |
| sci-fi colony | 119,182 | 53,718 | 25,088 | 0.26 s | 6.7 MB |
| corporate block | 41,132 | 41,000 | 40,208 | 0.01 s | 0.3 MB |

The corporate kit barely coarsens: it is chamfered boxes with storey-sized
openings, and nothing in it is small. The 8 x 8 metropolis bakes in 0.3 s to
12.4 MB. Much of a file is its RON manifest, since the recipe is in it whole. File sizes
are as of the 2026-09-25 review fixes, which store a mesh that two levels share
once and indent the manifest.

### A game draws levels with `VisibilityRange`

Bevy 0.19's `VisibilityRange` gives each level a distance band with a
crossfade margin, per entity, so instances of one part at one level still batch.
The featureless `ashlar-bevy` loads a baked building as an asset and spawns
each level's pieces with their band.

Two details keep the bands honest. The distance is Bevy's, camera to the
piece's origin, and a merged group's mesh is in building space, so it is drawn
about the centre of its level-zero bounds at every level; otherwise a group a
block long would switch level by the camera's distance to the building origin.
We did not use `VisibilityRange::use_aabb` for this, because Bevy documents it
as breaking the crossfade unless every level has the same bounds. And a piece
a coarser level did not change is one entity whose band spans both levels,
not two identical entities crossfading into one another.

The file stores such a piece once as well: the mesh table writes identical
records once, and every level that draws them names the same index.

## Consequences

- A game links `ashlar`, `ashlar-surface` and the featureless `ashlar-bevy`:
  no Manifold, no graph engine, no C++ toolchain in its build.
- A content step runs `ashlar-manifold` and `ashlar-material` once, and writes
  `.ashlar` buildings beside the KTX2 materials.
- Damage stays a runtime-kernel feature (ADR 0005): a game that cuts holes in
  play links the mesher, and bakes its undamaged state all the same.

## Amendment, 2026-09-25: thin, not small, and `Element::far`

The first rule judged an element by its largest side, and the far levels hardly
coarsened: a sill three metres long and twenty centimetres deep, a mullion, a
strip light, a rail, all survived to any distance, well under a pixel wide.
The 4 x 4 metropolis of the dark city drew 1.59 M triangles at its far level
against 3.93 M at level zero.

An element or union member is now judged by the middle of its three extents,
the narrower side of its broadest face: how wide it looks from where it looks
widest. That is the rule cutters already had. A plate a finger thick but four
metres square stays; a rail as long stays only as long as it is thick enough.

What glows is the exception, because a lit line reads from afar at a fraction
of a pixel. `Element::far` keeps an element and every member of it at every
level; only its segments coarsen. The dark city marks its crowns, corner
lines, neon signs, lanterns and lamps far.

Measured the same way as the table above:

| Scene | Level 0 | Level 1 | Level 2 |
| --- | --- | --- | --- |
| metropolis (4 x 4, the dark city) | 3,928,156 | 1,161,406 | 760,902 |
| city block (the dark city) | 402,248 | 119,424 | 80,654 |
| sci-fi colony | 119,182 | 48,130 | 19,292 |
| corporate block | 41,132 | 33,696 | 14,116 |

The corporate kit now coarsens too: its fins, mullions, sills and rails go.
