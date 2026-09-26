# ashlar-showcase

The example content: the kits, scenes and material definitions the preview, the
reference gallery, the web demo and the tests draw. It is not published and a
game never depends on it; it is here to show what the crates build and to keep
them honest. [The tour of the kits](../../docs/guide/kits.md) walks through
what each kit is.

- `src/lib.rs` — `Scene::ALL`, every scene by name, and `building(scene)`.
  The study pieces (the facade and entrance bays, the outpost and the office)
  and the primitive fixture live here and in `src/example.rs`.
- `src/corporate.rs`, `src/interior.rs`, `src/scifi*`, `src/city/` — the
  corporate tower, the house with an interior, the sci-fi kit and the dark city.
- `src/library.rs` — the material library the scenes bind: the default
  `library:*` definitions plus the showcase's own `showcase:*` ones.
- `src/materials.rs` — the four graphs particular to these scenes (the wet and
  cut-aware concrete, the weathered brick, the emissive strip).
- `src/sheet.rs` — the `sheet-<name>` and `detail-<name>` specimen scenes, one
  of each for every baked library material.

## Examples

| Example | What it does |
| --- | --- |
| `content` | The content step: every scene baked to `assets/buildings/<scene>.ashlar` at every level of detail, and the library exported beside them. `just content`. |
| `export-materials` | The same content step over the material library alone, optionally narrowed to one material and with level-0 PNGs beside the KTX2 for review. `just materials`. |

Everything they write is ignored by git; the Rust graphs are the source.

## Tests

`tests/` holds the kits to their contracts: every scene builds, meshes closed
and binds every slot to a definition the library holds; no part puts two
materials on one plane; collision proxies cover what they should; rooms and
portals are consistent; merged storeys keep the unmerged building's coverage
and colliders; blasts apply; and the levels of detail bake.
