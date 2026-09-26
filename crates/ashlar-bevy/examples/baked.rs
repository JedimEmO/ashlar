//! A game drawing baked content: no cargo features, no kernel, no graph engine.
//!
//! This is the whole production path in one screen. A content step wrote a
//! building as a `.ashlar` file — every level of detail, its collision proxies,
//! portals and rooms — and the materials it wears as KTX2 maps beside a
//! `library.materials.ron`. [`AshlarPlugin`] loads both through the asset
//! server and spawns the building with each level in its distance band; this
//! example adds a camera and two lights and frames the building once it has
//! spawned.
//!
//! The files are not committed. Write them first with the content step,
//! `just content`, which bakes the showcase's scenes into
//! `assets/buildings/<scene>.ashlar` and exports the material library into
//! `assets/materials/`; then run
//!
//! ```text
//! cargo run --example baked -p ashlar-bevy -- metropolis
//! ```
//!
//! `cargo tree -e normal -p ashlar-bevy` has neither `ashlar-manifold` nor
//! `ashlar-material` in it: that is the point.
//!
//! `--screenshot <path>` captures one frame and exits, which is how CI looks at
//! it without a person in front of it. `--night` lights it by night instead: a
//! dim cold moon, almost no ambient, a blue-black sky and bloom, so a city's
//! lit windows, neon and street lamps are what the frame is made of.
use std::path::PathBuf;

use ashlar_bevy::prelude::{
    AshlarBuilding, AshlarBuildingSpawned, AshlarPlugin, BakedBuildingAsset,
};
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
};

/// The workspace's `assets`, where the content step writes. A game points
/// [`AssetPlugin`] at its own.
const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");

/// The exported material library every baked building of the showcase wears.
const LIBRARY: &str = "materials/library.materials.ron";

/// What to load, where a `--screenshot` run writes its frame, and whether it
/// is night.
#[derive(Resource)]
struct Options {
    building: String,
    screenshot: Option<PathBuf>,
    night: bool,
}

/// The moon, in lux. Far above a real moon's third of a lux, because the camera
/// keeps Bevy's default exposure by night too: what matters is its ratio to the
/// emissive surfaces, which exposure does not scale, and at this a dark wall is
/// a dim shape and a lit window a highlight. The preview's `--night` uses the
/// same number.
const MOON: f32 = 1000.0;

fn setup(mut commands: Commands, server: Res<AssetServer>, options: Res<Options>) {
    commands.spawn((
        Name::new(options.building.clone()),
        AshlarBuilding {
            building: server.load(format!("buildings/{}.ashlar", options.building)),
            materials: server.load(LIBRARY),
        },
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: if options.night { MOON } else { 12_000.0 },
            color: if options.night {
                Color::srgb(0.62, 0.72, 1.0)
            } else {
                Color::WHITE
            },
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(40.0, 80.0, 50.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    let camera = commands
        .spawn((
            Camera3d::default(),
            Transform::from_xyz(30.0, 20.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y),
        ))
        .id();
    if options.night {
        // Bloom is what turns a neon tube into a glow; it brings its own HDR
        // target.
        commands
            .entity(camera)
            .insert(bevy::post_process::bloom::Bloom::NATURAL);
        commands.insert_resource(ClearColor(Color::srgb(0.012, 0.016, 0.03)));
        commands.insert_resource(GlobalAmbientLight {
            brightness: 30.0,
            ..default()
        });
    }
}

/// Stand the camera off the building's level-zero bounds once it has spawned.
fn frame(
    mut spawned: MessageReader<AshlarBuildingSpawned>,
    roots: Query<&AshlarBuilding>,
    buildings: Res<Assets<BakedBuildingAsset>>,
    mut camera: Single<&mut Transform, With<Camera3d>>,
) {
    for message in spawned.read() {
        let Some(baked) = roots
            .get(message.entity)
            .ok()
            .and_then(|root| buildings.get(&root.building))
        else {
            continue;
        };
        let Some(level) = baked.0.level(0) else {
            continue;
        };
        let (mut low, mut high) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for piece in level.pieces() {
            for point in &piece.mesh.positions {
                let point = piece.pose.transform_point(*point).as_vec3();
                low = low.min(point);
                high = high.max(point);
            }
        }
        if low.x > high.x {
            continue;
        }
        let centre = (low + high) / 2.0;
        let reach = (high - low).length().max(4.0);
        **camera = Transform::from_translation(centre + Vec3::new(0.7, 0.45, 0.8) * reach)
            .looking_at(centre, Vec3::Y);
    }
}

/// Leave with a failure when the building could not be spawned: the reason is
/// on the root, and waiting for a frame that will never come helps nobody.
fn fail(failed: Query<&ashlar_bevy::prelude::AshlarFailed>, mut exit: MessageWriter<AppExit>) {
    for failure in &failed {
        error!("the building did not spawn: {}", failure.0);
        exit.write(AppExit::error());
    }
}

/// Capture one frame and leave, a few seconds after the building spawned so
/// its maps have arrived.
fn capture(
    mut commands: Commands,
    options: Res<Options>,
    spawned: Query<(), With<ashlar_bevy::prelude::AshlarSpawned>>,
    mut frames: Local<u32>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(path) = options.screenshot.clone() else {
        return;
    };
    if spawned.is_empty() {
        return;
    }
    *frames += 1;
    if *frames == 120 {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path))
            .observe(
                |_: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>| {
                    exit.write(AppExit::Success);
                },
            );
    }
    // The observer above is what normally exits; this keeps a run that cannot
    // capture from hanging forever in CI.
    if *frames > 600 {
        exit.write(AppExit::Success);
    }
}

fn main() {
    let mut building = "outpost".to_owned();
    let mut screenshot = None;
    let mut night = false;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--screenshot" {
            screenshot = arguments.next().map(PathBuf::from);
        } else if argument == "--night" {
            night = true;
        } else {
            building = argument;
        }
    }
    App::new()
        .add_plugins((
            DefaultPlugins.set(AssetPlugin {
                file_path: ASSETS.to_owned(),
                ..default()
            }),
            AshlarPlugin::default(),
        ))
        .insert_resource(Options {
            building,
            screenshot,
            night,
        })
        .add_systems(Startup, setup)
        .add_systems(Update, (frame, capture, fail))
        .run();
}
