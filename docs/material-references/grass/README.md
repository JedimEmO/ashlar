# Dense cut grass

The reference is the BlenderKit material sphere supplied with this task. Its
material page, flat maps, physical scale and lighting setup were not supplied,
so this pass matches the visible surface character under Ashlar's fixed studio
rig rather than claiming pixel agreement with hidden source maps.

The graph is `library:grass` in
[reference_grass.rs](../../../crates/ashlar-material/src/stdlib/reference_grass.rs).
`just preview --scene detail-grass` is a two-metre patch of it with its blades
standing, and "Reproduce" below makes the studio render and the flat maps.

## Read of the reference

The two spheres are the same size in the frame, so a feature can be measured on
one and compared with the other. That only becomes a statement about
millimetres if the reference sphere shows the same number of repeats ours does.
Ours is `Sphere::new(2.0 / PI)` — a *radius*, so the ball is 1.273 m across —
with its `u` doubled, which is two two-metre repeats around a four-metre
circumference. Nothing supplied with the reference says what its tiling is.
Everything below is therefore: at the tiling that makes the two pictures the
same picture, here is the grass.

- Dense turf with no exposed soil, and no flat ground visible anywhere between
  the blades.
- Tufted and combed. The blades of a tuft lie the same way and fairly flat;
  neighbouring tufts lie differently. Detrend the sphere's radius against the
  sphere and the residual crosses zero 354 times a turn, which is a lump every
  23 mm of arc — those lumps are tufts, and a 64-cell clump lattice over two
  metres is a tuft every 31 mm, which is the same thing.
- Long, coarse strokes: 12 to 35 mm at the scale above, not the 4 to 6 mm of
  even speckle our first pass drew.
- Strong variation at every scale coarser than a blade. Band-pass the luminance
  between two blurs, which takes the rig's own gradient across the ball out of
  the number, and the reference deviates by 0.44 of its mean in the 2 to 10 mm
  band, 0.31 in the 10 to 30 mm band and 0.19 in the 30 to 90 mm band: patches
  of tan thatch, patches of bright yellow-olive and patches of darker green,
  over a fine bright-clump-and-black-void grain.
- Yellow-olive rather than green. Its mean linear red is 0.92 of its green.
- Matte, with small blade highlights.

### How tall the pile is

The old doc said 18 mm and gave no working. The reference's rim gives one. Its
band of partial coverage — from the first pixel that is not background to the
first that is solid — is 10.1 mm deep at the median and 22.0 mm at the
ninetieth percentile, with single strands out past 30 mm; its detrended radius
runs from −8.8 to +10.0 mm. A blade leaning as far as this lawn's do stands
about half its length proud, so a crown whose ninetieth percentile is 22 mm is a
blade of about **40 mm**, which is also what a domestic lawn is cut to. That is
[`HEIGHT_SCALE_METRES`](../../../crates/ashlar-material/src/stdlib/reference_grass.rs) now.

At 18 mm the same measurement of *our* sphere gave a 3.7 mm median band and
nothing past 10 mm, which is the even fine fuzz the render had instead of a
lumpy crown. The declared height was the single number most of the flatness came
from.

The vendor sphere includes directional lighting, tone mapping and likely true or
adaptive displacement. Those are part of the appearance but not base-colour
data. Ashlar's comparison uses its own documented studio rig and derives normals
from one height field.

## Construction

`library:grass` is in three sections, and the order matters.

The **bed** is noise alone, at five scales rather than three, because the two
scales that were missing are the ones the reference varies over hardest: a
four-cell field makes metre-scale patches, a 16-cell field makes hand-sized
ones, a 32-cell fractal field groups the growth, a 64-cell field makes
tuft-sized ones and a 128-cell field breaks up the surface between blades.

A fractal noise puts most of its amplitude in its base octave, which is why the
tuft-sized field had to be its own generator: `clumps` at a 62 mm cell and `mid`
at 125 mm both answer in the band *above* the tufts, and no amount of gain on
either closes a gap at 20 mm.

