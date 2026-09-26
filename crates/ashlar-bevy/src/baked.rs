//! Baked content as Bevy assets: the path a game ships.
//!
//! A content step bakes buildings to `.ashlar` files (`ashlar::BakedBuilding`,
//! ADR 0006) and materials to KTX2 maps beside a file-backed material library.
//! [`AshlarPlugin`] teaches an app to load both through its [`AssetServer`] and
//! to spawn what they describe:
//!
//! - [`BakedBuildingAsset`], loaded from `*.ashlar`, is a building at every
//!   level of detail with its collision proxies, portals and rooms;
//! - [`MaterialLibraryAsset`], loaded from `*.materials.ron`, is the material
//!   library that names the maps, preflighted as it loads so a missing or
//!   miscoloured map is a load error naming the key rather than a black wall;
//! - [`AshlarBuilding`] on an entity asks for one building dressed in one
//!   library. When both have loaded, the plugin spawns the building's pieces as
//!   children of that entity — every level, each with a [`VisibilityRange`]
//!   band — and its collision proxies as children carrying [`AshlarCollider`],
//!   puts its rooms and portals on the entity as [`AshlarSpaces`], marks it
//!   [`AshlarSpawned`] and sends [`AshlarBuildingSpawned`].
//!
//! No feature is needed for any of it. The featureless crate draws `Plain` and
//! `Files` surfaces, which is what a content step writes; a library naming
//! a material graph is refused with a message naming the feature that would
//! bake it, because a bake in front of a frame is a tool's cost and not a
//! game's.
//!
//! ```no_run
//! use ashlar_bevy::prelude::{AshlarBuilding, AshlarPlugin};
//! use bevy::prelude::*;
//!
//! fn setup(mut commands: Commands, server: Res<AssetServer>) {
//!     commands.spawn((
//!         AshlarBuilding {
//!             building: server.load("buildings/outpost.ashlar"),
//!             materials: server.load("materials/library.materials.ron"),
//!         },
//!         Transform::from_xyz(0.0, 0.0, 0.0),
//!     ));
//! }
//!
//! App::new()
//!     .add_plugins((DefaultPlugins, AshlarPlugin::default()))
//!     .add_systems(Startup, setup)
//!     .run();
//! ```
//!
//! # Levels of detail
//!
//! Every level of a baked building is spawned at once, each piece with a
//! [`VisibilityRange`], and Bevy draws the band the camera is in. Level `n`
//! draws from level `n - 1`'s `until` to its own; the last level draws to any
//! distance, unless it too has an `until`, past which the building is culled.
//! Across each boundary the two levels crossfade over a margin of
//! [`AshlarPlugin::crossfade`] times the boundary distance — a tenth by
//! default, so a level ending at 60 m fades out between 60 and 66 m while the
//! next fades in over the same metres.
//!
//! The distance is Bevy's: from the camera to the piece's origin. An element
//! piece's origin is its instance's placement, which is the same point at
//! every level, so the crossfade is between two views of one thing standing in
//! one place. A merged group's mesh is in building space, so it is drawn about
//! the centre of its level-zero bounds, the same point at every level: a large
//! merged building switches level group by group, by each group's own
//! distance, and not by how far the camera is from the building's origin.
//!
//! A piece a coarser level left unchanged is one entity whose band runs on
//! through that level. Two identical pieces crossfading into each other would
//! draw the same triangles twice across the margin for nothing.
//!
//! ## In the browser
//!
//! Bevy 0.19.1 cannot crossfade a [`VisibilityRange`] on WebGL2. A device with
//! fewer than six storage buffers per shader stage gets a uniform buffer for
//! the visibility ranges instead, and the two halves of that fallback disagree:
//! `mesh_view_bindings.wgsl` declares binding 14 as
//! `array<vec4<f32>, 64>`, 1024 bytes, while the bind group layout in
//! `mesh_view_bindings.rs` gives it a `min_binding_size` of
//! `Vec4::min_size()`, 16 bytes. (Between four and five storage buffers the
//! layout picks a storage buffer while the shader still declares the uniform,
//! which fails the same way.) The shader reads binding 14 only to dither a
//! crossfade, so the first mesh with a crossfading range fails pipeline
//! validation and the app quits; an *abrupt* range, one with empty margins,
//! never compiles that code.
//!
//! So on such a device every piece gets an abrupt range, cut at the middle of
//! each crossfade margin: [`Bands::Auto`], the default, decides from the
//! [`RenderDevice`]'s limits when a building spawns. Bevy still culls each
//! piece to its band on the CPU; what a visitor loses is the dithered fade,
//! not the levels, and a game's own [`Visibility`] is untouched. Every
//! browser today runs Bevy on WebGL2, which is where this bites; a native
//! device reports enough storage buffers and keeps the crossfade exactly as
//! before.
//!
//! # Sharing
//!
//! A part is evaluated once however many times a building places it, and the
//! plugin uploads it once: meshes are cached by building asset and contents,
//! and materials by library asset and the whole [`ashlar::Binding`], so every
//! instance of a part, at every level that draws it unchanged and in every
//! building spawned from one file, is one mesh and one material and batches.
//! What a building resolves to is kept too, so its thousandth copy clones
//! handles and reads nothing. [`AshlarCache`] is that cache; a change to either
//! asset, or an asset nothing holds any more, purges its entries.
use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result};
use ashlar::{BakedBuilding, MaterialLibrary};
use bevy::{
    asset::{AssetLoader, LoadContext, LoadState, io::Reader},
    camera::visibility::VisibilityRange,
    prelude::*,
    render::renderer::RenderDevice,
};

