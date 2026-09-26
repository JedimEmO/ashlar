//! The contract between the two backends, on a real GPU.
//!
//! `ashlar-material` lowers a graph once and runs it twice: the interpreter in
//! [`ashlar_material::interp`] is the reference, and [`ashlar_material::wgsl`]
//! prints the same IR as WGSL. Nothing but this file says the two agree. Every
//! other test in the workspace checks one side — that the emitted text parses,
//! that the bake tiles, that the partition cuts where it says it does — and a
//! backend can pass all of them while quietly computing a different function.
//!
//! So: for every primitive [`Op`] the vocabulary can reach, a partition whose
//! runtime side is that op over a bound texture and a live parameter; for every
//! node whose arithmetic is newer than the study graphs that would otherwise
//! reach it, a probe computed entirely in the fragment; for every study graph
//! with every parameter live; and for the lattice hash, which is the one thing
//! here that has to be bit-exact rather than close. Each is
//! dispatched as the emitted compute kernel over the texel centres of a
//! [`RESOLUTION`] square and compared with the interpreter over the same
//! partition, texel for texel.
//!
//! **This needs a GPU**, so every test that dispatches one is `#[ignore]`d and
//! `just conformance` is what runs them. A machine with no adapter fails at
//! [`Gpu::open`] with Bevy's own "unable to find a GPU" message rather than
//! silently passing. The exceptions are the two tests that only partition, both
//! of which run in `just ci`:
//! [`the_op_cases_between_them_reach_every_op_the_two_backends_implement`], so
//! that an op with no case fails on the branch that added the op, and
//! [`every_node_case_is_computed_in_the_fragment_rather_than_read_back`], so
//! that a probe which quietly stopped being a probe fails there too.
//!
//! What is deliberately *not* under test: the bake's quantisation. The bound
//! textures are uploaded as `Rgba32Float` holding the CPU planes' own numbers,
//! not as the eight-bit images a shipped material binds, because a difference
//! of half an eight-bit code between a `Rgba8Unorm` image and an `f32` plane
//! would swamp the thing this file is about. `tests/shaders.rs` is where the
//! upload formats are checked, and `tests/bake.rs` where the encoding is.
//!
//! The tolerances, and why each is what it is, are on [`COLOUR_STEP`],
//! [`NORMAL_STEP`] and [`RAW_TOLERANCE`]. Every case prints its own worst
//! difference, so a change that moves the backends apart shows up as a number
//! long before it shows up as a failure; run with `--nocapture` to read them.
#![allow(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "fixtures built in the test, and a hash this file requires to be bit-exact"
)]

use std::{collections::BTreeSet, fmt::Write as _, num::NonZeroUsize};

use ashlar_material::{
    BrickOutput, Channel, Input, Material, MaterialGraph, MaterialGraphBuilder,
    MaterialGraphLibrary, MathOp, Param, ParamValue, PbrOutput, SdfOp, ShapeKind, ShapeOutput,
    WeaveOutput, WeavePattern,
    interp::{Inputs, Interpreter, Plane, hash2, hash3},
    ir::{Filter, Op, Target},
    nodes::{
        Blur, Bricks, CircleMap, CircleSplatter, Clamp, Combine, CutFlag, Decompose,
        DirectionalWarp, HeightToMask, Math, Mix, Noise, SdfCombine, SdfMask, Shape, Switch, Time,
        Triplanar, Warp, Weave, WorldMask, WorldNormal, WorldPos,
    },
    partition::{BoundTexture, Partition, partition},
    planes::{BakeCache, rasterise_buffers},
    wgsl::{self, ComputeShader, HASH, MeshInputs},
};
use bevy::{
    platform::future::block_on,
    render::{
        render_resource::{
            AddressMode, BindGroupEntry, BindGroupLayoutEntry, BindingResource, BindingType,
            Buffer, BufferBindingType, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
            ComputePassDescriptor, Extent3d, FilterMode, MapMode, MipmapFilterMode, Origin3d,
            PipelineCompilationOptions, PipelineLayoutDescriptor, PollType,
            RawComputePipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor,
            ShaderModule, ShaderModuleDescriptor, ShaderSource, ShaderStages,
            TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect, TextureDescriptor,
            TextureDimension, TextureFormat, TextureSampleType, TextureUsages, TextureView,
            TextureViewDescriptor, TextureViewDimension,
        },
        renderer::{RenderDevice, RenderQueue},
        settings::{Backends, WgpuSettings},
    },
};

/// Texels per side every case is compared at.
///
/// The smallest resolution a bake takes. Sixty-five thousand texels of every
/// op is already far more than a disagreement needs to show in, and the CPU
/// reference is the slow half.
const RESOLUTION: u32 = 256;

/// Texels per side the shipped graphs are compared at: the finest lattice the
/// default library lays, which is the runtime bake's own ceiling.
const SHIPPED_RESOLUTION: u32 = 512;

/// Threads the CPU reference rasterises with.
///
/// Explicit and bounded, as everything in this workspace that rasterises is:
/// the bake has been seen to fall over at thirty-two on this machine.
const THREADS: usize = 8;

/// What a colour channel is allowed to differ by: one eight-bit code.
///
/// The number the design names, and the one that matters, because every colour
/// the two backends compute ends up in an eight-bit image or an eight-bit
/// swapchain: a difference smaller than this cannot be seen and cannot be
/// stored.
const COLOUR_STEP: f32 = 1.0 / 255.0;

/// What a normal lane is allowed to differ by: two eight-bit codes.
///
/// Twice the colour budget, because a normal is derived rather than computed —
/// four taps of an expression, a difference, and a normalisation — so an error
/// in the height it came from reaches it multiplied.
const NORMAL_STEP: f32 = 2.0 / 255.0;

/// What a raw scalar plane is allowed to differ by: one part in ten thousand.
///
/// A roughness, a height, a metallic or an occlusion is not a colour: it feeds
/// lighting or parallax, where a difference is amplified rather than quantised
/// away, so an eight-bit step is the wrong budget for one and this is a
/// fortieth of it.
///
/// Why not tighter still: the worst any case here shows on the reference
/// machine is eight parts in a million — a splatter, whose per-instance turn is
/// a sine and a cosine each of which is paid for once per instance — and every
/// one of those comes from the two places the two backends are *allowed* to
/// differ — `sin` and `cos`, whose accuracy WGSL states in ULP and each vendor
/// meets differently, and the hardware's bilinear weights, which the
/// specification only requires to carry eight fractional bits of subtexel
/// precision where the CPU's [`Plane::sample`] carries all of them. More than
/// ten times that is headroom
/// for another vendor, and still three orders of magnitude inside anything
/// anyone can see. The observed maxima are in the crate's README, so a run that
/// lands near this number is a run that found something.
const RAW_TOLERANCE: f32 = 1.0e-4;

// --------------------------------------------------------------------------
// The device
// --------------------------------------------------------------------------

/// A headless `wgpu` device, and the queue that feeds it.
///
/// Through [`initialize_renderer`](bevy::render::renderer::initialize_renderer)
/// rather than through `wgpu` directly, and in this crate rather than in
/// `ashlar-material`, for one reason each. This crate already links `wgpu` —
/// every version of it that `cargo deny` would see is the one Bevy pins — so
/// the conformance test adds no dependency at all, where the same test in
/// `ashlar-material` would add `wgpu` to a crate whose whole point is that it
/// has neither Bevy nor a GPU in it. And `RenderDevice` is the handle the rest
/// of this crate deals in, so a failure here is a failure in the same terms the
/// adapter reports one.
///
/// No `App`: `RenderPlugin` would want a window, a schedule and a render world
/// to hang this off, and none of them has anything to do with dispatching one
/// kernel.
struct Gpu {
    device: RenderDevice,
    queue: RenderQueue,
    /// What the adapter calls itself, so a reported difference can be read
    /// against the driver that produced it.
    adapter: String,
}

impl Gpu {
    /// Open the default adapter, or fail saying so.
    fn open() -> Self {
        let settings = WgpuSettings::default();
        let backends = settings.backends.unwrap_or(Backends::all());
        let resources = block_on(bevy::render::renderer::initialize_renderer(
            backends, None, &settings,
        ));
        let info = &*resources.2;
        let adapter = format!("{} ({:?}, {})", info.name, info.backend, info.driver_info);
        Self {
            device: resources.0,
            queue: resources.1,
            adapter,
        }
    }

    /// Compile one module, or fail with the text that would not compile.
    ///
    /// `create_and_validate_shader_module` is `wgpu`'s own front end, which is
    /// the same `naga` the crate's tests parse with — but here it goes on to
    /// the driver, which is the part no CPU test reaches. A module the front
    /// end refuses reaches `wgpu`'s uncaptured-error handler, which panics with
    /// the diagnostic; nothing here installs a handler that would swallow one.
    fn module(&self, source: &str, what: &str) -> ShaderModule {
        self.device
            .create_and_validate_shader_module(ShaderModuleDescriptor {
                label: Some(what),
                source: ShaderSource::Wgsl(source.into()),
            })
    }
}

// --------------------------------------------------------------------------
// Dispatching one kernel
// --------------------------------------------------------------------------

/// One bound image, as the lanes the kernel will read out of it.
///
/// The same packing [`shader`](ashlar_bevy::shader) does when it uploads a
/// compiled material's static half — several scalar planes may share one RGBA
/// image, each owning the lane its binding names — minus the quantisation,
/// because the bake's encoding is not what is under test here. The one thing
/// that *is* kept is the normal encoding: the emitter writes `v * 2 - 1` for a
/// [`Filter::Normal`] plane in a unorm image, so such a plane is written half
/// and half about zero here too, or the shader would undo an encoding nobody
/// applied.
struct Upload {
    resolution: u32,
    /// Level 0 alone for every case but the one that reads a level further
    /// down, which is the only way a dispatch can be asked what a compiled
    /// material looks like from a distance.
    levels: Vec<Vec<[f32; 4]>>,
}

/// The lanes of one image at level 0, and whether it is a normal map.
fn lanes_of(
    binding: &wgsl::TextureBinding,
    textures: &[BoundTexture],
    planes: &[Plane],
) -> (u32, Vec<[f32; 4]>, bool) {
    let mut resolution = 0;
    let mut lanes: Vec<[f32; 4]> = Vec::new();
    let mut normal = false;
    for buffer in &binding.buffers {
        let texture = &textures[buffer.index()];
        let plane = &planes[buffer.index()];
        if lanes.is_empty() {
            resolution = plane.resolution();
            lanes = vec![[0.0, 0.0, 0.0, 1.0]; plane.texels()];
        }
        assert_eq!(
            plane.resolution(),
            resolution,
            "image {} packs planes of two sizes",
            binding.image
        );
        normal |= matches!(texture.filter, Filter::Normal { .. });
        let width = texture.value_type.components();
        let first = usize::from(texture.lane);
        for (index, texel) in lanes.iter_mut().enumerate() {
            let value = plane.texel_at(index);
            for (lane, component) in value.iter().enumerate().take(width) {
                texel[first + lane] = *component;
            }
        }
    }
    assert!(!lanes.is_empty(), "image {} holds no plane", binding.image);
    (resolution, lanes, normal)
}

/// Write a normal map's three lanes half and half about zero, as the encoder
/// does and as the shader's decode expects. The fourth lane is the footprint's
/// coherence and runs `0..=1` already, so it is left alone.
fn half_and_half(lanes: &mut [[f32; 4]]) {
    for texel in lanes {
        for lane in texel.iter_mut().take(3) {
            *lane = lane.mul_add(0.5, 0.5);
        }
    }
}

fn upload_of(
    binding: &wgsl::TextureBinding,
    textures: &[BoundTexture],
    planes: &[Plane],
) -> Upload {
    let (resolution, mut lanes, normal) = lanes_of(binding, textures, planes);
    if normal && encoded(binding.format) {
        half_and_half(&mut lanes);
    }
    Upload {
        resolution,
        levels: vec![lanes],
    }
}

/// The same image with its whole chain, built the way a compiled material's is.
///
/// [`ashlar_material::bake::bound_mips`] rather than a box filter written here,
/// and deliberately: what the level-3 case measures is whether the *shipped*
/// chain — the renormalised normals and the coherence in their alpha — widens a
/// roughness into the one the bake wrote, so the chain under test has to be the
/// one a material actually binds. The quantisation is still left out, as it is
/// everywhere else in this file: the levels go up as `Rgba32Float` holding the
/// numbers, so what is left between the two sides is the arithmetic.
fn upload_chain_of(
    binding: &wgsl::TextureBinding,
    textures: &[BoundTexture],
    planes: &[Plane],
) -> Upload {
    let (resolution, lanes, normal) = lanes_of(binding, textures, planes);
    let mut levels = ashlar_material::bake::bound_mips(&lanes, resolution, normal);
    if normal && encoded(binding.format) {
        for level in &mut levels {
            half_and_half(level);
        }
    }
    Upload { resolution, levels }
}

/// Texels to a side of one block of the cut-flag pattern; see [`MeshPlanes`].
const CUT_BLOCK: usize = 32;

