# Visual material work and fidelity passes

Use this for a reference reproduction, fidelity pass, or composed variant.
Paths in this reference are relative to the repository root.

## Establish what the image is showing

Inspect the reference and current output before editing. Record the repeat's
physical size, approximate unit dimensions/course count, joint widths, relief,
colour family, roughness, and the distribution of wear or growth. A photograph
includes illumination and perspective; a vendor sphere also includes its own
lighting, tone mapping and displacement settings. Compare supplied flat PBR
maps where available. A cropped photograph need not tile.

Choose a few observable differences to fix. Work from layout and major shape
to material regions, then fine texture. Increasing noise frequency cannot fix
incorrect brick proportions, a uniform bevel, or implausibly deep cavities.
Retain the seed and preview rig while comparing revisions. If the user accepts
composition and asks for texture fidelity, keep placement masks stable.

## Build causes and keep the channels related

- Establish stable unit identity and ownership masks first: stone versus grout,
  clay versus lime, intact surface versus damage. Export useful masks through
  `PbrOutput::extra` so later variants can read the same decisions.
- Separate broad variation, unit variation and fine surface detail. Give each
  a physical interpretation and amplitude. Use coherent variation for deposited
  patches and local breakup for their edges; avoid uniform speckle everywhere.
- Derive wear from exposed edges, relief and substrate masks. Derive deposits
  from shelter or joints plus irregular colony fields. Pure global noise tends
  to ignore the material beneath it.
- Use a common coverage mask for a coating's colour, roughness, metalness and
  height. Related channels need not be identical noise copies: preserve their
  distinct physical roles. Keep broad lighting/shadows out of base colour;
  inspect base colour separately from AO and the lit result.
- For cavities, tune occurrence separately from depth and interior detail.
  A rare missing brick is a different decision from frequent porous damage.
  Hollow interiors generally need less relief than broken rims; excessive
  internal noise made the Bricks097 holes read as lumps.
- For moss, keep habitat separate from cushions, shoots and tip variation.
  Smooth low-frequency green noise read as sponge; unwarped cellular bumps
  read as pebbles. The current `stdlib/moss.rs` combines several scales and
  couples their colour and relief. Use it as an example, not a universal palette.

Normal detail changes shading; it cannot create protruding stems or silhouettes.
Choose geometry when those matter. See the [Blender displacement manual](https://docs.blender.org/manual/en/latest/render/materials/components/displacement.html)
for the distinction between bump shading and actual displacement. The
[Adobe material channel reference](https://experienceleague.adobe.com/en/docs/substance-3d-designer/using/workspace/3d-view/material-properties)
describes the distinct base-colour, roughness, normal and AO roles; Ashlar's
own node and bake code defines the exact conventions here.

## Scale and resolution are separate controls

For a square repeat of width `T` metres, map resolution `R`, and full height
scale `H` metres:

- Texel size is `T / R` metres. A 2 m repeat at 2048² is about 0.98 mm/texel.
- Set `normal_strength = H / T`; feed height-based AO the same UV-scaled height.
- An added thickness `d` metres needs normalized height `d / H`.
- Increasing `R` preserves physical relief. Do not double normal strength when
  doubling resolution: the bake's derivative already accounts for texel spacing.

Passing the finest-lattice validator is a minimum, not proof of visible detail.
Inspect the smallest features at the final resolution and in mips. Tiny sharp
features may alias, vanish, or become excessive normal contrast. Keep useful
height headroom for added layers; clamping at one can flatten moss on peaks.
Do not force every material to fill the height range or have the same contrast.

## A reviewable iteration

1. Export the material into a scratch directory with PNGs, using a fixed seed.
2. Inspect the full base and height maps, a native-resolution crop, and the
   placement masks. Compare the same region between revisions.
3. Inspect a lit plane or wall for shape and tiling, and the fixed studio sphere
   for roughness and light response. Check normal orientation and displacement
   scale before changing the graph to compensate for a rendering mismatch.
4. Change the fields responsible for the observed mismatch. Keep the previous
   export and render in their own directory for direct comparison.
5. Once accepted, the builder is already the shipped material: nothing baked is
   committed. Run the relevant checks and look at the `sheet-<name>` scene. Do
   renders and tests in sequence after an export finishes: a reader of a PNG
   while the writer replaces it can fail with `UnexpectedEof`.

A successful compile or a passing pixel hash is not visual acceptance. Inspect
what was generated and report any fidelity limitation that remains.

## Commands and files

The reference graphs are `crates/ashlar-material/src/stdlib/reference_*.rs`,
every other library surface is in the `stdlib/*.rs` file of its family, and the
behaviour tests are `crates/ashlar-material/tests/{references,surfaces,stdlib}.rs`.

```sh
# Fast isolated iteration: maps, level-0 PNGs and the bake report, into scratch.
cargo run --release -p ashlar-showcase --example export-materials -- target/pass 2048 brick png

# Reference beside ours: full view, 1x crops, 2x and 3x zooms.
python3 tools/ashlar-preview/scripts/reference-compare.py brick \
  target/pass/materials/library/brick target/pass/compare

# The same maps lit on the studio sphere.
cargo run --release -p ashlar-preview --example material-swatch -- \
  library:brick target/pass/brick-sphere.png sphere target/pass

# The behaviour tests.
cargo test --release -p ashlar-material --test references --test surfaces -- --test-threads=1

# Interactive: three distances baked beside compiled, and a close-up.
just preview --scene sheet-soi-cobblestone-moss-light
just preview --scene detail-brick-moss-heavy
```

The studio rig lays one repeat over its sphere as two metres whatever the
material's `tile_metres`, so a 1 m repeat reads at twice its size there. Judge
scale with that in mind, and compare at the same crop scale as the reference:
a sphere can hide what a 1x crop shows.

For a new library surface, add its builder to `stdlib::graphs()`; `stdlib::materials()`
derives its definition, and the preview gains its sheet and close-up from that.
Update the matching material-reference notes. Bricks097 reference comparisons
use a 1024² reference crop and downsample the 2048² bake.

Review the diff before finishing. Rendering can change unrelated captures even
when their maps did not change. Preserve existing user edits and restore only
files whose prior contents you recorded and know are unrelated; keep capture
hashes consistent with the images that remain.

## Checks with useful meaning

For a new coating, check amount zero against the base, unchanged unit identity,
finite channel ranges, and expected coverage behavior. Test habitat preference
where the design calls for it. Exact AO recovery requires either preserving AO
or using the same derivation as the base; do not assume it from colour passthrough.
A shared-compound edit affects all its consumers, so inspect both brick and cobble.

Read op counts, plane counts and bake timings. A warp of an expensive field
can duplicate work, and many buffered planes can dominate memory. If planning
stalls before rasterization, isolate build versus lowering versus bake rather
than reducing visual quality blindly. The cobble+moss work exposed a lowering
bug that copied unrelated expressions on every warp; it is now regression-tested
in `crates/ashlar-material/tests/lowering.rs`. Prefer measuring current behavior
to assuming every subgraph output adds another full runtime evaluation.
