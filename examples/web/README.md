# ashlar-web

The explorable demo: ashlar's baked scenes and its material library,
in a browser tab or a native window.
It is one Bevy app, and the project site's `demo/`.

- **Scenes.** A baked `.ashlar` building and the file-backed material library it wears,
  loaded through `AshlarPlugin` exactly as a game loads them,
  every level of detail in its distance band.
  Orbit or walk, light it by day or by night (bloom, and a lamp in every small strong emitter),
  hide the storeys above one, hide the exterior to see inside,
  and look at the level bands tinted or one level at a time.
- **Materials.** The default library on a sphere and a wall, loaded from files.
  Move a parameter and its graph re-bakes on the spot,
  on the main thread, because a browser gives a page no other.
  This is the one thing here a shipped game would not do:
  it needs `ashlar-bevy`'s `runtime-bake` tool feature and the graph engine.

Nothing it draws is committed.
`examples/content.rs` is its content step: it bakes the demo's scenes,
only the materials those scenes wear, and the gallery's maps, into an asset root under `target/`.

## Run it

```sh
just site          # content step, wasm build, manual, API docs -> target/site
just site-serve    # http://localhost:8080/demo/
```

Natively, over the files `just site` staged in `target/web/assets`:

```sh
just web --help                                  # options and scene names
just web                                         # the metropolis, by night
just web --scene interior --day --storey 0       # the house, ground floor only
just web --material brick --param wear=0.8       # the brick stage, re-baked
just web --scene city-alley --screenshot out.png # one frame, then exit
```

The browser build reads the same options from the page's query string:
`demo/?scene=city-alley&night=1`, `demo/?material=rusted-steel`.

| Option | What it does |
| --- | --- |
| `scene` | `metropolis`, `city-block`, `city-alley`, `city-landmark`, `corporate-block`, `interior`, `scifi-colony`, `scifi-outpost` |
| `night`, `day` | the lighting; each scene has its own default |
| `material` | open on the material stage with this library material |
| `param` | `name=value`, as if a slider had moved; colours are `r,g,b` linear |
| `storey` | hide every storey above this one |
| `interior` | hide the exterior |
| `lod` | `bands`, `tint`, or a level number to draw at every distance |
| `walk` | start at street level, walking |
| `assets` | native only: the asset root, default `target/web/assets` |
| `screenshot` | native only: capture a frame once everything has loaded, and exit |

## What is where

| File | What it does |
| --- | --- |
| `src/catalog.rs` | the scenes, the material descriptions and the bake resolutions, shared with the content step |
| `src/scenes.rs` | loading a scene, framing it, the storey, exterior and level views, and the gzipped building loader |
| `src/lighting.rs` | the preview's day and night rig, with a budget of lit lamps for WebGL2 |
| `src/gallery.rs` | the material stage and the live re-bake |
| `src/camera.rs` | orbit and walk, mouse and touch |
| `src/ui.rs` | the side panel (`bevy_egui`) |
| `examples/content.rs` | the web content step |
| `index.html`, `Trunk.toml`, `.cargo/config.toml` | the web build, and the size-first `web-release` profile it compiles with |
| `site/` | the landing page and `build.sh`, which `just site` and `.github/workflows/pages.yml` run |

## Sizes

A visitor downloads the app once, and then only the scene or material they pick.
Baked buildings travel gzipped as `.ashlar.gz`:
a static host serves an unknown extension uncompressed,
and a baked building deflates to between an eighth and a quarter.
Height maps are left out of the web export: no material slot reads one,
and WebGL2 cannot upload their sixteen-bit format.
Materials bake at 256 texels per repeat, or at a graph's finest lattice where that is finer,
which for most of the library is 512.

The app itself is about 30 MB of WebAssembly, 10 MB gzipped, which a static host compresses on the way.
It is built with the `web-release` profile in `.cargo/config.toml`:
optimised for size, except `ashlar-material` and `ashlar-strands`,
whose bake loop is what a visitor waits on when a slider moves.
A re-bake at 512 takes one to three seconds in a browser.

## Limits

- **Level bands switch without a crossfade in the browser.**
  Bevy 0.19.1 cannot crossfade a `VisibilityRange` on WebGL2,
  so `AshlarPlugin`'s default `Bands::Auto` gives each piece an abrupt range there,
  cut at the middle of each crossfade; see `ashlar_bevy::baked`.
  Natively the crossfade is untouched.
- WebGL2 draws one directional light, so there is no fill light in the browser; the ambient carries its share.
- At most 204 clustered lights: the night rig lights the 64 lamps nearest the camera.
- One shadow cascade in the browser, three natively.
- Strand layers (the grass blades) are not grown; their relief is in the maps.
- The four compiled `showcase:*` surfaces are shaders over the world,
  so the study scenes that wear them are not in the demo.