/// The file extension a baked building is loaded from.
pub const BUILDING_EXTENSION: &str = "ashlar";

/// The file extension a file-backed material library is loaded from. Two dots,
/// because a plain `.ron` belongs to whatever else a game reads as RON.
pub const LIBRARY_EXTENSION: &str = "materials.ron";

/// A baked building, as loaded from a `.ashlar` file.
#[derive(Asset, TypePath, Debug)]
pub struct BakedBuildingAsset(pub BakedBuilding);

/// A material library, as loaded from a `.materials.ron` file.
#[derive(Asset, TypePath, Clone, Debug)]
pub struct MaterialLibraryAsset(pub MaterialLibrary);

/// Loads [`BakedBuildingAsset`]s. The reader revalidates the recipe inside, so a
/// file written by one version of the rules and read by another says so.
#[derive(Default, TypePath)]
pub struct BakedBuildingLoader;

impl AssetLoader for BakedBuildingLoader {
    type Asset = BakedBuildingAsset;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &(),
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let building = BakedBuilding::read(&bytes)
            .with_context(|| format!("reading baked building {}", load_context.path()))
            .map_err(boxed)?;
        Ok(BakedBuildingAsset(building))
    }

    fn extensions(&self) -> &[&str] {
        &[BUILDING_EXTENSION]
    }
}

/// Loads [`MaterialLibraryAsset`]s, and preflights every map and strand set the
/// library names through the same asset reader unless told not to.
#[derive(TypePath)]
pub struct MaterialLibraryLoader {
    /// Open every texture and strand set the library names while it loads.
    pub preflight: bool,
}