/// A world position, a world normal and a cut flag per texel, for the cases
/// that read one.
///
/// A field over the mesh is only itself where the mesh varies under it: a
/// triplanar read at one point is one sample of its source repeated across the
/// square, a cut flag of one everywhere is a surface with no boundary on it,
/// and two backends that disagreed everywhere would still compare equal on
/// either. So the harness lays a mesh over the same square it compares — a
/// position that sweeps eight metres by six and climbs, a normal that turns
/// through a whole circle so that every face of a triplanar is the near one
/// somewhere, and a flag in [`CUT_BLOCK`]-texel blocks so both answers and the
/// boundary between them are in the frame — and feeds the *same numbers* to
/// both sides: [`wgsl::MeshInputs::Planes`] binds them as three storage
/// buffers, and the CPU reference reads them out of this.
///
/// The flag is blocks rather than a gradient because that is what it is on a
/// mesh: the weld splits the vertices either side of a cut, so every triangle's
/// three corners agree and a fragment reads exactly one or exactly zero. A
/// pattern that ramped between them would be testing an interpolation neither
/// backend performs.
///
/// Computed here rather than in the kernel on purpose. A mesh laid down by an
/// expression would put that expression under test as well, and what is under
/// test is the graph.
struct MeshPlanes {
    position: Vec<[f32; 3]>,
    normal: Vec<[f32; 3]>,
    cut: Vec<[f32; 3]>,
}

impl MeshPlanes {
    fn over(resolution: u32) -> Self {
        let side = resolution as usize;
        let mut position = Vec::with_capacity(side * side);
        let mut normal = Vec::with_capacity(side * side);
        let mut cut = Vec::with_capacity(side * side);
        for y in 0..side {
            for x in 0..side {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "a texel index below the resolution, which f32 holds exactly"
                )]
                let (u, v) = (
                    (x as f32 + 0.5) / resolution as f32,
                    (y as f32 + 0.5) / resolution as f32,
                );
                position.push([
                    (u - 0.5) * 8.0,
                    (v - 0.5) * 6.0 + 1.5,
                    (u + v).mul_add(3.0, -3.0),
                ]);
                let turn = std::f32::consts::TAU;
                let facing = [(turn * u).cos(), (turn * v).sin(), 0.5];
                let length = facing
                    .iter()
                    .fold(0.0_f32, |sum, lane| lane.mul_add(*lane, sum))
                    .sqrt();
                normal.push(facing.map(|lane| lane / length));
                let cut_here = (x / CUT_BLOCK + y / CUT_BLOCK).is_multiple_of(2);
                cut.push([f32::from(u8::from(cut_here)), 0.0, 0.0]);
            }
        }
        Self {
            position,
            normal,
            cut,
        }
    }

    /// The position, normal and cut flag at one texel, in the row-major order
    /// the kernel indexes and the reference walks.
    fn at(&self, texel: usize) -> ([f32; 3], [f32; 3], f32) {
        (self.position[texel], self.normal[texel], self.cut[texel][0])
    }

    /// One plane as the `vec4<f32>` bytes a storage buffer holds.
    fn bytes(lanes: &[[f32; 3]]) -> Vec<u8> {
        lanes
            .iter()
            .flat_map(|texel| {
                [texel[0], texel[1], texel[2], 0.0]
                    .into_iter()
                    .flat_map(f32::to_le_bytes)
            })
            .collect()
    }
}

/// Whether a format is one the emitter believes a normal was encoded into.
fn encoded(format: ashlar_material::bake::PlaneFormat) -> bool {
    use ashlar_material::bake::PlaneFormat::{R16Unorm, Rgba8Srgb, Rgba8Unorm};
    matches!(format, Rgba8Unorm | Rgba8Srgb | R16Unorm)
}

/// The bind group layout the generated module declares.
///
/// The output at zero, the runtime inputs and the parameter block just below
/// the textures, and one texture-and-sampler pair per bound image from a
/// hundred and one up — the same numbers [`wgsl`] names as constants, read off
/// the emitted kernel rather than written down twice.
fn layout_entries(kernel: &ComputeShader) -> Vec<BindGroupLayoutEntry> {
    let buffer = |binding: u32, ty: BufferBindingType| BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::COMPUTE,
        ty: BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let mut entries = vec![
        buffer(
            wgsl::OUTPUT_BINDING,
            BufferBindingType::Storage { read_only: false },
        ),
        buffer(wgsl::INPUTS_BINDING, BufferBindingType::Uniform),
        buffer(wgsl::PARAMS_BINDING, BufferBindingType::Uniform),
    ];
    if kernel.mesh() == MeshInputs::Planes {
        for binding in [
            wgsl::WORLD_POS_BINDING,
            wgsl::WORLD_NORMAL_BINDING,
            wgsl::CUT_FLAG_BINDING,
        ] {
            entries.push(buffer(
                binding,
                BufferBindingType::Storage { read_only: true },
            ));
        }
    }
    for binding in kernel.bindings() {
        entries.push(BindGroupLayoutEntry {
            binding: binding.texture,
            visibility: ShaderStages::COMPUTE,
            ty: BindingType::Texture {
                sample_type: TextureSampleType::Float { filterable: true },
                view_dimension: TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        });
        entries.push(BindGroupLayoutEntry {
            binding: binding.sampler,
            visibility: ShaderStages::COMPUTE,
            ty: BindingType::Sampler(SamplerBindingType::Filtering),
            count: None,
        });
    }
    entries
}

/// The bound planes as `Rgba32Float` textures, in binding order.
///
/// Full precision on purpose: the only thing left between the CPU's
/// [`Plane::sample`] and the GPU's `textureSampleLevel` is then the hardware's
/// bilinear weights, which is a difference worth measuring, where the bake's
/// eight-bit quantisation is one that would hide it.
fn upload_textures(gpu: &Gpu, kernel: &ComputeShader, uploads: &[Upload]) -> Vec<TextureView> {
    let mut views = Vec::with_capacity(uploads.len());
    for (binding, upload) in kernel.bindings().iter().zip(uploads) {
        let size = Extent3d {
            width: upload.resolution,
            height: upload.resolution,
            depth_or_array_layers: 1,
        };
        let texture = gpu.device.create_texture(&TextureDescriptor {
            label: Some(&binding.texture_name),
            size,
            mip_level_count: u32::try_from(upload.levels.len()).unwrap_or(1),
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba32Float,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut edge = upload.resolution;
        for (level, lanes) in upload.levels.iter().enumerate() {
            let bytes: Vec<u8> = lanes
                .iter()
                .flat_map(|texel| texel.iter().flat_map(|lane| lane.to_le_bytes()))
                .collect();
            gpu.queue.write_texture(
                TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: u32::try_from(level).unwrap_or(0),
                    origin: Origin3d::ZERO,
                    aspect: TextureAspect::All,
                },
                &bytes,
                TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(edge * 16),
                    rows_per_image: Some(edge),
                },
                Extent3d {
                    width: edge,
                    height: edge,
                    depth_or_array_layers: 1,
                },
            );
            edge = (edge / 2).max(1);
        }
        views.push(texture.create_view(&TextureViewDescriptor::default()));
    }
    views
}

/// Repeat in both axes and linear, which is what `runtime_bake::image` asks for
/// and what a plane's own wrapped bilinear read is.
fn repeat_sampler(gpu: &Gpu, what: &str) -> Sampler {
    gpu.device.create_sampler(&SamplerDescriptor {
        label: Some(what),
        address_mode_u: AddressMode::Repeat,
        address_mode_v: AddressMode::Repeat,
        address_mode_w: AddressMode::Repeat,
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        mipmap_filter: MipmapFilterMode::Nearest,
        ..SamplerDescriptor::default()
    })
}

/// Run one emitted kernel and read back what it wrote.
///
/// The answer is one `vec4<f32>` per port per texel, in the order
/// [`ComputeShader::ports`] gives and the order the kernel's own index
/// arithmetic writes.
fn dispatch(
    gpu: &Gpu,
    kernel: &ComputeShader,
    what: &str,
    uploads: &[Upload],
    params: &[u8],
    inputs: &Inputs<'_>,
    mesh: Option<&MeshPlanes>,
) -> Vec<[f32; 4]> {
    let device = &gpu.device;
    let entries = layout_entries(kernel);
    let layout = device.create_bind_group_layout(what, &entries);
    let pipeline = compile(gpu, kernel, what, &layout);
    let views = upload_textures(gpu, kernel, uploads);
    let sampler = repeat_sampler(gpu, what);
    let output_bytes = (kernel.output_len() * 16) as u64;
    let output = device.create_buffer(&BufferDescriptor {
        label: Some(what),
        size: output_bytes,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("readback"),
        size: output_bytes,
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let runtime = device.create_buffer(&BufferDescriptor {
        label: Some("AshlarInputs"),
        size: 32,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    gpu.queue.write_buffer(&runtime, 0, &runtime_bytes(inputs));
    let block = device.create_buffer(&BufferDescriptor {
        label: Some("AshlarParams"),
        size: u64::from(kernel.layout().size()),
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    gpu.queue.write_buffer(&block, 0, params);

    let mut bind_entries = vec![
        BindGroupEntry {
            binding: wgsl::OUTPUT_BINDING,
            resource: output.as_entire_binding(),
        },
        BindGroupEntry {
            binding: wgsl::INPUTS_BINDING,
            resource: runtime.as_entire_binding(),
        },
        BindGroupEntry {
            binding: wgsl::PARAMS_BINDING,
            resource: block.as_entire_binding(),
        },
    ];
    for (binding, view) in kernel.bindings().iter().zip(&views) {
        bind_entries.push(BindGroupEntry {
            binding: binding.texture,
            resource: BindingResource::TextureView(view),
        });
        bind_entries.push(BindGroupEntry {
            binding: binding.sampler,
            resource: BindingResource::Sampler(&sampler),
        });
    }
    // Three more buffers where the kernel reads a mesh per texel rather than
    // one for the whole square. They are bound here and owned until the pass
    // has been submitted.
    let mesh_buffers = (kernel.mesh() == MeshInputs::Planes)
        .then(|| upload_mesh(gpu, mesh.expect("a kernel over mesh planes was given none")));
    if let Some([position, normal, cut]) = mesh_buffers.as_ref() {
        for (binding, buffer) in [
            (wgsl::WORLD_POS_BINDING, position),
            (wgsl::WORLD_NORMAL_BINDING, normal),
            (wgsl::CUT_FLAG_BINDING, cut),
        ] {
            bind_entries.push(BindGroupEntry {
                binding,
                resource: buffer.as_entire_binding(),
            });
        }
    }
    let bind_group = device.create_bind_group(what, &layout, &bind_entries);

    let groups = kernel.resolution().div_ceil(wgsl::WORKGROUP);
    let mut encoder =
        device.create_command_encoder(&CommandEncoderDescriptor { label: Some(what) });
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some(what),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &*bind_group, &[]);
        pass.dispatch_workgroups(groups, groups, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, output_bytes);
    gpu.queue.submit([encoder.finish()]);

    read_back(gpu, &readback)
}

/// The emitted module as a pipeline over the layout it declares.
fn compile(
    gpu: &Gpu,
    kernel: &ComputeShader,
    what: &str,
    layout: &bevy::render::render_resource::BindGroupLayout,
) -> bevy::render::render_resource::ComputePipeline {
    let module = gpu.module(kernel.module(), what);
    let pipeline_layout = gpu
        .device
        .create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some(what),
            bind_group_layouts: &[Some(layout)],
            immediate_size: 0,
        });
    gpu.device
        .create_compute_pipeline(&RawComputePipelineDescriptor {
            label: Some(what),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: PipelineCompilationOptions::default(),
            cache: None,
        })
}

/// The three mesh planes as storage buffers, in the kernel's own order.
fn upload_mesh(gpu: &Gpu, mesh: &MeshPlanes) -> [Buffer; 3] {
    let upload = |label: &str, lanes: &[[f32; 3]]| {
        let bytes = MeshPlanes::bytes(lanes);
        let buffer = gpu.device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size: bytes.len() as u64,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue.write_buffer(&buffer, 0, &bytes);
        buffer
    };
    [
        upload("ashlar_world_pos", &mesh.position),
        upload("ashlar_world_normal", &mesh.normal),
        upload("ashlar_cut_flag", &mesh.cut),
    ]
}

/// The thirty-two bytes of `AshlarInputs`, laid out as the kernel declares it.
fn runtime_bytes(inputs: &Inputs<'_>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(32);
    for lane in inputs.world_pos {
        bytes.extend_from_slice(&lane.to_le_bytes());
    }
    bytes.extend_from_slice(&inputs.time.to_le_bytes());
    for lane in inputs.world_normal {
        bytes.extend_from_slice(&lane.to_le_bytes());
    }
    bytes.extend_from_slice(&inputs.cut_flag.to_le_bytes());
    bytes
}

/// Wait for the queue and read one mapped buffer as `vec4<f32>`s.
fn read_back(gpu: &Gpu, readback: &Buffer) -> Vec<[f32; 4]> {
    let slice = readback.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    gpu.device.map_buffer(&slice, MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    gpu.device
        .poll(PollType::wait_indefinitely())
        .expect("the device answered the dispatch");
    receiver
        .recv()
        .expect("the map callback ran")
        .expect("the readback buffer mapped");
    let view = slice.get_mapped_range();
    // Four bytes to a float, and four floats to a `vec4<f32>`.
    let values: Vec<[f32; 4]> = view
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| f32::from_le_bytes(*word))
        .collect::<Vec<f32>>()
        .as_chunks::<4>()
        .0
        .to_vec();
    drop(view);
    readback.unmap();
    values
}

// --------------------------------------------------------------------------
// The reference
// --------------------------------------------------------------------------

/// The interpreter over the same partition, at the same texel centres.
///
/// One band of rows per thread, which is what the bake's own rasteriser does,
/// with the thread count explicit and bounded. The answer is
/// `texels * roots` values, root-major within a texel, which is the order the
/// kernel writes its `vec4`s in.
fn cpu_reference(
    ir: &ashlar_material::ir::Ir,
    roots: &[ashlar_material::ir::ValueId],
    resolution: u32,
    inputs: &Inputs<'_>,
    mesh: Option<&MeshPlanes>,
    threads: usize,
) -> Vec<[f32; 3]> {
    let side = resolution as usize;
    let mut values = vec![[0.0_f32; 3]; side * side * roots.len()];
    let rows_per_band = side.div_ceil(threads.max(1));
    let bands: Vec<(usize, &mut [[f32; 3]])> = values
        .chunks_mut(rows_per_band * side * roots.len())
        .enumerate()
        .map(|(band, rows)| (band * rows_per_band, rows))
        .collect();
    std::thread::scope(|scope| {
        for (first_row, rows) in bands {
            scope.spawn(move || {
                let interpreter = Interpreter::for_values(ir, roots);
                let mut registers = interpreter.registers();
                for (index, slot) in rows.chunks_mut(roots.len()).enumerate() {
                    let y = first_row + index / side;
                    let x = index % side;
                    // The same numbers the kernel reads out of its three
                    // storage buffers, at the same row-major index.
                    let mut inputs = *inputs;
                    if let Some(mesh) = mesh {
                        let (position, normal, cut) = mesh.at(y * side + x);
                        inputs.world_pos = position;
                        inputs.world_normal = normal;
                        inputs.cut_flag = cut;
                    }
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "a texel index below the resolution, which f32 holds exactly"
                    )]
                    let uv = [
                        (x as f32 + 0.5) / resolution as f32,
                        (y as f32 + 0.5) / resolution as f32,
                    ];
                    interpreter
                        .run(uv, &inputs, &mut registers)
                        .expect("the partition's own planes answer its own samples");
                    for (root, answer) in roots.iter().zip(slot.iter_mut()) {
                        *answer = registers.get(*root);
                    }
                }
            });
        }
    });
    values
}

/// Every live parameter's value, three lanes each, in uniform-block order.
///
/// The convention [`Inputs::params`] and the Bevy side both use: a scalar in
/// the first lane, a colour in all three. The two backends have to read one
/// number the same way or nothing below this is a comparison.
fn live_values(material: &Material, split: &Partition) -> Vec<[f32; 3]> {
    split
        .runtime()
        .params()
        .iter()
        .map(|binding| {
            let param = material
                .param(&binding.name)
                .unwrap_or_else(|| panic!("{} is not a parameter of the graph", binding.name));
            match param.value {
                ParamValue::Color(rgb) => rgb,
                ParamValue::Float(value) => [value, 0.0, 0.0],
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "an authored count, not a bit pattern; the IR runs on f32"
                )]
                ParamValue::Int(value) => [value as f32, 0.0, 0.0],
                ParamValue::Bool(value) => [f32::from(u8::from(value)), 0.0, 0.0],
            }
        })
        .collect()
}

