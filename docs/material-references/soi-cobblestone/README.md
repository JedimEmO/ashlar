# SOI Cobblestone

A real, inspectable Substance Designer graph and its author's exported maps,
rebuilt as `library:soi-cobblestone` in
[reference_cobblestone.rs](../../../crates/ashlar-material/src/stdlib/reference_cobblestone.rs).
Seed 17, reviewed at 2048², 2 m repeat, 25 mm height scale.

`just preview --scene sheet-soi-cobblestone` shows it at three distances and
`--scene detail-soi-cobblestone` up close. The flat side-by-side sheets and the
studio sphere are regenerated with the commands in [the index](../README.md),
which also says how to run a pass.

## Provenance

Retrieved 2026-09-16 from [SOI's Material Pack #1](https://soi.itch.io/substance-designer-material-pack-1),
which the author lists under [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/).
Attribution: SOI / soidev. [Author's artwork page](https://www.artstation.com/artwork/l4ePe).

Committed here, with original bytes and CRLF endings preserved by
`.gitattributes`:

- [Soi_CobbleStone.sbs](Soi_CobbleStone.sbs), graph identifier `CobbleStone`,
  and the presets [1](1.sbsprs), [2](2.sbsprs), [3](3.sbsprs).
- [inventory.json](inventory.json): file hashes, dependencies, node counts,
  node IDs, connections and exposed parameters. A navigational summary; the
  `.sbs` XML is the source of truth.

Not committed: the author's bitmap pack (the free `Soi_CobblesStone_Metallic.zip`
download, about 96 MB, three variants of 2048² TGA maps). The comparison uses
`CobbleStone_01`. To restore the reference maps and the reference sphere:

```sh
# Converts to target/soi-reference/..., where reference-compare.py looks for it.
python3 tools/ashlar-preview/scripts/soi-reference.py path/to/Soi_CobblesStone_Metallic.zip
```

Conversion: BC read as sRGB, NOpenGL kept as the normal map, AO/R/M packed into
ORM, the eight-bit H widened to 16 bits by ×257. The script records the hashes
in a `soi-reference.json` beside the sphere it renders with `material-swatch`.

What is and is not verified about the source: the XML has 164 composition nodes
(13 output bridges), 207 connections and 31 exposed controls, and every
inventoried connection resolves. It uses two Tile Samplers, six Slope Blurs
(all 32 samples), three Warps, two Directional Warps, Bevel, Edge Detect,
Height Blend and a Splatter, with frames for Base, Sloping, Edge Damage,
Surface Noise, Grout, colour and roughness. It has no Flood Fill and no moss
branch. The file reports format `1.1.0.201710`, which is not a Designer build;
its `sbs://` library dependencies are not bundled; the graph was parsed, never
executed. The reference's normal strength and export settings are unknown, so
lit comparisons carry that confound.

Useful source UIDs:

| UID | Role |
| --- | --- |
| 1294158122 | Tile Sampler: per-tile identity |
| 1294217152 / 1294217160 | Cells guide and HQ Blur |
| 1294217226 | Gradient warp of the tile field |
| 1294163021 / 1294177920 | Edge Detect / Bevel |
| 1294202780 / 1294202882 / 1294202973 | Three damage scales |
| 1294208390 / 1295112500 | Per-tile rotated gradient and its warp |
| 1294212340 | Stone surface after detail layers |
| 1294177255 | Splattered grout aggregate |
| 1294186292 | Stone/grout Height Blend |
| 1294731400 | Directional intensity warp of colour per stone |

## Construction

A source-informed native reconstruction, not an importer or a node-for-node
port. Source UIDs are cited in the comments of `reference_cobblestone.rs`.

1. **Layout.** 8 × 10 running bond warped by the gradient of blurred cellular
   noise; a per-stone inset gives asymmetric joints. Plan corners are authored
   rounded rectangles (8–31 px radius, per corner), sides bow at low frequency,
   and a sparse thresholded noise knocks off a few corners.
2. **Damage.** Three Min slope blurs at three scales chip the soft
   pre-threshold mask, so a bite keeps its own lip.
3. **Profile.** Inner and outer jump-flood distances make a signed distance,
   blurred before an authored `Curve` reads it: a straight chamfer to a crisp
   arris, its width scaled per stone and per side. A `pillow` term rounds the
   top inside the arris.
4. **Lift.** One datum for every stone's low side plus a per-stone rise, on a
   crease-free rotated ramp; broad undulation only, with small grain and sparse
   pits.
5. **Grout.** A low bed (0.18) with coarse swell, and graded pebble lattices
   drawn as caps of the Voronoi site distance, each kept inside its cell.
   Height joins by exact maximum plus a quadratic bump that is zero outside the
   join band, so raising the grout can never lower a stone.
6. **Ownership.** A crisp contour at the stone toe, so stone colour and
   roughness cover the whole chamfer. Staining is a tint multiplied in, never a
   mix toward a colour: a gradient warm rim, a thin contact line, and
   thresholded stains decorrelated per stone so none crosses a joint.
7. **Stone face.** Strokes are a directional blur of a broadband seed at three
   per-stone angles, slid per stone, high-passed and cut by two narrow windows
   0.10 apart into hard-edged tonal planes plus one dark plane. Cuts closer
   than about 15 px stack into a ramp and read soft. Fine texture is the
   per-texel `Dissolve` hash summed to a continuous level, plus a 2–3 px
   clumped copy, at low amplitude. Flecks, hairlines and roughness shards are
   `Scratches` scatters modulated by noise. Nine near-neutral per-stone tint
   stops with a warm minority.
8. **Grout colour.** Per-texel sand through a golden ramp, flat cobble-grey
   chips from the finer pebble lattices, chips crowding junctions.
9. **Roughness** reads the same stroke planes, grit, shards, stains and a soft
   lift toward the edge with one thin rim line.

Exposed controls: `warping`, `edge_damage`, `sloping`, `surface_noise`,
`grout_height`, `roughness`, `dirt`. Diagnostic extras include `base`,
`damage`, `sloping`, `surface`, `grout`, `stone_mask`, `stone_id`; six ship as
512² `stage-*.png`. Bake: 7785 ops, 24 buffered planes, about 14 s on eight
threads.

Engine work this reference prompted: `GradientWarp` (scalar guide gradient,
explicit sample distance), `IntensityWarp` (fixed angle, field-controlled
distance), a 32-step slope blur, and the normal-Y convention fix in the Bevy
adapter and both generated shader passes. Similar node names do not establish
equivalence with Designer; the gradient warp's units, the bevel-as-curve, the
slope blur's normalisation and the height blend are native constructions, not
verified ports.

## State

Third pass, 2026-09-19 (`e425d37`). Close at full view and very close on the
lit sphere. Face luminance percentiles 10/50/90 are 0.048/0.101/0.175 against
the reference's 0.052/0.102/0.183; stone roughness mean 0.495 against 0.502.

What still differs, by eye:

- Stroke planes are more parallel and evenly pitched than the reference's,
  which vary more in width, stop and start, and cross-hatch on some stones.
- The reference has a faint bright shoulder just inside the arris; drawing it
  risks bringing back a ring round every stone.
- Sand grain is a shade coarse.
- On the sphere the chamfer is slightly lighter and glossier (mean roughness
  over the whole map 0.533 against 0.562).
- `the_soi_stages_evaluate_one_expression_between_them` bounds re-evaluation at
  3/2 of the instruction count, loosened from 6/5 when the graph outgrew the
  256 MiB plane budget.

## Composed moss variants

`library:soi-cobblestone-moss-light` and `library:soi-cobblestone-moss-heavy`
instance this graph and the shared `weathering:moss` compound. Grout ownership
and dirt masks guide growth, and the moss wrapper recomputes AO from the
combined height. Both preserve the 25 mm height scale and 2048² resolution;
`moss_amount = 0` reproduces the original material exactly. Their sheets,
`sheet-soi-cobblestone-moss-light` and `sheet-soi-cobblestone-moss-heavy`, sit
beside the base's in the preview.
