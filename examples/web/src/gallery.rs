//! The material stage: the default library on a sphere and a wall, and a live
//! re-bake of the one on show whenever a parameter moves.
//!
//! What opens is the file the content step wrote, loaded like any other map.
//! Moving a slider changes nothing until the pointer has rested for
//! [`DEBOUNCE`] seconds; then the panel says it is baking, and the next frame
//! bakes the graph on the main thread — one thread, because a browser gives a
//! page no others — and swaps the maps under the same material handle, so the
//! three specimens change together.
use std::{collections::BTreeMap, num::NonZeroUsize};

use ashlar::{MaterialDefinition, ParamValue, Surface};
use ashlar_bevy::{
    prelude::MaterialLibraryAsset,
    runtime_bake::{BakeCache, BakeContext, BakeKey, Baker, GraphTextures, create_graph_material},
};
use ashlar_material::{
    MaterialGraphLibrary, Param,
    bake::{BakeRequest, bake},
    stdlib,
};
use bevy::{mesh::VertexAttributeValues, platform::time::Instant, prelude::*};

use crate::{Mode, camera::Rig, catalog};

/// Seconds a slider has to rest before the material re-bakes.
pub(crate) const DEBOUNCE: f32 = 0.35;

/// The heading the stage is seen from, in radians: from the key light's side,
/// a little round, so the sphere is lit and modelled.
const STAGE_YAW: f32 = -1.95;

/// Registers the stage, starting on `material` with `params` applied as if a
/// slider had moved them.
pub(crate) fn plugin(
    material: Option<String>,
    params: Vec<(String, String)>,
) -> impl Fn(&mut App) + Send + Sync + 'static {
    move |app: &mut App| {
        let selected = material.as_deref().map_or_else(
            || "library:brick".to_owned(),
            |key| {
                if key.contains(':') {
                    key.to_owned()
                } else {
                    format!("library:{key}")
                }
            },
        );
        app.insert_resource(Pending(params.clone()))
            .insert_resource(Graphs(stdlib::graphs()))
            .init_resource::<BakeCache>()
            .add_systems(
                Startup,
                move |mut commands: Commands, server: Res<AssetServer>| {
                    commands.insert_resource(Gallery {
                        library: server.load(catalog::GALLERY_LIBRARY),
                        keys: Vec::new(),
                        selected: selected.clone(),
                        shown: None,
                        rows: Vec::new(),
                        changed: None,
                        state: BakeState::Files,
                        material: Handle::default(),
                    });
                },
            )
            .add_systems(Startup, stage)
            .add_systems(
                Update,
                (list, select, rebake, show_mode, frame_mode).chain(),
            );
    }
}

/// The graph library the stage re-bakes from: the default library, built
/// once, which is the same graphs the gallery's files were baked from.
#[derive(Resource)]
pub(crate) struct Graphs(pub MaterialGraphLibrary);

/// Parameter overrides from the command line or the query string, applied to
/// the first material shown.
#[derive(Resource)]
struct Pending(Vec<(String, String)>);

/// The stage's state, which the panel reads and writes.
#[derive(Resource)]
pub(crate) struct Gallery {
    /// The file-backed library the gallery shows.
    library: Handle<MaterialLibraryAsset>,
    /// Its keys, once it has loaded, in order.
    pub keys: Vec<String>,
    /// The material asked for.
    pub selected: String,
    /// The material on the stage.
    shown: Option<String>,
    /// Its graph's parameters, and the value each has now.
    pub rows: Vec<Row>,
    /// When a parameter last moved, if it has since the last bake.
    pub changed: Option<Instant>,
    /// What the specimens are wearing.
    pub state: BakeState,
    /// The one material every specimen wears.
    material: Handle<StandardMaterial>,
}

impl Gallery {
    /// Note that a parameter moved: the bake waits for the pointer to rest.
    pub(crate) fn touch(&mut self) {
        self.changed = Some(Instant::now());
    }

    /// Put every parameter back to the value the file was baked with.
    pub(crate) fn reset(&mut self) {
        for row in &mut self.rows {
            row.value = row.baked;
        }
        self.touch();
    }
}

