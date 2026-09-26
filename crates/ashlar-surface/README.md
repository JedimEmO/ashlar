# ashlar-surface

The material vocabulary `ashlar` and `ashlar-material` share. No Bevy, no graph
engine, no geometry, no IO.

A building recipe binds a slot to a material key. A material graph declares
parameters and bakes into a texture set. A `MaterialDefinition` is what joins
them: constants, a repeat size in metres, and a `Surface` that says where the
varying detail comes from.

| Type | What it is |
| --- | --- |
| `MaterialDefinition` | Base colour, roughness, metallic, emissive, `tile_metres`, a `Surface`, optional `StrandSettings` |
| `Surface` | `Plain`, `Files` on disk, `Graph` from a named graph at registration, or `Shader` compiled from one |
| `Surface::Files::baked_from` | The bake that wrote the files. Also what says the normal map is in the bake's convention: keep it on exported maps, leave it off painted ones |
| `Bake` | A graph key, parameter values and a resolution |
| `MaterialLibrary` | Definitions by content key, checked against whatever binds them |
| `Binding` | A slot's material key, and the graph parameters this slot overrides |
| `ParamValue` | `Float`, `Color`, `Int` or `Bool` |
| `StrandSettings` | Which strand layers a definition grows, and how far out |
| `ValidationError` | A path and a reason |

## Layering

```
ashlar-surface         <- you are here. serde, thiserror.
  <- ashlar            recipes bind these definitions.
  <- ashlar-material   graphs are what a Graph or Shader surface names.
```

You rarely depend on this crate directly. Both crates above it re-export every
name, so `ashlar::MaterialLibrary` and `ashlar_material::ParamValue` are the
same types as the ones here. It exists so that those two crates can share a
definition without either depending on the other; the reasoning is in the
2026-09-20 amendment to
[ADR 0003](https://github.com/JedimEmO/ashlar/blob/main/docs/adr/0003-procedural-materials.md).

Everything here names things by content key. A definition never holds a graph
and a library never holds a building: `MaterialLibrary::check_for` takes
anything that implements `BoundSlots`, which `ashlar::Building` does.
