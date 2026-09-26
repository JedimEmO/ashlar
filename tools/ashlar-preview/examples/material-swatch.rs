//! Fixed studio capture of a library material's exported PBR maps.
//!
//! ```sh
//! cargo run --release -p ashlar-preview --example material-swatch -- \
//!     library:<name> <out.png> [sphere|plane] [asset-root] [ashlar|opengl]
//! ```
//!
//! It reads the level-0 PNGs `export-materials <asset-root> <res> <name> png`
//! writes, and is the studio rig the reference comparisons in
//! `docs/material-references` are captured with.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "bounded mesh and image coordinates"
)]
use anyhow::{Context, Result, ensure};
use bevy::{
    app::ScheduleRunnerPlugin,
    asset::RenderAssetUsages,
    camera::RenderTarget,
    image::{ImageAddressMode, ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor},
    light::GeneratedEnvironmentMapLight,
    mesh::VertexAttributeValues,
    prelude::*,
    render::{
        RenderPlugin,
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::{
            Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
        },
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
    window::ExitCondition,
    winit::WinitPlugin,
};
use std::{
    num::NonZeroUsize,
    path::PathBuf,
    time::{Duration, Instant},
};

/// How many threads the swatch's strand scatter divides its cell rows across.
///
/// Four, which is what the preview uses and for the reason it gives: this
/// machine has been seen to fall over under full multi-core load, and a capture
/// already has a renderer compiling shaders beside it.
const STRAND_THREADS: Option<NonZeroUsize> = NonZeroUsize::new(4);

#[derive(Resource)]
struct Capture {
    path: PathBuf,
    target: Handle<Image>,
    maps: Vec<Handle<Image>>,
    frames: u32,
    pending: bool,
    started: Instant,
}

fn map_directory(preset: &str) -> String {
    let name = preset.strip_prefix("library:").unwrap_or(preset);
    format!("materials/library/{name}")
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let preset = args.next().context("provide a library material key")?;
    ensure!(
        ashlar_material::stdlib::materials()
            .materials
            .contains_key(&preset),
        "unknown library material {preset}"
    );
    let output = PathBuf::from(args.next().context("provide an output PNG")?);
    let plane = args.next().unwrap_or_else(|| "sphere".into());
    ensure!(
        plane == "sphere" || plane == "plane",
        "shape must be sphere or plane"
    );
    let root = PathBuf::from(args.next().unwrap_or_else(|| "assets".into())).canonicalize()?;
    let normal_format = args.next().unwrap_or_else(|| "ashlar".into());
    ensure!(
        matches!(normal_format.as_str(), "ashlar" | "opengl"),
        "normal format must be ashlar or opengl"
    );
    ensure!(args.next().is_none(), "too many arguments");
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let no_height = ashlar_material::stdlib::graphs()
        .get(&preset)
        .is_some_and(|g| g.output.height.is_none());
    let heights = if no_height {
        image::ImageBuffer::from_pixel(16, 16, image::Luma([32768_u16]))
    } else {
        image::open(root.join(format!("{}/height.png", map_directory(&preset))))?.into_luma16()
    };
    // The rig displays a two metre repeat; preserve the authored slopes.
    let height_scale = ashlar_material::stdlib::graphs()
        .get(&preset)
        .context("library graph")?
        .output
        .normal_strength
        * 2.0;
    let is_plane = plane == "plane";
    let mesh = displaced_mesh(&heights, height_scale, is_plane)?;
    // The strands stand on the *displaced* mesh, so a root sits on the surface
    // the height map drew rather than half a pile under it. They are grown here
    // rather than in `populate` because a scatter can fail and `main` is where
    // a failure is still a message rather than a panic in a system.
    let strands = strand_mesh(&preset, &mesh, is_plane)?;
    let mut app = App::new();
    app.insert_resource(ClearColor(Color::WHITE))
        .insert_resource(GlobalAmbientLight {
            brightness: 150.0,
            ..default()
        })
        .add_plugins(
            DefaultPlugins
                .set(RenderPlugin {
                    synchronous_pipeline_compilation: true,
                    ..default()
                })
                .set(AssetPlugin {
                    file_path: root.to_string_lossy().into_owned(),
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
                .disable::<bevy::audio::AudioPlugin>()
                .disable::<PipelinedRenderingPlugin>(),
        )
        .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_millis(1)))
        .add_systems(Update, capture);
    app.finish();
    app.cleanup();
    populate(
        &mut app,
        &preset,
        mesh,
        strands,
        is_plane,
        output,
        normal_format == "ashlar",
    );
    ensure!(matches!(app.run(), AppExit::Success), "capture failed");
    Ok(())
}

// An authored six-face studio environment: broad softboxes and a dim neutral
// room. Bevy convolves it into diffuse irradiance and GGX specular mip levels.
fn studio() -> Image {
    const SIDE: u32 = 128;
    let mut data = Vec::with_capacity((SIDE * SIDE * 6 * 4) as usize);
    for face in 0..6 {
        for y in 0..SIDE {
            for x in 0..SIDE {
                let bright = (face == 0 && (22..75).contains(&x) && (12..110).contains(&y))
                    || (face == 4 && (10..46).contains(&x) && (10..110).contains(&y))
                    || (face == 2 && (10..110).contains(&x) && (20..100).contains(&y));
                let color = if bright {
                    [240, 235, 224, 255]
                } else if face == 2 {
                    [100, 110, 125, 255]
                } else {
                    [65, 68, 74, 255]
                };
                data.extend_from_slice(&color);
            }
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: SIDE,
            height: SIDE,
            depth_or_array_layers: 6,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    image
}
fn capture(
    mut commands: Commands,
    mut state: ResMut<Capture>,
    server: Res<AssetServer>,
    mut exit: MessageWriter<AppExit>,
) {
    if state.started.elapsed() > Duration::from_secs(90) {
        eprintln!("capture timed out");
        exit.write(AppExit::error());
        return;
    }
    if state.pending {
        return;
    }
    for handle in &state.maps {
        if let Some(bevy::asset::LoadState::Failed(error)) = server.get_load_state(handle.id()) {
            eprintln!("texture failed: {error}");
            exit.write(AppExit::error());
            return;
        }
        if !server.is_loaded_with_dependencies(handle.id()) {
            return;
        }
    }
    state.frames += 1;
    if state.frames < 90 {
        return;
    }
    state.pending = true;
    let path = state.path.clone();
    commands
        .spawn(Screenshot::image(state.target.clone()))
        .observe(
            move |event: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>| match event
                .image
                .clone()
                .try_into_dynamic()
                .map_err(anyhow::Error::from)
                .and_then(|image| image.to_rgb8().save(&path).map_err(anyhow::Error::from))
            {
                Ok(()) => {
                    println!("saved {}", path.display());
                    exit.write(AppExit::Success);
                }
                Err(error) => {
                    eprintln!("{error}");
                    exit.write(AppExit::error());
                }
            },
        );
}

fn displaced_mesh(
    heights: &image::ImageBuffer<image::Luma<u16>, Vec<u16>>,
    height_scale: f32,
    is_plane: bool,
) -> Result<Mesh> {
    let mut mesh = if is_plane {
        Plane3d::default()
            .mesh()
            .size(2.0, 2.0)
            .subdivisions(512)
            .build()
    } else {
        Sphere::new(2.0 / std::f32::consts::PI).mesh().uv(512, 256)
    };
    if !is_plane
        && let Some(VertexAttributeValues::Float32x2(uvs)) =
            mesh.attribute_mut(Mesh::ATTRIBUTE_UV_0)
    {
        for uv in uvs {
            uv[0] *= 2.0;
        }
    }
    mesh.generate_tangents()
        .map_err(|e| anyhow::anyhow!("tangents: {e}"))?;
    let Some(VertexAttributeValues::Float32x2(uvs)) = mesh.attribute(Mesh::ATTRIBUTE_UV_0) else {
        anyhow::bail!("mesh UVs");
    };
    let offsets = uvs
        .iter()
        .map(|uv| {
            let x = uv[0].rem_euclid(1.0) * heights.width() as f32 - 0.5;
            let y = uv[1].rem_euclid(1.0) * heights.height() as f32 - 0.5;
            let sample = |dx: i32, dy: i32| {
                f32::from(
                    heights.get_pixel(
                        (x.floor() as i32 + dx).rem_euclid(heights.width().cast_signed()) as u32,
                        (y.floor() as i32 + dy).rem_euclid(heights.height().cast_signed()) as u32,
                    )[0],
                ) / 65535.0
            };
            let tx = x - x.floor();
            let ty = y - y.floor();
            let a = sample(0, 0) * (1.0 - tx) + sample(1, 0) * tx;
            let b = sample(0, 1) * (1.0 - tx) + sample(1, 1) * tx;
            ((a * (1.0 - ty) + b * ty) - 0.5) * height_scale
        })
        .collect::<Vec<_>>();
    if let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION)
    {
        for (position, h) in positions.iter_mut().zip(offsets) {
            let p = Vec3::from_array(*position);
            *position = (p + if is_plane {
                Vec3::Y * h
            } else {
                p.normalize() * h
            })
            .to_array();
        }
    }
    Ok(mesh)
}

/// The geometry of every strand layer the preset's graph declares, on the
/// surface it was just displaced into.
///
/// `None` where the graph declares none, which is every preset but the grass.
/// The UVs of both shapes are already in *repeats* of the material — the sphere
/// is two metres round per repeat because its circumference is four and its `u`
/// runs twice, and the plane is two metres square — so the conversion
/// `ashlar-bevy` makes for a real material definition has nothing to do here.
///
/// At full density rather than the benchmark sheet's third: this is the hero
/// shot of the material, and what it is for is showing what the layer actually
/// is.
fn strand_mesh(preset: &str, surface: &Mesh, is_plane: bool) -> Result<Option<Mesh>> {
    use ashlar_material::strands::{StrandRequest, SurfaceTriangles, mesh, place, scatter};

    let library = ashlar_material::stdlib::graphs();
    let graph = library.get(preset).context("library graph")?;
    if graph.strands.is_empty() {
        return Ok(None);
    }
    let positions = floats3(surface, Mesh::ATTRIBUTE_POSITION)?;
    let normals = floats3(surface, Mesh::ATTRIBUTE_NORMAL)?;
    let Some(VertexAttributeValues::Float32x2(uvs)) = surface.attribute(Mesh::ATTRIBUTE_UV_0)
    else {
        anyhow::bail!("surface UVs");
    };
    // A plane's own UV runs `0..1` over two metres, which is one repeat; the
    // sphere's `u` was already doubled where it was built.
    let uvs: Vec<[f32; 2]> = uvs.clone();
    let indices: Vec<u32> = match surface.indices() {
        Some(bevy::mesh::Indices::U32(indices)) => indices.clone(),
        Some(bevy::mesh::Indices::U16(indices)) => {
            indices.iter().map(|index| u32::from(*index)).collect()
        }
        None => anyhow::bail!("surface indices"),
    };
    let triangles = SurfaceTriangles {
        positions: &positions,
        normals: &normals,
        uvs: &uvs,
        indices: &indices,
    };
    let mut built = ashlar_material::strands::StrandMesh::default();
    for layer in graph.strands.keys() {
        let set = scatter(&StrandRequest {
            graph,
            library: &library,
            params: &std::collections::BTreeMap::new(),
            layer,
            field_resolution: ashlar_bevy::strands::STRAND_FIELDS,
            threads: STRAND_THREADS,
        })
        .with_context(|| format!("scattering {layer:?}"))?;
        let placed = place(&set, &triangles, 1.0);
        append(&mut built, mesh(&placed, set.shape()));
    }
    eprintln!(
        "{preset}: {} strand triangles on the {}",
        built.triangles(),
        if is_plane { "plane" } else { "sphere" }
    );
    Ok(Some(
        Mesh::new(
            bevy::mesh::PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, built.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, built.normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, built.uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, built.colors)
        .with_inserted_indices(bevy::mesh::Indices::U32(built.indices)),
    ))
}

/// One three-lane attribute of a mesh, copied out.
fn floats3(mesh: &Mesh, attribute: bevy::mesh::MeshVertexAttribute) -> Result<Vec<[f32; 3]>> {
    let name = attribute.name;
    let Some(VertexAttributeValues::Float32x3(values)) = mesh.attribute(attribute) else {
        anyhow::bail!("surface {name}");
    };
    Ok(values.clone())
}

/// One layer's geometry onto the end of another's, indices rebased.
fn append(
    into: &mut ashlar_material::strands::StrandMesh,
    from: ashlar_material::strands::StrandMesh,
) {
    let base = u32::try_from(into.positions.len()).unwrap_or(0);
    into.positions.extend(from.positions);
    into.normals.extend(from.normals);
    into.uvs.extend(from.uvs);
    into.colors.extend(from.colors);
    into.wind.extend(from.wind);
    into.indices
        .extend(from.indices.into_iter().map(|index| index + base));
}

#[expect(
    clippy::too_many_lines,
    reason = "one studio rig, spawned in the order it is read: the mesh, the \
              strands standing on it, the environment, the camera and the lights"
)]
fn populate(
    app: &mut App,
    preset: &str,
    mesh: Mesh,
    strands: Option<Mesh>,
    is_plane: bool,
    output: PathBuf,
    flip_normal_y: bool,
) {
    let world = app.world_mut();
    let server = world.resource::<AssetServer>();
    // The level-0 PNGs `export-materials ... png` writes beside its KTX2.
    let extension = "png";
    let maps = ["base", "normal", "orm"].map(|map| {
        server
            .load_builder()
            .with_settings(move |s: &mut ImageLoaderSettings| {
                s.is_srgb = map == "base";
                s.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                    address_mode_u: ImageAddressMode::Repeat,
                    address_mode_v: ImageAddressMode::Repeat,
                    ..ImageSamplerDescriptor::linear()
                });
            })
            .load(format!("{}/{map}.{extension}", map_directory(preset)))
    });
    let definition = ashlar_material::stdlib::materials()
        .materials
        .remove(preset);
    let emissive = definition.as_ref().map_or([0.0; 3], |d| d.emissive);
    let emission_map: Option<Handle<Image>> = emissive
        .iter()
        .any(|c| *c > 0.0)
        .then(|| server.load(format!("{}/emissive.png", map_directory(preset))));
    let mut loaded_maps = maps.to_vec();
    loaded_maps.extend(emission_map.iter().cloned());
    let material = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            emissive: LinearRgba::rgb(emissive[0], emissive[1], emissive[2]),
            emissive_texture: emission_map,
            base_color_texture: Some(maps[0].clone()),
            normal_map_texture: Some(maps[1].clone()),
            flip_normal_map_y: flip_normal_y,
            metallic_roughness_texture: Some(maps[2].clone()),
            occlusion_texture: Some(maps[2].clone()),
            metallic: 1.0,
            perceptual_roughness: 1.0,
            ..default()
        });
    let mesh = world.resource_mut::<Assets<Mesh>>().add(mesh);
    let pose = if is_plane {
        Transform::IDENTITY
    } else {
        Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
    };
    world.spawn((Mesh3d(mesh), MeshMaterial3d(material), pose));
    if let Some(strands) = strands {
        // The same material `ashlar-bevy` draws a strand layer through: white,
        // because the colour is the mesh's own root-to-tip gradient, and culled
        // on neither side, because a blade seen from behind is a blade.
        let material = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                base_color: Color::WHITE,
                perceptual_roughness: 0.9,
                metallic: 0.0,
                cull_mode: None,
                double_sided: true,
                ..default()
            });
        let strands = world.resource_mut::<Assets<Mesh>>().add(strands);
        world.spawn((Mesh3d(strands), MeshMaterial3d(material), pose));
    }
    let environment = world.resource_mut::<Assets<Image>>().add(studio());
    let target = world
        .resource_mut::<Assets<Image>>()
        .add(Image::new_target_texture(
            1024,
            1024,
            TextureFormat::Rgba8UnormSrgb,
            None,
        ));
    let camera = if is_plane {
        Vec3::new(1.45, 1.9, 2.0)
    } else {
        Vec3::new(0.0, 0.04, 3.0)
    };
    world.spawn((
        Camera3d::default(),
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: bevy::camera::ScalingMode::FixedVertical {
                viewport_height: if is_plane { 3.1 } else { 1.38 },
            },
            ..OrthographicProjection::default_3d()
        }),
        Camera {
            clear_color: ClearColorConfig::Default,
            ..default()
        },
        RenderTarget::Image(target.clone().into()),
        Transform::from_translation(camera).looking_at(Vec3::ZERO, Vec3::Y),
        GeneratedEnvironmentMapLight {
            environment_map: environment,
            intensity: 3500.0,
            ..default()
        },
        bevy::core_pipeline::tonemapping::Tonemapping::AcesFitted,
        Msaa::Sample4,
    ));
    world.spawn((
        DirectionalLight {
            illuminance: 6500.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(-2.0, 3.0, 2.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    // Gentle camera-right fill reveals the lower hemisphere without flattening relief.
    world.spawn((
        DirectionalLight {
            illuminance: 1800.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(3.0, 0.5, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    world.insert_resource(Capture {
        path: output,
        target,
        maps: loaded_maps,
        frames: 0,
        pending: false,
        started: Instant::now(),
    });
}