Those five are summed into `bed_mix` and then **stretched**, and the stretch is
the correction this pass owes the last one. Five noises averaged together do not
make a wider field than one of them; they make a narrower one, because their
deviations cancel. The sum runs from 0.47 to 0.67 between its fifth and
ninety-fifth percentiles, so every colour ramp below it was being read through a
fifth of its own range and the whole lawn came out one tone. `bed_tone` is that
sum through a `Levels` of gain five and a half, and it is what the two strand
colour ramps and the crown ramp are all read from.

`dryness` is a new exposed parameter, and it earns one because it says something
`lushness` cannot. `lushness` says what colour the whole lawn is; `dryness` says
how much of it disagrees, and where. A contrast-stretched hand-and-arm-scale
field is pushed either side of zero by the parameter and subtracted from
`lushness` locally, so the lawn gets *patches* of tan thatch instead of an
evenly olive cast. The dry ramp is tan now rather than a yellower green, because
mixing towards a yellower green only desaturated.

The **strands** are three layers over the same 256-cell lattice, or a divisor of
it.

- `blades` is the coarse half: one per cell, 40 mm long and 2.2 mm wide, leaning
  between a third and two thirds of a quarter turn along the contours of the
  clump field, at two segments. 62 251 of them.
- `fibres` is the fine half and it is what makes the mat a mat: four per cell,
  17 mm long and 1.7 mm wide, lying over further, at one segment. Each is sunk
  by up to a third of its own length, so what sticks out of the mat is often an
  end rather than a whole strand. 204 799 of them.
- `stragglers` is new, and it is what the silhouette is made of. A sparse
  upright few on half the lattice — 7 554 over four square metres — at the full
  40 mm and leaning only six to twenty-two degrees, where a blade leans forty.
  Their tips mix towards the dry ramp rather than the crown, because a blade
  that escaped the last cut is the one that went to seed.

All three are gathered into tufts on a 64-cell clump lattice. The share is 0.88
for the blades where it was 0.65, and that one number is what decides whether a
lawn is tufted or speckled: it is the fraction of every variation taken from the
tuft's hash rather than the strand's, so at 0.88 the sixteen blades of a tuft
agree about which way they lie to within a dozen degrees while the tuft beside
them disagrees by most of the range. At 0.65 they disagreed with each other
nearly as much as with their neighbours, and sixteen blades disagreeing inside
three centimetres is a speckle.

A full-detail two-metre repeat is **688 818 triangles** against a budget of
800 000, and
[a test asserts that number](../../../crates/ashlar-material/tests/references.rs) so
it and the doc comment cannot drift apart.

The **surface** is the same strands seen from above. Twelve `StrandRelief` nodes
splat them into planes — five of `blades`, four of `fibres` and three of
`stragglers`. `Mass` is every contribution *summed* rather than a union, so it
keeps rising where strands cross; `pile` is every layer's mass together, and it
is the mat the standing strands ride on.

Two things about which way the light goes, one fixed in the pass before this and
one in this pass.

The first: a lawn is a canopy, so the ground under it is the darkest thing in
frame and the crown is the brightest. `crown_color` mixes *up* from
`under_color` towards the strand colours, and `recess_shade` then shades the
result by how high the canopy stands over the texel. The pass before this
started from a fully lit bed and darkened wherever a strand stood, which puts
the shadow on the one thing that is in the light.

The second: it mixes up by `mat`, which is the mass, and not by `cover`, which
is the union. A union saturates slowly, so even a closed mat only reached about
half of it, and a mix weighted by that can never arrive at the strand colour —
the crown was two parts dark ground to one part blade everywhere. That is most
of why our sphere came back at half the reference's brightness while matching
its black point exactly. Two overlapping strands is a floor you cannot see
through, so that is where `mat` tops out.

