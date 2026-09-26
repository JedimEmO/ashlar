//! The interactive orbit viewer, and the scene setup the gallery reuses.
use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use ashlar::BuildingRecipe;
use ashlar::MeshedBuilding;
use ashlar_bevy as materials;
use ashlar_manifold::{ManifoldMesher, mesh_building};
use ashlar_material::MaterialGraphLibrary;
use bevy::{
    ecs::{error::Result as BevyResult, system::SystemParam},
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll},
    pbr::wireframe::{WireframeConfig, WireframePlugin},
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
};

use ashlar_material::partition::CostReport;
use materials::shader::{ProceduralMaterial, ProceduralMaterialPlugin};

use crate::{
    Catalog, Options, blast,
    panel::{self, Panel, PointerGrab, PressOrigin},
    reload::{self, GraphSource, Rebake, Redress, Reloaded},
};

#[derive(Component)]
pub(crate) struct PreviewObject;

/// The storey a spawned piece belongs to, if its group declares one.
///
/// Copied off `Piece::storey` at spawn so culling is a component query rather
/// than a walk back into the meshed building every frame.
#[derive(Component)]
pub(crate) struct PieceStorey(pub(crate) Option<i32>);

/// Which side of the envelope a spawned piece is seen from.
#[derive(Component)]
pub(crate) struct PieceSide(pub(crate) ashlar::Side);

/// The merge group a spawned group piece belongs to.
///
/// A blast answers with one re-meshed group, and the pieces it replaces are
/// found by this rather than by walking the `Preview`: despawning the group's
/// entities is one query, and it disposes of exactly the batches that group had
/// before, not the standalone elements beside it.
#[derive(Component)]
pub(crate) struct PieceGroup(pub(crate) String);

/// The standalone element a spawned piece came from.
///
/// Damage cuts merged groups only, so a blast can never remove an element piece
/// through the group path; the preview keeps the element's identity on the
/// entity so it can make the game's decision and despawn the piece the ball
/// engulfs. A piece with this component is never in [`PieceGroup`].
#[derive(Component)]
pub(crate) struct PieceElement {
    /// Placement the element belongs to.
    pub(crate) instance: String,
    /// Element within that placement's part.
    pub(crate) element: String,
}

/// The policy a top-down game would write in a few lines: hide the storeys
/// above a cut, and hide the outside to see the inside.
///
/// The library classifies and culls nothing; both of these fields are read by
/// [`apply_cull`] alone, and `max_storey` is `None` until a key or an option
/// sets it.
#[derive(Resource, Default)]
pub(crate) struct Cull {
    /// Hide every piece whose storey is above this one.
    pub(crate) max_storey: Option<i32>,
    /// Hide every piece seen from outside the envelope.
    pub(crate) hide_exterior: bool,
}

/// The material key an object was spawned with.
///
/// What a click has to land on to be worth anything: picking answers with an
/// entity, and the parameter panel needs the graph behind that entity's
/// surface. Carried on the object rather than looked up through its material
/// handle, because two keys can share a handle and only one of them is the
/// wall that was clicked.
#[derive(Component)]
pub(crate) struct SurfaceKey(pub String);

#[derive(Resource)]
pub(crate) struct Preview(pub MeshedBuilding);

#[derive(Resource)]
pub(crate) struct Orbit {
    yaw: f32,
    pitch: f32,
    distance: f32,
    initial_distance: f32,
    initial_yaw: f32,
    initial_pitch: f32,
    target: Vec3,
    initial_target: Vec3,
}

/// Every material the preview made, keyed by the binding that asked for it.
///
/// The key is a whole [`ashlar::Binding`] rather than a material key, because
/// an instance may put its own values on the graph parameters of the surface it
/// binds: two walls over one key at two seeds are two materials and have to be
/// two entries, and two walls over one key at one seed are one entry however
/// many instances ask for it. A binding that overrides nothing compares equal
/// to every other binding of that key, so a scene with no overrides in it keys
/// by the key it always did.
#[derive(Resource, Default)]
pub(crate) struct Palette {
    pub alternate: bool,
    pub handles: BTreeMap<ashlar::Binding, Handle<StandardMaterial>>,
    pub originals: BTreeMap<ashlar::Binding, StandardMaterial>,
    /// The compiled surfaces, which are a different asset type entirely.
    ///
    /// A `Surface::Shader` is a `StandardMaterial` with a generated fragment
    /// over it, so it is a [`ProceduralMaterial`] and not in `handles` at all.
    /// A key is in exactly one of the two maps, and [`setup`] spawns whichever
    /// it is in.
    ///
    /// Clay mode leaves these alone: the point of a live surface is the thing
    /// clay mode hides, and swapping it for a grey constant would mean a second
    /// pipeline for every graph. `P` therefore turns the walls grey and leaves
    /// the light strip pulsing, which is the honest picture of what is
    /// compiled and what is not.
    pub shaders: BTreeMap<ashlar::Binding, Handle<ProceduralMaterial>>,
}

/// Where the model is put, and where the camera looks once it is there.
///
/// A preview holds the model near the origin: a building is authored in
/// whatever coordinates its recipe uses, and `f32` renderer coordinates a
/// kilometre from the origin are coordinates with centimetres missing. What
/// that recentring must *not* do is move the model up or down, and it used to:
/// subtracting the bounds centre put a four-metre bay's floor two metres
/// underground and a fifteen-metre office's floor seven. Nothing could see the
/// difference until a material asked where the fragment was — a
/// [`WorldMask`](ashlar_material::nodes::WorldMask) on the faces rain reaches
/// answers a different question on every scene when the ground is at a
/// different height in each.
///
/// So the origin is the bounds centre in x and z and the model's own **floor**
/// in y: world y is height above the ground, in metres, in every scene. The
/// camera looks at [`Self::target`] instead of at the origin, so the framing is
/// the framing it always was.
#[derive(Resource)]
pub(crate) struct Center {
    /// The point the camera frames, in the model's own coordinates.
    pub model: bevy::math::DVec3,
    /// The model-space point that becomes the renderer's origin.
    pub origin: bevy::math::DVec3,
}

impl Center {
    /// The framing centre and the origin, from the model's own bounds.
    pub(crate) fn of(min: bevy::math::DVec3, max: bevy::math::DVec3) -> Self {
        let model = (min + max) * 0.5;
        Self {
            model,
            origin: bevy::math::DVec3::new(model.x, min.y, model.z),
        }
    }

    /// Where the camera looks, in renderer coordinates: straight up from the
    /// origin by half the model's height.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a renderer coordinate, which is where f64 becomes f32"
    )]
    pub(crate) fn target(&self) -> Vec3 {
        (self.model - self.origin).as_vec3()
    }
}

/// The material library and the graph library beside it, both validated.
///
/// A path given on the command line wins; otherwise a catalog scene brings its
/// own, and a hand-written `--recipe` brings neither and gets the diagnostic
/// colours. The graph library is read whether or not a surface names one, so a
/// scene that ships graphs has them validated at startup, which is where a
/// graph that stopped building should be noticed.
fn preflight(
    options: &Options,
    scene: Option<&crate::Scene>,
    building: &ashlar::Building,
) -> Result<(
    crate::Definitions,
    crate::Graphs,
    Option<std::path::PathBuf>,
)> {
    // A hand-written `--recipe` brings no scene, and so no libraries of its own.
    let scene = scene.filter(|_| options.recipe.is_none());
    let (graphs, source) = match (&options.graphs, scene) {
        (Some(path), _) => (materials::read_graphs(path)?, Some(path.clone())),
        (None, Some(scene)) => scene.load_graphs(&options.asset_root)?,
        (None, None) => (MaterialGraphLibrary::default(), None),
    };
    let mut loaded = match (&options.materials, scene) {
        (Some(path), _) => Some(materials::read_library_with_graphs(
            path,
            &options.asset_root,
            building,
            &graphs,
        )?),
        (None, Some(scene)) => scene.load_materials(&options.asset_root, building, &graphs)?,
        (None, None) => None,
    };
    if options.bake
        && let Some(loaded) = loaded.as_mut()
    {
        bake_recorded(loaded, &graphs)?;
    }
    Ok((crate::Definitions(loaded), crate::Graphs(graphs), source))
}

