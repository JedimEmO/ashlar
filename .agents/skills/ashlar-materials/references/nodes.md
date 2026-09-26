# Node reference

Every node in `ashlar_material::nodes`, read off `crates/ashlar-material/src/nodes/*.rs`.
Each struct is its own builder: the constructor takes what the node cannot do
without and setters take the rest. Anything typed `Input` accepts a node id
string, a literal (`f32`, `[f32; 3]`, `[f32; 2]`), `Input::param("name")`, or a
node value written inline (hoisted at build under the id `parent.port`).

Port types: `Float`, `Color` (linear RGB), `Vec2`. A `Float` reaching a `Color`
port broadcasts; a `Color` reaching a `Float` port becomes Rec. 709 luminance;
`Vec2` converts to nothing. Port classes used below: **Field** = Float or Color
passed through; **Any** = anything; **Displacement** = Vec2, or a float along
both axes. "SameAs" means the output type is the named input's; "Join" means
colour if either operand is a colour, else float.

Every node lowers on both backends (CPU bake and WGSL shader) unless the table
says otherwise. Every check listed is a build error by node path. Instruction counts quoted
below are from the crate README, section "What a bake costs".

## Generators

Each lays a lattice that wraps at an integer period, which is what makes it tile.
`period` per axis must be in `1..=4096` (`MAX_PERIOD`).

