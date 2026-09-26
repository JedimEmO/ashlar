//! The one test in the workspace that draws.
//!
//! Everything else checks a number on its way to the GPU. `tests/materials.rs`
//! and `tests/shaders.rs` check what a definition becomes without a device at
//! all; `tests/conformance.rs` opens a real one but dispatches a *compute*
//! kernel and compares texels, which is arithmetic rather than a picture. So
//! the whole of the draw path — a mesh uploaded with the attributes a material
//! reads, a KTX2 set arriving before the frame that wants it, a generated
//! fragment assembled by `naga_oil` against Bevy's shader defs and accepted by
//! the driver, and a prepass entry beside it — was held up by nothing. The
//! deferred-prepass bug fixed in 9be70ae is what came through that gap: every
//! test passed while a compiled surface was invisible on a deferred camera,
//! because no test had ever looked at a rendered pixel.
//!
//! This file looks at rendered pixels. A camera renders a fixed studio into an
//! offscreen image, the image is read back, and each case asks a question of a
//! named patch of it:
//!
//! - [`a_plain_and_a_textured_material_both_reach_the_frame`], which needs **no
//!   cargo features**, because the baked-files path is what a game compiles and
//!   it has to hold on its own;
//! - three cases behind `shader`, one per branch of that commit's
//!   `swap_fragment`: the generated WGSL meeting a real driver through the
//!   forward path with a prepass, the same thing on a camera that *also* runs a
//!   deferred prepass for somebody else, and a surface drawn deferred, which is
//!   9be70ae's own.
//!
//! **Every one of them needs a GPU**, so each is `#[ignore]`d and `just draw`
//! is what runs them — twice, once featureless and once with `shader`, because
//! the first case is a claim about a build with no features in it and a run
//! with the feature on does not make that claim. The exception is
//! [`the_patches_are_disjoint_and_land_where_the_slabs_stand`], which is the
//! framing arithmetic alone and so runs in `just ci`: a patch that drifted off
//! its slab would make every assertion below a measurement of the background.
//!
//! # What the studio is, and why it is this shape
//!
//! Two slabs side by side, an **orthographic** camera square on them, and a key
//! light off to one side. Orthographic because the patch arithmetic then has no
//! projection in it: a metre is the same number of pixels everywhere, so a box
//! in metres is a box in pixels and [`pixels_of`] is four multiplications. The
//! slabs stand well inside the frame, which leaves a band of background at the
//! top that every case checks is *still* the clear colour — the control that
//! says the patches are where this file thinks they are.
//!
//! The camera tonemaps with [`Tonemapping::None`] and does not dither. Both are
//! deliberate: a tonemapper is a lookup that moves a colour for reasons that
//! have nothing to do with which material drew it, and the dither would put a
//! code of noise into the one patch this file needs to be flat — the plain
//! material's, which is the control for the textured material's variation.
//! What is under test is the join, not the grade.
#![allow(
    clippy::unwrap_used,
    reason = "fixtures built in the test, where a failure is the test failing"
)]

use ashlar::{
    Building, Element, Geometry, Instance, MaterialDefinition, MaterialLibrary, Part, Pose, Surface,
};
use ashlar_manifold::{ManifoldMesher, mesh_building};
use bevy::{
    app::{TaskPoolOptions, TaskPoolPlugin},
    camera::{ClearColorConfig, RenderTarget, ScalingMode},
    core_pipeline::{
        prepass::{DeferredPrepass, DepthPrepass, NormalPrepass},
        tonemapping::{DebandDither, Tonemapping},
    },
    pbr::DefaultOpaqueRendererMethod,
    prelude::*,
    render::{
        RenderPlugin,
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::TextureFormat,
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
    window::ExitCondition,
    winit::WinitPlugin,
};
// Only the compiled cases read the render world's pipeline cache, and a
// featureless build that imported it would be importing something it never
// names.
#[cfg(feature = "shader")]
use bevy::render::{
    RenderApp,
    render_resource::{CachedPipelineState, PipelineCache, PipelineDescriptor},
};

/// The asset root the cases read, as an absolute path.
///
/// Nothing baked is committed, so the files a game would ship are made here,
/// once per run, by the same content step a game runs:
/// [`ashlar_material::export`] over the default library's brick and grass,
/// written under Cargo's scratch directory for this test target. What the
/// cases then load is what a game loads — KTX2 maps and a strand set off disk —
/// and the library crate under test still has no graph engine in it.
fn assets() -> &'static str {
    use ashlar_material::{
        bake::bake_with_report,
        export::{ExportRequest, export},
        ktx2::Supercompression,
        stdlib,
    };
    static ROOT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let root = concat!(env!("CARGO_TARGET_TMPDIR"), "/draw-assets");
        let graphs = stdlib::graphs();
        let mut definitions = stdlib::materials();
        definitions
            .materials
            .retain(|key, _| key == "library:brick" || key == "library:grass");
        let exported = export(&ExportRequest {
            graphs: &graphs,
            definitions: &definitions,
            directory: "materials",
            resolution: Some(512),
            threads: std::num::NonZeroUsize::new(4),
            backend: &bake_with_report,
        })
        .unwrap();
        for set in exported.sets {
            for (path, bytes) in set.files(Supercompression::Zstd).unwrap() {
                let path = std::path::Path::new(root).join(path);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, bytes).unwrap();
            }
        }
        root.to_owned()
    })
}

/// Pixels to a side of the frame every case renders into.
///
/// Square, so that [`ScalingMode::Fixed`] over a square view makes a metre the
/// same number of pixels in both axes. Small on purpose: sixty-five thousand
/// pixels is already thousands per patch, and the readback is synchronous.
const SIDE: u32 = 256;

/// Metres across the square the camera frames.
const VIEW: f32 = 4.0;

/// Metres to a side of each slab's front face, and how deep it stands.
///
/// Two of these plus a gap fit inside [`VIEW`] with room to spare, which is
/// what leaves the background band at the top that [`SKY`] measures.
const SLAB: [f64; 3] = [1.8, 1.8, 0.4];

/// The clear colour, as the sRGB codes an `Rgba8UnormSrgb` target holds it as.
///
/// Written as codes rather than as floats because that is the form every
/// comparison in this file makes: the frame comes back as bytes, and a pixel
/// "still the clear colour" is a byte comparison with no decode in the way.
/// Dark and blue-green, so that nothing a case draws — a red slab, a brick
/// wall, a green graph — is anywhere near it.
const CLEAR_CODES: [u8; 3] = [5, 15, 26];

/// How far a pixel may sit from [`CLEAR_CODES`] and still count as untouched.
///
/// Two codes. The clear value makes a round trip — a linear clear into an sRGB
/// target and back out through the readback — and a driver is not required to
/// land on the same byte it started from.
const CLEAR_SLACK: i16 = 2;

/// The plain material's base colour: a red nothing else in the studio is near.
///
/// Strong and deliberately off-balance, because the assertion it has to support
/// is "this patch is *that* material", and a ratio between lanes survives any
/// exposure the studio is set to where an absolute value does not.
const PLAIN: [f32; 3] = [0.85, 0.05, 0.05];

/// Metres of wall one repeat of the brick set covers, for this studio.
///
/// Smaller than the 1.72 m the set was drawn for, so that a 1.8 m slab shows
/// two repeats rather than one: what the textured case measures is variation
/// across the patch, and more of the pattern in frame is more of it to measure.
const TILE: f32 = 0.9;

/// The key light, in lux.
///
/// Chosen against Bevy's default exposure so the plain slab lands near the
/// middle of the range rather than clipped at the top: with `Tonemapping::None`
/// nothing rolls a highlight off, so a light set by eye from a windowed preview
/// would saturate every lane and make the colour assertion vacuous.
const KEY_LUX: f32 = 2_500.0;

/// The ambient term, in lux.
///
/// Low but not zero, so that a surface facing away from the key is still
/// distinguishable from the background: a case that asserted "not the clear
/// colour" over an unlit face would be asserting that black is not dark blue.
const AMBIENT_LUX: f32 = 120.0;

