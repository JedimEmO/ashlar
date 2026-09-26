//! Windowless, fixed-studio gallery, sharing the interactive renderer.
//!
//! The output is for visual inspection. There is no pixel baseline check:
//! coincident-edge draw ordering and driver revisions move pixels for reasons
//! that have nothing to do with the geometry, so a red build there taught
//! nothing. Look at `index.html`.
use std::{
    collections::VecDeque,
    fmt::Write as _,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use ashlar::MeshedBuilding;
use ashlar_bevy as materials;
use ashlar_manifold::{ManifoldMesher, mesh_building};
use bevy::{
    app::ScheduleRunnerPlugin,
    camera::RenderTarget,
    ecs::system::RunSystemOnce,
    prelude::*,
    render::{
        RenderPlugin,
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::TextureFormat,
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
    time::TimeUpdateStrategy,
    window::ExitCondition,
    winit::WinitPlugin,
};

use crate::{
    Catalog, Options,
    viewer::{
        Center, GALLERY_FRAMING, Palette, Preview, PreviewObject, bounds, framing, setup,
        setup_materials, setup_strands,
    },
};

const WIDTH: u32 = 800;
const HEIGHT: u32 = 600;
const VIEWS: [(&str, f32, f32); 3] = [
    ("front", -0.5, 0.25),
    ("rear", std::f32::consts::PI - 0.5, 0.25),
    ("elevated", -0.65, 0.65),
];
const WARMUP: u32 = 12;

/// The views this run takes: the three fixed ones, or the single one
/// `--reference-view` named.
///
/// A capture of a *surface* rather than of a model wants an angle of its own —
/// a silhouette shows at a grazing angle and at nothing else — and the three
/// above are chosen to show a building. One list rather than a fourth entry,
/// because a run that asked for a grazing close-up does not also want the
/// building shots at that distance.
fn views(options: &Options) -> Result<Vec<(String, f32, f32)>> {
    match options.reference_view.as_slice() {
        [] => Ok(VIEWS
            .iter()
            .map(|(name, yaw, pitch)| ((*name).to_owned(), *yaw, *pitch))
            .collect()),
        [yaw, pitch] => Ok(vec![("custom".to_owned(), *yaw, *pitch)]),
        other => anyhow::bail!(
            "--reference-view takes a yaw and a pitch in radians, as `yaw,pitch`;              got {} values",
            other.len()
        ),
    }
}

struct Prepared {
    name: String,
    display: String,
    mesh: MeshedBuilding,
    definitions: crate::Definitions,
    graphs: crate::Graphs,
}

#[derive(Resource)]
struct Batch {
    queue: VecDeque<Prepared>,
    output: PathBuf,
    target: Handle<Image>,
    current: String,
    distance: f32,
    /// What `--reference-distance` asked for, or `None` to frame the model.
    override_distance: Option<f32>,
    views: Vec<(String, f32, f32)>,
    view: usize,
    ready_frames: u32,
    pending: bool,
    advance: bool,
    started: Instant,
}

pub(crate) fn run(catalog: &Catalog, options: &Options) -> Result<()> {
    let started = Instant::now();
    let output = options.references.as_ref().expect("reference mode");
    std::fs::create_dir_all(output).context("creating reference directory")?;
    let queue = prepare(catalog, options)?;
    let views = views(options)?;
    write_gallery(output, &queue, &views, options.reference_distance)?;
    let count = queue.len() * views.len();
    let rig = crate::viewer::Rig::new(
        options.night,
        options.reference_key,
        options.reference_ambient,
    );
    let mut app = App::new();
    app.insert_resource(ClearColor(rig.sky))
        .insert_resource(GlobalAmbientLight {
            brightness: rig.ambient,
            ..default()
        })
        .insert_resource(rig.key)
        .insert_resource(crate::viewer::Night(rig.night))
        // The gallery is a set of images that have to be the same images next
        // time, and grass that sways answers a different one every run however
        // many frames it waits. `StrandPlugin` brings a breeze; this takes it
        // away again, after the plugin has had its say.
        .insert_resource(materials::wind::StrandWind::still())
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .init_resource::<Palette>()
        .init_resource::<materials::runtime_bake::BakeCache>()
        .init_resource::<materials::cards::CardCache>()
        .add_plugins(
            DefaultPlugins
                .set(RenderPlugin {
                    synchronous_pipeline_compilation: true,
                    ..default()
                })
                .set(AssetPlugin {
                    file_path: options.asset_root.to_string_lossy().into_owned(),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: ExitCondition::DontExit,
                    ..default()
                })
                .set(bevy::app::TaskPoolPlugin {
                    task_pool_options: bevy::app::TaskPoolOptions::with_num_threads(4),
                })
                .disable::<WinitPlugin>()
                .disable::<bevy::gilrs::GilrsPlugin>()
                .disable::<PipelinedRenderingPlugin>(),
        )
        // After `DefaultPlugins`, because a material plugin registers an asset
        // and an asset needs the `AssetServer` that `AssetPlugin` inserts. The
        // gallery renders whatever a scene binds, and a scene may bind a
        // compiled surface.
        .add_plugins((
            materials::shader::ProceduralMaterialPlugin,
            materials::gpu::GpuBakePlugin,
            materials::wind::StrandPlugin,
        ))
        .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_millis(1)))
        .add_systems(Update, tick);
    let target = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new_target_texture(
            WIDTH,
            HEIGHT,
            TextureFormat::Rgba8UnormSrgb,
            None,
        ));
    app.insert_resource(Batch {
        queue,
        output: output.clone(),
        target,
        current: String::new(),
        distance: 0.0,
        override_distance: options.reference_distance,
        views,
        view: 0,
        ready_frames: 0,
        pending: false,
        advance: true,
        started,
    });
    ensure!(
        matches!(app.run(), AppExit::Success),
        "reference rendering failed"
    );
    println!(
        "Rendered {count} references in {:.2}s: {}",
        started.elapsed().as_secs_f64(),
        output.join("index.html").display()
    );
    Ok(())
}

