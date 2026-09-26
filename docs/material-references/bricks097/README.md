# Bricks097

A photogrammetry scan of an old, worn brick wall, rebuilt as `library:brick`
in [reference_brick.rs](../../../crates/ashlar-material/src/stdlib/reference_brick.rs).
Seed 17, reviewed at 2048², 2 m repeat, 40 mm height scale.

`just preview --scene sheet-brick` shows it at three distances and
`--scene detail-brick` up close. The flat side-by-side sheets and the studio
sphere are regenerated with the commands in [the index](../README.md), which
also says how to run a pass.

## Provenance

[ambientCG Bricks097](https://ambientcg.com/view?id=Bricks097), CC0. It is a
scan, so there is no graph to trace and no node parity to claim: the target is
the same kind of wall built the same way, not the same bricks. Nothing from the
scan is committed; to restore the maps:

```sh
mkdir -p target/material-research/ambientcg && cd target/material-research/ambientcg
curl -L -o Bricks097_2K-PNG.zip "https://ambientcg.com/get?file=Bricks097_2K-PNG.zip"
unzip -o Bricks097_2K-PNG.zip
```

The scan is 2048 × 1024 and carries colour, roughness, displacement, both
normal conventions and AO. It has sixteen courses to 1024 px, which is our
repeat's sixteen courses, so `reference-compare.py brick` compares its left
1024 square against our full repeat downsampled to 1024². There is no reference sphere: a square cut from a 2:1 scan
does not tile.

What the scan shows: brick lengths from 70 px headers to 250 px stretchers in
no fixed bond; courses that sag and vary in thickness; rounded, chipped bricks;
wide sandy mortar nearly flush with the faces, lapping a few pixels onto some
edges, with some bed joints raked open for a brick or three; about three
missing bricks per square; mostly deep red-maroon clay with a burnt minority
and a salmon minority; a pale skin flaked into hard-edged lace over the dark
body on many bricks; soot on individual bricks.

## Construction

1. **One lattice.** Eight cells by sixteen courses, hashed with a sine hash on
   the wrapped cell and row indices so every texel that can see a cell agrees
   about it and the wall tiles. Identity and geometry both come off it; an
   earlier version that took identity from a `Bricks` generator and geometry
   from its own coordinates tore straight seams along the courses.
2. **Lengths.** Each lattice line carries a head joint displaced ±0.40 cell, so
   a brick is the gap between two independent draws: triangular about 128 px,
   with a wider range of headers and stretchers. Both sides of a joint resolve
   from the same draws.
3. **Courses.** The course coordinate itself carries a sag, a bed wave and a
   per-course thickness variation, over a low-frequency wall warp.
4. **Plans.** Rounded rectangles with four independent corner draws (2–20 px),
   a one-cell bow, fine fray, and independent per-side insets so a joint is the
   sum of two unrelated numbers. A one-cell `joint_wander` lets a joint taper; wider independent inset
   ranges allow narrow pinches beside broad pointing.
5. **Faces.** Per-brick rise, recess, tilt, crown and arris wear; a few large
   soft spalls; a rare, modestly proud brick. Smooth by construction:
   broadband relief read as cloud laid over the wall every time it was tried.
6. **Mortar.** Nearly flush, dished across its full width, with a bay-scale
   fill level, and `mortar_height = min(mortar_raw, face - 0.028)`: mortar can
   never stand above the brick it was struck against. Without that bound, a
   brick whose relief draws all leaned down lost the height race across its
   whole face and was painted as mortar. Raked runs open a full joint width
   along a bed for a brick or three, to a ragged medium-dark floor.
7. **Holes.** `missing_bricks` picks about one per square at the default seed by a hash on the
   owning cell (a smooth field tore bricks in half); the outline stops inside
   the cell so neighbouring bricks survive. The opening removes most of the
   original arris, with a larger remnant at one end and a shallow shoulder
   blending into rougher rubble.
   The upper recess is dark and the lower bed retains dusty fragments.
8. **Colour.** Three dark clay families picked categorically (burnt, maroon,
   red-brown) with a wide within-brick swing; a per-brick skin state, whole,
   lost, or flaked into hard-edged salmon islands with satellite specks, offset
   per brick so a flake never crosses a joint; per-brick soot with its own ramp
   toward one end; edge dirt multiplied in; sandy aggregate mortar with dark
   voids and a bay tone field; dark dusty rubble in the holes.
9. **Crust.** Lime follows the worn perimeter and a per-brick deposit level,
   broken by continuous fine noise. A narrow threshold ramp gives weathered
   margins rather than square flecks. Warped, interrupted cellular boundaries
   add thin mineral traces on selected faces. The same mask raises height and mortar
   roughness; `lime_residue = 0` removes it. Mortar also carries grey-green
   weathering across patches.
10. **Roughness** follows skin against body, mortar against brick, and soot.

Exposed controls: `joint_mm`, `erosion_mm`, `mortar_fill`, `relief_mm`,
`lime_residue`, `rake_mm`, `missing_bricks`. Nine extras ship as 512²
`stage-*.png`, among them `brick_id`, `brick_mask`, `hole_mask`, `joint_depth`,
`pre_fill_height` and `flaked_skin`. Bake: 4886 ops, one buffered plane, about
1 s.

`mortar_rises_and_erosion_preserves_brick_identity` samples `mortar_fill` at
0.1 and 0.5, so that parameter's range has to include both.

## State

Fifth fidelity pass, 2026-09-19. Compared full maps, native-resolution crops
and the studio sphere. The wall now has visible
broken lime deposits, redder mid-tone clay, less cloudy face variation, a wider
spread of brick lengths and joint widths, stronger course setbacks and longer
raked joints. Reduced the small-scale face relief so the course displacement
reads more clearly. The previously empty default lime branch now contributes;
a regression test checks visible coverage, zero at the off setting, monotonic
coverage and unchanged brick identity.

The follow-up pass breaks the cavity outlines into retained clay fragments,
blends their shoulders instead of cutting vertical rims, corrects the inverted
upper/lower dust shading and gives rubble its own roughness. Fine interrupted
mineral traces supplement the lime islands, salt specks are more visible, and
continuous mortar grain replaces the coarser value-noise clumps. A second
regression verifies that missing bricks only remove height and retain the bond.

A close-up review exposed a curled, almost melted rim around the cavities.
The next correction removes that continuous clay ring, reduces edge distortion
and floor noise, and raises the floor by 2.4 mm. Openings now reach the pointing
while fading before the cell boundary; a larger clay remnant stays at one end.

The final default reduces `missing_bricks` from 0.014 to 0.008 and raises the
floor another 3.2 mm. At seed 17 the cavity mask covers about 1.1% of the repeat,
down from 2.6%, so missing masonry is an occasional detail rather than a motif.

What still differs, by eye:

- The scan's crust has finer branching and more varied colour than our deposits.
- The masonry still follows a half-brick lattice; the scan has more local bond
  changes and broken ends.
- Cavity outlines remain simpler than the scan, which has undercuts and
  larger projecting fragments that a single height field cannot reproduce.
- Mortar grain is more uniform, and the scan has stronger isolated dark runs
  and yellow-green growth along some bed joints.

The reference preview uses unknown lighting and a different repeat density;
judge colour and feature scale primarily from the flat comparison sheets.

## Composed moss variants

`library:brick-moss-light` and `library:brick-moss-heavy` instance this graph
through `Subgraph`, then feed its channels and masks into `weathering:moss`.
The base brick remains the sole source of its layout and fired-clay surface.
Their `moss_amount` control varies coverage; zero exactly recovers the base.
Both have sheets, `sheet-brick-moss-light` and `sheet-brick-moss-heavy`, and
[reference_moss.rs](../../../crates/ashlar-material/src/stdlib/reference_moss.rs)
is the whole of their wiring.

The moss fidelity pass uses the supplied moss-on-stone photograph as a visual
reference for yellow-green tips, dark inter-clump gaps and fine surface detail.
Only the shared moss surface changed: the growth mask remained byte-identical.
Large smooth noise was replaced by smaller warped cushions and irregular fine
shoots. This remains a height-field surface, without individual leaf geometry.