impl AssetLoader for MaterialLibraryLoader {
    type Asset = MaterialLibraryAsset;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &(),
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let path = load_context.path().to_string();
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let library = parse_library(&bytes)
            .with_context(|| format!("reading material library {path}"))
            .map_err(boxed)?;
        if self.preflight {
            let textures = crate::texture_keys(&library)
                .with_context(|| format!("preflighting {path}"))
                .map_err(boxed)?;
            for (key, _) in textures {
                let bytes = load_context
                    .read_asset_bytes(key.clone())
                    .await
                    .with_context(|| format!("opening texture {key}"))
                    .with_context(|| format!("preflighting {path}"))
                    .map_err(boxed)?;
                crate::check_texture(&bytes, &key)
                    .with_context(|| format!("preflighting {path}"))
                    .map_err(boxed)?;
            }
            let sets = crate::strand_set_keys(&library)
                .with_context(|| format!("preflighting {path}"))
                .map_err(boxed)?;
            for (material, key) in sets {
                let bytes = load_context
                    .read_asset_bytes(key.clone())
                    .await
                    .with_context(|| format!("opening strand set {key}"))
                    .with_context(|| format!("material {material}"))
                    .map_err(boxed)?;
                crate::open_strand_set(&bytes, &key, &library.materials[&material])
                    .with_context(|| format!("material {material}"))
                    .map_err(boxed)?;
            }
        }
        Ok(MaterialLibraryAsset(library))
    }

    fn extensions(&self) -> &[&str] {
        &[LIBRARY_EXTENSION]
    }
}

/// Parse a library and check each definition on its own; the building-bound
/// half of the check happens when a building wears it.
fn parse_library(bytes: &[u8]) -> Result<MaterialLibrary> {
    let text = std::str::from_utf8(bytes).context("the library is not UTF-8")?;
    let library: MaterialLibrary = ron::from_str(text)?;
    for (key, definition) in &library.materials {
        definition.check(&format!("materials[{key}]"))?;
    }
    Ok(library)
}

/// An `anyhow` error as the error an asset loader reports, chain and all.
fn boxed(error: anyhow::Error) -> BevyError {
    let boxed: Box<dyn std::error::Error + Send + Sync> = format!("{error:#}").into();
    BevyError::from(boxed)
}

/// Loads baked buildings and material libraries, and spawns every
/// [`AshlarBuilding`] once both of its assets have loaded.
///
/// Needs an app with [`AssetPlugin`], and `Assets<Mesh>`, `Assets<Image>` and
/// `Assets<StandardMaterial>` initialised — which `DefaultPlugins` is.
#[derive(Clone, Debug)]
pub struct AshlarPlugin {
    /// Preflight every map and strand set a material library names while the
    /// library loads. On by default; a game that trusts its content step and
    /// wants the library to load without reading every map twice turns it off.
    pub preflight: bool,
    /// The crossfade across a level boundary, as a fraction of the boundary's
    /// distance. See the module documentation.
    pub crossfade: f32,
    /// How one level gives way to the next: crossfading where the device can
    /// draw it, cut where it cannot. See [`Bands`].
    pub bands: Bands,
}

impl Default for AshlarPlugin {
    fn default() -> Self {
        Self {
            preflight: true,
            crossfade: 0.1,
            bands: Bands::Auto,
        }
    }
}

/// How one level of detail gives way to the next across a boundary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Bands {
    /// [`Crossfade`](Self::Crossfade) where the render device can draw one and
    /// [`Abrupt`](Self::Abrupt) where it cannot, decided from the
    /// [`RenderDevice`] when a building spawns. An app with no render device
    /// yet crossfades natively and cuts on `wasm32`, where a browser's device is
    /// WebGL2 and arrives a few frames after startup. See the module
    /// documentation.
    #[default]
    Auto,
    /// Bevy's dithered crossfade over each margin.
    Crossfade,
    /// A cut at the middle of each margin: an abrupt [`VisibilityRange`],
    /// which Bevy culls on the CPU and never dithers.
    Abrupt,
}

/// The fewest storage buffers per shader stage at which Bevy 0.19.1's shader
/// and bind group layout agree on the visibility-range binding, which a
/// crossfade reads. See the module documentation.
const CROSSFADE_STORAGE_BUFFERS: u32 = 6;

