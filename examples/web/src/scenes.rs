//! The scenes: a baked building loaded the way a game loads one, and the
//! views a visitor can put over it.
//!
//! Everything a building draws is spawned by `AshlarPlugin` from a `.ashlar`
//! file and a file-backed material library; this module asks for one, frames
//! the camera on it when it arrives, and then only hides, shows and re-dresses
//! the pieces the plugin spawned. A storey cut is `AshlarPiece::storey`, the
//! exterior toggle `AshlarPiece::side`, and the level-of-detail views read
//! `AshlarPiece::level` and the `VisibilityRange` the plugin gave the piece.
//!
//! In a browser that range is an abrupt one, cut at the middle of each
//! crossfade, because Bevy 0.19.1 cannot crossfade on WebGL2 (see
//! `ashlar_bevy::baked`). Nothing here has to know: Bevy culls to either kind,
//! and the views only take a range off and put it back.
use std::collections::HashMap;
use std::io::Read;

use ashlar_bevy::prelude::{
    AshlarBuilding, AshlarBuildingSpawned, AshlarFailed, AshlarPiece, BakedBuildingAsset,
    MaterialLibraryAsset,
};
use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    camera::visibility::VisibilityRange,
    ecs::entity_disabling::Disabled,
    light::NotShadowCaster,
    prelude::*,
};

use crate::{
    LodView, Mode, View,
    camera::Rig,
    catalog,
    lighting::{DAY_EMISSIVE, Lamp, WEBGL2},
};

/// Registers the scene half.
pub(crate) fn plugin(app: &mut App) {
    app.register_asset_loader(GzBuildingLoader)
        .init_resource::<Status>()
        .insert_resource(Extent(60.0))
        .init_resource::<Emission>()
        .add_observer(record)
        .add_observer(record_range)
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                switch,
                spawned,
                failed,
                apply_view,
                apply_emission,
                show_mode,
            )
                .chain(),
        );
}

/// Loads a `.ashlar.gz` file: a baked building, gzipped by the web content
/// step so a static host that serves it uncompressed still sends an eighth of
/// it. Inflated here and read by the same reader the plain loader uses.
#[derive(Default, TypePath)]
struct GzBuildingLoader;

impl AssetLoader for GzBuildingLoader {
    type Asset = BakedBuildingAsset;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &(),
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut gzipped = Vec::new();
        reader.read_to_end(&mut gzipped).await?;
        let mut bytes = Vec::new();
        flate2::read::GzDecoder::new(gzipped.as_slice()).read_to_end(&mut bytes)?;
        let building = ashlar::BakedBuilding::read(&bytes)
            .map_err(|error| format!("reading {}: {error}", load_context.path()))?;
        Ok(BakedBuildingAsset(building))
    }

    fn extensions(&self) -> &[&str] {
        &["ashlar.gz"]
    }
}

/// How far the current scene reaches, in metres across its bounds: what the
/// shadows and the camera scale to.
#[derive(Resource, Clone, Copy, Debug)]
pub(crate) struct Extent(pub f32);

/// Where the current scene is in loading, for the panel.
#[derive(Resource, Default, Debug)]
pub(crate) struct Status {
    /// The scene asked for, as an index into [`catalog::SCENES`].
    pub scene: Option<usize>,
    /// How far it has got.
    pub load: Load,
    /// What it turned out to be, once spawned.
    pub info: Option<Info>,
}

/// How far a scene has got.
#[derive(Default, Debug, Clone, PartialEq)]
pub(crate) enum Load {
    /// Nothing asked for yet.
    #[default]
    Idle,
    /// The building file is on its way.
    Loading,
    /// Spawned; its maps may still be arriving.
    Ready,
    /// It will not spawn, and why.
    Failed(String),
}

/// What a spawned scene is, for the panel and the view.
#[derive(Debug, Clone)]
pub(crate) struct Info {
    /// The lowest and highest storey any piece declares.
    pub storeys: Option<(i32, i32)>,
    /// Each level of detail: its unique triangles and where it stops drawing.
    pub levels: Vec<(usize, Option<f32>)>,
    /// Lamps the night rig can light.
    pub lamps: usize,
}

/// The scene library, loaded once and worn by every scene.
#[derive(Resource)]
struct Library(Handle<MaterialLibraryAsset>);

/// The entity a scene is spawned under.
#[derive(Component)]
pub(crate) struct SceneRoot;

/// The ground a scene stands on, which darkens by night.
#[derive(Component)]
struct Ground;

/// The material the plugin gave a piece, kept so a view can re-dress it and
/// put it back.
#[derive(Component)]
struct Kept {
    material: Handle<StandardMaterial>,
}

/// One flat material per level of detail, for the tinted view.
#[derive(Resource)]
struct Tints(Vec<Handle<StandardMaterial>>);

