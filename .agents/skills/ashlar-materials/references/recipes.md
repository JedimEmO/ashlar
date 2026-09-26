# Which library graph to copy from

Every finished material in the default library is a Rust builder under
`crates/ashlar-material/src/stdlib/`, and those builders are the templates.
They are also the source: nothing baked is committed, so the builder you copy
from is the whole of the material it describes. Node ids are map keys, so
builder order does not matter; a node may read one added further down.

## By kind of surface

| You are making | Start from | Why that one |
| --- | --- | --- |
| Units in a bond: tiles, slabs, blocks | `paving_slabs`, `stone_cladding`, `ashlar_blocks` in `surfaces.rs` over `slab_lattice` in `layouts.rs` | The layout exports `unit_id`, `mask`, `edge_distance`, `joint_depth`, `local_u` and `local_v`; everything per unit reads those rather than a second lattice. |
| A laid unit with a drawn profile | `clay_roof_tiles` over `lapped_courses` (`ceramics.rs`), `corrugated_steel` over `corrugation` (`metals.rs`) | A profile across the unit is height first, and colour and wear read it. |
| Boards | `wood_floor` and `painted_boards` over `board_lattice` and `timber` (`timber.rs`) | Per-board grain from the board's own id, and a lap step that writes its own occlusion. |
| A cast or trowelled mass | `formed_concrete` over `cast_cement` (`surfaces.rs`), `plaster` over `lime_render` (`render.rs`) | Cloud and grain at several scales, and a relief that stays shallow on purpose. |
| A coat over a substrate, and its loss | `painted_metal` and `paint_film` (`metals.rs`), `damaged_plaster` (`render.rs`) | One loss mask drives colour, roughness, metalness and thickness; full loss recovers the substrate exactly. |
| Weathering on something that exists | `rusted_steel` (`metals.rs`), `brick_moss` and `cobblestone_moss` (`reference_moss.rs`) | The substrate is instanced whole and the compound reads its masks; see `weathering.md`. |
| Scattered pieces | `rubble` and `asphalt` (`ground.rs`) | Several sizes of cell, each with its own facets, buried by a deposit field. |
| A scan reproduced | `brick` (`reference_brick.rs`), `cobblestone` (`reference_cobblestone.rs`) | One lattice for identity and geometry, wear from the relief. Their docs are in `docs/material-references/`. |
| Geometry grown from the surface | `grass` (`reference_grass.rs`), `moss_carpet` (`reference_carpet.rs`) | Strand layers and the `StrandRelief` that keeps texture and blades one drawing. |
| Something that reads the world | `examples/showcase/src/materials.rs` | The four compiled `showcase:*` graphs instance a library wall and add one runtime input. |

## Idioms worth copying

- **Compose before you write.** A finished surface is a layout, a substance and
  a compound wired through `Subgraph` and `layer`. A new substance is worth
  writing; a second copy of a brick lattice is not.
- **One lattice, several outputs.** Identity, geometry and wear all come off
  the same cells, so a unit agrees with itself in every channel.
- **Levels as a multiplier.** `Levels::new(noise).out_range(0.7, 1.18)` turns a
  `0..=1` field into a shade factor for a `Multiply` blend.
- **Mix with a literal colour** as `b` and a mask as `t` is the layering idiom.
- **Wear from the relief:** `EdgeDetect` on the mask, slope `Blur` along the
  height, `Curvature` on the finished form, `HeightToMask` for what pools.
- **Bed the height and divide the factor back out of `normal_strength`**, the
  standard fix for a height range that leaves `0..=1`.
- **Write colours linear** into `Colorize` stops and constants.
- **Every period is a power of two**, so the material's period is its finest
  lattice and no lcm warning fires, and that lattice stays at or under 512
  because every library definition bakes at 512.
- **Write a live parameter's reach as planes it only scales.** A live value
  inside mixes drags every input of those mixes into the fragment;
  `ceramic_tile` writes colour and roughness as baked planes that `glaze` and
  `grime` scale and sum, which keeps it inside eight bound images.

## The RON form

A graph library serialises to RON with `ron::ser::to_string_pretty`, and that
is what the preview's `--graphs-out` writes and `--graphs` watches. It is a
working copy for tuning, never a source: copy the numbers back into the builder.

Note on the shape: a node is `Kind((fields))` with every field written out,
including defaults, because the structs are `#[serde(default,
deny_unknown_fields)]`; a unit node is `Uv(())`. Inputs are `Node("id")`,
`Param("name")` or `Const(Float(x))` / `Const(Color((r, g, b)))` /
`Const(Vec2((u, v)))`; inline nodes never appear, because `build` hoists them
before anything is serialised. A misspelled field is a parse error naming it,
and a library that no longer parses or validates is reported by the preview,
which keeps the last good library rather than emptying the scene.
