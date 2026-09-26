# ashlar-preview

The authoring tool: an orbit viewer and a headless reference gallery for
`ashlar` buildings, with material graphs baked or compiled as it goes and a
panel that moves a graph's parameters live. It links everything — the kernel,
the graph engine and every tool feature of `ashlar-bevy` — and is not
published; a game never depends on it. [The preview
chapter](../../docs/guide/preview.md) covers using it.

```sh
just preview --scene corporate-block     # a window
just references                          # the gallery, into target/references/
```

It is a library first: `src/main.rs` is the showcase's catalog handed to
`ashlar_preview::run`, and a game can register a `Catalog` of its own buildings
the same way (see the crate docs in `src/lib.rs`).

- `src/viewer.rs` — the window: camera rig, lighting (`--night`), wireframe,
  storey cut, screenshots.
- `src/references.rs` — the headless gallery and its `manifest.tsv`.
- `src/panel.rs` and `src/reload.rs` — the parameter panel, and reloading a
  graph library RON file when it changes on disk.
- `src/blast.rs` — shift-click to blast a merged scene, and `--blast` to do
  it from the command line.

## Examples and scripts

| What | Use |
| --- | --- |
| `examples/material-swatch.rs` | A fixed studio capture of one exported library material on a sphere or a plane: `cargo run --release -p ashlar-preview --example material-swatch -- library:brick out.png sphere`. |
| `scripts/reference-compare.py` | Side-by-side crops of a reference material's maps against a bake, for a fidelity pass. |
| `scripts/soi-reference.py` | Converts the SOI cobblestone author's maps and captures them under the same studio rig. |

The two scripts need Pillow (and numpy for the first);
[docs/material-references](../../docs/material-references/README.md) says
where each reference comes from and how the loop runs.
