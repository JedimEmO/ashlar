//! Material preflight and PBR creation, without a GPU.
//!
//! `read_library` is pure IO and validation. `create_material` needs an `AssetServer`, which is
//! not a render resource: `MinimalPlugins` plus `AssetPlugin` supplies one, and
//! registering Bevy's own image loader by hand is what `TexturePlugin::finish`
//! would otherwise have done behind a render device. That is enough to decode a
//! real PNG and read back the colour space and the sampler `create_material` asked for.
#![allow(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "exact analytic fixtures; a failure is a test failure"
)]
use std::{num::NonZeroUsize, path::Path};

use ashlar::{
    Binding, Building, Element, Geometry, Instance, MaterialDefinition, MaterialLibrary,
    ParamValue, Part, Surface,
};
use ashlar_bevy::runtime_bake::{BakeCache, BakeContext, BakeKey, Baker, GraphImages};
use ashlar_material::{
    MaterialGraph, MaterialGraphLibrary, PbrOutput,
    bake::{BakeRequest, Encoded, PlaneFormat, TextureSet, bake},
    ktx2,
    mips::levels,
    nodes::{Math, Noise},
};
use bevy::{
    MinimalPlugins,
    asset::{AssetPlugin, LoadState},
    image::{
        CompressedImageFormats, Image, ImageAddressMode, ImageFilterMode, ImageLoader,
        ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor,
    },
    prelude::*,
    render::render_resource::TextureFormat,
};
use tempfile::TempDir;

/// The graph library a preflight sees when nothing names a graph.
fn no_graphs() -> MaterialGraphLibrary {
    MaterialGraphLibrary::default()
}

/// A one-node library standing in for the showcase's: enough to lower, and
/// with a parameter so a wrong override has something to be wrong about.
fn graphs() -> MaterialGraphLibrary {
    let mut library = MaterialGraphLibrary::default();
    library.insert(
        MaterialGraph::builder("study:concrete")
            .param(ashlar_material::Param::float("wear", 0.25).range(0.0, 1.0))
            .node("grain", Noise::value().period(16))
            .node(
                "worn",
                Math::new(
                    ashlar_material::MathOp::Mul,
                    "grain",
                    ashlar_material::Input::param("wear"),
                ),
            )
            .output(
                PbrOutput::new()
                    .base_color("worn")
                    .roughness("grain")
                    .height("grain")
                    .normal_strength(0.01),
            )
            .into_graph(),
    );
    library
}

fn building() -> Building {
    let part = Part::builder("study:wall")
        .element(Element::new(
            "panel",
            Geometry::cuboid([2.0, 3.0, 0.2]),
            "outer",
        ))
        .build()
        .expect("part");
    Building::builder("study:screen")
        .part(part)
        .material("outer", "paint")
        .instance(Instance::new("only", "study:wall"))
        .build()
        .expect("building")
}

/// The same building with the one slot bound through a binding of its own,
/// which is how an instance puts its own parameter values on a shared key.
fn bound_building(binding: Binding) -> Building {
    let part = Part::builder("study:wall")
        .element(Element::new(
            "panel",
            Geometry::cuboid([2.0, 3.0, 0.2]),
            "outer",
        ))
        .build()
        .expect("part");
    Building::builder("study:screen")
        .part(part)
        .material("outer", "paint")
        .instance(Instance::new("only", "study:wall").binding("outer", binding))
        .build()
        .expect("building")
}

/// A real baked texture set, with its mip chain, at the smallest resolution a
/// bake allows.
///
/// Small but not synthetic: these are the bytes and the formats a content step
/// writes, so what the loader below reads is what a game would load. The graph
/// is two noises and a height, which is enough to bind every map the set
/// carries.
fn baked_set() -> TextureSet {
    let graph = MaterialGraph::builder("test:maps")
        .node("grain", Noise::value().period(16))
        .node("bumps", Noise::value().period(8))
        .output(
            PbrOutput::new()
                .base_color("grain")
                .roughness("grain")
                .height("bumps")
                .normal_strength(0.02),
        )
        .into_graph();
    bake(&BakeRequest {
        graph: &graph,
        library: &MaterialGraphLibrary::default(),
        params: &std::collections::BTreeMap::new(),
        resolution: BAKED,
        mips: true,
        // Named rather than one per core: a test binary runs its cases in
        // parallel already.
        threads: std::num::NonZeroUsize::new(2),
    })
    .expect("a two-noise graph bakes")
}

/// The resolution the KTX2 fixtures are baked at: the smallest a bake allows,
/// and nine levels of chain.
const BAKED: u32 = 256;

/// Named rather than one per core, everywhere a test bakes: a test binary runs
/// its cases in parallel already, and the bytes do not depend on the division
/// of the rows.
const THREADS: Option<NonZeroUsize> = NonZeroUsize::new(3);

/// The `Bake` both paths are compared on: the `graphs()` fixture, at the
/// fixture resolution, with a parameter a case can vary.
fn study_bake(wear: f32) -> ashlar::Bake {
    ashlar::Bake {
        graph: "study:concrete".into(),
        params: [("wear".to_owned(), ashlar::ParamValue::Float(wear))]
            .into_iter()
            .collect(),
        resolution: BAKED,
    }
}

/// That same bake run the way a content step runs it, mips and all: the bytes
/// that would have gone into the KTX2 files.
fn study_set(wear: f32) -> TextureSet {
    let library = graphs();
    bake(&BakeRequest {
        graph: library.get("study:concrete").expect("the fixture graph"),
        library: &library,
        params: &study_bake(wear).params,
        resolution: BAKED,
        mips: true,
        threads: THREADS,
    })
    .expect("the fixture graph bakes")
}

/// `create_graph_material` over a temporary context, which is what a system holding four
/// resources does in one line.
fn create_graph_material(
    app: &mut App,
    definition: &MaterialDefinition,
    graphs: &MaterialGraphLibrary,
    cache: &mut BakeCache,
) -> StandardMaterial {
    app.world_mut()
        .resource_scope(|world, mut images: Mut<Assets<Image>>| {
            ashlar_bevy::runtime_bake::create_graph_material(
                definition,
                world.resource::<AssetServer>(),
                &mut BakeContext {
                    graphs,
                    cache,
                    images: &mut images,
                    threads: THREADS,
                    baker: Baker::Cpu,
                },
            )
            .expect("a preflighted bake")
        })
}