impl Bands {
    /// Whether levels are cut rather than crossfaded on `device`.
    #[must_use]
    pub fn abrupt_on(self, device: Option<&RenderDevice>) -> bool {
        match self {
            Self::Crossfade => false,
            Self::Abrupt => true,
            Self::Auto => device.map_or(cfg!(target_arch = "wasm32"), |device| {
                device.limits().max_storage_buffers_per_shader_stage < CROSSFADE_STORAGE_BUFFERS
            }),
        }
    }
}

/// `range` cut at the middle of each margin: the band a crossfade would
/// switch at, with nothing to dither.
#[must_use]
pub fn abrupt(range: &VisibilityRange) -> VisibilityRange {
    VisibilityRange::abrupt(
        f32::midpoint(range.start_margin.start, range.start_margin.end),
        // The last level's band ends at `f32::MAX`; a midpoint of two of those
        // stays there.
        f32::midpoint(range.end_margin.start, range.end_margin.end),
    )
}

impl Plugin for AshlarPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<BakedBuildingAsset>()
            .init_asset::<MaterialLibraryAsset>()
            .register_asset_loader(BakedBuildingLoader)
            .register_asset_loader(MaterialLibraryLoader {
                preflight: self.preflight,
            })
            .init_resource::<AshlarCache>()
            .insert_resource(AshlarCrossfade {
                crossfade: self.crossfade,
                bands: self.bands,
            })
            .add_message::<AshlarBuildingSpawned>()
            .add_systems(Update, (reload_buildings, spawn_buildings).chain());
    }
}

/// Ask for a baked building dressed in a material library. The entity's own
/// [`Transform`] places the building; its pieces are spawned as children.
#[derive(Component, Clone, Debug)]
#[require(Transform, Visibility)]
pub struct AshlarBuilding {
    /// The `.ashlar` file.
    pub building: Handle<BakedBuildingAsset>,
    /// The `.materials.ron` library its material keys resolve in.
    pub materials: Handle<MaterialLibraryAsset>,
}

/// On an [`AshlarBuilding`] entity once its children have been spawned.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct AshlarSpawned;

/// On an [`AshlarBuilding`] entity whose assets failed to load or whose
/// materials could not be drawn, with the reason. Removing it retries.
#[derive(Component, Clone, Debug)]
pub struct AshlarFailed(pub String);

/// One drawable piece of a spawned building.
#[derive(Component, Clone, Debug)]
pub struct AshlarPiece {
    /// The first level of detail that draws it; zero is the building as
    /// authored. A piece coarser levels left unchanged draws on through them,
    /// and its [`VisibilityRange`] says to what distance.
    pub level: usize,
    /// What it is a piece of, as [`ashlar::Piece::label`] names it.
    pub label: String,
    /// The storey of the group it belongs to, if it has one: what a game hides
    /// to show the storey below.
    pub storey: Option<i32>,
    /// Which side of the envelope its faces are seen from.
    pub side: ashlar::Side,
}

/// One convex collision proxy of a spawned building, in the building's frame:
/// the points a physics crate builds a convex hull collider from.
#[derive(Component, Clone, Debug)]
pub struct AshlarCollider {
    /// The instance that placed it.
    pub instance: String,
    /// The element that declared it.
    pub element: String,
    /// The proxy's vertices, building-local metres.
    pub vertices: Vec<Vec3>,
}

/// The rooms and portals of a spawned building, in the building's frame, on
/// its [`AshlarBuilding`] entity.
#[derive(Component, Clone, Debug, Default)]
pub struct AshlarSpaces {
    /// Every declared room.
    pub rooms: Vec<ashlar::Room>,
    /// Every portal, placed once per instance of the part that marks it.
    pub portals: Vec<ashlar::Portal>,
}

/// Sent once per [`AshlarBuilding`] entity when its children have been spawned.
#[derive(Message, Clone, Copy, Debug)]
pub struct AshlarBuildingSpawned {
    /// The [`AshlarBuilding`] entity.
    pub entity: Entity,
}

