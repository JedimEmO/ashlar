# ashlar

Procedural architecture as ordinary Rust data. Describe a building as parts,
instances, material slots and attachment frames; validate it; hand it to a mesh
backend. No macro language, no architectural style baked in, and no renderer.

An *ashlar* is a squared, dressed block of stone, which is roughly what this
crate hands you.

## Adding it

The crate is not on crates.io yet, so we depend on it through git:

```toml
[dependencies]
ashlar = { git = "https://github.com/JedimEmO/ashlar" }
```

## Layering

```
ashlar                 <- you are here. glam, serde, ron, thiserror. No Bevy, no IO, no kernel.
  <- ashlar-manifold   evaluates recipes into triangles, over the native Manifold kernel.
  <- ashlar-bevy       uploads those as Bevy meshes and PBR materials.
```

An arrow never points the other way. A dedicated server can depend on this
crate alone and carry neither a renderer nor a C++ toolchain.

## Example

```rust
use ashlar::{Building, Collision, Element, Geometry, Instance, Part, Pose, ValidationError};

fn wall() -> Result<Building, ValidationError> {
    let bay = Part::builder("example:bay")
        .element(
            Element::new(
                "shell",
                Geometry::cuboid([4.0, 3.0, 0.3]).subtract(
                    Geometry::cuboid([1.5, 2.2, 0.5]).placed(Pose::at([1.25, 0.0, -0.1])),
                ),
                "surface",
            )
            .collision(Collision::Bounds),
        )
        .build()?;

    Building::builder("example:wall")
        .part(bay)
        .material("surface", "example:painted_metal")
        .instance(Instance::new("first", "example:bay"))
        .instance(Instance::new("second", "example:bay").placed(Pose::at([4.0, 0.0, 0.0])))
        .build()
}
```

Coordinates are building-local `f64` metres, right-handed and Y-up. World
placement is the game's business. `BuildingRecipe` is editable interchange data
that Serde reads and writes; `Building` is the validated snapshot a backend
consumes, and the only way to one is `build()`.
`cargo run -p ashlar --example recipe` assembles a larger courtyard from one
bay and prints its recipe as RON.

The output boundary is here too, not in the backend: `GeometryMesher` is the
port a kernel implements, and `MeshedBuilding` is what one hands back — shared
part meshes, declared collision proxies, and the recipe they were evaluated
from. `MeshedBuilding::pieces()` walks it into the pieces a renderer draws,
one per instance, element and cut batch, each already carrying the `Binding`
that instance's palette resolves for the slot its element declared. None of it
names a kernel type, so reading meshed results costs no C++ toolchain.

## Baked buildings and levels of detail

A game should not link a geometry kernel,
so a building is meshed once in a content step and shipped as a file.
`BakedBuilding` is that file's contents:
the validated recipe, one `BakedLevel` per level of detail,
and level zero's collision proxies and portals.
`BakedBuilding::write` and `BakedBuilding::read` are the container,
and `level(n)` answers a `MeshedBuilding`,
so everything that draws a freshly meshed building draws a baked one unchanged.

The levels come from the recipe itself rather than from decimating triangles.
`LodPolicy` says what to leave out at a distance:
elements and cutters under a size, segments round a cylinder, the interior side.
`LodPolicy::ladder()` is a three-level default we tune per game.
`ashlar_manifold::bake` is what evaluates the ladder;
[ADR 0006](https://github.com/JedimEmO/ashlar/blob/main/docs/adr/0006-baked-buildings-and-lod.md) has the design and the numbers.

The crate also owns `fit_ground`, a pure solver that levels a building on a
caller-supplied triangulated height patch and returns convex foundation and
ramp solids. It samples no world and creates no engine handles.

## Documentation

The authoring contract, including the validation rules, the UV projection and
the terrain solver, is in
[docs/guide/recipes.md](https://github.com/JedimEmO/ashlar/blob/main/docs/guide/recipes.md).
The founding decision is
[ADR 0001](https://github.com/JedimEmO/ashlar/blob/main/docs/adr/0001-recipe-domain-and-backends.md).

## Licence

MIT or Apache-2.0, at your option.