/// Turn every surface that records a bake into one, so the preview renders the
/// graphs rather than the files a content step wrote from them.
///
/// The study ships `Surface::Files` with `baked_from` beside it, and should:
/// a whole level shares those maps and loading four KTX2 files beats baking
/// them. That is also what makes the graph library inert in a preview — an edit
/// changes nothing on screen, because nothing on screen came from the graph.
/// `--bake` is the switch between the two, and it is one field per surface
/// because `baked_from` already holds the graph, the parameters and the
/// resolution the files were written from.
///
/// Each flip is preflighted the way `read` preflights a surface that was already
/// `Graph`: `baked_from` is provenance rather than a dependency, so nothing has
/// lowered it, and a library that shipped its textures long after its graphs
/// moved on should say which graph stopped compiling before the window opens
/// rather than fail inside the first bake.
fn bake_recorded(
    library: &mut ashlar::MaterialLibrary,
    graphs: &MaterialGraphLibrary,
) -> Result<()> {
    for (key, definition) in &mut library.materials {
        let ashlar::Surface::Files {
            baked_from: Some(bake),
            ..
        } = &definition.surface
        else {
            continue;
        };
        let graph = graphs.get(&bake.graph).with_context(|| {
            format!(
                "material {key} was baked from unknown graph {:?}",
                bake.graph
            )
        })?;
        ashlar_material::bake::preflight(&ashlar_material::bake::BakeRequest {
            graph,
            library: graphs,
            params: &bake.params,
            resolution: bake.resolution,
            mips: false,
            threads: None,
        })
        .with_context(|| format!("material {key} baking graph {:?}", bake.graph))?;
        definition.surface = ashlar::Surface::Graph(bake.clone());
    }
    Ok(())
}

/// Everything the command line asks for that is not the gallery.
#[expect(
    clippy::too_many_lines,
    reason = "one function that assembles the app, and every line sets a resource or a plugin"
)]
pub(crate) fn run(catalog: &Catalog, options: Options) -> Result<()> {
    ensure!(
        options.yaw.is_finite()
            && options.pitch.is_finite()
            && options.pitch.abs() < 1.4
            && options.zoom.is_finite()
            // A focus stands the camera in a street, a small fraction of a
            // city's diagonal from what it looks at.
            && options.zoom > if options.focus.is_some() { 0.002 } else { 0.05 }
            && options.zoom < 20.0,
        "invalid camera settings"
    );
    let scene = match &options.scene {
        Some(name) => Some(catalog.require(name)?),
        None => catalog
            .default_name()
            .map(|name| catalog.require(name))
            .transpose()?,
    };
    if let Some(path) = &options.write_example {
        let scene = scene.context("no scene to export; the catalog is empty")?;
        let text =
            ron::ser::to_string_pretty(scene.build()?.recipe(), ron::ser::PrettyConfig::default())?;
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
        return Ok(());
    }
    let building = if let Some(path) = &options.recipe {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        ron::from_str::<BuildingRecipe>(&text)
            .context("parsing building recipe")?
            .build()?
    } else {
        scene
            .context("no scene to preview; pass --recipe or register a scene")?
            .build()?
    };
    let (mut definitions, graphs, graph_source) = preflight(&options, scene, &building)?;
    if !options.night {
        by_day(&mut definitions);
    }
    let source = GraphSource::new(graph_source, options.graphs_out.clone());
    let asset_root = options.asset_root.to_string_lossy().into_owned();
    let (yaw, pitch, clay) = (options.yaw, options.pitch, options.clay);
    let meshed = mesh_building(&building, &ManifoldMesher::default())?;
    // Click-to-blast keeps a second, unioned copy of the scene, which re-unions
    // it once at startup. That is accepted here: the preview already pays one
    // mesh for the whole scene, and a hit that subtracts from what is already
    // unioned is the point of the fast path.
    let blast = blast::prepare(&building, &options)?;
    let (center, distance) = framing(&meshed, VIEWER_FRAMING)?;
    let distance = distance * options.zoom;
    // A focus is building space, as a blast centre is; the renderer's origin
    // is the model's footprint centre, so it moves by that.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a renderer coordinate, which is where f64 becomes f32"
    )]
    let target = options.focus.map_or_else(
        || center.target(),
        |focus| (bevy::math::DVec3::from_array(focus.0) - center.origin).as_vec3(),
    );
    let triangles = unique_triangles(&meshed);
    println!(
        "{}: {} shared parts, {} instances, {triangles} unique triangles",
        building.recipe().id,
        meshed.parts.len(),
        building.recipe().instances.len()
    );
    println!(
        "Drag left: orbit | drag right: pan | wheel: zoom | F: wireframe | P: materials/clay | \
         M: parameter panel | S: save graphs | Home: reset | Esc: exit"
    );
    if let Some(path) = source.path.as_deref() {
        println!("Watching {} for changes", path.display());
    }
    let cull = Cull {
        max_storey: options.max_storey,
        hide_exterior: options.hide_exterior,
    };
    let wireframe = options.wireframe;
    let serial_pipelines = options.serial_pipelines;
    let rig = Rig::new(options.night, options.key, options.ambient);
    let mut app = App::new();
    app.insert_resource(options)
        .insert_resource(cull)
        .insert_resource(definitions)
        .insert_resource(graphs)
        // The two caches the tool half and the game half keep, side by side.
        .init_resource::<materials::runtime_bake::BakeCache>().init_resource::<materials::cards::CardCache>()
        .insert_resource(source)
        .insert_resource(Preview(meshed))
        .insert_resource(center)
        .insert_resource(Orbit { yaw, pitch, distance, initial_distance: distance, initial_yaw: yaw, initial_pitch: pitch, target, initial_target: target })
        .insert_resource(Palette { alternate: clay, ..default() })
        .insert_resource(rig.key)
        .insert_resource(Night(rig.night))
        .insert_resource(ClearColor(rig.sky))
        .insert_resource(GlobalAmbientLight { brightness: rig.ambient, ..default() })
        .add_plugins(DefaultPlugins.set(bevy::render::RenderPlugin { synchronous_pipeline_compilation: serial_pipelines, ..default() }).set(AssetPlugin { file_path: asset_root, ..default() }).set(WindowPlugin {
            primary_window: Some(Window { title: "Building recipes | drag: orbit / pan | wheel: zoom | F: wireframe | P: materials/clay | M: panel | Home: reset".into(),
                resolution: (1280, 900).into(), present_mode: bevy::window::PresentMode::AutoVsync, ..default() }), ..default()
        }).set(bevy::app::TaskPoolPlugin { task_pool_options: bevy::app::TaskPoolOptions::with_num_threads(4) }))
        .add_plugins(WireframePlugin::default())
        // Clicking a wall is how the panel is pointed at a graph, and mesh
        // picking is not in `DefaultPlugins`: it ray casts on the CPU, which a
        // game usually wants to opt into. A preview is a few thousand triangles
        // behind an AABB test, and it is the thing being looked at.
        .add_plugins(bevy::picking::mesh_picking::MeshPickingPlugin)
        // A `Surface::Shader` draws through a second material type, and the
        // plugin is what registers its asset, its pipeline and its prepass.
        .add_plugins((
            ProceduralMaterialPlugin,
            materials::gpu::GpuBakePlugin,
            // A strand layer draws through an `ExtendedMaterial`, which is
            // a third asset type with a third pipeline; the plugin is what
            // registers it, its two vertex stages and the wind resource.
            materials::wind::StrandPlugin,
        ))
        .insert_resource(WireframeConfig { global: wireframe, ..default() })
        .add_systems(
            Startup,
            (
                setup_materials,
                blast::prepare_material,
                setup,
                setup_night_lights,
                setup_strands,
            )
                .chain(),
        )
        // `apply_cull` reads what `appearance` wrote this frame, so it runs
        // after it rather than a frame late.
        .add_systems(Update, (controls, appearance, apply_cull.after(appearance), capture));
    if let Some(blast) = blast {
        app.insert_resource(blast);
    }
    authoring(&mut app);
    let result = app.run();
    ensure!(
        matches!(result, AppExit::Success),
        "preview failed; see asset/render diagnostics"
    );
    Ok(())
}

