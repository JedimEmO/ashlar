//! Day and night, ported from the preview's rig (`--night`): a key and a fill,
//! an ambient, a sky, and by night bloom and a lamp in every small strong
//! emitter.
//!
//! The numbers are the preview's, and its doc comments say why each is what
//! it is; the one thing this adds is a budget. WebGL2 clusters at most 204
//! lights and a metropolis has hundreds of lamps, so every lamp is spawned
//! and only the ones nearest the camera are lit.
use bevy::{light::CascadeShadowConfigBuilder, post_process::bloom::Bloom, prelude::*};

use crate::{Mode, View, camera::MainCamera};

/// Registers the rig.
pub(crate) fn plugin(app: &mut App) {
    app.insert_resource(ClearColor(DAY_SKY))
        .add_systems(Startup, spawn)
        .add_systems(Update, (rig, budget_lamps));
}

/// The key by day, in lux.
const DAY_KEY: f32 = 14_000.0;

/// The fill by day, as a share of the key.
const DAY_FILL: f32 = 2500.0 / 14_000.0;

/// The ambient by day.
const DAY_AMBIENT: f32 = 350.0;

/// The sky by day.
const DAY_SKY: Color = Color::srgb(0.075, 0.09, 0.115);

/// The moon, in lux: far above a real one, because the camera keeps its
/// daytime exposure and what matters is the ratio to the emissive surfaces.
const NIGHT_KEY: f32 = 600.0;

/// The fill by night, as a share of the key: the sky glow over a city.
const NIGHT_FILL: f32 = 0.12;

/// The ambient by night: enough that a wall in shadow is not a hole.
const NIGHT_AMBIENT: f32 = 30.0;

/// The sky by night: blue-black, so a roofline reads against it.
const NIGHT_SKY: Color = Color::srgb(0.012, 0.016, 0.03);

/// How much of a definition's emission shows by day. The library's emissive
/// values are tuned by night; against the sun they turn every lit office into
/// an orange panel.
pub(crate) const DAY_EMISSIVE: f32 = 0.25;

/// The most lamps lit at once. Well inside WebGL2's 204 clustered lights, and
/// the preview's metropolis lights 384, so the nearest ones are chosen.
#[cfg(target_arch = "wasm32")]
const LIT_LAMPS: usize = 64;
#[cfg(not(target_arch = "wasm32"))]
const LIT_LAMPS: usize = 128;

/// The key light.
#[derive(Component)]
struct Key;

/// The fill light.
#[derive(Component)]
struct Fill;

/// A lamp the night rig may light: spawned hidden, and shown when it is one of
/// the [`LIT_LAMPS`] nearest the camera by night.
#[derive(Component)]
pub(crate) struct Lamp;