/// A definition whose surface is one bake, with constants a case can tell apart.
fn baked_definition(wear: f32, roughness: f32) -> MaterialDefinition {
    MaterialDefinition {
        roughness,
        surface: Surface::Graph(study_bake(wear)),
        ..MaterialDefinition::default()
    }
}

/// Write one encoded map as a KTX2 file under the asset root.
fn write_ktx2(root: &Path, key: &str, encoded: &Encoded, resolution: u32) {
    let path = root.join(key);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture directory");
    }
    std::fs::write(
        &path,
        ktx2::write(encoded, resolution).expect("a KTX2 file"),
    )
    .expect("fixture KTX2");
}

/// The same map again, with its levels compressed under scheme 2.
fn write_ktx2_zstd(root: &Path, key: &str, encoded: &Encoded, resolution: u32) {
    let path = root.join(key);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture directory");
    }
    std::fs::write(
        &path,
        ktx2::write_with(encoded, resolution, ktx2::Supercompression::Zstd)
            .expect("a supercompressed KTX2 file"),
    )
    .expect("fixture KTX2");
}

/// A two-pixel PNG is a real decodable file and costs nothing to write.
fn write_png(root: &Path, key: &str) {
    let path = root.join(key);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture directory");
    }
    image::RgbaImage::from_pixel(2, 2, image::Rgba([32, 64, 128, 255]))
        .save(&path)
        .expect("fixture PNG");
}

/// Write a one-material library whose surface is `surface`, and return its path.
fn write_library(root: &Path, surface: &str) -> std::path::PathBuf {
    let path = root.join("materials.ron");
    std::fs::write(
        &path,
        format!("(materials: {{\"paint\": (surface: {surface})}})"),
    )
    .expect("fixture library");
    path
}

/// A `Files` surface in RON, naming only the maps a case cares about.
fn surface(maps: &str) -> String {
    format!("Files({maps})")
}

/// A `Files` surface in Rust, in the order base colour, normal, ORM,
/// height, emissive. The variant carries its maps inline, so there is no
/// payload struct to spread a `Default` over.
fn textures(keys: [Option<&str>; 5]) -> Surface {
    let [base_color, normal, orm, height, emissive] = keys.map(|k| k.map(str::to_owned));
    Surface::Files {
        base_color,
        normal,
        orm,
        height,
        emissive,
        baked_from: None,
    }
}

fn read_error(root: &Path, surface: &str) -> String {
    let path = write_library(root, surface);
    format!(
        "{:#}",
        ashlar_bevy::read_library_with_graphs(&path, root, &building(), &no_graphs())
            .expect_err("preflight rejects this library")
    )
}

#[test]
fn preflight_accepts_a_library_whose_textures_all_decode() {
    let root = TempDir::new().expect("temp dir");
    for key in [
        "color.png",
        "normal.png",
        "orm.png",
        "height.png",
        "emissive.png",
    ] {
        write_png(root.path(), key);
    }
    let path = root.path().join("materials.ron");
    std::fs::write(
        &path,
        "(materials: {\"paint\": (tile_metres: (1.5, 2.5), surface: Files(\
             base_color: Some(\"color.png\"), normal: Some(\"normal.png\"), \
             orm: Some(\"orm.png\"), height: Some(\"height.png\"), \
             emissive: Some(\"emissive.png\")))})",
    )
    .expect("fixture library");
    let library =
        ashlar_bevy::read_library_with_graphs(&path, root.path(), &building(), &no_graphs())
            .expect("preflight");
    assert_eq!(library.materials["paint"].tile_metres, [1.5, 2.5]);
    assert_eq!(library.materials["paint"].texture_keys().count(), 5);
}

#[test]
fn preflight_refuses_a_texture_key_that_leaves_the_asset_root() {
    let root = TempDir::new().expect("temp dir");
    write_png(root.path(), "color.png");
    for key in ["/etc/hosts.png", "../outside.png", "sub/../../outside.png"] {
        let error = read_error(root.path(), &surface(&format!("base_color: Some({key:?})")));
        assert!(error.contains(key), "{error}");
        assert!(error.contains("relative to the asset root"), "{error}");
    }
}

#[test]
fn preflight_refuses_one_image_used_as_both_colour_and_data() {
    let root = TempDir::new().expect("temp dir");
    write_png(root.path(), "shared.png");
    let error = read_error(
        root.path(),
        &surface("base_color: Some(\"shared.png\"), normal: Some(\"shared.png\")"),
    );
    assert!(error.contains("shared.png"), "{error}");
    assert!(error.contains("sRGB color and linear data"), "{error}");
}

#[test]
fn preflight_names_the_key_of_a_texture_it_cannot_open_or_decode() {
    let root = TempDir::new().expect("temp dir");
    let error = read_error(root.path(), &surface("base_color: Some(\"absent.png\")"));
    assert!(error.contains("absent.png"), "{error}");
    assert!(error.contains("opening texture"), "{error}");

    std::fs::write(root.path().join("corrupt.png"), b"not a png").expect("fixture");
    let error = read_error(root.path(), &surface("normal: Some(\"corrupt.png\")"));
    assert!(error.contains("corrupt.png"), "{error}");
    assert!(error.contains("decoding texture"), "{error}");

    // The two maps the enum added are preflighted like the other three.
    for map in ["height", "emissive"] {
        let error = read_error(
            root.path(),
            &surface(&format!("{map}: Some(\"absent.png\")")),
        );
        assert!(error.contains("opening texture absent.png"), "{error}");
    }
}

#[test]
fn a_material_maps_its_tile_size_and_offset_into_the_uv_transform() {
    let root = TempDir::new().expect("temp dir");
    let mut app = headless(root.path());
    let definition = MaterialDefinition {
        base_color: [0.25, 0.5, 0.75],
        roughness: 0.4,
        metallic: 0.6,
        emissive: [0.0, 2.0, 0.0],
        tile_metres: [2.0, 4.0],
        uv_offset: [0.125, 0.25],
        ..MaterialDefinition::default()
    };
    let material = ashlar_bevy::create_material(&definition, app.world().resource::<AssetServer>());

    // UVs arrive in metres, so the scale is the reciprocal of the repeat size.
    assert_eq!(material.uv_transform.matrix2.x_axis, Vec2::new(0.5, 0.0));
    assert_eq!(material.uv_transform.matrix2.y_axis, Vec2::new(0.0, 0.25));
    assert_eq!(material.uv_transform.translation, Vec2::new(0.125, 0.25));
    assert_eq!(material.perceptual_roughness, 0.4);
    assert_eq!(material.metallic, 0.6);
    assert_eq!(material.emissive, LinearRgba::rgb(0.0, 2.0, 0.0));
    assert_eq!(material.base_color, Color::srgb(0.25, 0.5, 0.75));
    assert!(material.base_color_texture.is_none(), "no key, no handle");
    app.update();
}

