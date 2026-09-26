# Building recipes

`ashlar` is the authoring layer for script-defined architecture. It supplies
regular Rust types and consuming builders, with no macro language, Bevy, IO or
mesh generation dependency. Recipes generate meshes in a separate adapter and
can be inspected in the standalone preview or fitted to terrain by the game that
embeds them. This is the building authoring workflow.

## Author a part, then assemble it

```rust
use ashlar::{Building, Element, Geometry, Instance, Part, Pose};

let panel = Part::builder("example:panel")
    .element(Element::new(
        "body",
        Geometry::cuboid([4.0, 3.0, 0.25]),
        "surface",
    ))
    .build()?;

let building = Building::builder("example:screen")
    .part(panel)
    .material("surface", "example:painted_metal")
    .instance(Instance::new("left", "example:panel"))
    .instance(
        Instance::new("right", "example:panel")
            .placed(Pose::at([4.0, 0.0, 0.0]))
            .material("surface", "example:timber"),
    )
    .build()?;
```

Run the complete example, which includes an opening, sockets, repetition and an
arbitrary rotation, to print a RON recipe:

```sh
cargo run -p ashlar --example recipe
cargo test -p ashlar
```

Ordinary Rust functions parameterize parts; loops repeat instances. A part is
registered once and referenced by ID. There is no enum of architectural styles,
mandatory storey height, facade pattern, default material, or required grid.

## Data and validation boundary

- `Geometry` holds a `Shape` and `Pose`. Shapes include plain/chamfered cuboids,
  cylinders, polygon extrusions, and subtraction, union, array and mirror
  expressions. Separate elements remain separate solids, so composition does not
  imply a boolean union.
- `Element` gives geometry a local identity, a semantic material slot, a UV mode
  and an optional collision proxy.
- `Part` contains named elements and optional named attachment frames (`Socket`).
  `PartBuilder::build` checks local validity. Parts remain editable data.