fn prepare(catalog: &Catalog, options: &Options) -> Result<VecDeque<Prepared>> {
    let scenes = if options.reference_scenes.is_empty() {
        catalog.scenes().iter().collect::<Vec<_>>()
    } else {
        options
            .reference_scenes
            .iter()
            .map(|name| catalog.require(name))
            .collect::<Result<Vec<_>>>()?
    };
    ensure!(!scenes.is_empty(), "no scenes to render");
    let mut queue = VecDeque::new();
    for scene in scenes {
        let name = scene.name().to_owned();
        ensure!(
            !queue.iter().any(|p: &Prepared| p.name == name),
            "duplicate scene: {name}"
        );
        let building = scene.build()?;
        let (graphs, _) = scene.load_graphs(&options.asset_root)?;
        let mut definitions =
            crate::Definitions(scene.load_materials(&options.asset_root, &building, &graphs)?);
        if !options.night {
            crate::viewer::by_day(&mut definitions);
        }
        let mesh = mesh_building(&building, &ManifoldMesher::default())?;
        bounds(&mesh)?;
        queue.push_back(Prepared {
            name,
            display: scene.display().to_owned(),
            mesh,
            definitions,
            graphs: crate::Graphs(graphs),
        });
    }
    Ok(queue)
}

fn next_scene(world: &mut World) -> Result<bool> {
    let Some(prepared) = world.resource_mut::<Batch>().queue.pop_front() else {
        return Ok(false);
    };
    let entities = world
        .query_filtered::<Entity, With<PreviewObject>>()
        .iter(world)
        .collect::<Vec<_>>();
    for entity in entities {
        world.despawn(entity);
    }
    let (center, distance) = framing(&prepared.mesh, GALLERY_FRAMING)?;
    let mut batch = world.resource_mut::<Batch>();
    batch.current = prepared.name;
    batch.distance = batch.override_distance.unwrap_or(distance);
    batch.view = 0;
    batch.ready_frames = 0;
    batch.advance = false;
    world.insert_resource(center);
    world.insert_resource(Preview(prepared.mesh));
    world.insert_resource(prepared.definitions);
    world.insert_resource(prepared.graphs);
    // Each scene brings its own graph library, so a key baked or compiled for
    // the last one says nothing about this one. The images live until their
    // handles do; the shaders are registered under ids nothing holds a strong
    // handle to, so forgetting them is what frees them.
    world
        .resource_mut::<materials::runtime_bake::BakeCache>()
        .clear();
    world.resource_scope(
        |world, mut registry: Mut<materials::shader::GraphShaders>| {
            registry.clear(&mut world.resource_mut::<Assets<bevy::shader::Shader>>());
        },
    );
    world.resource_mut::<Palette>().shaders.clear();
    world
        .run_system_once::<_, (), _>(setup_materials)
        .map_err(|e| anyhow::anyhow!("preparing reference materials: {e:?}"))?;
    world
        .run_system_once(setup)
        .map_err(|e| anyhow::anyhow!("spawning reference scene: {e:?}"))?;
    world
        .run_system_once(crate::viewer::setup_night_lights)
        .map_err(|e| anyhow::anyhow!("lighting reference lamps: {e:?}"))?;
    // The strands last, as the interactive viewer runs them: they are spawned
    // off the same elements `setup` just spawned, and a capture of a lawn with
    // no blades in it is the picture this phase exists to replace.
    world
        .run_system_once::<_, (), _>(setup_strands)
        .map_err(|e| anyhow::anyhow!("growing reference strands: {e:?}"))?;
    aim(world);
    Ok(true)
}