#[test]
fn colour_maps_load_as_srgb_and_data_maps_as_linear_with_repeat_sampling() {
    let root = TempDir::new().expect("temp dir");
    for key in [
        "color.png",
        "normal.png",
        "orm.png",
        "height.png",
        "emissive.png",
    ] {
        write_png(root.path(), key);
    }
    let mut app = headless(root.path());
    let material = ashlar_bevy::create_material(
        &MaterialDefinition {
            surface: textures([
                Some("color.png"),
                Some("normal.png"),
                Some("orm.png"),
                Some("height.png"),
                Some("emissive.png"),
            ]),
            ..MaterialDefinition::default()
        },
        app.world().resource::<AssetServer>(),
    );
    let colour = material.base_color_texture.clone().expect("colour handle");
    let normal = material.normal_map_texture.clone().expect("normal handle");
    let orm = material
        .metallic_roughness_texture
        .clone()
        .expect("orm handle");
    let emissive = material.emissive_texture.clone().expect("emissive handle");
    assert_eq!(
        material.occlusion_texture.as_ref(),
        Some(&orm),
        "one ORM image serves both slots"
    );

    for handle in [&colour, &normal, &orm, &emissive] {
        settle(&mut app, handle);
    }
    let images = app.world().resource::<Assets<Image>>();
    for (handle, srgb) in [
        (&colour, true),
        (&normal, false),
        (&orm, false),
        // Emissive out of the bake is linear HDR, not an sRGB photograph.
        (&emissive, false),
    ] {
        let image = images.get(handle).expect("decoded image");
        assert_eq!(
            image.texture_descriptor.format == TextureFormat::Rgba8UnormSrgb,
            srgb,
            "wrong colour space for {:?}",
            image.texture_descriptor.format
        );
        let ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u,
            address_mode_v,
            ..
        }) = &image.sampler
        else {
            panic!("materials must name their own sampler, not inherit the default");
        };
        assert_eq!(*address_mode_u, ImageAddressMode::Repeat);
        assert_eq!(*address_mode_v, ImageAddressMode::Repeat);
    }
}

#[test]
fn a_ktx2_map_loads_through_bevy_with_its_whole_chain_and_its_colour_space() {
    let root = TempDir::new().expect("temp dir");
    let set = baked_set();
    write_ktx2(root.path(), "color.ktx2", &set.base_color, set.resolution);
    write_ktx2(root.path(), "normal.ktx2", &set.normal, set.resolution);
    write_ktx2(root.path(), "orm.ktx2", &set.orm, set.resolution);
    let height = set.height.as_ref().expect("the graph binds a height");
    write_ktx2(root.path(), "height.ktx2", height, set.resolution);

    // Preflight first: this is the path that used to decode through the `image`
    // crate, which does not read KTX2 at all.
    let path = root.path().join("materials.ron");
    std::fs::write(
        &path,
        "(materials: {\"paint\": (surface: Files(base_color: Some(\"color.ktx2\"), \
         normal: Some(\"normal.ktx2\"), orm: Some(\"orm.ktx2\"), height: Some(\"height.ktx2\")))})",
    )
    .expect("fixture library");
    let library =
        ashlar_bevy::read_library_with_graphs(&path, root.path(), &building(), &no_graphs())
            .expect("a KTX2 set preflights");

    let mut app = headless(root.path());
    let material = ashlar_bevy::create_material(
        &library.materials["paint"],
        app.world().resource::<AssetServer>(),
    );
    let colour = material.base_color_texture.clone().expect("colour handle");
    let normal = material.normal_map_texture.clone().expect("normal handle");
    for handle in [&colour, &normal] {
        settle(&mut app, handle);
    }
    let images = app.world().resource::<Assets<Image>>();
    for (handle, format, name) in [
        // The colour space comes from the loader settings `create_material` names per
        // slot, as it does for a PNG; the container's own descriptor says the
        // same thing, and the two agreeing is the point.
        (&colour, TextureFormat::Rgba8UnormSrgb, "base colour"),
        (&normal, TextureFormat::Rgba8Unorm, "normal"),
    ] {
        let image = images.get(handle).expect("decoded image");
        assert_eq!(image.texture_descriptor.format, format, "{name}");
        // Every level down to 1x1 arrived. A map that lost its chain here is a
        // wall the hardware point samples at distance, which is the whole
        // reason these files are KTX2 and not PNG.
        assert_eq!(
            image.texture_descriptor.mip_level_count as usize,
            levels(BAKED),
            "{name} lost its mip chain"
        );
        assert_eq!(image.texture_descriptor.size.width, BAKED, "{name}");
        assert_eq!(image.texture_descriptor.size.height, BAKED, "{name}");
        // Bevy concatenates the levels, so the byte count is the whole chain
        // and not level 0.
        let bytes: usize = match name {
            "base colour" => set.base_color.mips.iter().map(Vec::len).sum(),
            _ => set.normal.mips.iter().map(Vec::len).sum(),
        };
        assert_eq!(
            image.data.as_ref().expect("image data").len(),
            bytes,
            "{name}"
        );
    }
}