/// Consecutive ready frames a case renders before it captures.
///
/// An asset being loaded is not the same as its texture being on the device:
/// the image arrives in the main world, and the render world uploads it in the
/// *next* extract. Four is several times that, and cheap.
const SETTLE: u32 = 4;

/// The most frames any case will render before giving up.
///
/// A cap rather than a count, because what a case waits for is a load state and
/// not a number of frames — but a load that never completes must fail the test
/// rather than hang the suite.
const FRAMES: u32 = 900;

// --------------------------------------------------------------------------
// The frame, and the patches measured in it
// --------------------------------------------------------------------------

/// One rendered frame, as the sRGB bytes the target holds.
struct Frame {
    pixels: Vec<[u8; 4]>,
}

impl Frame {
    /// The captured image, checked to be the target this file set up.
    ///
    /// The format and the size are asserted rather than adapted to: every
    /// number below is in terms of both, and a screenshot that came back as
    /// something else would be quietly measured in the wrong units.
    fn of(image: &Image) -> Self {
        assert_eq!(
            image.texture_descriptor.format,
            TextureFormat::Rgba8UnormSrgb,
            "the capture came back in a format this file does not decode"
        );
        assert_eq!(
            (image.width(), image.height()),
            (SIDE, SIDE),
            "the capture came back at a size the patch arithmetic is not for"
        );
        let data = image
            .data
            .as_ref()
            .expect("a captured screenshot carries its bytes");
        Self {
            pixels: data.as_chunks::<4>().0.to_vec(),
        }
    }

    /// What one named box of the frame came to.
    fn patch(&self, what: &str, min: [f32; 2], max: [f32; 2]) -> Patch {
        let [x0, y0, x1, y1] = pixels_of(min, max);
        let mut sum = [0.0_f32; 3];
        let mut lumas = Vec::with_capacity(((x1 - x0) * (y1 - y0)) as usize);
        let mut untouched = 0_usize;
        for y in y0..y1 {
            for x in x0..x1 {
                let texel = self.pixels[(y * SIDE + x) as usize];
                let linear = [
                    linear_of(texel[0]),
                    linear_of(texel[1]),
                    linear_of(texel[2]),
                ];
                for (lane, value) in sum.iter_mut().zip(linear) {
                    *lane += value;
                }
                lumas.push(luma(linear));
                if is_clear(texel) {
                    untouched += 1;
                }
            }
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "a pixel count below the frame's own, which f32 holds exactly"
        )]
        let texels = lumas.len() as f32;
        assert!(texels > 0.0, "{what} is an empty patch");
        let mean_luma = lumas.iter().sum::<f32>() / texels;
        let deviation = (lumas
            .iter()
            .map(|value| (value - mean_luma).powi(2))
            .sum::<f32>()
            / texels)
            .sqrt();
        // Against the patch's own mean, so that the number says how figured the
        // surface is and not how brightly it is lit. Brick is a dark material
        // and the plain slab is not, and an absolute deviation would make the
        // same pattern read as a tenth of the figure on one and half of it on
        // the other.
        let figure = if mean_luma > 0.0 {
            deviation / mean_luma
        } else {
            0.0
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "the same count, compared against itself"
        )]
        let clear = untouched as f32 / texels;
        Patch {
            what: what.to_owned(),
            mean: sum.map(|lane| lane / texels),
            figure,
            clear,
        }
    }
}

/// What one patch of one frame came to, in linear light.
///
/// Linear rather than the sRGB bytes it was read from, because both things
/// asked of a patch are ratios — one lane against another, one patch's
/// variation against another's — and sRGB is a curve that changes a ratio
/// depending on where on it the numbers sit.
struct Patch {
    /// What this patch is, for the report line and the failure message.
    what: String,
    /// Mean linear RGB over the patch.
    mean: [f32; 3],
    /// How *figured* the surface is: the standard deviation of linear
    /// luminance over the patch, against the patch's own mean. Scale-free on
    /// purpose, so that the number this file thresholds is a property of the
    /// surface and not of the studio's exposure. Zero for a flat surface, and
    /// exactly zero — see the module docs on why nothing here dithers.
    figure: f32,
    /// Fraction of the patch still holding the clear colour.
    clear: f32,
}

impl Patch {
    /// The brightest lane, which is what "this is lit at all" is about.
    fn peak(&self) -> f32 {
        self.mean.iter().copied().fold(0.0_f32, f32::max)
    }
}

impl std::fmt::Display for Patch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{:<22} mean ({:.4}, {:.4}, {:.4})  figure {:.4}  clear {:.1}%",
            self.what,
            self.mean[0],
            self.mean[1],
            self.mean[2],
            self.figure,
            self.clear * 100.0
        )
    }
}

/// The pixel box a world-space box in the camera's plane projects to.
///
/// Exact, because the camera is orthographic and square on that plane: the
/// view is [`VIEW`] metres across [`SIDE`] pixels, world `+Y` is up and pixel
/// rows run down. The ends are exclusive, as a range is.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a frame two hundred and fifty-six pixels wide, clamped into itself before it is cast"
)]
fn pixels_of(min: [f32; 2], max: [f32; 2]) -> [u32; 4] {
    let side = SIDE as f32;
    let across = |value: f32| ((value / VIEW + 0.5) * side).round();
    let down = |value: f32| ((0.5 - value / VIEW) * side).round();
    let clamp = |value: f32| value.clamp(0.0, side) as u32;
    // `down` flips the axis, so the box's top edge comes from its *larger* y.
    [
        clamp(across(min[0])),
        clamp(down(max[1])),
        clamp(across(max[0])),
        clamp(down(min[1])),
    ]
}

