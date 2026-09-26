# Changelog

Notable changes to the ashlar workspace. Every crate shares one version.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
the project uses [semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

First extraction of the building system out of the game it grew in. Nothing has
been released yet, so this section is the whole story so far.

### Added

- **A manual and a site** (2026-09-26). The docs folder is an mdBook: an
  introduction, getting started, the preview, a tour of the kits, the
  architecture and a glossary beside the existing guides and the decisions.
  `just site` assembles it with the API docs and a browser demo into
  `target/site`, and `.github/workflows/pages.yml` deploys that to GitHub
  Pages. The README is an entry page with screenshots; `first_building` is the
  getting-started code as a runnable example.
- **A browser demo** (`examples/web`): the showcase's scenes baked and loaded
  the way a game loads them, by day and by night, with orbit and walk cameras,
  a storey cut, the interior and a level-of-detail view; and a material stage
  whose graph parameters are sliders that re-bake in the browser, on one
  thread. It runs natively too, for development. It streams pieces by
  distance, so a browser's frame over the metropolis takes half the time it
  did; it draws without shadows by night, and on a phone it draws less and
  opens with its panel folded.
- **Level bands that work on WebGL2**: `AshlarPlugin::bands`, `Bands::Auto`
  by default, cuts each level at the middle of its crossfade margin with an
  abrupt `VisibilityRange` on a render device that cannot crossfade one: Bevy
  0.19.1's WebGL2 fallback declares the range uniform as 64 `vec4`s in the
  shader and 16 bytes in the bind group layout. Native devices crossfade as
  before.
- `Content::review_pngs`: the content step can write each map's full level as a
  PNG beside its KTX2, for looking at. `just materials` is that review export.

- **The production path**: a game ships baked files and links no kernel and no
  graph engine. [docs/guide/integration.md](https://github.com/JedimEmO/ashlar/blob/main/docs/guide/integration.md) walks it
  end to end.
- **`ashlar::BakedBuilding` and the `.ashlar` file** (ADR 0006): a meshed
  building and its coarser levels of detail, with level zero's collision proxies,
  portals and rooms, as a RON manifest plus a binary mesh table. `write` and
  `read`; a read refuses what it cannot account for, by kind, and never panics.
- **Levels of detail from the recipe**: `LodPolicy` (`until`, `min_feature`,
  `segment_scale`, `drop_interior`), `Geometry`, `Part` and `Building::simplified`,
  and `LodPolicy::ladder()`, three levels to 60 m, 250 m and beyond.
  `ashlar_manifold::bake` meshes a building at every level, re-meshing only
  what a level changed; a building nothing of which survives a level bakes that
  level empty. The 4 x 4 metropolis draws 3.8 M, 1.35 M and 0.91 M
  triangles at its three levels.
- **`ashlar_bevy::baked::AshlarPlugin`**, with default features: loads `.ashlar`
  buildings and `.materials.ron` libraries as assets and spawns an
  `AshlarBuilding` as `AshlarPiece` children, one `VisibilityRange` band per
  level with a crossfade. Colliders arrive as `AshlarCollider`, rooms and portals
  as `AshlarSpaces`, and `AshlarBuildingSpawned` or `AshlarFailed` says how it went.
  Meshes and materials are shared across instances and across levels that
  left a piece unchanged, through `AshlarCache`; a merged group switches level
  by its own distance; a rewritten file respawns its buildings, a file that
  arrives after a failure clears it, and an asset nothing holds leaves the cache.
- **`ashlar-content`**: the content step as a library. `Content::ship` bakes a
  material library to KTX2 and strand sets and each building to `.ashlar`, under
  one asset root, writing through a temporary file and a rename.
  `flatten_overrides` turns each distinct per-instance graph override into a
  definition of its own, keyed `<material>--<hash>`, so it bakes once however
  many buildings use it. `just content` runs it over the showcase.
- **The integration template**: `examples/integration-content` bakes a small
  village of the game's own, and `examples/integration-game` draws it with one
  plugin and one component. The split a game copies.
- **The showcase's geometry, taken to production quality** (2026-09-26), in
  five reviewed passes. The dark city's towers come in three styles with their
  own massing, designed crowns, built terraces and podiums with glazed doors;
  the landmark gets a colonnade, sky floors and a lantern; every storey is an
  enterable office and every ground floor a lobby. The street level gets
  lowered crossings, built props and seeded alleys and markets; the sci-fi kit
  is built out and brought into the city's palette as its corporate colony; the
  corporate kit, the house and the rooftop pieces get their joints, frames and
  supports. The 4 x 4 metropolis draws 4.28 M / 1.20 M / 0.78 M triangles by
  level, held there by a test.
- **Levels of detail judge thin, not small** (2026-09-25, ADR 0006
  amendment): an element or union member is dropped by the middle of its
  extents rather than its largest, so sills, mullions, rails and strips go at
  a distance however long they are, and `Element::far()` keeps a glowing line
  at every level. The 4 x 4 metropolis's far level draws 0.76 M triangles
  rather than 1.59 M.
- **A dark city** (2026-09-25): the city kit is rebuilt in one look, the
  corporate kit's language in dark stained concrete and lit for the night.
  Towers step in twice as they rise to ninety storeys, landmarks pass a
  hundred, and every storey is still an enterable office with a room. Six- and
  ten-metre back alleys run between a lot's towers, dressed with gutters,
  dumpsters, pipes, air-conditioners, fire escapes, caged sodium lamps, neon
  blade signs and cables strung overhead; tenements with fire escapes and night
  markets fill other lots. Which offices are lit is a per-storey material
  override, so a thousand storeys of one part cost a handful of materials.
  Scenes `city-alley`, `city-tower`, `city-landmark` and `city-tenement` are
  new. Its materials are new stdlib graphs: `library:stained-concrete`
  (streaked from each storey's slab edge, grime at its foot, over a new
  `weathering:streaks` compound), `library:office-window` and
  `library:shopfront` (lit interiors in the emissive map), and a `wet`
  parameter on `library:asphalt`, `library:road` and `library:paving-slabs`
  (plus `tint` on paving) whose default leaves the dry bake byte for byte. The
  preview gains `--night` (a moon key, bloom, and a shadowless light in every
  small strongly emitting piece, so lamps pool and signs wash their wall),
  `--key`, `--ambient` and `--focus X,Y,Z` for a street-level orbit; the
  `baked` example gains `--night`. The glass, stepped, round and slab families, `city::Style`,
  `metropolis_styled` and `metropolis-corporate` are gone.
- **Packaging**: licence files in every publishable crate, docs.rs metadata,
  and `just configs`, which CI runs, checking and documenting the game and tool
  configurations one at a time so a feature-gated link cannot hide.

- `Geometry::revolve_arc(profile, segments, sweep)`: part of a turn, so a round
  wall is cut into runs that each carry a tight convex proxy.
- Walkable interiors for the sci-fi kit (domes, dome house, hab pod and tube
  chain, module, tower) built additively with rooms, portals and furniture;
  `scifi::Habitat::place` adds one to a building at a pose with a look.
- A city kit and `city::metropolis(seed, blocks)`: enterable storeys, a
  street kit, and a deterministic street grid of lots. Scenes `city-kit`,
  `city-block`, `metropolis`, `metropolis-large`, `city-<piece>`. Its first
  tower families gave way to the dark city above.
- City materials: `library:curtain-wall`, `library:window-band`,
  `library:road`, `library:interior-panelling`, `library:holo-sign`.

- A frontier sci-fi kit in `examples/showcase/src/scifi.rs`, in the idiom of
  Anarchy Online and Star Wars Galaxies: adobe dome huts (two sizes) and a
  two-tier dome house, a capsule hab pod on stilts, walkway tubes that chain
  pod to pod by socket, a flared hab tower, a prefab module, a quonset hangar,
  a landing pad, perimeter walls, posts, a blast gate and a force fence, a
  moisture vaporator, a generator, a storage tank, a vendor kiosk, conduit
  runs, dish antennas, lamps, crates and barrels. Scenes `scifi-kit`,
  `scifi-outpost`, `scifi-colony`, and `scifi-<piece>` for each piece alone.
- Four library materials for it: `library:hull-plating` (panelled composite
  with fasteners, grilles and hatches; `color`, `wear`), `library:tread-plate`
  (`rust`), `library:adobe` and `library:desert-sand` (`color` each).

- `Geometry::revolve(profile, segments)` and `Shape::Revolve`: a `[radius,
  height]` profile turned a full circle about Y, evaluated by the kernel's own
  revolve at an authored segment count. Domes, capsules, tanks and flared
  towers, which no extrusion reaches.

- **`ashlar`**: the Bevy-free recipe domain. `Geometry`, `Shape` and `Pose`;
  `Element`, `Part`, `Socket`, `Instance`, `BuildingRecipe` and the validated
  `Building`; `MaterialLibrary` and `MaterialDefinition`; the `GeometryMesher`
  port with `TriangleMesh` and `planar_uvs`; and `fit_ground` with
  `GroundingSpec`, `GroundingPolicy`, `TerrainPatch` and `Grounding`.
- **`ashlar-manifold`**: `ManifoldMesher` and `mesh_building`, evaluating
  recipes through the native Manifold kernel into `MeshedBuilding`.
- **`ashlar-bevy`**: `Definitions`, `read`, `create`, `mesh` and `ground_mesh`,
  turning meshes and material libraries into Bevy assets with generated
  tangents and preflighted textures.
- **`ashlar-showcase`** (unpublished): the outpost, office and corporate kits,
  their grounding contract, and the generator for their tileable PBR maps.
- **`ashlar-preview`** (unpublished): the orbit viewer and the headless
  reference gallery, as a library over a pluggable `Catalog` of scenes with a
  thin binary that supplies the showcase.

### Added since the extraction

- **Expression nodes**: `Shape::Union`, `Shape::Array` and `Shape::Mirror`, with
  `Geometry::union`, `union_all`, `arrayed` and `mirrored`, evaluated through
  the kernel and documented for cut-face provenance.
- **UV modes**: `UvMode` per element, with `Planar` (the default and the old
  behaviour), `Box` and `Cylindrical`, plus `box_uvs`, `cylindrical_uvs` and
  `project_uvs` beside `planar_uvs`.
- **Indexed meshes**: `TriangleMesh` carries an index buffer, with `weld`,
  `unweld`, `triangles`, `corners` and `is_consistent`. The Bevy adapter uploads
  `Indices::U32`.
- **Derived collision**: `Collision` on an element and `ConvexSolid` in the
  domain; `MeshedBuilding::part_colliders` and `colliders()` in the mesher. See
  [ADR 0002](https://github.com/JedimEmO/ashlar/blob/main/docs/adr/0002-attachment-and-derived-collision.md).
- **`Catalog::asset_root`**: a downstream preview binary points `--asset-root`
  at its own game's assets by default, instead of at this workspace's.
- **Grounding to collision**: `ConvexSolid::from(&GroundSolid)`, so a game
  builds one compound collider out of a fitted site and the proxies its
  elements declare.
- **Socket attachment**: `Instance::attach` and `attach_aligned`, with
  `Attachment` and `Facing`, resolved into poses by `build()`.
- **Terrain triangulation**: `Diagonal` and `TerrainPatch::with_diagonal`.
- **API hardening**: `#[non_exhaustive]` on the enums a consumer matches on, a
  `version` field on `BuildingRecipe` checked at build time, a `glam`
  re-export, and an index inside `Building` so `material`, the new `part` and
  the new `instance` are map lookups.
- `ashlar-manifold` documents on docs.rs without a C++ toolchain or a vendored
  kernel, and records why in its manifest.
- **Tests for `ashlar-bevy`**, which had none: mesh upload, welding, ground
  meshes, the UV transform, texture colour spaces and sampling, and every
  rejection `read` makes. All of it runs without a GPU, on `MinimalPlugins`
  plus `AssetPlugin` and Bevy's own image loader registered by hand.
- **Property tests** over the pure geometry, on `proptest`: profile validation
  answers rather than panicking and accepts every strictly convex ring,
  `planar_uvs` preserves edge length, `weld` then `unweld` is the identity on
  the triangles, and `Pose::transform_point` is an isometry.
- **Gallery tests** in `ashlar-preview` for everything below the screenshot:
  the scene queue, the unknown-scene error, the `index.html` and `manifest.tsv`
  writers, and the camera framing.
- **Crate documentation**: each of `ashlar`, `ashlar-manifold` and `ashlar-bevy`
  opens with what it is, where it sits in the layering and a worked example
  that is a real doctest, and the builder entry points carry short examples of
  their own. Sixteen doctests where there was one.
- **Per-crate `README.md`** for every publishable crate, with `readme` and
  `documentation` in their manifests, so a crates.io listing and a docs.rs
  sidebar say what the crate is, what it costs to build and which Bevy the
  adapter pairs with.
- **`ashlar-material`**: procedural material graphs as plain data, beside
  `ashlar` rather than under it. `MaterialGraph`, `Param`, `Input`, `PbrOutput`
  and a vocabulary of generator, transform, filter, buffered and structure
  `Node`s, with `build()` returning a validated `Material` the way
  `BuildingRecipe::build` returns a `Building`: node paths of the form
  `nodes[id].inputs[name]`, type inference with the float/colour conversions,
  cycles named by the ring they close, and a `MaterialGraphLibrary` that
  resolves subgraphs and refuses recursion. Every port carries an integer
  `Period`, so a graph whose output does not tile is rejected by node path
  before anything is baked. A built material lowers once into the typed SSA
  expression in `ir`, which folds constants — this is how a parameter exposed
  for baking stops being an instruction — gives one id to identical
  sub-expressions, drops everything no output reads, and hashes to a stable
  64-bit cache key; `interp` is the reference backend that runs that
  expression over `f32`, and its only randomness is the lattice hash the
  showcase's pixel loops already used, bit for bit, so the study's textures
  survive the move to graphs. The first set of nodes lowers with it — the
  coordinate, value noise and its octaves, brickwork's mask, levels, the
  arithmetic blend modes, a gradient, the scalar operators, invert, mix,
  decompose and combine — each pinned by a hand-computed value and by
  evaluating the field at both seams and a repeat along, over random periods
  and parameters; a node or a setting no backend has reached yet is a refusal
  naming the node that asked rather than a quietly different picture. A brick
  bond is held to the whole wall rather than to one row, so an offset that only
  looks like a fraction — and would walk visibly out of bond across a tall
  wall — is refused at `offset` instead of claiming a period in v it does not
  have. `bake` turns a graph and a set of parameter values into a tileable
  texture set: the expression run once per texel over one repeat, rows
  rasterised across `std::thread::scope` threads with no extra dependency, and
  the same bytes however the rows were divided, since a texel reads nothing but
  its own coordinate. The normal is derived from the height plane rather than
  computed beside it, so relief and parallax describe one surface; the `f32`
  planes are exposed before anything is quantised, and `bake_with_report` says
  what the bake cost and what range the height actually reached, which is the
  one way a correct bake can still ship a flat map. `bake::preflight` stops after the lowering, for
  a caller that wants to know whether a graph would bake rather than what it
  bakes to.
  See
  [ADR 0003](https://github.com/JedimEmO/ashlar/blob/main/docs/adr/0003-procedural-materials.md).
- **Buffered filters** in `ashlar-material`, and the plane pipeline under them.
  `Blur`, `OcclusionFromHeight`, `Distance`, `Erode`, `Dilate` and `Buffer`
  lower to a plan and an `Op::Sample`: everything upstream is rasterised once
  into a `Plane`, the filter runs over it as ordinary Rust, and everything
  downstream reads the result bilinearly and with wrap. Every filter reads its
  neighbours across the seam, so a blurred noise still meets itself and the
  period a port carries still means what it says — a filter that clamped at the
  edge would leave a period that was right and a picture that was wrong, so each
  is tested against a field whose answer is different if it had. Every radius is
  a reach in UV rather than a count of texels: the blur is a Gaussian truncated
  at its radius with sigma a third of it, the occlusion marches eight directions
  and keeps the steepest rise in each, the distance is a jump flood over the
  torus in UV units, and the morphologies take the square neighbourhood of the
  radius, which is separable and gives an axis-aligned mask back rather than
  rounding its corners. A plane is the expensive thing in a bake, so two nodes
  that ask for the same filter over the same expression share one, a buffered
  node no output reads leaves none, and `bake::rasterise_with` takes a
  `BakeCache` that keeps planes across bakes for a preview that re-bakes on
  every slider. `BakeError` is `#[non_exhaustive]` and gained `Lattice`: a bake
  whose graph lays a lattice finer than the texels asked for is refused, because
  below that the answer is not a coarser surface but an arbitrary sample of one.
- **Mip chains** in `ashlar-material`. `BakeRequest::mips` fills every map with
  every level down to 1x1, filtered from the `f32` planes rather than from the
  encoded bytes and rather than by re-evaluating the graph at a lower
  resolution, which would make each level a different picture of the surface
  instead of a blurrier one. Each level is the 2x2 box filter of the one above
  it, which at an even size needs no wrap and no edge case at all: the groups
  partition the plane, so the mip of a repeat is still a repeat. Two planes are
  not a plain average. The chain carries the un-normalised mean of the level-0
  normals, so a level ships that mean's direction and its *length* is what says
  how much slope the level lost; the roughness is widened for it by the design's
  Toksvig form `r' = sqrt(r^2 + (1 - |n_avg|) * k)` over a documented
  `mips::TOKSVIG_K`, clamped to one, and widened from the box-filtered level-0
  roughness so the same lost variance is counted once rather than compounded
  down the chain. Without that a bumpy wall turns to glass at distance, which is
  the whole reason the pass exists. Everything else — base colour, metallic,
  occlusion, height, emissive — is box filtered plainly and encoded in its own
  format at every level, so height stays 16-bit and emissive stays half-float
  all the way down. `Planes::mips` is the chain as numbers and
  `Planes::encode_mips` is the chain as bytes; `BakeReport::levels` says how
  many there are, and `BakeError::Mips` is gone.
- **A KTX2 writer** in `ashlar-material`, and the study ships KTX2. `ktx2::write`
  turns one `Encoded` map into a file with every level inside it: the 80-byte
  header, the level index, the mandatory data format descriptor, a `KTXwriter`
  key and the levels, which are stored smallest first — the reverse of the
  index's order — each aligned as the specification asks. No dependency and no
  supercompression. The four formats a bake produces are the four Bevy's loader
  takes without transcoding, and the descriptor is what tells it that the base
  colour is sRGB and the normal map is not; the round trip is tested by parsing
  the bytes back with the `ktx2` crate Bevy itself pins, including comparing the
  descriptor against the one that crate generates for the same format.
  `ktx2::inspect` reads a file's header and level index back and checks them
  against its own length, which is what a startup preflight can afford:
  `ashlar-bevy` validates every `.ktx2` key with it, builds Bevy with the `ktx2`
  feature and no supercompression decoder, and refuses a supercompressed file by
  name. The showcase's asset writer now bakes with mips and writes
  `base.ktx2`, `normal.ktx2`, `orm.ktx2` and — new, because no PNG here could
  hold it — the sixteen-bit `height.ktx2`; the study's PNGs are deleted and
  `--write-png` beside `--write-study-assets` exports level 0 of each map for
  looking at. Level 0 is unchanged, so every golden hash is the number it was.
- **Runtime bake** in `ashlar-bevy`: a `Surface::Graph` definition becomes
  `Image` assets when the material is created, in the new `runtime_bake` module.
  Each plane is one image at its own format — `Rgba8UnormSrgb`, `Rgba8Unorm`,
  `R16Unorm`, `Rgba16Float`, untranscoded — with every mip level concatenated
  largest-first, repeat addressing, trilinear filtering and
  `RenderAssetUsages::RENDER_WORLD`, because these texels exist to be uploaded
  and a texture set is nineteen megabytes at 1024. The bake always asks for
  mips: a runtime-baked wall is looked at from the distances a file-baked one
  is, and Bevy builds no chain for an image it was handed. The bytes are the
  bytes the KTX2 of the same `Bake` carries, which is what makes moving a
  surface between files and a registration-time bake a change of variant and
  nothing in the picture — the study's facade rendered from a library flipped to
  `Graph` is pixel for pixel the facade rendered from its files. `BakeCache` is
  the resource that keeps them, keyed by the whole bake rather than a digest of
  it, so two definitions naming one `Bake` share one texture set and a hash
  collision cannot hand a building somebody else's textures; a per-building seed
  is then a distinct set in memory with no distinct file. `create` is unchanged
  and still needs nothing but an `AssetServer`; `runtime_bake::create_graph_material` is
  the superset that takes the graph library, the cache and `Assets<Image>` and
  handles every variant, so a library that can hold a graph surface never
  matches on the variant itself. A bake is CPU work that blocks — 0.16 seconds
  for the study concrete at 1024 across this machine's cores — so
  `runtime_bake::bake_images` is that work as a pure function of a bake and a
  graph library, callable from an `AsyncComputeTaskPool` task with only
  `BakeCache::insert` left for the main thread; the preview calls it
  synchronously, which is what a preview should do.
- **The second node set** in `ashlar-material`: every node the vocabulary
  declares now lowers, and so does every output of every one of them. Perlin
  noise on a lattice that wraps, with the same octaves value noise has and a
  gradient taken from a pair of hashes rather than from a sine and a cosine, so
  no noise in the crate calls `libm`; `Voronoi` with three metrics, a jitter and
  its distance, cell, edge and border outputs; `Tiles` over a grid, a hexagonal
  lattice and a herringbone weave, with the same mask, bevel, id and fill
  outputs brickwork has and with `Bricks` gaining the three it was missing;
  `Pattern`, `Shape` and `Scratches`; the resamplers `Transform`, `Warp`,
  `Mirror`, `Kaleidoscope` and `Tile`; `Curve` as a monotone cubic, `Adjust`,
  `Clamp`, `Switch` and `NormalFromHeight`; the seven blend modes past the
  arithmetic four; and `Subgraph`, inlined into the arena that instanced it so
  that it costs nothing at runtime and folds and shares as if it had been
  written out by hand. A resampler is `Lowering::substitute`, which emits the
  sub-expression again with another coordinate where `Op::Uv` stood: the honest
  cost of reading a source through a frame is that source again, and only the
  instructions that actually depend on the coordinate are copied. The
  periodicity property runs over the whole vocabulary at random integer periods
  and parameters, and each node also carries a hand-computed value or a stated
  invariant — Perlin is a half at every lattice corner, a Voronoi distance is
  zero at a feature point, a subgraph is the graph it instances inlined by hand.
  Three things are refused rather than lowered, each by path: a rounded or cut
  brick corner, the directional and slope blurs, and a **simplex** noise, whose
  skew is `(sqrt(3) - 1) / 2` and therefore has no integer period at all — the
  refusal says so rather than promising one later, and points at Perlin.
- **The finest lattice is carried through the graph**, rather than read off each
  node alone. A `Transform` that scales a noise by four lays four times the
  cells, and a `Tile` that scatters a shape sixteen ways lays sixteen times what
  the shape laid; reading the nodes one at a time answered the noise's own count
  and let a bake resolve a quarter of what it wrote. `Material::finest_lattice`
  now propagates — multiplied by a transform's scale with the axes exchanged on
  a quarter turn, by a tile's instance count and by a quadrant kaleidoscope's
  fold — and counts the graphs a `Subgraph` instances, since lowering inlines
  them. This is the concern phase two's first step left written down in
  `period.rs`, and it is what `BakeError::Lattice` now compares against.
- **An authoring loop in `ashlar-preview`**: the graph library is watched, and a
  parameter panel edits it in place. A change to the file re-reads and
  re-validates it and creates every material again from it, writing through the
  handles the scene already holds, so a save in an editor and the wall changing
  are one motion — measured at 279 ms for the study's five materials at 1024.
  The watch is a poll of the file's stamp twice a second rather than `notify`:
  that crate is not in this workspace's dependency graph, and Bevy's
  `file_watcher` feature, which is `notify`, was measured to bring a second
  `windows-sys` family past `cargo deny`'s duplicate rule. The stamp carries a
  digest of the bytes beside the timestamp, because two writes inside one kernel
  tick share an `st_mtime_ns` — 195 times out of 200 on this filesystem — and a
  missed edit is the one thing a watcher may not do. `M` opens the panel: every
  `Param` of one graph as a slider, a colour as three and a switch as a
  checkbox, editing the library as the pointer moves and baking when it is let
  go, since a 1024 bake is a fifth of a second and that is a fine pause after a
  drag and an impossible one during it. `S` or the Save button writes the
  library back to RON, over the file it was read from or wherever
  `--graphs-out` says. Clicking a wall points the panel at the graph behind it,
  through the `baked_from` a textured surface records; the arrows walk the
  library for the graphs nothing on screen uses. The widgets are Bevy's own:
  `bevy_ui_widgets` ships headless `Slider` and `Checkbox` in 0.19, so only
  their appearance is written here, and the alternative was measured first —
  `bevy_egui` 0.42, the release built against Bevy 0.19, brings a second
  `itertools` and a second `guillotiere`, neither under an existing `skip-tree`
  entry. The step adds no dependency at all.
- **`--bake`** on the preview turns every surface that records a graph into a
  `Surface::Graph`, so the preview renders the graphs rather than the files a
  content step wrote from them. That is what makes an edit visible: the study
  ships files, and should, and until now a graph library in a preview was a
  thing that could only be validated. The picture is the same either way —
  the facade rendered with `--bake` is pixel for pixel the facade rendered from
  its KTX2 files, checked.
- **Three authored materials, and a sheet to read them on.** `study:brick`,
  `study:plaster` and `study:painted-metal` in
  `examples/showcase/src/materials.rs` are the first surfaces in this workspace
  that are not ports of a pixel loop, and they are what the phase-two node set,
  the buffered filters and the mip chain were built for. The brick takes its
  tint per brick from `BrickOutput::Id` through a gradient of clay colours, its
  joint from a recess the horizon march darkens, and its grime from a `Distance`
  measured out of the mortar, because what dirties a brick wall is the joint it
  sits next to; the plaster crazes along a warped `Voronoi` border whose cracks
  are the *falloff* around a one-texel line rather than the line, which is the
  only way to get a crack a fixed width in UV instead of in texels; the painted
  metal chips to bare steel where `Scratches` and a knock field say it has been
  hit, rusts in the rim a second `Dilate` leaves around each chip, and is the
  one graph in the workspace whose `metallic` is a field rather than a constant,
  because paint is a dielectric and the steel under a chip is not. Each takes a
  `variation` parameter, which is a domain shift rather than a seed: a node's
  `seed` is a field of the node and not an input, so what a parameter can reach
  is a `Warp` over the bound outputs, and a constant displacement of the
  sampling frame is another piece of the same endless wall. The writer bakes all
  five graphs to KTX2, the corporate block bakes three of them at registration —
  the tower, the annex and the wing are one plaster graph at three values of one
  parameter, with the `BakeCache` handing out three texture sets over a hundred
  and more instances and no file for any of them — and the outpost's repaired
  bay is clad in the brick. New **material sheet scenes**, one per graph in the
  library, put each material on a ball and a cube at three texel densities in
  `just references`: the gallery frames a scene to its own bounds, so a row laid
  out in depth can only span two to one in distance, and one, a third and a
  ninth of the tiling is the same number as one, three and nine times the
  distance with the specimen still a size somebody can look at. What the sheet
  says, and what still looks wrong, is in
  `docs/research/material-sheet` (removed; in git history).
- **The study's concrete gained a horizon and lost its clipping.** Its occlusion
  is `OcclusionFromHeight` over the panel's own relief now, not the `1 - pores *
  0.3` the pixel loop wrote — the one number in the phase-one port that was a
  stand-in rather than a translation, and the reason a joint half a centimetre
  deep cast no shadow into its own corner. The filter reads height as a length
  in UV, so what it is handed is the height scaled by the same relief the normal
  is derived with. And the height is bedded onto the whole of `0..=1` by a
  `Levels` with the same factor divided back out of the normal strength, because
  its relief ran from -0.216 to 0.370 and the sixteen-bit map was clamping the
  panel joint flat while the `f32` normal was not. A new test reads
  `BakeReport::height_range` for every shipped graph and fails on a relief that
  leaves the interval, and another holds every graph's lattice inside the 512 the
  corporate kit bakes at. The concrete's occlusion, normal and height maps moved
  and its golden hashes moved with them; its base colour did not.

- **The study's surfaces are graphs.** `examples/showcase/src/materials.rs`
  carries the concrete and the metal as `ashlar-material` graphs — the port of
  the pixel loops that were in `textures.rs`, node for node and number for
  number — and the asset writer bakes them at 1024 instead of looping over
  texels. The graph library is written beside the material library as
  `assets/buildings/recipe-study/materials.graphs.ron`, and every textured
  surface records the graph, parameters and resolution it came from under
  `baked_from`, so a map can be traced to what wrote it and the step re-run.
  Two things moved in the port and are documented where they moved: the pixel
  loop's normal strength was per two texels at 1024 and a graph's is per UV
  unit, and the loop's colour constants were display-referred where a graph's
  `base_color` is linear, so the study's tone is computed as the loop computed
  it and then decoded through the sRGB transfer the bake's encoder puts back.
  The maps are within an eight-bit code of the ones the loop wrote, the
  remaining difference being that a bake samples texel centres where the loop
  sampled corners. The golden hash of every plane is recorded in
  `examples/showcase/tests/materials.rs` so the interpreter cannot drift
  silently, and the numeric seam check that lived in `textures.rs` is now in
  `ashlar-material`'s own `tests/periods.rs`, at the study's own periods. The
  facade and the entrance rendered from the graph-baked maps, the two graphs as
  RON and what a bake costs are in
  `docs/research/material-graph` (removed; in git history).
- **The shader partition** in `ashlar-material`: `partition::partition` takes a
  built material, a `Target::Shader` and the resolution its textures would be
  baked at, and answers what a fragment has to compute and what a texture can
  hold. Every value in the lowered expression is coloured *runtime* where it
  depends on a live parameter or on one of the four inputs a texture cannot
  hold, and *static* otherwise; the cut is at the frontier, and every maximal
  static sub-expression a runtime op reads becomes a bound texture rooted in the
  partition's own IR, so `planes::rasterise_buffers` bakes the static half with
  nothing new written and the WGSL emitter prints the runtime half over the same
  op set the interpreter runs. An output that reaches nothing runtime is a
  texture in its own right, sampled straight into its PBR slot, and the
  material's normal — which the bake derives from height outside the graph —
  comes back as a root of its own: a bound normal map over a static height,
  four offset taps at the bake's own span over a live one, and nothing at all
  where `normal_strength` is zero. A value built only of constants and the
  coordinate is written into the shader rather than bound, which is also what
  stops the cut from binding a texture for `Op::Uv` and sampling it at itself.
  `CostReport` is the number an author acts on — runtime ops per output, texture
  reads, bound textures, planes, live uniforms, the neighbourhood taps and their
  ceiling — and it says `no live inputs: bake this` when the fragment computes
  nothing, which is what all five study graphs say with their parameters baked.
  With `variation` live instead, `study:brick` is 2683 ops per fragment against
  1189 baked, because a `Warp` over the bound outputs re-emits every instruction
  that depends on the coordinate and a live height is differenced four times
  over: the design's "a parameter that feeds everything costs everything", as a
  number, before it ships. A parameter that reaches a blur, an occlusion, a
  distance or a morphology cannot move per frame — those are a plane or they are
  nothing — so it is *frozen*, folded at its value the way a bake folds it and
  named in the report, rather than the graph being refused or the slider
  silently doing nothing; `study:plaster`'s `cracking` and
  `study:painted-metal`'s `chipping` are both of that kind. A runtime input above
  such a filter has nothing to fold to and is refused by node path. A material
  may bind eight textures, with scalars packed four to an RGBA image above that
  and a refusal by path above that again; a packed image is half floats whatever
  ports its lanes serve, because a format belongs to an image rather than to a
  plane and a `height` asking for sixteen-bit codes cannot share one with a
  `roughness` asking for eight unorm bits. A plane that only feeds another plane
  is rasterised and dropped, and costs no binding. Checked by rasterising the
  partition's own planes and running its IR through the interpreter: a bound
  texture answers at a texel centre exactly what the sub-expression it replaced
  answers in a bake, and the four taps answer what the baked normal plane does.
- **Four runtime input nodes** in `ashlar-material`: `Time`, `WorldPos`,
  `WorldNormal` and `CutFlag`, which lower to the `Op`s the IR has carried since
  phase one and which the node table did not expose. All four are zero in a
  bake, because a bake has no frame and no mesh, and all four are
  period-neutral rather than free: a period says how many times a field comes
  back to itself across UV, and these are not fields over UV at all — at one
  instant, at one fragment, each is the same number across the whole repeat,
  exactly as a parameter is. Calling them free would refuse every graph that
  read one, on the strength of a question they are not answers to.
- **The WGSL emitter** in `ashlar-material`: `wgsl::emit` prints a runtime
  partition as one `let` per instruction inside a function that takes the
  coordinate and the four runtime inputs and answers the PBR ports the graph
  bound, and `wgsl::emit_compute` prints the same function as a
  `@workgroup_size(8, 8)` kernel over the texel centres of a square, which is
  what the conformance test dispatches and what a GPU bake would. Around it: the
  `AshlarParams` uniform block at binding 100, laid out the way WGSL lays one
  out — a scalar aligned to four, a colour to sixteen, consecutive floats packed
  four to a row, the order the graph declares its parameters in, the block
  rounded up to sixteen, and a `ParamLayout` per parameter so the Bevy step
  fills bytes without re-deriving any of it; one `texture_2d<f32>` and `sampler`
  pair per bound image from binding 101; a `MaterialExtension` fragment over
  `pbr_input_from_standard_material` that *multiplies* into the PBR slots the
  way the same material's baked maps would and writes its tangent-space normal
  through `calculate_tbn_mikktspace`, under the `VERTEX_TANGENTS` def that
  declares the tangent and leaving the geometric normal alone without it; and a
  prepass fragment writing the same normal so screen-space occlusion and the
  deferred path see the relief. Every
  op is the arithmetic the interpreter does rather than the WGSL built-in that
  is nearly it — `mix` is `a + (b - a) * t`, `clamp` is `min(max(x, low), high)`,
  and division, `smoothstep` and `normalize` go through helpers that keep the
  guarded case each has — and the lattice hash is the same integer
  multiply-xorshift over the same modulo-period reduction, in `u32`, whose
  arithmetic WGSL defines as wrapping. Validated with `naga` at the version Bevy
  0.19.1 pins, as a dev-dependency with only `wgsl-in`: every node of the
  vocabulary in isolation with a live parameter forcing it into the fragment,
  every study graph with every parameter live and with none, and the compute
  kernel. What is validated is the generated core — the block, the bindings, the
  helpers and the SSA function, with the bind group written as a literal and a
  stub entry point appended; the Bevy-flavoured wrapper around it carries
  `#import` lines and `#{MATERIAL_BIND_GROUP}`, which are `naga_oil` directives
  that plain `naga` does not parse, and is checked by the Bevy step compiling
  one. Compiling one found the one thing that had to change afterwards: the
  clock lives in two different bind groups, binding 11 of `mesh_view_bindings`
  in the main pass and binding 1 of the prepass's own smaller view layout, so an
  emitted prepass that reads `Time` declares `ashlar_globals` there itself
  rather than importing the main pass's name.
- **`ProceduralMaterial`** in `ashlar-bevy`: a `ashlar::Surface::Shader`
  definition now compiles rather than being refused.
  `shader::create_shader_material` partitions the graph, bakes the static half into
  `Image` assets through the phase-two `runtime_bake::BakeCache` under an exact
  `ShaderKey` — graph, resolution, live names, and the value of every parameter
  that was folded — emits both stages as WGSL, registers them as `Shader` assets
  under ids derived from the graph, and answers
  `ExtendedMaterial<StandardMaterial, GraphExtension>`. `tile_metres` and
  `uv_offset` go through `uv_transform` exactly as `create` and `create_graph_material`
  put them there, so a wall does not change how it tiles when its surface moves
  between files, a bake and a shader.
  `MaterialExtension::fragment_shader` cannot vary per graph, so `GraphExtension`
  carries a `GraphKey` as its `bind_group_data` — Bevy makes that part of the
  pipeline cache key and hands it to `specialize`, which swaps the descriptor's
  fragment shader for the weak handle whose UUID the key derives, choosing the
  prepass text when the `PREPASS_PIPELINE` shader-def is present and doing
  nothing where there is no fragment stage at all. `GraphKey` is a sixty-four-bit
  digest because Bevy's bind group data has to be `Copy`; `GraphShaders` keeps
  the exact `ShaderKey` beside every entry, so a collision is a startup error
  naming both graphs rather than one wall wearing another's shader. The bind
  group is fixed by the type: a 256-byte uniform block at binding 100 and eight
  texture-and-sampler pairs from 101, which is the number the partition refuses
  above. `shader::ProceduralMaterialPlugin` registers the material, its pipeline
  and its prepass. `read` preflights a `Shader` surface by lowering,
  partitioning and printing it, so everything but the WGSL itself is a startup
  error; validating the WGSL needs `naga`, which is a dev-dependency of
  `ashlar-material` on purpose.
- **`bake::encode_bound`** in `ashlar-material`: one arbitrary plane quantised
  into an image with its whole mip chain, using the bake's own ordered dither,
  sRGB transfer, sixteen-bit height codes, half floats and wrapped box filter,
  with a normal written half and half about zero and renormalised at every
  level. This is what makes a compiled material's bound textures the same bytes
  the same graph's baked maps are.
- **`MaterialGraph::with_params`** in `ashlar-material`: the graph with a
  caller's parameter values in place of its declared defaults, refusing an
  unknown name or a wrong type at `params[name]`. The bake already did this
  privately; the shader partition needs the same step, so it is public and
  written once.
- **The conformance test** in `ashlar-bevy`: `tests/conformance.rs`, run by
  `just conformance`, is the contract between the two backends and the reason
  the IR exists. Every test in it that dispatches one is `#[ignore]`d because
  it **needs a GPU**: it
  opens a headless device through Bevy's own `initialize_renderer` — no window,
  no `App`, and no new dependency, since this crate already links the `wgpu`
  Bevy pins — dispatches the compute kernel `wgsl::emit_compute` writes, and
  compares it with `interp` over the same partition, texel for texel at 256
  squared. One partition per primitive `Op`, each a static field cut into a
  bound texture with a live parameter over it, with a coverage test — the one
  that needs no adapter and so runs in `just ci` — that fails when an op has no
  case; a row for the one *layout* a table of ops cannot reach, ten scalar cuts
  packed four to an image, where every sample has to read the lane it landed in;
  every study graph with every parameter live; and the
  lattice hash against the emitted helper itself, which has to agree bit for
  bit rather than within a tolerance. The bound textures are uploaded as
  `Rgba32Float` holding the CPU planes' own numbers, so the runtime half is
  what is under test and not the bake's quantisation. Budgets: one eight-bit
  step for a colour, two for a normal, one part in ten thousand for a raw
  scalar plane. Worst observed on an RTX 4090 under Vulkan: four parts in a
  million, and the hash exactly zero.
- **Two live materials in the showcase**, which is what the whole of phase 3 was
  for. `study:strip` is a light strip whose emissive is
  `diffuser * (0.75 + 0.25 * sin(time * rate))` — the first graph here that a
  texture cannot hold, since an image has no axis to store time along — and it
  is bound to the study's `light` key as a `Surface::Shader`. Its `rate` is a
  `Bake` parameter on purpose: how fast a light beats is a decision about what
  the light is, and the phase is the clock rather than a parameter. It compiles
  to fourteen instructions over three bound textures; the diffuser's noises are
  baked. `study:concrete` gained a live `wetness` that collects water in its own
  height field's low places, darkens the albedo towards a third of itself and
  drops the roughness to near a varnish; the study library ships it twice, as
  the `Files` surface every wall uses and as `study:concrete-wet`, a
  `Surface::Shader` over the same graph with `wetness` bound, which the
  entrance's coping and threshold carry. Eleven instructions and one uniform.
  At `wetness` zero the graph is the wall it always was, bit for bit — `Op::Mix`
  is `a + (b - a) * t` — so the shipped KTX2 files and the golden hashes are
  unchanged by the parameter existing. See
  `docs/research/shader-materials` (removed; in git history).
- **The material sheet is baked beside live**: every column of a sheet is now a
  pair of specimens, the maps a bake wrote and the graph they were written from
  compiled, so "a graph with no live parameter is the same picture either way"
  is a claim somebody can check by looking. `sheet::DELIVERY` is the second axis
  and `sheet::material_key` takes it. `tools/ashlar-preview/tests/sheet.rs`
  checks it as bytes rather than as pixels: a compiled material's bound
  `base_color` and `normal` images are byte for byte the maps the bake wrote,
  whole mip chain included.
- **The preview drives live parameters without a rebake.** `reload::redress`
  writes a new uniform block over every compiled material's, and the panel asks
  for it on every step of a slider drag rather than on release, because a live
  parameter rasterises no texel, emits no WGSL and rebuilds no pipeline. A
  definition that pinned a live parameter keeps the value it pinned: the panel
  edits graphs, and a definition that named a value has chosen one.
  `panel::graph_of` answers for a `Shader` surface now, and the cost report the
  preview prints when one loads carries the wall-clock time of the compile
  beside it.
- **A GPU bake, through the same emitter.** `ashlar-bevy::gpu` bakes a graph on
  the render device: `partition::over_planes` is the shader partition turned
  inside out — nothing cut, the whole expression on the runtime side, and every
  plane's root named as a port of its own through `Ir::rooted_at` — and
  `wgsl::emit_compute_ports` writes one kernel per port over only the images
  that port samples. A bake is then a dispatch per plane with `planes::filter`
  run on the CPU between them, a dispatch for the outputs, and the normal, the
  mip chain, the Toksvig widening and the encoding still on the CPU over the
  read-back `f32` planes, which is what keeps a GPU bake's normal identical to a
  CPU bake's rather than merely close. `Baker::{Cpu, Gpu}` on `BakeContext` and
  `ShaderContext` says which backend runs; `gpu::GpuBakePlugin` puts a
  `GpuBaker` in the world wherever the app has a device, and the preview's
  runtime bakes and its panel's re-bake take it. The CPU bake stays the
  reference: it writes the shipped bytes and needs no adapter, and
  `--write-study-assets` only leaves it for `--gpu`. Measured on an RTX 4090 in
  a debug build: `just references` 65.1 s to 19.6 s, the study's painted metal
  at 1024 from 3.11 s to 1.08 s and its plaster from 2.29 s to 1.03 s, with a
  small graph slightly *slower* because a dispatch costs a shader compile
  whatever it evaluates. `just conformance` grew
  `the_gpu_bake_is_the_cpu_bake_within_the_conformance_budget`, which compares
  the two bakes of every study graph at 512 as encoded bytes, map by map and
  level by level: worst difference one eight-bit code.
- **World-space inputs, and a material that decides for itself where it is
  wet.** `WorldPos` and `WorldNormal` were emitted and conformance-tested with
  nothing bound to them; `ashlar-material` now gives them something to do and
  gives a bake an answer about them. `WorldMask { field, axis, threshold,
  softness }` takes one axis of the world normal or the world position through a
  soft threshold — `WorldMask::up()` is the faces rain lands on, and
  `WorldMask::below(6.0)` the faces under six metres — and `Triplanar { source,
  tile_metres, sharpness }` reads a tiled source three times by world position,
  once on each axis plane, blended by the normal, for a surface that must ignore
  UV seams; it is a resampler, so its source is emitted three times and the cost
  report counts all three, and its source must *tile*, checked by path, because
  the field it lays repeats every `tile_metres` along each axis. All of them are
  period-neutral: they are not fields over UV, so the question a period answers
  does not apply to them. **A bake now refuses a world-space input** by the path
  of the node that asked, with a message naming the `Shader` surface that does
  answer — zero seconds is a picture and a world position of zero is a whole
  wall at one point, so `Time` and `CutFlag` still bake at zero and these two do
  not bake at all. `wgsl::emit_compute_world` emits a kernel that reads a world
  position and a world normal **per texel** from two storage buffers, because a
  triplanar at one fixed position is one sample of its source repeated across the
  square and two backends that disagreed everywhere would compare equal;
  `just conformance` uses it for four new cases and for the study's own
  world-space graph. The study ships that graph: `study:concrete-wet` instances
  `study:concrete` and multiplies its live `wetness` by an up-facing mask and a
  near-ground one, so the coping of every storey of the office stops being a wet
  band across the facade while a doorway threshold stays exactly as wet as it
  was — eleven runtime instructions become twenty-five, with the bound textures
  unmoved. The preview also learned where the ground is: it recentred a model on
  its own bounds, which put a four-metre bay's floor two metres underground, and
  it now puts the model's feet at zero and lets the camera make up the
  difference, so world height is height above the ground in every scene.
- **The cut flag as a vertex attribute, and a material that dresses its own
  reveals.** `ashlar::TriangleMesh` has carried one flag per triangle since the
  kernel went in, and the only thing a building could do with it was
  `Element::cut_material`, a second mesh and a second material key.
  `ashlar-bevy::mesh` now uploads it as `ATTRIBUTE_ASHLAR_CUT`, one flag per
  vertex, on any surface that has a cut face — and that attribute *is* Bevy's
  second UV set, because a `MaterialExtension` draws through Bevy's own vertex
  stage into a `VertexOutput` with no free location, and the alternative to
  repurposing a slot is emitting a vertex shader that reimplements `mesh.wgsl`
  and `prepass.wgsl` to carry one float. Vertex colour was the other free slot
  and is worse: `pbr_fragment` assigns it over the material's base colour rather
  than multiplying. A mesh with no cut face carries no attribute and the
  generated fragment reads zero under an `#ifdef`, which is the same answer.
  Because the flag is per triangle and an attribute is per vertex,
  **`TriangleMesh::weld` now treats the edge of a cut as a seam**, as it already
  treats a crease and a UV seam: splitting rather than taking a maximum, because
  an interpolated maximum is a ramp across the uncut face instead of a boundary.
  A bake still answers zero for the flag — one texture set serves both kinds of
  face — and `BakeReport::cut_flag` says when it did. `wgsl::WorldInputs` is
  renamed `MeshInputs` and `emit_compute_world` to `emit_compute_mesh`, because
  `Planes` now lays a cut flag over the square beside the world position and
  normal; `just conformance` gains a row that reads it out of that plane in
  blocks, so both arms of a mix and every boundary between them are compared.
  The study ships `study:concrete-cut-aware`: four `Subgraph` nodes on
  `study:concrete`, a `CutFlag` and two `Mix`es, nine runtime instructions and
  no uniform at all. The facade bay wears it and drops its cut slot; the
  entrance bay keeps the slot, because a reveal that really is a different
  material still wants one. The two pictures differ by at most seven codes of
  255, over exactly the pixels the flag reached, and `just references` costs
  4 s more over 39 shots — all of it the static half of a second compiled
  material being baked once per scene, because `BakeCache` keys a bound image by
  the graph that asked for it rather than by the plane behind it.
- **Per-instance parameter overrides, and one material key where there were
  three.** A slot has always bound a material key, and an instance has always
  been able to override which key; what a graph surface adds is a reason to want
  *nearly* the same one, since three buildings of a block are one render at three
  values of one parameter. So a binding is now `ashlar::Binding` — a key, and
  optionally values of its own for the graph parameters of the surface behind it
  — and `Instance::materials` and a recipe's palette hold those. Its RON is two
  forms in one field: the bare `"metro:stone"` where nothing is overridden, which
  is what every recipe written before this holds and what such a binding still
  writes itself back as, and `(material: "metro:stone", params: {"variation":
  Float(0.37)})` where something is. Validation is in the two places that can see
  the answer: `MaterialLibrary::check_for` refuses an override on a `Plain` or
  `Files` surface, which binds no parameters at all, and `ashlar_bevy::read_library`
  refuses a name the *graph* does not declare or declares at another type, both
  at `instances[<id>].materials[<slot>]`. `ashlar_bevy::definition` writes a
  binding's values into the definition it names, so nothing below it learns that
  an override happened and the two caches key what they always keyed: a `Graph`
  override is its own `BakeKey` and its own texture set, shared by every instance
  that asked for the same one; a `Shader` override of a *live* parameter is the
  same pipeline and the same bound images with a different uniform block; of a
  folded one it is a second compiled graph. What an override cannot reach is the
  definition's own constants, so a per-instance tint is still a second key. The
  corporate block is the worked example: `metro:stone-annex` and
  `metro:stone-wing` are gone from `assets/buildings/corporate/materials.ron`,
  all three buildings bind `metro:stone`, and each puts its own `variation` on
  its own bays — three texture sets over a hundred and more instances, asserted
  as such, and the block renders within five codes of 255 of what it did, all of
  it the two base-colour multipliers those two keys also carried.
- **The third node set, and the three materials re-authored over it.**
  `ashlar-material` gains the nodes a worn surface is actually made of:
  `Blur`'s directional kind (the same Gaussian along one angle, which is what
  makes a brushed metal out of a grain) and its **slope** kind (each texel
  walked `steps` times down the gradient of a height input and the source
  averaged at every stop, so rust creeps out of a chip and dust out of a joint);
  `DirectionalWarp`, which carries a field along an angle field by a fixed
  distance rather than by a vector, so a crack bends instead of pinching;
  `Curvature` from a height, as the signed field or as the `Peaks` and `Cavity`
  masks of one plane, by four taps or by the difference of the height and its
  own blur; `EdgeDetect`, a difference over a radius so a mask's edge is one
  whatever the radius; `HeightToMask { low, high, softness }`; and a per-instance
  `mask` on `Tile`, read once at each instance's cell centre and resolved
  against that instance's own hash, so a masked scatter thins out rather than
  dissolving. There is deliberately no `GradientMap`: `Colorize` is that node
  and its gradient already takes any number of stops, and the other reading of
  the name wants an image input this vocabulary does not have. A slope blur is
  also the first filter to read *two* planes, so a `Filter` may now name a
  guide `BufferId` — followed by the dead-plane sweep, the renumbering, the
  plane cache key and the partition alike — and `planes::filter` takes the
  planes before the one it is filtering. The study's brick, plaster and painted
  metal are re-authored over all of it: brickwork whose arrises are chipped
  where `EdgeDetect` found them and whose mortar dust is smeared up the course
  above by a slope blur, with grime and bloom read off the relief by
  `Curvature`; a render whose crazing *runs* under a `DirectionalWarp` and whose
  spalled patches carry the chalky lip their own edge draws; and paint that
  fails over the rims of the panel's dents because that is where `Curvature`
  says the film was already lifting. Their KTX2 sets, their golden hashes and
  the `materials.graphs.ron` the study ships are regenerated.
- **Zstd supercompression in the KTX2 writer, and `assets/` down from 94 MB to
  24 MB.** `ktx2::write_with` takes a `Supercompression` beside the map and the
  resolution, and `Supercompression::Zstd` compresses each level into a frame of
  its own under scheme 2, recording the uncompressed length beside the stored
  one and packing the levels with the one-byte alignment the specification gives
  a stored level that is not texels. Nothing else about the file moves: the
  format, the dimensions, the data format descriptor and the `KTXwriter` key are
  written exactly as before, and `ktx2::write` is `write_with` with no scheme.
  The shipped study maps are rewritten under it — every one of the twenty decodes
  to the bytes it decoded to before, checked level for level — and the numbers
  are per map rather than uniform: the 8-bit planes come back at 2% to 26% of
  their bytes and the 16-bit heights at 81% to 95%, because the low byte of a
  quantised noise field is very nearly random. The encoder is the C library and
  is behind `ashlar-material`'s new `zstd` feature, off by default, which the
  showcase's asset writer turns on and nothing that reads a map does; the
  decoder is `ruzstd`, pure Rust and decode-only, which `ashlar-bevy` gets by
  building Bevy with `zstd_rust`. `ktx2::inspect` reads a supercompressed file's
  claims — checking the *uncompressed* length against the size each level's own
  dimensions imply, which is what a reader with no decoder can check — and
  `ashlar-bevy`'s preflight now refuses every scheme except zstd by number
  rather than refusing all of them. `just references` renders the gallery from
  the compressed files unchanged, pixel for pixel.
- **Phase 4's capture**, in
  `docs/research/materials-depth` (removed; in git history): the
  six entries above with their measurements, and the three items the phase left
  undone with the reason for each. The bake timed both ways at 1024; the three
  re-authored maps beside the maps they replaced, and the material sheet that
  barely shows the difference between them; the wet concrete and the cut-aware
  concrete, whose own pictures stay in
  shader materials; the corporate
  block's three walls out of one key; and the twenty files at a quarter of their
  bytes. The plan's own phase 4 list is marked to match.
- **Subgraphs that take signals, and graphs that export masks.** A library graph
  in `ashlar-material` now declares what it takes: `GraphInput { name,
  value_type, default }` is a node like any other, so validation, the
  topological order, period inference and lowering all reach an input without
  being taught about a second kind of table, and a compound with nothing wired
  into it still builds, checks, bakes and previews — it is its defaults, which
  is the picture its author chose them for. `Subgraph` gained `inputs` beside
  `params`, with a `.input(name, value)` builder that takes anything a port
  takes, a node written inline included. The two bind different things at
  different times: a parameter is a number, folded when the instance is lowered
  and never live, while an input is a whole field that carries a type, a period
  and a lattice across the boundary. Carrying them is why the instanced graph is
  now inferred *again* under what was wired in — an input of period 8 read
  through an inner `Transform` of scale 2 lays 16 cells, and an instance built
  once per key could only ever claim 8 — so an instance is built and cached per
  binding signature, keyed by the graph key plus the type, period and lattice of
  every bound field and written `key@<hash>` in a path when something is bound.
  Two nodes that bind the same fields share one instance and one inlined copy;
  two that bind different fields are two graphs and cost two. Recursion is still
  refused by key alone, whatever is bound, because a graph that includes itself
  is a cycle however it is specialised. `PbrOutput::extra` is the other half:
  named float masks beside the six PBR channels, read by
  `SurfaceOutput::Extra(name)`, held to the same tiling rule and counted into
  the same repeat, because a mask that does not meet itself at the seam is as
  wrong as a colour that does not. A bake writes no map for one and a shader
  binds nothing for one — they are roots only inside an inlined instance, so the
  texture set and the `BakeReport` of a graph with an extra are the set and the
  report of the same graph without it. They exist so that a compound which
  decided something, where the wear took or where the water sat, can hand that
  decision back instead of making its caller build it a second time and get an
  answer a texel off. Two smaller things moved to make room: a port name is a
  `Cow<'static, str>` rather than a `&'static str`, which is what lets a node
  declare its ports at runtime, and `SurfaceOutput` lost its `Copy` for the
  string an extra's name is. Everything an author can get wrong is refused by
  path, with what the graph does declare: an unknown input name, a type the
  declaration does not admit under the two conversions, two inputs sharing a
  name, an extra named after a PBR channel, an `Extra` the graph does not
  export, and a field that does not tile bound into a compound whose output
  reaches it — that last one at the *outer* node, with the inner path in the
  message, because the node that bound it is the only place its author can fix
  it.
- **A conformance probe per new node.** `ashlar-bevy/tests/conformance.rs`
  carried one partition per primitive `Op` and every study graph; between them
  those reach every arm the two backends implement, but they say nothing about a
  node that *composes* those arms — and a lowering can lean on arithmetic that
  only parts once the numbers are real, which is what a division whose divisor
  reaches a seam, a `mix` at a weight of exactly one and a `pow` of a base that
  goes negative do. So `node_cases()` joins `op_cases()`: a probe per node and
  per new output this phase added — a twisted `CircleMap` over a hole, a masked
  `CircleSplatter` with every variation, a hollow `Gear` as a distance, a
  rounded `Box` beside a hollow `Capsule`, an `SdfCombine` hard and filleted, an
  `SdfMask`, and a plain, a twill and a satin `Weave` — each the `tests/wgsl.rs`
  graph for that node, a warp over a live parameter, which makes the whole probe
  runtime. `every_new_node_agrees_with_the_interpreter` dispatches them under
  `just conformance`; a second test that needs no adapter, and so runs in
  `just ci`, asserts the partition cut none of them into a bound texture, since
  a probe that quietly became a texture read would pass saying nothing. The
  worst any of them differs by on the reference machine is eight parts in a
  million, in the splatter, whose per-instance turn is a sine and a cosine —
  which is one of the two places the specifications let the backends differ at
  all. The observed maximum in the README and the design doc moved from four
  parts in a million to that.
- **`Weave`**, the one genuinely new generator of this phase: woven cloth, with
  `x` warp threads running along v and `y` weft threads along u, a `width`
  saying how much of its own pitch a thread fills, and a `Plain`, `Twill` or
  `Satin` crossing. The three patterns are one rule at different numbers — the
  warp is on top where the residue of `warp - move * weft - offset` modulo the
  repeat falls under the float — so a twill is a float of half its repeat
  marching a thread a weft and a satin a float of all but one thread whose
  single binding point moves by a step coprime with the repeat. That move is
  chosen from the repeat rather than authored, because a move that shares a
  factor with it binds some threads twice and others never; a repeat that
  admits no move at all is four threads and six, and both are refused rather
  than laid as the twill they would come out as. The period is the thread
  counts divided by the pattern's repeat, and the counts must therefore divide
  it: a plain weave of seven threads meets its own opposite crossing at the
  seam, which is a fault running the length of the cloth, so it is refused at
  `x` the way a brick bond that does not divide its rows is refused at
  `offset`. `Height` is one relief model rather than a field per output: each
  thread is a half-cosine ridge across its own width, lifted where it passes
  over another and pushed down where it passes under, *in proportion to the
  crossing thread's own cross-section*. That proportion is the whole of why the
  relief is continuous where the pattern changes — the pattern changes at the
  middle of the gap between two threads, where the cross-section carrying the
  lift has already fallen to zero, so both sides read the thread at its own
  plain crest and agree. Beside it `Mask` is the cloth against its holes,
  `Warp` and `Weft` are the topmost thread and are exactly each other's
  complement over the cloth, which makes them a pair of tint masks, and `Id` is
  a hash per thread rather than per crossing, so a tint runs the length of a
  thread.
- **`SdfCombine` and `SdfMask`**, the boolean and the ramp that go with a
  shape's new distance output. A combine puts two signed distance fields
  through a union, an intersection or a subtraction — `min`, `max` and
  `max(a, -b)`, because a field is negative inside its shape — and answers a
  distance again, so booleans chain and a mask over the last of them is what
  turns the result into a picture. `smooth` is a fillet width in UV: at zero the
  node emits the hard boolean and nothing else, bit for bit, because the
  polynomial smooth minimum divides by that width and the crate's guarded
  division would answer the *average* of the two fields rather than an infinity;
  anything more rounds the meeting of the two over a band that wide, which is a
  weld rather than a joint and a boss growing out of a plate rather than sitting
  on it. The fillet is written as one expression with two signs rather than as a
  smooth minimum and a smooth maximum, so a union and an intersection cannot
  round by different amounts. `SdfMask` is the inward ramp a `Shape` has always
  put its own distance through, factored into a node so the two cannot drift: a
  shape read as a mask and the same shape read as a distance and masked
  afterwards are one instruction list, and a test pins the hash and the texels.
  That identity is the whole point — it is what makes a hole cut in a plate
  readable as a shape rather than as a second thing that looks like one.
- **`Shape` grows**: two kinds, two shaping fields and a second output, with
  every configuration an author could already write lowering to the same
  instruction list it always did — the mask is unchanged bit for bit, and a test
  pins the hash of the lowered expression rather than a picture of it. The new
  kinds are a **capsule**, the segment of `length` lying along u with a cap of
  radius `size` at each end, and a **gear**, a disc with `sides` teeth cut
  `depth` deep into it. A capsule is the one kind whose reach is not `size`, so
  the rule that keeps a shape off the seam is read against half its length too
  and refused at `.length`; a gear's teeth are cut inward from `size`, so the
  radius it promised still bounds it, and what it answers is a *radial* distance
  rather than a Euclidean one, which the node says plainly because it widens the
  edge ramp on a tooth's flank. `round` is the usual corner radius — the shape
  drawn smaller and the distance moved back out — which rounds a box, a polygon
  and a star, folds away on the three kinds with no corner, and makes a rounded
  box take the exact distance to a square instead of the Chebyshev one, that
  being flat across a whole quadrant outside a corner and so having nothing to
  round. `hollow` is the onion: the band from the boundary to that far inside
  it, an annulus of a circle and a frame of a box, taken inward so a hollow
  shape reaches exactly as far as the solid one the check already bounded.
  `output` adds the **signed distance** beside the mask, unclamped and untouched
  by `edge`, which is the field a boolean between two shapes will combine and
  the reason the mask is a ramp over a distance rather than a shape drawn twice.
- **`CircleSplatter`**, the ring scatter: a source laid round one or more
  concentric rings inside the repeat, which is what draws a bolt circle, a ring
  of rivets, the ticks of a dial or the holes of a drain. It is a `Tile` on a
  circle rather than on a grid, and deliberately the same tool otherwise — an
  instance is the source's own unit drawn `scale` across, varied by hashes of
  its own place in the ring, windowed to that unit and combined with its
  neighbours by taking the larger, and the `mask` is read once per instance at
  that instance's own centre against its own hash, so a half mask keeps about
  half the bolts whole rather than dimming all of them. The rings stand at the
  outer edge of the bands they cut `inner` to `radius` into, so the outermost is
  at `radius` whatever `rings` is, and one check — `radius` plus what an
  instance reaches is at most a half — keeps every one of them off the seam,
  which is what lets the node tile once whatever it scattered. What an instance
  reaches is half its width while it stands square to the repeat and its
  half-diagonal once `face_centre` or a rotation variation turns it, because
  what is drawn is the source's unit square and a turned square presents a
  corner; a ring that fits square is therefore not always a ring that fits
  facing. Every variation of an instance's size or place takes away rather than
  adds, for the reason a scatter's does: the check is made once against the
  fields as written, and a variation that could grow an instance past it would
  make it a bound on nothing. A rotation is the one that takes nothing away,
  and so the one the check itself reads. The cost is stated rather than
  hidden: a texel draws the instance nearest it in angle and the one either side
  of it, so the source is evaluated three times per ring.
- **`CircleMap`**, the polar resampler: a source read round a disc centred in the
  repeat, its u the angle and its v the normalised radius, with `turns` and
  `rings` copies across the two and a `twist` that carries the source round by so
  many turns per unit of radius. It is *windowed* rather than a plain polar
  frame, and that is the correction rather than a limitation: a polar frame over
  the whole repeat answers a different angle at `u = 0` than at `u = 1`, so it
  never meets itself at the seam and cannot tile at all. A disc no wider than the
  repeat with a fill outside it does, which is the rule `Shape` has always lived
  under, so the radius stops at a half, the coordinate is wrapped before the disc
  is measured, and everything past either radius answers `outside`. The source
  must itself tile, the way a `Triplanar`'s must, because the angular axis wraps
  into the source's u. The centre pinches — every angle meets there — so a source
  whose `v = 0` row is not constant shows a knot at the middle, and the node's
  own documentation says to read a gradient there and put the noise further out.
- **A standard library**, `ashlar_material::stdlib`: the first graphs this crate
  ships rather than reads. Five weathering compounds — `weathering:edge_wear`,
  `dirt_dust`, `rust`, `peeling_paint` and `moisture` — and four pattern
  recipes — `patterns:radial_gradient`, `angular_gradient`, `wood` and `cracks`
  — answered together by `stdlib::graphs()` and merged into a game's own library by the
  new `MaterialGraphLibrary::extend`, which is two passes so that a refused
  merge leaves the library untouched and a collision errors at `graphs[<key>]`.
  None of them is a node, and that is the point: a wall rained on for thirty
  years is a dozen primitives wired the way the study's brick wires its own, so
  what is worth shipping is the wiring, and shipping it as a graph means the
  crate's own tests hold it to the same tiling, lattice, lowering and WGSL rules
  as anything an author writes. A compound takes the substrate channels it
  intends to change as `GraphInput`s whose defaults bake a plausible picture
  with nothing wired in, exposes the author's decisions as parameters that fold,
  writes back the channels it names and no others — `dirt_dust` binds no height,
  because a film of dust is microns on a wall whose relief is centimetres, and
  reading one off it is refused rather than silently passed through — and
  exports the mask it decided with as an extra, so a caller putting its own dirt
  in the same hollows layers on that decision instead of building a second one
  that agrees to within a texel. Three of them take a *place* rather than an
  amount, which is the honest shape of what they model: rust is measured out of
  a `seed_mask` by a `Distance` plane, a peel's lip and undercut are measured off
  the edge of a `peel_mask`, and a waterline is a contour on the caller's own
  height. Their defaults are the terminal state — a plate rusted through rather
  than a wall with a patch on it — because a constant input cannot draw a
  boundary, and the boundary is exactly what the caller wires. No compound reads
  a runtime input: a bake refuses `WorldMask` by path, so world-space bias is a
  plain float input the caller drives from `WorldMask::up()`, a live parameter or
  a constant, and `moisture` is built for the live case — pointwise throughout,
  no buffered filter, so a live `wet` freezes nothing and costs forty-four
  fragment instructions. Two conventions run through all nine: `amount` is
  always the same contour, `clamp((field - (1 - amount)) / softness, 0, 1)`, over
  whatever field that compound decided with; and every compound that reads a
  *neighbourhood* of the height takes a `relief` parameter, because a curvature
  and an occlusion read their input as a length in the units of UV while a
  height input is a `0..=1` field, which is also what makes a compound wear a
  shallow surface less than a deep one. What could not be exposed was not
  exposed: a filter radius, a noise seed, an octave count, a generator's period
  and a `Distance`'s range are node *fields*, and a parameter reaches only a
  port, so those are documented constants and a boundary that had to move is read
  as a contour off a plane rasterised once out to a constant reach — which makes
  that constant the parameter's ceiling, since past it the plane saturates.
  `MaterialGraphBuilder::layer(id, subgraph, outputs)` is the sugar that goes
  with them: a compound answers a surface rather than a field, so reading it
  costs one `Subgraph` node per output, and those nodes' bindings have to agree
  exactly — an instance is inferred per binding signature, so one stray binding
  on one of four near-identical nodes makes a second instance and a second
  inlined copy of its arithmetic. `layer` writes them from one node, under
  `<id>.<port>`. The whole library joins the two shipped-graph loops in
  `tests/wgsl.rs` and the every-parameter-live case in
  `ashlar-bevy/tests/conformance.rs`, both of which now run over the study *and*
  the standard library; `tests/stdlib.rs` runs its generic claims over every key
  `stdlib::graphs()` answers rather than over a list, so a compound added later is held
  to the same bar without anybody remembering to add it.
- **Five study surfaces assembled out of the standard library**, which is the
  claim the whole expansion was built to test: a graph in a library stops being
  a thing you copy and becomes a thing you wire into. `study:rusted-steel` is
  the study's own metal instanced through one `layer`, its height bedded out of
  the plate's 0.14 of relief into a band in the middle of the unit, and then
  three compounds in the order the damage happened — `weathering:edge_wear` over
  the rolled crests, `weathering:rust` measured out of a Perlin cut saying
  where the water stood, `weathering:dirt_dust` in whatever hollow was left. It
  contains one noise anybody wrote for it and every other decision in it is made
  by a compound the crate ships. `study:weathered-paint` is the other half of
  the argument: the painted metal's panel through `weathering:peeling_paint`,
  and then the *same* rust compound seeded by that peel's own `bare` extra,
  which is the composition point the extras exist for — the film lifts, the
  steel under it is bare, and the rust starts exactly at the edge the peel
  drew rather than at a second mask that agrees to within a texel. Its metalness
  is lifted to one by the caller across the bare patches and taken back down by
  the rust's own mask, because neither compound can see the other. The two drawn
  surfaces have no noise deciding anything at all: `study:drain-cover` is a cast
  iron cover in its frame, drawn from signed distances — a `Gear` boss of twelve
  teeth whose tips stand exactly at the inner ring's inner edge so the smooth
  union fillets twelve webs, hollow rings and a rounded frame through
  `SdfCombine`, thirty-two slots laid by a `CircleMap` of sixteen turns and two
  rings, six hexagonal bolt heads by a `CircleSplatter`, and grit in the corners
  by `dirt_dust` — and `study:canvas` is thirty-two threads each way through a
  plain `Weave`, tinted per thread off its `Id` hash, with one anisotropic
  Perlin read twice, the second read turned a quarter turn, which is what makes
  one noise serve both the warp fibre and the weft. Counts rather than
  densities, in both. `study:brick-weathered` is the fifth and the only compiled
  one: the study brick through `dirt_dust` biased by `WorldMask::up()` and
  `moisture` with a live `wet`, which is the world-bias idiom the standard
  library is written around — a compound may never read a runtime input, so it
  takes a plain float and the caller decides what that float means. A bake
  refuses it by the path of the node that asked for a world normal; compiled at
  1024 it is fifty-one fragment instructions and ten texture reads over five
  bound images out of nineteen planes, with nothing frozen, because everything
  above the waterline is arithmetic over the coordinate and went into the
  textures. It binds the *brick's* height rather than the flooded one the
  moisture offers, and that is the one decision in the five that cost measuring:
  taking the flooded height costs four finite-difference taps, eight bound
  images instead of five, and `widens: false` — level-zero roughness at every
  distance, on a wall whose wet joints sit at 0.02 — to flatten a film in an
  eleven-millimetre bed joint that is not a puddle.
- **Twenty more maps, and the five that already shipped unchanged byte for
  byte.** The four surfaces that bake are written by the showcase's asset
  writer like the rest, and `assets/` goes from 24 605 508 bytes in 30 files to
  44 465 773 in 50, with `GOLDEN_256` and `GOLDEN_1024` re-recorded at
  thirty-six entries each. That the old five did not move was the test this
  step existed to run: phase 2 re-factored `Shape` so that its mask is an
  `SdfMask` over its distance, and twenty KTX2 files coming back container and
  compressed level identical is what says the re-factor kept the bit-for-bit
  promise it made. What did move is the checked-in graph library, because a
  `Subgraph` resolves by key at load and the writer therefore writes every
  compound a shipped surface reaches beside it:
  `assets/buildings/recipe-study/materials.graphs.ron` went from eight graphs
  and 68 320 bytes to twenty-two and 151 000. Two tests that read that file and
  then merged `stdlib::graphs()` into it had to stop merging, and
  `ashlar-material/tests/wgsl.rs` now makes the stronger claim instead — every
  compound on disk must be the graph `ashlar_material::stdlib` builds, node for
  node — which fails loudly on a compound edited in Rust and never written out,
  where the merge only ever failed on a collision. The same file's compute-stage
  count went from twenty-five stages to seventy-two: twenty graphs that bake
  with one outputs stage each, and fifty-two buffered filters between them, of
  which twenty-one are in the two layered surfaces alone. One compression number
  is worth keeping: `drain-cover/height` stores at **8.1 %** of its uncompressed
  chain where the three noise-derived heights store at 90 % to 93 %. The rule
  was never about bit depth, it was about entropy, and a height that takes five
  values with soft edges between them is the one thing in this repository zstd
  can eat.
- **The expansion's capture**, in
  `docs/research/material-synthesis` (removed; in git history)
  — the instruction count of every new node and every compound, the four bakes
  with their periods, lattices and height ranges, the compiled cost report
  verbatim beside the flooded-height comparison, the asset growth per map, and
  the gallery at 51 shots in 36.43 s against 39 in 19.5 s. One surface did not
  read on a lit wall, and the review sent it back rather than letting the
  capture record it: `study:rusted-steel` was rusty everywhere except in its
  colour — 72 % of the plate between 0.25 and 0.75 metalness and an albedo that
  never passed 0.29 saturation, because its seed band was a soft ramp near a
  half rather than a set of patches, and forty per cent of a dark oxide mixed
  uniformly into a light steel grey is a mid grey-brown. Four numbers in the
  showcase fixed it and none of them is in `weathering:rust`: the `Levels` band
  on the seed Perlin became a cut, the spread narrowed from 0.05 to 0.03 so the
  halo is narrower than the gaps between seeds, the seed lattice turned from
  four by sixteen to eight by four so a patch runs down the plate, and the
  plate hands the compound its own two oxide colours, because that mask peaks
  near two thirds and a third of a light steel left in every rusted texel is a
  grey. **50 % of the tile is now at metalness exactly one** — the plate
  `study:metal` bakes, bit for bit — against 8.5 % before, and the saturation
  ninetieth percentile goes from 0.16 to 0.26; four golden hashes at each
  resolution and four KTX2 files were re-recorded with it. Two things that did
  not read were left as follow-ups with their numbers. The material sheet is a
  weaker instrument for these four than for the three it was built for, and now
  for a reason it can be held to: a canvas thread is four tenths of a pixel in
  the largest column the sheet ever draws, and a drain cover is one object
  rather than a field, so 2.5 of them fit the near specimen. And the weathered
  brick at rest *is* the plain brick, by construction — its `wet` defaults to
  zero and its dirt is gated on a world normal that is approximately zero on a
  vertical wall — so the shot had to be made twice, and moving that one slider
  moves 4.02 % of the frame at a mean of 12.8 codes, almost all of it on the
  joint lines the eye was already reading. The plan's own follow-up list carries
  those, one the review found — a compound that binds no metalness or no
  occlusion is read *silently*, because only `height` and `emissive` are
  optional on a `PbrOutput` and the other four always answer their defaults —
  and one the retune ran into: `weathering:rust`'s mask cannot reach one, since
  the Perlin that decides how thick the scale is doubles as the one that
  decides whether there is any.

- **Strand layers**: geometry scattered from the same graph fields the surface
  is made of. `MaterialGraph::strands` holds named `StrandLayer`s beside the
  `PbrOutput`; `ashlar_material::strands` scatters one into a deterministic
  `StrandSet` over a repeat, plants it on plain triangles and builds ribbons or
  three-sided tubes; the buffered `StrandRelief` node splats the *same* set from
  directly above into planes the PBR half reads, so the texture the camera sees
  once the geometry has faded is the same strands. `Direction`, `Combine2` and
  `VoronoiOutput::Offset` are the three nodes a strand field needed and a texel
  did not. `ashlar::StrandSettings` on a `MaterialDefinition` asks for layers,
  level-of-detail distances, a density cut and shadow casting, and
  `ashlar_bevy::strands` answers one mesh per UV repeat per level, thinned by a
  rank prefix and widened by `1/sqrt(keep)` under a `VisibilityRange`. See
  [ADR 0004](https://github.com/JedimEmO/ashlar/blob/main/docs/adr/0004-strand-layers.md).

- **Cards**: the level past the last band of real strands, in
  `ashlar_bevy::cards`. Where a definition names a
  `StrandSettings::card_metres`, every clump of a layer becomes two crossed
  quads at the mean of its roots, sized from its own strands and oriented on the
  surface frame, wearing a four-variant atlas of the *same* set drawn side on by
  `ashlar_material::strands::cards`. The atlas is cached in `BakeCache` under
  the scatter's key plus the numbers it was drawn at, its mip chain is rescaled
  level by level to keep the alpha coverage level zero had — a box filter alone
  thins a cutout until a field of cards evaporates — and it is bound as
  `AlphaMode::Mask`, never blended, so depth, shadows and the prepass still
  work. The band starts on the margin the last level of detail ends on, so the
  dither crossfade is continuous, and past `card_metres` there is still nothing
  but the relief. Cards do not sway: they bind a normal map, the wind extension's
  two vertex stages declare no tangent to read one in, and at ten metres a
  strand's tip travel is a fraction of a pixel. `benchmark:grass` draws them from
  7 to 14 m.

- **Wind**: `ashlar_bevy::wind`, a `MaterialExtension` over `StandardMaterial`
  whose vertex stage bends a strand along a `StrandWind` resource by
  `strength * v² * stiffness * gust`, reading the `[phase, v, stiffness]`
  attribute every strand mesh already carried. The prepass and the shadow pass
  get the same displacement from the same function, or a blade's shadow stays
  where the blade was. `StrandWind::still()` is zero strength and renders
  byte-identically to a plain `StandardMaterial`, which is what the reference
  gallery sets.

- **`benchmark:moss-carpet`**: a second strand material, to show the vocabulary
  is not grass-shaped. One `StrandProfile::Fibre` layer of upright shoots grown
  off `weathering:moss`'s own coverage mask, with no flow field under it.

- **`--reference-key`** on the preview gallery, beside `--reference-ambient`: a
  building is lit to read across a facade and a material at arm's length is not,
  and under a key meant for a facade an 18 mm pile is a pale floor with the
  shadows blown off it. The material-scale captures of the grass are taken at
  the swatch studio's own 6500 lux.

- **Anisotropic filtering** on every surface's maps, at eight taps
  (`ashlar_bevy::ANISOTROPY`), for both a `Files` surface and a runtime bake.
  A trilinear sampler picks its mip off the longer axis of the footprint, so a
  wall seen at an angle loses its detail in the view that shows most of it.

- **The join between a meshed building and its materials**, in the library
  rather than in every renderer. `MeshedBuilding::pieces()` walks instances
  and their elements and yields each drawable `Piece` with the `Binding` in
  force on it — the cut slot for a cut batch, the instance's override where it
  has one — and `ashlar_bevy::dressed_pieces()` adds the definition that binding
  names. `Piece::shared_key()` names what pieces share the work derived from
  geometry and material together, which is what a strand layer is grown once
  per. The preview's three hand-written loops are now that walk.

- **`MaterialGraph::tile_metres`**, the repeat a graph was drawn for. Relief is
  metres per repeat and a strand's length is absolute metres, so a graph was
  always authored against a tiling and had nowhere to say so. Advisory, skipped
  when unset, and part of no bake or shader key. `ashlar_bevy::tiling` compares
  it with the definition's at `read`: a repeat that changed shape warns, a
  scaled one under a strand layer warns, and a uniform scale is a debug line,
  because a specimen sheet divides the tiling on purpose.

- **`MaterialDefinition::tile_scale`**, the opt-out that closes the gap the
  entry above left open: a uniform scale used to be a debug line whatever
  caused it, so an honest `tile_metres` typo (2.4 for 1.72) read exactly like
  the sheet's own deliberate ninth. Now a definition declares the factor it
  scales its graph's repeat by on purpose, `ashlar_bevy::scale_verdict` reads
  it beside the scale `tiling` found, and only a declared, matching factor
  stays quiet; a uniform scale with no declaration, or one that disagrees, is
  a `warn!` naming the fix. Advisory like `tile_metres` itself and skipped when
  unset. The material sheet, the benchmark and the corporate kit's
  whole-repeats-per-storey walls (`docs/guide/recipes.md`, "Whole repeats
  across a stacked or arrayed module") all declare it now, and no shipped
  library warns.

- **`ashlar-strands`**, and with it a lawn a game can ship. ADR 0004 said
  "generating a set is milliseconds, so there is no strand file format and no
  serialized `StrandSet`"; the milliseconds were a measurement of a doctest
  lawn — the shipped one is 528 ms on eight cores — and the premise under them,
  that a game holds the graph, is the one the 2026-09-20 decision reverses.
  The new crate holds everything after the scatter: `Strand`, `StrandSet`,
  `place`, `mesh`, the rank-cut levels and the card atlas, on `glam`, `serde`
  and `thiserror`. `ashlar-material` keeps `scatter` and the `StrandRelief`
  splat and re-exports every name from `ashlar_material::strands`, so no path a
  caller wrote has moved.

- **`ashlar_strands::file`**, the baked strand set. One file per material
  definition holding every layer it grows, little-endian, versioned, with a
  directory entry per layer and a field-major payload — one field is one run of
  bytes, and the values in it are alike. Lossless: `read(write(set))` is `set`,
  which is what lets a test assert that a baked lawn and a scattered one mesh
  to identical triangles. Stored rather than compressed, because zstd takes
  15 % off the grass and 18 % off the moss and that does not pay for a decoder
  in every game that draws a lawn; the header records a scheme anyway. The
  reader validates every offset, length, name and count against the file, and
  every strand for a non-finite number, a rank outside `0..=1` and a negative
  size — the three whose failure is not a bad blade but a broken chunk.

- **`StrandSettings::baked_set`**, the content key that names it, the way
  `Surface::Files` names its maps. Serde-defaulted and skipped when absent,
  so every shipped library is byte for byte the library it was, validated in
  `check`, and preflighted by `ashlar_bevy::read_library` before a window exists — the
  identifier and version by hand in a featureless build, and the directory and
  the definition's own layer names under `strands`, which is the stale case.
  `MaterialDefinition::check` no longer insists a definition growing strands
  name a graph: a `baked_set` is the other answer, and a game's lawn may sit on
  a `Plain` surface.

- **`ashlar-bevy`'s `strands` is a game feature**, and `strand-scatter` is the
  tool feature beside it. `cargo tree -e normal --features strands` has no
  `ashlar-material` in it. `StrandSets` is the value the two halves meet on:
  `read` builds one from a file and `scatter_sets` from a graph, and
  `create_strands` takes one and cannot tell which. Card atlases move out of
  `BakeCache` into a `CardCache` of their own, keyed by where the set came from
  rather than by a graph. `examples/lawn.rs` is thirty lines between a file and
  a lawn, and `just draw` grows one on a GPU and looks at the pixels.

- **The benchmark writes its lawns.** `grass` and `moss-carpet` ship
  `set.strands` beside their four KTX2 maps — 24.09 MiB over three layers and
  10.98 MiB over one — and a golden beside `committed_preview_maps_match_fresh_bakes`
  holds them against a fresh scatter. No baked texel moved.
- **Merged geometry, interiors and damage** (ADR 0005). Every face of a
  `TriangleMesh` names its source: `sources: Vec<FaceSource>` replaces
  `cut_faces: Vec<bool>`, with `FaceOrigin::{Body, Cutter, Damage}`, and
  `is_cut(face)` and `cut_faces()` ask the old question. A cutter may name its
  own slot with `Geometry::cut_material`, `Geometry::cutters()` defines the
  numbering, and `Element::slot_for` resolves a face.
  A recipe may be `merged`: `MergeGroup`, `Instance::group`,
  `Element::standalone`, `Building::merge_groups`, and
  `GeometryMesher::mesh_group` with `PlacedGeometry` and `GroupMesh`, whose
  default refuses. `MeshedBuilding::groups` holds a `MergedGroup` of
  `GroupBatch`es per group, in building space, with box UVs in the building
  frame and normals from the new kernel-free `ashlar::shade_normals`.
  Interiors are classified and never culled: `Side` and `Element::interior`,
  a storey on every `Piece`, `Geometry::portal` with
  `GeometryMesher::portals`, `PortalShape`, `Portal` and
  `MeshedBuilding::portals`, where a portal is measured against the core its
  cutter removes, and `Room` with `Building::room_at`.
  Damage is data: `Damage`, `DamageLog`, `Damage::blast`, `Damage::collapsing`,
  `Shape::Hull`, `Geometry::hull`, `Geometry::ball` and the kernel-free
  `Geometry::bounds`; `mesh_building_with_damage`, which is `GroupSolids::build`
  replaying each record through the same `apply` play uses, and `GroupSolids`,
  `GroupHit`, `Debris` and `DebrisKind`, held to the same volume, area and
  sources by a test. A collapsing hit drops a piece that reaches neither the
  ground nor anything supported, across every group, so a piece held up by
  another group falls when that group goes; a piece already loose in the
  undamaged building is left alone unless a hit changed it, and `Debris` carries
  the piece's `bounds` for a renderer to decide what falls with it.
  `GeometryMesher::mesh_group` takes `PlacedCut`, so a cut says whether what it
  leaves unattached falls; `TriangleCollider`,
  `MeshedBuilding::mesh_colliders`, `groups_touched`, `standalone_touched`,
  `replace_group` and `record`.
  `ashlar_bevy::batch_drawables` draws what a hit produced, and the crate still
  links no kernel. The showcase gains `corporate::merged_by_storey` and the
  `interior` house, and the preview gains the `corporate-merged`,
  `corporate-block-merged` and `interior` scenes, storey and side culling on
  `PageUp`, `PageDown` and `I`, and shift-click or `--blast x,y,z` to blast.
  `cargo run -p ashlar-manifold --example damage_server` is a server that links
  no Bevy.

- **A default material library, and one way to ship it.** `ashlar-surface`
  sits under `ashlar` and `ashlar-material` and owns the definition vocabulary,
  so `ashlar_material::stdlib` ships finished materials and not only graphs:
  twenty-eight `library:*` surfaces built from `layouts:*`, `substances:*` and
  the weathering compounds, with a definition for each from
  `stdlib::materials()` baked at 512 when it is registered. The brick,
  cobblestone, grass and moss carpet moved in as `library:*`, and every other
  textured surface was reviewed against an ambientCG scan to their standard.
  `ashlar_material::export` is the content step for a build that wants files:
  it answers the texture sets, the strand-set bytes and a library whose
  surfaces name them, and does no IO. `ashlar_bevy::check_library` and
  `check_library_with_graphs` preflight an in-memory library the way `read` and
  `read_library_with_graphs` preflight a file, and `ashlar_bevy::gpu::dispatchable`
  sends a graph the device cannot split to the CPU bake. The preview takes a
  scene's libraries as Rust values through `Scene::materials` and
  `Scene::graphs`, and gains a `sheet-<name>` and a `detail-<name>` scene for
  every baked library material. `just materials` exports the showcase's library
  into the ignored `assets/materials/`. See the 2026-09-24 amendment to
  [ADR 0003](https://github.com/JedimEmO/ashlar/blob/main/docs/adr/0003-procedural-materials.md).

### Changed since the extraction

- **Names a consumer reads without a glossary** (2026-09-25). "Baked" now only
  ever means the content step's files, and the rest was renamed to match:
  - `Surface::Baked` is `Surface::Graph` and `Surface::Textures` is
    `Surface::Files`, in Rust and in RON (`Graph((..))`, `Files(..)`);
  - `ashlar_bevy::procedural` is `runtime_bake`, after its feature;
    `create_baked` is `create_graph_material`, `shader::create_shader` is
    `create_shader_material`, and `BakedImages`/`BakedTextures`/`baked_images`
    are `GraphImages`/`GraphTextures`/`graph_images`;
  - `ashlar_bevy::read`, `check` and `create` are `read_library`,
    `check_library` and `create_material`, and the `*_with_graphs` pair follows;
    `surfaces` is `dressed_pieces` (`SurfacePiece` is `DressedPiece`),
    `MeshedBuilding::surfaces` is `pieces`, and `SpawnContext` is
    `UploadContext`;
  - `ashlar_bevy::prelude` holds everything the game path names;
    `BuildingSpawned` is `AshlarBuildingSpawned` and `AshlarLod` is
    `AshlarCrossfade`;
  - `stdlib::stdlib()` is `stdlib::graphs()`;
  - `Content::materials`, `materials_on`, `building` and `write` are
    `write_materials`, `write_materials_on`, `write_building` and `write_file`,
    and `flatten_bindings` is `flatten_overrides`.

- **`Piece` says what it is a piece of.** `instance`, `part`, `element` and
  `is_cut` moved into `PieceOrigin::Element`, beside `PieceOrigin::Group`, and a
  piece gained `storey` and `side`. A merged batch has no single instance to put
  in those fields. `Piece::mesh_key()` is what uploaded geometry is shared by,
  `shared_key()` is that key and the binding, and `label()` names an entity.
  `ashlar_bevy::drawables` keys by `mesh_key`, which also stops two cut batches
  of one element colliding under the old `(part, element, is_cut)` key.
- **The kernel is built with the system compiler in this workspace.**
  `.cargo/config.toml` forces `CC` and `CXX`, because an activated Anaconda
  environment exports its own and that GCC builds a kernel whose static
  initialisers never run under `rust-lld`: `Manifold::sphere` and `refine` died
  on SIGFPE. `crates/ashlar-manifold/tests/kernel_init.rs` is the canary.
- **This workspace builds x86-64 with `-C target-feature=+sse4.1,+fma`**, from
  a new `.cargo/config.toml`. A bake is about 21% faster on the SOI cobblestone
  and 16% on the brick, and no baked byte moves: a hardware FMA is the number
  `fmaf` returns and `roundss` is the floor `floorf` returns, so the goldens,
  the shipped-resolution cases, `just conformance` and `just draw` all pass
  unaltered. **It raises the CPU this workspace's binaries need to roughly
  Haswell (2013) or Excavator (2015)** — older hardware gets SIGILL, not a slow
  run. The flags are scoped to `cfg(target_arch = "x86_64")`, so an ARM build is
  unaffected, and cargo reads them only for builds *in* this workspace: a game
  depending on `ashlar-material` compiles it at its own baseline and bakes the
  same bytes more slowly. `RUSTFLAGS` in the environment replaces them rather
  than adding to them, which is why the CI workflow now repeats them beside its
  `-D warnings`.
- **`interp::floor_cell` uses the hardware floor again**, reverting half of
  18c88b5: under `+sse4.1` the bit-twiddled integer floor is slower than the
  `roundss` it was written to avoid, by 3.6% of the SOI bake and 8.0% of the
  brick. `interp::cell` keeps its integer reduction, because no target feature
  turns `rem_euclid` into an instruction. `interp::floor` is gone with the
  branch it wrapped, and so is the test that pinned the two floors against each
  other, which had become `x.floor() == x.floor()`.
- **`MaterialDefinition::surface`** replaces the three optional texture keys
  with a `Surface` enum: `Plain`, `Files` (base colour, normal, ORM, height
  and emissive, with the `Bake` a content step recorded), `Graph(Bake)` and
  `Shader`, over the new plain-data `ParamValue`. The enum denies unknown
  fields, so an unknown field inside a surface is a parse error naming it and a
  map may be omitted rather than written `None`. This is a breaking change to
  the material RON form as much as to the Rust type: a library written against
  the old `base_color_texture`, `normal_texture` and `orm_texture` keys no
  longer parses, and there is no compatibility form. The study's
  `materials.ron` is regenerated by the same writer that writes its textures,
  and the corporate library was converted by hand.
  See [ADR 0003](https://github.com/JedimEmO/ashlar/blob/main/docs/adr/0003-procedural-materials.md).
- **`ashlar_bevy::read_library` takes a graph library** beside the material library, as
  the design asks, and `read_graphs` reads and validates one from RON. A
  `Surface::Graph` is preflighted by validating and lowering its graph, so an
  unknown graph key, a parameter the graph does not declare or a node the
  backend cannot lower is a startup error naming the material — the way a
  missing PNG already was. A `Surface::Shader` is preflighted the same way, by
  lowering it, partitioning it and emitting both stages, so everything a
  compiled material can be refused for except the WGSL itself is refused before
  the window opens. `graph_params` converts the parameter values a `Bake` carries
  into the graph crate's, which is the one place the two `ParamValue` enums
  meet. The preview passes a scene's graph library through `Scene::graph_library`
  or `--graphs`.
- **`reload::rebake` re-makes compiled surfaces too**, with the shader registry
  cleared beside the bake cache and for the same reason: a `ShaderKey` names a
  graph and says nothing about what its nodes do, so after an edit every entry
  would answer the shader it had before. An edit to a compiled graph used to
  cost a restart.
- **`bake::encode_bound` quantises the way the five named maps do.** It dithered
  every format and renormalised a normal chain before filtering it; the bake
  dithers only the sRGB colour — dither in a direction field or a roughness is
  noise in data nobody looks at directly — and filters the un-normalised normal
  chain, renormalising what each level ships. Both are now the same on the two
  paths, which is what makes a bound texture and the map it replaces byte for
  byte identical. Before: one code on an eighth of a normal map's texels.
- The preview's camera framing is one `framing` function and two named
  standoff constants instead of the same expression written out three times
  with two different factors.
- The showcase kits declare collision on their elements instead of carrying
  hand-typed collider tables, and the study kit's walls attach bay to bay
  instead of multiplying a bay index. `ashlar_showcase::outpost_colliders` and
  `ashlar_showcase::corporate::colliders` are gone.
- **A compiled graph's bound textures are cached by the plane behind each
  image** rather than by the `ShaderKey` of the graph that asked for it, so two
  compiled graphs over one wall bake that wall once. `planes::PlaneKey` is the
  new identity — the canonical encoding of a plane's whole closure after
  folding, each plan written once with its filter chain, its resolution and its
  value type, which is what keeps the key the sum of the graph above the filter
  rather than something that doubles with every level of sharing — and
  `runtime_bake::PlaneImageKey` is an image's: that key per lane, and the format.
  Both are exact keys rather than digests, for the reason `BakeKey` already
  gives, and `planes::plane_keys` answers them without rasterising anything.
  Where a graph shares only some of its images, `planes::rasterise_wanted`,
  `planes::plane_closure` and `gpu::GpuBaker::planes_wanted` rasterise the
  planes it is missing and the planes those read, and skip the rest on either
  backend. `study:concrete-wet` after `study:concrete-cut-aware` costs 194 ms
  where it cost 1391, `just references` renders the gallery in 19.5 s instead
  of 23.0 s, and not a byte of any shipped map moves.
  `BakeCache::insert_static` now takes handles rather than images, since a
  compiled graph may bind nothing new, and `BakeCache::rasterised` and
  `bound_images` are what a caller counts the saving with.
- **A GPU bake uploads each bound image once**, rather than once per pass.
  `gpu::GpuBaker::rasterise` kept one cache of uploaded textures for its plane
  dispatches and a second for its outputs dispatch, so the outputs pass packed
  and wrote every plane the plane dispatches had just put on the device a second
  time; there is now one cache for the whole bake. Packing a megatexel plane
  into `Rgba32Float` bytes and writing it is about 37 ms at 1024 squared in a
  debug build, against about 60 ms for the dispatch that reads it, so at 1024
  `study:painted-metal` goes from nine uploads and 1.19 s to five and 1.08 s,
  `study:plaster` from six and 1.08 s to four and 1.01 s, and `study:brick` from
  six and 1.35 s to five and 1.30 s. The bytes do not move: the same GPU writer
  answers the same twenty files. `gpu::GpuBaker::uploads` is a counter of images
  written to the device, and
  `the_gpu_bake_is_the_cpu_bake_within_the_conformance_budget` holds every study
  graph to the uploads a cache that shares every finished image makes.
- **A GPU bake of a graph whose planes are packed four to an image answers the
  right texels.** `gpu::GpuBaker` cached an uploaded texture as soon as the
  dispatch binding it had every lane *that dispatch reads*, which for a packed
  image is not every lane the image holds: the kernel for an early plane samples
  one lane of four, and the half-written texture it uploaded was then handed,
  from the cache, to every later dispatch and to the outputs pass that reads all
  four. Completeness and the upload are now over the image's own lanes, out of
  the partition. Nothing shipped was wrong — the study's maps are baked on the
  CPU — but a game running a runtime GPU bake of a graph past
  `MAX_BOUND_TEXTURES` bindings got three zeroed channels: at 512 texels
  `study:rusted-steel`'s roughness was 0.61 out over 98 % of the tile and
  `study:weathered-paint`'s height over all of it, against a worst difference of
  1.2e-5 now. `just conformance` is the only thing that says so, and it did not
  say it before phase 4 because no study graph packed.

- **A compiled material's roughness widens with distance**, as a baked one's
  always has. `mips::encode_mips` derives the Toksvig term from the normal
  plane's own coherence and can only do it because a bake holds both planes at
  once; a partition binds separate images, so a compiled wall kept its close-up
  roughness at every distance and went glassy where its baked twin stayed matt.
  The coherence now rides in the **alpha channel of the bound normal map's
  chain** — `bake::bound_mips` writes `|n_avg|` there, one per texel per level,
  and level 0 is one — and the emitted fragment applies the bake's own
  `r' = sqrt(r^2 + (1 - a) * k)` over `mips::TOKSVIG_K` after the runtime
  roughness expression. Six instructions and no extra fetch: the alpha comes
  off a read the shader already makes, at the same coordinate and so at the same
  level the hardware chose. It holds whether the roughness is a texture read, a
  literal or eleven instructions over a live uniform, which is why it is emitted
  rather than baked into the bound roughness map's own chain — the study's wet
  concrete has no roughness map to have widened. A material whose normal is
  itself computed per fragment has no `|n_avg|` to read and keeps its level-0
  roughness; `CostReport::widens` is false there and the report says so in words.
  A graph with `normal_strength` at zero is not that case — a flat normal plane
  loses no coherence, so the bake's own term is zero at every level and the two
  agree by widening nothing — and the report stays quiet about it.
  Measured: the compiled `study:concrete` at mip 3 of a 1024 bake is the bake's
  own ORM roughness to within half an eight-bit code, where without the term it
  is four codes glossier. On the material sheet the compiled specimens of
  `sheet-brick` move on 30 445 pixels and by up to 11 codes, almost all of it
  darker; `sheet-concrete`'s move on 855 and by one, because that wall's relief
  is shallow enough that its widening is four tenths of a code of roughness at
  any level against brick's twenty-three. All three sheet captures in
  `docs/research/shader-materials/` are re-shot, the third because a set of
  three captured on two different days compares nothing. `Partition::widening`,
  `wgsl::emit_compute_mesh`'s new `level` argument and
  `ComputeShader::level` are what a harness reads a chain with — a fragment
  picks its level from derivatives a dispatch does not have, and at level 0 the
  term is exactly nothing.

- **`just conformance` runs on one test thread.** Every case in that file opens
  its own adapter through `initialize_renderer`, so the default thread count
  starts one renderer per ignored test at once, and about one run in four the
  NVIDIA driver takes that badly enough to kill the process on a signal before
  the first case reports. It predates the case added above — it reproduces with
  that one skipped — but a fifth concurrent adapter is not the direction to push
  it in. Serially the suite is 25 seconds instead of 16 and has not failed.

- **`MeshedBuilding`, `ElementMesh`, `ElementCollider` and `InstanceCollider`
  live in `ashlar`**, beside the `GeometryMesher` port they are the output of.
  They named no kernel type, and `ashlar-bevy` could not see them where they
  were. `ashlar-manifold` re-exports them, so no import changes.

- **A compiled graph leaves the deferred prepass alone.** Its generated prepass
  writes no G-buffer, so on a camera with a `DeferredPrepass` the surface
  vanished; it now falls back to its plain `StandardMaterial` there and says
  so. `GpuBakePlugin` no longer offers a baker on a device without compute, and
  warns when it was added before the render plugin rather than silently meaning
  CPU bakes. `GraphShaders::clear` takes the `Assets<Shader>` and frees the
  shaders nothing else could.

- **A bake is faster again, to the same bytes.** `MEMO_BUDGET` is one gibibyte,
  because the graphs outgrew 256 MiB and a third of the interpreter's work had
  become recomputation; `interp::cell` is integer arithmetic; and the benchmark
  writes its four KTX2 files on a thread each, which was most of the wait. The
  benchmark regeneration goes from 4m31 to 2m33.

- **The preview's `--write-study-assets` is `--write-assets`**, and runs the
  catalog's content step, which for the showcase is the library export.
  `--write-png` and `--gpu` keep their meaning beside it.

### Changed from the original in-game version

- The three crates were renamed: `scrolls-buildings` to `ashlar`,
  `scrolls-building-mesh` to `ashlar-manifold`, `scrolls-building-render` to
  `ashlar-bevy`, and `veyra-buildings` to `ashlar-showcase`.
- The preview is a library plus a binary. Scenes come from a `Catalog` a
  downstream game builds over its own content, and `--scene` and
  `--reference-scenes` validate against that catalog rather than a fixed enum.
- The study texture generator moved from the preview tool into the showcase
  crate, where the keys and the weathering it encodes actually belong.

### Removed

- The finished plans, progress logs and research notes under `docs/design` and
  `docs/research` (2026-09-26); what was still true moved into the guides, the
  decisions and the skills, and git keeps the rest. Dead code went with them:
  the showcase's unused grounding specs, `pack-reference`, the showcase's own
  material exporter (the content step is the one way to write materials), the
  preview's `--write-assets`, `--write-png` and `--gpu`, unused dev-dependencies,
  and `ashlar_bevy::{Definitions, Graphs}`, which moved into the preview.

- The reference gallery's pixel baseline comparison, with its
  `--reference-baseline` flag. Coincident-edge draw ordering and driver
  revisions moved pixels for reasons unrelated to the geometry. The gallery is
  for visual inspection.
- The game-only site-fitting example and its dependencies on the game's
  worldgen, registry and core crates.
- Every baked file in the repository. The 550 MB of benchmark maps, renders
  and strand sets under `assets/materials/benchmark`, the study's maps under
  `assets/buildings/recipe-study`, and the material and graph libraries written
  as RON beside them, including the hand-edited corporate palette. The Rust
  graphs are the source and a bake happens when it is needed.
- The nine `study:*` graphs, which the library replaced, the `benchmark:` key
  shim with its module and example, the study's asset writer, and the
  `material-library` and `sync-materials` examples, which `export-materials`
  replaces. The showcase's four compiled surfaces are `showcase:*` now.
- What nothing used any more (2026-09-25): `ashlar_bevy::Definitions` and
  `Graphs`, which only the preview read and which now live there;
  `ashlar-manifold`'s re-export of `ashlar`'s meshed-building types, which are
  imported from `ashlar`; the two `stdlib::*_HEIGHT_SCALE_METRES` aliases; the
  showcase's grounding specs, which no scene fitted to terrain, and its
  `pack-reference` example, whose KTX2 output nothing read; and
  `ashlar_preview::Source` with the two catalog getters that returned it.
- The finished plans, progress logs, review notes and research captures under
  `docs/design` and `docs/research`. What stayed true is in the guides, the
  ADRs, the skills and the rustdoc; the rest is in git history.