#[test]
fn a_supercompressed_map_loads_as_the_same_image_as_the_map_it_compresses() {
    // The reading half of the container's supercompression, and the only check
    // in this workspace that Bevy is built with a decoder for it: the shipped
    // study maps are written under scheme 2, so a loader without one reads a
    // startup error instead of a wall. The two files here hold one bake, and
    // what the loader hands back has to be one image.
    let root = TempDir::new().expect("temp dir");
    let set = baked_set();
    write_ktx2(root.path(), "plain.ktx2", &set.base_color, set.resolution);
    write_ktx2_zstd(root.path(), "small.ktx2", &set.base_color, set.resolution);
    let plain_bytes = std::fs::metadata(root.path().join("plain.ktx2"))
        .expect("fixture")
        .len();
    let small_bytes = std::fs::metadata(root.path().join("small.ktx2"))
        .expect("fixture")
        .len();
    assert!(
        small_bytes < plain_bytes,
        "{small_bytes} compressed against {plain_bytes} plain"
    );

    // Preflight takes it: the scheme is one this adapter's Bevy can read.
    let path = root.path().join("materials.ron");
    std::fs::write(
        &path,
        "(materials: {\"paint\": (surface: Files(base_color: Some(\"small.ktx2\")))})",
    )
    .expect("fixture library");
    ashlar_bevy::read_library_with_graphs(&path, root.path(), &building(), &no_graphs())
        .expect("a supercompressed KTX2 map preflights");

    let mut app = headless(root.path());
    let server = app.world().resource::<AssetServer>().clone();
    let load = |key: &'static str| {
        server
            .load_builder()
            .with_settings(|settings: &mut ImageLoaderSettings| settings.is_srgb = true)
            .load::<Image>(key)
    };
    let (plain, small) = (load("plain.ktx2"), load("small.ktx2"));
    for handle in [&plain, &small] {
        settle(&mut app, handle);
    }
    let images = app.world().resource::<Assets<Image>>();
    let (plain, small) = (
        images.get(&plain).expect("decoded image"),
        images.get(&small).expect("decoded image"),
    );
    assert_eq!(
        small.texture_descriptor.format,
        plain.texture_descriptor.format
    );
    assert_eq!(
        small.texture_descriptor.mip_level_count as usize,
        levels(BAKED)
    );
    assert_eq!(
        small.texture_descriptor.mip_level_count,
        plain.texture_descriptor.mip_level_count
    );
    assert_eq!(small.texture_descriptor.size, plain.texture_descriptor.size);
    // Level for level, texel for texel: Bevy hands back the whole chain
    // concatenated, so one comparison covers every level and the order they are
    // in. This is the assertion the compression has to survive.
    assert_eq!(
        small.data.as_ref().expect("image data"),
        plain.data.as_ref().expect("image data")
    );
    let chain: usize = set.base_color.mips.iter().map(Vec::len).sum();
    assert_eq!(small.data.as_ref().expect("image data").len(), chain);
}

#[test]
fn preflight_refuses_a_supercompression_scheme_bevy_has_no_decoder_for() {
    // Scheme 3 is ZLIB, which Bevy would need its `flate2` feature for and this
    // adapter does not build. Written by hand into a file the writer made,
    // because the writer can only produce the two schemes that do load.
    let root = TempDir::new().expect("temp dir");
    let set = baked_set();
    let mut file = ktx2::write(&set.orm, set.resolution).expect("a KTX2 file");
    // `supercompressionScheme` is the ninth `u32` after the identifier.
    let at = ktx2::IDENTIFIER.len() + 8 * 4;
    file[at..at + 4].copy_from_slice(&3_u32.to_le_bytes());
    std::fs::write(root.path().join("zlib.ktx2"), &file).expect("fixture");
    let error = read_error(root.path(), &surface("orm: Some(\"zlib.ktx2\")"));
    assert!(error.contains("supercompression scheme 3"), "{error}");
    assert!(error.contains("Zstandard decoder (scheme 2)"), "{error}");
}

#[test]
fn a_sixteen_bit_height_map_keeps_its_depth_through_the_loader() {
    // The height plane is the one map a PNG bake could not write at all, and
    // R16 is what it ships as. If this ever came back as eight bits, the
    // parallax and the relief would disagree about the same surface.
    let root = TempDir::new().expect("temp dir");
    let set = baked_set();
    let height = set.height.as_ref().expect("the graph binds a height");
    assert_eq!(height.format, PlaneFormat::R16Unorm);
    write_ktx2(root.path(), "height.ktx2", height, set.resolution);

    let mut app = headless(root.path());
    let handle: Handle<Image> = app
        .world()
        .resource::<AssetServer>()
        .load_builder()
        .with_settings(|settings: &mut ImageLoaderSettings| settings.is_srgb = false)
        .load("height.ktx2");
    settle(&mut app, &handle);
    let images = app.world().resource::<Assets<Image>>();
    let image = images.get(&handle).expect("decoded image");
    assert_eq!(image.texture_descriptor.format, TextureFormat::R16Unorm);
    assert_eq!(
        image.texture_descriptor.mip_level_count as usize,
        levels(BAKED)
    );
}

#[test]
fn preflight_names_a_ktx2_key_it_cannot_open_or_parse() {
    let root = TempDir::new().expect("temp dir");
    let error = read_error(root.path(), &surface("base_color: Some(\"absent.ktx2\")"));
    assert!(error.contains("opening texture absent.ktx2"), "{error}");

    // A file with the right extension and the wrong contents: the `image` crate
    // would not have noticed this one either way, because it cannot read KTX2.
    std::fs::write(root.path().join("corrupt.ktx2"), b"not a container").expect("fixture");
    let error = read_error(root.path(), &surface("normal: Some(\"corrupt.ktx2\")"));
    assert!(error.contains("decoding texture corrupt.ktx2"), "{error}");
    assert!(error.contains("not a KTX2 file"), "{error}");

    // A KTX2 file that is truncated after its header: structurally a KTX2 file,
    // and not one whose levels are all there.
    let set = baked_set();
    let whole = ktx2::write(&set.orm, set.resolution).expect("a KTX2 file");
    std::fs::write(root.path().join("short.ktx2"), &whole[..whole.len() / 2]).expect("fixture");
    let error = read_error(root.path(), &surface("orm: Some(\"short.ktx2\")"));
    assert!(error.contains("truncated KTX2 file"), "{error}");
}

/// An app with an asset server and Bevy's image loaders, and no render device.
fn headless(root: &Path) -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin {
            file_path: root.to_string_lossy().into_owned(),
            ..default()
        },
    ))
    .init_asset::<Image>()
    .register_asset_loader(ImageLoader::new(CompressedImageFormats::NONE));
    app
}

/// Drive the app until one asset has loaded. The loader runs on the IO task
/// pool, so a load needs frames rather than a wait.
fn settle(app: &mut App, handle: &Handle<Image>) {
    for _ in 0..2000 {
        app.update();
        match app
            .world()
            .resource::<AssetServer>()
            .load_state(handle.id())
        {
            LoadState::Loaded => return,
            LoadState::Failed(error) => panic!("{error}"),
            LoadState::NotLoaded | LoadState::Loading => {}
        }
    }
    panic!("texture never finished loading");
}

