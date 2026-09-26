# The Bevy adapter, in depth

The long form of `crates/ashlar-bevy/README.md`: how the adapter joins meshes
and materials without the plugin, what its preflight checks, how the tool
features bake and compile material graphs, and how the two bake backends are
held to one answer. The README is the page a game reads; this is the page a
contributor reads.

Some timings below were measured on the retired study kit, whose graphs have
since become the default library's. They are kept because they are what the
decisions were made on.

## Joining meshes and materials

`drawables` is the join done once. It walks `dressed_pieces`, uploads each distinct
mesh with `mesh` and creates each distinct material with `create_material`, and the two
are deduplicated by two different keys on purpose: geometry by
`Piece::mesh_key()` (part, element and resolved slot), because a part is
evaluated once however many times it is placed, and materials by the whole `Binding` rather than by the material
key, because an instance may write its own parameter values over the definition
a key names and two values of one parameter are two materials. Keying materials
by the key is the bug this function exists to stop a caller writing. A batch
with no triangles is dropped from the list, and its binding is still resolved,
so an emptied element that names a missing definition is still an error.

`read_library` is the startup check: it parses a material library, validates it against
the building that uses it, refuses a texture key that leaves the asset root or
that is used as both sRGB colour and linear data, and opens every map it names.
A missing or miscoloured map is then a startup error naming the key rather than
a black surface. It says nothing about material graphs — a `Files` surface
carries its `baked_from` as provenance and loads the files beside it, and a
featureless build has nothing to lower a graph with. `read_library_with_graphs`, under
`runtime-bake`, is `read_library` plus the graph half: every `Graph` surface lowered,
every `Shader` one partitioned, every strand layer checked against the graph
that declares it and every repeat compared.

A library does not have to come from a file.
The default library is Rust, `ashlar_material::stdlib::materials()`, and a game
that merges its own definitions into it holds the result as a value.
`check_library` and `check_library_with_graphs` are `read_library` and `read_library_with_graphs` for exactly that:
the same preflight over a `MaterialLibrary` already in memory, with an origin
string standing in for the path in errors.

A map is a PNG or a KTX2. A PNG is decoded through the `image` crate, as before;
a KTX2 is checked rather than decoded, and how deeply depends on the build. Under
`runtime-bake` it goes through `ashlar-material`'s own reader, which walks the
header *and* the level index and says whether the file's claims fit inside the
file — that is the crate that writes the container, so the check costs no second
parser. With default features there is no KTX2 reader in the tree, and pulling one in for a
startup check would undo the point of the split, so the header alone is read by
hand: the identifier, which tells a KTX2 from a PNG somebody renamed, and the
supercompression scheme. What a game gives up is exactly *truncated after the
header*, which becomes an error from Bevy's loader instead of one from preflight.
KTX2 is what a baked material should ship as, because it carries
the whole mip chain and a PNG carries level 0 alone — Bevy builds no chain for an
image it loaded, so a set of PNGs is a set the hardware point samples as soon as
the wall is far away. Bevy is built here with its `ktx2` feature and with
`zstd_rust`, which is `ruzstd`: pure Rust, decode only, and what reads the
zstd-supercompressed levels `ashlar-material`'s writer produces — the retired
study's maps were 24 MB that way rather than 94. The encoder is the other side of the
file and is not linked here. A file under any *other* supercompression scheme is
refused by number at preflight rather than failing at load.

A definition's `surface` says where its varying detail comes from: `Plain` for
constants, `Files` for files on disk, and `Graph` or `Shader` for a material
graph. `read_library_with_graphs` takes a `MaterialGraphLibrary` beside the material
library — `read_graphs` reads one from RON and validates it — and preflights both
graph surfaces, so a broken graph is a startup error the way a missing file
already is. A `Graph` one is validated and lowered; a `Shader` one is validated,
lowered, partitioned and printed as WGSL for both stages, under the `shader`
feature that holds the partitioner. What is checked always follows what is
compiled, and in the safe direction: a build that cannot check a surface cannot
create it either. The one thing preflight cannot do is *validate* that WGSL:
`naga` is a dev-dependency of `ashlar-material` on purpose, so even a tool
linking this crate does not link a shader front end it will never call, and the
generated text is validated in that crate's tests and compiled for real by the
first frame that draws the material. The graph side of the path is
[the material guide](materials.md).

