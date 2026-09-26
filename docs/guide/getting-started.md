# Getting started

This chapter takes you from a fresh clone to a building of your own, meshed and on screen.
We will look at the showcase first, because it shows what the pieces add up to,
then write a small building, give it a material, and look at it.

## What you need

Rust 1.96 or newer and [`just`](https://github.com/casey/just).
Anything that meshes, which is the content step and this workspace, also needs `cmake`, a C++ compiler and Git,
because the first build of `ashlar-manifold` clones and compiles the Manifold kernel.
On Linux, Bevy needs its usual system packages:
`libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev pkg-config`.

The first build takes a while for exactly that reason.
Luckily, it happens once; after that Manifold is a cached static library like any other.
`cargo fetch` does not cache Manifold's sources, so an offline build points `MANIFOLD_CSG_LIB_DIR` at a prebuilt install.

Compiler selection follows your platform and environment.
If an activated Anaconda environment supplies `CC` and `CXX`, use the system compiler:

```sh
export CC=/usr/bin/gcc CXX=/usr/bin/g++   # Linux; use your platform's compiler paths
cargo clean -p manifold-csg-sys          # only if the kernel was already built with Anaconda
```

Anaconda's GCC can build a kernel whose static initialisers never run under `rust-lld`,
causing `SIGFPE`. [Native build prerequisite](recipes.md#native-build-prerequisite)
has the diagnosis and an alternative linker workaround.

Inside this workspace, `.cargo/config.toml` enables `+sse4.1,+fma` on x86-64,
which takes about 14% off a bake and changes no baked byte.
This needs roughly a 2013 Intel or 2015 AMD CPU;
`RUSTFLAGS="-C target-feature=-sse4.1,-fma"` builds for an older one.

## Look at the showcase

The preview is the authoring tool, and the quickest way to see what ashlar does:

```sh
just preview --scene corporate-block
just preview --scene metropolis --night --focus 30,2,10 --zoom 0.03 --pitch 0.1
just preview --scene city-tower --hide-exterior --max-storey 3
just preview --scene sheet-brick
```

The first is the corporate kit, the second a street in the dark city by night,
the third the inside of a tower with everything above its third storey cut away,
and the last the brick material on a sheet of specimens.
Drag to orbit, scroll to zoom, and press `M` for the material panel.
[The preview](preview.md) chapter has every key and flag.

Every one of those buildings is a Rust function in `examples/showcase`,
and every surface is a material graph in `ashlar-material`'s default library.
Nothing in them is a model file.

## A building of our own

Let's make the smallest building worth the name: a wall of two bays, each with a door-sized opening.
A new binary crate needs `ashlar` for the recipe and `ashlar-manifold` to mesh it:

```toml
[dependencies]
ashlar = { git = "https://github.com/JedimEmO/ashlar" }
ashlar-manifold = { git = "https://github.com/JedimEmO/ashlar" }
```

```rust
use ashlar::{Building, Collision, Element, Geometry, Instance, Part, Pose};
use ashlar_manifold::{ManifoldMesher, mesh_building};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bay = Part::builder("example:bay")
        .element(
            Element::new(
                "shell",
                Geometry::cuboid([4.0, 3.0, 0.3]).subtract(
                    Geometry::cuboid([1.2, 2.2, 0.5]).placed(Pose::at([1.4, 0.0, -0.1])),
                ),
                "wall",
            )
            .collision(Collision::Bounds),
        )
        .build()?;

    let building = Building::builder("example:wall")
        .part(bay)
        .material("wall", "library:brick")
        .instance(Instance::new("first", "example:bay"))
        .instance(Instance::new("second", "example:bay").placed(Pose::at([4.0, 0.0, 0.0])))
        .build()?;

    let meshed = mesh_building(&building, &ManifoldMesher::default())?;
    let triangles: usize = meshed.pieces().map(|piece| piece.mesh.triangle_count()).sum();
    println!("{triangles} triangles, {} collision proxies", meshed.colliders().len());
    Ok(())
}
```

Notice how little of this is geometry.
The bay is one element: a box with a smaller box subtracted from it,
and the cutter runs past both faces of the wall, so it makes an opening rather than a recess.
The element names a *slot*, `wall`, and the building binds that slot to a material key.
The part never learns which material it wears, which is what lets one part dress as brick in one building and concrete in the next.

The instances place the part twice.
A cuboid's origin is its minimum corner, so the second bay starts where the first one ends.
`build()` validates the whole recipe and, when something is wrong, returns an error with a path such as
`instances[second].part: unknown part "example:bya"`, which is the first thing to read when a building will not build.

## Giving it a material

`library:brick` is one of the forty surfaces in `ashlar_material::stdlib`.
`stdlib::materials()` answers a `MaterialLibrary` with a definition for each,
and `stdlib::graphs()` answers the graphs they are baked from.
A definition names its graph through `Surface::Graph`, which a tool bakes into images when it registers the material.

A game never bakes.
The content step writes each graph's KTX2 maps to disk once,
and the library the game loads names those files through `Surface::Files` instead.
[Materials](materials.md) is the guide to writing graphs of your own,
and [Shipping to a game](integration.md) is the guide to that content step.

## Seeing it

There are two ways to put our wall on screen.

The quick one is the preview: write the recipe as RON and open it.
`BuildingRecipe` is plain serde data, so `ron::to_string(building.recipe())` is all it takes,
and `just preview --recipe wall.ron` draws it.
Without a material library it draws every slot in its own diagnostic colour,
which is exactly what we want while the shape is still moving;
`--materials` names a library when it is time to dress it.

The real one is the game path.
`examples/integration-content` and `examples/integration-game` in this repository are a content crate and a game crate,
the split a game copies:

```sh
cargo run --release -p integration-content   # bake the village's buildings and materials to files
cargo run --release -p integration-game      # load them with AshlarPlugin and look
```

The content crate is where our wall would go.
The game crate is one short `main.rs` around `AshlarPlugin` and an `AshlarBuilding`,
and it compiles without the kernel, the graph engine or a C++ toolchain.

## Where next

From here the two authoring guides take over.
[Building recipes](recipes.md) covers parts, sockets, collision, interiors, merged buildings and terrain fitting,
and [Materials](materials.md) covers graphs, the default library, weathering and strands.
The showcase's kits, walked through in [A tour of the kits](kits.md), are the largest worked examples of both.