/// One sRGB code as the linear value it stands for.
fn linear_of(code: u8) -> f32 {
    let value = f32::from(code) / 255.0;
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// Relative luminance of a linear colour, in the usual Rec. 709 weights.
fn luma(linear: [f32; 3]) -> f32 {
    0.2126_f32
        .mul_add(linear[0], 0.7152 * linear[1])
        .mul_add(1.0, 0.0722 * linear[2])
}

/// Whether one pixel is still the colour the pass cleared to.
fn is_clear(texel: [u8; 4]) -> bool {
    texel
        .iter()
        .zip(CLEAR_CODES)
        .all(|(code, want)| (i16::from(*code) - i16::from(want)).abs() <= CLEAR_SLACK)
}

/// The clear colour as Bevy takes it.
fn clear() -> Color {
    Color::srgb_u8(CLEAR_CODES[0], CLEAR_CODES[1], CLEAR_CODES[2])
}

/// Whether a mean colour is Bevy's "something is missing" magenta.
///
/// Red and blue up with green far below both. A surface that came out this way
/// did not draw what it was asked to, whatever else it did. Only the compiled
/// cases ask: a baked-files material has no shader of its own to lose.
#[cfg(feature = "shader")]
fn is_magenta(mean: [f32; 3]) -> bool {
    mean[0] > 0.15 && mean[2] > 0.15 && mean[1] < 0.2 * mean[0].min(mean[2])
}

// --------------------------------------------------------------------------
// The studio
// --------------------------------------------------------------------------

/// How the one camera renders, which is the whole of what the compiled cases
/// vary.
///
/// A resource rather than a spawn function per case, because the camera is
/// otherwise the same camera every time and a second copy of it is a second
/// place for the framing to drift.
///
/// The three combinations below are the three branches of `swap_fragment`,
/// which is the function 9be70ae added and which nothing had ever drawn
/// through. Two of them look alike from the outside and are not: a camera that
/// merely *has* a `DeferredPrepass` pushes `DEFERRED_PREPASS` into the main
/// pass of every material on it, deferred or not, so a forward surface beside a
/// deferred one must still be swapped — and a surface that is genuinely
/// deferred must not.
#[derive(Resource, Clone, Copy, Default)]
struct Rig {
    /// A depth and normal prepass: the forward path's own, and the one that
    /// makes Bevy assemble the *generated prepass entry* rather than only the
    /// main-pass fragment.
    normals: bool,
    /// A deferred prepass on the camera. On its own this changes nothing about
    /// which path a material takes and everything about which shader-defs its
    /// main pass is compiled with.
    g_buffer: bool,
    /// And materials that take it: `OpaqueRendererMethod::Auto` resolves
    /// through this resource, which Bevy defaults to forward, so a camera with
    /// a `DeferredPrepass` and nothing else still draws every `Auto` material
    /// the forward way.
    deferred: bool,
}

impl Rig {
    /// No prepass at all: the plainest camera a game draws with.
    fn plain() -> Self {
        Self::default()
    }

    /// Forward, with a depth and normal prepass in front of it.
    #[cfg(feature = "shader")]
    fn prepass() -> Self {
        Self {
            normals: true,
            ..Self::default()
        }
    }

    /// Forward, on a camera that also runs a deferred prepass for somebody
    /// else's materials.
    #[cfg(feature = "shader")]
    fn beside_deferred() -> Self {
        Self {
            normals: true,
            g_buffer: true,
            deferred: false,
        }
    }

    /// Deferred, for real. No `NormalPrepass`: a deferred prepass carries the
    /// normal in its G-buffer and asking for both is asking twice.
    #[cfg(feature = "shader")]
    fn deferred() -> Self {
        Self {
            normals: false,
            g_buffer: true,
            deferred: true,
        }
    }
}

/// Where the camera draws, so that the capture can ask for the same image.
#[derive(Resource)]
struct Target(Handle<Image>);

/// The frame a `Screenshot` observer put here.
#[derive(Resource, Default)]
struct Captured(Option<Image>);

/// A headless app with one camera, one light and somewhere to draw.
///
/// The plugin set is `tools/ashlar-preview`'s reference gallery in miniature,
/// and for its reasons: no window, no winit, and no pipelined rendering —
/// without which the render world lives in another thread between frames and
/// [`Studio::pipeline_errors`] could not read its cache. Two differences from the
/// gallery, both because this is a test binary rather than a program:
/// `LogPlugin` is off, since every case here builds its own `App` in one
/// process and the logger is a global that is set once; and the frames are
/// pumped by hand with [`App::update`] rather than by a `ScheduleRunnerPlugin`,
/// which is what lets a case wait on a load state and still cap its frames.
struct Studio {
    app: App,
}

impl Studio {
    fn open(rig: Rig) -> Self {
        let mut app = App::new();
        app.insert_resource(rig)
            .insert_resource(GlobalAmbientLight {
                brightness: AMBIENT_LUX,
                ..default()
            })
            .init_resource::<Captured>();
        if rig.deferred {
            // Before `DefaultPlugins`, which only `init_resource`s this and so
            // leaves an insert alone. What it changes is what
            // `OpaqueRendererMethod::Auto` means, which is the only thing that
            // decides whether a material a camera *could* draw deferred
            // actually is.
            app.insert_resource(DefaultOpaqueRendererMethod::deferred());
        }
        app.add_plugins(
            DefaultPlugins
                .set(RenderPlugin {
                    // A pipeline that fails has to have failed by the time
                    // the case looks at the cache, and not two frames
                    // later.
                    synchronous_pipeline_compilation: true,
                    ..default()
                })
                .set(AssetPlugin {
                    file_path: assets().to_owned(),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: ExitCondition::DontExit,
                    ..default()
                })
                // Bounded, as everything in this workspace that spreads
                // work is: the machine this runs on has been seen to fall
                // over at thirty-two.
                .set(TaskPoolPlugin {
                    task_pool_options: TaskPoolOptions::with_num_threads(4),
                })
                .disable::<WinitPlugin>()
                .disable::<bevy::gilrs::GilrsPlugin>()
                .disable::<PipelinedRenderingPlugin>()
                .disable::<bevy::log::LogPlugin>(),
        );
        let target =
            app.world_mut()
                .resource_mut::<Assets<Image>>()
                .add(Image::new_target_texture(
                    SIDE,
                    SIDE,
                    TextureFormat::Rgba8UnormSrgb,
                    None,
                ));
        app.insert_resource(Target(target));
        app.add_systems(Startup, aim);
        Self { app }
    }

    /// Render until everything a spawned material names has arrived, then read
    /// the target back.
    ///
    /// The wait is a load state and not a frame count, which is the difference
    /// between this and `examples/building.rs`: a capture taken before a KTX2
    /// set arrives is a picture of the definition's constants, and a picture of
    /// the definition's constants is exactly what the textured case must not be
    /// able to pass on.
    fn draw(&mut self) -> Frame {
        // What `App::run` does before its first frame and `App::update` does
        // not. `RenderPlugin` opens the adapter in `finish`, so without this
        // there is no `RenderDevice` in either world and the first update dies
        // in `bevy_pbr`'s batching systems rather than saying there is no
        // renderer. It is here rather than in `open` because a case adds its
        // own plugins and systems to the app after it is opened, and `finish`
        // is the line after which it may not.
        self.app.finish();
        self.app.cleanup();
        let mut frames = 0;
        let mut settled = 0;
        while frames < FRAMES && settled < SETTLE {
            self.app.update();
            frames += 1;
            settled = if ready(self.app.world()) {
                settled + 1
            } else {
                0
            };
        }
        assert!(
            settled >= SETTLE,
            "nothing this scene binds had finished loading after {frames} frames"
        );
        let target = self.app.world().resource::<Target>().0.clone();
        self.app
            .world_mut()
            .spawn(Screenshot::image(target))
            .observe(
                |event: On<ScreenshotCaptured>, mut captured: ResMut<Captured>| {
                    captured.0 = Some(event.image.clone());
                },
            );
        while frames < FRAMES && self.app.world().resource::<Captured>().0.is_none() {
            self.app.update();
            frames += 1;
        }
        let captured = self
            .app
            .world()
            .resource::<Captured>()
            .0
            .clone()
            .unwrap_or_else(|| panic!("no frame came back from the target in {frames} frames"));
        println!("  drew in {frames} frames");
        Frame::of(&captured)
    }

    /// Every pipeline the render world failed to build, named and explained.
    ///
    /// The place to ask, rather than stderr: a shader Bevy will not assemble or
    /// a driver will not accept lands in `PipelineCache` as
    /// [`CachedPipelineState::Err`], and a test that grepped the log would pass
    /// on a machine whose log went somewhere else. Only the compiled cases ask,
    /// because they are the only ones that bring a shader of their own.
    #[cfg(feature = "shader")]
    fn pipeline_errors(&self) -> Vec<String> {
        let cache = self
            .app
            .sub_app(RenderApp)
            .world()
            .resource::<PipelineCache>();
        cache
            .pipelines()
            .filter_map(|pipeline| match &pipeline.state {
                CachedPipelineState::Err(error) => {
                    Some(format!("{}: {error}", label_of(&pipeline.descriptor)))
                }
                _ => None,
            })
            .collect()
    }
}

/// What a cached pipeline calls itself, or a stand-in when it says nothing.
#[cfg(feature = "shader")]
fn label_of(descriptor: &PipelineDescriptor) -> String {
    let label = match descriptor {
        PipelineDescriptor::RenderPipelineDescriptor(render) => render.label.clone(),
        PipelineDescriptor::ComputePipelineDescriptor(compute) => compute.label.clone(),
    };
    label.map_or_else(
        || "an unlabelled pipeline".to_owned(),
        |name| name.to_string(),
    )
}

/// Whether every map a spawned material names has arrived.
///
/// A map handed to `Assets<Image>` directly — which is every bound texture a
/// compiled graph has — was never loaded from anywhere and the server has no
/// state for it, so it is skipped rather than waited on. That is the same rule
/// the reference gallery follows, and it is why the compiled cases settle on
/// [`SETTLE`] frames alone.
fn ready(world: &World) -> bool {
    let server = world.resource::<AssetServer>();
    let mut all = true;
    for (_, material) in world.resource::<Assets<StandardMaterial>>().iter() {
        for handle in [
            &material.base_color_texture,
            &material.normal_map_texture,
            &material.metallic_roughness_texture,
            &material.occlusion_texture,
            &material.emissive_texture,
        ]
        .into_iter()
        .flatten()
        {
            let Some(state) = server.get_load_state(handle.id()) else {
                continue;
            };
            assert!(
                !matches!(state, bevy::asset::LoadState::Failed(_)),
                "a texture this scene binds failed to load: {state:?}"
            );
            all &= server.is_loaded_with_dependencies(handle.id());
        }
    }
    all
}

/// The one camera and the one light.
///
/// Orthographic and square on the slabs; see the module docs for why that is
/// what makes [`pixels_of`] exact. The light is off to one side rather than
/// straight down the view axis so that a normal map has something to do with
/// the picture, and shadows are off because nothing here casts one that any
/// patch is inside.
fn aim(mut commands: Commands, rig: Res<Rig>, target: Res<Target>) {
    let mut camera = commands.spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(clear()),
            ..default()
        },
        RenderTarget::Image(target.0.clone().into()),
        Projection::from(OrthographicProjection {
            scaling_mode: ScalingMode::Fixed {
                width: VIEW,
                height: VIEW,
            },
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(0.0, 0.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y),
        // Off, for two reasons that happen to agree: a deferred camera requires
        // it, and a patch measured through a resolve is a patch with a little
        // of its neighbours in it.
        Msaa::Off,
        Tonemapping::None,
        DebandDither::Disabled,
    ));
    if rig.normals {
        camera.insert(NormalPrepass);
    }
    if rig.normals || rig.g_buffer {
        // A `DeferredPrepass` is meaningless without the depth prepass beside
        // it, and a `NormalPrepass` implies one anyway.
        camera.insert(DepthPrepass);
    }
    if rig.g_buffer {
        camera.insert(DeferredPrepass);
    }
    commands.spawn((
        DirectionalLight {
            illuminance: KEY_LUX,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(-3.0, 4.0, 6.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

// --------------------------------------------------------------------------
// Where the slabs stand, and the patches over them
// --------------------------------------------------------------------------

/// The slab's extents in the `f32` the render layer deals in.
///
/// [`SLAB`] is `f64` because that is what `ashlar` authors geometry in, and
/// every use of it below is a transform or a patch box, which are not.
#[expect(
    clippy::cast_possible_truncation,
    reason = "authored extents, written as literals a couple of decimals long"
)]
fn slab() -> [f32; 3] {
    [SLAB[0] as f32, SLAB[1] as f32, SLAB[2] as f32]
}

/// Where one slab stands, given where its left edge is: the pose that centres
/// it on the camera's axis.
///
/// The `Transform` twin of [`pose_at`], for the compiled cases, which spawn a
/// mesh themselves rather than taking the transform a `Drawable` carries.
#[cfg(feature = "shader")]
fn slab_at(left: f32) -> Transform {
    let [_, height, depth] = slab();
    Transform::from_xyz(left, -height / 2.0, -depth / 2.0)
}

/// The same placement as an authored pose, which is what a building holds.
fn pose_at(left: f32) -> Pose {
    let [_, height, depth] = slab();
    Pose::at([
        f64::from(left),
        f64::from(-height / 2.0),
        f64::from(-depth / 2.0),
    ])
}

/// Where one slab's front face sits in the camera's plane, as a box in metres.
///
/// The slabs are placed by their minimum corner — `Manifold::cube` is built
/// that way and `ashlar` keeps it — so a slab placed at `x` spans `x` to
/// `x + SLAB[0]`.
fn face_of(left: f32) -> ([f32; 2], [f32; 2]) {
    let [width, height, _] = slab();
    ([left, -height / 2.0], [left + width, height / 2.0])
}

/// Where the left slab, which wears the plain material, stands.
const LEFT: f32 = -1.9;

/// Where the right slab, which wears whatever the case is about, stands.
const RIGHT: f32 = 0.1;

/// How far inside a face a patch is measured.
///
/// A patch is the face minus this on every side, so that an edge pixel — where
/// the rasteriser has part of the background in it — is never measured. Two
/// tenths of a metre is thirteen pixels at this framing.
const INSET: f32 = 0.2;

/// The box a patch of one slab's face covers.
fn patch_of(left: f32) -> ([f32; 2], [f32; 2]) {
    let (min, max) = face_of(left);
    (
        [min[0] + INSET, min[1] + INSET],
        [max[0] - INSET, max[1] - INSET],
    )
}

/// A band of frame above both slabs, which nothing in any case draws into.
///
/// The control for the whole file: if this is not the clear colour then the
/// camera is not framing what this file believes it is, and every patch below
/// is being measured somewhere other than where it was meant to be.
const SKY: ([f32; 2], [f32; 2]) = ([-1.5, 1.2], [1.5, 1.8]);

// --------------------------------------------------------------------------
// The featureless case: baked files, which is what a game compiles
// --------------------------------------------------------------------------

/// Two slabs, one per material slot.
///
/// Deliberately two *parts* rather than two elements of one, so that the piece
/// walk, the mesh deduplication and the per-binding material creation in
/// [`ashlar_bevy::drawables`] all have more than one of everything to get
/// wrong. `merged` unions the two placed elements into the building's one
/// default group, which is the other shape the same six numbers have to answer.
fn building(merged: bool) -> anyhow::Result<Building> {
    let slab = |id: &str, slot: &str| {
        Part::builder(id)
            .element(Element::new("face", Geometry::cuboid(SLAB), slot))
            .build()
    };
    let builder = Building::builder("test:studio")
        .part(slab("left", "plain")?)
        .part(slab("right", "figured")?)
        .instance(Instance::new("left", "left").placed(pose_at(LEFT)))
        .instance(Instance::new("right", "right").placed(pose_at(RIGHT)))
        .material("plain", "test:plain")
        .material("figured", "test:brick");
    let builder = if merged { builder.merged() } else { builder };
    Ok(builder.build()?)
}

/// A constant colour and a shipped KTX2 set, as the two things a game's
/// material library holds.
///
/// The brick definition is `examples/building.rs`'s, with the repeat tightened
/// to [`TILE`]: white base colour and unit roughness and metallic, because each
/// of those is already in the maps and a definition's constants multiply what a
/// map says. So a textured patch that came out flat came out flat because no
/// map arrived.
fn library() -> MaterialLibrary {
    let map = |name: &str| Some(format!("materials/library/brick/{name}"));
    let materials = [
        (
            "test:plain".to_owned(),
            MaterialDefinition {
                base_color: PLAIN,
                roughness: 1.0,
                metallic: 0.0,
                surface: Surface::Plain,
                ..MaterialDefinition::default()
            },
        ),
        (
            "test:brick".to_owned(),
            MaterialDefinition {
                base_color: [1.0, 1.0, 1.0],
                roughness: 1.0,
                metallic: 1.0,
                tile_metres: [TILE, TILE],
                surface: Surface::Files {
                    base_color: map("base.ktx2"),
                    normal: map("normal.ktx2"),
                    orm: map("orm.ktx2"),
                    height: map("height.ktx2"),
                    emissive: None,
                    baked_from: None,
                },
                ..MaterialDefinition::default()
            },
        ),
    ]
    .into_iter()
    .collect();
    MaterialLibrary { materials }
}

/// What the mesher produced and what dresses it, as one resource.
#[derive(Resource)]
struct Shell {
    meshed: ashlar::MeshedBuilding,
    library: MaterialLibrary,
}

/// The whole spawn, through the same one call a game makes.
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
            Mesh3d(drawable.mesh),
            MeshMaterial3d(drawable.material),
            drawable.transform,
        ));
    }
    Ok(())
}