`read_library_with_graphs` also *advises* on the one number both halves carry. A graph
may declare the repeat it was authored against — `MaterialGraph::tile_metres`,
which no bake or key reads — and a definition's own `tile_metres` is the repeat
it lays that graph over. `tiling` compares them, is pure and is available in
every build, and the read logs the answer per definition:
nothing when the graph declares none, when the two agree, or when the whole
material is scaled by exactly the factor the definition's `tile_scale`
declares (a specimen sheet does that deliberately to fake distance); a
warning for any other uniform scale, naming the `tile_scale` that would
silence it; and a warning whatever `tile_scale` says when the repeat is a
different *shape*, or is scaled under a definition that grows strands — a
strand's length is absolute metres and does not scale with the repeat. It is never an error: a definition is allowed to lay
a graph at whatever size a wall needs.

A `Graph` surface is baked into `Image` assets when the material is created.
That is `runtime_bake::create_graph_material`, which takes the graph library, a
`runtime_bake::BakeCache` and `Assets<Image>` beside what `create_material` takes and
handles every variant, so a library that can hold a graph surface never matches
on the variant itself; `create_material` stays the one-borrow, cannot-fail call a game
shipping texture files makes, and hands a graph surface back as its constants.
The images carry the same bytes, formats and mip chain the KTX2 of the same
`Bake` would have carried — the same four formats, untranscoded, with repeat
addressing and trilinear filtering — and live in the render world only. The
cache is keyed by the whole bake, so two definitions naming one `Bake` share one
texture set and a per-building seed is a distinct set with no distinct file.

No material this crate makes reads a height map. The bake writes one,
preflight opens a `Files` one and a runtime bake keeps one on `GraphImages`,
but it reaches no `StandardMaterial` slot, and a compiled surface's `height`
port reaches none either. Bevy's relief slot, `depth_map`, wants black-is-top
depth and a parallax scale in the mesh's own units, and a `0..=1` height field
carries neither, so the inversion and the scale are a game's to decide. What
the height buys here is the normal, which the bake derives from it.

An *instance* may put its own values on the graph parameters of the surface a
slot binds — that is `ashlar::Binding`, which is a material key in RON where it
overrides nothing and `(material: "...", params: {...})` where it does —
and `ashlar_bevy::definition` is where the two meet: it answers the definition a
binding names with those values written into it, so nothing below it learns that
an override happened. `read_library_with_graphs` preflights every binding as well as every
definition, by lowering the overridden surface, so a parameter the graph does not
declare is a startup error naming the instance, the slot and the parameter. What
it costs is what the value reaches: an override of a `Graph` surface is a
distinct bake and so a distinct texture set, shared between every instance that
asked for the same one; an override of a *live* parameter of a `Shader` surface
is the same pipeline and the same bound textures with a different uniform block,
and of a folded one is a second compiled graph. An override reaches the graph's
parameters and never the definition's own constants, which stay the library's.

A `Shader` surface is compiled instead of baked, by `shader::create_shader_material`, and
what comes back is not a `StandardMaterial` at all: it is a
`shader::ProceduralMaterial`, which is `ExtendedMaterial<StandardMaterial,
GraphExtension>` — the same constants, the same `uv_transform`, the same
lighting, with a generated fragment over them. The graph is *partitioned*:
everything that does not move is baked into the same `BakeCache` and everything
that does is emitted as WGSL and evaluated per fragment.

The static half is cached **by the plane behind each bound image** rather than
by the graph that asked for it. A `ShaderKey` — the graph, the resolution, which
parameters stayed live and the value of every parameter that did not — still
names one compiled graph's list of images, but the images themselves are keyed
by `runtime_bake::PlaneImageKey`: the format, and the identity of the plane in
each of its lanes, which is `ashlar_material::planes::PlaneKey` — the canonical
encoding of that plane's whole closure after folding, each plan written once
with its filter chain, its resolution and its value type. So two compiled graphs
that instance
one wall through `Subgraph` share its planes instead of each rasterising them:
`showcase:concrete-wet` and `showcase:concrete-cut-aware` are two shaders and
two fragment halves over one `library:formed-concrete`. Measured on the study
concrete they instanced before it, the second of them cost about 0.18 seconds
where it used to cost 1.3. A graph whose every bound image is
already there rasterises nothing at all, and one that shares some of them
rasterises only its own — `planes::rasterise_wanted` and `GpuBaker::planes_wanted`
take the planes a caller is missing and the planes those read, and skip the rest
on either backend. The key is exact rather than a digest, for the reason
`BakeKey` gives: a collision would be one building wearing another's texels. It
is the only key in that cache whose size is worth thinking about — a plane key
is the sum of its closure's sub-expressions, five kilobytes for one of the
retired study concrete's planes and a hundred for one of its painted metal's, and
`PlaneKey`'s own documentation has the measured table.

