//! A whole building, drawn by a game: no cargo features, no material graphs.
//!
//! This is the join `AshlarPlugin` makes, done by hand, for a caller that
//! holds its own meshes rather than a `.ashlar` file (`baked.rs` is the plugin
//! path). A recipe is authored in code, `ashlar-manifold` evaluates it once per
//! part, a material library binds its two slots to baked texture sets that
//! already exist on disk, and [`ashlar_bevy::drawables`] turns the two into
//! meshes, materials and poses.
//! Nothing here bakes anything, and nothing here links a graph engine: export
//! the maps once with `just materials` (the content step), then build it with
//! `cargo run --example building -p ashlar-bevy` and the tool features stay
//! off.
//!
//! The library is written in code because an example should be readable in one
//! screen. A game holding a library in memory hands it to
//! [`ashlar_bevy::check_library`], and one reading RON uses [`ashlar_bevy::read_library`]; both
//! open every map before the window does, so a missing file is a startup error
//! naming the key.
//!
//! `--screenshot <path>` captures one frame and exits, which is how CI looks at
//! it without a person in front of it.
use std::{collections::BTreeMap, path::PathBuf};

use ashlar::{
    Building, Collision, Element, Geometry, Instance, MaterialDefinition, MaterialLibrary, Part,
    Pose, Surface,
};
use ashlar_manifold::{ManifoldMesher, mesh_building};
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
};

/// Where the exported texture sets live, as an absolute path.
///
/// The workspace's `assets`, not this crate's: `ashlar-bevy` ships no content,
/// and the maps this example wears are the default library's, which
/// `just materials` exports there. A game points [`AssetPlugin`] at its own.
const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");

/// The default library's brick and plaster, as the two maps on disk and the
/// metres of wall one repeat of each covers.
///
/// The repeats are the ones the graphs behind these files were drawn for. A
/// definition is free to lay a map at another size, but a repeat of a different
/// *shape* stretches relief that was authored in metres.
const SETS: [(&str, &str, f32); 2] = [
    ("example:brick", "brick", 2.0),
    ("example:plaster", "plaster", 2.0),
];