`crown_ramp` is the third fix and it is the quietest. Every tip in the material
was mixed two thirds of the way towards *one constant colour*, and a tip is the
brightest and most visible part of a blade. Two thirds of the lawn's crown
therefore carried no spatial variation at all, however wide the bed's range was.
The crown is a ramp off `bed_tone` now, like everything else.

The height is `pile` and the noise fields plus `ridge`, which is `crown`
multiplied by the coverage that drew it. `Height` alone is the topmost
contribution and nothing else, so it steps from a full blade to bare surface
across one texel, and at a millimetre a texel that step is a vertical wall that
saturates the normal map.

The relief and the geometry are therefore the same strands rather than two
drawings that have to be kept in step — the same scatters, read through field
planes at the same 512, whatever resolution the surface is baked at.

`lushness`, `tip_brightness`, `roughness` and `dryness` are exposed parameters.
`patches`, `mid`, `clumps`, `bed_tone`, `dry_mask`, `relief_cover`,
`fibre_cover`, `straggler_cover`, `relief_height`, `pile`, `mat`, `canopy` and
`tip_mask` are exported as diagnostics.

The bake uses 2048² maps. At a two-metre repeat this is about 0.98 mm per texel;
the 40 mm height scale gives `normal_strength = 0.040 / 2.0 = 0.020`. A blade is
2.2 mm and a fibre 1.7 mm, which are both comfortably over one texel there —
under one texel the splat can no longer draw a strand and the bake says so by
path, which is why the 512² stage diagnostics below print a lattice warning.

## How close it is

Measured on a 45 cm window of each sphere, at the same scale, after the pass:

| | Reference | Ashlar | |
| --- | --- | --- | --- |
| Mean luminance | 0.170 | 0.166 | matched |
| Luminance deviation | 0.140 | 0.141 | matched |
| Fifth / ninety-fifth percentile | 0.014 / 0.450 | 0.012 / 0.451 | matched |
| Linear red over green | 0.919 | 0.919 | matched |
| Linear blue over green | 0.251 | 0.264 | a little blue |
| Band-pass 2–10 mm | 0.436 | 0.479 | a little strong |
| Band-pass 10–30 mm | 0.307 | 0.254 | **83 per cent** |
| Band-pass 30–90 mm | 0.192 | 0.198 | matched |
| Silhouette deviation | 5.6 mm | 4.0 mm | **71 per cent** |
| Rim fringe, median / p90 | 10.1 / 22.0 mm | 8.3 / 15.6 mm | **82 / 71 per cent** |

Before the pass those last four read 0.220, 0.131, 1.5 mm and 3.7 / 6.0 mm, and
the mean luminance was 0.084 against the reference's 0.170.

## Remaining gap

**The tuft band is five sixths of the reference's, and it is the last real
difference.** Put the two 12 cm crops side by side and the reference's dark is
near-black voids between bright clumps, while ours is a darker version of the
same continuous mat. Three things were tried against it and measured: raising
`clump_tint` from 0.42 to 0.66 moved the band by 0.002 and cost brightness;
adding a dedicated 31 mm noise moved it by 0.011; adding tuft-scale relief to
the height field, where the 15 mm occlusion radius would pick it up, moved it by
nothing. What did move it, by 0.03, was darkening the floor the mat stands on.
That says the remaining gap is structural rather than a parameter: the reference
has holes in its canopy at the tuft scale and ours has a continuous canopy with
a darker floor under it. Closing it means the clump making a *dome* — a tuft
whose strands converge and whose neighbours leave a gap — rather than a huddle
with converging tips.

**The silhouette is seven tenths as ragged.** The straggler layer took it from
half to seven tenths for 30 216 triangles, and more of them keeps helping, but
the two rims are made differently: the reference's is a displaced height field
in Cycles, which gives soft lumps several millimetres deep, and ours is thin
blade geometry, which antialiases into a finer fringe. That is a difference in
mechanism, not in tuning.