- `Instance` references a part and supplies a placement or an attachment, plus
  optional material overrides. Recipe bindings provide defaults. Material keys
  are opaque strings; material definitions and texture loading are resolved
  separately by the render adapter. An override is a `Binding`: a key, and
  optionally values of its own for the parameters of the graph behind that key —
  see [one material key, a value per instance](#one-material-key-a-value-per-instance).
- `BuildingRecipe` carries a `version`. A document written by a newer schema is
  refused rather than reinterpreted; one written before the field existed reads
  as version 1, which is what it is.
- `BuildingBuilder::build` and `BuildingRecipe::build` validate the whole assembly
  and return an immutable `Building`. They recheck edited parts, duplicate names,
  part references, transforms and every used material slot. Errors identify the
  offending path. Instance overrides must refer to slots actually used by that
  instance's part; recipe palettes may contain additional unused slots.

`BuildingRecipe` supports Serde for editable interchange. A `Building` serializes
as that recipe but cannot be directly deserialized. Load a `BuildingRecipe`, then
call `build()`. `building.recipe()` provides read-only access;
`building.into_recipe()` permits edits followed by revalidation. The RON schema
is experimental; runtime recipe loading and a persistent format migration contract
are not implemented yet.

## Coordinates and geometry semantics

All coordinates are building-local `f64` metres, right-handed and Y-up. World
placement remains the embedding game's concern. `Pose` is translation plus a unit
quaternion, applied as `translation + rotation * point`; there is no scale.
Dimension changes happen in the geometry recipe. `placed` and `rotated` set
absolute local values, not incremental transforms.

Cuboids run from `[0, 0, 0]` to their positive X/Y/Z dimensions. Cylinders run from
Y=0 to Y=height around the Y axis and carry an explicit subdivision count.
Extrusions use an unclosed simple polygon in X/Z, extruded from Y=0 to Y=depth.
Either winding and concave profiles are accepted. Holes, duplicate consecutive
vertices, collinear neighbours and self-intersections are rejected. Polygon
checks use floating-point predicates, not a CAD tolerance or repair kernel;
a future mesher must handle its own numerical tolerances and topology checks.

Subtraction operands have their own poses in their parent's frame. The result's
pose is applied after the operation. Cut faces inherit the containing element's
material slot unless `Element::cut_material` binds a separate reveal slot.

Three more nodes compose solids rather than cut them. `union` and `union_all`
fuse solids in one operation. `arrayed(count, step)` repeats a solid, copy `n`
carrying `step` applied `n` times, so a translation gives a row and a rotation
gives a ring around the local origin; `MAX_ARRAY_COUNT` bounds it.
`mirrored(MirrorPlane::new(axis, offset))` reflects a solid across an
axis-aligned plane and gives back the reflection alone, so a symmetric part is
a union of a solid and its mirror. Cut provenance follows the operands: a union
creates no cut faces of its own, every copy of an array carries the provenance
of its source, and a reflection keeps the provenance of the faces it mirrors.
Validation limits expression depth to 64, but does not evaluate
booleans, prove overlap, determine manifoldness or promise tessellation success.
A future backend must report unsupported operations explicitly.

An optional grid is an editor hint only. It neither snaps positions nor restricts
rotations.

## Sockets that attach

`Instance::attach(own_socket, target_instance, target_socket)` derives a
placement instead of stating one. **Sockets face outward along +Z**, so
attaching turns the attached part half a turn about the socket's Y axis and the
two outward directions oppose, the way two modules meet at a shared edge.
`attach_aligned` lays the frames on top of each other instead, for a socket that
marks a direction to continue in.

A bay with a `left` socket at its origin facing -X and a `right` socket at its
far edge facing +X, attached left to right, lands the next bay exactly one bay
along with the same rotation. That is what `walls` in the study kit does; only
the corner bay of each side is placed by hand. `build()` resolves attachments in
dependency order, so a target may appear later in the file, and reports unknown
sockets, unknown targets, self-attachment and cycles by path. The derived pose
is written into the instance. `placed` clears an attachment, because two sources
for one pose is an authoring mistake, not a merge.

An attachment names an instance by ID. A building assembled out of another
building's instances, as `corporate::block` is, has to rewrite attachment
targets along with the IDs it renames.

## Collision proxies

`Element::collision` declares what an element contributes to static collision:
`None` by default, `Bounds` for the axis-aligned box of the evaluated solid in
part space, or `Hull` for its convex hull. The mesher derives the proxy from the
same surface it meshed, once per part definition, and
`MeshedBuilding::colliders()` places one per instance. Trim, slats, lights and
signage stay `None` and cost nothing.

A proxy is convex and conservative: **an opening subtracted inside an element is
not subtracted from its proxy**. An opening that has to be walked through is the
gap between elements, not a hole inside one. `ashlar` itself derives nothing; it
owns the declaration and the `ConvexSolid` the backend hands back.

## Generate and preview

```sh
just preview
# Export the built-in Rust example for editing:
just preview --write-example /tmp/building.ron
just preview --recipe /tmp/building.ron --materials /tmp/city.ron --graphs /tmp/city.graphs.ron
# Bounded visual verification:
just preview --screenshot /tmp/building.png
just preview --wireframe --screenshot /tmp/building-wire.png
```

Drag left to orbit, drag right to pan, scroll to zoom. **F** toggles wireframe,
**P** toggles textured materials and neutral clay, **M** opens the material
parameter panel, **S** saves the graph library it is editing, **Home** resets the
initial framing, **Escape** exits. `--clay`, `--yaw`, `--pitch` and `--zoom` make
comparisons reproducible. Captures wait for texture dependencies to load before
counting warmup frames; texture failures return an error. Screenshot mode ignores
live view, palette and panel input. Restart after editing a recipe or a material
library. A graph library read from a file with `--graphs` is watched and
reloaded while the preview runs; the showcase's own scenes hand their graphs over
as Rust values, so there is nothing to watch until `--graphs-out` saves what the
panel tuned. `--bake` makes a library that names exported files render the
graphs they were baked from instead.

The default `--scene outpost` and the larger `--scene office` share the same
facade and entrance definitions in `examples/showcase/src/lib.rs`.
`--scene facade` and `--scene entrance` isolate the finished parts. The earlier
primitive fixture remains available as `--scene basic` in
`examples/showcase/src/example.rs`. All architectural dimensions, layouts and
style choices belong to this example content, and
[the tour of the kits](kits.md) describes them.

For custom `--recipe` files, pass `--materials` to load a material library, and
`--graphs` for the graphs its surfaces name.
Without them, the preview uses stable diagnostic colors. The built-in scenes
bring the showcase's library with them, which is Rust in
`examples/showcase/src/library.rs` over the default library in
`ashlar_material::stdlib`. Texture keys are relative to `--asset-root`, which
defaults to the repository's assets folder.
An explicitly selected library must resolve all used keys, including cut slots;
missing or corrupt PNGs fail before the window opens.

### Previewing your own content

`ashlar-preview` is a library with a thin binary over it. The scene list is a
`Catalog` the binary supplies, not a fixed enum, so a game gets the same viewer
and the same gallery over its own buildings in a few dozen lines and without
depending on `ashlar-showcase`:

```rust
use ashlar_preview::{Catalog, Options, Scene, run};

fn main() -> anyhow::Result<()> {
    let catalog = Catalog::new()
        .scene(
            Scene::new("tower", || my_game::tower())
                .materials(my_game::materials)
                .graphs(my_game::graphs),
        )
        .scene(Scene::new("plaza", || my_game::plaza()).material_library("buildings/plaza.ron"))
        .scene(Scene::new("depot", || my_game::depot()).display_name("Supply depot"))
        .default_scene("tower")
        .asset_root(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
    run(&catalog, Options::parse_for(&catalog))
}
```

`Scene::new` takes a closure, so nothing is meshed until it is asked for.
`--scene` and `--reference-scenes` accept the registered names and are validated
against the catalog, so `--help` lists exactly what was registered and an unknown
name reports the alternatives. A scene takes its libraries as Rust values with
`materials` and `graphs`, or as RON files with `material_library` and
`graph_library`, whose keys are relative to `--asset-root`. That root is this
workspace's assets folder unless the catalog names its own with `asset_root`.
Either way the library is preflighted before the window opens.
The preview writes no files a game ships; that is the content step's job,
`ashlar-content`.

## The mesh backend

The `GeometryMesher` port, the `TriangleMesh` result and the `MeshedBuilding`
that holds a whole assembly's worth of them live in `ashlar`: none of that
vocabulary names a kernel type, so a consumer reading meshed results never
compiles one.
The `ashlar-manifold` adapter implements the port using
[manifold-csg](https://docs.rs/manifold-csg/0.4.1), safe Rust bindings around the
native Manifold solid kernel. Plain/chamfered cuboids, Y-up cylinders, concave extrusions in
either winding and nested, arbitrarily rotated subtraction are supported.
Full subtraction returns an empty mesh; disjoint cutters leave the solid intact.
Kernel failures include the part/element/expression path.

```rust
use ashlar_manifold::{mesh_building, ManifoldMesher};
let output = mesh_building(&building, &ManifoldMesher::default())?;
```

Each used part is evaluated once. `MeshedBuilding` preserves instance poses,
material slots, overrides and socket metadata alongside shared part meshes.

`MeshedBuilding::pieces()` is the join between the two halves, and what a
renderer draws from: one `Piece` per instance, per element and per resolved
slot, carrying the instance's pose, the part's triangles and the `Binding` that
instance's palette resolves for that slot. A cutter's own slot comes first, then
the element's cut slot, then its main slot, and the instance's own override
where it has one. A merged building yields its group batches first; see "Merged
buildings, interiors and damage" below. Key uploaded geometry by
`Piece::mesh_key()`, and key what is derived from the geometry *and* the
material by `Piece::shared_key()`, which is that key and the binding: two
placements of one part share an uploaded mesh and a scattered strand layer, and
two values of one graph parameter are two texture sets and must not.
`ashlar_bevy::dressed_pieces` walks the same pieces with
each definition resolved on top.

Meshes use f64 positions and normals with outward winding, indexed: vertices are
shared where position, normal and UV agree within `WELD_TOLERANCE`, so creases
and UV seams still split and flat continuous surfaces do not. On the corporate
scene that is 5071 vertices rather than 7056 for the same 2352 triangles.
Welding is an optimization, not a repair: two vertices either side of a
quantization bucket edge stay separate, which costs a vertex and no geometry.
`unweld` gives every triangle its own corners back, which is what a per-face
projection needs. The preview uploads each element/surface batch once and narrows
to f32 at its renderer boundary. Default normals crease at 45 degrees and smooth
below it, and an edge at exactly 45 degrees is a crease, so a 45 degree chamfer
keeps its faces flat instead of being decided by rounding. Cylinder sides are
smooth and their caps are creased without any authoring. The explicit
`chamfered_cuboid(size, bevel)` primitive adds real edge/corner geometry while
retaining the requested bounds.
It uses planar bevels, and is not a general modifier for arbitrary boolean edges.
Bevels must be positive and smaller than half every box dimension.

UVs are metres in part space, in one of three modes per element. `Planar`, the
default, uses an orthonormal face frame: the dominant axis chooses its
orientation, angled faces are not foreshortened, and every face starts its
repeat at its own origin. `Box` reads the world axes with one origin per axis, so
coplanar faces of separate elements share a grid and their repeats line up
across a wall of modules; a face at an angle to its dominant axis is
foreshortened, as in any triplanar mapping. `Cylindrical { axis }` puts arc
length along U and height along V, so a cylinder tiles at metre scale with a
single seam, and faces perpendicular to the axis fall back to the plane mapping.
The mode is applied after evaluation. Foundation and ramp meshes use the planar
projection. Physical
texture scale comes from the material's `tile_metres`, so repeated and rotated
modules preserve density without scaling geometry. These coordinates have seams;
they are not an atlas, lightmap unwrap or seamless cylindrical mapping. Bevy
receives generated tangents for normal maps. Levels of detail are the recipe
simplified and baked into a `.ashlar` file ([ADR 0006](../adr/0006-baked-buildings-and-lod.md));
GLB export and runtime streaming remain future work.

The default adapter rejects cylinders above 4096 segments and results above one
million triangles after each solid operation. These checks are output budgets,
not hard memory/time limits on native boolean evaluation. Recipe validation
limits expression nesting to 64. Evaluation is synchronous and runs before the
preview window opens; an in-game consumer must schedule it off the main thread.
The preview caps its Bevy worker pool at four and uses vertical sync. It rejects
empty assemblies and coordinates outside +/- one million local metres.

### Whole repeats across a stacked or arrayed module

A part is meshed once, in its own frame, and every instance of it starts its
UV repeat at zero — the mesh is shared and cannot know where it will stand. So
when several instances of one part stack into a storey or array into a bay run
to read as one continuous surface, that surface only stays continuous if the
module is a whole number of the material's `tile_metres`: a storey of two
repeats reads as one wall, and a storey of 1.6 repeats shows a cut at every
floor line, however good the material. `UvMode::Box` does not reach this — it
lines up coplanar faces *within* one part around one shared origin, not across
separate instances, each of which carries its own copy of the mesh and starts
its own origin at zero. Until a structural answer lands, size the module to
the material instead: pick `tile_metres` so a whole number of repeats divides
the module (the storey height, most often), rather than the graph's own
authored repeat. Read alone, that disagreement is what `ashlar_bevy::tiling`
cannot tell apart from a `tile_metres` typo, so declare it: `tile_scale` is
the factor by which the definition scales the graph's own repeat on purpose.
The corporate kit's stone wall tiles `library:stone-cladding` at 1.9 m — two
whole repeats over its 3.8 m storey — against the 2 m the graph was authored
for, and writes `tile_scale: Some(1.9 / 2.0)` beside `tile_metres` to say so;
see `corporate_materials` in `examples/showcase/src/library.rs`. The structural fix — a
building-space or merged mapping that does not care where an instance's local
UV started — is
[ADR 0005](../adr/0005-merged-geometry-and-damage.md), described under
"Merged buildings, interiors and damage" below.

### Native build prerequisite

The recipe domain remains Rust-only. The mesher requires Git, CMake and a C++
compiler, except when building documentation: `manifold-csg-sys` returns from
its build script under `DOCS_RS`, and rustdoc links nothing, so docs.rs needs
neither. The pinned adapter version currently builds Manifold v3.5.3 and fetches
its source and Clipper2 on the first build; `cargo fetch` alone does not cache
these native sources. Subsequent builds reuse the target directory. See the
upstream `MANIFOLD_CSG_LIB_DIR` override for a prebuilt/offline installation.
The adapter's parallel feature is disabled. For constrained machines, limit
Cargo jobs and CPU affinity for a clean native build; upstream CMake invokes
`--parallel` without an explicit job count.

One toolchain is known to build a kernel that crashes.
An activated Anaconda environment exports `CC` and `CXX`, its GCC emits the kernel's static initialisers into a legacy `.ctors` section,
and `rust-lld` never runs them, so `Manifold::sphere`, `refine` and the smoothing calls die on SIGFPE.
Nothing `ashlar` evaluates reaches those calls, which is why a blast solid is a hull of our own points.
Build the kernel with the system compiler in that environment:
`export CC=/usr/bin/gcc CXX=/usr/bin/g++` on Linux, using the appropriate paths on other platforms.
The workspace leaves compiler selection to the caller, including on Windows, NixOS and when cross-compiling.
Alternatively, link with `-C link-arg=-fuse-ld=bfd`.
`crates/ashlar-manifold/tests/kernel_init.rs` is the canary: it calls `Manifold::sphere`, and on a bad build the test process dies rather than failing.
To confirm it on a binary that crashes, `readelf -p .comment` names the C++ compiler that built the kernel,
and the kernel's four `_GLOBAL__sub_I_*` initialisers sit in a `.ctors` section while `.init_array` holds only Rust's;
GNU ld folds `.ctors` in and `rust-lld` does not. CMake caches its compiler, so rebuilding with another needs `cargo clean -p manifold-csg-sys`.
Inside this workspace `RUSTFLAGS` replaces the target features `.cargo/config.toml` sets,
so there the `bfd` workaround is `RUSTFLAGS="-C link-arg=-fuse-ld=bfd -C target-feature=+sse4.1,+fma"`.

## Merged buildings, interiors and damage

By default a building is drawn as the parts it was authored from.
Every element is meshed alone, a part's meshes are shared between its instances, and nothing is unioned.
That is cheap and it instances well.
It is also why a wall of bays has internal faces nobody sees, coincident faces that fight for depth,
normals that break at every joint, and a texture that starts again at every storey.

[ADR 0005](../adr/0005-merged-geometry-and-damage.md) adds the other way to draw one,
and everything in this section follows from a single fact it made the mesher keep:
every face knows which solid it came from.

### Every face names its source

`TriangleMesh::sources` holds one `FaceSource` per triangle, an `operand` and a `FaceOrigin`.
The origin is `Body`, `Cutter(k)` or `Damage(j)`, and `TriangleMesh::is_cut(face)` is the old cut flag asked as a question.

The operand is an index, not an instance name, and this may look like the wrong way round.
The reason is that a mesher is handed geometry and nothing else, so it cannot know what an instance is.
The caller holds the table from operand to `(instance, element)`, and for a merged group that table is `MergedGroup::operands`.

Cutters are numbered depth first, and `Geometry::cutters()` *is* that numbering.
A backend handing out `Cutter(k)` and a resolver looking cutter `k` up read the same function, so they cannot disagree.

### A cutter may wear its own reveal

Two openings in one wall used to share one cut slot.
A cutter can now name its own with `Geometry::cut_material`:

```rust
let wall = Geometry::cuboid([6.0, 3.0, 0.3])
    .subtract(door)                                 // falls back to the element's cut slot
    .subtract(window.cut_material("sill"));         // wears its own
```

`Element::slot_for(origin)` resolves a face: the cutter's slot, then the element's `cut_material_slot`, then its `material_slot`.
A cutter slot needs a binding like any other slot, and `build()` refuses a recipe that leaves one out.
`mesh_building` yields one `ElementMesh` per resolved slot, so an element may now come back as several cut batches.
That is why uploaded geometry is keyed by `Piece::mesh_key()` and no longer by a cut flag.

### Merge groups

```rust
let tower = Building::builder("metro:tower")
    .merged()
    .group(MergeGroup::new("storey-1").storey(1))
    .instance(Instance::new("bay-a", "metro:bay").group("storey-1"))
    // ...
    .build()?;
```

A merged building unions each group into one solid and meshes it once, in building space.
An instance that names no group is in the default group, which is the building itself.
`Element::standalone()` keeps an element its own mesh whatever group its instance is in;
use it for glass, lights, doors and anything else that moves or is swapped.
`Element::far()` is the level-of-detail twin of that:
a coarser level drops what is thinner than its feature size, and a far element stays anyway,
which is what a neon line or a crown of lights wants.

What comes back is `MeshedBuilding::groups`: a `MergedGroup` per group, split into a `GroupBatch` per binding and side.
The material follows the face and not the mesh it happens to be in.
Each face resolves its binding through its own instance's palette, exactly as `Building::binding` resolves it per element,
so an instance override survives the union.

Parts stacked exactly face to face union with no internal wall and no seam triangles.
A gap of a nanometre is still a valid result, but it is two components, so author joints to touch.

Same-facing coplanar faces are the other case: when two elements of one part put faces on one plane in two materials,
drawn as parts they fight for depth, and merged the kernel gives each coplanar triangle to one operand,
so slivers of one material show in the other.
We offset the trim by a centimetre, or give both the same slot.
Faces on one plane facing opposite ways are two solids touching, and those are fine.

UVs are projected in the building frame.
`Planar` and `Box` elements are box-mapped there, which is continuous across every former joint
and does not move when a later cut re-triangulates a wall.
A `Cylindrical` element is still wrapped around its own axis, in its own part frame, so a column stays a column.

Normals are computed by `ashlar::shade_normals` and not by the kernel.
The kernel's own pass was nearly the whole cost of extracting a mesh, and ours is the same crease rule.
An edge at the sharp angle is a crease, so a 45 degree chamfer keeps its faces flat however the union cut them.

**Merging gives up instancing, and that is a real cost.**
A merged group is a unique mesh, and its triangle count goes *up* by a few per cent because the union re-triangulates.
Measured in a release build, the corporate tower merges by storey in about 140 ms where the unmerged path takes 3.
So `merged` is off unless a recipe says otherwise, a repeated kit may well stay unmerged,
and a far level of detail can be the old shared parts.

**The group is also the unit of damage.**
A hit re-meshes the groups it touches and no others, so group size is the cost of a hit.
A storey or a bay is the natural size, and a group boundary is a seam again,
so put it where a seam is honest: at a floor line, not across a wall.
`corporate::merged_by_storey` in the showcase is the worked example, and `just preview --scene corporate-merged` shows it.

A `GeometryMesher` that cannot union keeps the default `mesh_group`, which answers an error by path.
It still meshes every unmerged building.

### Pieces

`MeshedBuilding::pieces()` yields group batches first and element pieces after them.
A `Piece` carries a `PieceOrigin`, which is `Element { instance, part, element, slot, is_cut }` or `Group { group, batch }`,
beside its `storey`, `side`, `pose`, `mesh` and `binding`.
A group piece's pose is the identity, because a group is already in building space.

`Piece::mesh_key()` says what uploaded geometry may be shared by.
Element pieces over one part, element and slot share their triangles, and a group batch is shared with nothing.
`Piece::label()` is a readable name for an entity or a diagnostic.
`ashlar_bevy::drawables` handles both kinds, so a game that spawns through it needs no change to draw a merged building.

### Interiors

Walls and floors stay elements, and only openings are carved.
Carving a room out of a mass would be shorter to write, and it would make the mass's convex proxy fill the room.
The colliders of the previous section stay correct only while solids are *added*.

The library does not cull. It classifies, and the policy is the game's:

- `Element::interior()` declares a side, and a group is split by binding *and* `Side`.
  A liner solid over the inner face of a wall unions in with no seam, and its faces are their own `Interior` batch
  even when the wall and the liner wear the same plaster.
- A group's `storey` reaches every `Piece`, group and standalone alike.
- `Geometry::portal(id)` on a cutter publishes the opening.
  `MeshedBuilding::portals()` answers one `Portal` per placement, in building space:
  a rectangle at the middle of the wall's thickness, clipped to the wall, with its normal and its storey.
- `Room` is a declared box with a pose, a group and the portal ids that open onto it.
  `Building::room_at(point)` and `Room::contains` need no kernel, so a server can ask which room a player is in with `ashlar` alone.

Side is declared and not detected, and this is deliberate.
A detected answer would change when a blast opens a room to the sky; a declared one does not.
The price is that it is per element, so a floor slab declared exterior is exterior on its top face as well.
Give it a thin liner over the face that is inside, which is what the walls already do.

A portal is a rectangle in this version, so an arched opening publishes its bounding rectangle.
It is measured against the core where the cutter removes material, not against the bounds of the whole solid.
A doorway through one leg of an L-shaped element is therefore a portal, at the middle of that leg.
A portal cutter cannot sit under an `Array` or a `Mirror`, because its frame would not be one rigid pose.

`just preview --scene interior` is a small house built this way.
`PageUp` and `PageDown` move the highest storey shown and `I` hides the exterior,
which is a top-down game's whole culling policy in a dozen lines.

### Damage is data

```rust
let mut log = DamageLog::default();
log.push(Damage::blast([6.0, 1.5, 0.15], 1.2, "rubble"));
let meshed = mesh_building_with_damage(&building, &log, &ManifoldMesher::default())?;
```

A `Damage` is a solid, a pose and the slot its exposed faces wear.
A `DamageLog` is an ordered list of them, and it is both the save format and the wire format:
a server sends "this solid, here", and every client derives the same hole.
The slot is resolved against the building's own palette, because a hole belongs to no instance.

`Damage::blast` builds its solid with `Geometry::ball`, which is a convex hull of points we generate ourselves.
This may seem like a roundabout way to make a sphere.
The reason is that the point set is then identical on every machine,
and a hull needs none of the kernel entry points that crash on some toolchains (see the native build note above).

`mesh_building_with_damage` is what a load, a late-joining client and a replaying server call, and it is `GroupSolids::build`.
`GroupSolids::apply` keeps one kernel solid per group, so a hit subtracts from what is already unioned, and it is the path for play.
The two are one code path, because `build` replays each record through `apply`, so they cannot drift.
They live in `ashlar-manifold` because keeping a solid means naming a kernel type.

A replay and a played hit agree on volume, area, bounds and the area each source contributes.
They do *not* promise the same triangulation, so nothing may key on triangle order or count.

A hit on the tower's largest storey group is about 9 ms in a release build, debris included.
That belongs on a task and not in a frame, and `GroupSolids` is `Send` so that it can be.
Apply hits one at a time and in order; the log is the order things happened in.

`GroupSolids::apply` answers a `GroupHit` per group the solid actually changed:

- `group` is the re-meshed `MergedGroup`. Feed it to `MeshedBuilding::replace_group`, and `record` the damage once.
- `debris` is one `Debris` per connected piece of what was removed, with its volume and centre.
  Its faces kept their sources, so the outside of a chunk of brick wall is still brick and the fracture wears the damage slot.
  Breaking it up further is out of scope.
- Exposed faces are classed `Interior`, because what a hole exposes is the inside of something.

A hit may also be `collapsing`: `Damage::collapsing` makes it drop what it cut free.
After the solid is subtracted, a connected piece is supported when it reaches the ground or touches something supported, of its own group or another, and what is not supported falls.
Support crosses groups, so cutting the storey under another drops that one too.
A piece that was already loose in the undamaged building is left alone unless a hit changed it, though a rail or a sign stood off its wall may well be one a game wants to fall with its storey.
A single column still holds a storey up, so this is geometry and not structural analysis.
The flag is on the record rather than in the mesher, and a replay and a played hit are one code path, because the log is the wire format and every client has to derive the same building.

Collision follows. A convex proxy cannot have a hole in it,
so a group that has been hit gets `MergedGroup::collider`, a `TriangleCollider` built from the same solid.
`MeshedBuilding::colliders()` then leaves that group's proxies out, and `mesh_colliders()` yields the replacements.

Damage cuts merged groups only.
`MeshedBuilding::standalone_touched` names the standalone pieces inside a hit,
and whether glass shatters or a door falls off is the game's decision.
`MeshedBuilding::groups_touched` is a bounds test with no kernel in it,
so a server that trusts its clients can still say which chunks a hit invalidates.

Who computes the shape is a deployment choice.
A server that must know it links `ashlar-manifold` and pays for a C++ toolchain;
`cargo run -p ashlar-manifold --example damage_server` is that server, and it links no Bevy.
`ashlar-bevy` never links the kernel: `batch_drawables` draws the batches a hit produced, wherever it was computed.
In the preview, shift-click on a merged scene blasts it, and `--blast x,y,z` does the same for a capture.

## PBR materials and exposed cuts

`MaterialLibrary` is a serializable map from opaque material keys to
`MaterialDefinition`. It is separate from the recipe and contains no building
style enum. Every definition carries the constants:

- Opaque sRGB base color multiplier.
- Roughness and metallic multipliers in 0..=1.
- Linear HDR emissive color; emission is visible but does not illuminate neighbours.
- Physical texture repeat size in metres and UV offset in repeats.
- An optional `tile_scale`, saying that the repeat above is deliberately a
  multiple of the graph's own authored size rather than a `tile_metres` typo —
  see "Whole repeats across a stacked or arrayed module" below.

Where the varying detail comes from is one `surface` field, a `Surface` enum:

```ron
"game:signal": (base_color: (0.55, 0.24, 0.045), roughness: 0.55, surface: Plain),
"library:formed-concrete": (
    tile_metres: (2.0, 2.0),
    surface: Graph((graph: "library:formed-concrete", params: {"seams": Float(1.0)}, resolution: 512)),
),
"game:concrete-files": (
    tile_metres: (2.0, 2.0),
    surface: Files(
        base_color: Some("materials/library/formed-concrete/base.ktx2"),
        normal: Some("materials/library/formed-concrete/normal.ktx2"),
        orm: Some("materials/library/formed-concrete/orm.ktx2"),
        height: Some("materials/library/formed-concrete/height.ktx2"),
        baked_from: Some((graph: "library:formed-concrete", params: {}, resolution: 512)),
    ),
),
```

- `Plain` is constants only: glass, lights and trim.
- `Files` names files on disk. `base_color` is sRGB and multiplies the
  `base_color` constant; `normal` is a linear tangent-space map, read in the
  bake's convention when `baked_from` is present and as OpenGL +Y when it is
  not, so a painted map leaves `baked_from` out and an exported one keeps it; `orm` is
  linear packed R=occlusion, G=roughness, B=metallic; `height` is a linear
  relief field; and `emissive` is linear HDR that multiplies the `emissive`
  constant, which defaults to zero, so a material naming an emissive map raises
  that constant too or it glows not at all. Any of the five may be omitted.
  `baked_from` records the bake a content step ran to write them, so the step
  can be re-run. A map is a PNG or a KTX2, and a baked one should be a KTX2:
  the container carries the whole mip chain, where a PNG carries level 0 and
  Bevy builds no chain for an image it loaded, so a PNG set is one the hardware
  point samples as soon as the wall is far away. KTX2 is also the only one of
  the two that can hold the sixteen-bit height plane at all. The export writes
  zstd-supercompressed KTX2, and `ashlar-bevy` enables Bevy's pure-Rust decoder
  for it, so a game needs no C library to read one.
- `Graph((graph: "library:formed-concrete", params: {...}, resolution: 512))`
  names a material graph to bake into images when the material is registered.
  It is what every definition of the default library is, and it is for
  authoring: the preview and the tool features bake it, and a game with
  `ashlar-bevy`'s default features refuses it. A content step exports these
  definitions as `Files` files; see [the integration guide](integration.md).
- `Shader(graph: "showcase:concrete-wet", params: {...})` names a graph to
  compile, for parameters that move per frame and for the things no image can
  hold at all — the clock, the fragment's world position, its normal. The
  showcase's two worked examples are `showcase:light`, over a graph whose emissive is a sine of
  `Time`, and `showcase:concrete-wet`, which *instances* the graph the walls are
  baked from and gates a live `wetness` by the fragment's own world normal and height,
  so the same material is wet on a coping and dry on the wall under it. A bake
  refuses a world-space input rather than folding it to zero, which is why that
  is a second graph over the first rather than one more parameter on it.

A surface refuses a field it does not know, so a misspelled map key is a parse
error naming the field rather than a surface that silently loses every map it
meant to name. `Graph` doubles its parentheses because it wraps the same `Bake`
a `Files` surface records under `baked_from`.

The last two are the [procedural material](materials.md)
path. `ashlar_bevy::read_library_with_graphs` takes a `MaterialGraphLibrary` beside the
material library — `ashlar_bevy::read_graphs` reads one from RON — and
`check_library_with_graphs` does the same for a library already in memory, such as
`ashlar_material::stdlib::materials()` merged with a game's own. Both preflight every
`Graph` and `Shader` surface by validating and lowering its graph, so an unknown
graph key, a parameter the graph does not declare, a cycle or a node the backend
cannot lower is a startup error naming the material and the path inside the
graph; a `Shader` surface is partitioned and printed as WGSL on top of that, so
a live set the backend cannot honour or a graph that binds more than eight
textures is refused there too. A `Shader` surface is then compiled by
`ashlar_bevy::shader::create_shader_material` into a `ProceduralMaterial`, which is a
`StandardMaterial` with a generated fragment over it; add
`ashlar_bevy::shader::ProceduralMaterialPlugin` to an app that may draw one. A
`Graph` one is baked into images when the material is created, by
`ashlar_bevy::runtime_bake::create_graph_material`, which takes the graph library, a
`BakeCache` and `Assets<Image>` beside what `create_material` takes; the images are the
bytes, formats and mip chain the KTX2 of that same `Bake` would carry, and two
definitions naming one `Bake` share one texture set, so a per-building seed
costs a set in memory and no file on disk. Plain `create_material` has nothing to bake
with and hands a graph surface back as its constants. A bake resolution must be
a power of two in 256..=4096, and parameter values must be finite; both are
rejected by path.
`baked_from` on a `Files` surface is provenance rather than a dependency, so
it is not lowered: the files it names are already on disk and are what load.

The renderer applies repeat sampling and validates color-space use. A single
image key cannot be reused as both sRGB color and linear data, and a `height`
map is preflighted but not uploaded — a runtime bake keeps its height plane on
the cache entry and binds it to no slot either, for the same reason. Bevy 0.19 does have a relief slot,
`StandardMaterial::depth_map`, but it reads black as the top surface where these
height fields read white as the top, and filling it also means choosing a
parallax depth in the mesh's own units that a 0..=1 field does not carry; both
wait for the bake.

Texture generation is not part of the core, and nothing baked is committed.
The source of every material is its Rust graph: the default library in
`crates/ashlar-material/src/stdlib/`, and the showcase's four compiled surfaces
in `examples/showcase/src/materials.rs`. A game that wants files rather than a
bake at registration runs the content step, `ashlar_material::export`, over its
definitions; it answers the texture sets, the strand sets and a library whose
surfaces name those files with `baked_from` beside them. The showcase's writer
is `examples/showcase/src/export.rs`:

```sh
just materials                  # every material, into the ignored assets/materials/
just materials 2048 brick png   # one, at 2048, with level-0 PNGs to look at
```

Glass is opaque reflective glazing; transparency, transmission and decal
blending are not implemented in this first contract.

```rust
let panel = Element::new(
    "body",
    Geometry::chamfered_cuboid([4.0, 4.8, 0.8], 0.03)
        .subtract(Geometry::cuboid([2.0, 2.0, 1.0])
            .placed(Pose::at([1.0, 1.5, -0.1]))),
    "painted",
).cut_material("exposed");
```

Bind both slots through recipe defaults or instance overrides. Manifold surface
provenance identifies faces introduced by cutters, including nested expressions;
this is not a position-based guess. The output retains one cut flag per triangle.
`mesh_building` yields one batch per resolved slot, keyed by
`MeshKey::Element { part, element, slot }`, as "A cutter may wear its own reveal" above
describes. Without a cut slot, all faces share the original element slot.

A second slot is not the only way. A material graph can read the flag itself
through its `CutFlag` node — `ashlar-bevy` uploads it as a per-vertex attribute,
and `TriangleMesh::weld` splits the vertices either side of a cut so the value a
fragment reads is exactly one or exactly zero — and darken or roughen the faces
a cutter made without a second element, a second mesh or a second key. That
wants a `Surface::Shader`, since a bake is one texture set for both kinds of
face; `showcase:concrete-cut-aware` in the showcase is the worked example, and the
study kit keeps `cut_material` on its entrance bay because a reveal that really is a
different material still wants one.
Changing a library or instance material override does not change its mesh.

## One material key, a value per instance

A slot binds a material key, and an instance may override that binding. An
override may also carry **parameter values of its own**, for the
graph the material's surface names:

```ron
instances: [
    (id: "tower/bay-0", part: "metro:bay", pose: (...), materials: {}),
    (id: "annex/bay-0", part: "metro:bay", pose: (...), materials: {
        "stone": (material: "metro:stone", params: {"variation": Float(0.37)}),
    }),
],
```

A binding is therefore one of two forms, and both are the same field: the bare
key `"metro:stone"` where nothing is overridden, which is what every recipe
written before this existed holds and what a binding with no overrides still
writes itself back as, and `(material: "metro:stone", params: {...})` where
something is. In Rust it is `ashlar::Binding`:

```rust
instance.binding(
    "stone",
    Binding::new("metro:stone").param("variation", ParamValue::Float(0.37)),
)
```

What it is for is variety without a library entry per building. The corporate
block is the worked example: three buildings over one
`showcase:corporate-stone`, which is `library:stone-cladding` at a `variation`
that moves the whole surface to another piece of the same endless wall, and each
building names its own value on its own instances. One definition in
`examples/showcase/src/library.rs` is bound by every bay of all three.

The rules, and where each is enforced:

- **Only a surface that binds graph parameters takes them**, which means
  `Graph` and `Shader`. A `Plain` or `Files` surface has nothing to override
  — a `Files` surface's `baked_from` is provenance rather than a request, and
  the files it names are what load — and `MaterialLibrary::check_for` refuses
  one by the path of the binding, `instances[<id>].materials[<slot>]`.
- **The names must be parameters the graph declares**, at the type it declares
  them. Only a crate holding the graph library can know that, so
  `ashlar_bevy::read_library` checks it, by the same path, by lowering the overridden
  surface exactly as it lowers a definition's own.
- **An override replaces values and nothing else.** The graph, the bake
  resolution and the definition's own constants — base colour, roughness,
  `tile_metres` — stay the library's. A per-instance *tint* is therefore not
  this: a tint multiplier lives on the definition, so two tints are still two
  keys.
- **An instance that overrides a slot replaces the palette's binding for it**,
  rather than adding to it, exactly as an override of the key alone does.

What it costs is what the value reaches. `ashlar_bevy::definition(&library,
binding)` answers the definition with the overrides written into it, and
everything below that deals in definitions:

- A **`Graph`** override is a different `Bake`, so it is a different
  `BakeKey` and its own texture set in memory — and every instance that asked
  for the same one shares it, which is what makes a hundred bays over three
  seeds three sets.
- A **`Shader`** override of a *live* parameter is the same pipeline, the same
  shader and the same bound textures with a different uniform block, which is
  the cheapest thing in this system: nothing is rasterised and nothing is
  rebuilt, because none of them ever saw the value.
- A **`Shader`** override of a *folded* parameter is a second compiled graph,
  because folding a parameter puts it into the bound textures and into the
  generated text. Allowed, counted, and the reason the cost report exists.

## Fit buildings to terrain

`GroundingSpec` declares a support footprint, floor, slab underside and entrance
`Socket` (+Z outward). `GroundingPolicy` supplies limits. These plain types and
`fit_ground` live in `ashlar`; they contain no architectural palette, world
source, physics engine or renderer. Content-specific dimensions, limits and
static collision proxies live with the content, as they do in
`ashlar-showcase`.

The caller gathers a `TerrainPatch` of world-aligned triangles, typically a
one-metre grid matching its own LOD0 collision, and hands it to the pure solver.
`TerrainPatch::with_diagonal` says which corner pair each cell splits along; a
patch read along the other diagonal is a different surface from the one the
game's own players walk on. `new` keeps the anti-diagonal.
That solver clips the triangles to the yawed footprint, every support segment
and both approach sections, so heights between corners participate in extrema
and ramp clearance. It raises the floor, embeds segmented perimeter supports,
and searches for a level landing plus a straight grade-limited ramp. Invalid
inputs, missing patch coverage, excessive support depth and unreachable
approaches return typed errors. Larger footprints need a larger caller-supplied
patch.

`Grounding` supplies a world pose, convex solids, conservative vegetation bounds
and diagnostics. `ashlar_bevy::ground_mesh` draws those exact solid corners, and
a physics adapter converts them to convex static collision. The shared render
adapter also owns metre UV upload, tangents, material preflight and PBR texture
loading. The native Manifold kernel stays in authoring, so a headless runtime
that consumes fitted results sees only pure data and collision.

Levels of detail are baked ahead of time (ADR 0006), and there is no live site
refitting or building streaming yet. Moving doors and terrain modification are
separate future work.

## The showcase kits

The `outpost`, `office`, `facade` and `entrance` scenes are one kit. The
`corporate`, `corporate-annex` and `corporate-block` scenes are a second kit and
palette in `ashlar_showcase::corporate`. Thirteen part definitions produce
different heights and rotated courtyard wings. The shared geometry builders, the
mesher and the render adapter have no corporate-specific behaviour.

```sh
just preview --serial-pipelines --scene corporate-block
```

[The tour of the kits](kits.md) shows each of them.

The `sheet-<name>` scenes are not buildings. There is one for every baked
material of the default library, `sheet-brick` to `sheet-wood-floor`, and each
is that material on a ball and a cube at three texel densities, for looking at a
surface rather than at a shape. Every column is a pair — the graph baked at 2048
when it is registered, and the same graph compiled — so that "a graph with no
live parameter is the same picture either way" is something somebody can check
by looking. `detail-<name>` is the same material on one two-metre patch, close
enough to judge a joint or a blade.

## Reference gallery

`just references` renders every scene at three fixed angles, headlessly
in one process. Open `target/references/index.html`. Use
`--reference-scenes corporate` for a subset. The gallery is for visual
inspection: there is no pixel baseline comparison, because coincident-edge draw
ordering and driver revisions move pixels for reasons that have nothing to do
with the geometry. The studio is fixed: 800×600 at MSAA 4×, front, rear and
elevated views framed at 1.4 times the model's bounds diagonal, twelve warm-up
frames after textures load and after each camera move, serial shader
compilation and direct drawing, and a 120-second timeout that fails the run.
Direct drawing is there because indirect draws were seen to reorder coincident
edges between runs.

## Stability

`Shape`, `GroundError`, `UvMode`, `Collision`, `Facing` and `Diagonal` are
`#[non_exhaustive]`: a consumer matches on them, and a new node, failure mode or
projection must not break that match. A backend rejects by path what it does not
implement. The crate re-exports the `glam` it was built against as
`ashlar::glam`; a consumer that resolves a second glam gets a `DVec3` that is a
different type from the one in a recipe, and converts at every boundary.

## Next boundary

General edge bevels, authored UV atlases, layered wear/decal materials, convex
decomposition and GLB export can build on this evaluator. A later macro can construct the same
types without becoming their only authoring interface.