/// How levels crossfade; set from [`AshlarPlugin::crossfade`] and
/// [`AshlarPlugin::bands`].
#[derive(Resource, Clone, Copy, Debug)]
pub struct AshlarCrossfade {
    /// Fraction of a boundary's distance the two levels crossfade over.
    pub crossfade: f32,
    /// How one level gives way to the next.
    pub bands: Bands,
}

/// The meshes and materials spawned buildings share.
#[derive(Resource, Default)]
pub struct AshlarCache {
    /// Meshes by building and by content, so a part a coarser level did not
    /// change is one upload however many levels draw it.
    meshes: HashMap<(AssetId<BakedBuildingAsset>, u64), Handle<Mesh>>,
    materials: BTreeMap<(AssetId<MaterialLibraryAsset>, ashlar::Binding), Handle<StandardMaterial>>,
    /// Everything a building resolves to, once per building and library pair,
    /// so the thousandth instance of a building clones handles and nothing else.
    templates: HashMap<
        (AssetId<BakedBuildingAsset>, AssetId<MaterialLibraryAsset>),
        std::sync::Arc<Spawn>,
    >,
}

impl AshlarCache {
    /// Distinct meshes uploaded so far.
    #[must_use]
    pub fn meshes(&self) -> usize {
        self.meshes.len()
    }

    /// Distinct materials created so far.
    #[must_use]
    pub fn materials(&self) -> usize {
        self.materials.len()
    }
}

/// The [`VisibilityRange`] of level `index`, or `None` for a building with one
/// level that draws at every distance.
#[must_use]
pub fn level_range(
    levels: &[ashlar::BakedLevel],
    index: usize,
    crossfade: f32,
) -> Option<VisibilityRange> {
    let margin = |distance: f32| distance * crossfade.max(0.0);
    let start = index
        .checked_sub(1)
        .and_then(|previous| levels.get(previous)?.until)
        .map_or(0.0..0.0, |distance| distance..distance + margin(distance));
    let end = match levels.get(index)?.until {
        Some(distance) => distance..distance + margin(distance),
        None if index == 0 => return None,
        None => f32::MAX..f32::MAX,
    };
    // Bevy asks that the fade in ends before the fade out starts.
    let start = start.start..start.end.min(end.start);
    Some(VisibilityRange {
        start_margin: start,
        end_margin: end,
        use_aabb: false,
    })
}

/// Everything one building spawns, resolved before anything is spawned, so a
/// failure leaves the entity untouched.
type PieceSpawn = (
    Handle<Mesh>,
    Handle<StandardMaterial>,
    Transform,
    AshlarPiece,
    Option<VisibilityRange>,
);

#[derive(Clone)]
struct Spawn {
    pieces: Vec<PieceSpawn>,
    colliders: Vec<AshlarCollider>,
    spaces: AshlarSpaces,
}

