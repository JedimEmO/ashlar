# Material graphs in depth

This is the long guide to `ashlar-material`: tiling, the lowering, the
buffered filters, shader compilation, what a bake writes and costs, and the
standard library. The crate's [README](https://github.com/JedimEmO/ashlar/blob/main/crates/ashlar-material/README.md)
is the short version; start there.

Procedural material graphs as ordinary Rust data. Wire noises, patterns,
transforms and blends into a PBR output; validate it; bake it into a tileable
texture set, or compile it into a shader. No Bevy, no IO, no node editor.

The vocabulary follows [Material Maker](https://www.materialmaker.org/) where a
Material Maker node exists, so porting a graph by hand is mechanical.

## Layering

```
ashlar                 ashlar-material   <- you are here. glam, serde, thiserror.
  <- ashlar-manifold                        No Bevy, no IO, no kernel.
                                            One feature, `zstd`, off by default:
                                            the KTX2 writer's supercompression.
                       ashlar-strands    <- the half of a strand layer after
                                            the scatter: the set, its file,
                                            place, mesh and cards. This crate
                                            depends on it and re-exports it.
  <- ashlar-bevy       <-                bakes graphs to Image assets and
                                          compiles them to Bevy materials.
```

`ashlar` and this crate sit side by side and know nothing of each other: a graph
knows nothing about buildings, and a building names a graph by key only. Under
both is `ashlar-surface`, the vocabulary a material definition is written in,
which this crate re-exports; it is why a `ParamValue` here is the one a recipe
binds. `ashlar-bevy` is the first crate that depends on both. An arrow never points
the other way.

## Tiling is checked, not hoped for

Every signal carries an integer `Period`: `Tiled { u, v }` means the field is
laid on a lattice of that many cells across UV `[0, 1)` and meets itself at the
seam. Generators take an integer period and wrap their lattice, pointwise nodes
take the least common multiple
of their inputs, and transforms may only change a period in ways that keep it
an integer. A period is what the field actually does rather than what its
counts suggest: brickwork in a running bond repeats every *second* row, and a
bond that does not divide the rows is rejected at `offset`, because the wall
would not meet itself across the v seam. Every bound output must tile, and so
must the multiple across them, since that multiple is the repeat a bake writes;
a graph that cannot tile is rejected by node path before anything is baked.

What a period does not say is that a *coordinate* meets itself: `Uv` tiles once
because the lattice it addresses does, while `u` runs from 0 to 1 and jumps
back. Every generator wraps the coordinate itself, so this only shows where a
coordinate is used as a value — a `Warp` offset built from a bare `Uv`
displaces differently on each side of the seam — and the node's own
documentation says what to build such an offset from instead.

## One lowering, two backends

Every node lowers into a small typed intermediate representation of pure
per-texel expressions: an SSA list of three dozen primitive operations over
`f32`, `vec2` and `vec3`. The CPU interpreter runs that list, and the shader
backend prints it; a node is written once, so a baked material and its live
twin cannot drift apart, and only the primitives are implemented twice.
Lowering folds constants — which is how a parameter exposed for baking
disappears — gives one id to identical sub-expressions, and drops everything no
output reads. The result hashes to a stable 64-bit cache key.

Every node the vocabulary declares lowers, and so does every output of every one
of them: the coordinate; value and Perlin noise with their octaves; cellular
noise with three metrics and its distance, cell, edge and border outputs;
brickwork and a grid, hexagonal or herringbone tiling, each with a mask, a
bevel, a per-tile id and a fill; the regular patterns, the signed-distance
shapes — a disc, a box, a polygon, a star, a capsule and a gear, each of them
roundable and hollow and read either as a mask or as the signed distance
itself — a scatter of scratches and a woven cloth, plain, twill or satin, read
as a mask, as relief, as one set of threads against the other or as a hash per
thread; the resamplers — transform, warp, directional
warp, gradient and intensity warps, mirror, kaleidoscope, a circle map that reads a source round a disc inside
the repeat, a circle splatter that scatters one round a ring inside it, and a
tile whose instances a mask and their own hash decide; levels, a monotone curve, a gradient, brightness and hue, the scalar
operators, invert, mix, clamp, switch, decompose, combine and its two-lane
form, a direction field, all eleven blend
modes, a height-to-mask band and a normal from height; the buffered filters
below; the four runtime inputs — time, world position, world normal and the
mesh's cut flag — and the triplanar and world mask built out of the two
world-space ones; and a subgraph, which is inlined into the graph that
instanced it, together with the typed inputs that graph declares.

There is no `GradientMap`, which a Material Maker author will look for. What
that name usually means — a float through colour stops — is `Colorize`, whose
gradient takes any number of stops; the other thing it means, a gradient
applied to an imported image, has nothing to apply to, because a graph has no
image input.

A graph takes signals as well as parameters, and the difference between the two
is when they are resolved. A parameter bound at a subgraph is a number, folded
into constants when the instance is lowered, and never live: a live parameter of
an instanced graph would want a uniform the outer graph never declared. An input
is a whole field wired into a `GraphInput` node of the instanced graph, so it
carries a type, a period and a lattice inward — which means the instance is
inferred *again* under what was wired in, because an input of period 8 read
through an inner transform of scale 2 lays 16 cells and nothing short of
inferring it again says so. Instances are therefore cached per binding signature
rather than per key: two nodes that bind the same fields share one instance and
one inlined copy, and two that bind different ones pay for both. With nothing
bound, a `GraphInput` is its default literal at unit period, which is what lets a
compound still build, bake and be looked at on its own. Beside the six PBR
channels a graph may export named float masks; a bake writes no map for one and a
shader binds nothing for one, and they exist so that a compound which decided
something — where the wear took, where the water sat — can hand that decision
back rather than make its caller build it again and get an answer a texel off.

The runtime inputs are the one place where which backend is asking changes the
answer. Time and the cut flag are **zero in a bake**, because the instant an app
started and an uncut face are both pictures; `BakeReport::cut_flag` says when a
bake answered that way, since a graph authored to darken its own reveals bakes
to a surface with no reveals in it. In a compiled material the cut flag comes
off the mesh: `ashlar-bevy` uploads one flag per vertex and splits the vertices
either side of a cut, so a fragment reads exactly one or exactly zero. A world position and a world normal
are **refused** by one, at the node that asked, with a message naming the shader
surface that does answer: a world position of zero is every fragment of a wall
at one point, which for a triplanar is one sample of its source smeared across a
whole map, and a world normal of zero is not a direction at all, so a mask of
the faces rain lands on would answer the same number everywhere and a bake would
ship it as a surface. The *op* is unchanged either way — what is refused is a
material somebody asked to bake, not an expression somebody asked to evaluate.

Two *settings* are still refused, and a refusal names the node that asked rather
than quietly answering something else. One is a backend waiting its turn — a
rounded or cut brick corner. The other is not waiting for anything: a
**simplex** noise has no integer period at all, because the skew that turns the
square lattice into the triangular one is `(sqrt(3) - 1) / 2` and no whole
number of repeats brings an irrational skew back to itself. It validates, it
infers a period, and asking a backend to run one says why and points at Perlin,
which has the same character and a lattice that wraps.

A resampler is the one thing in the lowering that costs rather than folds. The
IR is one expression per texel and `Op::Uv` is that texel's coordinate, so a
transform, a warp or a scatter cannot move a value that has already been
computed: it emits its source again with another coordinate where the texel's
own stood. Only the instructions that actually depend on the coordinate are
copied, identical copies still collapse, and a sample of a plane is re-read at
the new coordinate rather than becoming a second plane. A graph that reads one
source through four frames pays for four, which is what it asked for.

Randomness is integer hashing and nothing else: a wrapping multiply-xorshift
over lattice coordinates reduced modulo the period, which is bit-identical
between Rust and WGSL and is why a noise wraps at its own period rather than
approximately. A Perlin gradient is a pair of those hashes normalised rather
than an angle through a sine and a cosine, so no noise here calls `libm` and no
two backends can disagree about a transcendental function's last bit.

"Cannot drift apart" is a claim, and one test in the workspace makes it: the
`#[ignore]`d `tests/conformance.rs` in `ashlar-bevy`, which `just conformance`
runs on a machine with a GPU. It dispatches the emitted compute kernel and
compares it with this crate's own interpreter over the same partition, texel for
texel — one partition per primitive op, one probe per node whose arithmetic no
shipped graph reaches yet, every shipped graph with every parameter live, the whole
GPU bake of every shipped graph against the whole CPU bake of it as encoded bytes,
and the lattice hash, which agrees bit for bit. On the reference machine
nothing differs by more than eight parts in a million, where a colour has a
budget of one part in 255. Everything else in this crate checks one backend;
that one checks that they are the same function.

## The filters that need a neighbourhood

A blur, an occlusion, a distance, an erosion, a dilation, a curvature or an edge
detect cannot be a texel's own expression: reading a neighbourhood per texel
would be the whole expression again once per tap. So the lowering cuts the graph
there. Everything upstream is rasterised once into a plane, the filter runs over
that plane as ordinary Rust, and everything downstream reads the result
bilinearly with wrap. `Buffer` is the same cut with no filter, for pinning a
plane's resolution or paying for a sub-expression once.

Wrapped is the word that carries the weight: every filter reads its neighbours
across the seam, so a blurred noise still meets itself and the period a port
carries still means what it says. A filter that clamped at the edge would leave
a period that was right and a picture that was wrong, which is why each of them
is tested against a field whose answer is different if it had.

Every radius is a reach in UV rather than a count of texels, so a graph
describes the same surface at 512 and at 4096: the blur is a Gaussian truncated
at its radius, with its standard deviation a third of it, and the same kernel
laid along one angle is what makes a brushed metal out of a grain; the occlusion
marches eight directions out to its radius and keeps the steepest rise in each;
the distance is a jump flood over the torus, in UV units; the erosion and
dilation take the square neighbourhood of the radius, which is separable and
gives an axis-aligned mask back unchanged where a disc would round its corners;
and the curvature and the edge detect answer *differences* over the radius
rather than derivatives, so the edge of a mask is one whatever the radius and
the crest of a relief answers the height it stands proud by.

One filter reads two planes rather than one. A **slope blur** walks each texel
down the gradient of a *height* and averages the source at every place it
stopped, which is what drags rust out of a chip and dust out of a joint — wear
that follows the relief instead of ignoring it. So its filter names the plane it
walks beside the plane it was given; the guide is rasterised first, costs no
binding in a shader because nothing samples it, and is followed by everything
that walks the plan list.

One more node is buffered, for a different reason. `StrandRelief` splats a
graph's own strand layer — the same roots, lengths and colours the geometry is
scattered from — back into a plane seen from directly above, so the relief the
camera sees once the blades have faded out is the blades. A strand crosses many
lattice cells, so splatting each footprint once into a plane is the bounded
way to answer it. [ADR 0004](../adr/0004-strand-layers.md) is where it comes
from.

`Blur::slope(source, guide).slope_mode(SlopeMode::Min)` takes the minimum
along the walk instead: use it for chipped stone silhouettes and edge erosion.
`SlopeMode::Max` builds deposits; `Average` remains the backward-compatible
default. Min and max include the starting sample and operate per channel.
All modes wrap across the tile and use the shared buffered pipeline for CPU
bakes and shader delivery. The mode participates in the plane cache key.

A plane is the expensive thing in a bake — a megatexel of `f32` per lane,
rasterised and filtered once — so two nodes that ask for the same filter over
the same expression share one, a buffered node no output reads leaves none, and
a `BakeCache` handed to `rasterise_with` keeps the planes whose expression,
filter and resolution did not change. That is what a preview re-baking on every
turn of a slider wants.

What decides all of that is `planes::PlaneKey`, and it is public because the
layer above keeps the *encoded* planes and has to ask the same question. It is
the canonical encoding of the plane's whole closure after folding: the plane,
the planes it samples or is guided by and theirs in turn, each written once and
each sub-expression renumbered from its own root, with an `Op::Sample` standing
in as a position in that list, and with every plan's filter chain, resolution
and value type. An exact key rather than a digest of one: two keys are equal
exactly when the planes are, and the comparison is a byte comparison, because a
collision here would put one wall's texels on another. That is not free — a key
is the sum of its closure, so a few kilobytes over a shallow graph and about a
hundred over the deepest in the retired study library — and the type's own
documentation has the measured table. `planes::plane_keys` answers them
without rasterising anything, so a caller can find out what it already has
before paying for a texel, and `planes::rasterise_wanted` then runs only the
planes it names and the planes those read — `planes::plane_closure` is that
dependency sweep.

## Compiled to a shader instead

The same lowering is a fragment shader. `partition` colours every value in it:
**runtime** where it depends on a parameter the author exposed as live, or on
time, world position, world normal or the mesh's cut flag — the four things a
texture cannot hold — and **static** everywhere else. Then it cuts at the
frontier. Every maximal static sub-expression a runtime op reads becomes a bound
texture, baked by the same plane pipeline the buffered filters use; what is left
is an expression over the uniforms, the runtime inputs and samples of those
textures. An output that reaches nothing runtime is a texture in its own right,
sampled straight into its PBR slot, and the material's normal — which the bake
derives from height outside the graph — comes back in as a root of its own:
a bound normal map where the height is static, and four offset taps of it where
it is not.

The point of the pass is the number it prints. A live parameter at the final
blend costs one blend per fragment; one that warps the bound outputs, which is
what a per-building `variation` is, costs every instruction that depends on the
coordinate — the retired study brick was 1189 instructions baked and 2683 per
fragment with its `variation` live, and the report says so before the shader ships. A
graph with no live inputs reports zero runtime ops and the verdict
`no live inputs: bake this`.

A world-space graph is the other end of that scale, and the showcase ships one.
`showcase:concrete-wet` instances `library:formed-concrete`, the baked wall, and
multiplies its live `wetness` by two `WorldMask`s — the faces that point up, and
the faces under six metres — so one material is wet on a coping and dry on the
wall beneath it, which was a slot somebody bound by hand until a fragment could
ask. It costs a few dozen instructions over bound textures, and the two masks
are most of them.

A bound texture goes into the format the bake writes that map in where it *is*
one of the five maps, and into half floats where it is an interior cut, whose
range the graph never promised. A graph binding more than the eight images a
bind group has room for packs its scalars four to an image first — and a format
belongs to an image rather than to a plane, so everything that packs goes into
half floats whatever port it serves. Packing therefore trades the bake's own
quantisation for fitting, which is the kind of thing a cost report exists to
say.

Two things the cut cannot do, and says rather than fudging. A blur, an
occlusion, a distance or a morphology reads an unbounded neighbourhood: it is a
plane or it is nothing, so a parameter that reaches one is **frozen** — folded
at its value the way a bake would fold it, and named in the report, because a
slider that silently does nothing is worse than one that says why. And a runtime
input above such a filter has no value to fold, so it is refused by node path.

## Printed as WGSL

`wgsl::emit` turns a partition into shader text: one `let` per instruction, in
the order the interpreter runs them, inside a function that takes the coordinate
and the four runtime inputs and answers the ports the graph bound. Every op is
written as the arithmetic the interpreter does rather than as the WGSL built-in
that is nearly it — `mix` is `a + (b - a) * t`, because WGSL's `mix` rounds the
other way; `clamp` is `min(max(x, low), high)`, because WGSL leaves its `clamp`
indeterminate when the bounds cross and a graph is allowed to cross them; and
division, `smoothstep` and `normalize` go through small helpers that keep the
guarded case each of them has. The lattice hash is the same integer
multiply-xorshift over the same modulo-period reduction, in `u32`, whose
arithmetic WGSL defines as wrapping. One op is narrower on this side and is
written down rather than hidden: `pow` says nothing about a negative base, and
`f32::powf` answers for one. One more difference is against the *bake* rather
than the interpreter, and cannot be closed: a height that evaluates to an
infinity bakes to a flat normal, because the plane pipeline guards a non-finite
length, and compiles to a `NaN` one, because `normalize` of an infinite vector
is `NaN` — and WGSL lets an implementation assume infinities never happen, so a
comparison guarding it would be a promise this side cannot keep.

Around that function: the `AshlarParams` uniform block at binding 100, laid out
the way WGSL lays one out — a scalar aligned to four, a colour to sixteen,
consecutive floats packed four to a sixteen-byte row, in the order the graph
declares its parameters, with the block rounded up to sixteen — and a
`ParamLayout` per parameter so the Bevy side fills bytes without re-deriving any
of it; one `texture_2d<f32>` and `sampler` pair per bound image from binding
101; a fragment over `pbr_input_from_standard_material` that *multiplies* into
the PBR slots the way the same material's baked maps would, and writes its
tangent-space normal through `calculate_tbn_mikktspace` where the mesh carries
tangents and leaves the geometric normal alone where it does not; and a prepass fragment
writing the same normal, so screen-space occlusion and the deferred path see the
procedural relief and not the flat mesh. `wgsl::emit_compute` prints the same
function as a `@workgroup_size(8, 8)` kernel over the texel centres of a square,
which is what the conformance test dispatches and what the GPU bake in
`ashlar-bevy` runs. `wgsl::emit_compute_ports` is that kernel for a *subset* of
the ports, declaring only the images those ports sample: a GPU bake dispatches
one plane at a time over `partition::over_planes`, which is this partition
turned inside out — nothing cut, the whole expression on the runtime side, and
every plane's root named as a port of its own so a kernel can be written for it.

Two texts come out of one emitter, and only one of them is a module `naga`
parses. The Bevy-flavoured one opens with `#import bevy_pbr::…` and writes its
bind group as `#{MATERIAL_BIND_GROUP}`; both are `naga_oil` preprocessor
directives. The *core* — the block, the bindings, the helpers and the function —
is the same generated code with a literal group and no imports, and it is what
the crate's tests parse and validate at the `naga` version Bevy pins: every node
of the vocabulary in isolation with a live parameter forcing it into the
fragment, every shipped graph with every parameter live and with none, and the
compute kernel. The wrapper around it is checked by the Bevy step compiling one,
and compiling one is what found the last thing in it: `globals` is binding 11 of
`mesh_view_bindings` in the main pass and binding 1 of the prepass's own smaller
view layout, where nothing declares it because no Bevy prepass shader reads it,
so a prepass that reads the clock declares `ashlar_globals` there itself rather
than importing the main pass's name.

`bake::encode_bound` is the other half of the compiled path, and it lives with
the bake rather than with the emitter because it is the bake's own policy: a
bound texture is quantised with the same sRGB transfer, the same sixteen-bit
height codes and the same half floats the five PBR maps are, and mipped with the
same wrapped box filter down to 1x1. A live material and its baked twin have to
be the same picture down to the eighth bit, and this is where that is decided —
so the two places that policy is not "round every channel the same way" are
followed here exactly:

- **Only the colour is dithered.** The bake puts a 4x4 ordered Bayer cell in
  front of the quantiser for its sRGB base colour, because banding in a gradient
  that crosses less than one code per texel is what that exists to hide, and
  rounds its normal, its ORM and its height plainly, because dither in a
  direction field or a roughness is noise in data nobody looks at directly.
  `encode_bound` keys on the sRGB format, which is exactly the map the bake
  dithers.
- **A normal chain is filtered un-normalised.** The mean of four unit normals is
  short where they disagree, and that shortness is the variance the next level
  needs; renormalising is therefore what a level *ships*, not what the next one
  is built from.

Both were found by comparing the two halves of a material sheet's pair as bytes
rather than as pixels, and before them a bound normal map differed from the
`normal.ktx2` of the same graph in one code on an eighth of its texels.

## What a bake writes

`bake` takes a graph, a resolution and the parameter values to fold in, and
answers a `TextureSet`: sRGB base colour, a tangent-space normal, occlusion,
roughness and metallic packed into one linear map, and the height and emissive
maps a graph binds or does not. The normal is derived from the height plane by
a wrapped central difference, so a parallax shader reads the same field the
relief was lit from. The one way the two can part company is a height that
leaves `0..=1`: the plane and the normal keep the value, the 16-bit map
saturates, and the report says what the range was so that the graph can be
fixed with a `Levels` rather than the map shipped flat. The `f32` planes are
exposed before encoding, because that is what the mip chain filters and what a
test reads without arguing about rounding.

A bake is refused when the graph lays a lattice finer than the texels asked
for — a noise of 512 cells baked at 256, octaves included. Below that the
answer is not a coarser version of the surface but an arbitrary sample of it,
different at every resolution, and the fix is more texels or a coarser graph.
The count is carried through the graph rather than read off each node alone,
because a resampler changes how fine a lattice lands: a transform that scales a
noise by four lays four times the cells, a scatter of sixteen instances lays
sixteen times what its source laid, and a graph counts what it instances,
because lowering inlines it.

Asking for mips writes the whole chain: every level down to one texel, each the
2x2 box filter of the one above it, taken over the `f32` planes and not over the
encoded bytes. At an even size the groups partition the plane, so there is no
wrap to run and no edge case to get wrong, and the mip of a repeat is still a
repeat. Two planes are not a plain average, and they are the two that would
otherwise make a wall shimmer: the chain carries the un-normalised mean of the
level-0 normals, ships its direction, and widens the roughness by how much
shorter that mean got — Toksvig's `r' = sqrt(r^2 + (1 - |n_avg|) * k)`, over a
constant the crate documents and defends. Renormalise without that and a bumpy
surface turns to glass at distance. Height stays 16-bit and emissive stays
half-float at every level.

## What a bake ships as

A texture set goes to disk as KTX2, one file per map, with every level inside
it. PNG cannot carry a chain, and Bevy does not build one for an image it
loaded, so a PNG set throws away the renormalised normals and the widened
roughness the chain exists for and is point sampled the moment a wall is far
away. It cannot carry the sixteen-bit height plane at all.

`ktx2::write` is the whole container: the eighty-byte header, the level index,
a data format descriptor and a `KTXwriter` key. No dependency, because the parts
of KTX2 a bake uses are a layout and four `VkFormat` numbers — `R8G8B8A8_SRGB`,
`R8G8B8A8_UNORM`, `R16_UNORM` and `R16G16B16A16_SFLOAT`, each of which Bevy's own
loader takes without transcoding. The pieces that are easy to get wrong are the
ones the tests read back with somebody else's parser: the levels run largest
first in the index and smallest first in the file, each aligned as the
specification asks, and the descriptor says which transfer function the bytes are
in, which is where a loader learns that the base colour is sRGB and the normal
map is not.

`ktx2::write_with` is the same file with its levels **supercompressed**, one
zstd frame per level under scheme 2, the uncompressed length recorded beside the
stored one and the alignment dropped to a byte as the specification says it is
for a stored level that is not texels. That is what `export` writes: the old
study maps went from 94 MB of levels to 24 MB of frames, about a second a map to write and nothing
measurable to read. The 8-bit planes come back at a tenth to a third of their
bytes and the 16-bit height barely moves, its low byte being noise. It is the
one thing in this crate behind a feature — `zstd`, off by default — because the
encoder is a C library and only a step that writes files needs one; a game reads
them with whatever decoder its engine already has, which for `ashlar-bevy` is
the pure-Rust `ruzstd` Bevy builds in.

`ktx2::inspect` reads a file's header and level index back and checks them
against the file's own length, whichever scheme they are under — for a
compressed level it is the recorded uncompressed length that has to come to the
level's own size, which is the one thing a reader with no decoder can check. That is what a startup preflight wants — is this
a KTX2 file, is it the shape it claims — without decoding a texel, and it is
what `ashlar-bevy` validates a `.ktx2` texture key with.

## The content step

A `Surface::Graph` definition bakes when the material is registered, and that is
the default for the whole library: nothing is written to disk, and the source of
every material is its Rust graph.
A game that would rather load files than pay for a bake at startup runs the same
definitions through `export::export` first.
It bakes every `Graph` surface, scatters every strand layer that names no set,
and answers three things:
the texture sets, the strand-set bytes, and a `MaterialLibrary` whose surfaces
now name KTX2 files under a directory, each recording the bake it came from in
`baked_from`.

```rust
use ashlar_material::{bake::bake_with_report, export, ktx2::Supercompression, stdlib};

let graphs = stdlib::graphs();
let exported = export::export(&export::ExportRequest {
    graphs: &graphs,
    definitions: &stdlib::materials(),
    directory: "materials",
    resolution: None,
    threads: std::num::NonZeroUsize::new(8),
    backend: &bake_with_report,
})?;
for set in &exported.sets {
    for (path, bytes) in set.files(Supercompression::Zstd)? {
        // `materials/library/brick/base.ktx2` and the rest: yours to write.
    }
}
```

This may seem like a strange place to stop, one step short of a file.
It is the same rule as the rest of the crate: no IO.
`ExportedSet::files` is the whole of what a writer needs, so the step that owns
the disk owns the only line that touches it.
Two definitions that bake the same graph at the same parameters share one set of
files, which is how a scene-specific definition that only changes the repeat
costs nothing to export.

Writing is not free either. `ktx2::ZSTD_LEVEL` is 19, and `ExportedSet::files`
compresses every level of every map on the calling thread, one map after
another: measured 2026-09-20 on the SOI cobblestone at 2048, its four files took
21.5 s against a 13 s bake. The maps are independent, so a writer that wants the
time back can call `ktx2::write_with` over `export::maps(&set.maps)` on a thread
per map; the bytes do not depend on the order.

In this repository the showcase's writer is `examples/showcase/src/export.rs`,
and `just materials` runs it into `assets/materials/`, which git ignores.

## What a bake costs

Baking is CPU work per material, and the cost is per texel, not per frame.
A 1024-squared bake of a graph with roughly two hundred operations per output
is a few seconds single-threaded and under a second across eight cores; rows
are rasterised in parallel through `std::thread::scope` on native targets,
with no extra dependency, and inline on one thread where there is only one
span to run, which is what `wasm32` gets. That is generous for a content step that writes files, and it is
why a material every level shares should be baked to files once rather than
baked at startup. `bake_with_report` says what one actually cost: wall clock,
the size of the whole expression, how much of it each output reaches, and how
many planes its buffered filters needed.

What a node costs is its instruction count, and the second set spread that out:
a grid of tiles is twenty-two instructions and a pattern the same, a shape from
fourteen for a disc to thirty-three for a star or a gear, a herringbone weave
forty-six, a Perlin octave sixty, a cellular noise
about a hundred and fifty — nine cells searched — and a scatter of scratches
about three hundred and seventy, because a texel tests the nine cells a segment
could reach it from. A `Tile` costs its source again per instance it might draw:
one while `overlap` is zero, nine past it. A `Shape`'s own fields are priced the
same way, measured on a probe whose wiring is four of every count here: a box is
seventeen, a capsule twenty-one and a polygon thirty-two; asking for the signed
distance rather than the mask *saves* three, because the inward ramp is what the
mask adds to it; a shell costs four whatever it hollows; and a corner radius is
two over a polygon or a star and eight over a box, which is the exact distance to
a square standing in for the Chebyshev one, that being flat across a whole
quadrant and so having no corner to round. A `CircleMap` is twenty-four
instructions over the source it re-emits once — measured on a probe whose source
is a `Uv` axis, which is six of the thirty — and twenty when its twist, its
fill and its second turn fold away, because a polar frame is an `atan2`, a
length and a handful of multiplies whichever way it is set. A `CircleSplatter`
re-emits its source *three times per ring*, the instance nearest the texel in
angle and the one either side of it, and that is the whole shape of its bill:
over the same `Uv` axis a plain single ring is eighty instructions, of which six
are the source; with all four variations, a face-centre turn and a mask that is
a field rather than a literal it is a hundred and ninety-two; and a second ring
of that takes it to three hundred and forty-three. Rings and variations are
therefore what to reach for last. The two signed-distance nodes are the cheap
end of the same ledger, measured on a probe whose two decomposed UV axes are
seven of the count: a hard `SdfCombine` is one instruction, a `min` or a `max`,
and two for a subtraction, which negates first; filleted it is eleven, twelve or
thirteen, the polynomial being a clamped ramp, a mix and a quadratic. An
`SdfMask` is three — a smoothstep and the complement of it — which is exactly
what a `Shape`'s mask costs over its own distance, eleven against fourteen,
because it is the same three instructions. A `Weave` is priced the same way and
by its output rather than by its cloth: twenty instructions as a mask, thirty
as one set of threads against the other, thirty-five as a hash per thread and
about fifty as relief, which is where both cross-sections, the crossing and the
two crests the maximum chooses between are all paid for. Its pattern costs
almost nothing — a plain weave and a twill are the same instructions at
different constants, and a satin adds three for the move and the offset that
scatter its binding points. Measured at 1024 squared across this
machine's cores, that is a hundredth of a second for a grid, six hundredths for
a Perlin, eight for a Voronoi and eighteen for either of the two that search a
neighbourhood.

A plane is the other half of that bill, and it is paid once rather than per
texel: at 1024 squared, across eight cores, rasterising a three-octave noise
into one is about two tenths of a second, and the filter over it is another
tenth for an erosion, a third for a blur of a hundredth of the repeat, and
about half for a horizon occlusion or a jump-flood distance. A graph with four
of them costs about two seconds, which is the same order as the rest of the
bake and is why the plane cache exists.

The mip chain is a third again of level 0 in bytes and in texels, and costs
about that: measured at 1024 squared on a graph binding all six outputs, level 0
takes a tenth of a second and the ten levels under it, filtered and encoded, add
a quarter of that again. It is single-threaded on purpose — averaging four
numbers is memory-bound, and the pass that made level 0 has already spent the
machine on it.

Resolution costs more than its texel count. A filter's radius is in UV, so its
reach in texels grows with the side, and a separable blur is `n² × 2rn` taps:
cubic in the side. Measured 2026-09-20 on eight threads, 1024 to 2048 cost 4.8
times on the SOI cobblestone and the brick rather than four, so a 4096 bake
extrapolates to five or six times a 2048 one. The grass moves less (2.4 times)
because its scatter runs at `strands::FIELD_RESOLUTION` whatever the bake asks
for. There is a floor, too: nothing bakes below its finest lattice, and the
reference graphs lay 512 cells, so 256 is a `BakeError::Lattice`. The same run
found the bake 82 per cent efficient at eight threads, with no serial section
worth attacking.

Every speed-up the bake has had — the live walk, per-stage memoisation, strips,
hoisted samplers, between 2026-09-18 and 2026-09-20 — left its bytes unchanged,
and that is the rule for the next one: the same `f32` operations in the same
order. No `mul_add` where there was a multiply and an add, no reassociated
sums, the same tap order in every filter, the same `std` transcendental per
lane. A value stored and read back is the value recomputed, which is why
memoisation is allowed and reassociation is not. The planes' byte-for-byte
filter tests, the strips' bit-for-bit tests against the per-texel walk and
`just conformance` hold it. A hardware FMA is allowed only because it is the
correctly rounded `fmaf` it replaces, which is `.cargo/config.toml`'s argument.

The default is one thread per core, bounded so that no span is shorter than a
few dozen rows: several bakes at once — a test binary, or a task pool building
a street of houses — would otherwise each ask for the whole machine, and the
smallest bake has nothing to gain from more threads than it has rows worth
giving them. `BakeRequest::threads` names a count where one is wanted, and is
honoured as named. The answer is the same bytes whatever the number.

Same bytes is a promise about one machine. Across platforms the promise is
the same picture: the sRGB encoding and the `Pow`, `Exp2`, `Log2`, `Sin`,
`Cos` and `Atan2` operators go through Rust's `f32` maths functions, whose
last bit is the platform's, so a value that lands on the edge of a code can
round the other way on another operating system or architecture. The noises
and the lattice hash use none of them, which is why most of a surface does not
move. A content pipeline that caches or compares bakes by their bytes should
key on the platform too.

Baking at runtime is for variety — a per-building seed, a tint the game picked
— and its costs are real: every distinct parameter set is its own texture set
in memory, and the bake belongs off the main thread. A material whose only
change is a value that moves per frame should be a shader instead, where
nothing is baked at all.

## A standard library

`ashlar_material::stdlib::graphs()` is the one set of graphs this crate ships
rather than reads, and it is a whole default material library.
The finished surfaces are keyed `library:<name>`: concrete, paving, stone
cladding, ashlar, plaster, the reference brick and cobblestone and their moss,
four steels, timber, roof and wall tile, asphalt, rubble, grass, the city's
facades, road and night surfaces, and a small utility set, forty in all.
The ground surfaces — `asphalt`, `road` and `paving-slabs` — take a `wet`
parameter, rain through `weathering:moisture`, whose default of zero leaves
the dry bake exactly as it was; `stained-concrete` is a storey of dark cast
concrete streaked from its slab edge, and `office-window` and `shopfront` are
lit interiors behind glass, their colour and brightness the definition's
`emissive`.
They are built bottom-up rather than written one by one.
A `layouts:<name>` graph draws unit identity and joints and no surface,
a `substances:<name>` graph is a surface with no layout,
and a finished material instances both through `Subgraph`.
Under all of them are the weathering compounds and four pattern recipes,
keyed `weathering:<name>` and `patterns:<name>`.

`stdlib::materials()` hands out the other half: a `MaterialDefinition` for every
finished surface, carrying its physical `tile_metres`, its default parameters and
a `Surface::Graph` at 512. A game merges the graphs into its own library with
`MaterialGraphLibrary::extend`, which refuses a key collision rather than
replacing either side, merges the definitions into its material library, and
binds `library:formed-concrete` like any key of its own.
Its graphs can then name any of these keys from `Subgraph` nodes. They live in the crate rather than in an example because the
crate's own tests are what hold them to the tiling, lattice, lowering and WGSL
rules that make a graph safe to *instance* at all, and because a game that wants
dirt in its hollows should get it by adding a dependency rather than by copying
six hundred lines of builder.

A compound takes the substrate's channels it intends to change, as
`GraphInput`s whose defaults make it bake to a plausible picture with nothing
wired in; it exposes the author's decisions as parameters, which fold when the
instance is lowered; it writes back the channels it names and no others; and it
exports the mask it decided with as an extra, so a caller can put its own dirt
in the same hollows rather than build a second mask that agrees with the first
to within a texel. Height and emissive are optional on `PbrOutput`, so reading
one a compound does not write is a refusal by path; base colour, roughness,
metalness and occlusion always have a value, so reading one a compound never
touched answers `PbrOutput`'s default — a metalness of zero, say — with no
message. Read the channels the table lists and no others. No compound reads a runtime input — a bake refuses
`WorldMask` by path, so a compound that read one could never be baked or tested
on its own. Where the caller wants the weather gated by the world, the compound
takes a plain float and the caller wires `WorldMask::up()`, a live parameter or
a constant into it.

| Key | Inputs | Channels it writes | Extra | Ops alone | Ops live |
| --- | --- | --- | --- | --- | --- |
| `weathering:edge_wear` | `height`, `base_color`, `roughness`, `metallic`, `substrate_color`, `substrate_roughness`, `substrate_metallic`, `bias` | base colour, roughness, metalness, height | `mask` | 182 | 74 |
| `weathering:dirt_dust` | `height`, `base_color`, `roughness`, `bias` | base colour, roughness | `cavity_mask` | 249 | 18 |
| `weathering:rust` | `height`, `base_color`, `roughness`, `metallic`, `seed_mask` | base colour, roughness, metalness, height | `rust_mask` | 398 | 123 |
| `weathering:peeling_paint` | `height`, `paint_color`, `paint_roughness`, `substrate_color`, `substrate_roughness`, `peel_mask` | base colour, roughness, occlusion, height | `bare` | 28 | 66 |
| `weathering:moisture` | `height`, `base_color`, `roughness`, `metallic`, `wet` | base colour, roughness, metalness, height | `wet_mask` | 6 | 44 |
| `weathering:streaks` | `base_color`, `roughness`, `source`, `bias` | base colour, roughness | `mask` | 408 | 25 |
| `patterns:radial_gradient` | — | base colour, roughness, height | `gradient` | 31 | 47 |
| `patterns:angular_gradient` | — | base colour, roughness | `gradient` | 33 | 10 |
| `patterns:wood` | — | base colour, roughness, height | `rings` | 286 | 150 |
| `patterns:cracks` | `mask` | base colour, roughness, height | `crack_mask` | 354 | 68 |

The two counts are the two ways a compound is paid for, and they are not the
same number at all. **Ops alone** is `Ir::len` of the graph lowered for a bake
with nothing wired in: every input is its default constant and every parameter
has folded, so this is what the compound's own arithmetic comes to once the
optimiser has had the easy half of it. It is the number to read as the cost of
*baking* the compound into somebody's texture set, and it is small for
`moisture` — six — for a reason worth knowing: with a flat height at 0.5 and
the waterline at 0.35, the whole submergence is a comparison between two
constants and folds away. Bind a real height and the arithmetic comes back. **Ops
live** is the fragment cost with every parameter kept as a uniform, measured as
the instructions a partition at 512 texels leaves reachable from the outputs;
that is the cost of compiling the compound into a shader with all of its
decisions still turnable. Two of the ten cost more live than they do baked —
`patterns:radial_gradient` at 31 against 47 and `weathering:peeling_paint` at 28
against 66 — and both for the same reason: baked with nothing wired in, each is
arithmetic over constants that folds nearly to nothing, while live it keeps
every parameter and, where it binds a height, the four taps the derived normal
costs. The graphs that lay a real lattice go the other way, `patterns:wood` from
286 to 150 and `patterns:cracks` from 354 to 68, because what the partition
leaves in the fragment is only what a live parameter reaches.

Two more compounds ship under `weathering:` and were added after the table was
measured. `weathering:moss` grows moss over whatever it is wired onto: it takes
`base_color`, `roughness`, `metallic`, `height` and `bias` like the others,
plus `shelter`, the habitat moss prefers — the mortar, the hollows — and
`coverage`, a field over its `amount` that is a good place for a live control.
It writes base colour, roughness, metalness and height and exports its `mask`;
growth only ever raises the height, and with `amount` or `bias` at zero every
channel comes back exactly as it went in. It leaves occlusion alone, so the
caller derives that again from the new height. `weathering:paint_film` is a
dielectric coat over the channels it is given, plus a `paint_color` and a
`loss_mask`: loss is shared by colour, roughness, metalness and thickness, so
full loss is the substrate again. It exports `remaining`, and
`library:painted-metal` is built on it.

A parameter only reaches an input *port*, so a filter radius, a noise seed, an
octave count and a generator's period — all of them node fields — are documented
constants in these builders rather than parameters. Exposing one would validate,
bind and silently do nothing. The `amount` of a compound is instead a contour on
whatever field it decided with, `clamp((field - (1 - amount)) / softness, 0, 1)`,
which is the same meaning in every graph here; and a compound that reads a
*neighbourhood* of the height takes a `relief` parameter, because a curvature
and an occlusion read the height as a length in the units of UV while a height
input is a `0..=1` field. It is the same number the caller passes to
`normal_strength`, which is what makes a compound instanced onto a shallow
surface wear less than the same compound on a deep one.

`MaterialGraphBuilder::layer` is the sugar for reading several of one
instance's outputs. A compound answers a surface rather than a field, and each
output is a `Subgraph` node of its own whose bindings must agree exactly — an
instance is inferred per binding signature, so a fifth input on one of four
near-identical nodes would quietly make it a second instance. `layer` writes
them from one node, adding `<id>.<port>` per listed output:

```rust
let material = MaterialGraph::builder("game:brick-weathered")
    // Live in the compiled twin: the rain the game moves.
    .param(Param::float("rain", 0.0).range(0.0, 1.0).live())
    // The wall itself: three channels off one instance of the library brick.
    .layer(
        "brick",
        Subgraph::new("library:brick"),
        &[SurfaceOutput::BaseColor, SurfaceOutput::Roughness, SurfaceOutput::Height],
    )
    // Dirt where the world says the rain cannot wash it off. The compound
    // takes a plain float; the caller decides what it means.
    .node("upward", WorldMask::up().softness(0.35))
    .layer(
        "dirt",
        Subgraph::new("weathering:dirt_dust")
            .input("height", "brick.height")
            .input("base_color", "brick.base_color")
            .input("roughness", "brick.roughness")
            .input("bias", "upward")
            // The brick's own relief, so the cavity filter reads a length.
            .param("relief", ParamValue::Float(0.02)),
        &[SurfaceOutput::BaseColor, SurfaceOutput::Roughness],
    )
    .layer(
        "wet",
        Subgraph::new("weathering:moisture")
            .input("height", "brick.height")
            .input("base_color", "dirt.base_color")
            .input("roughness", "dirt.roughness")
            .input("wet", Input::param("rain")),
        &[SurfaceOutput::BaseColor, SurfaceOutput::Roughness, SurfaceOutput::Height],
    )
    .output(
        PbrOutput::new()
            .base_color("wet.base_color")
            .roughness("wet.roughness")
            .height("wet.height")
            .normal_strength(0.02),
    )
    .build_in(&library)?;
```

That wall reads a world normal, so it is a `Shader` rather than a bake, and the
partition is the point: one uniform per fragment over bound textures, with
nothing frozen. The brick and the dirt are both dry arithmetic over the coordinate and both went into the
textures; only `moisture`, which is downstream of the live `rain`, is left in
the fragment, and it contains no buffered filter for the freeze rule to catch.

## Example

```rust
use ashlar_material::{
    BlendMode, Channel, GraphError, Input, Material, MaterialGraph, MathOp, Param,
    PbrOutput,
    nodes::{Blend, Colorize, Decompose, Invert, Levels, Math, Noise, Uv},
};

fn concrete() -> Result<Material, GraphError> {
    MaterialGraph::builder("game:concrete")
        .param(Param::color("tint", [0.66, 0.65, 0.64]))
        .node("coarse", Noise::value().period(4))
        .node("grain", Noise::value().period(128))
        .node("pores", Levels::new("grain").in_low(0.87))
        // A cast panel's joints run across it and not down it, so they are a
        // band in v: eight rows, and the height drops where a row is within
        // 0.007 of its edge. `Bricks` would lay a wall, vertical joints and
        // all, which is a different surface.
        .node("rows", Math::new(MathOp::Mul, Decompose::new(Uv::new(), Channel::G), 8.0))
        .node("into_row", Math::new(MathOp::Sub, Math::unary(MathOp::Fract, "rows"), 0.5))
        .node("seams", Math::new(MathOp::Step, Math::unary(MathOp::Abs, "into_row"), 0.493))
        // `height` is a `0..=1` field, and taking a joint of 0.16 out of a
        // noise that starts at zero would run it negative: the map would floor
        // where the normal derived from the plane still had slope. Bedding the
        // grain at 0.16 first keeps the two describing one surface.
        .node("bedded", Levels::new("grain").out_range(0.16, 1.0))
        .node("height", Blend::new(BlendMode::Subtract, "bedded", "seams").opacity(0.16))
        .node("albedo", Colorize::new("coarse").gradient([
            (0.0, [0.60, 0.59, 0.58]),
            (1.0, [0.74, 0.73, 0.71]),
        ]))
        .node("tinted", Blend::new(BlendMode::Multiply, "albedo", Input::param("tint")))
        // An inline node needs no id of its own; this one becomes `dirty.b`.
        .node("dirty", Blend::new(BlendMode::Multiply, "tinted", Invert::new("pores")))
        .output(
            PbrOutput::new()
                .base_color("dirty")
                .roughness(Levels::new("coarse").out_range(0.85, 0.97))
                .height("height")
                .normal_strength(0.015),
        )
        // How much wall one repeat is. Advisory — see below.
        .tile_metres([2.4, 2.4])
        .build()
}
```

`MaterialGraph` is editable interchange data that Serde reads and writes;
`Material` is the validated snapshot a backend consumes, carrying the inferred
type and period of every port, and the only way to one is `build`. Errors carry
a path: `nodes[height].inputs[b]`, the way an `ashlar` recipe's do.

### `tile_metres`, the one number both halves carry

A graph is authored *per repeat*, and some of what it says is in metres all the
same: `normal_strength` is metres of relief per unit height per repeat, and a
strand layer's lengths and radii are absolute metres. Both are only true at one
repeat size, and that size is set on the other side of the seam, by the
`tile_metres` of the `ashlar::MaterialDefinition` that lays the graph on a wall.

`MaterialGraph::tile_metres: Option<[f32; 2]>` is where a graph says which size
that was. It is advice and nothing else: no bake, lowering, shader key or
texture cache reads it, so declaring it changes no byte any graph ever baked,
and a graph that declares nothing serialises exactly as it always did. What it
buys is that a reader of a shipped graph can see the scale its relief was drawn
at instead of inferring it, and that `ashlar-bevy`, the one crate holding both
libraries, can compare it with what a definition tiles at: `ashlar_bevy::tiling`
is that comparison, and the graph preflight — `read_library_with_graphs` and
`check_library_with_graphs` — logs it. A definition that lays the graph at a
different size warns unless its own `tile_scale` names that factor, which is
how a specimen sheet says it shrinks a material on purpose to fake distance
and how a typo of 2.4 for 1.72 still gets caught. One that lays it at a
different *shape*, or scales a graph it grows strands from, warns whatever
`tile_scale` says, because relief authored in metres of a repeat cannot follow
a stretch and a blade's length is absolute metres.

## Documentation

The founding decision is
[ADR 0003](https://github.com/JedimEmO/ashlar/blob/main/docs/adr/0003-procedural-materials.md).
The Bevy side — the bake cache, the compiled `ProceduralMaterial`, the GPU bake
and the conformance suite between the two backends — is
[the adapter in depth](https://github.com/JedimEmO/ashlar/blob/main/docs/guide/bevy-adapter.md).
The node vocabulary is the rustdoc of `ashlar_material::nodes`, one module per
family.

## Licence

MIT or Apache-2.0, at your option.
