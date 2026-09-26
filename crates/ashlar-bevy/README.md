# ashlar-bevy

The Bevy adapter for [`ashlar`](https://crates.io/crates/ashlar) buildings.
It uploads evaluated triangles as indexed `Mesh`es with metre UVs and generated
tangents, turns a material library into `StandardMaterial`s with correct colour
spaces and repeat sampling, and preflights every texture a library names before
a window exists.

## Layering

```
ashlar                 the recipe domain. No Bevy, no IO, no kernel.
  <- ashlar-manifold   evaluates recipes into triangles, over the native Manifold kernel.
  <- ashlar-bevy       <- you are here. bevy_asset, bevy_mesh, bevy_render, bevy_pbr, bevy_image.

ashlar-strands         strand sets read off disk. Under the `strands` game feature.
  <- ashlar-bevy

ashlar-material        material graphs. Only under a tool feature; see below.
  <- ashlar-bevy
```

`ashlar-bevy` re-exports `ashlar`, so a game needs no second dependency to
name a `Room`, a `Portal` or a `Building`.

An arrow never points the other way. Positions narrow from `f64` to `f32` here
and nowhere below.

## The game path: baked files and `AshlarPlugin`

A production game ships baked content and links no kernel and no graph
engine. A content step (your own small binary over `ashlar-content`, or
`just content` in this workspace) writes:

- `buildings/<name>.ashlar`: a building at every level of detail, with its
  collision proxies, portals and rooms (`ashlar::BakedBuilding`, ADR 0006);
- `materials/library.materials.ron` and the KTX2 maps it names.

With default features, this crate loads both as assets and spawns the building:

```rust,no_run
use ashlar_bevy::prelude::{AshlarBuilding, AshlarPlugin};
use bevy::prelude::*;

fn setup(mut commands: Commands, server: Res<AssetServer>) {
    commands.spawn((
        AshlarBuilding {
            building: server.load("buildings/outpost.ashlar"),
            materials: server.load("materials/library.materials.ron"),
        },
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));
}

App::new()
    .add_plugins((DefaultPlugins, AshlarPlugin::default()))
    .add_systems(Startup, setup)
    .run();
```

What the plugin does, and what a game reads back:

| | |
| --- | --- |
| Loading | `.ashlar` is a `BakedBuildingAsset`; `.materials.ron` is a `MaterialLibraryAsset`, preflighted as it loads: every map and strand set it names is opened through the asset reader, so a missing or miscoloured file is a load error naming the key. |
| Spawning | When both have loaded, every level's pieces become children of the `AshlarBuilding` entity with `Mesh3d`, `MeshMaterial3d`, a `Transform` and an `AshlarPiece` (level, label, storey, side). The entity gets `AshlarSpawned` and a `AshlarBuildingSpawned` message is sent; a failure puts `AshlarFailed(reason)` on it instead and panics nothing. |
| Levels of detail | Each piece carries Bevy's `VisibilityRange`: level *n* draws from level *n − 1*'s `until` to its own, crossfading over `AshlarPlugin::crossfade` (a tenth) of the boundary distance. A last level with `until: None` draws to any distance. On WebGL2, where Bevy 0.19.1 cannot crossfade a range, `Bands::Auto` cuts each level at the middle of its margin instead. |
| Sharing | Meshes are cached per building asset and contents, materials per library asset and `Binding` (`AshlarCache`), so every instance of a part, at every level that draws it unchanged and in every building spawned from one file, is one mesh and one material. |
| Physics | Each collision proxy is a child with `AshlarCollider { instance, element, vertices }` in the building's frame: the points a physics crate builds a convex hull collider from. |
| Rooms and portals | `AshlarSpaces { rooms, portals }` on the building entity, in the building's frame. |
| Reload | A modified building or library asset is re-spawned in place. |

The plugin draws `Plain` and `Files` surfaces, which is what the
content step writes. A library that names a material graph (`Graph`,
`Shader`) fails the building with a message naming the feature that would bake
it. `examples/baked.rs` is the whole path with a camera: `cargo run --example
baked -p ashlar-bevy -- <name>` once the content step has written the files.

One convention to know when writing a library by hand: a `Files` normal map
is read in the bake's convention (green flipped) exactly when its definition
records `baked_from`. The content step always records it; a library over maps
from another tool leaves it `None`. See `create_material`.

## Bevy version

One Bevy release at a time. This is the **Bevy 0.19** adapter.

| `ashlar-bevy` | Bevy |
| --- | --- |
| 0.1 | 0.19 |

## Features

**Every feature here is off by default**, so a game linking this crate with no
features gets meshes, PBR materials over texture files, the preflight that
opens them and `drawables`, and no `ashlar-material` anywhere in its tree.

The features are of two kinds and the difference is the layering: a **tool**
feature pulls the graph engine and a **game** feature does not. A game consumes
baked files; a tool is what wrote them.

| feature | kind | implies | what it adds | who turns it on |
| --- | --- | --- | --- | --- |
| `strands` | **game** | — | `strands`, `cards`, `wind`: grow geometry from a baked strand set, with the rank-cut levels of detail, the card impostors past them and the wind that moves both. Pulls `ashlar-strands` and nothing else | a game with grass, moss, fur or carpet in it |
| `runtime-bake` | tool | — | `runtime_bake`: bake a `Surface::Graph` into `Image` assets when the material is created, the `BakeCache` that shares them, and `read_library_with_graphs`, the preflight that lowers every graph a library names | a preview, a content step, a test |
| `gpu-bake` | tool | `runtime-bake` | `gpu`: the same bake as compute dispatches on the render device, and `Baker::Gpu` | anything with a device and an author waiting on it |
| `shader` | tool | `runtime-bake` | `shader`: partition a `Surface::Shader`, bake its static half and draw the rest as a generated fragment; `ProceduralMaterialPlugin` | a preview whose parameter panel moves a wall between frames |
| `strand-scatter` | tool | `strands`, `runtime-bake` | `strands::scatter_sets` and `StrandKey`: scatter a set out of a graph instead of reading one off disk | the content step that *writes* a baked set, and the preview |

What a game gives up by saying nothing is what it could not have used at play
anyway. A runtime bake is hundreds of milliseconds of CPU in front of a frame; a
compiled surface is that plus a pipeline. Both are right for a preview, a
content step and a test, and wrong for a game that is being played.

`strands` grows grass from a baked set: `ashlar_strands::file`, one file per
definition holding every layer it grows, which a definition names from
`StrandSettings::baked_set` and the content step writes.
`cargo tree -e normal --features strands` has no `ashlar-material` in it, and a
test holds that.

Scattering a set is the other half and is still a tool's job, behind
`strand-scatter`. The seam between the two is `StrandSets`: `read_library` builds one
from a file, `scatter_sets` builds one from a graph, and `create_strands` takes
one and cannot tell which. What the preview draws off a graph is what a game
draws off disk.

A `Graph` or `Shader` definition still **loads** in a default build — a
library may name graphs long after a game stopped baking them — and is refused
where the material would be created, by an error naming the feature.

## Without the plugin

A game that meshes at runtime — one that cuts damage into walls in play,
say — or that wants its own spawn logic, uses the functions under the plugin.
`cargo run --example building -p ashlar-bevy` is that path with no features on:
a recipe authored in code, meshed, dressed in two exported KTX2 sets and
spawned. Run `just materials` first, because the maps are not committed.
`cargo run --example lawn -p ashlar-bevy --features strands` does the same for a
baked strand set, drawn as grass. Both take `--screenshot <path>` and capture
one frame. The spawn is this:

```rust
use ashlar::{MaterialLibrary, MeshedBuilding};
use bevy::prelude::*;

/// Whatever the mesher produced, kept out of the render layer until now.
#[derive(Resource)]
struct Shell {
    meshed: MeshedBuilding,
    library: MaterialLibrary,
}

fn spawn(
    mut commands: Commands,
    shell: Res<Shell>,
    server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) -> Result {
    let mut cx = ashlar_bevy::UploadContext {
        server: &server,
        meshes: &mut meshes,
        materials: &mut materials,
    };
    for drawable in ashlar_bevy::drawables(&shell.meshed, &shell.library, &mut cx)? {
        commands.spawn((
            Mesh3d(drawable.mesh),
            MeshMaterial3d(drawable.material),
            drawable.transform,
        ));
    }
    Ok(())
}
```

`drawables` is the join done once: each distinct mesh uploaded, each distinct
material created, materials keyed by the whole `Binding` so two instances with
different parameter values get two materials. `read_library` and `check_library` are the
startup preflight over a library in a file or in memory: every map opened,
every key checked against the building, and a missing or miscoloured map is an
error naming the key rather than a black wall.

The tool features — the runtime bake, the GPU baker, compiled `Shader`
surfaces and the strand scatter — and how the two bake backends are held to
one answer are described in
[the adapter in depth](https://github.com/JedimEmO/ashlar/blob/main/docs/guide/bevy-adapter.md).

This crate carries no solid kernel and no C++ toolchain: it consumes meshes,
whoever evaluated them.

## Testing

Each integration test declares the half of the crate it holds, so
`cargo test -p ashlar-bevy` with no features runs the game half and skips
what cannot exist: `tests/meshes.rs` has no `required-features` because mesh
upload is the game path, `tests/materials.rs` needs `runtime-bake`,
`tests/shaders.rs` needs `shader`, and `tests/conformance.rs` needs `gpu-bake`
and `shader`. `just ci` runs the workspace, where the preview turns all four on.

Everything here runs headless on `MinimalPlugins`, except `tests/conformance.rs`,
which compares the GPU bake with the CPU bake on a real device and runs under
`just conformance`.

## Documentation

- [Using ashlar in a game](https://github.com/JedimEmO/ashlar/blob/main/docs/guide/integration.md):
  the content step, the plugin, levels of detail and troubleshooting.
- [docs/guide/recipes.md](https://github.com/JedimEmO/ashlar/blob/main/docs/guide/recipes.md):
  the material contract, the UV modes and the texture rules.
- [The adapter in depth](https://github.com/JedimEmO/ashlar/blob/main/docs/guide/bevy-adapter.md).

## Licence

MIT or Apache-2.0, at your option.
