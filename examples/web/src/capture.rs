//! `--screenshot <path>`: capture one frame once everything on screen has
//! loaded, and exit. How the demo is looked at without a person in front of
//! it, natively; a browser run is checked with a headless browser instead.
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
};

use crate::{
    Mode,
    gallery::{BakeState, Gallery},
    scenes::{Load, Status},
};

/// Registers the capture, writing to `path`.
pub(crate) fn plugin(path: String) -> impl Fn(&mut App) + Send + Sync + 'static {
    move |app: &mut App| {
        app.insert_resource(Capture {
            path: path.clone(),
            settled: 0,
            frames: 0,
            taken: false,
        })
        .add_systems(Last, capture);
    }
}

#[derive(Resource)]
struct Capture {
    path: String,
    /// Frames since everything reported loaded.
    settled: u32,
    /// Frames since the app started.
    frames: u32,
    taken: bool,
}

/// Frames to let a loaded scene settle before capturing: the lamps budget,
/// shadow maps and bloom all take a frame or two, and a texture uploaded this
/// frame draws the next.
const SETTLE: u32 = 30;

/// Frames after which a capture that never became ready gives up.
const GIVE_UP: u32 = 5000;

#[expect(
    clippy::too_many_arguments,
    reason = "a Bevy system's parameters are its inputs"
)]
fn capture(
    mut commands: Commands,
    mut capture: ResMut<Capture>,
    mode: Res<Mode>,
    status: Res<Status>,
    gallery: Option<Res<Gallery>>,
    worn: Query<&MeshMaterial3d<StandardMaterial>>,
    materials: Res<Assets<StandardMaterial>>,
    server: Res<AssetServer>,
    mut exit: MessageWriter<AppExit>,
) {
    capture.frames += 1;
    if capture.frames > GIVE_UP {
        error!("gave up waiting to capture {}", capture.path);
        exit.write(AppExit::error());
        return;
    }
    if capture.taken {
        return;
    }
    let ready = match *mode {
        Mode::Scenes => match &status.load {
            Load::Ready => true,
            Load::Failed(reason) => {
                error!("the scene failed: {reason}");
                exit.write(AppExit::error());
                return;
            }
            Load::Idle | Load::Loading => false,
        },
        Mode::Materials => gallery.is_some_and(|gallery| {
            gallery.changed.is_none()
                && gallery.state != BakeState::Baking
                && !gallery.keys.is_empty()
        }),
    };
    // Every map anything on screen wears has arrived or failed.
    let loaded = worn.iter().all(|material| {
        materials.get(&material.0).is_none_or(|material| {
            [
                &material.base_color_texture,
                &material.normal_map_texture,
                &material.metallic_roughness_texture,
                &material.emissive_texture,
            ]
            .into_iter()
            .flatten()
            .all(|image| {
                server.is_loaded(image)
                    || server.load_state(image).is_failed()
                    || server.get_load_state(image).is_none()
            })
        })
    });
    if !(ready && loaded) {
        capture.settled = 0;
        return;
    }
    capture.settled += 1;
    if capture.settled < SETTLE {
        return;
    }
    capture.taken = true;
    let path = capture.path.clone();
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path))
        .observe(
            |_: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>| {
                exit.write(AppExit::Success);
            },
        );
}
