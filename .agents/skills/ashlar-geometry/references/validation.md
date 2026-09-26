# Validation and mesher failure modes

Every domain error is a `ValidationError { path, reason }`, printed as
`path: reason`. Sources: `crates/ashlar/src/{geometry,recipe,material,ground}.rs`
and `crates/ashlar-manifold/src/lib.rs`; the cases are exercised in
`crates/ashlar/tests/*.rs` and `crates/ashlar-manifold/tests/geometry.rs`.

## Order of checks in `BuildingRecipe::build`

1. `version`, `id`, `grid`.
2. `instances` non-empty.
3. Palette bindings under `materials`.
4. Each part under `parts[<part>]`: unique id, then the part's own checks.
5. Each instance under `instances[<instance>]`: unique id, pose, its own
   bindings, part reference, override slots, missing slots.
6. Attachments, resolved targets first.
7. Every resolved pose, checked again.

`PartBuilder::build` runs step 4 alone for one part, so a geometry mistake
surfaces before the assembly exists. Checks stop at the first failure.

## Recipe-level paths

| Path | Reason | Cause and fix |
| --- | --- | --- |
| `version` | `unknown recipe schema version, expected 1` | A document written by a newer crate. Do not edit the number; use the crate that wrote it. A missing field reads as 1. |
| `id` | `name must not be blank` | Blank or whitespace building id. |
| `grid` | `grid must be finite and positive` | `grid(0.0)`, negative or infinite. The grid is a hint only; drop it if unsure. |
| `instances` | `building requires at least one instance` | Parts registered but nothing placed. |
| `materials` | `name must not be blank` | Blank slot name in the palette. |
| `materials[<slot>]` | `name must not be blank` | Blank material key bound to a slot. |
| `materials[<slot>].params[<param>]` | `name must not be blank` / `parameter value must be finite` | A palette `Binding` with a blank parameter name or a NaN or infinite value. |

## Part paths

| Path | Reason | Cause and fix |
| --- | --- | --- |
| `parts[<part>]` | `duplicate identity` | Two parts with one id, including a part registered twice. Register once and reference by id. |
| `parts[<part>].id` from `PartBuilder::build`, `parts[<part>]` from the assembly | `name must not be blank` | Blank part id. |
| `parts[<part>]` | `part requires at least one element` | A builder with sockets only. |
| `parts[<part>].elements[<element>]` | `duplicate identity` / `name must not be blank` | Two elements sharing an id, typically a loop that forgot to format the index into the name. |
| `parts[<part>].elements[<element>].material_slot` | `name must not be blank` | Empty slot string. |
| `parts[<part>].elements[<element>].cut_material_slot` | `name must not be blank` | `cut_material("")`. |
| `parts[<part>].sockets[<socket>]` | `duplicate identity` / `name must not be blank` | Two sockets with one name. |
| `parts[<part>].sockets[<socket>]` | `translation must be finite` / `rotation must be a finite unit quaternion` | A socket pose built from a non-normalized quaternion. Use `DQuat::from_rotation_y` and friends. |

## Geometry paths

Geometry paths start at `parts[<part>].elements[<element>].geometry` and
descend through `.solid`, `.cutters[<i>]`, `.solids[<i>]`, `.step`,
`.plane` and `.pose`. `Geometry::check` uses the root `geometry` alone.