#[test]
fn a_height_map_is_preflighted_but_reaches_no_standard_material_slot() {
    let root = TempDir::new().expect("temp dir");
    write_png(root.path(), "height.png");
    let mut app = headless(root.path());
    let material = ashlar_bevy::create_material(
        &MaterialDefinition {
            surface: textures([None, None, None, Some("height.png"), None]),
            ..MaterialDefinition::default()
        },
        app.world().resource::<AssetServer>(),
    );
    // `depth_map` is the slot a height map would plausibly reach, and this
    // adapter deliberately leaves it empty: Bevy wants black-is-top depth and a
    // `parallax_depth_scale` in the mesh's own units, neither of which a 0..=1
    // height field decides on its own. Assert it with the rest so wiring it is a test
    // failure and a decision rather than a silent change of picture.
    for handle in [
        &material.base_color_texture,
        &material.normal_map_texture,
        &material.metallic_roughness_texture,
        &material.occlusion_texture,
        &material.emissive_texture,
        &material.depth_map,
    ] {
        assert!(handle.is_none(), "height must not fill another slot");
    }
    app.update();
}

#[test]
fn a_baked_surface_is_preflighted_against_the_graph_library_beside_it() {
    let root = TempDir::new().expect("temp dir");
    let surface =
        "Graph((graph: \"study:concrete\", params: {\"wear\": Float(0.5)}, resolution: 512))";
    let path = write_library(root.path(), surface);
    // A graph surface opens no file, so what makes it a startup error is the
    // lowering: with the library beside it the bake plans, without it the key
    // resolves to nothing.
    ashlar_bevy::read_library_with_graphs(&path, root.path(), &building(), &graphs())
        .expect("a graph surface names no files to open");
    let error = format!(
        "{:#}",
        ashlar_bevy::read_library_with_graphs(&path, root.path(), &building(), &no_graphs())
            .expect_err("an unknown graph is a startup error")
    );
    assert!(error.contains("study:concrete"), "{error}");
    assert!(error.contains("unknown material graph"), "{error}");
    assert!(error.contains("paint"), "{error}");
}

#[test]
fn a_bake_whose_parameters_or_resolution_are_wrong_is_refused_by_path() {
    let root = TempDir::new().expect("temp dir");
    for (surface, expected) in [
        (
            "Graph((graph: \"study:concrete\", params: {\"polish\": Float(0.5)}, resolution: 512))",
            "no parameter \"polish\"",
        ),
        (
            "Graph((graph: \"study:concrete\", params: {\"wear\": Color((1.0, 0.0, 0.0))}, resolution: 512))",
            "takes a Float for \"wear\"",
        ),
    ] {
        let path = write_library(root.path(), surface);
        let error = format!(
            "{:#}",
            ashlar_bevy::read_library_with_graphs(&path, root.path(), &building(), &graphs())
                .expect_err("a bad override is a startup error")
        );
        assert!(error.contains(expected), "{error}");
        assert!(error.contains("params["), "{error}");
    }

    // A resolution outside the bake's range is refused by `MaterialDefinition`
    // itself, before the graph library is consulted at all.
    let path = write_library(
        root.path(),
        "Graph((graph: \"study:concrete\", resolution: 300))",
    );
    let error = format!(
        "{:#}",
        ashlar_bevy::read_library_with_graphs(&path, root.path(), &building(), &graphs())
            .expect_err("a bad resolution is a startup error")
    );
    assert!(error.contains("power of two"), "{error}");
}

#[test]
fn a_shader_surface_is_preflighted_by_compiling_it() {
    let root = TempDir::new().expect("temp dir");
    let path = write_library(
        root.path(),
        "Shader(graph: \"study:concrete\", params: {\"wear\": Float(0.3)})",
    );
    // The same graph the `Graph` case above bakes, compiled instead: a surface
    // that reaches `create_shader_material` has already been lowered, partitioned and
    // printed, so the only thing left that can fail is the GPU compiling it.
    ashlar_bevy::read_library_with_graphs(&path, root.path(), &building(), &graphs())
        .expect("a shader surface over a graph that compiles");
    // And a parameter the graph does not declare is a startup error naming it,
    // exactly as it is for a bake.
    let path = write_library(
        root.path(),
        "Shader(graph: \"study:concrete\", params: {\"sparkle\": Float(0.3)})",
    );
    let error = format!(
        "{:#}",
        ashlar_bevy::read_library_with_graphs(&path, root.path(), &building(), &graphs())
            .expect_err("a parameter the graph does not declare")
    );
    assert!(error.contains("params[sparkle]"), "{error}");
    assert!(error.contains("study:concrete"), "{error}");
}

#[test]
fn a_graph_library_is_read_and_validated_from_ron() {
    let root = TempDir::new().expect("temp dir");
    let path = root.path().join("materials.graphs.ron");
    std::fs::write(
        &path,
        ron::ser::to_string_pretty(&graphs(), ron::ser::PrettyConfig::default()).unwrap(),
    )
    .expect("fixture graph library");
    let library = ashlar_bevy::read_graphs(&path).expect("a valid graph library");
    assert!(library.get("study:concrete").is_some());

    // A graph that parses but does not build is refused here rather than at the
    // first surface that names it.
    let broken = root.path().join("broken.graphs.ron");
    std::fs::write(
        &broken,
        "(graphs: {\"a\": (id: \"a\", nodes: {\"n\": Invert((input: Node(\"missing\")))},          output: (base_color: Node(\"n\")))})",
    )
    .expect("fixture graph library");
    let error = format!(
        "{:#}",
        ashlar_bevy::read_graphs(&broken).expect_err("an unknown node reference")
    );
    assert!(error.contains("nodes[n].inputs[input]"), "{error}");
}

#[test]
fn create_without_a_bake_context_leaves_a_baked_surface_flat() {
    let root = TempDir::new().expect("temp dir");
    let mut app = headless(root.path());
    let material = ashlar_bevy::create_material(
        &MaterialDefinition {
            base_color: [0.2, 0.3, 0.4],
            surface: Surface::Graph(study_bake(0.5)),
            ..MaterialDefinition::default()
        },
        app.world().resource::<AssetServer>(),
    );
    // `create_material` takes an `AssetServer` and nothing else, so it has nothing to
    // bake with and nowhere to put the result. A graph surface comes back as
    // the definition's constants, and a library that can hold one goes through
    // `create_graph_material` instead. Asserted rather than assumed, so that giving
    // `create_material` a bake path of its own would be a decision and not a surprise.
    assert_eq!(material.base_color, Color::srgb(0.2, 0.3, 0.4));
    assert!(material.base_color_texture.is_none());
    app.update();
}