// --------------------------------------------------------------------------
// One case
// --------------------------------------------------------------------------

/// One partition, and the runtime inputs to read it at.
struct Case {
    /// What is under test, for the report line and the failure message.
    name: String,
    material: Material,
    split: Partition,
    /// Seconds, a world position, a world normal and a cut flag. Non-zero on
    /// purpose: a backend that dropped one of them would still pass at zero.
    time: f32,
    world_pos: [f32; 3],
    world_normal: [f32; 3],
    cut_flag: f32,
    /// Where the three mesh inputs come from. [`MeshInputs::Uniform`] is the
    /// three fields above, one value for the whole square; `Planes` lays a
    /// [`MeshPlanes`] over it instead, which is what a case that reads the world
    /// by *position*, or the mesh by which side of a cut it is on, needs to be a
    /// comparison at all.
    inputs: MeshInputs,
}

impl Case {
    fn new(name: impl Into<String>, material: Material, split: Partition) -> Self {
        Self {
            name: name.into(),
            material,
            split,
            time: 1.75,
            world_pos: [3.25, -1.5, 0.75],
            world_normal: [0.3, 0.6, 0.742_775],
            cut_flag: 1.0,
            inputs: MeshInputs::Uniform,
        }
    }

    /// Read the mesh's three inputs per texel rather than per dispatch.
    fn varying(mut self) -> Self {
        self.inputs = MeshInputs::Planes;
        self
    }

    /// Whether the partition reads any of the three at all, which is what
    /// decides the line above for a graph nobody wrote by hand.
    fn reads_the_mesh(split: &Partition) -> bool {
        split
            .runtime()
            .insts()
            .iter()
            .any(|inst| matches!(inst.op, Op::WorldPos | Op::WorldNormal | Op::CutFlag))
    }
}

/// What one port of one case came to, on both sides.
struct Difference {
    port: String,
    /// The largest absolute difference over every texel and every lane.
    worst: f32,
    /// What that port is allowed.
    allowed: f32,
    /// Where it happened, for a failure that has to be chased.
    at: (u32, u32),
}

/// Dispatch one case and compare it with the interpreter, port by port.
fn run(gpu: &Gpu, case: &Case) -> Vec<Difference> {
    let split = &case.split;
    let ir = split.runtime();
    let planes = rasterise_buffers(
        ir,
        split.resolution(),
        NonZeroUsize::new(THREADS),
        &mut BakeCache::new(),
    )
    .expect("the partition's planes rasterise");
    let kernel = wgsl::emit_compute_mesh(
        split,
        RESOLUTION,
        &split.outputs().keys().cloned().collect::<Vec<String>>(),
        case.inputs,
        0,
    )
    .expect("the partition emits a kernel");
    let mesh = (case.inputs == MeshInputs::Planes).then(|| MeshPlanes::over(RESOLUTION));
    let uploads: Vec<Upload> = kernel
        .bindings()
        .iter()
        .map(|binding| upload_of(binding, split.textures(), &planes))
        .collect();

    let values = live_values(&case.material, split);
    let inputs = Inputs {
        time: case.time,
        world_pos: case.world_pos,
        world_normal: case.world_normal,
        cut_flag: case.cut_flag,
        params: &values,
        buffers: &planes,
    };
    let params = kernel.layout().bytes(&values);
    let answers = dispatch(
        gpu,
        &kernel,
        &case.name,
        &uploads,
        &params,
        &inputs,
        mesh.as_ref(),
    );
    assert_eq!(
        answers.len(),
        kernel.output_len(),
        "{}: the kernel wrote a different number of values than it said it would",
        case.name
    );

    let roots: Vec<_> = kernel
        .ports()
        .iter()
        .map(|(port, _)| {
            ir.root(port)
                .unwrap_or_else(|| panic!("{}: port {port} has no root", case.name))
        })
        .collect();
    let reference = cpu_reference(ir, &roots, RESOLUTION, &inputs, mesh.as_ref(), THREADS);

    let ports = kernel.ports().len();
    let side = RESOLUTION as usize;
    let mut differences = Vec::with_capacity(ports);
    for (slot, (port, value_type)) in kernel.ports().iter().enumerate() {
        let allowed = tolerance(port);
        let mut worst = 0.0_f32;
        let mut at = (0, 0);
        for texel in 0..side * side {
            let mine = reference[texel * ports + slot];
            let theirs = answers[texel * ports + slot];
            for lane in 0..value_type.components() {
                let difference = (mine[lane] - theirs[lane]).abs();
                if difference > worst {
                    worst = difference;
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "a texel index below the resolution"
                    )]
                    {
                        at = ((texel % side) as u32, (texel / side) as u32);
                    }
                }
            }
        }
        differences.push(Difference {
            port: port.clone(),
            worst,
            allowed,
            at,
        });
    }
    differences
}

/// What a port is allowed to differ by.
fn tolerance(port: &str) -> f32 {
    match port {
        "base_color" | "emissive" => COLOUR_STEP,
        "normal" => NORMAL_STEP,
        _ => RAW_TOLERANCE,
    }
}