/// The authoring loop, registered on the interactive viewer alone.
///
/// The gallery renders a scene and exits, so nothing there watches a file or
/// draws a slider; keeping this out of the shared setup is what keeps the
/// gallery's frame budget the renderer's.
fn authoring(app: &mut App) -> &mut App {
    app.init_resource::<Rebake>()
        .init_resource::<Redress>()
        .init_resource::<Reloaded>()
        .init_resource::<Panel>()
        .init_resource::<PressOrigin>()
        .init_resource::<blast::BlastPress>()
        .add_observer(panel::pressed)
        .add_observer(panel::picked)
        .add_observer(panel::dragged)
        .add_observer(panel::toggled)
        .add_observer(panel::activated)
        // Blasting is an authoring tool, not a capture one: a hit takes tens of
        // milliseconds, so it belongs where a person is watching, beside the
        // panel. `BlastState` exists only for a merged scene, so everything
        // here is inert on any other.
        .add_observer(blast::pressed)
        .add_observer(blast::clicked)
        .add_systems(
            Update,
            (
                panel::toggle,
                panel::shortcuts,
                (reload::watch, reload::rebake, reload::redress).chain(),
                (panel::follow_reload, panel::rebuild).chain(),
                panel::thumbs,
                panel::readouts,
                (blast::resize, blast::drive_blasts, blast::fall),
            ),
        )
}

/// How far outside its own diagonal a camera starts. The gallery frames a
/// little tighter than the interactive viewer, which leaves the first orbit
/// drag somewhere to go.
pub(crate) const GALLERY_FRAMING: f32 = 1.4;
/// The interactive viewer's starting distance, before `--zoom`.
pub(crate) const VIEWER_FRAMING: f32 = 1.45;

/// Where a camera looks and how far off it starts, from the building's extent.
/// Both the viewer and the gallery frame a scene this way, so a screenshot and
/// its reference image are the same shot at different distances.
pub(crate) fn framing(preview: &MeshedBuilding, factor: f32) -> Result<(Center, f32)> {
    let (min, max) = bounds(preview)?;
    Ok((
        Center::of(min, max),
        (max - min).as_vec3().length() * factor,
    ))
}

pub(crate) fn bounds(preview: &MeshedBuilding) -> Result<(bevy::math::DVec3, bevy::math::DVec3)> {
    let mut min = bevy::math::DVec3::splat(f64::INFINITY);
    let mut max = -min;
    for piece in preview.pieces() {
        for point in &piece.mesh.positions {
            let point = piece.pose.transform_point(*point);
            ensure!(
                point.is_finite() && point.abs().max_element() < 1e6,
                "preview supports local coordinates within one million metres"
            );
            min = min.min(point);
            max = max.max(point);
        }
    }
    ensure!(
        min.is_finite() && max.is_finite(),
        "recipe contains no visible geometry"
    );
    Ok((min, max))
}

/// The unique triangles a meshed building draws: the shared element meshes plus
/// the merged group batches, which a walk over `parts` alone would miss. A
/// batch is welded, so only `triangle_count` knows its faces.
fn unique_triangles(meshed: &MeshedBuilding) -> usize {
    meshed
        .parts
        .values()
        .flatten()
        .map(|element| element.mesh.triangle_count())
        .chain(
            meshed
                .groups
                .iter()
                .flat_map(|group| &group.batches)
                .map(|batch| batch.mesh.triangle_count()),
        )
        .sum()
}

/// What a `Surface::Graph` needs beyond the definitions: the graph library it
/// names into, the cache that keeps one texture set per distinct bake, and
/// somewhere to put a new one.
///
/// One system parameter rather than three, because they are always taken
/// together and `setup_materials` already holds as many resources as a system
/// should.
#[derive(SystemParam)]
pub(crate) struct Baking<'w> {
    graphs: Res<'w, crate::Graphs>,
    cache: ResMut<'w, materials::runtime_bake::BakeCache>,
    /// Where a strand layer's card atlas is found and remembered.
    ///
    /// Beside the bake cache rather than in it: the atlases are the game half's
    /// and the bakes are the graph engine's, and the preview is the one place
    /// that holds both.
    atlases: ResMut<'w, materials::cards::CardCache>,
    images: ResMut<'w, Assets<Image>>,
    threads: Option<Res<'w, BakeThreads>>,
    /// The device to bake on, where the app has one.
    ///
    /// `Option` because a headless test app on `MinimalPlugins` has no
    /// renderer, and because the CPU bake is what those tests are checking
    /// against anyway. A windowed preview and the reference gallery both have
    /// one, and both want it: a compiled surface's static half is the whole of
    /// its picture, and it is the second and a half the preview otherwise
    /// spends in front of its first frame.
    gpu: Option<Res<'w, materials::gpu::GpuBaker>>,
}

/// What growing a strand layer needs beyond [`Baking`]: somewhere to put the
/// chunk meshes, and the two material types a layer draws through.
///
/// Two of them, because a card level is a plain `StandardMaterial` and a strand
/// level is the wind extension over one — different asset types, and a system
/// that spawns both holds both.
#[derive(SystemParam)]
pub(crate) struct Growing<'w> {
    meshes: ResMut<'w, Assets<Mesh>>,
    strands: ResMut<'w, Assets<materials::wind::StrandWindMaterial>>,
    cards: ResMut<'w, Assets<StandardMaterial>>,
}

/// What a `Surface::Shader` needs beyond [`Baking`]: somewhere to put a
/// generated shader, and the registry that says which graphs already have one.
///
/// A second system parameter rather than two more fields on the first, because
/// [`reload`](crate::reload) takes `Baking` and re-bakes surfaces without ever
/// compiling one.
#[derive(SystemParam)]
pub(crate) struct Compiling<'w> {
    shaders: ResMut<'w, Assets<bevy::shader::Shader>>,
    registry: ResMut<'w, materials::shader::GraphShaders>,
    compiled: ResMut<'w, Assets<ProceduralMaterial>>,
}

impl Compiling<'_> {
    /// Forget every compiled graph.
    ///
    /// The registry's keys name a graph rather than describing it — a
    /// [`ShaderKey`](materials::runtime_bake::ShaderKey) is the graph's key, its
    /// resolution, its live names and its folded values, and not one word of
    /// what its nodes do — so after a library was edited in place every entry
    /// claims to be something it is not, and a re-registration would answer the
    /// shader it already had. Exactly the reason
    /// [`Baking::clear_cache`] exists, for exactly the same kind of key.
    pub(crate) fn forget(&mut self) {
        self.registry.clear(&mut self.shaders);
    }

    /// The compiled materials, to write a recompiled or re-uniformed one into.
    pub(crate) fn assets(&mut self) -> &mut Assets<ProceduralMaterial> {
        &mut self.compiled
    }

    /// What a compiled graph cost, for the line the preview prints.
    pub(crate) fn report(&self, key: materials::shader::GraphKey) -> Option<&CostReport> {
        self.registry.get(key).map(|entry| &entry.report)
    }
}

/// How many threads a bake divides its rows across when nothing has said.
///
/// Eight rather than one per core. The bytes do not depend on the division —
/// that is a claim `ashlar-material` pins — and a debug binary baking several
/// megatexel surfaces back to back across thirty-two threads is the one thing
/// in this workspace that has ever been seen to fall over, at about one run in
/// three on this machine. Eight keeps nearly all of the speed-up and has not.
pub(crate) const BAKE_THREADS: Option<std::num::NonZeroUsize> = std::num::NonZeroUsize::new(8);

/// An explicit thread budget for a bake, overriding [`BAKE_THREADS`].
///
/// `BakeThreads(None)` means one per core. A test inserts a small number
/// instead, because a test binary is already running its cases across every
/// core.
#[derive(Resource, Default)]
pub(crate) struct BakeThreads(pub Option<std::num::NonZeroUsize>);