#[expect(
    clippy::too_many_arguments,
    reason = "a Bevy system's parameters are its dependencies"
)]
fn spawn_buildings(
    mut commands: Commands,
    roots: Query<(Entity, &AshlarBuilding), (Without<AshlarSpawned>, Without<AshlarFailed>)>,
    buildings: Res<Assets<BakedBuildingAsset>>,
    libraries: Res<Assets<MaterialLibraryAsset>>,
    server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut cache: ResMut<AshlarCache>,
    lod: Res<AshlarCrossfade>,
    device: Option<Res<RenderDevice>>,
    mut spawned: MessageWriter<AshlarBuildingSpawned>,
) {
    // Decided per spawn rather than once, because a browser's render device
    // arrives after startup; see `Bands::Auto`.
    let cut = lod.bands.abrupt_on(device.as_deref());
    for (entity, request) in &roots {
        for (id, what) in [
            (request.building.id().untyped(), "building"),
            (request.materials.id().untyped(), "material library"),
        ] {
            if let Some(LoadState::Failed(error)) = server.get_load_state(id) {
                let reason = format!("the {what} failed to load: {error}");
                tracing::error!("{reason}");
                commands.entity(entity).insert(AshlarFailed(reason));
            }
        }
        let (Some(building), Some(library)) = (
            buildings.get(&request.building),
            libraries.get(&request.materials),
        ) else {
            continue;
        };
        let mut cx = crate::UploadContext {
            server: &server,
            meshes: &mut meshes,
            materials: &mut materials,
        };
        let template_key = (request.building.id(), request.materials.id());
        let resolved = match cache.templates.get(&template_key) {
            Some(template) => Ok(template.clone()),
            None => resolve(
                (request.building.id(), &building.0),
                (request.materials.id(), &library.0),
                lod.crossfade,
                &mut cache,
                &mut cx,
            )
            .map(|spawn| {
                let template = std::sync::Arc::new(spawn);
                cache.templates.insert(template_key, template.clone());
                template
            }),
        };
        match resolved {
            Ok(spawn) => {
                let spawn = Spawn::clone(&spawn);
                commands.entity(entity).with_children(|parent| {
                    for (mesh, material, transform, piece, range) in spawn.pieces {
                        let mut child = parent.spawn((
                            Mesh3d(mesh),
                            MeshMaterial3d(material),
                            transform,
                            piece,
                        ));
                        if let Some(range) = range {
                            child.insert(if cut { abrupt(&range) } else { range });
                        }
                    }
                    for collider in spawn.colliders {
                        parent.spawn((collider, Transform::IDENTITY));
                    }
                });
                commands
                    .entity(entity)
                    .insert((spawn.spaces, AshlarSpawned));
                spawned.write(AshlarBuildingSpawned { entity });
            }
            Err(error) => {
                let reason = format!("{error:#}");
                tracing::error!("spawning a baked building: {reason}");
                commands.entity(entity).insert(AshlarFailed(reason));
            }
        }
    }
}

impl AshlarCache {
    /// A piece's mesh, drawn about `pivot`, uploaded once per building.
    fn mesh(
        &mut self,
        building: AssetId<BakedBuildingAsset>,
        mesh: &ashlar::TriangleMesh,
        pivot: Vec3,
        meshes: &mut Assets<Mesh>,
    ) -> Result<Handle<Mesh>> {
        let key = (building, content_hash(mesh, pivot));
        if let Some(handle) = self.meshes.get(&key) {
            return Ok(handle.clone());
        }
        let mut built = crate::mesh(mesh)?;
        if pivot != Vec3::ZERO {
            built.translate_by(-pivot);
        }
        let handle = meshes.add(built);
        self.meshes.insert(key, handle.clone());
        Ok(handle)
    }

    /// A binding's material, created once per library.
    fn material(
        &mut self,
        library: AssetId<MaterialLibraryAsset>,
        binding: &ashlar::Binding,
        definition: &ashlar::MaterialDefinition,
        cx: &mut crate::UploadContext<'_>,
    ) -> Handle<StandardMaterial> {
        let key = (library, binding.clone());
        if let Some(handle) = self.materials.get(&key) {
            return handle.clone();
        }
        let handle = cx
            .materials
            .add(crate::create_material(definition, cx.server));
        self.materials.insert(key, handle.clone());
        handle
    }
}

/// A hash of a mesh's contents and the pivot it is drawn about: two pieces
/// with the same hash are the same upload.
fn content_hash(mesh: &ashlar::TriangleMesh, pivot: Vec3) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for value in mesh
        .positions
        .iter()
        .chain(&mesh.normals)
        .flat_map(ashlar::glam::DVec3::to_array)
        .chain(mesh.uvs.iter().flatten().copied())
    {
        value.to_bits().hash(&mut hasher);
    }
    mesh.indices.hash(&mut hasher);
    pivot.to_array().map(f32::to_bits).hash(&mut hasher);
    hasher.finish()
}