**The strands do not self-shadow.** A strand is a small fraction of a shadow
texel at any cascade that covers a scene, so `cast_shadows` is off: with it on,
two hundred thousand blades cast blocky black holes with the stair-stepping of
the shadow map in them rather than dappling. The pile's own occlusion is in the
texture, so the *bed* has depth and the standing geometry does not, and there is
no contact shadow where a blade meets the mat.

**The far level of detail steps its blade length down in one jump** rather than
shrinking across the band, so a slow camera can catch the switch at the 7 m
line. A continuous shrink would scale a vertex towards its own root, and a
vertex carries how far along the strand it is rather than where the strand
began; carrying the root is a second three-float attribute on every strand
vertex, about 17 MB over one full-detail repeat of the benchmark lawn
(measured 2026-09-19), and that was not worth the smoother line.

**A card is two crossed quads**, so a lawn seen from almost directly above shows
them edge on and is the relief plus a fine speckle. That is the right answer for
a patch of ground and the wrong one for the `detail-grass` wall, where the
strands grow horizontally out of a vertical face: the card band there is carried
by the silhouette rather than by the face. `card_metres` is still 14 m, which
was sized for an 18 mm pile and is now conservative for a 40 mm one; moving it
out would make the two far captures below compare the wrong pair of things, so
it was left where it is.

**And the relief is a top-down splat**: a strand leaning *out of* the surface
contributes only its footprint, so a very upright layer reads as dots rather
than as strokes. That is why the mat leans as far as it does, why `stragglers`
contributes flecks to the texture and strokes to nothing, and why
`library:moss-carpet`, which stands up, accepts dots.

## Strands at four distances

The `detail-grass` patch at 0.75 m on a grazing angle, at 2 m, at 9 m and at
18 m. Close up the strands stand off the surface and break its silhouette; at
2 m a 40 mm pile is a few pixels and the geometry roughens the edge; at 9 m the
geometry has been replaced by cards, two crossed quads for each of the lawn's
clumps; at 18 m it is past `card_metres` and what is left is the relief alone,
which is the same strands.

All four are taken at the material studio's own key and fill rather than the
gallery's (`--reference-key 6500 --reference-ambient 150`). The gallery frames
whole buildings and lights them to read across a facade; at the scale of a
material that same rig flattens every recess the relief has and blows the
shadows off the pile.

## Reproduce

```sh
# The maps, strand set and level-0 PNGs, under target/grass-pass/materials/library/grass/.
cargo run --release -p ashlar-showcase --example export-materials -- \
  target/grass-pass 2048 grass png

just preview --scene sheet-grass
just preview --scene detail-grass
```

The studio swatch, which is the image compared against the reference:

```sh
cargo run --release -p ashlar-preview --example material-swatch -- \
  library:grass target/grass-pass/sphere.png sphere target/grass-pass
```

The swatch grows its strands from the graph compiled *into* it, and loads its
maps from disk. Rebuild it after every graph edit or it will render new textures
under the previous pass's geometry, which costs a round of confusing
measurements.

The four captures above, which want a camera and a rig at the scale of the
material rather than of the model:

```sh
cargo build -p ashlar-preview
./target/debug/ashlar-preview --references target/grass-close \
  --reference-scenes detail-grass --reference-distance 0.75 \
  --reference-view=-1.05,0.10 --reference-ambient 150 --reference-key 6500
./target/debug/ashlar-preview --references target/grass-mid \
  --reference-scenes detail-grass --reference-distance 2.0 \
  --reference-view=-0.9,0.18 --reference-ambient 150 --reference-key 6500
./target/debug/ashlar-preview --references target/grass-cards \
  --reference-scenes detail-grass --reference-distance 9.0 \
  --reference-view=-0.9,0.18 --reference-ambient 150 --reference-key 6500
./target/debug/ashlar-preview --references target/grass-far \
  --reference-scenes detail-grass --reference-distance 18.0 \
  --reference-view=-0.9,0.18 --reference-ambient 150 --reference-key 6500
```

The last two share the mid shot's angle, because what they are being compared
against is each other: the same view inside the card band and past it. Each run
writes `detail-grass-custom.png` into its own directory.