/// One parameter of the material on show.
#[derive(Clone, Debug)]
pub(crate) struct Row {
    /// The parameter as the graph declares it: name, default and range.
    pub param: Param,
    /// Its value now.
    pub value: ParamValue,
    /// Its value in the file the stage opened on.
    pub baked: ParamValue,
}

/// What the specimens are wearing.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BakeState {
    /// The maps the content step wrote.
    Files,
    /// A bake is next frame; the panel says so this one.
    Baking,
    /// A bake of the parameters as they are, at this resolution, in this
    /// many milliseconds.
    Baked {
        /// Texels per repeat.
        resolution: u32,
        /// How long the bake took on the main thread.
        millis: f32,
    },
    /// The parameters as they are do not bake, and why.
    Failed(String),
}

/// A specimen on the stage.
#[derive(Component)]
pub(crate) struct StageRoot;

/// Scale a mesh's UVs from its own 0..1 to metres, which is the unit every
/// ashlar mesh carries and every definition's `tile_metres` divides.
fn in_metres(mut mesh: Mesh, metres: Vec2) -> Mesh {
    if let Some(VertexAttributeValues::Float32x2(uvs)) = mesh.attribute_mut(Mesh::ATTRIBUTE_UV_0) {
        for uv in uvs {
            uv[0] *= metres.x;
            uv[1] *= metres.y;
        }
    }
    // A normal map needs tangents, and the primitives come without them.
    if let Err(error) = mesh.generate_tangents() {
        tracing::warn!("specimen tangents: {error}");
    }
    mesh
}

fn stage(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let material = materials.add(StandardMaterial::default());
    let facing = Quat::from_rotation_y(STAGE_YAW);
    let toward = facing * Vec3::Z;
    let root = commands
        .spawn((StageRoot, Transform::default(), Visibility::Hidden))
        .id();
    let sphere = meshes.add(in_metres(
        Sphere::new(1.0).mesh().uv(96, 48),
        Vec2::new(std::f32::consts::TAU, std::f32::consts::PI),
    ));
    let wall = meshes.add(in_metres(
        Rectangle::new(5.0, 3.5).mesh().build(),
        Vec2::new(5.0, 3.5),
    ));
    let floor = meshes.add(Plane3d::default().mesh().size(40.0, 40.0));
    let neutral = materials.add(StandardMaterial {
        base_color: Color::srgb(0.12, 0.13, 0.15),
        perceptual_roughness: 0.9,
        ..default()
    });
    commands.entity(root).with_children(|stage| {
        stage.spawn((
            Mesh3d(sphere),
            MeshMaterial3d(material.clone()),
            Transform::from_xyz(0.0, 1.0, 0.0),
        ));
        stage.spawn((
            Mesh3d(wall),
            MeshMaterial3d(material.clone()),
            Transform::from_translation(-toward * 1.8 + Vec3::Y * 1.75).with_rotation(facing),
        ));
        stage.spawn((Mesh3d(floor), MeshMaterial3d(neutral)));
    });
    // The gallery resource is inserted by a startup system of its own, so the
    // handle travels in a resource of its own and `select` copies it over.
    commands.insert_resource(StageMaterial(material));
}

/// The handle every specimen wears.
#[derive(Resource)]
struct StageMaterial(Handle<StandardMaterial>);

/// Fill the key list once the gallery library has loaded.
fn list(mut gallery: ResMut<Gallery>, libraries: Res<Assets<MaterialLibraryAsset>>) {
    if !gallery.keys.is_empty() {
        return;
    }
    if let Some(library) = libraries.get(&gallery.library) {
        gallery.keys = library.0.materials.keys().cloned().collect();
    }
}

/// Parse a parameter override against the value it replaces.
fn parse(value: &str, like: ParamValue) -> Option<ParamValue> {
    Some(match like {
        ParamValue::Float(_) => ParamValue::Float(value.parse().ok()?),
        ParamValue::Int(_) => ParamValue::Int(value.parse().ok()?),
        ParamValue::Bool(_) => ParamValue::Bool(matches!(value, "1" | "true" | "on")),
        ParamValue::Color(_) => {
            let channels: Vec<f32> = value
                .split(',')
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()?;
            ParamValue::Color(channels.try_into().ok()?)
        }
    })
}