/// A transform's translation and rotation as bits, for comparing two exactly.
fn transform_bits(transform: &Transform) -> [u32; 7] {
    let mut bits = [0; 7];
    for (bit, value) in bits.iter_mut().zip(
        transform
            .translation
            .to_array()
            .into_iter()
            .chain(transform.rotation.to_array()),
    ) {
        *bit = value.to_bits();
    }
    bits
}

/// Where each merged group of level zero is drawn from: the centre of its
/// bounds. A level switches on its distance from the camera to the piece's
/// origin, and a merged group's mesh is in building space, so drawn from the
/// building's own origin a large group would switch level by its distance to
/// one end.
fn group_pivots(baked: &BakedBuilding) -> HashMap<String, Vec3> {
    let Some(level) = baked.levels.first() else {
        return HashMap::new();
    };
    level
        .groups
        .iter()
        .filter_map(|group| {
            let [low, high] = group.bounds?;
            Some((group.id.clone(), ((low + high) * 0.5).as_vec3()))
        })
        .collect()
}

/// The band of a piece drawn at one level and, unchanged, at the next.
fn extend_band(
    earlier: Option<&VisibilityRange>,
    later: Option<&VisibilityRange>,
) -> Option<VisibilityRange> {
    let (earlier, later) = (earlier?, later?);
    Some(VisibilityRange {
        start_margin: earlier.start_margin.clone(),
        end_margin: later.end_margin.clone(),
        use_aabb: false,
    })
}

/// Resolve every piece, collider and space of one building, uploading meshes
/// and creating materials through the cache.
fn resolve(
    (building_id, baked): (AssetId<BakedBuildingAsset>, &BakedBuilding),
    (library_id, library): (AssetId<MaterialLibraryAsset>, &MaterialLibrary),
    crossfade: f32,
    cache: &mut AshlarCache,
    cx: &mut crate::UploadContext<'_>,
) -> Result<Spawn> {
    library
        .check_for(baked.building())
        .context("the material library does not dress this building")?;
    let pivots = group_pivots(baked);
    let mut pieces: Vec<PieceSpawn> = Vec::new();
    // The pieces of the level before, by what makes two pieces the same
    // drawing: one that did not change at this level extends that entity's
    // band rather than spawning a twin.
    let mut previous: HashMap<(AssetId<Mesh>, AssetId<StandardMaterial>, [u32; 7], String), usize> =
        HashMap::new();
    for index in 0..baked.levels.len() {
        let range = level_range(&baked.levels, index, crossfade);
        let meshed = baked.level(index).context("a level in range")?;
        let mut current = HashMap::new();
        for surface in crate::dressed_pieces(&meshed, library) {
            let crate::DressedPiece { piece, definition } = surface?;
            let label = piece.label();
            crate::files_only(&definition, piece.binding, &label)?;
            if piece.mesh.positions.is_empty() {
                continue;
            }
            let pivot = match piece.origin {
                ashlar::PieceOrigin::Group { group, .. } => {
                    pivots.get(group).copied().unwrap_or(Vec3::ZERO)
                }
                _ => Vec3::ZERO,
            };
            let mesh = cache
                .mesh(building_id, piece.mesh, pivot, cx.meshes)
                .with_context(|| format!("uploading {label}"))?;
            let material = cache.material(library_id, piece.binding, &definition, cx);
            let rotation = piece.pose.rotation.as_quat();
            let transform = Transform {
                translation: piece.pose.translation.as_vec3() + rotation * pivot,
                rotation,
                ..default()
            };
            let same = (
                mesh.id(),
                material.id(),
                transform_bits(&transform),
                label.clone(),
            );
            if let Some(&at) = previous.get(&same) {
                // Drawn unchanged at this level too: the entity's band runs on.
                pieces[at].4 = extend_band(pieces[at].4.as_ref(), range.as_ref());
                current.insert(same, at);
                continue;
            }
            current.insert(same, pieces.len());
            pieces.push((
                mesh,
                material,
                transform,
                AshlarPiece {
                    level: index,
                    label,
                    storey: piece.storey,
                    side: piece.side,
                },
                range.clone(),
            ));
        }
        previous = current;
    }
    let level_zero = baked.level(0).context("a baked building has level zero")?;
    let colliders = level_zero
        .colliders()
        .into_iter()
        .map(|collider| AshlarCollider {
            instance: collider.instance,
            element: collider.element,
            vertices: collider
                .solid
                .vertices
                .iter()
                .map(|v| v.as_vec3())
                .collect(),
        })
        .collect();
    let spaces = AshlarSpaces {
        rooms: baked.building().recipe().rooms.clone(),
        portals: level_zero.portals(),
    };
    Ok(Spawn {
        pieces,
        colliders,
        spaces,
    })
}

