# ADR 0005: Merged geometry, face provenance, and damage as data

Status: accepted, 2026-09-20, and implemented as decided. The timings this
decision rests on were measured first, and are at the end; what building it
changed is marked where it changed.

## Context

A building is drawn as the parts it was authored from. `mesh_building` meshes
every element on its own, shares a part's meshes between its instances, and
never unions anything, so a tower is a pile of closed solids that touch. That
is cheap and it instances well. It is also why a wall of bays has internal
faces nobody sees, coincident faces that fight for depth, normals that break at
every joint, and a texture that starts again at every storey: a part is meshed
once in its own frame and cannot know where it will stand.

Three things are wanted that the pile cannot give. A building that is one
continuous surface. An interior, with its own finishes and a way to draw or
hide it. And damage: a hole anywhere, made during play, that a server may have
to know the shape of.

They look like three features. They are one missing fact. The kernel knows
which input solid every output triangle came from, and the mesher throws that
away for everything except the cut flag.

## Decision

### Every face knows where it came from

`TriangleMesh` carries a source per face instead of a `bool` per face. A source
names the element, the instance that placed it, and whether the face came from
the element's own surface or from a named cutter. `cut_faces` becomes the
question "is this face's source a cutter", so nothing that reads it changes
meaning.

The material follows from the source, not from the mesh the triangle happens to
be in. The element's slot, the cut slot, and the instance's own binding are
resolved per face exactly as `Building::binding` resolves them per element
today. `MeshedBuilding::pieces()` keeps its shape; a piece is now a batch of
faces that share a binding, not a whole element.

This may look like bookkeeping. It is the whole design: once a face knows its
source, merging cannot lose a material, a cutter can carry its own finish, and
debris knows what it is made of.

### A merge group is unioned, and it is also the chunk

A building declares merge groups: sets of placed elements that are unioned into
one solid and meshed once, in building space. The default group is the
building. An element opts out by declaration, because glass, lights, doors and
anything that moves or is swapped must stay its own mesh.

The port grows one method. `GeometryMesher::mesh` takes one geometry;
`mesh_group` takes placed geometries and answers one `TriangleMesh` with
sources. A backend that cannot union answers an error, and the unmerged path
remains for it.

**The group is also the unit of damage.** A blast re-meshes the groups it
touches and no others, so the grouping an author chooses for seams is the
grouping that bounds the cost of a hit. A storey or a bay is the natural size.
A group boundary is a seam again, so it belongs where a seam is honest: at a
floor line, not across a wall.

**Merging gives up instancing, and that is a real cost.** A merged group is a
unique mesh. A street of identical towers merged per building is a street of
unique meshes. So merging is a per-building choice, a repeated kit may stay
unmerged, and a far level of detail may well be the old shared parts.

### UVs are in building space

A merged mesh knows where every face stands, so its UVs are projected in the
building frame. `UvMode::Box` there is continuous across every former joint,
and it does not move when a cut re-triangulates a wall, which per-face planar
mapping would. Unmerged parts keep today's modes.

### Interiors are authored additively, and classified

Walls and floors stay elements. Carving a room out of a mass would be shorter
to write, and it would make the mass's convex proxy fill the room; ADR 0002's
colliders stay correct only while solids are added. Openings are carved, as
they are now.

A liner solid over the inner face of a wall unions in, and its faces wear its
slot, which is inside-and-outside finishes with no seam. A cutter may name a
slot of its own, so two openings in one wall need not share a reveal.

The library does not cull. It classifies: a batch says whether it is exterior
or interior and which storey it belongs to, an opening's cutter is published as
a portal polygon, and a declared room volume is published as data. A top-down
game hides the storeys above the player, a first-person game walks the portals,
and a third-person game ignores all of it. That policy is the game's.

### Damage is data

A damage record is a solid, a pose and a slot for the faces it exposes. A
building's damage is an ordered list of them. Applying one subtracts it from
the groups it touches and re-meshes those. The list is the save format and the
wire format: a server sends "this solid, here", and every client derives the
same hole.

The volume removed is the intersection, and its faces carry their sources, so
debris is cut from it with the right materials on it.

**Collision for a damaged group is its triangle mesh.** A convex proxy cannot
have a hole in it. Undamaged elements keep their proxies; a group that has been
hit answers with a mesh collider built from the same evaluated solid.

