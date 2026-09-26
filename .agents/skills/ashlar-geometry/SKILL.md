---
name: ashlar-geometry
description: Teaches how to author building geometry with the ashlar recipe API in this repository. Use when asked to author a part, add a building part, make a facade, door, window or roof, assemble a building, attach to a socket, add a collision proxy, fix a mesh that has a hole or is not closed, dress cut faces, choose a UV mode, fit a building to terrain, or add a showcase scene to the preview.
---

# Authoring building geometry with ashlar

`ashlar` describes a building as plain Rust data. A `Part` is a reusable solid
made of named `Element`s, each holding a `Geometry` expression, a material
slot, a UV mode and a collision declaration. A `Building` places parts as
`Instance`s and binds slot names to opaque material keys. The domain crate
never tessellates: `ashlar-manifold` evaluates each used part once over the
native Manifold kernel, and `ashlar-bevy` uploads the result. Read `references/api.md` for signatures, `references/walkthrough.md`
for the showcase kits taken apart, and `references/validation.md` for every
error path.

## Mental model

- A recipe is data. `Building::builder(id)...build()` returns an immutable
  `Building`; `into_recipe()` gives back the editable `BuildingRecipe`, and
  `build()` revalidates it. Serde reads and writes the recipe, never the
  building, so a loaded document always passes through validation.
- A part is elements, and elements stay separate solids. Composition inside a
  part is not a union: two elements that overlap are two meshes and two draw
  batches. Fuse inside one element with `union`, `union_all`, `arrayed` or
  `mirrored` when one closed surface is wanted.
- A `Geometry` is a tree. The primitives are `cuboid`, `chamfered_cuboid`,
  `cylinder`, `extrude`, `revolve` (a `[radius, height]` profile turned about
  Y, for domes, pods and tanks) and `hull`; `subtract`, `union`, `arrayed` and `mirrored` wrap
  an existing tree in a new node. Every node carries its own `Pose`, applied
  after the node evaluates, expressed in the parent node's frame.
- An instance is placed at a pose or attached by sockets. A `Socket` is a
  named frame on a part. `Instance::attach` derives the pose at build time and
  writes it into the instance, so nothing downstream knows an attachment
  happened.
- Material slots are semantic names on elements. The building binds each slot
  to a key; an instance may override the binding of a slot its part actually
  uses. `cut_material` gives the faces a cutter created a second slot, and the
  mesher then splits that element into an outer batch and a cut batch.
- Validation returns a path. Every `ValidationError` names a location such as
  `parts[study:facade].elements[panel].geometry.cutters[0]` or
  `instances[front-0-1].attach.target`, then the reason. Read the path before
  reading the geometry.
- The mesher evaluates each used part once. `mesh_building` skips parts that
  no instance references, meshes every element of the rest, derives declared
  collision proxies from the same evaluated solid, and
  `MeshedBuilding::colliders()` places one proxy per instance by transform
  alone.

## Coordinates and units

Everything is building-local `f64` metres, right-handed, Y up. World
placement belongs to the game that embeds the building.

- A cuboid runs from the origin to its positive size, so `Pose::at` positions
  its minimum corner, not its centre.
- A cylinder stands on Y=0 around the Y axis with an authored segment count.
  A revolve turns about the same axis; keep a round piece's axis at its part
  origin so `UvMode::Cylindrical` maps it metre-true. `examples/showcase/src/scifi.rs`
  is the worked kit: domes, capsule pods, a flared tower, tanks, a quonset.
  Rotate it with `DQuat::from_rotation_x(FRAC_PI_2)` to bore a horizontal
  hole, as the facade bay does for its fixing holes.
- An extrusion profile is `[x, z]` pairs, unclosed, in either winding, extruded
  from Y=0 to Y=depth along +Y. Concave profiles are fine; holes, repeated
  points, collinear neighbours and self-intersections are refused.
- A `Pose` is a translation plus a unit quaternion and has no scale, so
  dimensions live in the geometry. `placed` and `rotated` set absolute
  values, never increments; the transform and composition formulas are in
  `references/api.md`.
- The showcase convention puts a bay's exterior face at Z=0 with the wall body
  extending toward +Z: outward trim sits at negative Z, recessed glass at
  positive Z, and an unrotated wall side faces the negative Z half-space. The
  left socket sits at the origin facing -X and the right socket at X=4 facing
  +X.