| Path suffix | Reason | Cause and fix |
| --- | --- | --- |
| any node | `geometry nesting exceeds 64 levels` | More than 64 nested nodes, usually a long chain of `subtract` calls. Build one `Shape::Difference` with a `cutters` vector, or split the solid into elements. |
| `.pose` | `translation must be finite` / `rotation must be a finite unit quaternion` | A node placed with NaN or a scaled quaternion. |
| cuboid | `box dimensions must be finite and positive` | A zero or negative extent; zero is an authoring mistake, not a thin solid. |
| chamfered cuboid | `chamfer must be finite, positive and smaller than half every box extent` | `bevel <= 0` or `bevel >= min(size) / 2`. |
| cylinder | `cylinder dimensions must be finite and positive` | Non-positive radius or height. |
| cylinder | `cylinder requires at least three segments` | `segments < 3`. |
| extrusion | `extrusion depth must be finite and positive` | Non-positive depth. |
| extrusion | `profile requires at least three vertices` | Two points or fewer. |
| extrusion | `profile coordinates must be finite` | NaN or infinity in the profile. |
| extrusion | `profile contains a zero-length edge` | Two consecutive identical points, including a closing point equal to the first; leave the profile unclosed. |
| extrusion | `profile has collinear neighbours or overflowing coordinates` | Three consecutive points on a line, or coordinates whose cross product overflows. Remove the middle point. |
| extrusion | `profile self-intersects` | Bow-tie or crossing edges. Holes are not supported; subtract a second extrusion instead. |
| extrusion | `profile must enclose finite nonzero area` | Degenerate ring. |
| difference | `difference requires a cutter` | A raw `Shape::Difference` with an empty `cutters` vector. |
| union | `union requires two solids` | `union_all` over fewer than two solids. |
| array | `array requires at least one copy` / `array exceeds the copy limit` | `count == 0` or above `MAX_ARRAY_COUNT` (1024). Use an instance loop for larger repetition. |
| `.step` | pose reasons above | Non-unit rotation in the array step. |
| `.plane` | `mirror plane offset must be finite` | NaN offset. |

## Instance paths

| Path | Reason | Cause and fix |
| --- | --- | --- |
| `instances[<instance>]` | `duplicate identity` / `name must not be blank` | Two instances with one id; a loop that formats the same name twice. |
| `instances[<instance>].pose` | `translation must be finite` / `rotation must be a finite unit quaternion` | Authored or derived pose invalid. Checked before and after attachment resolution. |
| `instances[<instance>].materials` | `name must not be blank` | Blank slot key in an override map. |
| `instances[<instance>].materials[<slot>]` | `name must not be blank` | Blank material key in an override. |
| `instances[<instance>].materials[<slot>].params[<param>]` | `name must not be blank` / `parameter value must be finite` | Bad parameter override; the blank name is still placed in the path. |
| `instances[<instance>].part` | `unknown part "<id>"` | The instance names a part that was never registered, or a typo in the id. |
| `instances[<instance>].materials[<slot>]` | `override names a slot absent from this part` | An override for a slot no element of that part uses, including a misspelling such as `surafce`. Ask the built building with `material(id, slot).is_some()` before overriding, as `corporate::block` does. |
| `instances[<instance>].materials[<slot>]` | `material slot has no binding` | A slot an element uses, cut slots included, that neither the palette nor the instance binds. Add `.material(slot, key)` on the builder. |

## Attachment paths

| Path | Reason | Cause and fix |
| --- | --- | --- |
| `instances[<instance>].attach.socket` | `part has no socket "<name>"` | The attaching part lacks the named socket. |
| `instances[<instance>].attach.target` | `unknown instance "<id>"` | Target id not in the recipe; after renaming ids for composition, targets must be renamed too. |
| `instances[<instance>].attach.target_socket` | `part has no socket "<name>"` | The target's part lacks the named socket. |
| `instances[<instance>].attach` | `instance attaches to itself` | `target` equals the instance's own id. |
| `instances[<instance>].attach` | `attachment cycle` | A ring of attachments with no placed root. Place one instance with `placed` to anchor the chain; declaration order is irrelevant. |

## Material library paths

`MaterialLibrary::check_for(&building)` runs when a library is loaded beside
a building, in the preview and in `ashlar_bevy::read_library`.

| Path | Reason | Cause and fix |
| --- | --- | --- |
| `materials` | `name must not be blank` | Blank key in the library. |
| `materials[<key>]` | `UV offset must be finite` / `color, roughness and metallic must be finite in 0..=1` / `emissive must be finite and nonnegative` / `texture repeat size must be finite and positive with a finite reciprocal` | Definition constants out of range. |
| `materials[<key>].surface.<map>` | `name must not be blank` | Blank texture key under `base_color`, `normal`, `orm`, `height` or `emissive`. |
| `materials[<key>].surface.graph` | `name must not be blank` | Blank graph key on a `Shader` or `Graph` surface. |
| `materials[<key>].surface.params[<param>]` | `name must not be blank` / `parameter value must be finite` | Bad graph parameter on the definition. |
| `materials[<key>].surface.resolution` / `.baked_from.resolution` | `bake resolution must be a power of two in 256..=4096` | 128, 1000 or 8192. |
| `instances[<instance>].materials[<slot>]` | `missing material definition "<key>"` | A bound key the library does not define, cut slots included. |
| `instances[<instance>].materials[<slot>]` | `material "<key>" has a Plain surface, which binds no graph parameters; ...` | A parameter override on a `Plain` or `Files` surface. Only `Graph` and `Shader` take overrides; a tint is a second key. |

