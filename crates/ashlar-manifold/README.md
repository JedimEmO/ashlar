# ashlar-manifold

The mesh backend for [`ashlar`](https://crates.io/crates/ashlar) building
recipes, over the native [Manifold](https://github.com/elalish/manifold) solid
kernel. It evaluates the subtraction, union, array and mirror expressions a
recipe describes into indexed triangles with metre UVs, cut-face provenance and
the convex collision proxies the elements declared.

## Layering

```
ashlar                 the recipe domain. No Bevy, no IO, no kernel.
  <- ashlar-manifold   <- you are here. Adds the native kernel. Still no Bevy.
  <- ashlar-bevy       uploads the result as Bevy meshes and PBR materials.
```

An arrow never points the other way. This is the only member with a native
dependency, so a runtime that consumes already-meshed or fitted results never
compiles C++.

## Adding it

The crate is not on crates.io yet, so we depend on it through git.
It belongs in a content step or a tool, not in a shipping game:

```toml
[dependencies]
ashlar-manifold = { git = "https://github.com/JedimEmO/ashlar" }
```

## Native build prerequisite

The kernel is native. **A first build clones and compiles upstream Manifold**,
which needs Git, `cmake` and a C++ compiler, and which `cargo fetch` does not
cache. Point `MANIFOLD_CSG_LIB_DIR` at a prebuilt install to skip both the clone
and the compile. Documentation is the exception: `manifold-csg-sys` returns from
its build script under `DOCS_RS` and rustdoc links nothing, so docs.rs builds
this crate as it stands. Nothing in `ashlar` itself has that cost.

Upstream CMake invokes `--parallel` without a job count, so on a constrained
machine limit Cargo jobs and CPU affinity for the first build.

## Example

```rust
use ashlar::{Building, MeshError, MeshedBuilding};
use ashlar_manifold::{ManifoldMesher, mesh_building};

fn evaluate(building: &Building) -> Result<MeshedBuilding, MeshError> {
    let meshed = mesh_building(building, &ManifoldMesher::default())?;

    // One entry per used part, evaluated once however many times it is placed.
    for (part, elements) in &meshed.parts {
        for element in elements {
            println!("{part}/{}: {} triangles", element.id, element.mesh.triangle_count());
        }
    }
    // One drawable piece per instance, element and cut batch, each with the
    // binding that instance's palette resolves for it.
    for piece in meshed.pieces() {
        println!("{}/{}: {}", piece.instance, piece.element, piece.binding);
    }
    // One placed convex proxy per instance of each element that declared one.
    println!("{} colliders", meshed.colliders().len());
    Ok(meshed)
}
```

## Baking for a game

`bake` meshes a building at every level of a `LodPolicy` ladder,
and answers an `ashlar::BakedBuilding` that writes to one file:

```rust
use ashlar::{Building, LodPolicy};
use ashlar_manifold::{ManifoldMesher, bake};

fn ship(building: &Building) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let baked = bake(building, &LodPolicy::ladder(), &ManifoldMesher::default())?;
    Ok(baked.write()?)
}
```

Level zero is exactly what `mesh_building` makes, colliders and portals included.
Each later level meshes the recipe as the policy simplifies it,
and a part that did not change is not meshed again.

`MeshedBuilding`, `ElementMesh`, `ElementCollider` and the `Piece` the walk
yields are `ashlar`'s types, not this crate's: none of them names a kernel type,
so they belong below the port they are the output of, and are imported from
there.

Meshes are `f64`, outward-wound and indexed: vertices are shared where position,
normal and UV agree, so creases and UV seams stay split and flat continuous
surfaces do not. Normals are derived here from the sharp angle, 45 degrees by
default, and an edge at that angle is a crease. Kernel failures carry the part,
element and expression path that caused them.

## Examples

`cargo run -p ashlar-manifold --example damage_server` is a damage server that
links no Bevy: it builds a small merged wall, replays
`examples/damage_log.ron` against one kept solid per group, prints what each hit
changed, and checks the result against the stateless mesher.

## Documentation

The mesh contract, the UV modes and the tessellation budgets are in
[docs/guide/recipes.md](https://github.com/JedimEmO/ashlar/blob/main/docs/guide/recipes.md).

## Licence

MIT or Apache-2.0, at your option.