`MaterialExtension::fragment_shader` is a static function and cannot vary per
graph, so `GraphExtension` does not use it. It carries a `GraphKey` as its
`bind_group_data`, which Bevy makes part of the pipeline cache key and hands to
`specialize`, and `specialize` swaps the descriptor's fragment shader — the main
pass one or the prepass one, told apart by the `PREPASS_PIPELINE` shader-def —
for a weak `Handle<Shader>` whose UUID is derived from that key. `create_shader_material`
is what put a shader under that id. Two definitions over one graph with one live
set and one set of folded values therefore share a pipeline and differ only in a
uniform; two that fold a parameter differently do not, because folding one
changes the generated text. `shader::ProceduralMaterialPlugin` registers the
material, its pipeline and its prepass; without it a compiled surface has no
asset type to be.

The bind group is fixed by the type and not by the graph: a 256-byte uniform
block at binding 100 and eight texture-and-sampler pairs from 101, of which a
graph fills the front. Eight is the number the partition refuses above. Each
image is encoded the way the bake encodes the map it *is* — the same dither, the
same transfer, the same chain — except where several planes were packed four to
one image, which have to share one format and so share half floats.

A *live* parameter is that uniform block and nothing else, which is the whole
point of having partitioned. `shader::block` fills one from a name-to-value map
against the layout the emitter answered, and writing it over a material's old
one is the entire update: no plane is re-rasterised, no WGSL re-emitted, no
pipeline rebuilt, because none of them ever saw the value. That is what lets
`ashlar-preview`'s parameter panel move a compiled wall on every step of a drag
while a baked one waits for the pointer to let go. A parameter the partition
*folded* is the other case and costs the lot — it went into the bound textures
and sometimes into the generated text — which is why `GraphKey` covers the
folded values as well as the live set.

The bake is CPU work and it blocks: about 0.16 seconds and 19 MiB of images for
the retired study concrete at 1024 across this machine's cores, chain included. Calling
`create_graph_material` on the main thread is right for a preview, a content step and a
test, and wrong for a game at play; `runtime_bake::bake_images` is the same work
as a pure function of a bake and a graph library, so it runs in an
`AsyncComputeTaskPool` task and only `BakeCache::insert` — moving the finished
images into `Assets<Image>` — has to happen on the main thread. Its documentation
carries that pattern in full.

Or it does not have to be CPU work. `gpu::GpuBaker` bakes the same graph through
the same emitter on the render device — a compute dispatch per plane, one for
the outputs, and the normal, the chain and the encoding still on the CPU over
the read-back `f32` planes — and `Baker::{Cpu, Gpu}` on `BakeContext` and
`ShaderContext` is how a caller says which. `gpu::GpuBakePlugin` puts a baker in
the world wherever the app has a device, and every caller asks with an `Option`,
because a headless test and a build server have none and the CPU bake is the
reference anyway. A graph the device cannot split goes to the CPU as well:
a plane pass binds the planes it reads as textures, at most eight of them, and
the grass's canopy reads eleven. `gpu::dispatchable` is that question, asked
before the dispatch, because it is a limit of the device and not a fault to report.
What it is worth: at 1024 in a debug build, the retired study's
painted metal goes from 3.1 s to 1.1 s and its plaster from 2.3 s to 1.0 s,
while its cast concrete does not move and its plain metal gets slower — a
dispatch costs a round trip whatever it evaluates, and a small graph's
expression was never the expensive part. The bytes agree with the CPU bake to
within one eight-bit code, which `just conformance` checks map by map and level
by level.

A plane goes to the next dispatch as sixteen bytes a texel, and it is packed and
written **once per bake**: the dispatch for each plane and the one for the
material's outputs share a single cache of uploaded textures, so the outputs
never re-write what the planes just put on the device. That is not tidiness but
the larger half of the cost — an upload is about 37 ms at 1024 squared in a
debug build against about 60 ms for the dispatch reading it, which was 110 ms of
the retired study painted metal's 1.08 s. `GpuBaker::uploads` counts what reached the
device, and `just conformance` holds every shipped graph to one upload per bound
image. It stops at the bake: keeping the textures alive across bakes would hold
16 MB of device memory a plane on a resource with nothing to evict with, and
would still pay for the dispatch that recomputed the plane, because nothing on
this side caches planes.