Whether an override names a parameter the graph actually declares is checked
by `ashlar_bevy::read_library`, by the same path, because the graph library lives
outside the domain crate. A misspelled field inside a surface in RON is a
parse error naming the field, not a validation error.

## Grounding errors

`fit_ground` returns `GroundError`, not a path.

| Variant | Cause and fix |
| --- | --- |
| `Invalid("terrain patch dimensions or heights")` | `TerrainPatch::new` with a non-finite origin, a non-positive or non-finite step, a side outside `2 ..= 1025`, a height count not equal to `size[0] * size[1]`, or a non-finite height. |
| `Invalid("sample rectangle or pose")` | `range` called with an inverted rectangle or a pose that is not yaw-only. |
| `Invalid("footprint, entrance or fitting policy")` | Inverted footprint, `slab_bottom >= floor`, an entrance socket with pitch or roll, a non-finite `at` or `yaw`, a non-positive policy value, `support_width * 2` at or above the footprint's smaller side, `max_span < 0.1`, more than 1024 ramp search steps, or more than 1024 support segments per side. |
| `OutsidePatch` | The yawed footprint, a support segment, the landing, the ramp or a toe probe reaches past the patch, or clips to nothing. Gather a larger patch around `at`. |
| `SupportDepth { required, limit }` | Ground under a support segment, the landing or the ramp lies more than `max_depth` below the slab. Move the site or raise the limit. |
| `NoApproach` | No ramp length in `search_step ..= max_ramp_length` gives a toe step within `max_toe_step`, a grade within `max_grade` and clear ground along the landing and ramp. An obstacle between threshold and toe triggers this. |

## Mesher failure modes

`ManifoldMesher::mesh` and `mesh_building` return `MeshError { path, reason }`.
`mesh_building` prefixes the mesher's path with
`parts[<part>].elements[<element>].`, so a kernel failure inside a cutter
reads `parts[study:facade].elements[panel].geometry.cutters[0]: ...`.

| Path | Reason | Cause and fix |
| --- | --- | --- |
| a validation path | the validation reason | The mesher re-runs `Geometry::check` first; a raw-data edit that bypassed `build()` fails here. |
| `mesher.sharp_angle` | `must be between 0 and 180 degrees` | A custom `ManifoldMesher` with a bad crease angle. |
| expression path | `cylinder exceeds segment budget` | `segments` above `max_segments` (4096 by default). |
| expression path | `solid exceeds triangle budget` | Any intermediate result above `max_triangles` (one million). Checked after every subtraction, union, array and mirror, and after each node's pose. |
| expression path | kernel status text | Manifold reported a non-manifold or invalid result for that node. |
| expression path | `unsupported shape ...` | A `Shape` variant this backend does not implement; the enum is non-exhaustive. |
| `geometry` | `kernel omitted normals` / `invalid surface run` / `vertex index overflow` / `invalid kernel vertex index` | Malformed kernel output; report rather than work around. |
| `geometry` | `non-finite or degenerate output triangle` | A zero-area triangle in the kernel output, typically from coincident faces of a solid and a cutter. Offset the cutter so faces do not coincide. |
| `geometry` | `invalid surface UV frame` | `planar_uvs` could not frame a face; the same degenerate-face cause. |
| `parts[<part>].elements[<element>]` | `surface cannot be projected under this UV mode` | A non-`Planar` `UvMode` met a degenerate triangle. |
| `parts[<part>].elements[<element>]` | hull status text / `unsupported collision ...` | `Collision::Hull` failed in the kernel, or an unknown `Collision` variant. |
| `<element>` | `mesher returned inconsistent surface metadata` | A custom mesher whose buffers disagree with `TriangleMesh::is_consistent`. |

An empty result is not an error. Subtracting a solid from itself, or a
cutter that covers the whole solid, returns `TriangleMesh::default()`; the
element is still listed with zero triangles when it has no cut slot, dropped
from the batches when it has one, and derives no collision proxy. A visible
hole, a recess where an opening was meant, a proxy across a doorway or a
texture seam is authoring rather than a failure; the rules for each are in
the "Rules that bite" section of `SKILL.md`.