impl Baking<'_> {
    /// Forget every baked set and every card atlas drawn from one.
    ///
    /// What a library edited in place needs: the cache keys name graphs rather
    /// than describing them, so after an edit every entry claims to be
    /// something it no longer is. A card atlas is keyed by the same names, and
    /// kept past an edit it would draw the old blades at a distance.
    pub(crate) fn clear_cache(&mut self) {
        self.cache.clear();
        self.atlases.clear();
    }

    /// The borrow bundle `create_graph_material` takes.
    pub(crate) fn context(&mut self) -> materials::runtime_bake::BakeContext<'_> {
        let baker = materials::runtime_bake::Baker::or_cpu(self.gpu.as_deref());
        materials::runtime_bake::BakeContext {
            graphs: &self.graphs.0,
            cache: &mut self.cache,
            images: &mut self.images,
            threads: self
                .threads
                .as_ref()
                .map_or(BAKE_THREADS, |budget| budget.0),
            baker,
        }
    }

    /// The borrow bundle `scatter_sets` takes.
    ///
    /// The graph library and the cache the bake already uses. The preview is
    /// the tool half of the split: a game reads a baked set off disk, and this
    /// scatters one out of the graph the author is editing, so that a
    /// definition edited in place grows the lawn it now describes.
    pub(crate) fn scatter_context(&mut self) -> materials::strands::ScatterContext<'_> {
        materials::strands::ScatterContext::new(&self.graphs.0, &mut self.cache)
            .threads(STRAND_THREADS)
    }

    /// The borrow bundle `create_strands` takes.
    ///
    /// The game half, and it holds none of the graph: a card level binds an
    /// atlas, so a layer that grows one has to put two pictures somewhere and
    /// remember that it did.
    pub(crate) fn strand_context(&mut self) -> materials::strands::StrandContext<'_> {
        materials::strands::StrandContext::new(&mut self.images, &mut self.atlases)
    }

    /// The borrow bundle `create_shader_material` takes, over the same three resources
    /// plus the two a compiled graph needs.
    pub(crate) fn shader_context<'a>(
        &'a mut self,
        compiling: &'a mut Compiling<'_>,
    ) -> materials::shader::ShaderContext<'a> {
        let baker = materials::runtime_bake::Baker::or_cpu(self.gpu.as_deref());
        materials::shader::ShaderContext {
            graphs: &self.graphs.0,
            cache: &mut self.cache,
            images: &mut self.images,
            shaders: &mut compiling.shaders,
            registry: &mut compiling.registry,
            // The preview shows what a game would ship, and a game would ship
            // the default.
            resolution: None,
            threads: self
                .threads
                .as_ref()
                .map_or(BAKE_THREADS, |budget| budget.0),
            baker,
        }
    }
}

/// Create one `StandardMaterial` per bound material key.
///
/// Fallible because a `Surface::Graph` is baked here, on the main thread and in
/// front of the first frame: a fifth of a second of CPU per distinct 1024 set,
/// which is what a preview should pay to show the thing it is previewing rather
/// than start faster and show it late. Everything that can
/// go wrong with a graph was already refused by `preflight`, so a failure here
/// is a bug rather than bad content, and it says so rather than rendering a
/// diagnostic colour that looks like a material choice.
pub(crate) fn setup_materials(
    preview: Res<Preview>,
    definitions: Res<crate::Definitions>,
    server: Res<AssetServer>,
    mut baking: Baking,
    mut compiling: Compiling,
    mut palette: ResMut<Palette>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) -> BevyResult {
    for piece in preview.0.pieces() {
        palette_entry(
            piece.binding,
            &definitions,
            &server,
            &mut baking,
            &mut compiling,
            &mut palette,
            &mut materials,
        )?;
    }
    Ok(())
}