/// A plain material and a textured one, drawn, and each found where it stands.
///
/// Three questions, and each one is a different way for the draw path to be
/// broken while every other test in the crate passes:
///
/// 1. **Something reached the frame at all.** A mesh uploaded without the
///    attributes the pipeline declares, a camera pointed at nothing, a material
///    whose asset never became a `RenderAsset` — all of them end here, as a
///    frame that is still the colour it was cleared to.
/// 2. **The plain patch is the plain material's colour.** Which says the join
///    put the right material on the right piece: `drawables` keys materials by
///    binding and meshes by part, element and cut batch, and a mix-up there
///    swaps two slabs without failing anything else.
/// 3. **The textured patch is figured.** Which says a *map* arrived. The
///    definition's constants are white, unit roughness and unit metallic, so a
///    slab drawn without its maps is flat — and flat is the one thing this
///    assertion does not accept.
#[test]
#[ignore = "needs a GPU: `just draw`"]
fn a_plain_and_a_textured_material_both_reach_the_frame() {
    two_slabs(false);
}

/// The same building with its two slabs merged into one group, and the same two
/// patches over them.
///
/// What the merged path has to preserve. The two slabs are now ONE group in
/// building space, drawn as two batches at the identity transform, and the same
/// two patches must come to the same colours. That is the claim that the group
/// path puts the right material on the right faces and puts them where the
/// instances stood — the two ways merging could quietly break the picture while
/// every test that reads a number instead of a pixel still passes.
///
/// The slabs stand a gap apart, so the union is two disconnected components and
/// merging adds and removes no face at either patch; only the mesh a patch's
/// triangles arrive in changes.
#[test]
#[ignore = "needs a GPU: `just draw`"]
fn a_merged_building_reaches_the_frame_the_same_way() {
    two_slabs(true);
}