/// Run every case, print what each came to, and fail naming every port that
/// went past its budget.
///
/// One report rather than one assertion per case, because the interesting
/// failure is "these three ops drifted" and a test that stops at the first one
/// hides the other two.
fn check(gpu: &Gpu, cases: &[Case], what: &str) {
    let mut report = String::new();
    let _ = writeln!(report, "\n{what}, on {}:", gpu.adapter);
    let mut failures = Vec::new();
    for case in cases {
        let differences = run(gpu, case);
        // One line per case, naming the port that came nearest its budget:
        // every port is checked, and printing all of them would bury the four
        // that are ever interesting under a hundred that are exactly zero.
        let worst = differences
            .iter()
            .max_by(|left, right| {
                (left.worst / left.allowed)
                    .partial_cmp(&(right.worst / right.allowed))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .expect("every partition writes at least one port");
        let _ = writeln!(
            report,
            "  {:<44} {:>2} ports, worst {:<12} {:.3e} of {:.3e}",
            case.name,
            differences.len(),
            worst.port,
            worst.worst,
            worst.allowed
        );
        for difference in &differences {
            if difference.worst > difference.allowed || !difference.worst.is_finite() {
                failures.push(format!(
                    "{}: {} differs by {:.4e} at texel {:?}, which is past {:.4e}",
                    case.name, difference.port, difference.worst, difference.at, difference.allowed
                ));
            }
        }
    }
    println!("{report}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// --------------------------------------------------------------------------
// One partition per primitive op
// --------------------------------------------------------------------------

/// The static field every op case reads through a bound texture.
fn grain() -> Noise {
    Noise::value().period(8)
}

/// A second one, for the ops that take two fields.
fn other() -> Noise {
    Noise::value().period(4).seed(1)
}

/// A third.
fn third() -> Noise {
    Noise::value().period(2).seed(2)
}

/// A case whose graph declares exactly the parameters it uses, every one live.
///
/// The shape every row below has: a static field, which the partition cuts into
/// a bound texture, and a live parameter, which is what pulls the op under test
/// out of the texture and into the fragment. That is as close to a one-op
/// partition as a graph can be written, and it is the shape the design's
/// "a live parameter at the output pulls in one op" describes.
fn case(
    name: &str,
    floats: &[(&str, f32)],
    colours: &[(&str, [f32; 3])],
    build: impl FnOnce(MaterialGraphBuilder) -> MaterialGraphBuilder,
) -> Case {
    let mut builder = MaterialGraph::builder("test:op");
    for (parameter, value) in floats {
        builder = builder.param(Param::float(*parameter, *value).live());
    }
    for (parameter, value) in colours {
        builder = builder.param(Param::color(*parameter, *value).live());
    }
    let material = build(builder)
        .build()
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    let target = Target::shader_for(&material);
    let split =
        partition(&material, &target, RESOLUTION).unwrap_or_else(|error| panic!("{name}: {error}"));
    Case::new(name, material, split)
}

/// One partition per primitive op, each named by the op it is about.
///
/// Read this as the table the ADR promises: "only the thirty-odd `Op`s are
/// implemented twice, and the conformance test covers exactly that set". Some
/// rows carry more than one op because a node lowering does — `sqrt` is a `max`
/// against a constant and then a square root, and `sin` is a multiply into
/// radians — and each says so in its name.
#[expect(
    clippy::too_many_lines,
    reason = "a table with one row per primitive op; splitting it would hide that it is a table"
)]
fn op_cases() -> Vec<Case> {
    vec![
        case("Add", &[("x", 0.375)], &[], |b| {
            b.node("n", Math::new(MathOp::Add, grain(), Input::param("x")))
                .output(PbrOutput::new().roughness("n"))
        }),
        case("Sub", &[("x", 0.375)], &[], |b| {
            b.node("n", Math::new(MathOp::Sub, grain(), Input::param("x")))
                .output(PbrOutput::new().roughness("n"))
        }),
        case("Mul", &[("x", 1.625)], &[], |b| {
            b.node("n", Math::new(MathOp::Mul, grain(), Input::param("x")))
                .output(PbrOutput::new().roughness("n"))
        }),
        case("Div", &[("x", 0.375)], &[], |b| {
            b.node("n", Math::new(MathOp::Div, grain(), Input::param("x")))
                .output(PbrOutput::new().roughness("n"))
        }),
        // The guarded case, which is the whole reason division is a helper
        // function rather than a slash: WGSL leaves a division by zero
        // undefined and the interpreter answers zero.
        case("Div by zero, which answers zero", &[("x", 0.0)], &[], |b| {
            b.node("n", Math::new(MathOp::Div, grain(), Input::param("x")))
                .output(PbrOutput::new().roughness("n"))
        }),
        case("Min", &[("x", 0.5)], &[], |b| {
            b.node("n", Math::new(MathOp::Min, grain(), Input::param("x")))
                .output(PbrOutput::new().roughness("n"))
        }),
        case("Max", &[("x", 0.5)], &[], |b| {
            b.node("n", Math::new(MathOp::Max, grain(), Input::param("x")))
                .output(PbrOutput::new().roughness("n"))
        }),
        case(
            "Abs, over a field that goes negative",
            &[("x", 0.5)],
            &[],
            |b| {
                b.node("under", Math::new(MathOp::Sub, grain(), Input::param("x")))
                    .node("n", Math::unary(MathOp::Abs, "under"))
                    .output(PbrOutput::new().roughness("n"))
            },
        ),
        case("Floor", &[("x", 7.0)], &[], |b| {
            b.node("under", Math::new(MathOp::Mul, grain(), Input::param("x")))
                .node("n", Math::unary(MathOp::Floor, "under"))
                .output(PbrOutput::new().roughness("n"))
        }),
        case("Fract", &[("x", 7.0)], &[], |b| {
            b.node("under", Math::new(MathOp::Mul, grain(), Input::param("x")))
                .node("n", Math::unary(MathOp::Fract, "under"))
                .output(PbrOutput::new().roughness("n"))
        }),
        // `Sqrt` lowers to a `Max` against a constant zero and then the root,
        // so this row carries `Const`, `Max` and `Sqrt` together.
        case("Const, Max and Sqrt", &[("x", 0.4)], &[], |b| {
            b.node("under", Math::new(MathOp::Sub, grain(), Input::param("x")))
                .node("n", Math::unary(MathOp::Sqrt, "under"))
                .output(PbrOutput::new().roughness("n"))
        }),
        // A positive base on purpose: `f32::powf` answers for a negative one
        // and WGSL's `pow` says nothing about it. That difference is documented
        // in `wgsl.rs` and is not something a tolerance can cover.
        case(
            "Pow, over a base that stays positive",
            &[("x", 2.2)],
            &[],
            |b| {
                b.node("n", Math::new(MathOp::Pow, grain(), Input::param("x")))
                    .output(PbrOutput::new().roughness("n"))
            },
        ),
        case("Exp2", &[("x", 0.5)], &[], |b| {
            b.node("under", Math::new(MathOp::Sub, grain(), Input::param("x")))
                .node("n", Math::unary(MathOp::Exp2, "under"))
                .output(PbrOutput::new().roughness("n"))
        }),
        case(
            "Log2, over an argument that stays positive",
            &[("x", 0.5)],
            &[],
            |b| {
                b.node("under", Math::new(MathOp::Add, grain(), Input::param("x")))
                    .node("n", Math::unary(MathOp::Log2, "under"))
                    .output(PbrOutput::new().roughness("n"))
            },
        ),
        case(
            "Sin, and the multiply into radians",
            &[("x", 3.0)],
            &[],
            |b| {
                b.node("under", Math::new(MathOp::Mul, grain(), Input::param("x")))
                    .node("n", Math::unary(MathOp::Sin, "under"))
                    .output(PbrOutput::new().roughness("n"))
            },
        ),
        case(
            "Cos, and the multiply into radians",
            &[("x", 3.0)],
            &[],
            |b| {
                b.node("under", Math::new(MathOp::Mul, grain(), Input::param("x")))
                    .node("n", Math::unary(MathOp::Cos, "under"))
                    .output(PbrOutput::new().roughness("n"))
            },
        ),
        case(
            "Atan2, over both signs",
            &[("x", 0.5), ("y", 0.5)],
            &[],
            |b| {
                b.node("a", Math::new(MathOp::Sub, grain(), Input::param("x")))
                    .node("b", Math::new(MathOp::Sub, other(), Input::param("y")))
                    .node("n", Math::new(MathOp::Atan2, "a", "b"))
                    .output(PbrOutput::new().roughness("n"))
            },
        ),
        case("Step", &[("x", 0.5)], &[], |b| {
            b.node("n", Math::new(MathOp::Step, grain(), Input::param("x")))
                .output(PbrOutput::new().roughness("n"))
        }),
        case("Smoothstep", &[("x", 0.25)], &[], |b| {
            b.node(
                "n",
                Math::new(MathOp::Smoothstep, grain(), Input::param("x")),
            )
            .output(PbrOutput::new().roughness("n"))
        }),
        // The guarded case: a bevel of zero is a smoothstep whose edges meet,
        // which WGSL's own `smoothstep` divides by zero on and the emitted
        // helper answers as a step.
        case(
            "Smoothstep with edges that meet",
            &[("x", 0.13)],
            &[],
            |b| {
                b.node(
                    "under",
                    Bricks::new()
                        .rows(4)
                        .columns(2)
                        .bevel(0.0)
                        .output(BrickOutput::Bevel),
                )
                .node("n", Warp::new("under", Input::param("x")))
                .output(PbrOutput::new().roughness("n"))
            },
        ),
        // The third node set brings no new op, and these two rows are what says
        // so: each is a node whose whole lowering is arithmetic the emitter
        // already writes, checked against the interpreter that defines it. A
        // directional warp is the sine and cosine of a field rather than of a
        // constant, so the turn into radians happens per texel where every
        // other rotation in the crate folds; a height-to-mask is the guarded
        // smoothstep twice, in the one arrangement where its two ramps overlap.
        case(
            "a DirectionalWarp, whose angle is a field",
            &[("x", 0.2)],
            &[],
            |b| {
                b.node("angle", Math::new(MathOp::Add, other(), Input::param("x")))
                    .node("n", DirectionalWarp::new(grain(), "angle").amount(0.08))
                    .output(PbrOutput::new().roughness("n"))
            },
        ),
        case(
            "a HeightToMask, whose ramps meet",
            &[("x", 0.35)],
            &[],
            |b| {
                b.node("under", Math::new(MathOp::Add, grain(), Input::param("x")))
                    .node("n", HeightToMask::band("under", 0.4, 0.55).softness(0.3))
                    .output(PbrOutput::new().roughness("n"))
            },
        ),
        case("Mix", &[("t", 0.35)], &[], |b| {
            b.node("n", Mix::new(grain(), other(), Input::param("t")))
                .output(PbrOutput::new().roughness("n"))
        }),
        case("Clamp", &[("x", 0.35)], &[], |b| {
            b.node("under", Math::new(MathOp::Sub, grain(), Input::param("x")))
                .node("n", Clamp::new("under").range(0.2, 0.7))
                .output(PbrOutput::new().roughness("n"))
        }),
        // The graph is allowed to write bounds that cross; WGSL's `clamp` is
        // undefined there and the emitter writes `min(max(..))` for it.
        case("Clamp with bounds that cross", &[("x", 0.35)], &[], |b| {
            b.node("under", Math::new(MathOp::Sub, grain(), Input::param("x")))
                .node("n", Clamp::new("under").range(0.7, 0.2))
                .output(PbrOutput::new().roughness("n"))
        }),
        case("Select", &[("x", 0.5)], &[], |b| {
            b.node("c", Math::new(MathOp::Sub, grain(), Input::param("x")))
                .node("n", Switch::new("c", other(), third()))
                .output(PbrOutput::new().roughness("n"))
        }),
        // The one op that has to be bit-exact rather than close: the lattice
        // hash, reached through a brick id under a live warp.
        case("Hash2, which must be exact", &[("x", 0.13)], &[], |b| {
            b.node(
                "under",
                Bricks::new().rows(4).columns(4).output(BrickOutput::Id),
            )
            .node("n", Warp::new("under", Input::param("x")))
            .output(PbrOutput::new().roughness("n"))
        }),
        case("Length", &[("x", 0.13)], &[], |b| {
            b.node("under", Shape::new(ShapeKind::Circle))
                .node("n", Warp::new("under", Input::param("x")))
                .output(PbrOutput::new().roughness("n"))
        }),
        // A Perlin gradient is a normalise of a hashed vector and a dot with
        // the offset, which is the only place either op comes up.
        case("Dot and Normalize", &[("x", 0.13)], &[], |b| {
            b.node("under", Noise::perlin().period(4))
                .node("n", Warp::new("under", Input::param("x")))
                .output(PbrOutput::new().roughness("n"))
        }),
        case(
            "Sample, of a plane a filter left behind",
            &[("x", 1.5)],
            &[],
            |b| {
                b.node("blurred", Blur::new(grain()).radius(0.02))
                    .node("n", Math::new(MathOp::Mul, "blurred", Input::param("x")))
                    .output(PbrOutput::new().roughness("n"))
            },
        ),
        case("Compose", &[("x", 1.25)], &[], |b| {
            b.node("r", Math::new(MathOp::Mul, grain(), Input::param("x")))
                .node("n", Combine::new("r", other(), third()))
                .output(PbrOutput::new().base_color("n"))
        }),
        case("Extract", &[], &[("tint", [0.2, 0.7, 0.45])], |b| {
            b.node("lane", Decompose::new(Input::param("tint"), Channel::G))
                .node("n", Math::new(MathOp::Mul, grain(), "lane"))
                .output(PbrOutput::new().roughness("n"))
        }),
        // The coordinate itself, and a bound texture read somewhere other than
        // a texel centre, which is where the hardware's bilinear weights come
        // into it.
        case(
            "Uv, and a sample away from a texel centre",
            &[("x", 0.137)],
            &[],
            |b| {
                b.node("n", Warp::new(grain(), Input::param("x")))
                    .output(PbrOutput::new().roughness("n"))
            },
        ),
        // The design's own live material: a clock into the emissive slot. The
        // emissive port is a colour, so this is also where the colour budget
        // is exercised over something that moves.
        case("Time, into the emissive slot", &[], &[], |b| {
            b.node("n", Math::new(MathOp::Mul, Time::new(), 0.25))
                .output(PbrOutput::new().roughness("n").emissive("n"))
        }),
        // The two world-space inputs, read per texel: a fixed world would
        // compare one number against one number, which every backend agrees
        // about.
        case("WorldPos, over a world that moves", &[], &[], |b| {
            b.node("n", WorldPos::new())
                .output(PbrOutput::new().base_color("n"))
        })
        .varying(),
        case("WorldNormal, over a world that turns", &[], &[], |b| {
            b.node("n", WorldNormal::new())
                .output(PbrOutput::new().base_color("n"))
        })
        .varying(),
        // And the two nodes built out of them. A triplanar is the one node in
        // the vocabulary that emits its source three times, at three
        // coordinates none of which is the texel's own, so what this compares
        // is three copies of a value noise's lattice hash read at a world
        // position — the `pow` of the weights and the guarded division with
        // them.
        case(
            "a Triplanar, blended by a normal that turns",
            &[],
            &[],
            |b| {
                b.node("n", Triplanar::new(grain()).tile_metres(2.5).sharpness(4.0))
                    .output(PbrOutput::new().roughness("n"))
            },
        )
        .varying(),
        case("a WorldMask on the faces that point up", &[], &[], |b| {
            b.node("up", WorldMask::up())
                .node("low", WorldMask::below(2.0).softness(0.75))
                .node("gate", Math::new(MathOp::Mul, "up", "low"))
                .node("n", Math::new(MathOp::Mul, grain(), "gate"))
                .output(PbrOutput::new().roughness("n"))
        })
        .varying(),
        case("CutFlag", &[], &[], |b| {
            b.node("n", Math::new(MathOp::Mul, CutFlag::new(), 0.5))
                .output(PbrOutput::new().roughness("n"))
        }),
        // The same op over a mesh that has a boundary on it, which is the shape
        // a cut-aware material is: two surfaces and the flag choosing between
        // them. At one fixed flag the `Mix` is one of its two arms everywhere
        // and the other arm is never evaluated against the reference; over a
        // plane of blocks both arms and every block edge are in the frame.
        case("a CutFlag choosing between two surfaces", &[], &[], |b| {
            b.node("raw", Math::new(MathOp::Mul, other(), 0.6))
                .node("n", Mix::new(grain(), "raw", CutFlag::new()))
                .output(PbrOutput::new().roughness("n"))
        })
        .varying(),
        // The partition's own root: a live height has no bound normal map, so
        // the material's normal is four taps of the height and a normalise.
        case(
            "a live height, whose normal is four taps",
            &[("x", 0.8)],
            &[],
            |b| {
                b.node("h", Math::new(MathOp::Mul, grain(), Input::param("x")))
                    .output(PbrOutput::new().height("h").normal_strength(0.02))
            },
        ),
        // Not an op but a layout, and the one the rest of this table cannot
        // reach: ten scalar cuts is more images than a bind group holds, so the
        // partition packs them four to an image and every sample has to read the
        // lane it landed in rather than the red channel. This row is the whole
        // round trip for that — the packing, the upload and the swizzle — and it
        // is a lane the CPU reference reads out of a plane of its own.
        case(
            "packed lanes, four to an image",
            &[("x", 0.625)],
            &[],
            |b| {
                let mut builder = b;
                let mut total: Option<String> = None;
                for index in 0..10_u32 {
                    let noise = format!("n{index}");
                    let worn = format!("w{index}");
                    builder = builder
                        .node(&noise, Noise::value().period(4).seed(index))
                        .node(
                            &worn,
                            Math::new(MathOp::Mul, noise.as_str(), Input::param("x")),
                        );
                    total = Some(match total {
                        None => worn,
                        Some(previous) => {
                            let id = format!("s{index}");
                            builder = builder.node(
                                &id,
                                Math::new(MathOp::Add, previous.as_str(), worn.as_str()),
                            );
                            id
                        }
                    });
                }
                builder.output(PbrOutput::new().roughness(total.unwrap().as_str()))
            },
        ),
    ]
}

// --------------------------------------------------------------------------
// One partition per node the second expansion added
// --------------------------------------------------------------------------

/// A probe graph per node, and per output, whose arithmetic is new.
///
/// [`op_cases`] above is a table of *ops*, and its coverage test says that
/// between them they reach every arm the two backends implement. That claim
/// holds for the nodes below without a single row here: a weave is fract,
/// floor, cos and a select, and every one of those already has a case. What it
/// does not say is that a node *composes* them into the same expression twice.
/// The two backends lower from one IR, so a node cannot disagree about which op
/// to use — but it can lower to arithmetic that only differs once the numbers
/// are real, which is what a division whose divisor reaches zero at a seam, a
/// `mix` at a weight of exactly one, or a `pow` of a base that goes negative
/// does. A study graph would catch it, and in phase 4 one will; until then
/// these rows are the only place a polar frame, a fillet and a cloth are
/// evaluated on a GPU at all.
///
/// The recipe is `tests/wgsl.rs`'s, and deliberately the same graphs: a
/// [`Warp`] whose offset is a live parameter over the node under test. A warp
/// re-emits everything its source computes at a moved coordinate, and a moved
/// coordinate that depends on a uniform makes every one of those copies
/// runtime, so the node's own instructions land in the fragment instead of in a
/// bound texture. Every window edge, thread edge and instance rim is off the
/// texel grid on purpose: a discontinuity that lands exactly on a texel centre
/// is a texel where the two backends are allowed to answer either side, and
/// none of these rows would notice a sign error there.
#[expect(
    clippy::too_many_lines,
    reason = "a table with one row per node under test; splitting it would hide that it is a table"
)]
fn node_cases() -> Vec<Case> {
    /// The tiled field a resampler reads.
    fn source() -> Noise {
        Noise::value().period(8)
    }
    vec![
        // The whole polar frame: an atan2, a normalised radius, two turns, a
        // twist that shears the angle by the radius, and the select that fills
        // the hole and everything past the rim.
        case(
            "CircleMap, twisted, over a hole",
            &[("shift", 0.25)],
            &[],
            |b| {
                b.node("src", source())
                    .node(
                        "under",
                        CircleMap::new("src")
                            .radius(0.4)
                            .inner(0.05)
                            .turns(2)
                            .twist(0.5)
                            .outside(0.25),
                    )
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            },
        ),
        // Two rings of six instances with all four variations and a field
        // mask, which is three source reads per ring, a per-instance sine and
        // cosine, a hashed standoff and the gate. The most arithmetic any node
        // in the vocabulary emits per texel.
        case(
            "CircleSplatter, masked, with every variation",
            &[("shift", 0.25)],
            &[],
            |b| {
                b.node("disc", Shape::new(ShapeKind::Circle))
                    .node("gate", Noise::value().period(4).seed(5))
                    .node(
                        "under",
                        CircleSplatter::new("disc")
                            .mask("gate")
                            .count(6)
                            .rings(2)
                            .radius(0.3)
                            .inner(0.08)
                            .scale(0.12)
                            .scale_variation(0.25)
                            .rotation_variation(0.4)
                            .radius_variation(0.3)
                            .opacity_variation(0.5)
                            .face_centre()
                            .seed(11),
                    )
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            },
        ),
        // The gear's angular fold and tooth ramp behind the distance output,
        // which is the one output that leaves the unit interval: a signed
        // distance is negative inside and unclamped, so a backend that
        // saturated it would show here and nowhere else.
        case(
            "a hollow Gear as a distance",
            &[("shift", 0.25)],
            &[],
            |b| {
                b.node(
                    "under",
                    Shape::new(ShapeKind::Gear)
                        .sides(9)
                        .size(0.42)
                        .depth(0.12)
                        .hollow(0.08)
                        .output(ShapeOutput::Distance),
                )
                .node("n", Warp::new("under", Input::param("shift")))
                .output(PbrOutput::new().roughness("n"))
            },
        ),
        // The two kinds whose distance is Euclidean, one rounded and one a
        // hollowed segment, on two ports of one partition so each is compared
        // against its own budget rather than through a blend of the pair.
        case(
            "a rounded Box and a hollow Capsule",
            &[("shift", 0.25)],
            &[],
            |b| {
                b.node(
                    "panel",
                    Shape::new(ShapeKind::Box).size(0.4).edge(0.02).round(0.12),
                )
                .node(
                    "slot",
                    Shape::new(ShapeKind::Capsule)
                        .size(0.06)
                        .edge(0.01)
                        .length(0.3)
                        .hollow(0.03),
                )
                .node("r", Warp::new("panel", Input::param("shift")))
                .node("m", Warp::new("slot", Input::param("shift")))
                .output(PbrOutput::new().roughness("r").metallic("m"))
            },
        ),
        // A subtraction at a width of zero, which the node emits as the bare
        // `max(a, -b)` and nothing else. The claim the crate makes about it is
        // bit equality with the hand-written boolean, so the interesting
        // number here is a zero rather than a small one.
        case(
            "SdfCombine, a hard subtraction at smooth 0",
            &[("shift", 0.25)],
            &[],
            |b| {
                b.node("plate", plate())
                    .node("hole", hole())
                    .node("under", SdfCombine::new(SdfOp::Subtract, "plate", "hole"))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            },
        ),
        // And the same pair filleted, which is the branch carrying the whole
        // polynomial: the negation, a division by the width, a clamp, a `mix`
        // whose weight reaches both ends exactly, and the quadratic. Three of
        // the four places WGSL is allowed to round differently from the
        // interpreter are in that list.
        case(
            "SdfCombine, a filleted subtraction at smooth 0.05",
            &[("shift", 0.25)],
            &[],
            |b| {
                b.node("plate", plate())
                    .node("hole", hole())
                    .node(
                        "under",
                        SdfCombine::new(SdfOp::Subtract, "plate", "hole").smooth(0.05),
                    )
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            },
        ),
        // The ramp on its own, over a field with corners in it, at an edge
        // narrow enough that the smoothstep spends most of its width inside a
        // single texel.
        case(
            "SdfMask over a star's distance",
            &[("shift", 0.25)],
            &[],
            |b| {
                b.node(
                    "src",
                    Shape::new(ShapeKind::Star)
                        .sides(5)
                        .size(0.35)
                        .output(ShapeOutput::Distance),
                )
                .node("under", SdfMask::new("src").edge(0.04))
                .node("n", Warp::new("under", Input::param("shift")))
                .output(PbrOutput::new().roughness("n"))
            },
        ),
        // A plain weave as relief: both cross-sections, the crossing rule and
        // the two crests the maximum chooses between, on a cloth whose threads
        // do not divide the resolution.
        case("a plain Weave as relief", &[("shift", 0.25)], &[], |b| {
            b.node(
                "under",
                Weave::new()
                    .x(12)
                    .y(6)
                    .width(0.78)
                    .output(WeaveOutput::Height),
            )
            .node("n", Warp::new("under", Input::param("shift")))
            .output(PbrOutput::new().roughness("n"))
        }),
        // The same field under a twill, whose crossing marches a thread per
        // weft: the same instructions at different constants, and the row that
        // says the fractional residue lands on the same side of the float on
        // both backends.
        case("a twill Weave as relief", &[("shift", 0.25)], &[], |b| {
            b.node(
                "under",
                Weave::new()
                    .x(12)
                    .y(8)
                    .width(0.72)
                    .pattern(WeavePattern::Twill { step: 4 })
                    .output(WeaveOutput::Height),
            )
            .node("n", Warp::new("under", Input::param("shift")))
            .output(PbrOutput::new().roughness("n"))
        }),
        // And a satin read as thread ids, which is the arm that emits the
        // satin's own move, two hashes and the select between them. A hash is
        // the one thing in this file that has to be bit-exact rather than
        // close, and here it is reached through a node rather than through the
        // emitted helper.
        case(
            "a satin Weave as thread ids",
            &[("shift", 0.25)],
            &[],
            |b| {
                b.node(
                    "under",
                    Weave::new()
                        .x(10)
                        .y(10)
                        .width(0.85)
                        .pattern(WeavePattern::Satin { step: 5 })
                        .seed(3)
                        .output(WeaveOutput::Id),
                )
                .node("n", Warp::new("under", Input::param("shift")))
                .output(PbrOutput::new().roughness("n"))
            },
        ),
    ]
}

