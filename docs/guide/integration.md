# Using ashlar in a game

This guide takes a game from nothing to drawing its own buildings,
with levels of detail, collision proxies and rooms, from files it ships.
It is the path the two crates under `examples/integration-content` and
`examples/integration-game` walk, so every step here has code you can run.

## The shape of it

A game at play should not bake anything.
A material bake costs a fraction of a second, and meshing a building
means linking a C++ geometry kernel into the game.
Neither belongs in front of a frame, and neither belongs in a shipped binary.

So we split the work in two, the way a game already splits art from code:

- **A content step** runs on your machine or a build server.
  It links the geometry kernel and the material graph engine,
  and writes files into your game's asset directory.
- **The game** loads those files.
  It links `ashlar-bevy` with its default features, which are none,
  and pulls neither `ashlar-manifold` nor `ashlar-material`.

This may look like extra ceremony for something that could just run at startup.
But it is the same split a game makes for textures and models,
and it is what keeps a C++ toolchain out of your game's build.

The content itself is Rust in both halves.
Buildings are recipes written with `ashlar`, and materials are graphs from
`ashlar-material`, starting from its default library of forty surfaces.
Nothing baked is ever the source; the files are regenerated whenever the Rust changes.

## Dependencies

Nothing is on crates.io yet, so both crates depend on the repository through git.
The content crate is a tool:

```toml
[dependencies]
ashlar = { git = "https://github.com/JedimEmO/ashlar" }
ashlar-content = { git = "https://github.com/JedimEmO/ashlar" }
ashlar-material = { git = "https://github.com/JedimEmO/ashlar" }
```

The game is not:

```toml
[dependencies]
ashlar-bevy = { git = "https://github.com/JedimEmO/ashlar" }
bevy = "0.19"
```

`ashlar-bevy` pairs with one Bevy release at a time, and this one is Bevy 0.19.
It re-exports `ashlar`, so the game needs no second dependency to name a `Room` or a `Portal`.