/// The body both featureless cases run: one studio, one building, and one set
/// of assertions, differing only in whether the building is merged.
fn two_slabs(merged: bool) {
    let mut studio = Studio::open(Rig::plain());
    studio
        .app
        .insert_resource(Shell {
            meshed: mesh_building(
                &building(merged).expect("the studio is a valid building"),
                &ManifoldMesher::default(),
            )
            .expect("the studio meshes"),
            library: library(),
        })
        .add_systems(Startup, spawn);
    let frame = studio.draw();

    let sky = frame.patch("background", SKY.0, SKY.1);
    let (min, max) = patch_of(LEFT);
    let plain = frame.patch("plain slab", min, max);
    let (min, max) = patch_of(RIGHT);
    let figured = frame.patch("textured slab", min, max);
    let what = if merged {
        "one merged group"
    } else {
        "baked files, no features"
    };
    println!("\n{what}:\n  {sky}\n  {plain}\n  {figured}");

    assert!(
        sky.clear > 0.99,
        "the band above the slabs is not the clear colour, so the camera is not \
         framing what this file thinks it is: {sky}"
    );
    for patch in [&plain, &figured] {
        assert!(
            patch.clear < 0.01,
            "nothing was drawn over {}: {patch}",
            patch.what
        );
        assert!(
            patch.peak() > MIN_LIT,
            "{} drew something too dark to be a lit surface: {patch}",
            patch.what
        );
    }
    assert!(
        plain.mean[0] > PLAIN_RATIO * plain.mean[1] && plain.mean[0] > PLAIN_RATIO * plain.mean[2],
        "the plain slab is not its own base colour, so the wrong material was put on it: {plain}"
    );
    assert!(
        figured.figure > MIN_FIGURE,
        "the textured slab is flat, so its maps never reached the frame and what drew \
         was the definition's constants: {figured}"
    );
    assert!(
        plain.figure < MIN_FIGURE / 4.0,
        "the plain slab varies, so `figure` is measuring something other than the \
         surface and the assertion above proves nothing: {plain}"
    );
}

/// How far the plain material's red lane must stand above the other two.
///
/// A ratio rather than a colour, so the studio can be relit without this
/// moving. Five, against the seventeen the definition's own lanes are apart
/// once the ambient term is in: comfortably inside what the material says and
/// nowhere near what a wrong material, a white one or an unlit one would give.
const PLAIN_RATIO: f32 = 5.0;

/// The dimmest a lit patch's brightest lane may be, in linear light.
///
/// Between the two things it has to tell apart, with a factor of two either
/// side: on the reference machine the background's brightest lane is 0.010 and
/// the darkest surface any case draws — brick, which is a dark material seen
/// through its own occlusion map — comes to 0.100. Neither number moves with
/// the driver, because the studio has no dither, no tonemapper and no shadow in
/// it.
const MIN_LIT: f32 = 0.05;

/// How much a patch must vary for a surface to count as figured.
///
/// Measured rather than chosen, and centred between what was measured on both
/// sides. On the reference machine a figured surface comes to 0.14 for the
/// brick set over two repeats and 0.56 for the compiled graph; a flat one comes
/// to *zero* — not nearly zero, but zero, because the camera does not dither
/// and an orthographic view of a flat face is one shade. The interesting number
/// is the third: with `swap_fragment` deliberately broken so that the base
/// material draws in the graph's place, that fallback measures 0.024, which is
/// the highest reading anything unfigured has produced here. So the gap this
/// sits in runs from 0.024 to 0.14, and it is placed in the middle of it: about
/// two and a half times the loudest false reading and the same factor below the
/// quietest true one. A quarter of this again is the flatness the plain slab
/// has to stay inside, which is what says the measure means what it claims.
const MIN_FIGURE: f32 = 0.06;

/// The framing arithmetic, with no adapter in it.
///
/// Deliberately *not* `#[ignore]`d, alone in this file, so that `just ci` runs
/// it: every assertion above is a statement about a box of pixels, and a box
/// that had drifted off its slab, run off the frame or overlapped its
/// neighbour would make all of them measurements of something else while still
/// passing. This is the one thing about the studio a machine with no GPU can
/// check.
#[test]
fn the_patches_are_disjoint_and_land_where_the_slabs_stand() {
    let inside = |box_: [u32; 4], what: &str| {
        assert!(box_[0] < box_[2] && box_[1] < box_[3], "{what} is empty");
        assert!(
            box_[2] <= SIDE && box_[3] <= SIDE,
            "{what} runs off the frame: {box_:?}"
        );
    };
    let (min, max) = patch_of(LEFT);
    let plain = pixels_of(min, max);
    let (min, max) = patch_of(RIGHT);
    let figured = pixels_of(min, max);
    let sky = pixels_of(SKY.0, SKY.1);
    inside(plain, "the plain patch");
    inside(figured, "the textured patch");
    inside(sky, "the background band");

    assert!(
        plain[2] <= figured[0],
        "the two patches overlap, so neither measures one material: {plain:?} and {figured:?}"
    );
    // Each patch inside its own slab's face, and the band above both of them.
    for (left, patch, what) in [(LEFT, plain, "plain"), (RIGHT, figured, "textured")] {
        let (min, max) = face_of(left);
        let face = pixels_of(min, max);
        assert!(
            patch[0] >= face[0]
                && patch[2] <= face[2]
                && patch[1] >= face[1]
                && patch[3] <= face[3],
            "the {what} patch is not inside the {what} slab: {patch:?} against {face:?}"
        );
        assert!(
            sky[3] <= face[1],
            "the background band overlaps the {what} slab: {sky:?} against {face:?}"
        );
    }
}

// --------------------------------------------------------------------------
// The compiled cases: a graph, a driver, and a prepass
// --------------------------------------------------------------------------

/// A compiled `Surface::Shader`, drawn for real.
///
/// Behind `shader` because that is the feature the whole path lives behind, and
/// in this file rather than in `tests/shaders.rs` because that file is
/// deliberately deviceless: it checks that the right text is registered under
/// the right id and stops where a pipeline would begin.
#[cfg(feature = "shader")]
mod compiled {
    use ashlar_bevy::{
        runtime_bake::{BakeCache, Baker},
        shader::{
            GraphShaders, ProceduralMaterial, ProceduralMaterialPlugin, ShaderContext,
            create_shader_material,
        },
    };
    use ashlar_material::{
        Input, MaterialGraph, MaterialGraphLibrary, Param, PbrOutput,
        nodes::{Blend, BlendMode, Noise},
    };
    use bevy::shader::Shader;

    use super::*;

    /// Texels per repeat the static half is baked at.
    ///
    /// The smallest a bake takes. What this case is about is the *fragment*,
    /// and the bound textures under it only have to exist.
    const TEXELS: u32 = 256;