**Who computes that is a deployment choice, and the ADR keeps it one.** A server
that must know the shape links `ashlar-manifold` and pays for a C++ toolchain.
A server that trusts its clients, or has no damage, still depends on `ashlar`
alone. A kernel-free answer, a distance-field evaluation of `Geometry` in
`ashlar` that a hole is one `max` in, stays open and is not decided here.

## What was measured

A spike on the corporate kit, 2026-09-20. The
kernel's C++ is always an optimised build, so dev and release agree to within
noise for everything below.

**A storey-sized group merges in tens of milliseconds and takes a hit in under
twenty.** The slowest group of the tower, 156 solids and 9,900 triangles, unions
and extracts in 68 ms. Subtracting a blast from one group and re-meshing it is
17 to 19 ms, of which the subtraction is 4 to 5 and the rest is the kernel
computing normals. The debris intersection is 2 to 4 ms. The whole block as one
group is half a second per hit, which is the argument for groups in one number.
So arbitrary holes hold, at the granularity this ADR chose, and a hit belongs on
a task rather than in a frame.

**Parts stacked exactly face to face union cleanly.** No seam triangles, exact
volume and area, and a third of the surface area gone as internal faces. The
triangle count goes *up* by about a third, because the union re-triangulates;
merging buys a continuous surface, not a cheaper one.

**Provenance needed one decision, and building it made the decision easier than
the spike did.** Copies of a shared part carry one original ID, so without help
between 0 and 8 per cent of merged triangles map back to a placement. The spike
fixed that with `as_original` on each placed copy, which erases the cutter IDs
that mark cut faces, and left how the cut flag survives open between three
alternatives. Two of them are wrong before they are slow. Matching a merged
triangle back by source and plane cannot say *which* cutter, because two windows
in one wall share a sill plane, and a cutter's own slot needs exactly that.
Telling copies apart by a run's transform is ambiguous whenever an array step
inside a part equals the spacing between its instances. The third needs no
`as_original` at all: every element is evaluated once per placement, every
evaluation constructs fresh primitives, fresh primitives get fresh original IDs,
and the evaluator already records which IDs sit under which cutter. So a run ID
names one operand and one origin by construction. Its price is inside the
numbers below: the tower merged by storey is 141 ms in a release build against
the spike's 160, and the block 291 against 321 to 335.

**A property channel was the fourth way, measured and not taken.**
`set_properties` survives a boolean exactly, and a cutter's faces carry the
cutter's tag, so one tag per placed copy gives identity and the cut mark
together. It cost extraction 2.2 times on the tower and 7.2 times on the block
in a dev build (2.4 in release), because the kernel re-runs a Rust closure per
vertex, through an FFI trampoline, during extraction.

**Normals left the kernel.** `calculate_normals` was nearly all of the
extraction cost and grows faster than the triangle count. `ashlar::shade_normals`
derives them on our side from the same sharp-angle rule, angle weighted so the
answer does not depend on how a flat face was split. With it, a hit on the
tower's largest storey group is about 9 ms in a release build, debris, batching
and collider included. A dev build is slower than the spike's, because that pass
is now unoptimised Rust where the kernel's was always optimised C++.

**One blocker, and it is not in the design.** `Manifold::sphere`, `refine` and
the smoothing calls crash with SIGFPE on this machine. The kernel's C++ is
compiled by an Anaconda GCC that emits legacy `.ctors`, `rust-lld` never runs
them, and four static initialisers stay unrun. Linking with `bfd` fixes it, and
so would building the kernel with the system compiler. Every shape the recipes
use today avoids those calls, which is why nothing had hit it. A game that
spawns a kernel sphere as a blast solid would. It was settled twice. This
workspace builds the kernel with the system compiler, and
`crates/ashlar-manifold/tests/kernel_init.rs` is the canary. And the library
never reaches those calls at all: a blast solid is `Geometry::ball`, a convex
hull of points we generate, which is also what makes it the same solid on every
client.

## Out of scope

Convex decomposition. Structural collapse, though the attachment graph of ADR
0002 is what it would query. Fracture patterns for debris. A web build, where
the native kernel is an open question. A second mesher backend.

Two kernel levers were never measured: `Manifold::with_context`, which makes an
evaluation cancellable and is the obvious way to budget a hit on a task, and the
adapter's `parallel` feature, which pulls TBB and would thread the union (350 ms
for the whole block in the spike, single-threaded).