| Node | Builder and fields (defaults) | Output | Period | Checks and notes |
| --- | --- | --- | --- | --- |
| `Uv` | `Uv::new()` | Vec2 | 1x1 | The period says the lattice repeats once, not that the value meets itself: `u` jumps from 1 to 0 at the seam. Safe inside generators, which wrap it; a `Warp` offset built from a bare `Uv` is seamed. Decompose it for a ramp along one axis. |
| `Noise` | `Noise::value()` / `::perlin()` / `::simplex()`; `.period(n)` or `.periods(u, v)` (1, 1); `.seed(0)`; `.octaves(1)` in `1..=12`; `.persistence(0.5)` in `0..=1`; `.lacunarity(2)` in `1..=8`, integer only | Float in `0..=1` | base `period` per axis | Finest octave `period * lacunarity^(octaves-1)` must be `<= 4096`; that finest count is what a bake must resolve. **Simplex validates but is refused by both backends at lowering**: an irrational skew has no integer period. Octave `k` hashes on `seed + k`, so `period(4).octaves(2)` shares a lattice with `period(8).seed(1)`. Perlin reads flatter than value noise at the same amplitude and usually wants a `Levels` after it. |
| `Voronoi` | `Voronoi::new()`; `.period(n)` / `.periods(u, v)` (4, 4); `.seed(0)`; `.metric(Euclidean\|Manhattan\|Chebyshev)`; `.jitter(1.0)` in `0..=1`; `.width(0.05)` in `0..=1`, cell units; `.output(Distance\|Cell\|Edge\|Border)` | Float | `period` | Nine cells searched. Distance and Edge are in cell units and clamped at one; Cell is a random float per cell; Border is one inside a band of `width` along the boundary. About 150 instructions. |
| `Bricks` | `Bricks::new()`; `.rows(4)`; `.columns(2)`; `.offset(0.5)` in `0..=1`; `.mortar(0.02)` in `0..=0.5` exclusive, brick-local units; `.bevel(0.05)`; `.round(0.0)`; `.corner(0.0)`; `.seed(0)`; `.output(Mask\|Bevel\|Id\|Fill)` | Float | `{u: columns, v: rows / bond}` where bond is the rows a running offset takes to return to a whole brick (1 stack, 2 half, 3 third) | Bond must divide `rows` or the build fails at `offset`; write `1.0 / 3.0`, not `0.3333`. Mortar is taken off all four sides, so one column still has a vertical joint. `Mask` is one on the face, zero in the joint. **`round` or `corner` above zero validates but is refused at lowering on both backends.** Lattice for a bake is `[columns, rows]`. |
| `Tiles` | `Tiles::new()`; `.pattern(Grid\|Hex\|Herringbone)`; `.rows(4)`; `.columns(4)`; `.gap(0.02)` in UV; `.bevel(0.05)`; `.seed(0)`; `.output(BrickOutput)` | Float | Grid `[columns, rows]`; Hex `[columns, rows/2]`; Herringbone `[columns/4, rows/4]` | Hex needs even rows; herringbone needs rows and columns divisible by four. `gap * cells` must be under one per axis. Grid is 22 instructions, herringbone 46. |
| `Pattern` | `Pattern::new(Stripes\|Checker\|Sine\|Triangle\|Square)`; `.x(1)`; `.y(1)`; `.mix(Multiply\|Add\|Max\|Min\|Average\|Difference)` | Float | `[x, y]` | `Checker` defaults to and requires `Difference`; any other mix on a checker is refused. |
| `Shape` | `Shape::new(Circle\|Box\|Polygon\|Star\|Capsule\|Gear)`; `.size(0.4)`; `.edge(0.05)`; `.sides(6)` (>= 3 for Polygon/Star/Gear); `.length(0.5)` capsule only; `.depth(0.1)` gear only, `<= size`; `.round(0.0)` `<= size`; `.hollow(0.0)`; `.output(Mask\|Distance)` | Float: one inside for `Mask`, the signed distance in UV for `Distance` | 1x1 | `size + edge <= 0.5` or the shape would be cut at the seam, and for a capsule `length / 2 + size + edge <= 0.5` instead, refused at `.length` — a capsule is the segment of `length` along u with a cap of radius `size` at each end, so `size` is not its reach. Falloff is taken inward from the edge; `Distance` ignores `edge` entirely. A gear cuts `sides` teeth from `size` down to a root circle at `size - depth`, with a tooth pointing along +u where a polygon puts its vertex, and answers a **radial** distance, so the edge ramp is wider on a flank than on a crest. `round` draws the shape smaller and moves the distance back out, which rounds a box, a polygon and a star and folds away on a circle, a capsule and a gear. `hollow` is the onion: the band from the boundary to `hollow` inside it, an annulus of a circle and a frame of a box, taken inward so it never grows the shape. Lays a lattice of one whatever it draws. Scatter it with `Tile` or a `CircleSplatter`; cut one shape out of another by reading both as `Distance`, combining with `SdfCombine` and ramping the result with `SdfMask`. Costs: a disc 14 instructions, a box 17, a capsule 21, a polygon 32, a star or a gear 33; `Distance` is three fewer than `Mask`, a shell four more, a corner radius two more on a polygon or a star and eight on a box. |
| `Scratches` | `Scratches::new()`; `.count(32)` in `1..=4096`; `.length(0.2)`; `.width(0.004)`; `.angle(0.0)` degrees; `.angle_spread(180.0)`; `.seed(0)` | Float | 1x1 | One segment per cell of a square lattice sized to the count. A segment reaching more than three cells is refused at `length`: shorten them or add more. About 370 instructions. |
| `Weave` | `Weave::new()`; `.x(8)` warp threads along u; `.y(8)` weft threads along v; `.width(0.8)` fraction of a thread's own pitch, over 0 and under 1; `.pattern(Plain\|Twill { step }\|Satin { step })`; `.output(Mask\|Height\|Warp\|Weft\|Id)`; `.seed(0)` | Float | `[x / repeat, y / repeat]`, the repeat being 2 for `Plain` and `step` for the other two | **`x` and `y` must both be multiples of the pattern's repeat** (`WeavePattern::repeat()` answers it) or the build fails at `.x` or `.y`: thread `repeat` crosses the way thread zero did, so a count that does not divide leaves a fault down the seam. A twill's `step` is at least 3 — two threads is `Plain` — and a satin's must admit a move coprime with it that is neither 1 nor `step - 1`, which rules out 2, 3, 4 and 6 and nothing else; both are refused at `.pattern.step`. `Mask` is one on a thread and zero in the gap; `Warp` and `Weft` are the topmost thread, and over the cloth each is exactly what the other is not, which makes them a pair of tint masks; `Id` is a hash per thread taken from whichever is on top, one number the length of a thread, and it is the weft's in a gap. `Height` is zero in a gap, three quarters along a thread between crossings and one at a crest passing over, continuous everywhere because the lift is carried by the crossing thread's own cross-section, which is already zero where the pattern changes. Lattice is `[x, y]`, the threads rather than the crossing. Costs: 20 instructions as a mask, 30 as `Warp` or `Weft`, 35 as `Id`, about 50 as `Height`; a satin adds 3 over a plain weave or a twill. |

