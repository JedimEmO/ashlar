# The default material library: lineup

Decided 2026-09-20. One default library replaces the split between the
benchmark materials and the thinner `study:` and `metro:` surfaces the
geometries wear. It has to cover every slot the current kits bind, be a rounded
starting selection, and read as examples to build on.

Seventeen surfaces and a small utility set. Three exist already. Every
reference is CC0.

**Status, 2026-09-24:** built. Every row below is a `library:*` graph in
`crates/ashlar-material/src/stdlib/`, the brick, cobblestone and grass moved in
under `library:*` keys, and the `study:` and `metro:` surfaces this replaces are
gone. This page stays as the decision record; the "Graph today" and "Replaces"
columns name what existed on 2026-09-20. Every row was rebuilt against its
reference in one fidelity pass on 2026-09-24, so the whole library reads at one
level. The city and sci-fi kits added surfaces after this lineup; they are
listed under [Added since](#added-since).

## The bar

Match by eye. A reference is there to say what the material is, how it was
made and what it looks like worn; nobody is asked to reproduce its pixels, and
no comparison statistic is a target. What has to match is the feel and the
quality of the two benchmarks: stable unit identity, wear that comes from the
relief and the substrate rather than from global noise, channels that agree
with one another, and a surface that holds up at 1x on a 2 m repeat.

The loop in [the index](README.md) still applies, with the reference sphere
and flat maps beside ours for looking at, not for measuring.

## The lineup

Repeat sizes are ambientCG's where it states one. Where it does not, the
graph declares its own `tile_metres` from the unit sizes it draws.

### Have

| Material | Reference | Graph today |
| --- | --- | --- |
| Brick | [Bricks097](bricks097/README.md) | `benchmark:brick` |
| Cobblestone | [SOI Cobblestone](soi-cobblestone/README.md) | `benchmark:soi-cobblestone` |
| Grass | [Dense cut grass](grass/README.md) | `benchmark:grass` |

The moss variants and `benchmark:moss-carpet` come along unchanged.

### Core: what the current geometries need

| Material | Reference | Serves | Replaces |
| --- | --- | --- | --- |
| Formed concrete | [Concrete031](https://ambientcg.com/a/Concrete031) | study kit walls, slabs, `spine`, liners, cut faces | `study:concrete` |
| Plaster | [PaintedPlaster017](https://ambientcg.com/a/PaintedPlaster017) (scan) | house exterior; interior walls through a smoothness parameter | `study:plaster` |
| Stone cladding | [Tiles143](https://ambientcg.com/a/Tiles143), 2 m | the corporate `stone` slot | `metro:stone` over plaster |
| Paving slabs | [PavingStones136](https://ambientcg.com/a/PavingStones136) (scan) | forecourt | `metro:paving` over concrete |
| Steel | [Metal055A](https://ambientcg.com/a/Metal055A) | `metal` | `study:metal` |
| Painted metal | [PaintedMetal004](https://ambientcg.com/a/PaintedMetal004) | frames, doors, `metro:metal`, `paint` | `study:painted-metal` |
| Wood floor | [WoodFloor043](https://ambientcg.com/a/WoodFloor043), 1.3 m | house floors | nothing; floors wear `spine` |
| Clay roof tiles | [RoofingTiles014B](https://ambientcg.com/a/RoofingTiles014B), 2.9 m | house roof | nothing; roofs wear `metal` |

### Rounded: breadth, and examples of composition

| Material | Reference | Why it is here |
| --- | --- | --- |
| Rusted steel | [Metal053C](https://ambientcg.com/a/Metal053C) | steel under `weathering:rust`; the `rust` slot |
| Corrugated steel | [CorrugatedSteel007A](https://ambientcg.com/a/CorrugatedSteel007A), [B](https://ambientcg.com/a/CorrugatedSteel007B), [C](https://ambientcg.com/a/CorrugatedSteel007C) | one graph, clean to rusted on one control |
| Damaged plaster | [PaintedPlaster016](https://ambientcg.com/a/PaintedPlaster016) | plaster fallen away over the library's own brick |
| Ashlar blocks | [Bricks066](https://ambientcg.com/a/Bricks066), 2.4 m | dressed, margined stone; the crate's namesake |
| Ceramic tile | [Tiles107](https://ambientcg.com/a/Tiles107), 1 m | glazed, glossy, interior |
| Painted boards | [WoodSiding009](https://ambientcg.com/a/WoodSiding009) (scan) | doors, trim, siding |
| Asphalt | [Road012B](https://ambientcg.com/a/Road012B) | ground beside the paving; `patterns:cracks` |
| Rubble | [Ground110](https://ambientcg.com/a/Ground110) (scan), 2.1 m | the damage `rubble` slot, which nothing defines today |

### Utility

No references. Glass with a faint smudge and roughness field in place of a
flat colour, a dark recess, the emissive strip (`study:strip` carried over) and
signage ink. These stay small; they exist so that a building bound entirely
from the library has no hole in it.

### Left out

Marble and travertine, loose gravel, rubble-stone walling, wood shingles and
ambientCG's facade atlases. None fills a slot a geometry has, and none needs a
technique the list above does not already exercise.

### Added since

The kits that came after the lineup brought surfaces of their own into the
library rather than into the showcase, so a game gets them too. None has an
ambientCG reference; each was judged in its kit's scenes.

| Material | What it is | Kit |
| --- | --- | --- |
| `library:curtain-wall` | glazed curtain wall laid on a 4 m bay and 3.8 m storey grid, a fraction of panes lit | city |
| `library:window-band` | banded cladding with recessed window strips on the same grid | city |
| `library:office-window` | office glazing with blinds; lit or dark from one bake | city |
| `library:shopfront` | a lit shop behind glass; the definition's `emissive` is how bright | city |
| `library:holo-sign` | a glowing advertising panel behind a glass cover | city |
| `library:road` | a marked carriageway with ruts, cracks, a repair and a `wet` control | city |
| `library:stained-concrete` | dark formed concrete streaked with soot and water | city |
| `library:interior-panelling` | recessed interior wall panels | sci-fi interiors |
| `library:hull-plating` | composite hull plates with fasteners, grime and worn paint | sci-fi |
| `library:tread-plate` | diamond tread plate, polished on the lugs, rust in the recesses | sci-fi |
| `library:adobe` | sand-coloured adobe with patches, water runs and hairlines | sci-fi |
| `library:desert-sand` | wind-rippled sand with grit patches and pebbles | sci-fi |

## Shared parts

Seventeen graphs written separately would be seventeen copies of the same five
ideas. The library is built bottom-up instead, and every row above is either a
part, or a composition over parts. Parts are instanced through `Subgraph` and
`layer`, never copied, the way the moss variants instance the brick.

**Layouts.** A layout draws no surface. It exports unit identity, a unit mask,
the distance to the unit's edge and the joint depth, and takes the counts, bond
offset, joint width and per-unit irregularity as parameters.

| Part | Used by |
| --- | --- |
| Slab lattice: rectangular units in a bond, from `benchmark::rectangles` and the brick's single-lattice hashing | stone cladding, paving slabs, ashlar blocks, ceramic tile |
| Board lattice: long units with staggered butt joints | wood floor, painted boards |
| Lapped courses: units that overlap the course below, with a profile across each | clay roof tiles |
| Corrugation: a profile along one axis, with fixing points | corrugated steel |

**Substances.** A substance is a surface with no layout: colour, roughness and
fine relief, varied per unit when it is handed a unit identity.

| Part | Used by |
| --- | --- |
| Cast cement: paste, fines, pinholes, pour staining | formed concrete, paving slabs, rubble, and the mortar or grout of every laid material |
| Cut limestone: pitted, faintly bedded | stone cladding, ashlar blocks |
| Lime render: trowelled or thrown, hairline cracks | plaster, damaged plaster |
| Fired clay: the brick's colour families and skin | clay roof tiles, brick rubble; glazed over for ceramic tile |
| Timber: `patterns:wood`, with knots and per-board tone | wood floor, painted boards |
| Steel: mill scale or brushed, per-sheet tone | steel, and the substrate of every metal below |
| Bitumen and aggregate | asphalt |

**Coats and wear.** These exist already in `ashlar_material::stdlib` and are
used as they are: `edge_wear`, `dirt_dust`, `rust`, `peeling_paint`,
`moisture`, `moss`, `cracks`. One coat is missing and is added once: a paint
film that takes a substrate and a loss mask, which the painted metal, the
painted boards and the corrugated steel all need.

What that makes of the rounded tier:

| Material | Composition |
| --- | --- |
| Painted metal | steel + paint film + `edge_wear` |
| Rusted steel | steel + `rust` |
| Corrugated steel | corrugation + painted metal + `rust`, one `age` control from 007A to 007C |
| Painted boards | board lattice + timber + paint film + `peeling_paint` |
| Damaged plaster | brick + lime render, with a loss mask choosing between them and the render's thickness in the height |
| Paving slabs | slab lattice + cast cement + `moss` in the joints |
| Ashlar blocks | slab lattice + cut limestone + a drafted margin read from the edge distance |
| Ceramic tile | slab lattice + fired clay under a glaze + cast cement grout |
| Asphalt | bitumen and aggregate + `cracks` + patches |
| Rubble | scattered fragments coloured from cast cement and fired clay |

Written from scratch, then, are the four layouts, the seven substances, one
coat, and the formed concrete's panel seams and tie holes. Everything else is
wiring.

Two things to watch while doing it. A composition's cost is the lowered op
and plane report, not a guess: read it for each one. And a shared part edited
for one consumer changes all of them, so a change to a part is reviewed across
every material that instances it.

## Order

Parts before the things made of them, and a geometry-facing material as early
as each part allows:

1. Cast cement, then formed concrete.
2. Slab lattice, then paving slabs.
3. Cut limestone, then stone cladding and ashlar blocks.
4. Lime render, then plaster, then damaged plaster over the brick.
5. Steel, the paint film, then painted metal, rusted steel, corrugated steel.
6. Timber and the board lattice, then wood floor and painted boards.
7. Lapped courses and fired clay, then roof tiles and ceramic tile.
8. Asphalt, rubble, utility.
9. Rebind the study kit, the corporate tower and the house onto the library,
   and retire `study:` and `metro:` surfaces that nothing reads any more.

## Where it lives

Graphs go into `ashlar_material::stdlib` beside `weathering:*` and
`patterns:*`: parts under `layouts:*` and `substances:*`, finished materials
under `library:*`. That needs no new crate and inherits the tiling, lowering
and WGSL tests in `crates/ashlar-material/tests/stdlib.rs`. The benchmark
brick, cobblestone and grass move in under `library:*` keys at the end, since
the damaged plaster needs the brick there anyway.

Definitions ship beside the graphs. `ashlar-surface` sits below both `ashlar`
and `ashlar-material` since the 2026-09-20 amendment to
[ADR 0003](../adr/0003-procedural-materials.md), so the material crate can name
a `MaterialDefinition`, and the library hands out both halves: the graph
library, and a `MaterialLibrary` with one definition per finished material
carrying its `tile_metres`, a `Graph` surface at a default resolution and its
default parameters. A recipe binds `library:formed-concrete` and gets a wall;
a game that wants files instead bakes the same `Bake` in a content step and
swaps the surface. The showcase keeps only what is particular to its own
scenes.

## Fetching a reference

Nothing from ambientCG is committed. Each reference's page, linked above,
carries its preview sphere and a tiling view of its maps. The maps themselves
come down as the brick's do:

```sh
mkdir -p target/material-research/ambientcg && cd target/material-research/ambientcg
curl -L -o Concrete031_2K-PNG.zip "https://ambientcg.com/get?file=Concrete031_2K-PNG.zip"
unzip -o Concrete031_2K-PNG.zip
```
