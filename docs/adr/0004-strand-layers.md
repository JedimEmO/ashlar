# ADR 0004: Strand layers are geometry scattered from graph fields

Status: accepted, 2026-09-19.

## Context

ADR 0003 made a material one height field over a continuous surface. The grass
pass showed what that costs. `benchmark:grass` spent six `Tile` scatters of
capsules to *suggest* blades and was still a picture of grass: no silhouette at
the edge of the sphere, no blade standing over its neighbour, no overhang, and
nothing that changed with the view. Fur, moss, carpet, thatch and bristles all
have that shape, so the gap is a kind of surface rather than one material.

The reading we did first says the same thing from the other side. Substance 3D
Designer has no strands at all: its grass materials are Tile Sampler and Shape
Splatter laying *shapes into a height map*, and everything the material knows
about a blade is what survived being flattened. Material Maker and Blender's
hair are the two directions out of that — a richer splatter, or real curves —
and the game accounts of dense grass, the Ghost of Tsushima talk in particular,
are all on the curve side with a clump structure over it.

So the decision is not "how do we draw grass". It is where strands live
relative to the graph that already describes the surface, given that the two
have to agree.

Sources: Substance 3D Designer's [Tile Sampler](https://experienceleague.adobe.com/en/docs/substance-3d-designer/using/substance-graphs/nodes-reference-for-substance-graphs/node-library/texture-generators/patterns/tile-sampler),
[Shape Splatter](https://experienceleague.adobe.com/en/docs/substance-3d-designer/using/substance-graphs/nodes-reference-for-substance-graphs/node-library/texture-generators/patterns/shape-splatter)
and [Splatter Circular](https://experienceleague.adobe.com/en/docs/substance-3d-designer/using/substance-graphs/nodes-reference-for-substance-graphs/node-library/texture-generators/patterns/splatter-circular);
Material Maker's [Splatter](https://rodzill4.github.io/material-maker/doc/node_transform_splatter.html);
Blender's [hair nodes](https://docs.blender.org/manual/en/4.2/modeling/geometry_nodes/hair/index.html).
The Ghost of Tsushima talk (GDC 2021) was not reached; its clump model is taken
from two agreeing secondary accounts,
[a written reconstruction](https://tigerabrodi.blog/grass-in-ghost-of-tsushima)
and [a caption summary](https://gist.ly/youtube-summarizer/procedural-grass-systems-in-ghost-of-tsushima-achieving-art-direction).

## Decision

A material graph gains a second kind of output beside `PbrOutput`: a named
**strand layer**. `MaterialGraph::strands` is a `BTreeMap<String, StrandLayer>`
with `#[serde(default)]`, so every RON file written before this reads
unchanged.

A strand layer reads ordinary graph nodes, but it reads them **once per strand
at the root** rather than once per texel. What comes out is a `StrandSet`: a
deterministic list of strands over one repeat of the material, sorted by a
per-strand rank. `ashlar-material::strands` owns the scatter, the placement and
the mesh, and it needs only `glam`, so nothing below the renderer learns what a
building is.

**One scatter, three consumers.** The mesh builder grows ribbons or tubes from
the set, and a new buffered node, `StrandRelief`, splats the *same* set from
directly above back into planes the PBR half reads. That second consumer looks
like a detour, since the request was about geometry. It is what makes the level
of detail honest: when the blades fade out, what the camera sees is the same
blades seen from above, so the colour and the clump pattern do not change at
the switch. This is ADR 0003's "one lowering, two backends" applied one level
up.

**Strand fields are held to the tiling rule.** They join period inference
exactly as `PbrOutput::extra` does, and the scatter's own `count` joins the
least common multiple. A set that does not meet itself at the seam is as wrong
as a colour that does not.

**Density is a threshold, never a fade.** A strand's own hash is compared
against the `density` field and the strand is there or it is not. Strands that
thin out read as a sparse patch; strands that shrink towards nothing read as a
mistake. `Tile::mask` already argued this and this follows it.

**A level of detail is a cut on the rank.** Every strand carries one more hash
in `0..1`. A level that keeps a quarter keeps the strands below 0.25 and widens
the survivors by `1/sqrt(keep)`, so the coverage is the same and the survivors
are a *subset* — nothing reshuffles at a switch. Levels are Bevy
`VisibilityRange` entities per UV repeat, with dithered crossfade: it costs
draw calls and it costs us no render-graph code.

**Past the last cut a tuft is a picture of itself.** A rank prefix stops paying
when the strands it keeps are under a pixel wide, so the level after the
thinnest strands is *cards*: two crossed quads per clump, wearing the same set
drawn side on. They are alpha **mask** and never alpha blend — ADR 0003 puts
transparency out of scope, and a cutout keeps depth writes, shadows and the
prepass working — and their mip chain is rescaled to hold the coverage level
zero had, because a box filter halves the alpha of every edge texel and a field
of cards would otherwise evaporate at exactly the distance it exists for. Past
the card distance there is still nothing but the relief.

**Strand fields are static, and wind is the renderer's.** A field that reaches
`Time`, `WorldPos`, `WorldNormal`, `CutFlag`, `Triplanar` or `WorldMask` is
refused at the node that asked. A `Live` parameter is not refused; it is folded
at the value the request bound. Movement is a vertex stage over the finished
geometry, which is `ashlar-bevy::wind`.

### What we decided differently once we were writing it

Eleven of these, and each is a place the plan said one thing and the code says
another.

- **`VoronoiOutput::Offset` answers UV, not cells.** `Voronoi`'s distances are
  in cell units, so the offset should have been too. But nothing in the
  vocabulary scales a `Vec2` — `Math` takes a float or a colour — so an offset
  in cells could only be brought back to a displacement by taking it apart and
  putting it together again. Every port that accepts a displacement measures
  one in UV.
- **Angles are turns.** `Direction::from_angle` takes turns, as `MathOp::Sin`
  and `Atan2` already do. A material graph has no `pi`, and an author writing
  `0.25` for a quarter turn is writing what they meant.
- **The runtime refusal is a graph walk, not a lowering target.** A bake
  refuses `WorldPos` inside the lowering, because a bake *is* a `Target`. A
  scatter cannot: it lowers for the shader target so that a graph delivered as
  `Surface::Shader` may still carry world-space inputs in its **PBR** half,
  which is a delivery the design keeps in scope. So `strands::refuse_dynamic`
  walks the graph from the layer's own ports and names the same paths
  validation names.
- **The field resolution is a constant of the crate.** `FIELD_RESOLUTION` is
  512 and is not a parameter of the splat. A buffered filter upstream of a
  strand field has to be rasterised at *some* size, and two scatters at two
  sizes are two scatters: a relief inside a 2048 bake would read a 2048 slope
  where the mesh builder read a 512 one, and the blades in the texture would
  lean differently from the blades standing on them. Four millimetres a texel
  over a two-metre repeat is about one blade wide, which is as fine a question
  as a strand layer can ask.
- **Edge ownership is local.** A root on an edge shared by two triangles is
  planted once, and which triangle owns it is decided from the edge's two
  endpoints and the opposite corner alone — the edge oriented from its
  lexicographically smaller endpoint, the owner the triangle whose corner lies
  to the left. The plan said "the triangle with the lower index", which needs
  the mesh's own ordering; this needs nothing but the three points, so the
  answer does not change when a mesh is rewelded.
- **A clump owns the blade.** The plan had `clump` as a UV offset that gathers
  roots. That port still exists and still works, and it is not enough: the
  Tsushima accounts are explicit that a blade takes its height, direction,
  colour and bend from the clump point it belongs to. So a clump is an object
  with an id, a distance and a tip pull, and `clump_share` of *every*
  per-strand variation is taken from the clump's hash instead of the strand's.
  A lawn is a mat of little bunches that agree with themselves; without this it
  is a comb.
- **The relief gained `Mass` and `Occlusion`.** `Coverage` is a union and stops
  at one, which is the right answer to "is there a blade here" and the wrong
  one to "how much grass is here": two blades crossing read the same as one.
  `Mass` is every contribution summed. `Occlusion` is the same sum weighted by
  the height each contribution stood at. Without them the splat composited over
  nothing, and the graph had to build its pile out of a blur of the coverage,
  which spreads mass sideways instead of summing it.
- **The dark goes under the strands, not on them.** This one was a bug for two
  passes. `root_occlusion` darkens a blade towards its root, and a graph that
  starts from a lit bed and darkens wherever a strand stands puts the shadow on
  the one thing that is in the light. A lawn is a canopy: the ground under it is
  the darkest thing in frame and the crown is the brightest. `benchmark:grass`
  now mixes *up* from a dark bed towards the strand colours by coverage and
  then shades by how high the canopy stands over the texel.
- **A card is keyed on the clump's hash and then split by where it is.** The
  plan said a card is a clump, and a clump is a hash: every member of one
  carries it. Two things are wrong with the hash alone. Four thousand hashes
  over a repeat collide often enough that some card would span two tufts at
  opposite ends of the lawn, and a *set* is a list over one repeat that a
  surface may reach several times — both faces of a slab and all six of a box
  are the ordinary case — so one clump of one repeat is genuinely several tufts
  a hand's breadth apart. A card at the mean of those is a quad in the air
  facing nothing. So the members of a hash are split again: a strand joins the
  first group standing on the same side of the surface and within one clump
  cell's diagonal of it. Keying on the root's *cell* instead was tried and is
  wrong, because the `clump` port moves a root after its clump was chosen and a
  tuft therefore straddles two or three cells.
- **Cards are a plain `StandardMaterial`, so they do not sway.** A card binds a
  normal map, a normal map needs a tangent frame, and the wind extension's two
  generated vertex stages declare position, normal, UV, colour and the wind
  attribute — no tangent. Carrying one through both stages is a change to the
  one shader in this crate that a shadow pass depends on, for a tip travel of
  twelve millimetres at ten metres and beyond, which is a fraction of a pixel.
  So a card is drawn by Bevy's own vertex stage and stands still.
- **Every card of a layer is turned by its own hash.** Two crossed quads on a
  frame the surface handed them puts every card of a lawn in the *same* two
  planes, and a lawn seen from anywhere near along one of them shows its cards
  as a regular grid of slivers on the clump lattice. A turn per tuft leaves the
  same number of quads facing the same way on average and no lattice in them.

### What did not ship

**A continuous last band.** `LAST_BAND_LENGTH` still shortens the last level's
strands in one step at the bake rather than shrinking them across the band with
distance. A continuous shrink has to scale a vertex *towards its own root*, and
a vertex carries how far along the strand it is and not where the strand began;
carrying the root is a second three-float attribute on every strand vertex,
about 17 MB over one full-detail repeat of the benchmark lawn.

## Consequences

A lawn costs what a lawn costs. `benchmark:grass` is about 620 000 triangles
over one full-detail two-metre repeat across its two layers, and the rank cut
is the only thing that makes that affordable at distance. `StrandSettings`
carries a `density` so a scene that cannot afford the authored lawn says so,
and it keeps a prefix rather than thinning at random.

The card level is about 16 000 triangles over that same repeat — one clump in
4096 per layer, two quads each — which is under three per cent of the geometry
it stands in for, plus one shared 256² atlas pair per scatter. What it buys is
measurable rather than argued: at nine metres the card level changes the mean
colour of the lawn by less than one code in 255 and adds about half a per cent
to the pixels it covers, all of it on the silhouette, where a slab of grass
stops ending in a straight line.

Strands need the graph at runtime. `Surface::Graph` and `Surface::Shader`
qualify, and `Surface::Files` qualifies only through its `baked_from` and a
loaded graph library. Generating a set is milliseconds, so there is no strand
file format and no serialized `StrandSet`.

A partition cannot see a strand layer, because it lowers the PBR half alone. A
`Live` parameter that a strand field reads is therefore reported in
`Partition::frozen` as well as in `Partition::live`, and that overlap is the
honest report of a material whose two halves answer differently.

Strand geometry draws through an `ExtendedMaterial`, which is a third material
asset type beside `StandardMaterial` and the compiled `ProceduralMaterial`. A
scene decides once which of them its strands use. At zero wind the extension's
vertex stage displaces by zero before it computes anything, and the frame is
byte-identical to the one a plain `StandardMaterial` drew. A scene that grows
cards spawns *both* types for one layer, because the card level is the plain
one; a caller that spawns strand chunks by hand therefore has two material
handles to carry rather than one.

Out of scope and not blocked: collision and trampling, hair simulation,
order-independent transparency for fine fur, GPU-side scattering, and the
instanced flat-ground fast path.

## Amended 2026-09-20: a strand set has a baked form

Nothing above is withdrawn except one sentence, and the sentence is this, from
*Consequences*:

> Generating a set is milliseconds, so there is no strand file format and no
> serialized `StrandSet`.

Both halves of that turned out to be wrong, and they were wrong in different
ways.

**The milliseconds were a measurement of the wrong lawn.** Scattering
`benchmark:grass` is 528 ms on eight cores in a release build — 112 ms for
`blades`, 370 ms for `fibres`, 46 ms for `stragglers` — and the moss carpet is
204 ms. It *is* milliseconds for the thirty-two-cell lawn in a doctest. The
figure in the original text was never measured against the set this workspace
actually ships. (It also settles a profiling question left open that morning:
the grass bake at 512 spends about 1.1 s in "scatter and splat", and the split
is roughly half each.)

**The premise underneath it no longer holds.** The paragraph above that
sentence says strands need the graph at runtime, and that was fine while a
strand layer was something only a tool grew. The owner's decision of
2026-09-20 is that games consume baked files and must not link the graph
engine, and standing grass is wanted in games now. Half a second was never the
binding cost; the *dependency* was.

So there is a strand file format, and there is a serialized `StrandSet`.

- **`ashlar-strands`** is a new crate holding everything after the scatter:
  `Strand`, `StrandSet`, `place`, `mesh`, the rank-cut levels and the card
  atlas, on `glam`, `serde` and `thiserror`. `ashlar-material` depends on it,
  keeps `scatter` and the `StrandRelief` splat, and re-exports every name from
  `ashlar_material::strands`, so nothing written against this ADR has to move.
  The split is clean because the ADR made it clean: *"`ashlar-material::strands`
  owns the scatter, the placement and the mesh, and it needs only `glam`"* was
  already true of three quarters of the module.
- **`ashlar_strands::file`** is the format. One file per material definition
  holding every layer it grows, little-endian, versioned, with a directory
  entry per layer and a field-major payload. Lossless, so a baked lawn and a
  scattered one mesh to identical triangles and a test says so. Stored rather
  than compressed: zstd takes 15 % off the grass and 18 % off the moss, and
  that does not pay for a decoder in every game that draws a lawn. The header
  records a scheme all the same, so the measurement can be revisited without a
  second version.
- **`StrandSettings::baked_set`** names the file, the way `Surface::Files`
  names its maps. `MaterialDefinition::check` no longer insists that a
  definition growing strands name a graph: a `baked_set` is the other answer,
  and a game's lawn may sit on a `Plain` surface.
- **`ashlar-bevy`'s `strands` is a game feature now**, and `strand-scatter` is
  the tool feature beside it. `cargo tree -e normal --features strands` has no
  `ashlar-material` in it.

Two things the amendment does **not** change.

**The card atlas needs no file.** `cards` is a pure function of a `StrandSet`,
so a game that holds the set draws its own — 0.5 ms for the lawn's 256² pair.
The Consequences section's "one shared 256² atlas pair per scatter" is
answered by the set alone, and there is no second artifact to ship or to keep
in step.

**A lawn costs what a lawn costs, on disk too.** The grass set is 24.09 MiB and
the moss carpet 10.98 MiB, because a strand is twenty-three `f32` and a repeat
is a quarter of a million of them. Both are committed, beside the 87 MiB of
KTX2 the same preset already ships. The exits, in the order they should be
taken: a definition's own `density`, which keeps a rank prefix and so writes a
*shorter* file of the same lawn; and quantisation, which would halve the file
and end the bit-exactness the parity test rests on. Neither is needed yet and
both are cheaper than a third one.