/// Put the selected material on the stage from its files, and lay out its
/// parameters.
#[expect(
    clippy::too_many_arguments,
    reason = "a Bevy system's parameters are its inputs"
)]
fn select(
    mut gallery: ResMut<Gallery>,
    mut pending: ResMut<Pending>,
    stage_material: Res<StageMaterial>,
    libraries: Res<Assets<MaterialLibraryAsset>>,
    graphs: Res<Graphs>,
    server: Res<AssetServer>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut cache: ResMut<BakeCache>,
) {
    if gallery.material != stage_material.0 {
        gallery.material = stage_material.0.clone();
    }
    if gallery.shown.as_deref() == Some(gallery.selected.as_str()) {
        return;
    }
    let Some(library) = libraries.get(&gallery.library) else {
        return;
    };
    let Some(definition) = library.0.materials.get(&gallery.selected) else {
        tracing::warn!("the gallery has no material {:?}", gallery.selected);
        gallery.selected = gallery.keys.first().cloned().unwrap_or_default();
        return;
    };
    if let Some(mut material) = materials.get_mut(&gallery.material) {
        *material = ashlar_bevy::create_material(definition, &server);
    }
    // A bake of the last material is not this one's; let its images go.
    cache.clear();
    let baked: BTreeMap<String, ParamValue> = match &definition.surface {
        Surface::Files {
            baked_from: Some(bake),
            ..
        } => bake.params.clone(),
        _ => BTreeMap::new(),
    };
    let graph = definition.surface.graph().and_then(|key| graphs.0.get(key));
    gallery.rows = graph
        .map(|graph| {
            graph
                .params
                .iter()
                .map(|param| {
                    let value = baked.get(&param.name).copied().unwrap_or(param.value);
                    Row {
                        param: param.clone(),
                        value,
                        baked: value,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    gallery.shown = Some(gallery.selected.clone());
    gallery.state = BakeState::Files;
    gallery.changed = None;
    // Overrides from the command line apply once, to the first material.
    let overrides = std::mem::take(&mut pending.0);
    for (name, value) in overrides {
        let selected = gallery.selected.clone();
        let Some(row) = gallery.rows.iter_mut().find(|row| row.param.name == name) else {
            tracing::warn!("{selected} has no parameter {name}");
            continue;
        };
        if let Some(parsed) = parse(&value, row.value) {
            row.value = parsed;
        } else {
            tracing::warn!("{value:?} is not a value for {name}");
        }
        gallery.changed = Some(Instant::now());
    }
}

/// The definition a bake of the current rows is for: the gallery's own, with
/// its surface a graph at the rows' values.
fn definition(library: &MaterialLibraryAsset, gallery: &Gallery) -> Option<MaterialDefinition> {
    library.0.materials.get(&gallery.selected).cloned()
}

/// Re-bake the material on show once its parameters have rested.
fn rebake(
    mut gallery: ResMut<Gallery>,
    libraries: Res<Assets<MaterialLibraryAsset>>,
    graphs: Res<Graphs>,
    server: Res<AssetServer>,
    mut cache: ResMut<BakeCache>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if gallery.state == BakeState::Baking {
        let Some(library) = libraries.get(&gallery.library) else {
            return;
        };
        let Some(mut dressed) = definition(library, &gallery) else {
            return;
        };
        let started = Instant::now();
        gallery.state = match bake_rows(&gallery, &graphs.0, &mut dressed) {
            Ok((resolution, textures)) => {
                let Surface::Graph(bake) = &dressed.surface else {
                    unreachable!("bake_rows leaves a graph surface")
                };
                // One set at a time: the last bake's images go when the
                // material stops naming them.
                cache.clear();
                cache.insert(BakeKey::new(bake), textures, &mut images);
                let created = create_graph_material(
                    &dressed,
                    &server,
                    &mut BakeContext {
                        graphs: &graphs.0,
                        cache: &mut cache,
                        images: &mut images,
                        threads: NonZeroUsize::new(1),
                        baker: Baker::Cpu,
                    },
                );
                match created {
                    Ok(material) => {
                        if let Some(mut slot) = materials.get_mut(&gallery.material) {
                            *slot = material;
                        }
                        let millis = started.elapsed().as_secs_f32() * 1000.0;
                        tracing::info!(
                            "re-baked {} at {resolution} texels in {millis:.0} ms",
                            gallery.selected
                        );
                        BakeState::Baked { resolution, millis }
                    }
                    Err(error) => BakeState::Failed(format!("{error:#}")),
                }
            }
            Err(error) => BakeState::Failed(error),
        };
        return;
    }
    if let Some(changed) = gallery.changed
        && changed.elapsed().as_secs_f32() >= DEBOUNCE
    {
        gallery.changed = None;
        gallery.state = BakeState::Baking;
    }
}

/// Bake the rows' values into images, and rewrite `dressed` to name that
/// bake. Answers the resolution it baked at.
fn bake_rows(
    gallery: &Gallery,
    graphs: &MaterialGraphLibrary,
    dressed: &mut MaterialDefinition,
) -> Result<(u32, GraphTextures), String> {
    let key = dressed
        .surface
        .graph()
        .ok_or_else(|| format!("{} names no graph", gallery.selected))?
        .to_owned();
    let graph = graphs
        .get(&key)
        .ok_or_else(|| format!("the library has no graph {key}"))?;
    let params: BTreeMap<String, ParamValue> = gallery
        .rows
        .iter()
        .map(|row| (row.param.name.clone(), row.value))
        .collect();
    // A count moved up can lay a lattice finer than the floor; bake at it.
    let resolution = catalog::resolution(graphs, &key, &params, catalog::GALLERY_RESOLUTION);
    let set = bake(&BakeRequest {
        graph,
        library: graphs,
        params: &params,
        resolution,
        mips: true,
        threads: NonZeroUsize::new(1),
    })
    .map_err(|error| error.to_string())?;
    let mut textures = GraphTextures::from(&set);
    // Height reaches no material slot, and its sixteen-bit format is one
    // WebGL2 cannot upload.
    textures.height = None;
    dressed.surface = Surface::Graph(ashlar::Bake {
        graph: key,
        params,
        resolution,
    });
    Ok((resolution, textures))
}

/// Show the stage only in the materials mode.
fn show_mode(mode: Res<Mode>, mut roots: Query<&mut Visibility, With<StageRoot>>) {
    let wanted = if *mode == Mode::Materials {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut visibility in &mut roots {
        visibility.set_if_neq(wanted);
    }
}

/// Where the stage is seen from when the materials mode opens.
fn stage_rig() -> Rig {
    Rig {
        focus: Vec3::new(0.0, 1.3, 0.0),
        yaw: STAGE_YAW + 0.35,
        pitch: 0.16,
        distance: 7.0,
        eye: Vec3::new(0.0, 1.7, 4.0),
        extent: 8.0,
        ground: 0.0,
        bounds: [Vec3::new(-3.0, 0.0, -3.0), Vec3::new(3.0, 3.5, 3.0)],
    }
}

/// Each mode keeps its own camera: leaving the stage puts the view back where
/// the scene had it, and coming back finds the stage where it was left.
/// `other` is the rig of the mode not on screen.
fn frame_mode(
    mode: Res<Mode>,
    mut rigs: Query<&mut Rig>,
    mut other: Local<Option<Rig>>,
    mut last: Local<Option<Mode>>,
) {
    if *last == Some(*mode) {
        return;
    }
    let first = last.is_none();
    *last = Some(*mode);
    for mut rig in &mut rigs {
        if first {
            if *mode == Mode::Materials {
                *rig = stage_rig();
            }
            continue;
        }
        let back = other.take().unwrap_or_else(|| {
            if *mode == Mode::Materials {
                stage_rig()
            } else {
                rig.clone()
            }
        });
        *other = Some(std::mem::replace(&mut *rig, back));
    }
}
