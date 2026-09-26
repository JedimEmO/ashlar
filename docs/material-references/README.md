# Reference materials

Three real materials, each with real maps to compare against, rebuilt as native
Ashlar graphs. They are how the material engine is tested for fidelity: a graph
counts as close when its maps, laid beside the reference's, read as the same
material made the same way.

| Reference | What it is | Ashlar graph | Doc |
| --- | --- | --- | --- |
| SOI Cobblestone | A Substance Designer graph (CC0) with its source `.sbs` and the author's exported 2048² maps | `library:soi-cobblestone`, [reference_cobblestone.rs](../../crates/ashlar-material/src/stdlib/reference_cobblestone.rs) | [soi-cobblestone/](soi-cobblestone/README.md) |
| Dense cut grass | User-supplied BlenderKit studio preview | `library:grass`, [reference_grass.rs](../../crates/ashlar-material/src/stdlib/reference_grass.rs) | [grass/](grass/README.md) |
| Bricks097 | An ambientCG photogrammetry scan (CC0) of an old worn brick wall, 2K maps | `library:brick`, [reference_brick.rs](../../crates/ashlar-material/src/stdlib/reference_brick.rs) | [bricks097/](bricks097/README.md) |

These three set the bar for the whole default library. Every other textured
surface of the original lineup was reviewed against an ambientCG scan to the
same standard; the lineup, the reference per surface, the parts they share and
the kit surfaces added since are in [lineup.md](lineup.md).

Each reference's doc holds its provenance, how to fetch its maps, how the graph
is constructed and what still differs.

Nothing here is committed but the graphs. The reference maps live under ignored
`target/` paths and each doc says how to put them back, and our own maps,
spheres and comparison sheets are regenerated when a pass needs them rather than
kept in git. Neither reference's pixels are an input to its graph.

## Running a fidelity pass

The loop is bake, compare, look, edit. Both bakes are fast enough (brick about
1 s, cobblestone about 14 s at 2048) that looking is the slow step, which is as
it should be.

```sh
# Export one material, with level-0 PNGs, somewhere of its own.
cargo run --release -j 8 -p ashlar-showcase --example export-materials -- \
  target/pass 2048 soi-cobblestone png

# Reference on the left, ours on the right: full view, 1x crops, 2x and 3x zooms
# of base colour, roughness and height.
python3 tools/ashlar-preview/scripts/reference-compare.py soi-cobblestone \
  target/pass/materials/library/soi-cobblestone target/pass/compare

# The same maps lit, on the studio sphere (needs the GPU).
cargo run --release -p ashlar-preview --example material-swatch -- \
  library:soi-cobblestone target/pass/sphere.png sphere target/pass

# In the preview: three distances, baked beside compiled, and a close-up.
just preview --scene sheet-soi-cobblestone
just preview --scene detail-soi-cobblestone
```

For the brick, pass `brick` in place of `soi-cobblestone` throughout.

To finish a pass: `cargo fmt`, clippy with `-D warnings`, and the tests that
hold the reference graphs to their behaviour,
`cargo test -p ashlar-material --test references --test stdlib -- --test-threads=1`.
There is nothing to regenerate and commit afterwards: the graph is the material.

## What has worked, and what has not

These come from the passes that got the two materials to where they are.

- **Look before measuring.** Build the side-by-side crops at native resolution
  and look at them every round. Statistics are guard rails only. A posterised
  hatch that reads as fur, a uniform per-texel dither that reads as a print
  screen, bright grit gathered in clouds that reads as frost, and saturated
  stains that read as rust all matched their moments.
- **Describe what the crop looks like.** "The rim is a flat band with an inner
  edge, the reference's is a gradient" produces a fix. "Within-stone luminance
  deviation is 0.034 against 0.058" produces a parameter tweak.
- **Distrust a measurement taken through a mask nobody looked at.** One round
  raised the cobblestone's grout bed to match a joint-height median measured
  through a mask that included the stone toes, and drowned the low stones.
- **When a claim and the crop disagree, ask for a diagnostic.** Stroke edges
  reported as two pixels wide looked soft because three cuts landed inside
  thirteen pixels of each other and stacked into a ramp. Routing the mask to an
  output found that in one bake.
- **One structural area per change, one after another, on the one file**, with
  a snapshot of the source after each round.
- **Height races bury things.** Wherever two surfaces compete for ownership by
  height, bound the loser structurally (`min(mortar, face - margin)`) rather
  than hoping the relief draws never line up.
- **Any wall-scale grime field becomes smoke.** Dirt has to be decided per
  stone or per brick, or gated by something that is.

## Engine constraints worth knowing

- The library's definitions bake at 512 when they are registered, and a bake
  is refused when a generator's finest lattice (period × 2^(octaves − 1))
  exceeds the resolution. So every generator stays at or under 512, which is
  4 px at 2048. Finer detail comes from the per-texel `Dissolve` hash (exempt),
  thresholds, products, warps and directional blurs.
- `m(Step, a, b)` is 1 when `a >= b`, not GLSL's `step(edge, x)`.
- `Voronoi::Distance` belongs to the nearest site only; anything drawn wider
  than its cell is cut along the cell polygon.
- Blur a signed distance before reading it through a profile curve, not the
  profile afterwards; an exact distance over a texel-quantised boundary combs a
  steep flank.
- Ashlar normals are `normalize(-dH/du, -dH/dv, 1)` with rows increasing in V;
  the Bevy adapter flips Y for baked maps. Do not also flip the texture bytes.
- On the authoring machine, run cargo with a bounded `-j` and test suites
  serially.