- Sockets face outward along +Z of their own frame. `attach` turns the attached
  part half a turn about the target socket's Y axis so the two outward
  directions oppose, the way two modules meet at a shared edge.
  `attach_aligned` lays the two frames on top of each other, which is what
  stacking a floor on a `top` socket wants.

## Workflow

1. Write the part in Rust with the builder. Start from one body element,
   subtract openings with cutters that run past both faces, then add trim as
   further elements. Borrow the showcase's helpers: a `block(id, size, at,
   bevel, slot)` function that picks `chamfered_cuboid` when the bevel is
   positive and `cuboid` otherwise, and a `solid` wrapper over it that adds
   `Collision::Bounds`. Give elements stable ids, because error paths and mesh
   batches are keyed by them.
2. Declare sockets if the part will be chained or stacked: `left` at the
   origin facing -X, `right` at the bay width facing +X, an entrance socket on
   the threshold facing outward.
3. Bind materials. Every slot that any element of a used part names must be
   bound on the building or overridden on the instance, cut slots included, or
   `build()` stops at `instances[id].materials[slot]`. An instance override
   replaces the palette's binding for that one slot. A `Binding` carrying
   `param` values overrides graph parameters, which only a `Graph` or `Shader`
   surface has.
4. Assemble instances. Place the corner bay of each side by hand, then attach
   the rest with `attach("left", previous_id, "right")`. Repeat with loops;
   an `arrayed` node repeats a solid inside one element and is not a way to
   place instances.
5. Call `build()` and read the error path. `PartBuilder::build` checks names
   and geometry locally; `BuildingBuilder::build` rechecks every part, then
   references, bindings, poses and attachments.
6. Add a catalog scene. Add a variant to `Scene` in
   `examples/showcase/src/lib.rs`, route it in `building()`, and add a row to
   the `SCENES` table in `tools/ashlar-preview/src/main.rs`. The
   `materials` and `graphs` lookups default a new variant to the showcase's
   Rust library (`examples/showcase/src/library.rs` over
   `ashlar_material::stdlib`), so only a scene with its own palette needs a new
   arm. Bind slots to `library:*` keys; add a `showcase:*` definition in
   `library.rs` only for a scene-specific variant. Keep
   the command-line name lowercase and hyphenated; the catalog panics on a
   duplicate name.
7. Preview with `just preview --scene <name> --clay --wireframe`. Clay removes
   the materials so silhouettes and gaps read; wireframe shows the triangles a
   cutter or a chamfer added. Drag left to orbit, right to pan, scroll to zoom.
   `F` toggles wireframe, `P` toggles clay, `Home` resets framing, `Escape`
   exits.
8. Check the mesh in a test. Mesh the building with
   `mesh_building(&building, &ManifoldMesher::default())`, sum
   `triangle_count()` over `meshed.parts`, and check closedness the way
   `crates/ashlar-manifold/tests/geometry.rs` does: after `unweld`, every
   undirected edge must appear exactly twice with opposite directions. That
   helper is private to the test file, so copy it rather than importing it.
   The showcase kits are already meshed end to end in
   `tools/ashlar-preview/tests/tangents.rs`, which is the place to add a
   content assertion.
9. Capture with `just preview --scene <name> --screenshot <path>` and its
   `--wireframe` twin. Compare the scale before and after the change:
   `just references --reference-scenes <name>` writes the `instances` and
   `unique_triangles` of every scene to `target/references/manifest.tsv`.

## Rules that bite

- Subtraction keeps operand poses. `a.placed(p).subtract(c.placed(q))` builds
  a new node with an identity pose that contains both, and neither pose is
  reset. Each `subtract` call nests one more level; the nesting limit and its
  workaround are in `references/validation.md`.
- A cutter that stops inside the solid makes a recess; one that runs past both
  faces makes an opening. Subtracting a solid from itself yields an empty mesh
  rather than an error, and a cutter that misses leaves the solid intact.
- A union creates no cut faces. Cut provenance follows the cutters: every copy
  of an array carries its source's provenance, a mirror keeps the provenance
  of the faces it reflects, and a union keeps each operand's.
- Chamfers are a primitive, not a modifier. `chamfered_cuboid(size, bevel)`
  is the hull of inset corners and keeps the authored outer bounds; the bevel
  bound is in `references/api.md`. Nothing bevels the result of a boolean, so
  a softened opening needs a chamfered cutter.
- Normals crease at 45 degrees and smooth below it, so cylinder sides are
  smooth and their caps are creased without any authoring. An edge at exactly
  45 is a crease, so a 45 degree chamfer keeps its faces flat.
- UVs are metres in part space and the material's `tile_metres` turns them
  into repeats. `Planar`, the default, frames every face on its own origin, so
  coplanar faces of two elements do not share a repeat phase. `Box` reads the
  part axes with one origin, so a wall of modules tiles continuously, but an
  angled face is foreshortened and a face seen from the far side is mirrored.
  `Cylindrical { axis }` puts arc length along U with one seam and falls back
  to the plane mapping for caps. A non-default mode is applied after
  evaluation: the mesh is unwelded, projected and welded again.
- Welding shares vertices where position, normal, UV and cut flag agree within
  `WELD_TOLERANCE`; creases, UV seams and the edge of a cut stay split. It is
  an optimization and never a repair, so a hole in a mesh is a geometry
  problem.
- A collision proxy is convex and conservative. `Bounds` is the axis-aligned
  box of the evaluated solid, `Hull` its convex hull, both in part space and
  placed with the instance. An opening subtracted inside an element is not
  subtracted from its proxy, so a doorway that must be walked through is the
  gap between two elements. A proxy has to stay convex and tight, which ADR
  0002 is the reason for, so author one element per straight run of wall and
  never a whole L in one. Trim, slats, lights and signage keep the default
  `None`.
- Merging is a per-building choice and it is off by default. `.merged()` on
  the builder unions each merge group into one solid in building space:
  internal faces go, joints stop fighting for depth, and box UVs run through
  every former joint. It gives up instancing, so a repeated kit may stay
  unmerged. Groups are declared with `.group(MergeGroup::new(id).storey(n))`
  and named with `Instance::group`; an instance naming none is in the default
  group, the building. A group is also the unit a hit re-meshes, so size it
  like a storey or a bay and put its boundary at a floor line. Mark glass,
  lights, doors and anything swapped or animated `Element::standalone()`.
  Author joints to touch exactly: face to face unions clean, a nanometre gap
  is two components.
- A coarser level of detail drops what is thin. `LodPolicy::min_feature` judges
  an element or union member by the middle of its three extents, so a long
  sill, mullion, rail or strip light goes at a distance however long it is.
  Union fine trim into one element per kind so it goes together, and mark
  anything that glows and must read from afar — a crown of lights, a neon
  line, a lamp — `Element::far()`. See ADR 0006 and its amendment.
- Two elements must not put same-facing faces on one plane in two materials.
  Drawn as parts they fight for depth there, and merged the kernel gives each
  coplanar triangle to one operand and the choice is not ours, so slivers of
  one material show in the other. Offset trim by a centimetre, or give both
  the same slot. Faces on one plane facing opposite ways are fine: that is two
  solids touching, which is how a kit stacks. The lint is
  `no_part_shares_a_plane_between_two_materials` in
  `examples/showcase/tests/coincident.rs`.
- A cutter may name its own slot with `Geometry::cut_material`, valid only on
  a cutter. A face resolves cutter slot, then the element's cut slot, then its
  main slot, and every slot needs a binding. Cutters are numbered depth first
  by `Geometry::cutters()`.
- Interiors are added, never carved. Walls, floors and partitions are
  elements, because a room carved out of a mass would be filled by that mass's
  convex proxy. Declare `Element::interior()` on floors, partitions and
  liners; side is per element, so a slab whose top is inside a room wants a
  thin interior liner over it, as a wall does. A liner is its own element and
  needs its own cutter wherever the wall has an opening.
- `Geometry::portal(id)` marks a cutter as an opening and publishes a
  rectangle at the wall's mid-plane, per placement. The cutter must pass
  right through the wall it cuts: the portal is measured against the core the
  cutter removes, not the whole element's bounds, so a doorway through one leg
  of an L is a portal. It cannot sit under an array or a mirror, and ids are
  unique within one geometry. Mark the wall's cutter, not the liner's.
  `Room::new(id, size)` declares a box volume with a pose, a group and the
  portal ids opening onto it; it is data and is never meshed.
- Damage is data and applies to merged buildings only. A `Damage` is a solid,
  a pose and a slot bound in the building's own palette; `Damage::blast`
  makes a ball from points we generate, never a kernel sphere. Standalone
  elements are reported by `standalone_touched` and never cut.
- Attachment order in the file does not matter; cycles do. `build()` resolves
  attachments in dependency order, so a target may be declared later. `placed`
  after `attach` clears the attachment; `attach` after `placed` wins at build
  time. Composing one building out of another's instances must rewrite
  attachment targets along with the ids; `corporate::block` gets away without
  it only because that kit places every bay by hand.
- Grounding is a separate contract. `GroundingSpec` declares the load-bearing
  footprint, floor height, slab underside and an entrance socket with +Z
  outward; `GroundingPolicy` sets clearance, embedding, support and ramp
  limits. `fit_ground` reads a caller-supplied `TerrainPatch`, returns a world
  pose with foundation and ramp prisms, and refuses unsuitable sites with a
  typed `GroundError`. Keep the spec beside the geometry it describes, with the
  footprint matching the authored slab; `crates/ashlar/tests/grounding.rs` is
  the worked example.

## Shipping it

A game does not mesh at play. A content step (`ashlar-content`, a small binary
in the game's repository) meshes each building at every level of detail into a
`.ashlar` file, and `ashlar_bevy::prelude::AshlarPlugin` draws it with each level
in a `VisibilityRange` band. `docs/guide/integration.md` is the whole path, and
`examples/integration-content` is the template. What that asks of an author:

- Levels come from `LodPolicy`, not decimation. Past 60 m,
  `LodPolicy::ladder()` drops interior elements and every element, union
  member and cutter opening under 35 cm; past 250 m, everything under 1.5 m.
  Cylinders and revolves keep a half, then a quarter, of their segments.
- So mark what is inside with `Element::interior()`: liners, floors seen from
  inside, furniture. An interior left on the exterior side is drawn at every
  distance.
- Make small detail its own element or union member, not part of a large one,
  so it can drop. A vent fused into a wall panel stays as long as the panel.
- A dropped opening is carried by the texture from that distance, so a facade
  material should read as windows at a distance.
- A per-instance `Binding` with parameters becomes a material of its own at
  export (`flatten_overrides`). Distinct values are distinct bakes, so a few
  colours shared across many instances cost less than one per instance.

## Commands

```sh
cargo test -p ashlar                               # domain validation, no native build
cargo test -p ashlar-manifold                      # mesher; a first build compiles Manifold
just preview --scene facade --clay --wireframe     # isolate one part
just preview --scene outpost --screenshot /tmp/outpost.png
just preview --scene outpost --write-example /tmp/outpost.ron
just materials                                     # export the showcase library once
just preview --recipe /tmp/outpost.ron --materials assets/materials/library.materials.ron --graphs assets/materials/graphs.ron
just references --reference-scenes facade,entrance # headless gallery in target/references
just content 512 metropolis                        # bake a scene to assets/buildings/<scene>.ashlar
cargo run --example baked -p ashlar-bevy -- metropolis   # draw it as a game does, levels and all
```

Use `--yaw`, `--pitch` and `--zoom` for reproducible captures, `--frames` to
bound a run, and `--serial-pipelines` when a driver crashes compiling shaders.
A custom `--recipe` without `--materials` renders in diagnostic colours.
Restart the preview after editing a recipe or a material library; only a
graph library read with `--graphs` reloads while it runs. The built-in scenes
bring their materials with them as Rust, so they need no export. The first mesher build compiles Manifold
and needs Git, CMake and a C++ compiler.

## References

- `references/api.md`: every public type and constructor a part author uses,
  grouped, with signatures and one-line meanings.
- `references/walkthrough.md`: the facade bay element by element with the
  reason for each decision, then the corporate kit assembled into a tower, an
  annex and a courtyard block.
- `references/validation.md`: every validation error path with its cause and
  fix, then the mesher's failure modes.