/// The field a boolean keeps: a hard-edged square, read as a distance.
fn plate() -> Shape {
    Shape::new(ShapeKind::Box)
        .size(0.4)
        .edge(0.0)
        .output(ShapeOutput::Distance)
}

/// And the field it cuts out of it, whose rim is a curve the square's straight
/// edges cannot hide a sign error behind.
fn hole() -> Shape {
    Shape::new(ShapeKind::Circle)
        .size(0.18)
        .edge(0.0)
        .output(ShapeOutput::Distance)
}

// --------------------------------------------------------------------------
// The tests
// --------------------------------------------------------------------------

#[test]
#[ignore = "needs a GPU; run it with `just conformance`"]
fn every_primitive_op_agrees_with_the_interpreter() {
    let gpu = Gpu::open();
    let cases = op_cases();
    check(&gpu, &cases, "every primitive op");
}

/// Not `#[ignore]`d, and the second test in this file that is not.
///
/// It needs no adapter, and it is what makes the test below a comparison at
/// all. Every row of [`node_cases`] is written so that the partition cuts
/// nothing: a warp over a live parameter makes the whole probe runtime, and the
/// numbers the GPU answers are numbers it computed rather than numbers it read
/// back out of a texture the CPU rasterised. A probe that lost its live
/// parameter, or a node that stopped depending on the coordinate, would collapse
/// to one [`Op::Sample`] and pass the comparison trivially, saying nothing about
/// the lowering under test. So: no sampled instruction and no bound texture, in
/// any of them.
#[test]
fn every_node_case_is_computed_in_the_fragment_rather_than_read_back() {
    for case in node_cases() {
        let ir = case.split.runtime();
        let sampled = ir
            .insts()
            .iter()
            .filter(|inst| matches!(inst.op, Op::Sample(_)))
            .count();
        assert_eq!(
            (sampled, case.split.textures().len()),
            (0, 0),
            "{}: the partition cut part of the probe into a bound texture, so the GPU \
             reads that part back rather than computing it",
            case.name
        );
    }
}

/// The nodes the second expansion added, each on a probe of its own.
///
/// `#[ignore]`d like everything else that dispatches, and separate from the op
/// test above so that a failure reads as "this node's lowering disagrees"
/// rather than as "some op does". There is no coverage test beside this one:
/// what a node lowers to is the crate's own business and `ashlar-material`'s
/// `tests/wgsl.rs` already holds one row per kind of the vocabulary. This list
/// is the narrower claim that the arithmetic phase 2 wrote runs the same on a
/// GPU as it does in the interpreter, and it retires when phase 4's study
/// graphs reach these nodes through
/// [`every_shipped_graph_agrees_with_the_interpreter_with_every_parameter_live`].
#[test]
#[ignore = "needs a GPU; run it with `just conformance`"]
fn every_new_node_agrees_with_the_interpreter() {
    let gpu = Gpu::open();
    let cases = node_cases();
    check(&gpu, &cases, "every node the second expansion added");
}

/// Deliberately *not* `#[ignore]`d, alone in this file.
///
/// It needs no adapter — it only partitions — and it is what makes the test
/// above a claim about the `Op` set rather than about thirty-odd graphs: an arm
/// no case reaches is an arm the emitter could have written wrong and nothing
/// would say so. Running it in `just ci` means a new op with no case fails on
/// the branch that added the op, on a machine with no GPU in it.
#[test]
fn the_op_cases_between_them_reach_every_op_the_two_backends_implement() {
    let mut reached: BTreeSet<u64> = BTreeSet::new();
    for case in op_cases() {
        let ir = case.split.runtime();
        let roots: Vec<_> = case
            .split
            .outputs()
            .values()
            .filter_map(|source| source.value())
            .collect();
        let live = ir.reaches(&roots);
        for (index, inst) in ir.insts().iter().enumerate() {
            if live.get(index).copied() == Some(true) {
                reached.insert(tag(inst.op));
            }
        }
    }
    // Every arm but `Hash3`, which no node in the vocabulary emits: it is the
    // three-dimensional lattice a solid noise would want and nothing lowers to
    // one yet, so it is covered by
    // [`the_lattice_hashes_are_bit_exact`] against the emitted helper itself
    // rather than through a graph.
    let expected: BTreeSet<u64> = all_ops()
        .iter()
        .copied()
        .filter(|op| !matches!(op, Op::Hash3(_)))
        .map(tag)
        .collect();
    let missing: Vec<Op> = all_ops()
        .iter()
        .copied()
        .filter(|op| expected.contains(&tag(*op)) && !reached.contains(&tag(*op)))
        .collect();
    assert!(missing.is_empty(), "no case reaches {missing:?}");
}

/// Every arm of [`Op`], as one value each.
///
/// Written out rather than derived, because the point is to notice when an arm
/// is added: a new op with no row in [`op_cases`] fails the coverage test
/// above, which is the moment to write one.
fn all_ops() -> Vec<Op> {
    vec![
        Op::Const(0.0),
        Op::Uv,
        Op::Param(0),
        Op::Time,
        Op::WorldPos,
        Op::WorldNormal,
        Op::CutFlag,
        Op::Add,
        Op::Sub,
        Op::Mul,
        Op::Div,
        Op::Min,
        Op::Max,
        Op::Abs,
        Op::Floor,
        Op::Fract,
        Op::Sqrt,
        Op::Pow,
        Op::Exp2,
        Op::Log2,
        Op::Sin,
        Op::Cos,
        Op::Atan2,
        Op::Mix,
        Op::Step,
        Op::Smoothstep,
        Op::Clamp,
        Op::Select,
        Op::Hash2(0),
        Op::Hash3(0),
        Op::Length,
        Op::Dot,
        Op::Normalize,
        Op::Sample(ashlar_material::ir::BufferId::default()),
        Op::Compose,
        Op::Extract(0),
    ]
}

/// Which arm an op is, ignoring what it carries.
///
/// `Op` is `Copy` but not `Eq`, and its own tag is crate-private, so the arm is
/// read the way [`std::mem::discriminant`] reads one — hashed, because a
/// `Discriminant` is comparable but not orderable and a `BTreeSet` wants an
/// order.
fn tag(op: Op) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&std::mem::discriminant(&op), &mut hasher);
    std::hash::Hasher::finish(&hasher)
}