fn aim(world: &mut World) {
    // The model stands on the origin rather than straddling it, so what the
    // camera frames is half its height up. See `viewer::Center`. Named for the
    // framing rather than for the camera, because `Batch::target` below is the
    // image being rendered into.
    let framed = world.resource::<Center>().target();
    let batch = world.resource::<Batch>();
    let (_, yaw, pitch) = batch
        .views
        .get(batch.view)
        .cloned()
        .unwrap_or_else(|| ("front".to_owned(), -0.5, 0.25));
    let rotation = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch);
    let transform = Transform::from_translation(framed + rotation * Vec3::NEG_Z * batch.distance)
        .looking_at(framed, Vec3::Y);
    let target = batch.target.clone();
    let cameras = world
        .query_filtered::<Entity, With<Camera3d>>()
        .iter(world)
        .collect::<Vec<_>>();
    for camera in cameras {
        world.entity_mut(camera).insert((
            transform,
            RenderTarget::Image(target.clone().into()),
            Msaa::Sample4,
            bevy::render::view::NoIndirectDrawing,
        ));
    }
}

fn tick(world: &mut World) {
    if let Err(error) = step(world) {
        eprintln!("Reference capture failed: {error:#}");
        world.write_message(AppExit::error());
    }
}

fn step(world: &mut World) -> Result<()> {
    ensure!(
        world.resource::<Batch>().started.elapsed() < Duration::from_secs(120),
        "reference batch timed out"
    );
    if world.resource::<Batch>().advance && !next_scene(world)? {
        world.write_message(AppExit::Success);
        return Ok(());
    }
    if world.resource::<Batch>().pending {
        return Ok(());
    }
    let server = world.resource::<AssetServer>();
    for material in world.resource::<Palette>().originals.values() {
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
                anyhow::bail!("texture failed: {error}");
            }
            if !server.is_loaded_with_dependencies(handle.id()) {
                return Ok(());
            }
        }
    }
    let mut batch = world.resource_mut::<Batch>();
    batch.ready_frames += 1;
    if batch.ready_frames < WARMUP {
        return Ok(());
    }
    batch.pending = true;
    let view = batch
        .views
        .get(batch.view)
        .map_or_else(|| "front".to_owned(), |(name, _, _)| name.clone());
    let path = batch.output.join(format!("{}-{view}.png", batch.current));
    let target = batch.target.clone();
    world.spawn(Screenshot::image(target)).observe(
        move |event: On<ScreenshotCaptured>,
              mut batch: ResMut<Batch>,
              mut exit: MessageWriter<AppExit>,
              mut commands: Commands| {
            let result = event
                .image
                .clone()
                .try_into_dynamic()
                .map_err(anyhow::Error::from)
                .and_then(|image| image.to_rgb8().save(&path).map_err(anyhow::Error::from));
            if let Err(error) = result {
                eprintln!("Saving {} failed: {error}", path.display());
                exit.write(AppExit::error());
                return;
            }
            println!("saved {}", path.display());
            batch.pending = false;
            batch.ready_frames = 0;
            batch.view += 1;
            if batch.view == batch.views.len() {
                batch.advance = true;
            } else {
                commands.queue(aim);
            }
        },
    );
    Ok(())
}