## Transforms (resamplers)

A resampler re-emits its source at another coordinate, so it costs the
coordinate-dependent part of the source again per read.

| Node | Builder and fields (defaults) | Output | Period | Checks and notes |
| --- | --- | --- | --- | --- |
| `Transform` | `Transform::new(input)`; `.scale(s)` / `.scales(u, v)` (1, 1), nonzero; `.rotate(0.0)` degrees; `.translate(0.0, 0.0)` UV; `.clamped()` sets `repeat = false` | SameAs `input` (Any) | integer scale multiplies the period; a quarter turn exchanges the two axes' periods and the scale then multiplies the exchanged pair (a `4x8` source through a quarter turn and a scale of `2x3` tiles `16x12`); translation keeps it; anything else is `Free` | Coordinate order is scale, then rotation about the repeat centre, then translation: `out(p) = in(rotate(scale * p) + translate)`. The period arithmetic above reads the other way round because it works back from the result's frame to the source's. A clamped transform that moves the coordinate at all is `Free`. Bake lattice is multiplied by `ceil(|scale|)`. |
| `Tile` | `Tile::new(input)`; `.mask(1.0)` (Float); `.count(n)` / `.counts(u, v)` (4, 4); `.overlap(0.0)` instance widths; `.scale_variation(0.0)`, `.rotation_variation(0.0)`, `.opacity_variation(0.0)` all in `0..=1`; `.seed(0)` | SameAs `input` | 1x1, unless `mask` is `Free` | Instances combine by max. `mask` is read once per instance at its cell centre and compared to a per-instance hash: a threshold, not a multiply, so instances thin out rather than fade. A literal mask at or above one emits no gate. `overlap > 0` reads nine cells, so the source costs nine times. Bake lattice is the source's times `count`, or the mask's if finer. |
| `Warp` | `Warp::new(source, offset)`; `.amount(0.1)` UV | SameAs `source` (Any); `offset` is Displacement | lcm of source and offset | `out(p) = in(fract(p + amount * offset(p)))`. A float offset displaces both axes; a colour arrives as its luminance. A whole-unit constant displacement is a translation, which is how `variation` parameters work. |
| `DirectionalWarp` | `DirectionalWarp::new(source, angle)`; `.amount(0.05)` UV | SameAs `source`; `angle` Float in **turns** | lcm of source and angle | Direction from the field, distance fixed: lines bend without pinching. A noise in `0..=1` sweeps the whole circle. |
| `Mirror` | `Mirror::new(input)`; `.axis(MirrorAxis::U\|V)` (U) | SameAs `input` (Any) | **keeps the input's period unchanged** | Folds the whole unit as `0.5 - abs(0.5 - fract(t))`, so an eight-cell source stays eight cells, four of them reflections. This surprised: a fold that halved the count would only meet itself where cells are copies. A `Free` input stays `Free`. |
| `CircleMap` | `CircleMap::new(input)`; `.radius(0.5)` (`<= 0.5`); `.inner(0.0)` (`< radius`); `.turns(1)`, `.rings(1)` (`>= 1`); `.twist(0.0)` turns of angle per unit of normalised radius; `.outside(0.0)` the fill past the window | SameAs `input` (Any) | 1x1 whatever the source — but the source must itself tile, or it is refused at `nodes[id].inputs[input]` | A **windowed** polar read: with `n = (r - inner) / (radius - inner)` about the centre of the repeat, `out(p) = in(fract(theta / tau * turns + twist * n), n * rings)`, and `outside` everywhere past either radius. Windowed because a polar frame over the whole repeat does not tile at all; a disc inside the repeat with a fill round it does, which is the same rule `Shape` lives under. The source's tiling is required because the angular axis wraps into its u. **The centre pinches**: every angle meets at `r = inner`, so a source whose `v = 0` row is not constant knots there — read a gradient at the middle and put the noise further out, or open a hole with `inner`. Bake lattice is the finer of `lat_u * turns` and `ceil(lat_v * rings / (radius - inner))`, in both axes. About 24 instructions over the source, which it re-emits once. |
| `CircleSplatter` | `CircleSplatter::new(input)`; `.mask(1.0)` (Float); `.count(8)` instances per ring; `.rings(1)`; `.radius(0.35)` outermost ring; `.inner(0.0)` standoff; `.scale(0.2)` instance width in UV (> 0); `.scale_variation(0.0)`, `.rotation_variation(0.0)`, `.radius_variation(0.0)`, `.opacity_variation(0.0)` all in `0..=1`; `.face_centre()`; `.seed(0)` | SameAs `input` (Any) | 1x1, unless `mask` is `Free` | A `Tile` on a circle. Instances combine by max; an instance is the source's unit drawn `scale` across, windowed to that unit, and `mask` is read once at its own centre against its own hash — a threshold, not a multiply, so bolts thin out rather than fade. Rings stand at the outer edge of the bands they cut `inner..=radius` into, so the outermost is at `radius` whatever `rings` is and a single ring is the ring at `radius`. Checked by `radius + scale * (1 + scale_variation) * reach <= 0.5`, where `reach` is `0.5` for an instance square to the repeat and `1 / sqrt(2)` once `.face_centre()` or a `.rotation_variation()` turns it — an instance is a square, and a turned square reaches on its corner. That check is what keeps every instance off the seam and is why the period is 1x1, so a ring that fits square may still be refused at `.radius` facing. **Costs three source reads per ring** — the instance nearest in angle and its two neighbours — so an instance wider than two slots is clipped rather than drawn; keep `scale` under the spacing `count` leaves. The variations of size and place only ever take away, so the check above stays a bound; the rotation is the one the check reads instead. Bake lattice is `ceil(source / scale)` per axis, or the mask's if finer. Measured: 80 instructions over a `Uv` axis costing 6 for a plain ring, 192 with every variation and a masked gate, 343 for two such rings. |
| `Kaleidoscope` | `Kaleidoscope::new(input)`; `.count(4)` (>= 1) | SameAs `input` (Any) | 1x1 when `count == 4`, whatever the source, `Free` otherwise | Four sectors is a quadrant fold on both axes and tiles even over a free source; any other count is an angular fold whose wedge edges are diagonals. Bake lattice doubles at four. |