#[test]
fn a_baked_surface_carries_the_bytes_the_ktx2_of_that_bake_carries() {
    // The claim the whole runtime path rests on: moving a surface between files
    // and a registration-time bake is a change of variant in the library and
    // nothing in the picture. So bake the same `Bake` both ways — one through
    // `create_graph_material`, one written as KTX2 and read back by Bevy's own loader —
    // and compare the images level for level.
    let root = TempDir::new().expect("temp dir");
    let set = study_set(0.5);
    for (key, encoded) in [
        ("color.ktx2", &set.base_color),
        ("normal.ktx2", &set.normal),
        ("orm.ktx2", &set.orm),
    ] {
        write_ktx2(root.path(), key, encoded, set.resolution);
    }
    let height = set.height.as_ref().expect("the graph binds a height");
    write_ktx2(root.path(), "height.ktx2", height, set.resolution);

    let mut app = headless(root.path());
    let files = ashlar_bevy::create_material(
        &MaterialDefinition {
            surface: textures([
                Some("color.ktx2"),
                Some("normal.ktx2"),
                Some("orm.ktx2"),
                Some("height.ktx2"),
                None,
            ]),
            ..MaterialDefinition::default()
        },
        app.world().resource::<AssetServer>(),
    );
    let mut cache = BakeCache::new();
    let baked = create_graph_material(&mut app, &baked_definition(0.5, 1.0), &graphs(), &mut cache);

    // The height map is on neither material — `depth_map` stays empty for a
    // bake exactly as it does for files — so it is loaded on its own and
    // compared against the handle the cache kept.
    let loaded_height: Handle<Image> = app
        .world()
        .resource::<AssetServer>()
        .load_builder()
        .with_settings(|settings: &mut ImageLoaderSettings| settings.is_srgb = false)
        .load("height.ktx2");
    let images = GraphImages {
        base_color: files.base_color_texture.clone().expect("colour from file"),
        normal: files.normal_map_texture.clone().expect("normal from file"),
        orm: files
            .metallic_roughness_texture
            .clone()
            .expect("orm from file"),
        height: Some(loaded_height),
        emissive: None,
    };
    for handle in [&images.base_color, &images.normal, &images.orm] {
        settle(&mut app, handle);
    }
    settle(&mut app, images.height.as_ref().expect("height from file"));

    let key = BakeKey::new(&study_bake(0.5));
    let run = cache.get(&key).expect("the bake is in the cache").clone();
    assert!(baked.depth_map.is_none(), "height fills no relief slot");
    assert_eq!(
        baked.occlusion_texture.as_ref(),
        baked.metallic_roughness_texture.as_ref(),
        "one ORM image serves both slots, as it does for files"
    );
    assert!(run.emissive.is_none(), "the graph binds no emissive");

    let assets = app.world().resource::<Assets<Image>>();
    for (name, from_file, from_bake, encoded) in [
        (
            "base colour",
            &images.base_color,
            &run.base_color,
            &set.base_color,
        ),
        ("normal", &images.normal, &run.normal, &set.normal),
        ("orm", &images.orm, &run.orm, &set.orm),
        (
            "height",
            images.height.as_ref().expect("height from file"),
            run.height.as_ref().expect("height from the bake"),
            height,
        ),
    ] {
        let file = assets.get(from_file).expect("loaded image");
        let bake = assets.get(from_bake).expect("baked image");
        assert_eq!(
            bake.texture_descriptor.format, file.texture_descriptor.format,
            "{name}"
        );
        assert_eq!(
            bake.texture_descriptor.mip_level_count as usize,
            levels(BAKED),
            "{name} lost its mip chain"
        );
        assert_eq!(
            bake.texture_descriptor.mip_level_count, file.texture_descriptor.mip_level_count,
            "{name}"
        );
        assert_eq!(
            bake.texture_descriptor.size, file.texture_descriptor.size,
            "{name}"
        );
        let bytes = bake.data.as_ref().expect("baked data");
        assert_eq!(bytes, file.data.as_ref().expect("loaded data"), "{name}");
        // And both are the encoder's own bytes, largest level first, which is
        // the order Bevy reads a concatenated chain in.
        assert_eq!(*bytes, encoded.mips.concat(), "{name}");
        assert_eq!(
            bake.data_order,
            bevy::render::render_resource::TextureDataOrder::MipMajor
        );
    }
}

#[test]
fn a_baked_map_is_a_repeating_trilinear_texture_in_the_render_world_only() {
    let root = TempDir::new().expect("temp dir");
    let mut app = headless(root.path());
    let mut cache = BakeCache::new();
    let material =
        create_graph_material(&mut app, &baked_definition(0.5, 1.0), &graphs(), &mut cache);
    let run = cache
        .get(&BakeKey::new(&study_bake(0.5)))
        .expect("the bake is in the cache")
        .clone();
    assert_eq!(material.base_color_texture.as_ref(), Some(&run.base_color));

    let assets = app.world().resource::<Assets<Image>>();
    // Each plane in the format its own encoder wrote, with no transcoding
    // between the bake and the GPU.
    for (name, handle, format) in [
        (
            "base colour",
            &run.base_color,
            TextureFormat::Rgba8UnormSrgb,
        ),
        ("normal", &run.normal, TextureFormat::Rgba8Unorm),
        ("orm", &run.orm, TextureFormat::Rgba8Unorm),
        (
            "height",
            run.height.as_ref().expect("the graph binds a height"),
            // Sixteen bits, with no file needed to carry them.
            TextureFormat::R16Unorm,
        ),
    ] {
        let image = assets.get(handle).expect("baked image");
        assert_eq!(image.texture_descriptor.format, format, "{name}");
        assert_eq!(
            image.texture_descriptor.mip_level_count as usize,
            levels(BAKED),
            "{name}"
        );
        // The sampler is the one `create_material` asks Bevy's loader for on a file map:
        // repeat in both axes, and linear all the way through the chain, which
        // is what stops the mips this bake just built from showing as a ring on
        // the ground.
        let ImageSampler::Descriptor(descriptor) = &image.sampler else {
            panic!("a baked map must name its own sampler, not inherit the default");
        };
        assert_eq!(
            descriptor.address_mode_u,
            ImageAddressMode::Repeat,
            "{name}"
        );
        assert_eq!(
            descriptor.address_mode_v,
            ImageAddressMode::Repeat,
            "{name}"
        );
        assert_eq!(descriptor.min_filter, ImageFilterMode::Linear, "{name}");
        assert_eq!(descriptor.mag_filter, ImageFilterMode::Linear, "{name}");
        assert_eq!(descriptor.mipmap_filter, ImageFilterMode::Linear, "{name}");
        // These texels exist to be uploaded, and a texture set is megabytes:
        // the main world keeps no copy of one.
        assert_eq!(
            image.asset_usage,
            bevy::asset::RenderAssetUsages::RENDER_WORLD,
            "{name}"
        );
    }
}