    /// Rows the static bake divides across, explicit and bounded for the reason
    /// every other bake in this workspace states.
    const THREADS: Option<std::num::NonZeroUsize> = std::num::NonZeroUsize::new(4);

    /// The graph the compiled cases draw: a static field under a live tint.
    ///
    /// The smallest thing that is really a compiled surface. `tint` is live, so
    /// the partition cannot fold it away and the multiply survives into the
    /// fragment; `grain` is static, so there is a bound texture to read and the
    /// answer *varies across the face* — which is what lets the same
    /// [`MIN_FIGURE`] that catches a missing brick map catch a fragment that
    /// never ran.
    fn graph() -> MaterialGraph {
        MaterialGraph::builder("test:draw")
            .param(Param::color("tint", [0.25, 0.8, 0.35]).live())
            .node("grain", Noise::value().period(8))
            .node(
                "surface",
                Blend::new(BlendMode::Multiply, "grain", Input::param("tint")),
            )
            .output(
                PbrOutput::new()
                    .base_color("surface")
                    .roughness("grain")
                    .height("grain")
                    .normal_strength(0.01),
            )
            .into_graph()
    }

    /// The one slab the compiled cases draw, and the graph to dress it with.
    #[derive(Resource)]
    struct Compiled {
        mesh: ashlar::TriangleMesh,
        graphs: MaterialGraphLibrary,
    }

    /// Compile the graph and spawn the slab wearing it.
    ///
    /// The mesh goes up through [`ashlar_bevy::mesh`] rather than through a
    /// Bevy primitive on purpose: a generated fragment reads the tangent frame
    /// and the cut flag the `ashlar` mesh carries, and a cuboid built by
    /// `bevy_mesh` carries neither.
    #[expect(
        clippy::too_many_arguments,
        reason = "compiling a graph takes four asset stores and two registries, and a system \
                  asks for what it takes"
    )]
    fn spawn_compiled(
        mut commands: Commands,
        compiled: Res<Compiled>,
        mut meshes: ResMut<Assets<Mesh>>,
        mut images: ResMut<Assets<Image>>,
        mut shaders: ResMut<Assets<Shader>>,
        mut registry: ResMut<GraphShaders>,
        mut cache: ResMut<BakeCache>,
        mut materials: ResMut<Assets<ProceduralMaterial>>,
    ) -> Result {
        let definition = ashlar::MaterialDefinition {
            tile_metres: [TILE, TILE],
            surface: ashlar::Surface::Shader {
                graph: "test:draw".to_owned(),
                params: std::collections::BTreeMap::new(),
            },
            ..ashlar::MaterialDefinition::default()
        };
        let material = create_shader_material(
            &definition,
            &mut ShaderContext {
                graphs: &compiled.graphs,
                cache: &mut cache,
                images: &mut images,
                shaders: &mut shaders,
                registry: &mut registry,
                resolution: Some(TEXELS),
                threads: THREADS,
                baker: Baker::Cpu,
            },
        )?;
        commands.spawn((
            Mesh3d(meshes.add(ashlar_bevy::mesh(&compiled.mesh)?)),
            MeshMaterial3d(materials.add(material)),
            slab_at(RIGHT),
        ));
        Ok(())
    }

    /// The studio with one compiled slab in it, on whichever camera.
    fn studio(rig: Rig) -> Studio {
        let mut library = MaterialGraphLibrary::default();
        library.insert(graph());
        let meshed = mesh_building(
            &Building::builder("test:compiled")
                .part(
                    Part::builder("slab")
                        .element(Element::new("face", Geometry::cuboid(SLAB), "surface"))
                        .build()
                        .expect("a slab is a valid part"),
                )
                .instance(Instance::new("slab", "slab"))
                .material("surface", "test:draw")
                .build()
                .expect("a slab is a valid building"),
            &ManifoldMesher::default(),
        )
        .expect("the slab meshes");
        let mesh = meshed.parts["slab"]
            .first()
            .expect("the slab has one batch")
            .mesh
            .clone();
        let mut studio = Studio::open(rig);
        studio
            .app
            .add_plugins(ProceduralMaterialPlugin)
            .insert_resource(Compiled {
                mesh,
                graphs: library,
            })
            .add_systems(Startup, spawn_compiled);
        studio
    }

    /// A compiled graph, drawn through the forward path with a prepass in front
    /// of it.
    ///
    /// The case that matters most in this file. Nothing else anywhere assembles
    /// the generated WGSL for real: `ashlar-material` parses its own output
    /// with `naga`, and `tests/shaders.rs` checks the text was registered, but
    /// the text is a `naga_oil` module with `#import bevy_pbr::` in it and what
    /// it is finally compiled against is Bevy's own shader defs for the
    /// pipeline being specialised. A def this crate did not expect, an import
    /// that moved, a binding at a number Bevy now uses — every one of those is
    /// a pipeline that fails here and nowhere else.
    ///
    /// The prepass is on the camera deliberately: `specialize` is called once
    /// per stage, and only a camera with a `DepthPrepass` makes Bevy ask for
    /// the *prepass* entry. Without it half the generated text would never be
    /// compiled by anything.
    #[test]
    #[ignore = "needs a GPU: `just draw`"]
    fn a_compiled_graph_draws_through_a_camera_with_a_prepass() {
        let mut studio = studio(Rig::prepass());
        let frame = studio.draw();
        let errors = studio.pipeline_errors();

        let sky = frame.patch("background", SKY.0, SKY.1);
        let (min, max) = patch_of(RIGHT);
        let surface = frame.patch("compiled slab", min, max);
        println!("\na compiled graph, forward with a prepass:\n  {sky}\n  {surface}");

        assert!(
            errors.is_empty(),
            "pipelines failed:\n{}",
            errors.join("\n")
        );
        assert!(
            sky.clear > 0.99,
            "the band above the slab is not the clear colour: {sky}"
        );
        assert!(
            surface.clear < 0.01,
            "the compiled slab did not draw: {surface}"
        );
        assert!(
            surface.peak() > MIN_LIT,
            "the compiled slab drew something too dark to be a lit surface: {surface}"
        );
        assert!(
            !is_magenta(surface.mean),
            "the compiled slab drew the colour Bevy draws when something is missing: {surface}"
        );
        assert!(
            surface.figure > MIN_FIGURE,
            "the compiled slab is flat, so the generated fragment never ran and what drew \
             was the base material's constants: {surface}"
        );
    }

    /// The same graph, still forward, on a camera that also runs a deferred
    /// prepass.
    ///
    /// The half of 9be70ae that is easy to get wrong in the other direction.
    /// `DEFERRED_PREPASS` is pushed into the **main pass** of every material
    /// drawn on a camera that has a `DeferredPrepass`, whether or not that
    /// material is deferred — so a swap that stepped aside for that def alone
    /// would stop swapping here, and a forward surface standing next to a
    /// deferred one would quietly lose its graph. Only the two defs *together*
    /// name the deferred prepass pipeline.
    ///
    /// So this asserts the same figure the plain forward case does. A picture
    /// that went flat here is the over-correction.
    #[test]
    #[ignore = "needs a GPU: `just draw`"]
    fn a_forward_graph_keeps_its_shader_beside_a_deferred_prepass() {
        let mut studio = studio(Rig::beside_deferred());
        let frame = studio.draw();
        let errors = studio.pipeline_errors();

        let (min, max) = patch_of(RIGHT);
        let surface = frame.patch("compiled slab", min, max);
        println!("\na compiled graph, forward beside a deferred prepass:\n  {surface}");

        assert!(
            errors.is_empty(),
            "pipelines failed:\n{}",
            errors.join("\n")
        );
        assert!(
            surface.clear < 0.01,
            "the compiled slab did not draw: {surface}"
        );
        assert!(
            surface.figure > MIN_FIGURE,
            "the compiled slab went flat on a camera whose main pass merely mentions the \
             deferred prepass, so the swap is standing aside for a def that names the main \
             pass too: {surface}"
        );
    }

    /// The same graph drawn deferred, which is 9be70ae's own case.
    ///
    /// The generated prepass writes a normal and a motion vector and no
    /// G-buffer, so swapping it into the deferred prepass produced a fragment
    /// with a lighting-pass id of zero — which the deferred lighting pass does
    /// not light, and the surface vanished. The swap now steps aside there and
    /// `StandardMaterial`'s own deferred fragment draws instead: plainer than
    /// the graph, and *visible*, which is the half of the trade worth keeping.
    ///
    /// So this asserts visibility and not figure. A deferred camera that learns
    /// to draw the graph itself one day should not fail here for doing better.
    #[test]
    #[ignore = "needs a GPU: `just draw`"]
    fn a_compiled_graph_is_still_visible_on_a_deferred_camera() {
        let mut studio = studio(Rig::deferred());
        let frame = studio.draw();
        let errors = studio.pipeline_errors();

        let sky = frame.patch("background", SKY.0, SKY.1);
        let (min, max) = patch_of(RIGHT);
        let surface = frame.patch("compiled slab", min, max);
        println!("\na compiled graph, deferred:\n  {sky}\n  {surface}");

        assert!(
            errors.is_empty(),
            "pipelines failed:\n{}",
            errors.join("\n")
        );
        assert!(
            sky.clear > 0.99,
            "the band above the slab is not the clear colour, so the deferred lighting pass \
             wrote where nothing stands: {sky}"
        );
        assert!(
            surface.clear < 0.01,
            "the compiled slab vanished on a deferred camera, which is the bug 9be70ae \
             fixed: {surface}"
        );
        assert!(
            surface.peak() > MIN_LIT,
            "the compiled slab drew something too dark to be a lit surface: {surface}"
        );
        assert!(
            !is_magenta(surface.mean),
            "the compiled slab drew the colour Bevy draws when something is missing: {surface}"
        );
    }
}