/// Each scene material's emission as the library gave it, so day can turn it
/// down and night put it back.
#[derive(Resource, Default)]
struct Emission(HashMap<AssetId<StandardMaterial>, LinearRgba>);

/// The level tints: green is the building as authored, and each coarser level
/// warmer.
pub(crate) const TINTS: [Color; 6] = [
    Color::srgb(0.25, 0.75, 0.35),
    Color::srgb(0.9, 0.8, 0.2),
    Color::srgb(0.95, 0.5, 0.15),
    Color::srgb(0.85, 0.2, 0.2),
    Color::srgb(0.6, 0.25, 0.8),
    Color::srgb(0.3, 0.4, 0.9),
];

fn setup(
    mut commands: Commands,
    server: Res<AssetServer>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(Library(server.load(catalog::SCENE_LIBRARY)));
    commands.insert_resource(Tints(
        TINTS
            .iter()
            .map(|color| {
                materials.add(StandardMaterial {
                    base_color: *color,
                    perceptual_roughness: 0.8,
                    ..default()
                })
            })
            .collect(),
    ));
}

/// Load the scene the view asks for, once the scene half is on screen: a
/// visitor who opens on the material stage downloads no building.
fn switch(
    mut commands: Commands,
    mode: Res<Mode>,
    view: Res<View>,
    server: Res<AssetServer>,
    library: Res<Library>,
    roots: Query<Entity, With<SceneRoot>>,
    mut status: ResMut<Status>,
) {
    if *mode != Mode::Scenes || status.scene == Some(view.scene) {
        return;
    }
    for root in &roots {
        commands.entity(root).despawn();
    }
    let scene = &catalog::SCENES[view.scene];
    commands.spawn((
        SceneRoot,
        Name::new(scene.name),
        AshlarBuilding {
            building: server.load(scene.path()),
            materials: library.0.clone(),
        },
    ));
    *status = Status {
        scene: Some(view.scene),
        load: Load::Loading,
        info: None,
    };
}

/// The brightest a small piece's emission has to be, in its brightest
/// channel, before the night rig stands a lamp in it. A lit window is about
/// one, a strip light four, neon eight and a sodium lamp ten.
const LAMP_PEAK: f32 = 5.0;

/// The largest a piece can be across and still be a lamp rather than a sign
/// or a lit facade, in metres.
const LAMP_EXTENT: f64 = 2.5;

/// Lumens per unit of the brightest emissive channel.
const LAMP_LUMENS: f32 = 300_000.0;

/// How much of a lamp's light an upright sign throws.
const SIGN_SHARE: f32 = 0.03;

/// How far a lamp reaches, in metres.
const LAMP_RANGE: f32 = 16.0;