#[test]
#[ignore = "needs a GPU; run it with `just conformance`"]
fn every_shipped_graph_agrees_with_the_interpreter_with_every_parameter_live() {
    let gpu = Gpu::open();
    let library = shipped();
    let mut cases = Vec::new();
    for (key, material) in library.build_all().unwrap() {
        let target = Target::Shader {
            live: material
                .graph()
                .params
                .iter()
                .map(|param| param.name.clone())
                .collect(),
        };
        let split = match partition(&material, &target, SHIPPED_RESOLUTION) {
            Ok(split) => split,
            // The grass's strand reliefs are eleven planes, and with every
            // parameter live none of them folds; it ships baked.
            Err(error) if key == "library:grass" => {
                assert!(error.reason.contains("binds 11 textures"), "{error}");
                continue;
            }
            Err(error) => panic!("{key} with everything live: {error}"),
        };
        // A graph that reads the mesh is compared over a mesh that varies:
        // `study:concrete-wet` and its two masks, where at one fixed normal the
        // gate is one number and one number is not a test of a mask, and
        // `study:concrete-cut-aware`, where at one fixed flag there is no
        // boundary in the frame to get wrong.
        let varying = Case::reads_the_mesh(&split);
        let case = Case::new(key, material, split);
        cases.push(if varying { case.varying() } else { case });
    }
    // Every graph the default library ships but the grass.
    assert_eq!(cases.len(), shipped().graphs.len() - 1);
    check(
        &gpu,
        &cases,
        "every shipped graph, with every parameter live",
    );
}

/// Every graph this repository ships to a game: the default library.
///
/// A compound is instanced into a building by a game that took it off the
/// shelf, and the only thing saying its lowering runs the same on their GPU as
/// it does in the bake that made their textures is this test.
fn shipped() -> MaterialGraphLibrary {
    ashlar_material::stdlib::graphs()
}

/// The default library, where the graphs with real relief live.
fn study() -> MaterialGraphLibrary {
    shipped()
}

// --------------------------------------------------------------------------
// The roughness, from further away than level 0
// --------------------------------------------------------------------------

/// The mip level the widening is measured at.
///
/// Three, so the footprint is sixty-four texels and the normals under it have
/// had room to disagree, and so the square compared is sixteen thousand texels
/// rather than a handful. Level 0 would measure nothing at all: the coherence
/// of unit normals is one and the Toksvig term is exactly zero there, which is
/// the whole reason this case needs a kernel that reads a level it is told
/// rather than the level a bake is.
///
/// Deeper is not better, and it was measured rather than assumed: the coherence
/// a footprint loses stops growing once the footprint is wider than the relief
/// under it. On this wall level 4 is worth a third of level 3 and level 5 a
/// fifth, over a quarter and a sixteenth as many texels.
const WIDENING_LEVEL: u32 = 3;

/// Texels per repeat the widening case bakes at.
///
/// A thousand and twenty-four, which is what
/// `ashlar_bevy::shader::DEFAULT_RESOLUTION` is and what the study ships at,
/// and the number matters here where it does not elsewhere. How much coherence
/// a footprint loses is a fact about *texel size*: a normal is a central
/// difference over two texels, so the same wall at 256 has a quarter of the
/// slope and its level 3 moves a roughness by a tenth of an eight-bit code —
/// which this case could then pass while widening nothing at all. At 1024 the
/// term is worth four codes, and being within half of one is a statement. It
/// costs about five seconds of the suite, nearly all of it the two CPU bakes.
const WIDENING_RESOLUTION: u32 = 1024;

#[test]
#[ignore = "needs a GPU; run it with `just conformance`"]
fn a_compiled_roughness_at_distance_is_the_bakes_roughness_at_distance() {
    // The one claim in this file that is not "the two backends compute the same
    // function at level 0". A baked material's roughness widens as the chain
    // goes down, because `mips::encode_mips` holds the normal plane and the
    // roughness plane at once and adds the Toksvig term for the coherence each
    // level lost. A compiled material binds separate images, so it carries that
    // coherence in the alpha of the normal map's own chain and the fragment
    // applies the same formula to whatever level it read.
    //
    // This dispatches the compiled `study:concrete` at level 3 and compares it,
    // texel for texel, with level 3 of the ORM the bake wrote. The budget is one
    // eight-bit code, which is what the ORM holds a roughness in: the two are
    // the same arithmetic over the same numbers, and all that is between them is
    // the bake's own quantisation.
    let gpu = Gpu::open();
    let library = study();
    let material = library
        .build("library:formed-concrete")
        .expect("the study's concrete builds");
    let split = partition(
        &material,
        &Target::shader_for(&material),
        WIDENING_RESOLUTION,
    )
    .expect("the study's concrete partitions");
    assert!(
        split.report().widens,
        "a wall with a bound normal map widens: {}",
        split.report()
    );
    let planes = rasterise_buffers(
        split.runtime(),
        split.resolution(),
        NonZeroUsize::new(THREADS),
        &mut BakeCache::new(),
    )
    .expect("the partition's planes rasterise");

    // A square the size level 3 is, so the kernel's texel centres are that
    // level's texel centres and the hardware's bilinear weights are exactly
    // one and zero.
    let side = WIDENING_RESOLUTION >> WIDENING_LEVEL;
    let kernel = wgsl::emit_compute_mesh(
        &split,
        side,
        std::slice::from_ref(&"roughness".to_owned()),
        MeshInputs::Uniform,
        WIDENING_LEVEL,
    )
    .expect("the partition emits a kernel");
    let uploads: Vec<Upload> = kernel
        .bindings()
        .iter()
        .map(|binding| upload_chain_of(binding, split.textures(), &planes))
        .collect();
    let values = live_values(&material, &split);
    let inputs = Inputs {
        time: 0.0,
        world_pos: [0.0; 3],
        world_normal: [0.0, 1.0, 0.0],
        cut_flag: 0.0,
        params: &values,
        buffers: &planes,
    };
    let params = kernel.layout().bytes(&values);
    let answers = dispatch(
        &gpu,
        &kernel,
        "library:formed-concrete at level 3",
        &uploads,
        &params,
        &inputs,
        None,
    );

    let (level, format) = baked_roughness(&library);
    let texels = (side as usize) * (side as usize);
    assert_eq!(answers.len(), texels, "one roughness per texel");

    let mut worst = 0.0_f32;
    let mut at = 0;
    for (texel, answer) in answers.iter().enumerate() {
        // Green is the roughness lane of an ORM.
        let difference = (decode(&level, texel * 4 + 1, format) - answer[0]).abs();
        if difference > worst {
            worst = difference;
            at = texel;
        }
    }
    let (lost, widened) = worth_of_the_term(&uploads, &kernel, split.widening());
    println!(
        "\nthe compiled study concrete at level {WIDENING_LEVEL}, on {}:\n  worst {worst:.3e} of \
         {:.3e} over {texels} texels, at {:?}\n  the widening it agrees on: {lost:.3e} of \
         coherence lost, {widened:.3e} of roughness at 0.85, {:.1} eight-bit codes",
        gpu.adapter,
        COLOUR_STEP,
        (at % side as usize, at / side as usize),
        widened / COLOUR_STEP
    );
    assert!(
        worst <= COLOUR_STEP,
        "the compiled roughness at level {WIDENING_LEVEL} differs from the bake's by \
         {worst:.4e}, which is past one eight-bit code"
    );
    level_zero_is_unmoved(&gpu, &split, &planes, &params, &inputs);
    assert!(
        widened > COLOUR_STEP,
        "the chain lost {lost:.3e} of coherence at level {WIDENING_LEVEL}, which moves a \
         0.85 roughness by {widened:.3e} — under one eight-bit code, so this case would \
         have passed with no widening at all"
    );
}

/// [`WIDENING_LEVEL`] of the ORM a CPU bake of `study:concrete` writes, with
/// the format to read its codes back in.
///
/// The reference this case compares against, and the reason it is a CPU bake
/// rather than the shipped KTX2: the file ships at whatever resolution the
/// content step chose and this has to be the same square the partition's planes
/// are. The bake is the reference in this workspace, and this is it.
fn baked_roughness(
    library: &MaterialGraphLibrary,
) -> (Vec<u8>, ashlar_material::bake::PlaneFormat) {
    let baked = ashlar_material::bake::bake(&ashlar_material::bake::BakeRequest {
        graph: library
            .get("library:formed-concrete")
            .expect("the study ships its concrete"),
        library,
        params: &std::collections::BTreeMap::new(),
        resolution: WIDENING_RESOLUTION,
        mips: true,
        threads: NonZeroUsize::new(THREADS),
    })
    .expect("the study's concrete bakes");
    (
        baked.orm.mips[WIDENING_LEVEL as usize].clone(),
        baked.orm.format,
    )
}

/// What the widening was worth at [`WIDENING_LEVEL`]: the most coherence any
/// footprint lost, and what that moves a cast concrete wall's roughness by.
///
/// Read off the chain that carried it rather than off the answers, because the
/// answers are the two sides *agreeing*: a run where the coherence never
/// reached the shader at all would agree just as well, and would be agreeing
/// about nothing. `1 - |n_avg|` is the whole of the Toksvig input, so this is
/// the number that says the case had something to measure.
fn worth_of_the_term(
    uploads: &[Upload],
    kernel: &ComputeShader,
    normal: Option<ashlar_material::ir::BufferId>,
) -> (f32, f32) {
    let lost = uploads
        .iter()
        .zip(kernel.bindings())
        .find(|(_, binding)| binding.buffers.iter().any(|buffer| Some(*buffer) == normal))
        .map(|(upload, _)| &upload.levels[WIDENING_LEVEL as usize])
        .expect("the normal map is bound, because the widening reads it")
        .iter()
        .map(|texel| 1.0 - texel[3])
        .fold(0.0_f32, f32::max);
    // `r' - r` at the roughness a cast concrete wall actually has, which is
    // what the picture moves by.
    let rough = 0.85_f32;
    let widened = rough
        .mul_add(rough, lost * ashlar_material::mips::TOKSVIG_K)
        .sqrt()
        - rough;
    (lost, widened)
}

/// The same partition at level 0, against the interpreter.
///
/// The other half of the case above, and the one that says a failure there is a
/// failure of the *widening* rather than of the material. At level 0 every
/// normal is a unit vector, so the coherence is one and the Toksvig term is
/// exactly zero; a kernel that reads that level has to answer what the
/// interpreter answers, to the same budget every other roughness in this file
/// is held to.
fn level_zero_is_unmoved(
    gpu: &Gpu,
    split: &Partition,
    planes: &[Plane],
    params: &[u8],
    inputs: &Inputs<'_>,
) {
    let kernel = wgsl::emit_compute_mesh(
        split,
        WIDENING_RESOLUTION,
        std::slice::from_ref(&"roughness".to_owned()),
        MeshInputs::Uniform,
        0,
    )
    .expect("the partition emits a level-0 kernel");
    let uploads: Vec<Upload> = kernel
        .bindings()
        .iter()
        .map(|binding| upload_of(binding, split.textures(), planes))
        .collect();
    let answers = dispatch(
        gpu,
        &kernel,
        "library:formed-concrete at level 0",
        &uploads,
        params,
        inputs,
        None,
    );
    let reference = cpu_reference(
        split.runtime(),
        &[split
            .runtime()
            .root("roughness")
            .expect("concrete has a roughness")],
        WIDENING_RESOLUTION,
        inputs,
        None,
        THREADS,
    );
    let worst = reference
        .iter()
        .zip(&answers)
        .map(|(mine, theirs)| (mine[0] - theirs[0]).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        worst <= RAW_TOLERANCE,
        "the widening moved level 0, where the coherence is one and it must be nothing: \
         {worst:.4e}"
    );
}

// --------------------------------------------------------------------------
// The hash, which is the one thing that has to be exact
// --------------------------------------------------------------------------

/// A kernel over [`wgsl::HASH`] verbatim, answering both hashes for a table of
/// lattice cells.
///
/// Nothing generated: the text under test is the constant every emitted module
/// carries, and the only thing wrapped around it is an index and a store. The
/// table is bound as a storage buffer of `vec4<f32>` — `(x, y, z, seed)` — with
/// the period in a second, so the driver cannot constant-fold the answer.
const HASH_KERNEL: &str = r"
@group(0) @binding(0) var<storage, read_write> answers: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read> cells: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> periods: array<vec4<f32>>;

@compute @workgroup_size(64, 1, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= arrayLength(&cells) {
        return;
    }
    let cell = cells[id.x];
    let period = periods[id.x];
    let seed = bitcast<u32>(i32(cell.w));
    let two = ashlar_hash2(cell.xy, period.xy, seed);
    let three = ashlar_hash3(cell.xyz, period.xyz, seed);
    answers[id.x] = vec4<f32>(
        two,
        three,
        round(two * ASHLAR_HASH_SCALE),
        round(three * ASHLAR_HASH_SCALE),
    );
}
";