/// The baked lawn the strand case grows, as an asset key under [`assets`].
///
/// The default library's grass, written by the content step, because the
/// point of the case is that a *shipped* file draws.
#[cfg(feature = "strands")]
const LAWN: &str = "materials/library/grass/set.strands";

/// How much of the shipped lawn the strand case grows.
///
/// A third, which is the `StrandSettings::density` cut applied at full detail.
/// The whole set is a quarter of a million strands over a two-metre repeat and
/// this case wants a picture rather than a benchmark.
#[cfg(feature = "strands")]
const LAWN_DENSITY: f32 = 0.35;

/// How many times life size the lawn is drawn.
///
/// A blade of `library:grass` is forty millimetres, which at this studio's
/// [`VIEW`] over [`SIDE`] pixels is two and a half pixels: a frame in which
/// "the blades reached above the ground" and "the ground is one pixel taller
/// than this file thinks" are the same measurement. Twenty times is eight
/// hundred millimetres, which is fifty pixels, and it is exactly what an
/// orthographic camera standing twenty times closer would see — the lawn is not
/// distorted by it, only magnified.
#[cfg(feature = "strands")]
const LAWN_ZOOM: f32 = 20.0;

/// The strip of ground the lawn grows on, in metres: across, and *deep*.
///
/// Narrow in depth on purpose. The camera is orthographic and square on, so
/// every blade in the strip's depth projects into the same band of pixels; a
/// square metre of lawn would stack a hundred rows of blades into that band and
/// draw a green rectangle, which is a picture that cannot tell grass from
/// paint. A hand's breadth of depth leaves the band with blades in it and sky
/// between them, which is what [`Patch::figure`] then has something to measure.
#[cfg(feature = "strands")]
const LAWN_STRIP: [f64; 2] = [2.0, 0.12];

/// The canopy band, in metres after [`LAWN_ZOOM`].
///
/// Well clear of the ground, which is at zero, so everything in here was drawn
/// by something *standing up* off it. This is where the silhouette lives:
/// blades with background between them.
#[cfg(feature = "strands")]
const CANOPY: ([f32; 2], [f32; 2]) = ([-1.2, 0.20], [1.2, 0.60]);

/// The band the roots are in, where a lawn is at its densest.
///
/// The colour claim is measured here rather than in the canopy, and the reason
/// is arithmetic rather than taste: the canopy is four fifths background, and
/// this studio's clear colour is itself green over red by more than three, so a
/// ratio taken up there would be mostly a measurement of the sky.
#[cfg(feature = "strands")]
const ROOTS: ([f32; 2], [f32; 2]) = ([-1.2, 0.02], [1.2, 0.14]);

/// The band the ground itself fills, which is the control for the one above:
/// the lawn is lit and reaches the frame at all.
#[cfg(feature = "strands")]
const BED: ([f32; 2], [f32; 2]) = ([-1.2, -0.34], [1.2, -0.12]);

/// Where the lawn does not reach: a blade is forty millimetres and this is a
/// metre and a half above the bed.
#[cfg(feature = "strands")]
const OVER: ([f32; 2], [f32; 2]) = ([-1.2, 1.4], [1.2, 1.9]);

/// One strip of ground, as a mesh a strand set is planted on.
///
/// Built by hand rather than meshed from a building, because that is what the
/// case is about: a game holds triangles from somewhere — a heightfield, a
/// terrain streamer, a mesh off disk — and grows a baked lawn on them without
/// any of `ashlar`'s geometry half being involved. The UVs are in **metres**,
/// which is what `ashlar` measures them in; `StrandSurface` divides them by the
/// definition's own `tile_metres`.
#[cfg(feature = "strands")]
fn lawn_ground() -> ashlar::TriangleMesh {
    use ashlar::glam::DVec3;

    let [across, deep] = LAWN_STRIP;
    let corner = |x: f64, z: f64| DVec3::new(x, 0.0, z);
    let mut surface = ashlar::TriangleMesh::default();
    for (a, b, c) in [
        ((0.0, 0.0), (across, 0.0), (across, deep)),
        ((0.0, 0.0), (across, deep), (0.0, deep)),
    ] {
        surface.push_triangle(
            [
                corner(a.0 - across * 0.5, a.1),
                corner(b.0 - across * 0.5, b.1),
                corner(c.0 - across * 0.5, c.1),
            ],
            [DVec3::Y; 3],
            [[a.0, a.1], [b.0, b.1], [c.0, c.1]],
            ashlar::FaceSource::BODY,
        );
    }
    surface
}

/// The definition the lawn is grown from: no graph anywhere in it.
///
/// A `Plain` surface, which is the strongest form of the claim — there is no
/// material graph named, no `baked_from` to recover one through, and no texture
/// set either. What the definition says about strands is which layers, how many
/// of them, and the file they are in.
#[cfg(feature = "strands")]
fn lawn_definition() -> MaterialDefinition {
    MaterialDefinition {
        // The repeat `benchmark:grass` was drawn for. The set's roots are in UV
        // over one repeat, so this is what turns them into metres of strip.
        tile_metres: [2.0, 2.0],
        strands: Some(
            ashlar::StrandSettings::new(["blades", "fibres", "stragglers"])
                .density(LAWN_DENSITY)
                .cast_shadows(false)
                .baked_set(LAWN),
        ),
        ..MaterialDefinition::default()
    }
}

/// The bytes of the shipped lawn, read once and handed to the spawn system.
#[cfg(feature = "strands")]
#[derive(Resource)]
struct Lawn(Vec<u8>);