The content crate needs what `ashlar-manifold` needs:
`cmake`, a C++ compiler and Git, because its first build clones and compiles
upstream [Manifold](https://github.com/elalish/manifold).
The game needs whatever Bevy needs on your platform, and nothing more.

## The content step

The content binary is ordinary Rust.
It builds your buildings, picks your materials, and hands both to `ashlar_content::Content`:

```rust
use ashlar::LodPolicy;
use ashlar_content::Content;
use ashlar_material::stdlib;

let graphs = stdlib::graphs();
let definitions = stdlib::materials();

let content = Content::new("../my-game/assets");
let shipped = content.ship(
    &graphs,
    &definitions,
    "materials",
    &[("buildings/village.ashlar".to_owned(), village()?)],
    &LodPolicy::ladder(),
)?;
```

`ship` does three things, in this order.
It resolves per-instance material overrides into materials of their own (more on that below),
bakes every material the library names into KTX2 maps,
and meshes each building at every level of detail.
`Content::write_materials` and `Content::write_building` do the two halves separately,
when a game wants to bake materials once and buildings many times.

What lands in the asset directory:

| Path | What it is |
| --- | --- |
| `materials/<namespace>/<name>/base.ktx2`, `normal.ktx2`, `orm.ktx2`, `height.ktx2` | One material's maps, each with its full mip chain, zstd-supercompressed |
| `materials/<namespace>/<name>/set.strands` | The strand set of a material that grows grass or moss |
| `materials/library.materials.ron` | The material library naming those files, one definition per key |
| `materials/graphs.ron` | The graphs they were baked from, for tools; the game never reads it |
| `buildings/<name>.ashlar` | A building at every level of detail, with its colliders, portals and rooms |

Every write goes through a temporary file and a rename,
so a game that is running while the step runs never reads half a file.

`Content::resolution` bakes every material at one size instead of each definition's own.
A lower one is the right call while iterating, and the definition's own for a release.

## The game

The game adds one plugin and spawns one entity per building:

```rust
use ashlar_bevy::prelude::{AshlarBuilding, AshlarPlugin};
use bevy::prelude::*;

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, AshlarPlugin::default()))
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands, server: Res<AssetServer>) {
    commands.spawn((
        AshlarBuilding {
            building: server.load("buildings/village.ashlar"),
            materials: server.load("materials/library.materials.ron"),
        },
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));
}
```

That is the whole integration.
When both files have loaded, the plugin spawns every piece of every level as a child
of that entity, with its `Mesh3d`, its `MeshMaterial3d<StandardMaterial>`,
its `Transform` and a `VisibilityRange` for its distance band.
The entity's own `Transform` places the building in the world.

Meshes are shared per building by their contents, and materials per library and binding.
So a hundred copies of one building upload its meshes once,
and Bevy batches every instance of a piece into one draw.
A piece a coarser level did not change is one entity whose band runs on through that level,
rather than two copies crossfading into each other.
A merged group is drawn about the centre of its bounds,
so it switches level by its own distance from the camera and not by the building origin's.

The library loader opens every map and strand set it names while it loads.
A missing map is then a load error naming the key,
rather than a black wall three rooms later.
`AshlarPlugin { preflight: false, .. }` skips that, for a game that trusts its content step.

## What a spawned building gives you

Three things arrive beside the meshes, all in the building's own frame.

**Collision proxies.** Each is a child entity with an `AshlarCollider`:
the instance and element it came from, and the vertices of a convex solid.
A physics crate builds a convex hull collider from those points.
They are tight and convex by construction, one per straight run of wall,
so a doorway is the gap between two proxies and a room is never filled by one.
They are level zero's at every distance, because physics does not coarsen with the camera.

**Rooms and portals.** The root entity gets an `AshlarSpaces`,
with every declared `Room` and every `Portal`: the opening a door or window cutter publishes,
placed once per instance of the part.
That is what AI navigation, audio occlusion and "which room is the player in" read.

**Events.** `AshlarBuildingSpawned { entity }` is sent once the children exist,
and the root gets an `AshlarSpawned` marker.
A failure puts `AshlarFailed(reason)` on the root instead, and never panics.
Removing `AshlarFailed` retries.
With Bevy's file watcher on, a rewritten file respawns the building,
and a file that finishes loading after a failure clears the failure,
so a game started before its content step ran picks the buildings up once the files arrive.
The plugin's `AshlarCache` lets go of a building's meshes when nothing holds the building any more.

## Levels of detail

A coarser level is not a decimated mesh.
The recipe is procedural, so a simpler building is a simpler recipe,
and every level is still a closed, correctly shaded and mapped solid.
`ashlar::LodPolicy` says what goes at each distance:

| Field | What it does |
| --- | --- |
| `until` | Metres from the camera where the level stops drawing; `None` on the last level |
| `min_feature` | Elements, union members and openings thinner than this are left out |
| `segment_scale` | Cylinders and revolves keep this share of their segments, never fewer than eight |
| `drop_interior` | Interior elements go: liners, floors seen from inside, furniture |

Thin is judged by the middle of a solid's three extents: how wide it looks from where it looks widest.
That may seem strange for a sill three metres long, but twenty centimetres deep is what it is from across a street.
A dropped opening is not a hole in the wall at a distance;
the facade texture carries the window from there.
Something thin that still reads from afar because it glows, a neon line or a crown of light,
is authored `Element::far()` and stays at every level.

`LodPolicy::ladder()` is the default: as authored to 60 metres,
then no interiors, nothing thinner than 35 cm and half the segments to 250 metres,
then nothing thinner than 1.5 metres and a quarter of the segments beyond.
It is a starting point, and the right numbers depend on your camera and your art.
Measured on the showcase (triangles drawn, every instance counted):

| Scene | Level 0 | Level 1 | Level 2 |
| --- | --- | --- | --- |
| Metropolis, 4 x 4 blocks | 3,928,156 | 1,161,406 | 760,902 |
| City block | 402,248 | 119,424 | 80,654 |
| Sci-fi colony | 119,182 | 48,130 | 19,292 |
| Corporate block | 41,132 | 33,696 | 14,116 |

Level zero is where the cost is, and it is only ever drawn within sixty metres of the camera.

`AshlarPlugin::crossfade` is the fraction of a boundary's distance two levels blend over,
a tenth by default.

### In the browser

A game built for the web runs on WebGL2, where Bevy 0.19.1 cannot crossfade a `VisibilityRange`.
On a device with fewer than six storage buffers per shader stage,
the visibility ranges go in a uniform buffer that the shader declares as `array<vec4<f32>, 64>`, 1024 bytes,
while the bind group layout gives it a `min_binding_size` of `Vec4::min_size()`, 16 bytes.
The shader reads that binding only to dither a crossfade,
so the first mesh with a crossfading range fails pipeline validation and the app quits.

`AshlarPlugin::bands` is `Bands::Auto` by default, and it decides from the render device when a building spawns.
Where the device cannot crossfade, every piece gets an *abrupt* range instead,
cut at the middle of each crossfade margin.
Bevy still culls each piece to its band; a visitor loses the dithered fade, not the levels.
Nothing else changes: a game that hides pieces itself, a storey cut for instance, sets their `Visibility` as it would natively.
`Bands::Crossfade` and `Bands::Abrupt` force one or the other.
[The web demo](https://github.com/JedimEmO/ashlar/tree/main/examples/web) runs on exactly this.

## Per-instance overrides

A building may bind a slot with graph parameters of its own:
a hull colour per faction, a wall colour per cottage.

```rust
Instance::new("cottage-0", "village:cottage").binding(
    "wall",
    Binding::new("library:adobe").param("color", ParamValue::Color([0.62, 0.52, 0.40])),
)
```

A file cannot take a parameter at runtime, so the content step resolves each distinct
override into a material of its own before anything bakes.
`ashlar_content::flatten_overrides` keys it by a hash of the parameters,
so the same colour from a hundred buildings is one material and one bake,
and rewrites the buildings to bind it plainly.
`ship` does this for you; a game calling `materials` and `building` separately calls it first.

An override naming a parameter the graph does not have is refused by the export,
with the graph and parameter in the message.

## Iterating

Shipping files is the wrong loop for tuning a material or a facade.
`tools/ashlar-preview` is the authoring tool: it bakes materials from their graphs at registration,
gives every parameter a slider, and reloads a graph library when you save it.
It is a library with a thin binary, so a game registers its own scenes;
[the recipes guide](recipes.md#previewing-your-own-content) shows how,
and the `ashlar-materials` and `ashlar-geometry` skills under `.agents/skills/` are the workflows.

When something looks right in the preview, copy the numbers into the Rust builder,
because that is what the content step bakes.

## Troubleshooting

Every failure lands in `AshlarFailed` on the building's root entity, with the reason.

- **"the building failed to load"** or **"the material library failed to load"**,
  followed by the asset server's reason:
  the content step has not run, wrote somewhere else than the game's asset root,
  or wrote a file this version cannot read. The reader names what it refused.
  A preflight failure names the material and the map or strand set it could not open.
- **"the material library does not dress this building"**, followed by an instance and slot:
  the building binds a key the library does not hold.
  Bake the materials and the buildings from the same definitions, which `ship` does.
- **"binds no graph parameters, so an instance cannot override"**:
  a building with per-instance overrides was baked without `flatten_overrides`.
- **"names a material graph"**:
  a library with `Surface::Graph` or `Surface::Shader` definitions reached the game.
  The game path draws files; run the definitions through the content step.

## What it costs

The showcase's content step writes 59 material sets and a file per scene.
The 8 x 8 metropolis meshes at three levels in about a quarter of a second, to a 13 MB file;
much of that is the recipe, which the manifest carries whole.
Materials are the slow half, a few seconds per set at 2048 texels,
which is why `Content::resolution` exists.
`just content` runs it, and `cargo run --example baked -p ashlar-bevy -- metropolis` draws the result.

The design behind all of this is [ADR 0006](../adr/0006-baked-buildings-and-lod.md)
for buildings and [ADR 0003](../adr/0003-procedural-materials.md) for materials.