## Pointwise filters

One texel in, one out. Period is the least common multiple of the inputs the
node reads; a warning is raised when that is larger than every input.

| Node | Builder and fields (defaults) | Output | Checks and notes |
| --- | --- | --- | --- |
| `Blend` | `Blend::new(mode, a, b)`; `.opacity(1.0)` (Float input, may be a mask) | Join(a, b), both Field | Result is `mix(a, f(a, b), opacity)`: opacity zero is `a` untouched for every mode, and `Subtract` at `0.16` is `a - 0.16 * b`. Modes: Normal (`b`), Multiply, Screen, Overlay, Add, Subtract, Difference, Lighten, Darken, SoftLight (Pegtop), Dissolve. **Dissolve treats opacity as a threshold** against a per-texel hash on a 4096 lattice rather than as a weight. |
| `Levels` | `Levels::new(input)`; `.in_low(0.0)`, `.in_high(1.0)`, `.in_range(lo, hi)` (must not be empty); `.gamma(1.0)` (> 0); `.out_range(0.0, 1.0)`; `.luminance()` sets channel | SameAs `input` (Field) | `t = clamp((x - in_low) / (in_high - in_low), 0, 1) ^ gamma`, then `mix(out_low, out_high, t)`. So it clamps as well as remaps; a value past the input range saturates. Also the crate's clamp-and-pow: `Levels::new(x).gamma(2.4)` is `clamp(x)^2.4`. |
| `Curve` | `Curve::new(input)`; `.points([[x, y], ...])` (identity), >= 2 points, strictly increasing x | SameAs `input` (Field) | Monotone cubic (Fritsch and Carlson); holds end values outside the range. |
| `Colorize` | `Colorize::new(input)`; `.gradient([(position, [r, g, b]), ...])` (black to white); >= 1 stop, increasing positions | Color | Input is Float (a colour arrives as luminance). Piecewise linear in **linear** RGB. |
| `Adjust` | `Adjust::new(input)`; `.brightness(0.0)` added; `.contrast(1.0)` about 0.5; `.hue(0.0)` degrees about the grey axis; `.saturation(1.0)` >= 0 | Color | Applied in the order hue, saturation, contrast, brightness. Input is Color; a float broadcasts. |
| `Math` | `Math::new(op, a, b)`; `Math::unary(op, a)` | Join(a, b), or SameAs `a` for unary ops | Ops: Add, Sub, Mul, Div (zero where `b` is zero), Min, Max, Pow, Step, Smoothstep, Atan2; unary Abs, Sqrt (zero below zero), Floor, Fract, Sin, Cos, Log2, Exp2. `Step`: one where `a >= b`. `Smoothstep`: ramp from zero at `b` to one at `b + 1`. **Sin, Cos and Atan2 are in turns**, not radians; `Atan2` answers the angle of `(x: b, y: a)`. A unary op never reads `b`, so `b` moves neither type nor period. |
| `Decompose` | `Decompose::new(input, Channel::R\|G\|B)` | Float | The only node that reads a Vec2: R is u, G is v, and B on a Vec2 is refused. A float input is passed through. |
| `Combine` | `Combine::new(r, g, b)` | Color | Three Float ports. |
| `Invert` | `Invert::new(input)` | SameAs `input` (Field) | `1 - x` per channel. |
| `Clamp` | `Clamp::new(input)`; `.range(low, high)` (0, 1), both Float inputs | SameAs `input` (Field) | |
| `Mix` | `Mix::new(a, b, t)` | Join(a, b); `t` Float | `a + (b - a) * t`; at `t = 0` exactly `a`, bit for bit, which is why the study gates edits through `Mix` rather than a multiply. |
| `Switch` | `Switch::new(condition, on_true, on_false)` | Join(on_true, on_false) | Condition at or above a half takes `on_true`; a `Param::bool` arrives as 0 or 1. |
| `SdfCombine` | `SdfCombine::new(SdfOp::Union\|Intersect\|Subtract, a, b)`; `.smooth(0.0)` fillet width in UV, in `0..=1` | Float; both ports Float | A boolean over **signed distances**, not masks, answering a distance so booleans chain. Negative is inside, so Union is `min(a, b)`, Intersect `max(a, b)`, Subtract `max(a, -b)` — `a` is what a subtraction keeps. At `smooth == 0` it emits exactly that hard op; above zero it is the polynomial smooth minimum, `mix(b, a, h) -/+ k * h * (1 - h)` with `h = clamp(0.5 -/+ 0.5 * (b - a) / k, 0, 1)`, which rounds the crease over a band `smooth` wide and leaves everything further away untouched. The filleted field is approximately a distance, so a mask taken right at a fillet ramps a little narrow. Costs: 1 instruction hard (2 for a subtraction), 11 to 13 filleted. |
| `SdfMask` | `SdfMask::new(input)`; `.edge(0.05)` in `0..=1` | Float mask; port Float | The inward ramp `Shape` applies to its own distance, as a node: one inside, zero outside, `edge` of falloff taken inward so the mask reaches exactly as far as the field's boundary. `SdfMask::new(shape.output(Distance)).edge(shape.edge)` **is** `shape` under `Mask`, instruction for instruction — pinned by a test — so combine distances and mask once at the end. Zero is a hard edge. Costs 3 instructions. |
| `NormalFromHeight` | `NormalFromHeight::new(height)`; `.strength(0.02)` in `0..=1`, metres per unit height per repeat | Color in `-1..=1` (a direction, not encoded) | **Buffered in a bake**: the height is rasterised into a plane and differenced over texels, so it costs a plane (shared with any node rasterising the same height). Only needed to blend normals; `PbrOutput` derives the material's own normal from `height`. |
| `HeightToMask` | `HeightToMask::above(input, low)`, `::below(input, high)`, `::band(input, low, high)`; defaults low 0.5, high 1.0; `.softness(0.1)` in `0..=1`; `high >= low` | Float mask | Two smoothsteps multiplied. `softness` is the **full** ramp width, half either side of each bound (unlike `WorldMask`, whose softness is a half width). Zero is a hard band. |