#[test]
#[ignore = "needs a GPU; run it with `just conformance`"]
fn the_lattice_hashes_are_bit_exact() {
    let gpu = Gpu::open();
    let mut cells: Vec<[f32; 4]> = Vec::new();
    let mut periods: Vec<[f32; 4]> = Vec::new();
    // Cells on both sides of zero, at and past the period, and at the two
    // seams; periods that reduce and a period below one, which asks for no
    // reduction at all; seeds that reach the top of the word.
    for seed in [0_u32, 1, 7, 1_103_515_245, u32::MAX] {
        for period in [
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [8.0, 4.0, 3.0],
            [128.0, 128.0, 128.0],
        ] {
            for cell in [
                [0.0, 0.0, 0.0],
                [1.0, 2.0, 3.0],
                [7.0, 3.0, 2.0],
                [8.0, 4.0, 3.0],
                [-1.0, -1.0, -1.0],
                [-9.0, -5.0, -4.0],
                [127.0, 127.0, 127.0],
                [255.0, 511.0, 1023.0],
                [0.5, 1.5, -0.5],
                [-0.25, 12.75, 6.5],
            ] {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "the seed goes to the shader as the bits it is, through i32"
                )]
                let seed_lane = i32::from_ne_bytes(seed.to_ne_bytes()) as f32;
                cells.push([cell[0], cell[1], cell[2], seed_lane]);
                periods.push([period[0], period[1], period[2], 0.0]);
            }
        }
    }
    let source = format!("{HASH}{HASH_KERNEL}");
    let answers = dispatch_hash(&gpu, &source, &cells, &periods);

    let mut worst_f32 = 0_u32;
    for (index, (cell, period)) in cells.iter().zip(&periods).enumerate() {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the seed went out as the whole number its bits are, through i32"
        )]
        let seed = u32::from_ne_bytes((cell[3] as i32).to_ne_bytes());
        let mine2 = hash2(
            cell_of(cell[0], period[0]),
            cell_of(cell[1], period[1]),
            seed,
        );
        let mine3 = hash3(
            cell_of(cell[0], period[0]),
            cell_of(cell[1], period[1]),
            cell_of(cell[2], period[2]),
            seed,
        );
        let theirs = answers[index];
        assert_eq!(
            mine2.to_bits(),
            theirs[0].to_bits(),
            "hash2 at cell {cell:?} period {period:?}: {mine2} against {}",
            theirs[0]
        );
        assert_eq!(
            mine3.to_bits(),
            theirs[1].to_bits(),
            "hash3 at cell {cell:?} period {period:?}: {mine3} against {}",
            theirs[1]
        );
        // And the same claim on the integers the floats are a rescaling of,
        // which is what "bit-exact on the u32 output" means when the op's
        // answer is a float: `f32(bits) / 2^32` is exact both ways, so the
        // round trip recovers the same word on both sides or neither.
        let mine_bits = [scaled(mine2), scaled(mine3)];
        let their_bits = [scaled_back(theirs[2]), scaled_back(theirs[3])];
        assert_eq!(
            mine_bits, their_bits,
            "the hashed word at cell {cell:?} period {period:?}"
        );
        worst_f32 = worst_f32.max((mine2.to_bits()).abs_diff(theirs[0].to_bits()));
    }
    println!(
        "\nthe lattice hash, on {}: {} rows, worst difference {worst_f32} ulp",
        gpu.adapter,
        cells.len()
    );
}

/// The reduction the hash applies before it hashes, as the interpreter does it.
///
/// Its own `cell` is crate-private; this is the same arithmetic, and the test
/// above is what says the shader's is too.
fn cell_of(coordinate: f32, period: f32) -> u32 {
    let floored = coordinate.floor();
    let reduced = if period >= 1.0 {
        let remainder = floored % period;
        if remainder < 0.0 {
            remainder + period.abs()
        } else {
            remainder
        }
    } else {
        floored
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a lattice cell, which the shader converts the same way"
    )]
    u32::from_ne_bytes((reduced as i32).to_ne_bytes())
}

/// A hash back to the word it came from.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a hash in 0..1 scaled by 2^32, which is what the shader writes"
)]
fn scaled(value: f32) -> u32 {
    (value * 4_294_967_296.0).round() as u32
}

/// The same, for the word the shader already rounded.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a whole number the shader rounded, read back"
)]
fn scaled_back(value: f32) -> u32 {
    value as u32
}

/// Dispatch the hash kernel over one table and read its answers back.
///
/// Its own small plumbing rather than [`dispatch`]'s, because it binds three
/// storage buffers and no textures, no uniforms and no generated layout.
fn dispatch_hash(
    gpu: &Gpu,
    source: &str,
    cells: &[[f32; 4]],
    periods: &[[f32; 4]],
) -> Vec<[f32; 4]> {
    let device = &gpu.device;
    let module = gpu.module(source, "the lattice hash");
    let storage = |binding: u32, read_only: bool| BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::COMPUTE,
        ty: BindingType::Buffer {
            ty: BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(
        "the lattice hash",
        &[storage(0, false), storage(1, true), storage(2, true)],
    );
    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("the lattice hash"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&RawComputePipelineDescriptor {
        label: Some("the lattice hash"),
        layout: Some(&pipeline_layout),
        module: &module,
        entry_point: Some("main"),
        compilation_options: PipelineCompilationOptions::default(),
        cache: None,
    });

    let bytes = |table: &[[f32; 4]]| -> Vec<u8> {
        table
            .iter()
            .flat_map(|row| row.iter().flat_map(|lane| lane.to_le_bytes()))
            .collect()
    };
    let size = (cells.len() * 16) as u64;
    let answers = device.create_buffer(&BufferDescriptor {
        label: Some("answers"),
        size,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("readback"),
        size,
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let table = |label: &'static str, rows: &[[f32; 4]]| {
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue.write_buffer(&buffer, 0, &bytes(rows));
        buffer
    };
    let cell_buffer = table("cells", cells);
    let period_buffer = table("periods", periods);
    let bind_group = device.create_bind_group(
        "the lattice hash",
        &layout,
        &[
            BindGroupEntry {
                binding: 0,
                resource: answers.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: cell_buffer.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 2,
                resource: period_buffer.as_entire_binding(),
            },
        ],
    );

    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("the lattice hash"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("the lattice hash"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &*bind_group, &[]);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a table of a few hundred rows"
        )]
        pass.dispatch_workgroups((cells.len() as u32).div_ceil(64), 1, 1);
    }
    encoder.copy_buffer_to_buffer(&answers, 0, &readback, 0, size);
    gpu.queue.submit([encoder.finish()]);
    read_back(gpu, &readback)
}

// --------------------------------------------------------------------------
// The GPU bake, against the bake it is a second backend for
// --------------------------------------------------------------------------

/// Texels per repeat the two bakes are compared at.
///
/// Twice [`RESOLUTION`], because a bake is the thing being measured here rather
/// than one op, and a 512 bake is the smallest one that has a mip chain worth
/// comparing — nine levels, with the Toksvig widening on every one of them.
const BAKE_RESOLUTION: u32 = 512;

/// Keep the expected fallback explicit even in CI without a GPU. A new
/// resource-limit refusal must not silently reduce conformance coverage.
#[test]
fn only_the_known_bakes_require_cpu_fallback_at_the_bake_resolution() {
    let library = study();
    let params = std::collections::BTreeMap::new();
    let mut fallback = Vec::new();
    for (key, graph) in &library.graphs {
        let Ok(plan) = ashlar_material::bake::plan(&ashlar_material::bake::BakeRequest {
            graph,
            library: &library,
            params: &params,
            resolution: BAKE_RESOLUTION,
            mips: true,
            threads: NonZeroUsize::new(THREADS),
        }) else {
            continue;
        };
        if !ashlar_bevy::gpu::dispatchable(&plan.ir, BAKE_RESOLUTION) {
            fallback.push(key.as_str());
        }
    }
    assert_eq!(fallback, CPU_FALLBACKS);
}

/// These relief-heavy graphs exceed the GPU's binding budget at 512 texels.
const CPU_FALLBACKS: [&str; 2] = [
    "library:soi-cobblestone-moss-heavy",
    "library:soi-cobblestone-moss-light",
];

/// What one map of a bake is allowed to differ by, as a fraction of its own
/// full scale.
///
/// The budgets are [`COLOUR_STEP`], [`NORMAL_STEP`] and [`RAW_TOLERANCE`]
/// again, plus one quantisation step of the format the map ships in — because
/// what is compared here is the *encoded bytes*, and two values a hair either
/// side of a rounding boundary are one code apart however close they were.
fn map_budget(map: &str, format: ashlar_material::bake::PlaneFormat) -> f32 {
    use ashlar_material::bake::PlaneFormat::{R16Unorm, Rgba8Srgb, Rgba8Unorm, Rgba16Float};
    let step = match format {
        Rgba8Srgb | Rgba8Unorm => 1.0 / 255.0,
        R16Unorm => 1.0 / 65535.0,
        // Half floats near one; the emissive map is the only one in them and
        // this is the step at the magnitudes a diffuser reaches.
        Rgba16Float => 1.0 / 1024.0,
    };
    let budget = match map {
        "base_color" | "emissive" => COLOUR_STEP,
        // Derived twice over: the normal is a difference of the height plane,
        // and the roughness in the ORM is widened by how short the mean of
        // those normals got, so a difference in the height reaches both of them
        // multiplied.
        "normal" | "orm" => NORMAL_STEP,
        _ => RAW_TOLERANCE,
    };
    budget + step
}

/// One encoded texel lane as the number it stands for, in `0..=1` for the
/// unorm formats and as itself for the half floats.
fn decode(bytes: &[u8], index: usize, format: ashlar_material::bake::PlaneFormat) -> f32 {
    use ashlar_material::bake::PlaneFormat::{R16Unorm, Rgba8Srgb, Rgba8Unorm, Rgba16Float};
    match format {
        Rgba8Srgb | Rgba8Unorm => f32::from(bytes.get(index).copied().unwrap_or_default()) / 255.0,
        R16Unorm => {
            let word = u16::from_le_bytes([
                bytes.get(index * 2).copied().unwrap_or_default(),
                bytes.get(index * 2 + 1).copied().unwrap_or_default(),
            ]);
            f32::from(word) / 65535.0
        }
        Rgba16Float => half_to_f32(u16::from_le_bytes([
            bytes.get(index * 2).copied().unwrap_or_default(),
            bytes.get(index * 2 + 1).copied().unwrap_or_default(),
        ])),
    }
}

/// An IEEE half as an `f32`.
///
/// Written out rather than taken from a crate: `ashlar-material` encodes half
/// floats by hand for the same reason — one direction of one format is not a
/// dependency — and this is the other direction, used by nothing but this
/// comparison.
fn half_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits >> 15) << 31;
    let exponent = u32::from((bits >> 10) & 0x1f);
    let mantissa = u32::from(bits & 0x3ff);
    let word = match exponent {
        0 if mantissa == 0 => sign,
        // Subnormal: normalise it by hand. A bake writes one only where a value
        // is under 6e-5, which is a black texel either way.
        0 => {
            return f32::from_bits(sign | ((127 - 15 + 1) << 23) | (mantissa << 13)) - {
                f32::from_bits(sign | ((127 - 15 + 1) << 23))
            };
        }
        0x1f => sign | (0xff << 23) | (mantissa << 13),
        _ => sign | ((exponent + 127 - 15) << 23) | (mantissa << 13),
    };
    f32::from_bits(word)
}

/// Every map of a texture set, by the name the comparison reports it under.
fn maps(
    set: &ashlar_material::bake::TextureSet,
) -> Vec<(&'static str, &ashlar_material::bake::Encoded)> {
    let mut maps: Vec<(&'static str, &ashlar_material::bake::Encoded)> = vec![
        ("base_color", &set.base_color),
        ("normal", &set.normal),
        ("orm", &set.orm),
    ];
    if let Some(height) = set.height.as_ref() {
        maps.push(("height", height));
    }
    if let Some(emissive) = set.emissive.as_ref() {
        maps.push(("emissive", emissive));
    }
    maps
}