`mesh` uploads one attribute the geometry layer does not name: a surface with
any cut face in it carries `ATTRIBUTE_ASHLAR_CUT`, one flag per vertex, so a
compiled material can read `CutFlag` and treat the faces a cutter made
differently from the ones it did not. That attribute *is* Bevy's second UV set,
and deliberately: a `MaterialExtension` draws through Bevy's own vertex stage
into Bevy's own `VertexOutput`, which has no free location, and the alternative
to repurposing a slot is emitting a vertex shader that reimplements `mesh.wgsl`
and `prepass.wgsl` to carry one float. Vertex colour was the other free slot and
is worse, because `pbr_fragment` assigns it over the material's base colour
rather than multiplying. A mesh with no cut face carries no attribute and costs
no vertex bytes; the generated fragment reads zero where `VERTEX_UVS_B` is
undefined, which is the same answer. The per-triangle flag becomes one
unambiguous per-vertex value because `TriangleMesh::weld` treats the edge of a
cut as a seam.

`ground_mesh` draws the exact convex corners `fit_ground` produced, so what is
rendered and what is collided with are one set of vertices.

This crate carries no solid kernel and no C++ toolchain: it consumes meshes,
whoever evaluated them.

## Testing and conformance

Everything in `tests/` runs headless on `MinimalPlugins` and needs no adapter,
with one deliberate exception.


`tests/conformance.rs` is **the contract between the two backends**, and it
**needs a GPU**. Every test in it that dispatches one is `#[ignore]`d, and
`just conformance` is what runs them:

```sh
just conformance
```

It opens a headless `wgpu` device through Bevy's own
`render::renderer::initialize_renderer` — no window, no `App`, no render world,
and no new dependency, because this crate already links the `wgpu` Bevy pins —
then, for every case, dispatches the compute kernel `ashlar_material::wgsl`
emits from a partition and compares the result with `ashlar_material::interp`
over that same partition, texel for texel at 256 squared:

- **one partition per primitive IR op**, each a static field cut into a bound
  texture with a live parameter over it, which is as close to a one-op
  partition as a graph can be written. A coverage test — the one test in the
  file that is *not* `#[ignore]`d, because it only partitions — asserts the
  cases between them reach every arm of `Op` the node vocabulary can produce,
  so a new op with no case fails in `just ci`, on the branch that added the op
  and on a machine with no GPU in it.
- **every graph of the default library with every parameter live**, at 512,
  which is the same claim over thousands of instructions and up to eight bound
  textures rather than over three. The grass is the one it skips: all live, its
  eleven strand reliefs do not fit, and it ships baked.
- **the lattice hash**, against the emitted helper itself. This one is not a
  tolerance: `hash2` and `hash3` must agree bit for bit, because every noise
  and every scatter in the crate rests on them and a single differing bit is a
  different wall.
- **the whole GPU bake against the whole CPU bake**, at 512 squared, for every
  graph of the default library. The other three cases compare numbers; this one compares the
  *encoded bytes* of every map at every mip level, so the four-tap normal, the
  chain, the Toksvig widening and the quantisation are under the contract
  beside the expression. Worst observed: one eight-bit code, on a texel a hair
  either side of a rounding boundary. It prints what each bake cost, which is
  the other reason it exists — a runtime bake is paid for in front of a frame.

The bound textures of the first three cases are uploaded as `Rgba32Float`
holding the CPU planes' own numbers rather than as the eight-bit images a
shipped material binds, so what is under test is the runtime half alone; the
encoding is `tests/shaders.rs`'s, `ashlar-material`'s, and the bake case above's
to check.

The budgets, and what the two backends actually come to on the reference
machine — an RTX 4090 on Vulkan, driver 595.84:

| what | allowed | worst observed |
| --- | --- | --- |
| a colour channel (`base_color`, `emissive`) | 1/255 | 6.1e-7 |
| a normal lane | 2/255 | 1.2e-6 |
| a raw scalar plane (`roughness`, `metallic`, `occlusion`, `height`) | 1e-4 | 3.8e-6 |
| the lattice hash | zero | zero |

Three orders of magnitude of headroom, and all of what is left comes from the
two places the backends are allowed to differ: `sin` and `cos`, whose accuracy
WGSL states in ULP and each vendor meets differently, and the hardware's
bilinear weights, which the specification only requires to carry eight
fractional bits of subtexel precision where a CPU plane carries all of them.
A run that lands near a budget has found something; the fix belongs in the
emitter, not in the budget. Each case prints its own worst difference, which is
the number to read rather than the pass.
