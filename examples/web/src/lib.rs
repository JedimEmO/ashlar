//! The explorable demo: ashlar's baked scenes and its material library, in a
//! window or a browser tab.
//!
//! One Bevy app with two modes. **Scenes** loads a baked `.ashlar` building and
//! the file-backed material library it wears through
//! [`AshlarPlugin`](ashlar_bevy::prelude::AshlarPlugin) — the path a game ships,
//! levels of detail and all — and lets a visitor orbit it, walk its streets,
//! light it by night, cut its storeys away and see its level bands.
//! **Materials** shows the default library on a sphere, a wall and a floor,
//! loaded from files like everything else; moving one of a material's
//! parameters re-bakes its graph on the spot, single-threaded, which is the
//! one thing here a shipped game would not do.
//!
//! Nothing it draws is committed. `examples/content.rs` writes the files, and
//! `just site` runs it into a staging directory under `target/`.
use bevy::{asset::AssetMetaCheck, prelude::*};

pub mod catalog;

mod camera;
#[cfg(not(target_arch = "wasm32"))]
mod capture;
mod gallery;
mod lighting;
mod scenes;
mod ui;

/// Which half of the demo is on screen.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// A baked building.
    #[default]
    Scenes,
    /// The material stage.
    Materials,
}

/// How the level-of-detail bands are shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LodView {
    /// As a game draws them: each level in its distance band.
    #[default]
    Bands,
    /// Every piece tinted by the level drawn at its distance from the
    /// camera, still banded.
    Tint,
    /// One level at every distance.
    Force(usize),
}

/// What a visitor has chosen. The UI writes it; the systems that act on it
/// run when it changes.
#[derive(Resource, Clone, Debug)]
pub struct View {
    /// The scene, as an index into [`catalog::SCENES`].
    pub scene: usize,
    /// Lit by night.
    pub night: bool,
    /// Hide every storey above this one.
    pub max_storey: Option<i32>,
    /// Hide the pieces seen from outside, which is what opens a building up.
    pub hide_exterior: bool,
    /// How levels of detail are shown.
    pub lod: LodView,
    /// The camera walks rather than orbits.
    pub walk: bool,
}

/// How the app starts: from the command line natively, from the page's query
/// string in a browser.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The scene to open on, by name.
    pub scene: Option<String>,
    /// Day or night, where given; otherwise the scene's own.
    pub night: Option<bool>,
    /// Open on the material stage, showing this library key.
    pub material: Option<String>,
    /// Parameter overrides for that material, `name=value`, applied as if a
    /// slider had moved them.
    pub params: Vec<(String, String)>,
    /// Hide every storey above this one.
    pub max_storey: Option<i32>,
    /// Hide the exterior.
    pub hide_exterior: bool,
    /// The level-of-detail view.
    pub lod: LodView,
    /// Start at street level, walking.
    pub walk: bool,
    /// Where the asset root is. Natively this defaults to the staging
    /// directory `just site` writes; in a browser it is `assets` beside the
    /// page and cannot be changed.
    pub assets: Option<String>,
    /// Capture one frame to this file once everything has loaded, and exit.
    pub screenshot: Option<String>,
}

/// The default asset root for a native run: where `just site` stages the
/// content step's output.
#[cfg(not(target_arch = "wasm32"))]
const NATIVE_ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/web/assets");

/// Build the app.
pub fn app(options: Options) -> App {
    let scene = options
        .scene
        .as_deref()
        .and_then(|name| catalog::SCENES.iter().position(|scene| scene.name == name))
        .unwrap_or(0);
    let view = View {
        scene,
        night: options.night.unwrap_or(catalog::SCENES[scene].night),
        max_storey: options.max_storey,
        hide_exterior: options.hide_exterior,
        lod: options.lod,
        walk: options.walk,
    };
    #[cfg(not(target_arch = "wasm32"))]
    let file_path = options
        .assets
        .clone()
        .unwrap_or_else(|| NATIVE_ASSETS.to_owned());
    #[cfg(target_arch = "wasm32")]
    let file_path = "assets".to_owned();

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(AssetPlugin {
                file_path,
                // A `.meta` file is a request per asset and a 404 per request
                // over HTTP; the content step writes none.
                meta_check: AssetMetaCheck::Never,
                ..default()
            })
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "ashlar — explorable demo".into(),
                    canvas: Some("#ashlar".into()),
                    fit_canvas_to_parent: true,
                    prevent_default_event_handling: true,
                    resolution: (1280, 800).into(),
                    ..default()
                }),
                ..default()
            }),
    )
    .add_plugins(bevy_egui::EguiPlugin::default())
    // Preflight reads every map a library names before the library loads,
    // which over HTTP is every scene's maps before the first one draws. The
    // content step that wrote them is the check this demo trusts; a scene
    // then downloads only the maps it wears.
    .add_plugins(ashlar_bevy::prelude::AshlarPlugin {
        preflight: false,
        ..default()
    })
    .insert_resource(view)
    .insert_resource(if options.material.is_some() {
        Mode::Materials
    } else {
        Mode::Scenes
    })
    .add_plugins((
        camera::plugin,
        lighting::plugin,
        scenes::plugin,
        gallery::plugin(options.material.clone(), options.params.clone()),
        ui::plugin,
    ));
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = options.screenshot {
        app.add_plugins(capture::plugin(path));
    }
    app
}