/// The GPU bake against the CPU bake, map for map and level for level.
///
/// The claim phase 4's first item makes: the same graph baked through
/// [`ashlar_material::wgsl::emit_compute_ports`] on the device is the same
/// texture set the interpreter writes, inside the budget `just conformance`
/// states between the two backends everywhere else. It is the *bytes* that are
/// compared, chain included, because bytes are what a `Surface::Graph` wall
/// actually binds — so this covers the mip filter, the Toksvig widening and the
/// quantisation as well as the expression.
///
/// It prints what each bake cost, which is the other half of the point: a
/// runtime bake is paid for in front of a frame, and the number an author cares
/// about is the ratio.
///
/// It also counts what each bake *uploaded*, which is the one claim about a
/// GPU bake that texels cannot carry. A plane goes to the next dispatch as
/// sixteen bytes a texel, and packing a megatexel of them and writing it costs
/// more than the dispatch that reads it: about 37 ms against about 60 ms at
/// 1024 squared in a debug build. There were two caches, one per pass, so the
/// outputs dispatch packed and wrote every plane the plane dispatches had just
/// put on the device a second time — `study:painted-metal` uploaded nine images
/// where it binds five. A bake that uploads a plane twice answers exactly the
/// texels a bake that uploads it once does, only slower, so
/// [`GpuBaker::uploads`](ashlar_bevy::gpu::GpuBaker::uploads) is the only thing
/// that can fail when the sharing stops working.
///
/// The count rides here rather than in a case of its own because this case
/// already bakes every study graph on the device, and because every test in
/// this file opens an adapter of its own and they run in parallel: the suite is
/// intermittently unstable that way — see the phase capture's Limits — and a
/// fifth device is one more chance at it. [`partial_rasterisation`] rides here
/// for the same reason: it is the only automated thing that runs
/// [`GpuBaker::planes_wanted`](ashlar_bevy::gpu::GpuBaker::planes_wanted) with
/// a mask that skips something.
#[test]
#[ignore = "needs a GPU; run it with `just conformance`"]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end comparison of CPU, GPU and fallback outputs"
)]
fn the_gpu_bake_is_the_cpu_bake_within_the_conformance_budget() {
    use ashlar_bevy::{gpu::GpuBaker, runtime_bake::bake_images};

    let gpu = Gpu::open();
    let baker = GpuBaker::new(gpu.device.clone(), gpu.queue.clone());
    let library = study();
    let threads = NonZeroUsize::new(THREADS);
    let mut report = String::new();
    let _ = writeln!(
        report,
        "\nthe GPU bake against the CPU bake at {BAKE_RESOLUTION} texels, on {}:",
        gpu.adapter
    );
    let mut failures: Vec<String> = Vec::new();
    let mut compared = 0;
    let mut fallbacks = 0;
    // The lowerings this case baked, kept so that the partial rasterisation
    // below has real graphs to ask for half of.
    let mut lowered: Vec<(String, ashlar_material::ir::Ir)> = Vec::new();
    for key in library.graphs.keys() {
        let bake = ashlar::Bake {
            graph: key.clone(),
            params: std::collections::BTreeMap::new(),
            resolution: BAKE_RESOLUTION,
        };
        let started = std::time::Instant::now();
        let Ok(mine) = ashlar_material::bake::bake(&ashlar_material::bake::BakeRequest {
            graph: library.get(key).unwrap(),
            library: &library,
            params: &std::collections::BTreeMap::new(),
            resolution: BAKE_RESOLUTION,
            mips: true,
            threads,
        }) else {
            // Not a case: either a graph whose finest lattice is past this
            // resolution, or one a bake refuses outright — `study:concrete-wet`
            // reads the fragment's own world normal and height, and a GPU bake
            // is a bake.
            let _ = writeln!(report, "  {key:<28} skipped: this graph does not bake");
            continue;
        };
        let cpu = started.elapsed();
        // Through the adapter rather than through the crate, so what is timed
        // is what a `Surface::Graph` wall pays: the bake, the chain, the
        // encoding and the `Image`s.
        let started = std::time::Instant::now();
        let cpu_textures = bake_images(&bake, &library, threads).expect("the CPU path bakes");
        let cpu_images = started.elapsed();
        let started = std::time::Instant::now();
        let gpu_textures = ashlar_bevy::gpu::bake_images_gpu(&bake, &library, &baker, threads)
            .expect("the GPU path bakes");
        let gpu_images = started.elapsed();

        let uploaded_before = baker.uploads();
        let started = std::time::Instant::now();
        let plan = plan_of(&library, key, threads);
        // These graphs deliberately use the adapter's CPU fallback: their
        // relief exceeds the GPU binding budget. Hold that fallback to exact
        // bytes rather than calling the lower-level GPU-only method as if the
        // graph were dispatchable. Every other graph must still dispatch.
        if CPU_FALLBACKS.contains(&key.as_str()) {
            assert!(
                !ashlar_bevy::gpu::dispatchable(&plan.ir, BAKE_RESOLUTION),
                "{key}"
            );
            assert_fallback_images(&cpu_textures, &gpu_textures);
            fallbacks += 1;
            let _ = writeln!(
                report,
                "  {key:<28} CPU fallback: all maps and mips byte-exact"
            );
            continue;
        }
        let theirs = baker
            .rasterise(&plan, BAKE_RESOLUTION, threads)
            .unwrap_or_else(|error| panic!("{key}: the GPU bakes the same graph: {error:#}"))
            .encode_mips(ashlar_material::bake::Dither::Ordered);
        let gpu_time = started.elapsed();
        // Counted after the clock is read, so that asking the question costs
        // the timing nothing. Planning uploads nothing, so the difference is
        // the bake's.
        let uploads = baker.uploads() - uploaded_before;
        let images = images_uploaded_once(key, &plan.ir, uploads);

        let mut worst = 0.0_f32;
        let mut worst_map = "";
        let mut worst_level = 0;
        for ((map, left), (_, right)) in maps(&mine).into_iter().zip(maps(&theirs)) {
            let budget = map_budget(map, left.format);
            for (level, (ours, hers)) in left.mips.iter().zip(&right.mips).enumerate() {
                assert_eq!(
                    ours.len(),
                    hers.len(),
                    "{key} {map} level {level}: the two bakes wrote different lengths"
                );
                let lanes = ours.len() / left.format.bytes_per_texel().max(1) * 4;
                let mut level_worst = 0.0_f32;
                for lane in 0..lanes {
                    let difference =
                        (decode(ours, lane, left.format) - decode(hers, lane, left.format)).abs();
                    level_worst = level_worst.max(difference);
                }
                if level_worst > worst {
                    worst = level_worst;
                    worst_map = map;
                    worst_level = level;
                }
                if level_worst > budget || !level_worst.is_finite() {
                    failures.push(format!(
                        "{key}: {map} level {level} differs by {level_worst:.4e}, which is past \
                         {budget:.4e}"
                    ));
                }
            }
        }
        compared += 1;
        let _ = writeln!(
            report,
            "  {key:<28} cpu {cpu:>9.2?} ({cpu_images:>9.2?} with images)  gpu {gpu_time:>9.2?} \
             ({gpu_images:>9.2?} with images)  {uploads} uploads of {images} images  worst \
             {worst_map} L{worst_level} {worst:.3e}"
        );
        lowered.push((key.clone(), plan.ir));
    }
    let _ = writeln!(
        report,
        "  {}",
        partial_rasterisation(&baker, &lowered, threads)
    );
    println!("{report}");
    assert!(compared > 0, "no study graph baked at {BAKE_RESOLUTION}");
    assert_eq!(
        fallbacks,
        CPU_FALLBACKS.len(),
        "every fallback was exercised"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A resource-limited graph takes the public adapter's CPU path unchanged.
fn assert_fallback_images(
    cpu: &ashlar_bevy::runtime_bake::GraphTextures,
    fallback: &ashlar_bevy::runtime_bake::GraphTextures,
) {
    for (name, expected, actual) in [
        ("base", Some(&cpu.base_color), Some(&fallback.base_color)),
        ("normal", Some(&cpu.normal), Some(&fallback.normal)),
        ("orm", Some(&cpu.orm), Some(&fallback.orm)),
        ("height", cpu.height.as_ref(), fallback.height.as_ref()),
        (
            "emissive",
            cpu.emissive.as_ref(),
            fallback.emissive.as_ref(),
        ),
    ] {
        assert_eq!(expected.is_some(), actual.is_some(), "{name}");
        if let (Some(expected), Some(actual)) = (expected, actual) {
            assert_eq!(
                expected.texture_descriptor, actual.texture_descriptor,
                "{name}"
            );
            assert_eq!(expected.data, actual.data, "{name}");
        }
    }
}

/// The lowering of one study graph at [`BAKE_RESOLUTION`], planned as a bake.
fn plan_of(
    library: &MaterialGraphLibrary,
    key: &str,
    threads: Option<NonZeroUsize>,
) -> ashlar_material::bake::Plan {
    ashlar_material::bake::plan(&ashlar_material::bake::BakeRequest {
        graph: library.get(key).unwrap(),
        library,
        params: &std::collections::BTreeMap::new(),
        resolution: BAKE_RESOLUTION,
        mips: true,
        threads,
    })
    .expect("a graph that baked plans")
}

/// That asking the GPU for some of a graph's planes answers exactly the planes
/// asking for all of them did.
///
/// The CPU twin of this is pinned texel for texel in `ashlar-material`'s own
/// tests; this is the half that needs a device. It matters because a skipped
/// plane is not an absence but a one-texel placeholder — the list has to stay
/// parallel to `Ir::buffers` — so a mistake in the closure would be silent
/// texels rather than an error, and `just references` is the only other thing
/// that runs this path and it asserts nothing about texels.
///
/// It runs on one graph, at the smallest resolution a bake allows, and on the
/// baker this case already opened: the suite is intermittently unstable with
/// four devices in parallel, and this is not worth a fifth.
fn partial_rasterisation(
    baker: &ashlar_bevy::gpu::GpuBaker,
    lowered: &[(String, ashlar_material::ir::Ir)],
    threads: Option<NonZeroUsize>,
) -> String {
    use ashlar_material::planes::plane_closure;

    // The plan whose closure is the largest *proper* part of its own plan list,
    // over every graph this case baked. Proper because a mask that needed every
    // plan would run the whole thing and prove nothing; largest because the
    // interesting mask is one that both pulls dependencies in and leaves planes
    // out, rather than a plan that reads nothing.
    let subject = lowered
        .iter()
        .filter_map(|(key, ir)| {
            let plans = ir.buffers().len();
            (0..plans)
                .map(|at| (0..plans).map(|index| index == at).collect::<Vec<_>>())
                .map(|wanted| {
                    let size = plane_closure(ir, &wanted).iter().filter(|n| **n).count();
                    (size, wanted)
                })
                .filter(|(size, _)| *size < plans)
                .max_by_key(|(size, _)| *size)
                .map(|(size, wanted)| (size, key, ir, wanted))
        })
        .max_by_key(|(size, ..)| *size);
    let Some((_, key, ir, wanted)) = subject else {
        return "no study graph has a plane it can be asked for on its own".to_string();
    };
    let resolution = ashlar_material::bake::MIN_RESOLUTION;
    let all = baker
        .planes(ir, resolution, threads)
        .expect("the GPU rasterises every plane of a graph that baked");
    let some = baker
        .planes_wanted(ir, resolution, &wanted, threads)
        .expect("the GPU rasterises the planes it was asked for");
    let needed = plane_closure(ir, &wanted);
    assert_eq!(some.len(), all.len(), "{key}: a plane went missing");
    let mut ran = 0;
    for (index, (whole, part)) in all.iter().zip(&some).enumerate() {
        match part {
            Some(plane) => {
                ran += 1;
                assert_eq!(
                    plane.lanes(),
                    whole.lanes(),
                    "{key}: plane {index} is not the plane a whole rasterisation drew"
                );
            }
            None => assert!(
                !needed[index],
                "{key}: plane {index} was needed and was skipped"
            ),
        }
    }
    assert!(
        ran < all.len(),
        "{key}: every plane ran, so nothing was skipped to get wrong"
    );
    format!(
        "{key:<28} asked for {ran} of {} planes on the device, and got the same texels",
        all.len()
    )
}

/// How many images one GPU bake binds, having checked that it uploaded each of
/// them as few times as the cache allows.
///
/// Every image a split gives an id to is read by some dispatch — a plane no
/// instruction samples is given none — so a bake that uploads each image on its
/// first bind and binds the one it has after that uploads exactly as many as it
/// has ids.
///
/// "One upload per image" is only that simple while an image holds one plane.
/// A GPU baker caches an image the first time it binds one whose every lane
/// already exists, and cannot cache one with a lane still to come — what is in
/// that texture now is not what will be in it — so a *packed* image is uploaded
/// again, on purpose, for every dispatch that reads it early.
/// `study:rusted-steel` is the first study graph to pack: ten planes is past
/// [`MAX_BOUND_TEXTURES`](ashlar_material::partition::MAX_BOUND_TEXTURES), so
/// the split folds its scalars four to an image and the flat count stopped
/// being the claim.
///
/// So the expectation is computed rather than counted. A GPU bake dispatches
/// one kernel per plane, in buffer order, and then its outputs over the
/// finished planes; `emit_compute_ports` declares only the images the ports it
/// is given actually sample. Walking that same order with the same cache rule
/// gives the number of uploads a working cache makes, packing and all, and the
/// baker's own tally has to be it. A cache that stopped sharing overshoots this
/// exactly as it overshot the old constant.
///
/// The rule is about the *image's* lanes and not the binding's, which is the
/// distinction the baker got wrong until phase 4: a kernel that samples one
/// lane of a four-lane image has every lane its binding names, and three lanes
/// it has never seen.
fn images_uploaded_once(key: &str, ir: &ashlar_material::ir::Ir, uploads: u64) -> usize {
    use ashlar_material::partition::{is_plane_port, plane_port};

    let split = ashlar_material::partition::over_planes(ir, BAKE_RESOLUTION)
        .expect("a graph that baked splits into planes");
    let images: BTreeSet<u32> = split
        .textures()
        .iter()
        .filter_map(|texture| texture.id)
        .collect();

    // Which planes each image packs, which is what "complete" is about.
    let lanes_of = |image: u32| -> Vec<usize> {
        split
            .textures()
            .iter()
            .filter(|texture| texture.id == Some(image))
            .map(|texture| texture.buffer.index())
            .collect()
    };
    let mut cached: BTreeSet<u32> = BTreeSet::new();
    let mut expected = 0_u64;
    // The plane pass, one dispatch per buffer in order. An image is complete
    // when every plane it packs is earlier than this one, which is the same
    // test the baker makes against the planes it has so far.
    for (index, plan) in split.runtime().buffers().iter().enumerate() {
        let size = plan.resolution.unwrap_or(split.resolution()).max(1);
        let kernel = ashlar_material::wgsl::emit_compute_ports(&split, size, &[plane_port(index)])
            .unwrap_or_else(|error| {
                panic!("{key}: emitting the kernel for plane {index}: {error}")
            });
        for binding in kernel.bindings() {
            let complete = lanes_of(binding.image).iter().all(|lane| *lane < index);
            if !complete || cached.insert(binding.image) {
                expected += 1;
            }
        }
    }
    // The outputs pass. Every plane exists by now, so every binding is complete
    // and only an image nothing above it read is still an upload. Which batch a
    // port lands in cannot change that, so the ports are emitted one at a time
    // rather than in the device-sized chunks the baker dispatches.
    for port in split.runtime().roots().keys() {
        if is_plane_port(port) {
            continue;
        }
        let kernel = ashlar_material::wgsl::emit_compute_ports(
            &split,
            BAKE_RESOLUTION,
            std::slice::from_ref(port),
        )
        .unwrap_or_else(|error| panic!("{key}: emitting the kernel for {port}: {error}"));
        for binding in kernel.bindings() {
            if cached.insert(binding.image) {
                expected += 1;
            }
        }
    }
    assert_eq!(
        cached.len(),
        images.len(),
        "{key}: the split gave {} images an id and the dispatches bound {}, so an image nothing \
         reads was given one",
        images.len(),
        cached.len()
    );
    assert_eq!(
        uploads,
        expected,
        "{key} binds {} images and uploaded {uploads} of them, where a cache that shares every \
         finished image uploads {expected}",
        images.len()
    );
    images.len()
}