## Buffered filters (the bake boundary)

Each rasterises everything upstream into a plane at the bake resolution, runs
its filter over that plane with wrap, and reads the result back bilinearly.
Every radius is a reach in UV, so the same graph describes the same surface at
every resolution; a reach under half a texel is an identity. Two nodes asking
for the same filter over the same expression share one plane; an unread node
leaves none. In a shader they cannot run per fragment, so the compiler cuts the
graph there: a live parameter that reaches one is **frozen** (folded and named
in the cost report) and a runtime input above one is refused. Period is the
input's own.

| Node | Builder and fields (defaults) | Output | Notes |
| --- | --- | --- | --- |
| `Blur` | `Blur::new(input)` Gaussian; `Blur::directional(input, degrees)`; `Blur::slope(input, height)`; `.radius(0.01)` in `0..=1` UV; `.kind(Gaussian\|Directional\|Slope)`; `.angle(0.0)`; `.steps(8)` in `1..=16` (slope only) | SameAs `input` (Field) | Gaussian is truncated at the radius with sigma a third of it. Directional smears along one angle only: brushed metal. Slope walks each texel `steps` displacements of `radius / steps` down `height` and averages the source where it stopped, so the field travels *uphill*: rust out of a chip onto paint, dust out of a joint onto the brick above. `height` is read by the Slope kind alone and costs a second, unfiltered guide plane; period follows only what the kind reads. A slope blur over a hard-edged source with a nearly flat guide is the one construction that can put the CPU and GPU bakes outside the conformance budget; nothing refuses it. |
| `Curvature` | `Curvature::new(height)` signed; `::cavity(height)`; `::peaks(height)`; `.radius(0.01)`; `.kind(Laplacian\|Blurred)`; `.strength(8.0)` gain; `.output(Signed\|Peaks\|Cavity)` | Float; Signed is centred on 0.5, Peaks and Cavity are masks in `0..=1` | Answers a *difference* of heights (a crest a fiftieth proud answers a fiftieth), so `strength` is the gain that makes a mask. Peaks and Cavity over one height at one radius share one plane. Hollows are dirt and damp; crests are polish and chipping. |
| `EdgeDetect` | `EdgeDetect::new(input)`; `.radius(0.004)`; `.strength(1.0)` >= 0 | Float mask | Length of the two central differences over the radius, clamped. A step edge answers one whatever the radius; the radius sets line width. Finds an arris from a brick mask, a lip from a chip mask. |
| `OcclusionFromHeight` | `OcclusionFromHeight::new(height)`; `.radius(0.02)`; `.strength(1.0)` in `0..=1` | Float, one is open | Horizon march over eight directions. **Reads the height as a length in UV units**: feed it `height * normal_strength` (the study's `relief` helper), not the raw `0..=1` field, or the result is black. |
| `Distance` | `Distance::new(input)`; `.threshold(0.5)`; `.range(0.1)` in `0..=1`, > 0 | Float in `0..=1` | Jump-flood distance over the torus from texels at or above the threshold, divided by `range` and held at one. An empty mask answers one everywhere. Invert it for a halo around a line. |
| `Erode` / `Dilate` | `Erode::new(input)`, `Dilate::new(input)`; `.radius(0.01)` | SameAs `input` (Field) | Min or max over the **square** neighbourhood of the radius; an erode then a dilate returns an axis-aligned mask unchanged. |
| `Buffer` | `Buffer::new(input)`; `.resolution(n)` power of two in `16..=4096`, else the bake's | SameAs `input` (Any) | A cut with no filter: pays a sub-expression once, or pins a plane's resolution. |

## Runtime inputs

Not fields over UV. All carry the unit period, so they cost a pointwise node
nothing. Where they mean anything is a `Surface::Shader`.

| Node | Builder and fields | Output | In a bake | In a shader |
| --- | --- | --- | --- | --- |
| `Time` | `Time::new()` | Float seconds since start | zero (the picture at startup); not reported | the clock; drive it through `Math` `Sin` in turns |
| `CutFlag` | `CutFlag::new()` | Float, exactly 0 or 1 | zero, and `BakeReport::cut_flag` says so | one on faces the kernel cut, off the per-vertex attribute |
| `WorldPos` | `WorldPos::new()` | Color carrying xyz metres | **refused** at the node, even if no output reads it | fragment world position |
| `WorldNormal` | `WorldNormal::new()` | Color carrying a unit normal | **refused** | fragment world normal |
| `Triplanar` | `Triplanar::new(source)`; `.tile_metres(1.0)` > 0; `.sharpness(4.0)` >= 1 | SameAs `source` (Field) | **refused** | source read three times by world position, weighted by the normal. Source must tile (checked). Costs the source up to three times. Unused by any study graph. |
| `WorldMask` | `WorldMask::up()`, `::facing(WorldAxis)`, `::above(metres)`, `::below(metres)`; `.field(Normal\|Position)`; `.axis(X\|Y\|Z\|NegX\|NegY\|NegZ)`; `.threshold()` (0.5 cosine for facing, metres for position); `.softness(0.25)` half width, >= 0 | Float mask | **refused** | smoothstep on one component of the normal or position; `up()` is faces within sixty degrees of pointing up |

## Structure

The two ends of a compound. A graph declares what it takes with `GraphInput`
nodes and exports what it decided with `PbrOutput::extra(name, input)` — named
float masks beside the six PBR channels, which a bake writes no map for and a
shader binds nothing for, and which only a `Subgraph` reading
`SurfaceOutput::Extra(name)` ever sees. An extra may not take one of the six
channel names, and it must tile and joins the material's repeat like any other
bound output.

| Node | Builder and fields | Output | Notes |
| --- | --- | --- | --- |
| `GraphInput` | `GraphInput::float(name, default)` / `::color(name, [r, g, b])` / `::vec2(name, [u, v])` | the declared type | A typed signal the graph takes from whatever instances it, written as a node in the graph's own `nodes` map. No input ports. Unbound it is its default literal at period 1x1 and lattice 1x1, so a compound still builds, bakes and previews alone and its defaults are the picture its author chose; bound by a `Subgraph`, it is that field converted at the declared type, carrying that field's period and lattice inward. The name must pass the usual name rules and be unique in the graph; the default must be of the declared type and finite. Not `Input`, which is the enum a wired port holds. |
| `Subgraph` | `Subgraph::new("key")`; `.param(name, ParamValue)`; `.input(name, value)`; `.output(SurfaceOutput::BaseColor\|Roughness\|Metallic\|Occlusion\|Height\|Emissive\|Extra(name))` (BaseColor) | the chosen output's type, at that output's period | Inlined at lowering into the same arena and shared, so four subgraph nodes reading four outputs of one graph compute it once. Parameters bound here are **folded, never live**; an input is a whole field and is not folded at all. `.input` wires anything an input port takes into a `GraphInput` of the instanced graph — a name it does not declare, or a type its declaration does not admit under the two conversions, is refused with what it does declare. Because a bound field carries its type, period and lattice inward, the instance is **inferred again per distinct binding**: two nodes binding the same fields share one instance, two binding different ones are two. Needs a library: build with `MaterialGraph::build_in` or `MaterialGraphLibrary::build`, or the key is unknown. Reading Height or Emissive from a graph that binds none is an error, as is an `Extra` name the graph does not export through `PbrOutput::extra`; recursion is refused with the cycle named, by key alone, whatever is bound. Warnings raised inside the instanced graph surface under `graphs[key]`, or `graphs[key@<hash>]` when something is bound. An error inside an instance that binds something is reported at this node's own path instead, carrying the inner `graphs[key@<hash>]...` path in its message, because what failed may be what this node wired in; an instance that binds nothing fails under its own key, as it always has. |
| `Comment` | `Comment::new(text)` | none | Ignored by every backend; wiring one as an input is an error. |

## Per-node lattice for a bake

A bake at resolution `r` is refused (`BakeError::Lattice`) when the finest
lattice anywhere in the graph, including unread nodes and instanced graphs,
exceeds `r`. Generators count their own cells (a noise its finest octave, a wall
its rows and columns). `Transform` multiplies by `ceil(|scale|)` and swaps axes
on an odd quarter turn; `Tile` multiplies its source by `count`; a four-sector
`Kaleidoscope` doubles; a `CircleMap` answers the finer of `lat_u * turns` and
`ceil(lat_v * rings / (radius - inner))` in both axes, because it mixes them; a
`CircleSplatter` answers `ceil(source / scale)` per axis, or its mask's count
where that is finer; a `Weave` answers its thread counts rather than the
crossing they repeat at;
everything else carries the largest that reached it. A
field bound into an instance is counted by the graph that bound it and by
everything the instance does to it, so a period-512 noise through an input
refuses a bake at 256, and an inner `Transform` of scale 2 over the same field
asks for 1024.
