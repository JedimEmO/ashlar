# ashlar builder API surface

Everything below is exported from the `ashlar` crate root unless a module is
named. Positions are `ashlar::glam::DVec3` and rotations `DQuat`; use the
re-exported `glam` so recipe vectors and consumer vectors are one type.

## Geometry expressions

`Geometry { shape: Shape, pose: Pose }` is a solid expression with a
placement applied after evaluation. `Shape` is `#[non_exhaustive]`.

| Constructor | Meaning |
| --- | --- |
| `Geometry::cuboid(size: [f64; 3]) -> Geometry` | Axis-aligned box from the origin to `size`. Every extent positive. |
| `Geometry::chamfered_cuboid(size: [f64; 3], bevel: f64) -> Geometry` | Box with planar edge and corner chamfers that keeps its outer bounds. `0 < bevel < min(size) / 2`. |
| `Geometry::cylinder(radius: f64, height: f64, segments: u32) -> Geometry` | Cylinder on the Y axis from Y=0 to Y=height; `segments >= 3`. |
| `Geometry::extrude(profile: impl IntoIterator<Item = [f64; 2]>, depth: f64) -> Geometry` | Simple polygon in X/Z, either winding, extruded from Y=0 to Y=depth. |
| `Geometry::revolve(profile: impl IntoIterator<Item = [f64; 2]>, segments: u32) -> Geometry` | Simple polygon of `[radius, height]` pairs, radius never negative, turned a full circle about Y: domes, pods, tanks, flared towers. Concave is fine; a vertex at radius zero closes the solid on the axis. `segments >= 3`, within the mesher's segment budget. |
| `.placed(self, pose: Pose) -> Geometry` | Set this node's placement in its parent frame. |
| `.subtract(self, cutter: Geometry) -> Geometry` | Wrap in `Shape::Difference { solid, cutters: vec![cutter] }` with identity pose. Faces the cutter created carry cut provenance. |
| `.union(self, other: Geometry) -> Geometry` | `Shape::Union` of two solids; no cut faces of its own. |
| `Geometry::union_all(solids: impl IntoIterator<Item = Geometry>) -> Geometry` | One kernel union over the whole group; at least two solids. |
| `.arrayed(self, count: u32, step: Pose) -> Geometry` | `count` fused copies, copy `n` with `step` composed `n` times; `1 ..= MAX_ARRAY_COUNT` (1024). |
| `.mirrored(self, plane: MirrorPlane) -> Geometry` | The reflection alone, winding corrected. Union with the original for symmetry. |
| `.check(&self) -> Result<(), ValidationError>` | Validate a bare expression with the path `geometry`. |
| `.cut_material(self, slot: impl Into<String>) -> Geometry` | On a cutter only: the slot its exposed faces wear, ahead of the element's cut slot. |
| `.portal(self, id: impl Into<String>) -> Geometry` | On a cutter only, and not under an array or mirror: publish the opening as a portal rectangle. |
| `.cutters(&self) -> Vec<&Geometry>` | Cutter roots in depth-first order; index `k` is `FaceOrigin::Cutter(k)`. |
| `Geometry::hull(points) -> Geometry` | Convex hull of 4 to `MAX_HULL_POINTS` points that are not coplanar. |
| `Geometry::ball(radius: f64, rings: u32) -> Geometry` | A hull of generated points, centred on the origin; the blast solid. |
| `.bounds(&self) -> Option<[DVec3; 2]>` | Conservative box without evaluating; tight for an unrotated primitive. |

`Shape` variants, for reading or constructing raw data: `Cuboid { size }`,
`ChamferedCuboid { size, bevel }`, `Cylinder { radius, height, segments }`,
`Extrusion { profile: Vec<[f64; 2]>, depth }`,
`Revolve { profile: Vec<[f64; 2]>, segments }`, `Hull { points }`,
`Difference { solid: Box<Geometry>, cutters: Vec<Geometry> }`,
`Union { solids: Vec<Geometry> }`,
`Array { solid: Box<Geometry>, count: u32, step: Pose }`,
`Mirror { solid: Box<Geometry>, plane: MirrorPlane }`.

`MirrorPlane::new(axis: Axis, offset: f64)` names a plane perpendicular to
`axis` crossing it at `offset` metres. `Axis` is `X`, `Y` (default) or `Z`,
with `unit()`, `index()` and `tangents()` helpers.

## Poses

`Pose { translation: DVec3, rotation: DQuat }`, `Copy`, default identity.