/// Grow the baked lawn on the strip, and stand a bed under it.
///
/// The whole of what a game does, and every line of it is the game half: read
/// the file, plant it, upload the chunks. No `ashlar-material` is linked into
/// this build's library at all — the test binary has it as a dev-dependency for
/// the *other* cases, and nothing here touches it.
#[cfg(feature = "strands")]
#[expect(
    clippy::cast_possible_truncation,
    reason = "the strip's width is a small authored number, clamped before the cast"
)]
fn grow_lawn(
    mut commands: Commands,
    lawn: Res<Lawn>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    use ashlar_bevy::strands::{StrandContext, StrandSets, create_strands, strand_material};

    let definition = lawn_definition();
    let sets = StrandSets::read(LAWN, &lawn.0).expect("the shipped lawn reads");
    let mut cards = ashlar_bevy::cards::CardCache::new();
    let mut cx = StrandContext::new(&mut images, &mut cards);
    let grown = create_strands(&definition, &lawn_ground(), &sets, &mut cx)
        .expect("the shipped lawn grows on the strip");

    // The bed, so that the band under the blades is a lit surface rather than
    // the clear colour: a dark earth slab whose top face is exactly the plane
    // the roots were planted on.
    let bed = materials.add(StandardMaterial {
        base_color: Color::linear_rgb(0.06, 0.045, 0.03),
        perceptual_roughness: 1.0,
        ..default()
    });
    // Wide enough to fill the frame and no deeper than the strip it stands
    // under. The depth is the part that matters: the camera is eight metres
    // out along `+z`, so a bed as deep as it is wide would reach past it and
    // the frame would be the inside of a box.
    // The strip's own width, in the `f32` the render layer deals in: the
    // `f64`-to-`f32` boundary is here as it is everywhere else in this stack.
    let across = LAWN_STRIP[0].abs().min(f64::from(f32::MAX)) as f32;
    let deep = LAWN_STRIP[1].abs().min(f64::from(f32::MAX)) as f32;
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(across * LAWN_ZOOM * 4.0, 0.4, deep * LAWN_ZOOM))),
        MeshMaterial3d(bed),
        Transform::from_xyz(0.0, -0.2, 0.0),
    ));

    let mut strands = 0;
    let mut triangles = 0;
    for layer in grown {
        let material = materials.add(strand_material(&definition, layer.roughness));
        strands += layer.strands;
        triangles += layer.triangles;
        // Level zero only. The definition names no `lod_metres`, so that is the
        // one level there is, and it is drawn at every distance.
        for level in &layer.levels {
            for chunk in &level.chunks {
                commands.spawn((
                    Mesh3d(meshes.add(chunk.mesh.clone())),
                    MeshMaterial3d(material.clone()),
                    // The chunk's mesh is written relative to its own origin,
                    // so the origin goes in the transform — and the zoom goes
                    // on both, which is what makes it a camera move rather than
                    // a distortion.
                    Transform::from_translation(Vec3::from_array(chunk.origin) * LAWN_ZOOM)
                        .with_scale(Vec3::splat(LAWN_ZOOM)),
                ));
            }
        }
    }
    println!("  grew {strands} strands, {triangles} triangles from {LAWN}");
    assert!(strands > 0, "the shipped lawn planted nothing on the strip");
}

/// A lawn read off disk, grown on a bare strip of ground, reaching the frame.
///
/// The claim of the whole 2026-09-20 change, drawn: a build with **no graph
/// engine in it** takes a file, plants it on triangles it made itself, and what
/// comes back is grass standing above the ground it grows on.
///
/// Four questions, and each one is a different way for that to be broken while
/// the rest of the suite passes:
///
/// 1. **The band above the ground is no longer background.** Which is the
///    whole claim: a set that failed to read, a placement that planted nothing,
///    or chunks that never reached the render world all end here as a frame
///    that is still the colour it was cleared to. Setting
///    [`LAWN_DENSITY`] to zero is what this looks like when it fails, and it
///    fails only this assertion.
/// 2. **The band is not a solid wall either.** A lawn seen side on is blades
///    with sky between them. A strip this narrow cannot stack enough rows to
///    fill the band, so a band that *is* full is a green rectangle — which is
///    what a card, a bad atlas or a mesh with its widths blown up would draw.
/// 3. **The band is figured.** The same thing said the other way and the
///    harder one to fake: a field of thin lit blades against a dark background
///    has a large luminance deviation, and a flat surface has none.
/// 4. **It is green, and the green came out of the file.** A strand carries its
///    own root-to-tip colour as a vertex attribute and [`strand_material`]
///    binds white, so the only place that colour can have come from is the
///    twenty-three floats a strand was read back as.
#[cfg(feature = "strands")]
#[test]
#[ignore = "needs a GPU: `just draw`"]
fn a_baked_lawn_grows_on_a_bare_strip_and_stands_above_it() {
    let bytes = std::fs::read(std::path::Path::new(assets()).join(LAWN))
        .expect("the content step wrote the lawn");
    let mut studio = Studio::open(Rig::plain());
    studio
        .app
        .insert_resource(Lawn(bytes))
        .add_systems(Startup, grow_lawn);
    let frame = studio.draw();

    let over = frame.patch("above the lawn", OVER.0, OVER.1);
    let canopy = frame.patch("the canopy", CANOPY.0, CANOPY.1);
    let roots = frame.patch("down at the roots", ROOTS.0, ROOTS.1);
    let bed = frame.patch("the bed under them", BED.0, BED.1);
    println!("\nbaked strands, no graph engine:\n  {over}\n  {canopy}\n  {roots}\n  {bed}");

    assert!(
        over.clear > 0.99,
        "something drew a metre and a half above a forty-millimetre lawn, so the camera is not \
         framing what this file thinks it is: {over}"
    );
    assert!(
        bed.clear < 0.01,
        "the bed under the lawn did not draw, so the band above it is being measured against \
         nothing: {bed}"
    );
    assert!(
        canopy.clear < MOST_SKY,
        "the band above the ground is still background: nothing grew out of the baked set. \
         {canopy}"
    );
    assert!(
        canopy.clear > LEAST_SKY,
        "the band above the ground is solid, so what drew is a green rectangle rather than a \
         field of blades: {canopy}"
    );
    assert!(
        canopy.figure > MIN_FIGURE,
        "the band above the ground is flat, so what drew is not blades: {canopy}"
    );
    assert!(
        roots.clear < MOST_ROOT_SKY,
        "the lawn is as thin at the roots as it is at the tips, so what grew is not a lawn: \
         {roots}"
    );
    assert!(
        roots.mean[1] > LAWN_GREEN * roots.mean[0] && roots.mean[1] > LAWN_GREEN * roots.mean[2],
        "the blades are not green, and a strand's colour is a vertex attribute the file \
         carried — so the twenty-three floats did not come back: {roots}"
    );
}

/// The most of the canopy band that may still be background.
///
/// Measured rather than chosen, and the gap it sits in is narrow in absolute
/// terms and absolute in every other sense. The shipped lawn on this strip
/// leaves 92.2 % of the canopy band untouched, because a canopy seen side on is
/// mostly sky; a band with no lawn in it leaves **exactly** 100 %, not nearly —
/// the studio does not dither and the clear colour is the clear colour. So this
/// sits between the two at 98 %, and what it separates is "some grass reached
/// up here" from "nothing did".
#[cfg(feature = "strands")]
const MOST_SKY: f32 = 0.98;

/// The least of it that must still be background.
///
/// Below this the band is filled rather than figured, which is what a card or a
/// mesh with its widths blown up would draw.
#[cfg(feature = "strands")]
const LEAST_SKY: f32 = 0.05;

/// The most of the *root* band that may still be background.
///
/// A lawn is a canopy: what is at the bottom of it is nearly solid and what is
/// at the top is a scatter of tips, and a root band as empty as the canopy is a
/// handful of blades rather than a lawn. The shipped lawn measures 47.6 % here
/// against the canopy's 92.2 %, so this is placed well clear of the first and
/// well clear of the second.
#[cfg(feature = "strands")]
const MOST_ROOT_SKY: f32 = 0.75;

/// How far a lit blade's green lane must stand above its red and blue.
///
/// A ratio rather than a colour, so the studio can be relit without this
/// moving, and measured on both sides. The shipped lawn comes to 1.23 over red
/// in the root band and 3.2 over blue; a white or grey lawn in the same band
/// would come to about 1.0 over both, because a white key light moves every
/// lane by the same factor and the only thing left tilting the mean is the
/// background. Red is the binding one and this sits halfway between the two
/// readings.
///
/// What it says, once it holds, is that the *colour came out of the file*: a
/// strand carries its own root-to-tip gradient as a vertex attribute,
/// [`strand_material`](ashlar_bevy::strands::strand_material) binds white, and
/// there is nowhere else for green to have come from.
#[cfg(feature = "strands")]
const LAWN_GREEN: f32 = 1.12;