#[test]
fn two_definitions_with_one_bake_share_one_texture_set() {
    let root = TempDir::new().expect("temp dir");
    let mut app = headless(root.path());
    let library = graphs();
    let mut cache = BakeCache::new();
    // Two materials that differ in their constants and agree on their bake:
    // the study's concrete and its cut faces, which is exactly this shape.
    let first = create_graph_material(&mut app, &baked_definition(0.5, 1.0), &library, &mut cache);
    let before = app.world().resource::<Assets<Image>>().len();
    let second = create_graph_material(&mut app, &baked_definition(0.5, 0.4), &library, &mut cache);

    assert_eq!(cache.len(), 1, "one bake, one entry");
    assert_eq!(
        app.world().resource::<Assets<Image>>().len(),
        before,
        "the second definition added no images"
    );
    assert_eq!(first.base_color_texture, second.base_color_texture);
    assert_eq!(first.normal_map_texture, second.normal_map_texture);
    assert_eq!(first.occlusion_texture, second.occlusion_texture);
    // Shared textures, own constants.
    assert_eq!(second.perceptual_roughness, 0.4);
    assert_eq!(first.perceptual_roughness, 1.0);

    cache.clear();
    assert!(cache.is_empty());
    let again = create_graph_material(&mut app, &baked_definition(0.5, 1.0), &library, &mut cache);
    assert_ne!(
        again.base_color_texture, first.base_color_texture,
        "a cleared cache bakes again, which is what a reloaded library needs"
    );
}

#[test]
fn a_definition_with_a_different_parameter_gets_a_different_set() {
    let root = TempDir::new().expect("temp dir");
    let mut app = headless(root.path());
    let library = graphs();
    let mut cache = BakeCache::new();
    let worn = create_graph_material(&mut app, &baked_definition(1.0, 1.0), &library, &mut cache);
    let fresh = create_graph_material(&mut app, &baked_definition(0.25, 1.0), &library, &mut cache);

    assert_eq!(cache.len(), 2, "two bakes, two entries");
    assert_ne!(worn.base_color_texture, fresh.base_color_texture);
    let assets = app.world().resource::<Assets<Image>>();
    let colour = |material: &StandardMaterial| {
        assets
            .get(material.base_color_texture.as_ref().expect("colour"))
            .expect("baked image")
            .data
            .clone()
            .expect("baked data")
    };
    // Not just distinct handles: the parameter reached the texels. `wear`
    // multiplies the base colour, so the worn set is the brighter one.
    assert_ne!(colour(&worn), colour(&fresh));
    assert_eq!(colour(&worn), study_set(1.0).base_color.mips.concat());
    assert_eq!(colour(&fresh), study_set(0.25).base_color.mips.concat());
    // The resolution is part of the key as much as the parameters are.
    let mut coarse = study_bake(1.0);
    coarse.resolution = 512;
    assert_ne!(BakeKey::new(&coarse), BakeKey::new(&study_bake(1.0)));
}

#[test]
fn create_baked_sends_a_shader_surface_to_the_entry_point_that_compiles_one() {
    let root = TempDir::new().expect("temp dir");
    let mut app = headless(root.path());
    let library = graphs();
    let mut cache = BakeCache::new();
    let definition = MaterialDefinition {
        surface: Surface::Shader {
            graph: "study:concrete".into(),
            params: std::collections::BTreeMap::new(),
        },
        ..MaterialDefinition::default()
    };
    let error = app
        .world_mut()
        .resource_scope(|world, mut images: Mut<Assets<Image>>| {
            format!(
                "{:#}",
                ashlar_bevy::runtime_bake::create_graph_material(
                    &definition,
                    world.resource::<AssetServer>(),
                    &mut BakeContext {
                        graphs: &library,
                        cache: &mut cache,
                        images: &mut images,
                        threads: THREADS,
                        baker: Baker::Cpu,
                    },
                )
                .expect_err("a compiled graph is not a StandardMaterial")
            )
        });
    assert!(error.contains("create_shader_material"), "{error}");
    assert!(error.contains("study:concrete"), "{error}");
}

#[test]
fn a_bake_naming_a_graph_the_library_does_not_hold_says_what_it_does_hold() {
    // Unreachable after `read_library`, which preflights the same key against the same
    // library. Reachable for a caller that built its definitions by hand, and
    // the message is the preflight's so both read alike.
    let root = TempDir::new().expect("temp dir");
    let mut app = headless(root.path());
    let library = graphs();
    let mut cache = BakeCache::new();
    let mut bake = study_bake(0.5);
    bake.graph = "study:plaster".into();
    let definition = MaterialDefinition {
        surface: Surface::Graph(bake),
        ..MaterialDefinition::default()
    };
    let error = app
        .world_mut()
        .resource_scope(|world, mut images: Mut<Assets<Image>>| {
            format!(
                "{:#}",
                ashlar_bevy::runtime_bake::create_graph_material(
                    &definition,
                    world.resource::<AssetServer>(),
                    &mut BakeContext {
                        graphs: &library,
                        cache: &mut cache,
                        images: &mut images,
                        threads: THREADS,
                        baker: Baker::Cpu,
                    },
                )
                .expect_err("an unknown graph cannot bake")
            )
        });
    assert!(error.contains("unknown material graph"), "{error}");
    assert!(error.contains("study:plaster"), "{error}");
    assert!(error.contains("study:concrete"), "{error}");
}

#[test]
fn a_definition_and_a_graph_hold_one_parameter_value_type() {
    // There were two enums and a converter between them until `ashlar-surface`
    // went in below both crates. One type now, under both names, so a bake's
    // own map is the map a graph binds.
    let value: ashlar_material::ParamValue = ashlar::ParamValue::Int(-3);
    assert_eq!(value, ashlar_surface::ParamValue::Int(-3));
}