| Function | Meaning |
| --- | --- |
| `Pose::at(translation: [f64; 3]) -> Pose` | Translate without rotating. |
| `.rotated(self, rotation: DQuat) -> Pose` | Set an absolute unit-quaternion rotation. |
| `.transform_point(self, point: DVec3) -> DVec3` | `translation + rotation * point`. |
| `.compose(self, local: Pose) -> Pose` | Express `local` in this pose's parent: self then local. |
| `.inverse(self) -> Pose` | The placement that undoes this one. |

## Elements

`Element { id, geometry, material_slot, cut_material_slot: Option<String>, uv: UvMode, collision: Collision }`.

| Function | Meaning |
| --- | --- |
| `Element::new(id: impl Into<String>, geometry: Geometry, slot: impl Into<String>) -> Element` | A named solid on a material slot; `Planar` UVs, no collision. |
| `.cut_material(self, slot: impl Into<String>) -> Element` | Bind faces created by cutters to a second slot; the mesher splits the element into outer and cut batches. |
| `.uv(self, mode: UvMode) -> Element` | Choose the texture projection. |
| `.collision(self, collision: Collision) -> Element` | Declare a static proxy derived by the mesher. |
| `.standalone(self) -> Element` | Keep this element its own mesh in a merged building. |
| `.far(self) -> Element` | Keep this element at every level of detail however thin it is: lights, neon, signs. |
| `.interior(self) -> Element` | Declare `Side::Interior`; a group is split by binding and side. |
| `.slot_for(&self, origin: FaceOrigin) -> &str` | The slot a face wears: cutter slot, cut slot, main slot. |

`UvMode` is `#[non_exhaustive]`: `Planar` (default, per-face frame and origin),
`Box` (part axes, shared origin, foreshortens angled faces),
`Cylindrical { axis: Axis }` (arc length along U, height along V, one seam).

## Parts

`Part { id, elements: Vec<Element>, sockets: Vec<Socket> }`.

| Function | Meaning |
| --- | --- |
| `Part::builder(id: impl Into<String>) -> PartBuilder` | Start a reusable definition. |
| `PartBuilder::element(self, element: Element) -> PartBuilder` | Append a solid. |
| `PartBuilder::socket(self, socket: Socket) -> PartBuilder` | Append an attachment frame. |
| `PartBuilder::build(self) -> Result<Part, ValidationError>` | Validate names, geometry and socket poses under `parts[id]`. |

## Sockets

`Socket { id, pose }`. `Socket::new(id: impl Into<String>, pose: Pose) -> Socket`
records a frame relative to the part origin. By convention the frame's +Z
points outward from the part. Sockets are metadata until an instance attaches
to one; the same type names a `GroundingSpec` entrance.

An attachment is stored in the public field `instance.attach` as
`Option<Attachment { socket, target, target_socket, facing }>`, where `facing`
is `Opposed` (the attached part turns half a turn about the socket Y axis) or
`Aligned` (frames coincide). Neither `Attachment` nor `Facing` is re-exported
from the crate root, so they are read through `instance.attach` and set only
by `attach` and `attach_aligned`.

## Instances

`Instance { id, part, pose, attach: Option<Attachment>, materials: BTreeMap<String, Binding> }`.

| Function | Meaning |
| --- | --- |
| `Instance::new(id: impl Into<String>, part: impl Into<String>) -> Instance` | Reference a part by id at the identity pose. |
| `.placed(self, pose: Pose) -> Instance` | Set an absolute pose; clears any attachment. |
| `.attach(self, socket, target, target_socket) -> Instance` | Derive the pose so this part's socket faces the target instance's socket. All three arguments `impl Into<String>`. |
| `.attach_aligned(self, socket, target, target_socket) -> Instance` | Derive the pose with the two socket frames coincident. |
| `.material(self, slot, material) -> Instance` | Override one slot with a key and no parameters. |
| `.group(self, id: impl Into<String>) -> Instance` | Name a declared merge group; none means the building. |
| `.binding(self, slot, binding: Binding) -> Instance` | Override one slot with a key and graph parameter values. |

Resolution formula, from `BuildingRecipe::build`: with `own` the attaching
socket's pose and `other` the target socket's pose,
`pose = target.pose.compose(other.compose(facing)).compose(own.inverse())`,
where `facing` is the identity for `Aligned` and a half turn about Y for
`Opposed`.

## Building and recipe

`BuildingRecipe { version: u32, id, grid: Option<f64>, parts: Vec<Part>, instances: Vec<Instance>, materials: BTreeMap<String, Binding> }`
is the editable, Serde-facing form; `SCHEMA_VERSION` is 1.