fn write_gallery(
    output: &std::path::Path,
    scenes: &VecDeque<Prepared>,
    views: &[(String, f32, f32)],
    override_distance: Option<f32>,
) -> Result<()> {
    let mut html = String::from(
        "<!doctype html><meta charset='utf-8'><title>Building references</title><style>body{background:#151a21;color:#ddd;font:16px system-ui;margin:32px}section{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:16px}img{width:100%}figure{margin:0}h2{margin-top:40px}a{color:inherit}</style><h1>Building references</h1><p>Fixed studio, 800×600, MSAA 4×. Front / rear / elevated. For visual inspection; there is no pixel baseline. Click any image for full resolution.</p>",
    );
    let mut manifest = String::from(
        "scene\tview\twidth\theight\tyaw\tpitch\tdistance\tinstances\tunique_triangles\n",
    );
    for scene in scenes {
        write!(html, "<h2>{}</h2><section>", scene.display)?;
        let distance = match override_distance {
            Some(metres) => metres,
            None => framing(&scene.mesh, GALLERY_FRAMING)?.1,
        };
        let triangles: usize = scene
            .mesh
            .parts
            .values()
            .flatten()
            .map(|p| p.mesh.positions.len() / 3)
            .sum();
        for (view, yaw, pitch) in views {
            let file = format!("{}-{view}.png", scene.name);
            write!(
                html,
                "<figure><a href='{file}'><img src='{file}'></a><figcaption>{view}</figcaption></figure>"
            )?;
            writeln!(
                manifest,
                "{}\t{view}\t{WIDTH}\t{HEIGHT}\t{yaw}\t{pitch}\t{distance}\t{}\t{triangles}",
                scene.name,
                scene.mesh.building.recipe().instances.len()
            )?;
        }
        html.push_str("</section>");
    }
    std::fs::write(output.join("index.html"), html)?;
    std::fs::write(output.join("manifest.tsv"), manifest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything below the screenshot is ordinary code: the queue, the two
    /// writers and the framing. Only the capture itself needs a GPU, and
    /// nothing here starts one.
    fn options(arguments: &[&str]) -> Options {
        Options::try_parse_for(&crate::tests::catalog(), arguments).expect("options")
    }

    #[test]
    fn preparing_the_queue_meshes_every_requested_scene_in_the_order_asked_for() {
        let catalog = crate::tests::catalog();
        let queue = prepare(
            &catalog,
            &options(&[
                "preview",
                "--references",
                "out",
                "--reference-scenes",
                "two,one",
            ]),
        )
        .expect("both scenes mesh");
        assert_eq!(
            queue.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["two", "one"]
        );
        assert_eq!(queue[0].display, "Two");
        for prepared in &queue {
            assert_eq!(prepared.mesh.parts.len(), 1, "the part was evaluated");
            assert!(prepared.definitions.0.is_none(), "no library, no preflight");
        }

        // Without a selection the gallery renders the whole catalog.
        let all = prepare(&catalog, &options(&["preview", "--references", "out"]))
            .expect("the whole catalog meshes");
        assert_eq!(all.len(), catalog.scenes().len());
    }

    #[test]
    fn an_unknown_reference_scene_reports_the_ones_that_exist() {
        // The command line rejects this too, but a downstream binary can build
        // its own Options, so the queue has to say it as well.
        let mut options = options(&["preview", "--references", "out"]);
        options.reference_scenes = vec!["three".into()];
        let Err(error) = prepare(&crate::tests::catalog(), &options) else {
            panic!("an unknown scene must not silently render nothing");
        };
        let error = error.to_string();
        assert!(error.contains("three"), "{error}");
        assert!(error.contains("one, two"), "{error}");
    }

    #[test]
    fn the_gallery_writes_an_index_and_one_manifest_row_per_view() {
        let output = tempfile::TempDir::new().expect("temp dir");
        let queue = prepare(
            &crate::tests::catalog(),
            &options(&[
                "preview",
                "--references",
                "out",
                "--reference-scenes",
                "one",
            ]),
        )
        .expect("scene meshes");
        write_gallery(
            output.path(),
            &queue,
            &views(&options(&[])).expect("views"),
            None,
        )
        .expect("gallery");

        let html = std::fs::read_to_string(output.path().join("index.html")).expect("index");
        for (view, ..) in VIEWS {
            let file = format!("one-{view}.png");
            assert!(html.contains(&format!("<img src=\'{file}\'>")), "{html}");
        }

        let manifest = std::fs::read_to_string(output.path().join("manifest.tsv")).expect("tsv");
        let mut lines = manifest.lines();
        let columns = lines.next().expect("header").split('\t').count();
        let rows = lines.collect::<Vec<_>>();
        assert_eq!(rows.len(), VIEWS.len(), "one row per view of the one scene");

        let front: Vec<&str> = rows[0].split('\t').collect();
        assert_eq!(front.len(), columns);
        assert_eq!(&front[..4], ["one", "front", "800", "600"]);
        assert_eq!(front[7], "1", "one instance");
        assert!(
            front[6].parse::<f32>().expect("distance") > 1.0,
            "the camera stands off the building"
        );
    }

    #[test]
    fn reference_cli_is_separate_from_interactive_and_export_modes() {
        let catalog = crate::tests::catalog();
        let options = Options::try_parse_for(
            &catalog,
            [
                "preview",
                "--references",
                "out",
                "--reference-scenes",
                "one,two",
            ],
        )
        .expect("reference selection");
        assert_eq!(options.reference_scenes.len(), 2);
        assert!(
            Options::try_parse_for(&catalog, ["preview", "--reference-scenes", "one"]).is_err()
        );
        assert!(
            Options::try_parse_for(
                &catalog,
                ["preview", "--references", "out", "--screenshot", "shot.png"]
            )
            .is_err()
        );
        assert!(
            Options::try_parse_for(&catalog, ["preview", "--scene", "absent"]).is_err(),
            "scene names are validated against the catalog"
        );
    }
}