/// An app with no renderer gets no baker, and a context built from one bakes on
/// the CPU.
///
/// The fallback the whole GPU path rests on: `MinimalPlugins` has no
/// `RenderDevice`, `GpuBakePlugin` inserts nothing, and every caller that asks
/// the world for a baker asks with an `Option` and gets [`Baker::Cpu`]. A
/// headless test, a build server and a game started with `--no-render` are all
/// this case, and none of them may be an error.
///
/// The one test in this file that needs a feature of its own: without
/// `gpu-bake` there is no `GpuBaker` to be missing and no `Baker::Gpu` to fall
/// back from, so the claim has nothing to say.
#[cfg(feature = "gpu-bake")]
#[test]
fn an_app_without_a_renderer_bakes_on_the_cpu() {
    use ashlar_bevy::gpu::{GpuBakePlugin, GpuBaker};

    let root = TempDir::new().expect("temp dir");
    let mut app = headless(root.path());
    app.add_plugins(GpuBakePlugin);
    app.finish();
    assert!(
        app.world().get_resource::<GpuBaker>().is_none(),
        "MinimalPlugins has no render device to bake on"
    );
    assert!(Baker::or_cpu(None).gpu().is_none());
    assert!(matches!(Baker::default(), Baker::Cpu));
}

#[test]
fn an_instance_override_is_preflighted_against_the_graph_the_definition_names() {
    let root = TempDir::new().expect("temp dir");
    let path = write_library(
        root.path(),
        "Graph((graph: \"study:concrete\", params: {\"wear\": Float(0.5)}, resolution: 512))",
    );
    let read = |binding: Binding| {
        ashlar_bevy::read_library_with_graphs(
            &path,
            root.path(),
            &bound_building(binding),
            &graphs(),
        )
    };
    read(Binding::new("paint").param("wear", ParamValue::Float(0.9)))
        .expect("an override of a parameter the graph declares");

    // The two things only a crate holding the graph library can see, both at
    // the path of the binding that asked rather than of the definition.
    for (binding, expected) in [
        (
            Binding::new("paint").param("polish", ParamValue::Float(0.9)),
            "no parameter \"polish\"",
        ),
        (
            Binding::new("paint").param("wear", ParamValue::Color([1.0, 0.0, 0.0])),
            "takes a Float for \"wear\"",
        ),
    ] {
        let error = format!("{:#}", read(binding).expect_err("a bad override"));
        assert!(error.contains(expected), "{error}");
        assert!(
            error.contains("instances[only].materials[outer]"),
            "an override is reported where it was written: {error}"
        );
    }

    // And the thing the domain can see on its own: a surface with no graph
    // behind it has nothing to override, which is a validation error rather
    // than a value that quietly does nothing.
    let path = write_library(root.path(), "Plain");
    let error = format!(
        "{:#}",
        ashlar_bevy::read_library_with_graphs(
            &path,
            root.path(),
            &bound_building(Binding::new("paint").param("wear", ParamValue::Float(0.9))),
            &graphs(),
        )
        .expect_err("an override of a constant surface")
    );
    assert!(
        error.contains("instances[only].materials[outer]"),
        "{error}"
    );
    assert!(error.contains("Plain"), "{error}");
}

#[test]
fn an_instance_override_bakes_its_own_texture_set_and_shares_it_with_an_equal_one() {
    let root = TempDir::new().expect("temp dir");
    let mut app = headless(root.path());
    let library = graphs();
    let mut cache = BakeCache::new();
    // One definition, at the value the library chose.
    let mut definitions = MaterialLibrary::default();
    definitions
        .materials
        .insert("paint".into(), baked_definition(0.25, 1.0));
    let dressed = |wear: Option<f32>| {
        let mut binding = Binding::new("paint");
        if let Some(wear) = wear {
            binding = binding.param("wear", ParamValue::Float(wear));
        }
        ashlar_bevy::definition(&definitions, &binding)
            .expect("a bound definition")
            .into_owned()
    };

    let library_value = create_graph_material(&mut app, &dressed(None), &library, &mut cache);
    let worn = create_graph_material(&mut app, &dressed(Some(1.0)), &library, &mut cache);
    let again = create_graph_material(&mut app, &dressed(Some(1.0)), &library, &mut cache);
    let half = create_graph_material(&mut app, &dressed(Some(0.5)), &library, &mut cache);

    // Three distinct values, three sets, and the two instances that asked for
    // one value share it — which is the whole point of keying by the bake and
    // not by the instance.
    assert_eq!(cache.len(), 3, "three values of one parameter, three sets");
    assert_eq!(worn.base_color_texture, again.base_color_texture);
    assert_ne!(worn.base_color_texture, library_value.base_color_texture);
    assert_ne!(worn.base_color_texture, half.base_color_texture);
    assert_eq!(
        cache
            .keys()
            .map(ashlar_bevy::runtime_bake::BakeKey::graph)
            .collect::<std::collections::BTreeSet<_>>(),
        ["study:concrete"].into_iter().collect(),
        "an override changes the values and never the graph"
    );

    // And the value reached the texels: an overridden bake is the bake a
    // definition naming that value would have run, byte for byte.
    let assets = app.world().resource::<Assets<Image>>();
    let colour = |material: &StandardMaterial| {
        assets
            .get(material.base_color_texture.as_ref().expect("colour"))
            .expect("baked image")
            .data
            .clone()
            .expect("baked data")
    };
    assert_eq!(colour(&worn), study_set(1.0).base_color.mips.concat());
    assert_eq!(
        colour(&library_value),
        study_set(0.25).base_color.mips.concat()
    );
    // The definition's own constants are untouched by an override of its
    // parameters.
    assert_eq!(worn.perceptual_roughness, 1.0);
}

#[test]
fn ashlar_bakes_convert_normal_y_but_external_opengl_maps_do_not() {
    let root = TempDir::new().expect("temp dir");
    let mut app = headless(root.path());
    let mut definition = MaterialDefinition {
        surface: textures([None, Some("normal.png"), None, None, None]),
        ..MaterialDefinition::default()
    };
    assert!(
        !ashlar_bevy::create_material(&definition, app.world().resource::<AssetServer>())
            .flip_normal_map_y
    );
    if let Surface::Files { baked_from, .. } = &mut definition.surface {
        *baked_from = Some(study_bake(0.5));
    }
    assert!(
        ashlar_bevy::create_material(&definition, app.world().resource::<AssetServer>())
            .flip_normal_map_y
    );
    let material = create_graph_material(
        &mut app,
        &baked_definition(0.5, 1.0),
        &graphs(),
        &mut BakeCache::new(),
    );
    assert!(material.flip_normal_map_y);
}