/// A material library over the shipped sets: one definition per entry above.
///
/// White base colour and unit roughness and metallic, because every one of
/// those is already in the maps and a definition's constants *multiply* what a
/// map says. The normal map carries `baked_from`, which is how [`create_material`] knows
/// its green channel is the bake's convention rather than a photograph's.
///
/// [`create_material`]: ashlar_bevy::create_material
fn library() -> MaterialLibrary {
    let materials = SETS
        .iter()
        .map(|(key, set, repeat)| {
            let map = |name: &str| Some(format!("materials/library/{set}/{name}"));
            (
                (*key).to_owned(),
                MaterialDefinition {
                    base_color: [1.0, 1.0, 1.0],
                    roughness: 1.0,
                    metallic: 1.0,
                    tile_metres: [*repeat, *repeat],
                    surface: Surface::Files {
                        base_color: map("base.ktx2"),
                        normal: map("normal.ktx2"),
                        orm: map("orm.ktx2"),
                        height: map("height.ktx2"),
                        emissive: None,
                        // Recorded so the normal map is read in the bake's
                        // convention: see `ashlar_bevy::create_material`. The content
                        // step bakes the library's own definition, which is
                        // the graph's defaults at 512.
                        baked_from: Some(ashlar::Bake {
                            graph: format!("library:{set}"),
                            params: BTreeMap::new(),
                            resolution: 512,
                        }),
                    },
                    ..default()
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    MaterialLibrary { materials }
}

/// Three parts and two slots: a plaster ground floor, a brick upper storey with
/// its windows cut out, and a plaster cornice over both.
///
/// The upper storey is the part worth reading. Its windows are one cutter
/// arrayed three times, and `cut_material` sends the faces the cutter *made* to
/// the trim slot — so the reveals are plaster where the wall around them is
/// brick, out of one element and with no second solid to keep aligned. That
/// split is also why the meshes cannot simply be keyed by element: an element
/// with a cut slot is two batches, and `is_cut` is what tells them apart.
fn building() -> anyhow::Result<Building> {
    let ground = Part::builder("ground")
        .element(
            Element::new("body", Geometry::cuboid([8.0, 3.0, 6.0]), "trim")
                .collision(Collision::Bounds),
        )
        .build()?;
    let windows = Geometry::cuboid([1.3, 1.9, 7.0])
        .placed(Pose::at([1.4, 0.9, -0.5]))
        .arrayed(3, Pose::at([2.6, 0.0, 0.0]));
    let upper = Part::builder("upper")
        .element(
            Element::new(
                "body",
                Geometry::cuboid([8.0, 3.5, 6.0]).subtract(windows),
                "wall",
            )
            .cut_material("trim")
            .collision(Collision::Bounds),
        )
        .build()?;
    let cornice = Part::builder("cornice")
        .element(Element::new(
            "band",
            Geometry::cuboid([8.4, 0.35, 6.4]),
            "trim",
        ))
        .build()?;
    Ok(Building::builder("example:house")
        .part(ground)
        .part(upper)
        .part(cornice)
        .instance(Instance::new("ground", "ground"))
        .instance(Instance::new("upper", "upper").placed(Pose::at([0.0, 3.0, 0.0])))
        .instance(Instance::new("cornice", "cornice").placed(Pose::at([-0.2, 6.5, -0.2])))
        .material("wall", "example:brick")
        .material("trim", "example:plaster")
        .build()?)
}

/// What the mesher produced and what dresses it, carried into the render layer
/// in one resource so that spawning is one system and not a pipeline.
#[derive(Resource)]
struct Shell {
    meshed: ashlar::MeshedBuilding,
    library: MaterialLibrary,
}

/// Where a `--screenshot` run writes its frame, and nothing where a person is
/// watching instead.
#[derive(Resource)]
struct Capture(Option<PathBuf>);

/// The whole spawn: every drawable piece, then a key light and a camera.
///
/// One call does the join. [`ashlar_bevy::drawables`] walks the meshed
/// building, uploads each distinct part, element and cut batch once, creates
/// one material per distinct binding, and hands back what to spawn with the
/// instance's pose already in it. Keying materials by the binding rather than
/// by the material key is the part a hand-written loop gets wrong.
fn spawn(
    mut commands: Commands,
    shell: Res<Shell>,
    server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) -> Result {
    let mut cx = ashlar_bevy::UploadContext {
        server: &server,
        meshes: &mut meshes,
        materials: &mut materials,
    };
    for drawable in ashlar_bevy::drawables(&shell.meshed, &shell.library, &mut cx)? {
        commands.spawn((
            Name::new(drawable.piece.label()),
            Mesh3d(drawable.mesh),
            MeshMaterial3d(drawable.material),
            drawable.transform,
        ));
    }
    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(8.0, 14.0, 10.0).looking_at(Vec3::new(4.0, 3.0, 3.0), Vec3::Y),
    ));
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(16.0, 9.0, 15.0).looking_at(Vec3::new(4.0, 3.2, 3.0), Vec3::Y),
    ));
    Ok(())
}

/// Capture one frame and leave, for a run with nobody in front of it.
///
/// The wait is frames rather than load states because a KTX2 set is several
/// megabytes and a capture taken before it arrives is a picture of the
/// definition's constants — which is exactly the failure this example exists to
/// make visible.
fn capture(
    mut commands: Commands,
    capture: Res<Capture>,
    mut frames: Local<u32>,
    mut exit: MessageWriter<AppExit>,
) {
    *frames += 1;
    let Some(path) = capture.0.clone() else {
        return;
    };
    if *frames != 150 {
        return;
    }
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path))
        .observe(
            |_: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>| {
                exit.write(AppExit::Success);
            },
        );
    // The observer above is what normally exits; this keeps a run that cannot
    // capture from hanging forever in CI.
    if *frames > 600 {
        exit.write(AppExit::Success);
    }
}

fn main() -> anyhow::Result<()> {
    let building = building()?;
    let library = library();
    // Before a window opens: a missing map fails here, by key, rather than as
    // a pink wall.
    ashlar_bevy::check_library(&library, std::path::Path::new(ASSETS), &building)?;
    let mut arguments = std::env::args().skip(1);
    let screenshot = arguments
        .find(|argument| argument == "--screenshot")
        .and(arguments.next())
        .map(PathBuf::from);
    App::new()
        .add_plugins(DefaultPlugins.set(AssetPlugin {
            file_path: ASSETS.to_owned(),
            ..default()
        }))
        .insert_resource(Capture(screenshot))
        .insert_resource(Shell {
            meshed: mesh_building(&building, &ManifoldMesher::default())?,
            library,
        })
        .add_systems(Startup, spawn)
        .add_systems(Update, capture)
        .run();
    Ok(())
}