fn spawn(mut commands: Commands) {
    commands.spawn((
        Key,
        DirectionalLight {
            illuminance: DAY_KEY,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(-10.0, 18.0, -14.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    // WebGL2 draws one directional light, and a second is dropped with a
    // warning every frame; there the ambient carries the fill's share.
    if !WEBGL2 {
        commands.spawn((
            Fill,
            DirectionalLight {
                illuminance: DAY_KEY * DAY_FILL,
                color: Color::srgb(0.65, 0.78, 1.0),
                ..default()
            },
            Transform::from_xyz(10.0, 8.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y),
        ));
    }
}

/// Whether the renderer is WebGL2, which the browser build always is.
const WEBGL2: bool = cfg!(target_arch = "wasm32");

/// How much the ambient is raised where there is no fill light.
const NO_FILL_AMBIENT: f32 = 1.6;

/// The lighting a mode and a view ask for. The material stage is always lit
/// by day: a material is judged under the light it was tuned in.
fn is_night(mode: Mode, view: &View) -> bool {
    mode == Mode::Scenes && view.night
}

/// Set the key, fill, ambient, sky and bloom from the view, and the shadow
/// reach from the scene's size.
#[expect(
    clippy::too_many_arguments,
    reason = "a Bevy system's parameters are its inputs"
)]
fn rig(
    mut commands: Commands,
    mode: Res<Mode>,
    view: Res<View>,
    extent: Res<crate::scenes::Extent>,
    mut key: Query<(&mut DirectionalLight, Entity), (With<Key>, Without<Fill>)>,
    mut fill: Query<&mut DirectionalLight, (With<Fill>, Without<Key>)>,
    camera: Query<Entity, With<MainCamera>>,
    mut clear: ResMut<ClearColor>,
    mut ambient: ResMut<GlobalAmbientLight>,
) {
    if !(mode.is_changed() || view.is_changed() || extent.is_changed()) {
        return;
    }
    let night = is_night(*mode, &view);
    // The stage is a few metres across whatever scene was last loaded.
    let reach = if *mode == Mode::Materials {
        8.0
    } else {
        extent.0
    };
    let (illuminance, sky, glow) = if night {
        (NIGHT_KEY, NIGHT_SKY, NIGHT_AMBIENT)
    } else {
        (DAY_KEY, DAY_SKY, DAY_AMBIENT)
    };
    for (mut light, entity) in &mut key {
        light.illuminance = illuminance;
        // Moonlight is sunlight, but the eye reads it cold.
        light.color = if night {
            Color::srgb(0.62, 0.72, 1.0)
        } else {
            Color::WHITE
        };
        // Shadows reach past the far side of the scene from where the camera
        // opens on it, and no further: a cascade spread over a kilometre is a
        // blur on the street in front of you, and one that ends short of the
        // building leaves it standing over a shadow cut in half.
        commands.entity(entity).insert(
            CascadeShadowConfigBuilder {
                num_cascades: if cfg!(target_arch = "wasm32") { 1 } else { 3 },
                maximum_distance: (reach * 2.5).clamp(40.0, 500.0),
                first_cascade_far_bound: (reach * 0.3).clamp(10.0, 120.0),
                ..default()
            }
            .build(),
        );
    }
    for mut light in &mut fill {
        light.illuminance = illuminance * if night { NIGHT_FILL } else { DAY_FILL };
        light.color = if night {
            Color::srgb(0.4, 0.45, 0.75)
        } else {
            Color::srgb(0.65, 0.78, 1.0)
        };
    }
    clear.0 = sky;
    ambient.brightness = glow * if WEBGL2 { NO_FILL_AMBIENT } else { 1.0 };
    for camera in &camera {
        if night {
            // Bloom is what turns a neon tube into a glow.
            commands.entity(camera).insert(Bloom::NATURAL);
        } else {
            commands.entity(camera).remove::<Bloom>();
        }
    }
}

/// How often the lit lamps are chosen again, in seconds.
const REBUDGET: f32 = 0.25;

/// Light the lamps nearest the camera, by night; none by day.
fn budget_lamps(
    mode: Res<Mode>,
    view: Res<View>,
    time: Res<Time>,
    mut since: Local<f32>,
    camera: Query<&GlobalTransform, With<MainCamera>>,
    mut lamps: Query<(Entity, &GlobalTransform, &mut Visibility), With<Lamp>>,
) {
    *since += time.delta_secs();
    if *since < REBUDGET && !view.is_changed() && !mode.is_changed() {
        return;
    }
    *since = 0.0;
    let night = is_night(*mode, &view);
    let Ok(eye) = camera.single().map(GlobalTransform::translation) else {
        return;
    };
    let mut near: Vec<(f32, Entity)> = lamps
        .iter()
        .map(|(entity, at, _)| (at.translation().distance_squared(eye), entity))
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    near.truncate(if night { LIT_LAMPS } else { 0 });
    for (entity, _, mut visibility) in &mut lamps {
        let lit = near.iter().any(|(_, lamp)| *lamp == entity);
        visibility.set_if_neq(if lit {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
}