/// Create the palette entry for one binding, if it has none yet.
///
/// The one place a binding becomes a material. [`setup_materials`] calls it for
/// every binding the scene draws; the blast path calls it for the damage slot,
/// because the slot's binding is in the building palette whether or not any
/// piece on the scene ever wore it, and a blast's exposed faces do.
///
/// The binding's own parameter values are written into the definition here and
/// nowhere else: what a bake, a partition and the two caches below see is a
/// definition, so an override is a distinct texture set or a distinct uniform
/// block rather than a case any of them has to know about.
///
/// Fallible because a `Surface::Graph` is baked here, on the main thread:
/// a fifth of a second of CPU per distinct 1024 set, which is what a preview
/// should pay to show the thing it is previewing rather than start faster and
/// show it late. Everything that can go wrong with a graph was already refused
/// by `preflight`, so a failure here is a bug rather than bad content, and it
/// says so rather than rendering a diagnostic colour that looks like a material
/// choice.
pub(crate) fn palette_entry(
    binding: &ashlar::Binding,
    definitions: &crate::Definitions,
    server: &AssetServer,
    baking: &mut Baking,
    compiling: &mut Compiling,
    palette: &mut Palette,
    materials: &mut Assets<StandardMaterial>,
) -> BevyResult {
    let key = binding.material.as_str();
    if palette.handles.contains_key(binding) || palette.shaders.contains_key(binding) {
        return Ok(());
    }
    let definition = definitions
        .0
        .as_ref()
        .map(|library| materials::definition(library, binding))
        .transpose()
        .with_context(|| format!("material {key}"))?;
    // A compiled surface is a different asset type, so it forks here rather
    // than inside `create_graph_material`. Everything else — constants, files, a
    // runtime bake — is a `StandardMaterial` and is not told apart at all.
    if let Some(definition) = definition.as_deref()
        && matches!(definition.surface, ashlar::Surface::Shader { .. })
    {
        let started = std::time::Instant::now();
        let material = materials::shader::create_shader_material(
            definition,
            &mut baking.shader_context(compiling),
        )
        .with_context(|| format!("material {key}"))?;
        // What the graph costs per fragment, printed where a graph is being
        // authored: the design's "the cost is seen when the graph is written
        // and not when it ships". The elapsed time beside it is the *other*
        // cost and the one an author feels — a compiled surface bakes its
        // static half on this thread, in front of the first frame, exactly as a
        // `Surface::Graph` does.
        if let Some(report) = compiling.report(material.extension.graph) {
            println!(
                "{key} compiled in {:.0} ms: {report}",
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
        let handle = compiling.compiled.add(material);
        palette.shaders.insert(binding.clone(), handle);
        return Ok(());
    }
    let original = match definition.as_deref() {
        None => material(key, false),
        Some(definition) => materials::runtime_bake::create_graph_material(
            definition,
            server,
            &mut baking.context(),
        )
        .with_context(|| format!("material {key}"))?,
    };
    let visible = if palette.alternate {
        material(key, true)
    } else {
        original.clone()
    };
    palette.originals.insert(binding.clone(), original);
    palette
        .handles
        .insert(binding.clone(), materials.add(visible));
    Ok(())
}

/// How bright the key light is, in lux.
///
/// A resource rather than a constant because a capture of a *material* wants a
/// different one from a capture of a building, and for the reason
/// [`Options::reference_ambient`](crate::Options::reference_ambient) already
/// gives about the fill: the rig here frames whole buildings, and at the scale
/// of an eighteen-millimetre pile its key washes every recess out. The material
/// studio the swatches are rendered under uses 6500, and a close capture meant
/// to be compared with one of those should say so.
///
/// [`Self::default`] is the building rig's own, so a viewer or a gallery run
/// that says nothing renders what it always rendered.
#[derive(Resource, Clone, Copy, Debug)]
pub(crate) struct KeyLight(pub f32);

impl Default for KeyLight {
    fn default() -> Self {
        Self(14000.0)
    }
}

/// The night rig's key, in lux: a cold moon.
///
/// Far brighter than a real moon's third of a lux, because the preview's
/// camera keeps Bevy's default exposure (EV100 9.7) by day and by night, and
/// what a night street looks like is the eye having adapted: at that exposure
/// a real moon renders black. What matters is the ratio to the emissive
/// surfaces, which the exposure does not touch — a `StandardMaterial`'s
/// `emissive` is added after the exposure, in display units — so at this key
/// a dark wall is a dim silhouette and a lit window of emissive 1 is a
/// highlight, which is what a city by night is.
pub(crate) const NIGHT_KEY: f32 = 600.0;

/// The fill by night, as a share of the key: the sky glow over a city, from
/// the other side and bluer. By day it is 2500 / 14 000.
const NIGHT_FILL: f32 = 0.12;

/// The ambient by night: enough that a wall in shadow is not a hole.
pub(crate) const NIGHT_AMBIENT: f32 = 30.0;

/// The sky by night: blue-black, not black, so a roofline reads against it.
pub(crate) const NIGHT_SKY: Color = Color::srgb(0.012, 0.016, 0.03);

/// The sky by day.
pub(crate) const DAY_SKY: Color = Color::srgb(0.075, 0.09, 0.115);

/// How much of a definition's emission shows by day.
///
/// A library's emissive values are what a lit window, a sign or a lamp gives
/// out, and they are tuned by night, where they are the picture. Against the
/// sun the same numbers turn every lit office into an orange panel and every
/// tube into neon, which is not what a lit room looks like at noon: the light
/// is there but the day outshines it. So by day the rig turns them down.
pub(crate) const DAY_EMISSIVE: f32 = 0.25;

/// Turn a library's emission down to what reads by day; see [`DAY_EMISSIVE`].
pub(crate) fn by_day(definitions: &mut crate::Definitions) {
    if let Some(library) = definitions.0.as_mut() {
        for definition in library.materials.values_mut() {
            definition.emissive = definition.emissive.map(|channel| channel * DAY_EMISSIVE);
        }
    }
}

/// Whether the rig is lit by night, which [`setup`] reads for the colour of
/// its lights and for bloom on the camera.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub(crate) struct Night(pub bool);

/// The light a scene is rendered under: the key, the ambient and the sky, by
/// day or by night, with any explicit key or ambient over the top.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rig {
    pub key: KeyLight,
    pub ambient: f32,
    pub sky: Color,
    pub night: bool,
}

impl Rig {
    pub(crate) fn new(night: bool, key: Option<f32>, ambient: Option<f32>) -> Self {
        let (default_key, default_ambient, sky) = if night {
            (NIGHT_KEY, NIGHT_AMBIENT, NIGHT_SKY)
        } else {
            (KeyLight::default().0, 350.0, DAY_SKY)
        };
        Self {
            key: KeyLight(key.unwrap_or(default_key)),
            ambient: ambient.unwrap_or(default_ambient),
            sky,
            night,
        }
    }
}

/// The asset a binding draws through, which are two different component types.
///
/// A `Surface::Shader` is a [`ProceduralMaterial`]; everything else is a
/// `StandardMaterial`. Which one a piece wears is carried rather than inferred
/// because the two are different asset types with different components.
pub(crate) enum PieceMaterial {
    /// A plain material: constants, files, and a runtime bake.
    Standard(Handle<StandardMaterial>),
    /// A compiled surface, drawn through its own pipeline.
    Shader(Handle<ProceduralMaterial>),
}

/// The material a binding draws through in the palette, or `None` before
/// [`setup_materials`] created it.
pub(crate) fn palette_material(
    palette: &Palette,
    binding: &ashlar::Binding,
) -> Option<PieceMaterial> {
    if let Some(handle) = palette.shaders.get(binding) {
        return Some(PieceMaterial::Shader(handle.clone()));
    }
    palette
        .handles
        .get(binding)
        .map(|handle| PieceMaterial::Standard(handle.clone()))
}

/// Spawn one drawable piece of a meshed building.
///
/// [`setup`] calls it for every piece once, in front of the first frame; a
/// blast calls it for the batches of the group it re-meshed, and the two must
/// agree on the components, the material lookup and the renderer-space
/// transform. Factoring it here is what keeps them from drifting.
///
/// Returns `None` for a binding the palette has no material for, which is a bug
/// rather than a state to render: the caller skips it, and [`palette_entry`]
/// exists so a blast cannot reach that case.
pub(crate) fn spawn_piece(
    commands: &mut Commands,
    center: &Center,
    palette: &Palette,
    piece: &ashlar::Piece<'_>,
    mesh: Handle<Mesh>,
) -> Option<Entity> {
    let material = palette_material(palette, piece.binding)?;
    let mut entity = commands.spawn((
        PreviewObject,
        // The material *key*, not the binding: what a click on this wall wants
        // is the graph behind its surface, and an override changes values
        // rather than which graph they belong to.
        SurfaceKey(piece.binding.material.clone()),
        Name::new(piece.label()),
        Mesh3d(mesh),
        Transform {
            translation: (piece.pose.translation - center.origin).as_vec3(),
            rotation: piece.pose.rotation.as_quat(),
            ..default()
        },
        // The library's classification, kept on the entity so `apply_cull` is a
        // query over pieces rather than a walk into the building.
        PieceStorey(piece.storey),
        PieceSide(piece.side),
        Visibility::Inherited,
    ));
    if let ashlar::PieceOrigin::Group { group, .. } = piece.origin {
        entity.insert(PieceGroup(group.to_owned()));
    }
    if let ashlar::PieceOrigin::Element {
        instance, element, ..
    } = piece.origin
    {
        entity.insert(PieceElement {
            instance: instance.to_owned(),
            element: element.to_owned(),
        });
    }
    match material {
        PieceMaterial::Standard(handle) => {
            entity.insert(MeshMaterial3d(handle));
        }
        PieceMaterial::Shader(handle) => {
            entity.insert(MeshMaterial3d(handle));
        }
    }
    Some(entity.id())
}

// Conversion to f32 happens only at the renderer boundary after local recentering.
#[allow(clippy::cast_possible_truncation)]
#[expect(
    clippy::too_many_arguments,
    reason = "a system's parameters are the resources it reads"
)]
pub(crate) fn setup(
    mut commands: Commands,
    preview: Res<Preview>,
    center: Res<Center>,
    key_light: Res<KeyLight>,
    night: Option<Res<Night>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    palette: Res<Palette>,
) {
    let mut handles = BTreeMap::new();
    for piece in preview.0.pieces() {
        // A group batch belongs to no part, so its geometry is uploaded the
        // first time its key is seen here rather than by an up-front walk over
        // `parts`. Element pieces still share one upload per key, which is what
        // that walk bought.
        if piece.mesh.positions.is_empty() {
            continue;
        }
        let mesh = handles
            .entry(piece.mesh_key())
            .or_insert_with(|| {
                meshes.add(materials::mesh(piece.mesh).expect("validated mesh tangents"))
            })
            .clone();
        spawn_piece(&mut commands, &center, &palette, &piece, mesh);
    }
    let night = night.is_some_and(|night| night.0);
    let camera = commands
        .spawn((PreviewObject, Camera3d::default(), Transform::default()))
        .id();
    if night {
        // Bloom is what makes a neon tube a glow rather than a flat bright
        // stripe. It needs an HDR target, which it requires for itself.
        commands
            .entity(camera)
            .insert(bevy::post_process::bloom::Bloom::NATURAL);
    }
    let (min, max) = bounds(&preview.0).expect("bounds checked before startup");
    let extent = (max - min).as_vec3().length() * 3.0;
    commands.spawn((
        PreviewObject,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(extent, extent))),
        MeshMaterial3d(materials.add(StandardMaterial {
            // By night the ground the model stands on is asphalt-dark, or a
            // moonlit plane outshines the street on it.
            base_color: if night {
                Color::srgb(0.035, 0.037, 0.042)
            } else {
                Color::srgb(0.16, 0.18, 0.21)
            },
            perceptual_roughness: 1.0,
            ..default()
        })),
        Transform::from_translation(Vec3::Y * ((min - center.origin).as_vec3().y - 0.02)),
        bevy::pbr::wireframe::NoWireframe,
    ));
    commands.spawn((
        PreviewObject,
        DirectionalLight {
            illuminance: key_light.0,
            // Moonlight is sunlight, but the eye reads it cold.
            color: if night {
                Color::srgb(0.62, 0.72, 1.0)
            } else {
                Color::WHITE
            },
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(-10.0, 18.0, -14.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        PreviewObject,
        DirectionalLight {
            // The fill follows the key rather than standing still, so lowering
            // one does not quietly change the ratio between them.
            illuminance: key_light.0 * if night { NIGHT_FILL } else { 2500.0 / 14000.0 },
            color: if night {
                Color::srgb(0.4, 0.45, 0.75)
            } else {
                Color::srgb(0.65, 0.78, 1.0)
            },
            ..default()
        },
        Transform::from_xyz(10.0, 8.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// How bright a lamp's emissive has to be, in its brightest channel, before
/// the night rig stands a light in it. A lit window is about one, a strip
/// light four; neon is eight and a sodium lamp ten.
const LAMP_PEAK: f32 = 5.0;

/// The largest a piece can be, across its bounds in metres, and still be a
/// lamp rather than a sign or a lit facade.
const LAMP_EXTENT: f64 = 2.5;

/// Lumens per unit of the brightest emissive channel: at this a sodium lamp
/// six metres up pools about ten thousand lux under itself, which at the
/// preview's exposure is what reads on dark wet paving as a pool of its
/// colour.
const LAMP_LUMENS: f32 = 300_000.0;

/// How much of a lamp's light an upright sign throws, which shines every way
/// and so reaches the wall it hangs on.
const SIGN_SHARE: f32 = 0.03;

/// How far a lamp's light reaches, in metres.
const LAMP_RANGE: f32 = 16.0;

/// The most lamps the night rig lights. Point lights without shadows are cheap
/// in a clustered renderer, but not free, and a metropolis has hundreds.
const MAX_LAMPS: usize = 384;

/// By night, stand a shadowless light in every small piece that emits
/// strongly, so a street lamp pools its colour on the road and a blade sign
/// washes the wall behind it. By day, or with no library, nothing.
///
/// An emissive surface adds to the colour it is seen in and lights nothing
/// else, which is right for a window and wrong for a lamp. The piece's own
/// bounds say which it is: a lamp head is small and bright, a facade is large.
// Conversion to f32 happens only at the renderer boundary after local recentering.
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn setup_night_lights(
    mut commands: Commands,
    preview: Res<Preview>,
    definitions: Res<crate::Definitions>,
    center: Res<Center>,
    night: Option<Res<Night>>,
) {
    if !night.is_some_and(|night| night.0) {
        return;
    }
    let Some(library) = definitions.0.as_ref() else {
        return;
    };
    let mut lamps = 0;
    let mut kinds = BTreeMap::new();
    for piece in preview.0.pieces() {
        let Some(definition) = library.materials.get(&piece.binding.material) else {
            continue;
        };
        let [r, g, b] = definition.emissive;
        let peak = r.max(g).max(b);
        if peak < LAMP_PEAK || piece.mesh.positions.is_empty() {
            continue;
        }
        let (mut low, mut high) = (
            bevy::math::DVec3::splat(f64::MAX),
            bevy::math::DVec3::splat(f64::MIN),
        );
        for point in &piece.mesh.positions {
            let point = piece.pose.transform_point(*point);
            low = low.min(point);
            high = high.max(point);
        }
        if (high - low).max_element() > LAMP_EXTENT {
            continue;
        }
        if lamps == MAX_LAMPS {
            eprintln!("night: more than {MAX_LAMPS} lamps; the rest are left unlit");
            break;
        }
        lamps += 1;
        *kinds
            .entry(piece.binding.material.as_str())
            .or_insert(0_usize) += 1;
        let size = high - low;
        let color = Color::linear_rgb(r / peak, g / peak, b / peak);
        let at = ((low + high) * 0.5 - center.origin).as_vec3();
        // A flat head is a street lamp, and a street lamp shines down: a
        // spot keeps its pool on the ground rather than on the wall behind
        // it. Anything upright — a sign, a tube — glows every way.
        if size.y < 0.5 * size.x.max(size.z) {
            commands.spawn((
                PreviewObject,
                SpotLight {
                    color,
                    intensity: peak * LAMP_LUMENS,
                    range: LAMP_RANGE,
                    radius: 0.1,
                    inner_angle: 0.35,
                    outer_angle: 0.8,
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::from_translation(at - Vec3::Y * 0.2).looking_to(Vec3::NEG_Y, Vec3::Z),
            ));
        } else {
            commands.spawn((
                PreviewObject,
                PointLight {
                    color,
                    intensity: peak * LAMP_LUMENS * SIGN_SHARE,
                    range: LAMP_RANGE * 0.6,
                    radius: 0.1,
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::from_translation(at),
            ));
        }
    }
    if lamps > 0 {
        println!("night: {lamps} lamps lit: {kinds:?}");
    }
}

/// How many threads a strand scatter divides its cell rows across.
///
/// Four rather than [`BAKE_THREADS`]' eight, and for the reason that constant
/// gives at length: this machine has been seen to fall over under full
/// multi-core load, and a preview that grows a lawn now has a *second* thing
/// wanting every core in front of the first frame rather than one. A repeat of
/// sixty-five thousand strands is a few milliseconds of field evaluation, so
/// there is nothing here worth spending the other four on.
pub(crate) const STRAND_THREADS: Option<std::num::NonZeroUsize> = std::num::NonZeroUsize::new(4);

/// One strand chunk, ready to be spawned once per instance that wears it.
///
/// Built per part, element and binding rather than per instance, because a
/// scatter and a placement are the same answer for every instance of one part
/// wearing one material: a sheet stands twelve specimens over two parts, and
/// growing the lawn twelve times would be eleven lawns nobody asked for.
struct GrownChunk {
    name: String,
    mesh: Handle<Mesh>,
    material: ChunkMaterial,
    shadows: bool,
    /// Where the chunk sits in its element's own frame, which is what the
    /// mesh is written relative to and what a level of detail is measured
    /// from. `materials::strands::StrandChunk::origin` says why.
    origin: Vec3,
    /// Where this level is drawn, or `None` for a layer that declared no
    /// distances and is therefore always drawn.
    range: Option<bevy::camera::visibility::VisibilityRange>,
}

/// Which of the two materials a chunk draws through.
///
/// A strand level is a `StandardMaterial` with the wind vertex stage over it;
/// a card level is a plain one, because it binds a normal map and the wind
/// stages declare no tangent to read one in. They are different asset types, so
/// which one a chunk is has to be carried rather than inferred.
enum ChunkMaterial {
    /// A level of real strands, swaying.
    Strands(Handle<materials::wind::StrandWindMaterial>),
    /// The card level, standing still.
    Cards(Handle<StandardMaterial>),
}

/// Grow the strand layers every bound material asks for, and spawn them.
///
/// After [`setup`] rather than inside it, because the two are different costs:
/// spawning a meshed element is a handle, and growing a lawn on it is a scatter,
/// a placement and a few hundred thousand triangles. Keeping them apart is also
/// what lets the gallery skip this one day without unpicking the other.
///
/// Fallible for the reason [`setup_materials`] is: everything a strand layer can
/// be refused for was refused by `preflight`, so a failure here is a bug rather
/// than bad content, and it says so rather than quietly growing nothing.
// Conversion to f32 happens only at the renderer boundary after local recentering.
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn setup_strands(
    mut commands: Commands,
    preview: Res<Preview>,
    definitions: Res<crate::Definitions>,
    center: Res<Center>,
    mut baking: Baking,
    mut growing: Growing,
) -> BevyResult {
    let Some(library) = definitions.0.as_ref() else {
        return Ok(());
    };
    // Keyed by what the scatter actually depends on, which is what
    // `Piece::shared_key` names: the geometry the strands grow on, and the
    // binding whose graph decides how they grow.
    let mut grown: BTreeMap<(ashlar::MeshKey, &ashlar::Binding), Vec<GrownChunk>> = BTreeMap::new();
    for piece in preview.0.pieces() {
        if piece.mesh.positions.is_empty() {
            continue;
        }
        let binding = piece.binding;
        let key = piece.shared_key();
        if let std::collections::btree_map::Entry::Vacant(entry) = grown.entry(key) {
            let definition = materials::definition(library, binding)
                .with_context(|| format!("material {}", binding.material))?;
            // Scattered first, then grown: the two calls are the seam the
            // baked set file sits in, and the preview is deliberately on the
            // tool side of it. A game replaces the first line with a file
            // read and the second is the same line.
            let sets = materials::strands::scatter_sets(&definition, &mut baking.scatter_context())
                .with_context(|| format!("material {}", binding.material))?;
            let chunks = match &sets {
                Some(sets) => grow(
                    &definition,
                    piece.mesh,
                    sets,
                    &mut baking.strand_context(),
                    &mut growing,
                )
                .with_context(|| format!("material {}", binding.material))?,
                None => Vec::new(),
            };
            entry.insert(chunks);
        }
        for chunk in &grown[&key] {
            let rotation = piece.pose.rotation.as_quat();
            let mut entity = commands.spawn((
                PreviewObject,
                SurfaceKey(binding.material.clone()),
                Name::new(format!("{}/{}", piece.label(), chunk.name)),
                Mesh3d(chunk.mesh.clone()),
                Transform {
                    // The chunk's own origin, moved out of the mesh and into
                    // the transform so that Bevy measures a level of detail
                    // from the patch of lawn rather than from wherever the
                    // element it grows on happens to sit.
                    translation: (piece.pose.translation - center.origin).as_vec3()
                        + rotation * chunk.origin,
                    rotation,
                    ..default()
                },
                // Wireframe on a lawn is a solid white ball, which says nothing
                // about the mesh under it.
                bevy::pbr::wireframe::NoWireframe,
                // Mesh picking ray casts on the CPU, over every pickable mesh,
                // every time the pointer moves. A chunk is a quarter of a
                // million triangles and is not what anyone is trying to click:
                // the surface *under* it is, and it is still there. Without
                // this the parameter panel costs a second of CPU per mouse
                // movement over the grass.
                Pickable::IGNORE,
            ));
            // The material last, because which asset type a chunk draws through
            // is what the card level differs in.
            match &chunk.material {
                ChunkMaterial::Strands(handle) => {
                    entity.insert(MeshMaterial3d(handle.clone()));
                }
                ChunkMaterial::Cards(handle) => {
                    entity.insert(MeshMaterial3d(handle.clone()));
                }
            }
            if !chunk.shadows {
                entity.insert(bevy::light::NotShadowCaster);
            }
            if let Some(range) = &chunk.range {
                entity.insert(range.clone());
            }
        }
    }
    Ok(())
}

/// Every strand layer of one element, grown and uploaded.
///
/// A strand material is deliberately *not* in [`Palette`], which is what clay
/// mode rewrites: `P` turns the surfaces grey and leaves the blades green. That
/// is the same answer the palette already gives a compiled surface, and the
/// honest one — clay mode hides what a material decides, and a blade's shape is
/// geometry rather than a material decision.
///
/// One material per *layer* rather than per chunk: the chunks of a layer differ
/// in which repeat of the surface they cover and in nothing a material can see,
/// so sharing one handle is what keeps a lawn a handful of draw calls per
/// specimen instead of one batch per repeat.
fn grow(
    definition: &ashlar::MaterialDefinition,
    surface: &ashlar::TriangleMesh,
    sets: &materials::strands::StrandSets,
    context: &mut materials::strands::StrandContext<'_>,
    growing: &mut Growing,
) -> Result<Vec<GrownChunk>> {
    let Some(settings) = &definition.strands else {
        return Ok(Vec::new());
    };
    let started = std::time::Instant::now();
    let grown = materials::strands::create_strands(definition, surface, sets, context)?;
    let mut chunks = Vec::new();
    for layer in grown {
        // What the lawn costs, printed where a lawn is being authored, beside
        // the line a bake prints for the texture under it.
        println!(
            "{:?} grew {} strands in {} levels of {} chunks, {} triangles, {} cards{}, in {:.0} ms",
            layer.layer,
            layer.strands,
            layer.levels.len(),
            layer.levels.first().map_or(0, |level| level.chunks.len()),
            layer.triangles,
            layer.cards.as_ref().map_or(0, |cards| cards
                .chunks
                .iter()
                .map(|chunk| chunk.strands)
                .sum()),
            // What a card is, where there are any: the lattice they were
            // gathered on and how big one is, which is what an author changes
            // a clump count to move.
            layer.cards.as_ref().map_or(String::new(), |cards| format!(
                " on {}² at {:.0} mm",
                cards.settings.cells,
                cards.settings.metres * 1000.0
            )),
            started.elapsed().as_secs_f64() * 1000.0
        );
        // One material per *layer* rather than per level: the levels differ in
        // how many strands they draw and in nothing a `StandardMaterial` can
        // see, and sharing the handle is what lets Bevy batch the two that are
        // both drawn during a crossfade.
        let material = growing
            .strands
            .add(materials::strands::strand_wind_material(
                definition,
                layer.roughness,
            ));
        for level in layer.levels {
            for chunk in level.chunks {
                chunks.push(GrownChunk {
                    name: format!(
                        "{}[{},{}]@{}",
                        layer.layer, chunk.repeat[0], chunk.repeat[1], level.level.level
                    ),
                    mesh: growing.meshes.add(chunk.mesh),
                    material: ChunkMaterial::Strands(material.clone()),
                    shadows: settings.cast_shadows,
                    origin: Vec3::from_array(chunk.origin),
                    range: level.level.range.clone(),
                });
            }
        }
        // And the card level, which is one more material and one more chunk per
        // repeat: the atlas is shared by every surface with the same scatter,
        // and the quads are this element's own.
        let Some(level) = layer.cards else {
            continue;
        };
        let material = growing.cards.add(materials::cards::material(
            definition,
            &level.atlas,
            layer.roughness,
            level.cutoff,
        ));
        for chunk in level.chunks {
            chunks.push(GrownChunk {
                name: format!(
                    "{}[{},{}]@cards",
                    layer.layer, chunk.repeat[0], chunk.repeat[1]
                ),
                mesh: growing.meshes.add(chunk.mesh),
                material: ChunkMaterial::Cards(material.clone()),
                shadows: settings.cast_shadows,
                origin: Vec3::from_array(chunk.origin),
                range: Some(level.range.clone()),
            });
        }
    }
    Ok(chunks)
}

pub(crate) fn material(key: &str, alternate: bool) -> StandardMaterial {
    if alternate {
        return StandardMaterial {
            base_color: Color::srgb(0.55, 0.55, 0.55),
            perceptual_roughness: 0.85,
            ..default()
        };
    }
    let color = match (key, alternate) {
        ("shell", false) => Color::srgb(0.48, 0.5, 0.51),
        ("trim", false) => Color::srgb(0.18, 0.21, 0.24),
        ("accent", false) => Color::srgb(0.37, 0.19, 0.09),
        ("shell", true) => Color::srgb(0.75, 0.79, 0.8),
        ("trim", true) => Color::srgb(0.14, 0.36, 0.4),
        ("accent", true) => Color::srgb(0.8, 0.48, 0.12),
        _ => {
            let hash = key
                .bytes()
                .fold(0_u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
            Color::hsl(
                f32::from(u16::try_from(hash % 360).expect("hue range")),
                if alternate { 0.6 } else { 0.2 },
                0.55,
            )
        }
    };
    StandardMaterial {
        base_color: color,
        perceptual_roughness: if alternate { 0.35 } else { 0.8 },
        ..default()
    }
}

fn controls(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    mut orbit: ResMut<Orbit>,
    mut camera: Query<&mut Transform, With<Camera3d>>,
    grab: PointerGrab,
) {
    // The panel's sliders are dragged with the same button that orbits, so
    // while the pointer belongs to the panel the camera holds still.
    let interactive = grab.interactive();
    if interactive && keys.just_pressed(KeyCode::Home) {
        orbit.yaw = orbit.initial_yaw;
        orbit.pitch = orbit.initial_pitch;
        orbit.distance = orbit.initial_distance;
        orbit.target = orbit.initial_target;
    }
    if interactive && buttons.pressed(MouseButton::Left) {
        orbit.yaw -= motion.delta.x * 0.006;
        orbit.pitch = (orbit.pitch + motion.delta.y * 0.006).clamp(-1.4, 1.4);
    }
    if interactive {
        orbit.distance = (orbit.distance * (-scroll.delta.y * 0.12).exp())
            .clamp(0.1, orbit.initial_distance * 20.0);
    }
    let rotation = Quat::from_rotation_y(orbit.yaw) * Quat::from_rotation_x(orbit.pitch);
    if interactive && buttons.pressed(MouseButton::Right) {
        let scale = orbit.distance * 0.001;
        orbit.target += rotation * Vec3::new(motion.delta.x, motion.delta.y, 0.0) * scale;
    }
    // Facing the negative-Z facade from outside the model.
    let position = orbit.target + rotation * Vec3::NEG_Z * orbit.distance;
    for mut transform in &mut camera {
        *transform = Transform::from_translation(position).looking_at(orbit.target, Vec3::Y);
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "one place for the viewer's display keys, each its own resource"
)]
fn appearance(
    keys: Res<ButtonInput<KeyCode>>,
    mut wire: ResMut<WireframeConfig>,
    mut palette: ResMut<Palette>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut exit: MessageWriter<AppExit>,
    options: Option<Res<Options>>,
    grab: PointerGrab,
    cull: Option<ResMut<Cull>>,
    storeys: Query<&PieceStorey>,
) {
    if options.is_some_and(|o| o.screenshot.is_some()) {
        return;
    }
    let interactive = grab.interactive();
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
    }
    if keys.just_pressed(KeyCode::KeyF) {
        wire.global = !wire.global;
    }
    if keys.just_pressed(KeyCode::KeyP) {
        palette.alternate = !palette.alternate;
        for (binding, handle) in &palette.handles {
            if let Some(mut value) = materials.get_mut(handle) {
                *value = if palette.alternate {
                    material(&binding.material, true)
                } else {
                    palette
                        .originals
                        .get(binding)
                        .cloned()
                        .unwrap_or_else(|| material(&binding.material, false))
                };
            }
        }
    }
    // The top-down policy on keys. PageDown drops the cut one storey, PageUp
    // raises it and clears it once it is past the tallest storey present, and
    // `I` hides the exterior so the interior reads. `Cull` is absent in a
    // harness with no window, so there is nothing to write.
    let Some(mut cull) = cull else {
        return;
    };
    let highest = storeys.iter().filter_map(|storey| storey.0).max();
    if interactive && keys.just_pressed(KeyCode::PageDown) {
        cull.max_storey = match cull.max_storey {
            Some(storey) => Some(storey - 1),
            None => highest.map(|storey| storey - 1),
        };
    }
    if interactive && keys.just_pressed(KeyCode::PageUp) {
        cull.max_storey = match (cull.max_storey, highest) {
            (Some(storey), Some(highest)) if storey + 1 > highest => None,
            (Some(storey), _) => Some(storey + 1),
            (None, _) => None,
        };
    }
    if interactive && keys.just_pressed(KeyCode::KeyI) {
        cull.hide_exterior = !cull.hide_exterior;
    }
}

/// Hide the pieces the current [`Cull`] excludes and show the rest.
///
/// A piece with no storey is never hidden by `max_storey`, because a storey
/// cut has nothing to say about paving; only `hide_exterior` can reach one.
/// `Visibility` is written only when the policy changed, since a write every
/// frame is renderer change detection spent on nothing.
fn apply_cull(cull: Res<Cull>, mut pieces: Query<(&PieceStorey, &PieceSide, &mut Visibility)>) {
    if !cull.is_changed() {
        return;
    }
    for (storey, side, mut visibility) in &mut pieces {
        let above = cull
            .max_storey
            .zip(storey.0)
            .is_some_and(|(max, storey)| storey > max);
        let exterior = cull.hide_exterior && side.0 == ashlar::Side::Exterior;
        *visibility = if above || exterior {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
    }
}

fn capture(
    mut commands: Commands,
    options: Res<Options>,
    mut frames: Local<u32>,
    mut exit: MessageWriter<AppExit>,
    palette: Res<Palette>,
    server: Res<AssetServer>,
) {
    for material in palette.originals.values() {
        for handle in [
            &material.base_color_texture,
            &material.normal_map_texture,
            &material.metallic_roughness_texture,
            &material.emissive_texture,
        ]
        .into_iter()
        .flatten()
        {
            // A runtime-baked map was handed to `Assets<Image>` directly and
            // the server has never heard of it, so it has no load state and
            // nothing to wait for. Only a map that came from a file does.
            let Some(state) = server.get_load_state(handle.id()) else {
                continue;
            };
            if let bevy::asset::LoadState::Failed(error) = state {
                eprintln!("Texture failed: {error}");
                exit.write(AppExit::error());
                return;
            }
            if !server.is_loaded_with_dependencies(handle.id()) {
                return;
            }
        }
    }
    *frames += 1;
    let Some(limit) = options.frames.or(options.screenshot.as_ref().map(|_| 180)) else {
        return;
    };
    if *frames != limit {
        return;
    }
    if let Some(path) = &options.screenshot {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path.clone()))
            .observe(
                |_: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>| {
                    exit.write(AppExit::Success);
                },
            );
    } else {
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(arguments: &[&str]) -> Options {
        Options::try_parse_for(&crate::tests::catalog(), arguments).expect("options")
    }

    fn harness() -> (App, Entity, Handle<StandardMaterial>) {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<AccumulatedMouseMotion>()
            .init_resource::<AccumulatedMouseScroll>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<WireframeConfig>()
            .init_resource::<Palette>()
            .insert_resource(Orbit {
                yaw: -0.6,
                pitch: 0.35,
                distance: 20.0,
                initial_distance: 20.0,
                initial_yaw: -0.6,
                initial_pitch: 0.35,
                target: Vec3::ZERO,
                initial_target: Vec3::ZERO,
            })
            .add_message::<AppExit>()
            .add_systems(Update, (controls, appearance));
        let camera = app
            .world_mut()
            .spawn((Camera3d::default(), Transform::default()))
            .id();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(material("shell", false));
        app.world_mut()
            .resource_mut::<Palette>()
            .handles
            .insert("shell".into(), handle.clone());
        (app, camera, handle)
    }

    /// A one metre cube at the origin, so the framing numbers are exact.
    #[test]
    fn framing_stands_the_camera_off_the_buildings_own_diagonal() {
        let building = crate::tests::catalog()
            .require("one")
            .expect("scene")
            .build()
            .expect("building");
        let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("mesh");

        let (min, max) = bounds(&meshed).expect("bounds");
        assert!(min.abs_diff_eq(bevy::math::DVec3::ZERO, 1e-9), "{min}");
        assert!(max.abs_diff_eq(bevy::math::DVec3::ONE, 1e-9), "{max}");

        let (center, distance) = framing(&meshed, GALLERY_FRAMING).expect("framing");
        assert!(
            center
                .model
                .abs_diff_eq(bevy::math::DVec3::splat(0.5), 1e-9),
            "{}",
            center.model
        );
        // The origin the model is moved to is the centre in x and z and the
        // floor in y, so a fragment's world height is its height above the
        // ground and the camera makes up the difference.
        assert!(
            center
                .origin
                .abs_diff_eq(bevy::math::DVec3::new(0.5, 0.0, 0.5), 1e-9),
            "{}",
            center.origin
        );
        assert!(center.target().abs_diff_eq(Vec3::Y * 0.5, 1e-6));
        let diagonal = 3.0_f32.sqrt();
        assert!(
            (distance - diagonal * GALLERY_FRAMING).abs() < 1e-5,
            "{distance}"
        );
        // The interactive viewer starts a little further out than the gallery.
        let (_, live) = framing(&meshed, VIEWER_FRAMING).expect("framing");
        assert!(live > distance);
    }

    #[test]
    fn screenshot_mode_ignores_live_view_and_palette_input() {
        let (mut app, camera, _) = harness();
        app.insert_resource(options(&["preview", "--screenshot", "/tmp/unused.png"]));
        app.update();
        let initial = *app.world().get::<Transform>(camera).expect("camera");
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.press(KeyCode::KeyF);
        keys.press(KeyCode::KeyP);
        app.world_mut()
            .resource_mut::<AccumulatedMouseScroll>()
            .delta
            .y = 5.0;
        app.update();
        assert_eq!(
            *app.world().get::<Transform>(camera).expect("camera"),
            initial
        );
        assert!(!app.world().resource::<WireframeConfig>().global);
        assert!(!app.world().resource::<Palette>().alternate);
    }

    #[test]
    fn preview_controls_change_view_and_materials_without_rebuilding_geometry() {
        let (mut app, camera, handle) = harness();
        app.update();
        let initial = *app.world().get::<Transform>(camera).expect("camera");
        assert!(initial.translation.y > 0.0);
        assert!(initial.translation.is_finite());
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.press(KeyCode::KeyP);
        keys.press(KeyCode::KeyF);
        app.world_mut()
            .resource_mut::<AccumulatedMouseScroll>()
            .delta
            .y = 1.0;
        app.update();
        assert!(app.world().resource::<WireframeConfig>().global);
        assert!(app.world().resource::<Orbit>().distance < 20.0);
        assert_eq!(
            app.world()
                .resource::<Assets<StandardMaterial>>()
                .get(&handle)
                .expect("material")
                .base_color,
            material("shell", true).base_color
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut()
            .resource_mut::<AccumulatedMouseScroll>()
            .delta = Vec2::ZERO;
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Home);
        app.update();
        assert_eq!(
            *app.world().get::<Transform>(camera).expect("camera"),
            initial
        );
    }
}