| Function | Meaning |
| --- | --- |
| `Building::builder(id: impl Into<String>) -> BuildingBuilder` | Start an assembly. |
| `BuildingBuilder::grid(self, metres: f64)` | Record an editor hint; snaps nothing. |
| `BuildingBuilder::part(self, part: Part)` | Register a definition; unused parts are allowed and skipped by the mesher. |
| `BuildingBuilder::instance(self, instance: Instance)` | Place a part; forward references resolve at build. |
| `BuildingBuilder::material(self, slot, material)` | Default binding for a slot. |
| `BuildingBuilder::binding(self, slot, binding: Binding)` | Default binding with parameter values. |
| `BuildingBuilder::merged(self)` | Union elements by group instead of drawing parts; gives up instancing. |
| `BuildingBuilder::group(self, group: MergeGroup)` | Declare a group; `MergeGroup::new(id).storey(n)`. |
| `BuildingBuilder::room(self, room: Room)` | Declare a box volume; `Room::new(id, size).placed(pose).group(id).portal(id)`. |
| `BuildingBuilder::build(self) -> Result<Building, ValidationError>` | Validate the whole assembly and resolve attachments. |
| `BuildingRecipe::build(self) -> Result<Building, ValidationError>` | The same validation over loaded data. |
| `Building::recipe(&self) -> &BuildingRecipe` | Read the validated data. |
| `Building::into_recipe(self) -> BuildingRecipe` | Edit, then `build()` again. |
| `Building::part(&self, id) -> Option<&Part>` | Look up a definition. |
| `Building::instance(&self, id) -> Option<&Instance>` | Look up a placement, with its resolved pose. |
| `Building::material(&self, instance_id, slot) -> Option<&str>` | Effective key: instance override, else palette. |
| `Building::binding(&self, instance_id, slot) -> Option<&Binding>` | Effective binding, never a merge of the two. |
| `Building::bindings(&self)` | Every `(instance, slot, &Binding)` the parts actually draw, cut slots included. |

`ValidationError { path: String, reason: String }` displays as `path: reason`.

## Material binding