/// Stand a lamp in a piece spanning `near` to `far` that emits `emissive`, if
/// it is one: small, and bright in its brightest channel. Spawned hidden;
/// [`crate::lighting`] lights the nearest.
fn lamp(
    commands: &mut Commands,
    emissive: [f32; 3],
    near: ashlar::glam::DVec3,
    far: ashlar::glam::DVec3,
) -> Option<Entity> {
    let [r, g, b] = emissive;
    let peak = r.max(g).max(b);
    let size = far - near;
    if peak < LAMP_PEAK || size.max_element() > LAMP_EXTENT {
        return None;
    }
    let color = Color::linear_rgb(r / peak, g / peak, b / peak);
    let at = ((near + far) * 0.5).as_vec3();
    // A flat head is a street lamp and shines down; anything upright glows
    // every way.
    Some(if size.y < 0.5 * size.x.max(size.z) {
        commands
            .spawn((
                Lamp,
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
                Visibility::Hidden,
            ))
            .id()
    } else {
        commands
            .spawn((
                Lamp,
                PointLight {
                    color,
                    intensity: peak * LAMP_LUMENS * SIGN_SHARE,
                    range: LAMP_RANGE * 0.6,
                    radius: 0.1,
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::from_translation(at),
                Visibility::Hidden,
            ))
            .id()
    })
}

/// Frame the camera on a building that just spawned, stand it on a ground,
/// and put a lamp in every small strong emitter it has.
#[expect(
    clippy::too_many_arguments,
    reason = "a Bevy system's parameters are its inputs"
)]
fn spawned(
    mut commands: Commands,
    mut messages: MessageReader<AshlarBuildingSpawned>,
    roots: Query<&AshlarBuilding, With<SceneRoot>>,
    buildings: Res<Assets<BakedBuildingAsset>>,
    libraries: Res<Assets<MaterialLibraryAsset>>,
    view: Res<View>,
    mut status: ResMut<Status>,
    mut extent: ResMut<Extent>,
    mut rigs: Query<&mut Rig>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for message in messages.read() {
        let Some((baked, library)) = roots.get(message.entity).ok().and_then(|root| {
            Some((
                &buildings.get(&root.building)?.0,
                &libraries.get(&root.materials)?.0,
            ))
        }) else {
            continue;
        };
        let Some(level) = baked.level(0) else {
            continue;
        };
        let (mut low, mut high) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        let mut storeys: Option<(i32, i32)> = None;
        let mut lamps = 0;
        for piece in level.pieces() {
            if let Some(storey) = piece.storey {
                storeys =
                    Some(storeys.map_or((storey, storey), |(a, b)| (a.min(storey), b.max(storey))));
            }
            let (mut near, mut far) = (
                ashlar::glam::DVec3::splat(f64::MAX),
                ashlar::glam::DVec3::splat(f64::MIN),
            );
            for point in &piece.mesh.positions {
                let point = piece.pose.transform_point(*point);
                near = near.min(point);
                far = far.max(point);
            }
            if piece.mesh.positions.is_empty() {
                continue;
            }
            low = low.min(near.as_vec3());
            high = high.max(far.as_vec3());
            let Some(definition) = library.materials.get(&piece.binding.material) else {
                continue;
            };
            if let Some(lamp) = lamp(&mut commands, definition.emissive, near, far) {
                lamps += 1;
                commands.entity(message.entity).add_child(lamp);
            }
        }
        if low.x > high.x {
            continue;
        }
        let scene = catalog::SCENES[view.scene];
        for mut rig in &mut rigs {
            rig.frame(low, high, scene.distance, scene.pitch);
            if view.walk {
                rig.street_level();
            }
        }
        extent.0 = (high - low).length();
        // Far enough that the camera never sees past its edge.
        let reach = (extent.0 * 4.0).max(600.0);
        let ground = commands
            .spawn((
                Ground,
                Mesh3d(meshes.add(Plane3d::default().mesh().size(reach, reach))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgb(0.16, 0.18, 0.21),
                    perceptual_roughness: 1.0,
                    ..default()
                })),
                Transform::from_xyz(
                    f32::midpoint(low.x, high.x),
                    low.y - 0.02,
                    f32::midpoint(low.z, high.z),
                ),
            ))
            .id();
        commands.entity(message.entity).add_child(ground);
        status.load = Load::Ready;
        status.info = Some(Info {
            storeys,
            levels: baked
                .triangles()
                .into_iter()
                .zip(baked.levels.iter().map(|level| level.until))
                .collect(),
            lamps,
        });
    }
}

/// Say why a scene did not spawn.
fn failed(roots: Query<&AshlarFailed, With<SceneRoot>>, mut status: ResMut<Status>) {
    for failure in &roots {
        if status.load != Load::Failed(failure.0.clone()) {
            status.load = Load::Failed(failure.0.clone());
        }
    }
}

/// Keep the material the plugin gave each new piece. In a browser, a piece
/// seen from inside casts no shadow: the building's own walls already shade
/// its rooms from the sun, and the shadow pass is a third of the frame.
fn record(
    add: On<Add, AshlarPiece>,
    mut commands: Commands,
    pieces: Query<(&MeshMaterial3d<StandardMaterial>, &AshlarPiece)>,
) {
    if let Ok((material, piece)) = pieces.get(add.entity) {
        let mut entity = commands.entity(add.entity);
        entity.insert(Kept {
            material: material.0.clone(),
        });
        if WEBGL2 && piece.side == ashlar::Side::Interior {
            entity.insert(NotShadowCaster);
        }
    }
}

/// The band the plugin gave a piece, kept so the forced-level view can take
/// it off and put it back, and so [`crate::streaming`] can group by it. A
/// piece with none draws at every distance.
#[derive(Component)]
pub(crate) struct KeptRange(pub VisibilityRange);

/// Keep the band the plugin gives each new piece. The plugin inserts it after
/// spawning the piece, so this watches the range rather than the piece; a
/// range the view puts back is the same range kept again.
fn record_range(
    add: On<Add, VisibilityRange>,
    mut commands: Commands,
    pieces: Query<&VisibilityRange, With<AshlarPiece>>,
) {
    if let Ok(range) = pieces.get(add.entity) {
        commands.entity(add.entity).insert(KeptRange(range.clone()));
    }
}

/// Whether a piece draws at `level`: it starts at or before it, and its band
/// runs at least to where that level stops. A cut band ends past the
/// boundary too, at the middle of the crossfade it replaced.
fn covers(
    piece: &AshlarPiece,
    range: Option<&KeptRange>,
    levels: &[(usize, Option<f32>)],
    level: usize,
) -> bool {
    let until = levels
        .get(level)
        .and_then(|(_, until)| *until)
        .unwrap_or(f32::MAX);
    piece.level <= level && range.is_none_or(|range| range.0.end_margin.start >= until)
}