/// Keep spawned buildings in step with their files.
///
/// A file that changed on disk respawns what wears it: its children go, its
/// cache entries go, and the spawn system builds it again next frame. A file
/// that finished loading clears an earlier failure on what asks for it, so a
/// game started before its content step ran recovers when the files arrive.
/// An asset nothing holds any more takes its cache entries with it.
fn reload_buildings(
    mut commands: Commands,
    mut building_events: MessageReader<AssetEvent<BakedBuildingAsset>>,
    mut library_events: MessageReader<AssetEvent<MaterialLibraryAsset>>,
    roots: Query<(
        Entity,
        &AshlarBuilding,
        Option<&Children>,
        Has<AshlarFailed>,
    )>,
    colliders: Query<(), Or<(With<AshlarPiece>, With<AshlarCollider>)>>,
    mut cache: ResMut<AshlarCache>,
) {
    let buildings = AssetChanges::read(&mut building_events);
    let libraries = AssetChanges::read(&mut library_events);
    if buildings.is_empty() && libraries.is_empty() {
        return;
    }
    cache
        .meshes
        .retain(|(id, _), _| !buildings.stale.contains(id));
    cache
        .materials
        .retain(|(id, _), _| !libraries.stale.contains(id));
    cache.templates.retain(|(building, library), _| {
        !buildings.stale.contains(building) && !libraries.stale.contains(library)
    });
    for (entity, request, children, failed) in &roots {
        let building = request.building.id();
        let materials = request.materials.id();
        if buildings.modified.contains(&building) || libraries.modified.contains(&materials) {
            for child in children.into_iter().flatten() {
                if colliders.contains(*child) {
                    commands.entity(*child).despawn();
                }
            }
            commands
                .entity(entity)
                .remove::<(AshlarSpawned, AshlarSpaces, AshlarFailed)>();
        } else if failed
            && (buildings.loaded.contains(&building) || libraries.loaded.contains(&materials))
        {
            commands.entity(entity).remove::<AshlarFailed>();
        }
    }
}

/// What one frame's asset events say about one asset type.
struct AssetChanges<A: Asset> {
    /// Changed on disk: respawn what wears it.
    modified: Vec<AssetId<A>>,
    /// Finished loading: retry what failed on it.
    loaded: Vec<AssetId<A>>,
    /// Changed or no longer held: its cache entries are stale.
    stale: Vec<AssetId<A>>,
}

impl<A: Asset> AssetChanges<A> {
    fn read(events: &mut MessageReader<AssetEvent<A>>) -> Self {
        let mut changes = Self {
            modified: Vec::new(),
            loaded: Vec::new(),
            stale: Vec::new(),
        };
        for event in events.read() {
            match *event {
                AssetEvent::Modified { id } => {
                    changes.modified.push(id);
                    changes.stale.push(id);
                }
                AssetEvent::LoadedWithDependencies { id } => changes.loaded.push(id),
                AssetEvent::Unused { id } | AssetEvent::Removed { id } => changes.stale.push(id),
                AssetEvent::Added { .. } => {}
            }
        }
        changes
    }

    fn is_empty(&self) -> bool {
        self.modified.is_empty() && self.loaded.is_empty() && self.stale.is_empty()
    }
}