| Type or function | Meaning |
| --- | --- |
| `Binding { material: String, params: BTreeMap<String, ParamValue> }` | One slot's key plus overridden graph parameters. Serializes as the bare key when `params` is empty. |
| `Binding::new(material: impl Into<String>) -> Binding` | Key alone. `From<&str>` and `From<String>` do the same. |
| `.param(self, name: impl Into<String>, value: ParamValue) -> Binding` | Override one parameter. |
| `.is_plain(&self) -> bool` | No overrides. |
| `ParamValue` | `Float(f32)`, `Color([f32; 3])` linear RGB, `Int(i32)`, `Bool(bool)`; `is_finite()`. |
| `MaterialLibrary { materials: BTreeMap<String, MaterialDefinition> }` | Key to definition; `check_for(&self, building: &Building)` validates every binding a building draws. |
| `MaterialDefinition` | `base_color`, `roughness`, `metallic`, `emissive`, `tile_metres: [f32; 2]`, `tile_scale: Option<f32>` (declares a deliberate scale of the graph's own repeat), `uv_offset`, `surface: Surface`, `strands: Option<StrandSettings>`; `check(&self, path)`. |
| `Surface` | `Plain`, `Files { base_color, normal, orm, height, emissive, baked_from }`, `Graph(Bake)`, `Shader { graph, params }`. Only the last two accept parameter overrides. |
| `Bake { graph, params, resolution }` | Resolution a power of two in `256 ..= 4096`. |

## Collision

`Collision` is `#[non_exhaustive]`: `None` (default), `Bounds` (axis-aligned
box of the evaluated solid in part space), `Hull` (convex hull).

`ConvexSolid { vertices: Vec<DVec3> }` is what a backend hands back:
`ConvexSolid::from_bounds(min, max)`, `.bounds() -> Option<[DVec3; 2]>`,
`.placed(&self, pose) -> ConvexSolid`.

## Mesh output

Types a content test reads, from `ashlar` and `ashlar_manifold`.

| Type or function | Meaning |
| --- | --- |
| `trait GeometryMesher { fn mesh(&self, geometry: &Geometry) -> Result<TriangleMesh, MeshError>; }` | The evaluation port. |
| `ashlar_manifold::ManifoldMesher { sharp_angle: 45.0, max_segments: 4096, max_triangles: 1_000_000, weld_tolerance: WELD_TOLERANCE }` | Default tessellation policy. |
| `ashlar_manifold::mesh_building(building: &Building, mesher: &dyn GeometryMesher) -> Result<MeshedBuilding, MeshError>` | Evaluate every used part once. |
| `MeshedBuilding { building, parts: BTreeMap<String, Vec<ElementMesh>>, part_colliders: BTreeMap<String, Vec<ElementCollider>> }` | Shared part meshes; `colliders()` places proxies per instance. |
| `ElementMesh { id, material_slot, is_cut, mesh: TriangleMesh }` | One batch; two per element when a cut slot is set and both batches are non-empty. |
| `TriangleMesh { positions, normals, uvs, indices, sources }` | Indexed, outward-wound, f64. `triangle_count()`, `triangles()`, `corners()`, `is_consistent()`, `is_cut(face)`, `cut_faces()`, `weld(tolerance)`, `unweld()`, `project_uvs(mode) -> bool`. |
| `FaceSource { operand: u32, origin: FaceOrigin }` | Where a face came from; `FaceOrigin` is `Body`, `Cutter(k)` or `Damage(j)`. `operand` indexes what was handed to the mesher, and is `FaceSource::NO_OPERAND` on a damage face. |
| `MeshedBuilding::groups: Vec<MergedGroup>` | Merged buildings only. `MergedGroup { id, storey, operands, batches: Vec<GroupBatch>, bounds, collider }`, `GroupBatch { binding, side, mesh }` in building space. `parts` then holds standalone elements only. |
| `MeshedBuilding::pieces()` | `Piece { origin: PieceOrigin, storey, side, pose, mesh, binding }`, group batches first. `mesh_key()`, `shared_key()`, `label()`. |
| `MeshedBuilding::portals() -> Vec<Portal>` | One per marked cutter per placement, building space, with storey. |
| `Building::room_at(point)`, `Room::contains(point)` | Kernel-free room lookup. |
| `ashlar_manifold::mesh_building_with_damage(&building, &log, &mesher)` | Stateless replay of a `DamageLog`. |
| `ashlar_manifold::GroupSolids::build(&building, &log, mesher)`, `.apply(&damage) -> Vec<GroupHit>` | Kept solids for play; `GroupHit { group, debris }`. |
| `MeshedBuilding::mesh_colliders()`, `groups_touched`, `standalone_touched`, `replace_group`, `record` | Collision and routing after a hit. |
| `WELD_TOLERANCE` | `1e-6` metres. |
| `MeshError { path, reason }` | Backend failure by authored location. |

## Grounding

| Type or function | Meaning |
| --- | --- |
| `GroundRect::new(min: [f64; 2], max: [f64; 2])` | Horizontal X/Z bounds; `corners()`. |
| `GroundingSpec { footprint: GroundRect, floor: f64, slab_bottom: f64, entrance: Socket }` | Content-owned fitting contract; `slab_bottom < floor`; entrance +Z outward, yaw only. |
| `GroundingPolicy { clearance, embed, max_depth, support_width, max_span, landing_length, ramp_width, max_grade, max_ramp_length, search_step, vegetation_margin, max_toe_step }` | All positive; `support_width * 2` under the footprint's smaller side; `max_span >= 0.1`. |
| `TerrainPatch::new(origin: [f64; 2], step: f64, size: [usize; 2], heights: Vec<f64>) -> Result<TerrainPatch, GroundError>` | Row-major heights, X fastest, `2 ..= 1025` per side, cells split along `Diagonal::Anti`. |
| `TerrainPatch::with_diagonal(origin, step, size, heights, diagonal: Diagonal) -> Result<TerrainPatch, GroundError>` | The same with an explicit cell split; `Diagonal` is `Anti` (default) or `Main`. |
| `TerrainPatch::range(&self, rect: GroundRect, pose: Pose) -> Result<[f64; 2], GroundError>` | Exact ground extrema under a yawed rectangle. |
| `fit_ground(spec: &GroundingSpec, policy: GroundingPolicy, terrain: &TerrainPatch, at: [f64; 2], yaw: f64) -> Result<Grounding, GroundError>` | The pure solver. |
| `Grounding { pose, solids: Vec<GroundSolid>, exclusions: Vec<GroundRect>, terrain_range, ramp_length, ramp_grade, toe }` | World pose, building-local prisms, world-space occupancy. |
| `GroundSolid { vertices: [DVec3; 8] }` | Eight-corner prism; `cuboid(size, pose)`, `triangles()`, `placed(pose)`; converts into `ConvexSolid`. |
| `GroundError` | `Invalid(String)`, `OutsidePatch`, `SupportDepth { required, limit }`, `NoApproach`; `#[non_exhaustive]`. |