/// The level drawn at `distance`: the first whose band reaches past it.
fn drawn_at(levels: &[(usize, Option<f32>)], distance: f32) -> usize {
    levels
        .iter()
        .position(|(_, until)| until.is_none_or(|until| distance < until))
        .unwrap_or(levels.len().saturating_sub(1))
}

/// How often the tinted view re-reads the camera, in seconds.
const REBAND: f32 = 0.1;

/// Hide, show and re-dress pieces as the view says: the storey cut, the
/// exterior, and the level-of-detail view.
#[expect(clippy::type_complexity, reason = "a query over what a piece carries")]
#[expect(
    clippy::too_many_arguments,
    reason = "a Bevy system's parameters are its inputs"
)]
fn apply_view(
    mut commands: Commands,
    view: Res<View>,
    status: Res<Status>,
    tints: Res<Tints>,
    time: Res<Time>,
    mut since: Local<f32>,
    added: Query<(), Added<Kept>>,
    camera: Query<&GlobalTransform, With<crate::camera::MainCamera>>,
    // Streamed-out pieces too, so they come back cut and dressed as the view
    // says.
    mut pieces: Query<
        (
            Entity,
            &AshlarPiece,
            &Kept,
            Option<&KeptRange>,
            &GlobalTransform,
            &mut Visibility,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        Allow<Disabled>,
    >,
) {
    *since += time.delta_secs();
    // The tint follows the camera: it shows the level drawn from here.
    let retint = view.lod == LodView::Tint && *since >= REBAND;
    if !view.is_changed() && added.is_empty() && !retint {
        return;
    }
    *since = 0.0;
    let eye = camera
        .single()
        .map_or(Vec3::ZERO, GlobalTransform::translation);
    let levels = status
        .info
        .as_ref()
        .map_or(&[][..], |info| info.levels.as_slice());
    for (entity, piece, kept, range, at, mut visibility, mut material) in &mut pieces {
        let mut shown = view
            .max_storey
            .is_none_or(|cut| piece.storey.is_none_or(|storey| storey <= cut))
            && !(view.hide_exterior && piece.side == ashlar::Side::Exterior);
        match view.lod {
            LodView::Force(level) => {
                shown &= covers(piece, range, levels, level);
                commands.entity(entity).remove::<VisibilityRange>();
            }
            LodView::Bands | LodView::Tint => {
                if let Some(range) = range {
                    commands.entity(entity).insert(range.0.clone());
                }
            }
        }
        let dressed = match view.lod {
            LodView::Tint => {
                let level = drawn_at(levels, at.translation().distance(eye)).max(piece.level);
                tints.0[level.min(tints.0.len() - 1)].clone()
            }
            LodView::Bands | LodView::Force(_) => kept.material.clone(),
        };
        if material.0 != dressed {
            material.0 = dressed;
        }
        visibility.set_if_neq(if shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
}

/// By day, turn the scene's emission down to what reads against the sun; by
/// night, put it back. Darken the ground by night too, or a moonlit plane
/// outshines the street standing on it.
fn apply_emission(
    view: Res<View>,
    added: Query<(), Added<Kept>>,
    pieces: Query<&Kept, Allow<Disabled>>,
    grounds: Query<&MeshMaterial3d<StandardMaterial>, With<Ground>>,
    added_ground: Query<(), Added<Ground>>,
    mut original: ResMut<Emission>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !view.is_changed() && added.is_empty() && added_ground.is_empty() {
        return;
    }
    let scale = if view.night { 1.0 } else { DAY_EMISSIVE };
    for kept in &pieces {
        let id = kept.material.id();
        let Some(material) = materials.get(id) else {
            continue;
        };
        let emissive = *original.0.entry(id).or_insert(material.emissive);
        let wanted = emissive * scale;
        if material.emissive != wanted
            && let Some(mut material) = materials.get_mut(id)
        {
            material.emissive = wanted;
        }
    }
    for ground in &grounds {
        if let Some(mut material) = materials.get_mut(&ground.0) {
            material.base_color = if view.night {
                Color::srgb(0.035, 0.037, 0.042)
            } else {
                Color::srgb(0.16, 0.18, 0.21)
            };
        }
    }
}

/// Hide the scene while the material stage is up.
fn show_mode(mode: Res<Mode>, mut roots: Query<&mut Visibility, With<SceneRoot>>) {
    let wanted = if *mode == Mode::Scenes {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut visibility in &mut roots {
        visibility.set_if_neq(wanted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_drawn_is_the_first_whose_band_reaches_past_the_distance() {
        let levels = [(100, Some(60.0)), (50, Some(250.0)), (10, None)];
        assert_eq!(drawn_at(&levels, 10.0), 0);
        assert_eq!(drawn_at(&levels, 60.0), 1);
        assert_eq!(drawn_at(&levels, 1000.0), 2);
        assert_eq!(drawn_at(&[], 10.0), 0);
    }
}
