//! Baking a material graph on the GPU, through the emitter the shader uses.
//!
//! The CPU bake in [`ashlar_material::bake`] is the reference and runs
//! anywhere. It is also the slow half of everything this crate does in front of
//! a frame: a `Surface::Graph` wall and the static half of a compiled one are
//! both an interpreter walking a few hundred instructions over a megatexel,
//! and at 1024 squared in a debug build that is seconds rather than
//! milliseconds. The same expression is already printed as WGSL for the
//! fragment shader, and [`wgsl::emit_compute_ports`] prints it as a compute
//! kernel, so the device that is going to display the texels can compute them.
//!
//! # What runs where
//!
//! A bake is not one kernel, because a buffered filter is not an expression: a
//! blur, a horizon march or a jump flood reads a neighbourhood and is a
//! *plane*. So a GPU bake is a sequence, and
//! [`ashlar_material::partition::over_planes`] is what says what
//! the sequence is:
//!
//! 1. For each plane, in the dependency order the lowering already put them
//!    in: dispatch a kernel that evaluates that plane's own sub-expression over
//!    the planes before it, read the texels back, and run the filter on the CPU
//!    over the read-back `f32`s — the same [`planes::filter`] pass the CPU bake
//!    runs, bit for bit, because a jump flood is not something a tolerance
//!    covers.
//! 2. Dispatch the material's outputs over the finished planes, in as few
//!    dispatches as the device's storage budget allows.
//! 3. Derive the normal, build the mip chain, widen the roughness and encode —
//!    all on the CPU, over the read-back `f32` planes, through exactly the code
//!    a CPU bake uses.
//!
//! Step 3 stays on the CPU on purpose and it is measured rather than assumed.
//! At 1024 squared the whole post-pass — the four-tap normal, eleven mip
//! levels, Toksvig and the quantisation of five maps — is about a third of a
//! second beside the one to three seconds the expression itself took, and each
//! of its passes is a filter over a shrinking pyramid rather than a graph. It
//! is also the part where the two backends are required to be *identical*
//! rather than merely close: the normal is a wrapped central difference over
//! the height plane, so a GPU bake and a CPU bake of the same height plane
//! derive the same normal by construction. `docs/guide/bevy-adapter.md`
//! carries the measured numbers.
//!
//! # What is uploaded, and at what precision
//!
//! A plane goes to the next dispatch as an `Rgba32Float` texture holding the
//! numbers the plane holds — not as the eight-bit image a shipped material
//! binds. An intermediate plane of a bake is not a texture anybody looks at; it
//! is a value on the way to one, and quantising it between dispatches would be
//! a different bake rather than the same one at another place.
//! [`ashlar_material::partition::over_planes`] gives every bound
//! texture the one format the emitter decodes nothing for, so the shader reads
//! back exactly what was written.
//!
//! And **once per bake**, which is a measurement rather than a tidiness. An
//! upload is not the cheap half of a dispatch: packing a megatexel plane into
//! four-lane `f32` bytes and writing it is about 37 ms at 1024 squared in a
//! debug build, against about 60 ms for the dispatch that reads it, so of
//! a painted-metal graph's 1.18 s bake nine uploads of five images were 330 ms.
//! One cache of uploaded textures spans the whole of [`GpuBaker::rasterise`] —
//! plane dispatches and the outputs dispatch, which reads what they just wrote
//! — and [`GpuBaker::uploads`] is what the conformance test holds to one upload
//! per bound image.
//!
//! It stops at the bake, and that is also measured. Keeping the textures on a
//! [`GpuBaker`] across bakes, keyed by
//! [`PlaneKey`](ashlar_material::planes::PlaneKey), would be correct — a key
//! names a plane's texels exactly — and would still pay for the *dispatch* that
//! recomputed the plane, because nothing on this side caches planes. It would
//! buy back one upload of an unchanged plane, 37 ms of the ~100 ms that plane
//! costs, and it would hold 16 MB of device memory per plane on a `Resource`
//! that lives as long as the app and has nothing to evict with. The cache that
//! would pay is a plane cache — [`ashlar_material::planes::BakeCache`] is
//! exactly that on the CPU — and it is a different item.
//!
//! # Where the two backends are allowed to differ
//!
//! Exactly where `just conformance` says they are, and for the same two
//! reasons: `sin` and `cos`, whose accuracy WGSL states in ULP, and the
//! hardware's bilinear weights, which carry eight fractional bits of subtexel
//! precision where [`Plane::sample`](ashlar_material::interp::Plane::sample)
//! carries all of them — so a graph that samples a plane through a `Warp` is
//! where a difference shows. The CPU bake stays the reference, the shipped
//! files are written by it, and
//! `the_gpu_bake_is_the_cpu_bake_within_the_conformance_budget` in
//! `tests/conformance.rs` is what says the two agree.
//!
//! # Refusals
//!
//! A graph the GPU path cannot take is an error rather than a wrong picture,
//! and every caller falls back to the CPU on one: more bound planes at once
//! than a bind group holds, or a resolution whose output buffer is past the
//! device's storage limit.
//!
//! A *device* the GPU path cannot take is refused earlier and twice over.
//! [`GpuBakePlugin`] will not put a [`GpuBaker`] in the world on one, so an app
//! on a device with no compute or no storage buffers simply never has the
//! resource and every caller's `Option<Res<GpuBaker>>` is `None`; and
//! [`unsupported`] is checked again before a dispatch is built, because
//! [`GpuBaker::new`] and [`GpuBaker::headless`] are public and a content step
//! opens whichever adapter it is handed. It has to be a check rather than a
//! caught failure: `create_shader_module` and `create_compute_pipeline` hand
//! their errors to wgpu's uncaptured-error handler, which panics, rather than
//! answering the `Result` the surrounding code is written for.
use std::{
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Context, Result, bail, ensure};
use ashlar_material::{
    bake::{self, PlaneFormat, Planes},
    interp::Plane,
    ir::{BufferId, Ir},
    partition::{Partition, is_plane_port, over_planes, plane_port},
    planes,
    wgsl::{self, ComputeShader},
};
use bevy::{
    platform::future::block_on,
    prelude::{App, Plugin, Resource},
    render::{
        render_resource::{
            AddressMode, BindGroupEntry, BindGroupLayout, BindGroupLayoutEntry, BindingResource,
            BindingType, Buffer, BufferBindingType, BufferDescriptor, BufferUsages,
            CommandEncoderDescriptor, ComputePassDescriptor, ComputePipeline, Extent3d, FilterMode,
            MapMode, MipmapFilterMode, Origin3d, PipelineCompilationOptions,
            PipelineLayoutDescriptor, PollType, RawComputePipelineDescriptor, SamplerBindingType,
            SamplerDescriptor, ShaderModuleDescriptor, ShaderSource, ShaderStages,
            TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect, TextureDescriptor,
            TextureDimension, TextureFormat, TextureSampleType, TextureUsages, TextureView,
            TextureViewDescriptor, TextureViewDimension,
        },
        renderer::{RenderDevice, RenderQueue},
        settings::{Backends, WgpuSettings},
    },
};

impl std::fmt::Debug for GpuBaker {
    /// Neither half prints, and neither has anything to print: a `Debug` that
    /// said which `Arc` this is would say nothing about which device it is.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GpuBaker")
    }
}

/// Bytes one `vec4<f32>` of the kernel's output buffer occupies.
const OUTPUT_STRIDE: usize = 16;

/// Bytes the runtime-input block occupies, as
/// [`wgsl`](ashlar_material::wgsl)'s `AshlarInputs` declares it: two
/// three-lane vectors each followed by the scalar that fills the fourth lane.
///
/// A bake has no frame and no mesh, so every one of them is zero: the two
/// world-space ops never reach a bake at all, `Target::Bake` having refused
/// them by node path, and the clock and the cut flag read this block and get
/// the same zero the CPU interpreter's own `Inputs` defaults to.
const INPUTS: [u8; 32] = [0; 32];

/// A device and queue a bake can be dispatched on.
///
/// Cheap to clone — both halves are `Arc`s inside Bevy's own wrappers — and
/// holding one is what a caller needs to take the GPU path. In an app the two
/// come from the main world, where `RenderPlugin` puts them beside the render
/// world's copies:
///
/// ```no_run
/// use ashlar_bevy::gpu::GpuBaker;
/// use bevy::prelude::*;
/// use bevy::render::renderer::{RenderDevice, RenderQueue};
///
/// /// `Option`, because a headless app without a renderer has neither, and a
/// /// bake that falls back to the CPU is slower rather than broken.
/// fn bake_something(device: Option<Res<RenderDevice>>, queue: Option<Res<RenderQueue>>) {
///     let Some((device, queue)) = device.zip(queue) else {
///         return;
///     };
///     let baker = GpuBaker::new(device.clone(), queue.clone());
///     // `baker` now goes into a `Baker::Gpu` on a bake or shader context.
///     let _ = baker;
/// }
/// ```
#[derive(Clone, Resource)]
pub struct GpuBaker {
    device: RenderDevice,
    queue: RenderQueue,
    uploads: Arc<AtomicU64>,
}

/// Put a [`GpuBaker`] in the world wherever the app has a render device.
///
/// Added beside [`ProceduralMaterialPlugin`](crate::shader::ProceduralMaterialPlugin)
/// by an app that wants its runtime bakes on the device it is drawing with.
/// The insert happens in [`Plugin::finish`], which is where Bevy's own
/// `RenderPlugin` has just put the device into the main world; an app built
/// without a renderer gets no resource and every bake stays on the CPU, which
/// is why every caller takes `Option<Res<GpuBaker>>` and
/// [`Baker::or_cpu`](crate::runtime_bake::Baker::or_cpu) exists.
#[derive(Default)]
pub struct GpuBakePlugin;

impl Plugin for GpuBakePlugin {
    fn build(&self, _: &mut App) {}

    fn finish(&self, app: &mut App) {
        let world = app.world();
        let device = world.get_resource::<RenderDevice>().cloned();
        let queue = world.get_resource::<RenderQueue>().cloned();
        let Some((device, queue)) = device.zip(queue) else {
            // Two apps reach here and they want opposite things said, so this
            // says the one that covers both: a headless app has no renderer and
            // every bake was always going to be a CPU bake, and an app that
            // *has* a renderer got the plugin order wrong. `RenderPlugin`
            // inserts the device in its own `finish`, and `finish` runs in the
            // order plugins were added, so `GpuBakePlugin` added before
            // `DefaultPlugins` finds nothing here and never looks again.
            tracing::warn!(
                "GpuBakePlugin found no render device, so every material bake will run on the \
                 CPU. If this app has a renderer, add GpuBakePlugin *after* DefaultPlugins: \
                 Bevy's RenderPlugin inserts the device in its own Plugin::finish, and finish \
                 runs in the order the plugins were added."
            );
            return;
        };
        if let Some(missing) = unsupported(&device) {
            // `info` rather than `warn`: this one is a fact about the hardware
            // the app is running on rather than anything its author got wrong,
            // and the fallback it describes is the reference implementation.
            tracing::info!(
                "This render device {missing}, so ashlar's GPU bake is off and every material \
                 bake will run on the CPU. Nothing is lost but time: the CPU bake is the \
                 reference and the shipped files are written by it."
            );
            return;
        }
        app.insert_resource(GpuBaker::new(device, queue));
    }
}

/// What a device is missing that a bake needs, if anything.
///
/// A dispatch is a compute kernel writing one storage buffer, so a device that
/// reports zero for either of these cannot run one at all — and finding that
/// out by dispatching is not an option, because
/// [`RenderDevice::create_shader_module`] hands a module error to wgpu's
/// uncaptured-error handler rather than answering a `Result`, and
/// `create_compute_pipeline` does the same: the failure would be a panic rather
/// than the fallback every caller is written for. WebGL2's downlevel limits
/// report exactly this shape — `max_storage_buffers_per_shader_stage` and every
/// `max_compute_*` at zero (`wgpu-types`' `limits.rs:574`) — which is the case
/// this exists for.
///
/// The answer is the phrase that completes "this render device ...", so that a
/// caller's line reads as one sentence.
pub fn unsupported(device: &RenderDevice) -> Option<&'static str> {
    let limits = device.limits();
    if bevy::render::storage_buffers_are_unsupported(&limits) {
        return Some("binds no storage buffers");
    }
    if limits.max_compute_workgroup_size_x == 0 || limits.max_compute_invocations_per_workgroup == 0
    {
        return Some("runs no compute shaders");
    }
    None
}

impl GpuBaker {
    /// A baker over a device and its queue.
    pub fn new(device: RenderDevice, queue: RenderQueue) -> Self {
        Self {
            device,
            queue,
            uploads: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Open the default adapter with no window, no `App` and no render world.
    ///
    /// What a content step and a test want: `initialize_renderer` is the same
    /// entry point `RenderPlugin` uses, so the device is the one the app would
    /// have had. A machine with no adapter fails here with Bevy's own message
    /// rather than baking something subtly different.
    pub fn headless() -> Self {
        let settings = WgpuSettings::default();
        let backends = settings.backends.unwrap_or(Backends::all());
        let resources = block_on(bevy::render::renderer::initialize_renderer(
            backends, None, &settings,
        ));
        Self::new(resources.0, resources.1)
    }

    /// The device this bakes on.
    pub fn device(&self) -> &RenderDevice {
        &self.device
    }

    /// How many bound images this baker has packed and written to the device,
    /// counting from when it was made.
    ///
    /// Shared by every clone, as the device is. It exists because the cache
    /// below it is invisible from outside — a bake that uploads a plane twice
    /// answers exactly the same texels as one that uploads it once, only
    /// slower, so nothing but a count can fail when the sharing stops working.
    /// A caller takes the difference across one bake, which is what
    /// `the_gpu_bake_is_the_cpu_bake_within_the_conformance_budget` does to
    /// hold every study graph to one upload per bound image.
    pub fn uploads(&self) -> u64 {
        self.uploads.load(Ordering::Relaxed)
    }

    /// Rasterise every plane of a lowered expression, in
    /// [`Ir::buffers`] order.
    ///
    /// The GPU's [`rasterise_buffers`](ashlar_material::planes::rasterise_buffers),
    /// and the same answer: one [`Plane`] per plan, each the plan's
    /// sub-expression evaluated at the texel centres of its own square and then
    /// filtered. `threads` is how the *filters* divide their rows, because the
    /// filters are still CPU passes.
    ///
    /// The expression must not read a live parameter. Nothing that reaches here
    /// does: a bake folds every parameter, and a shader partition freezes any
    /// parameter that reaches a filter, which is the whole of
    /// [`Partition::frozen`](ashlar_material::partition::Partition::frozen)'s
    /// reason for existing. The uniform block is bound as zeros on that
    /// understanding.
    pub fn planes(
        &self,
        ir: &Ir,
        resolution: u32,
        threads: Option<NonZeroUsize>,
    ) -> Result<Vec<Plane>> {
        let split = over_planes(ir, resolution)
            .map_err(anyhow::Error::from)
            .context("splitting an expression into the planes a GPU bake dispatches")?;
        self.rasterise_planes(
            &split,
            &vec![true; ir.buffers().len()],
            threads,
            &mut Uploaded::default(),
        )
    }

    /// The planes `wanted` names and the planes those read, and no others.
    ///
    /// The GPU's
    /// [`rasterise_wanted`](ashlar_material::planes::rasterise_wanted), and the
    /// same answer plane for plane: a caller that already holds some of a
    /// compiled graph's bound images asks for the rest, and a plane nobody
    /// wants is a dispatch and a shader compile that never happen. Both
    /// backends go through the same lookup and the same
    /// [`plane_closure`](ashlar_material::planes::plane_closure), so which
    /// device drew a plane never decides whether it is shared.
    pub fn planes_wanted(
        &self,
        ir: &Ir,
        resolution: u32,
        wanted: &[bool],
        threads: Option<NonZeroUsize>,
    ) -> Result<Vec<Option<Plane>>> {
        let needed = planes::plane_closure(ir, wanted);
        let split = over_planes(ir, resolution)
            .map_err(anyhow::Error::from)
            .context("splitting an expression into the planes a GPU bake dispatches")?;
        let planes = self.rasterise_planes(&split, &needed, threads, &mut Uploaded::default())?;
        Ok(planes
            .into_iter()
            .zip(&needed)
            .map(|(plane, needed)| needed.then_some(plane))
            .collect())
    }

    /// Bake one planned graph as far as the `f32` planes.
    ///
    /// The GPU's [`rasterise`](ashlar_material::bake::rasterise): every plane,
    /// then every bound output, then the normal derived from the height plane
    /// by the bake's own wrapped central difference. What comes back is what
    /// [`Planes::encode_mips`] turns into a texture set, so everything past
    /// this point is the CPU bake unchanged.
    pub fn rasterise(
        &self,
        plan: &bake::Plan,
        resolution: u32,
        threads: Option<NonZeroUsize>,
    ) -> Result<Planes> {
        let split = over_planes(&plan.ir, resolution)
            .map_err(anyhow::Error::from)
            .context("splitting a graph into the stages a GPU bake dispatches")?;
        // One cache of uploaded images for the whole bake rather than one per
        // pass. The outputs dispatch reads exactly the planes the plane
        // dispatches have just put on the device, and an upload is not the
        // cheap half: packing a megatexel plane into `Rgba32Float` bytes and
        // writing it costs about 37 ms at 1024 squared in a debug build,
        // against about 60 ms for the dispatch that reads it. Both passes walk
        // one `split`, which is what makes this sound — an image id is an index
        // into that split's bound textures and means nothing outside it.
        let mut uploaded = Uploaded::default();
        let planes = self.rasterise_planes(
            &split,
            &vec![true; plan.ir.buffers().len()],
            threads,
            &mut uploaded,
        )?;
        let ports = self.rasterise_outputs(&split, resolution, &planes, &mut uploaded)?;
        let texels = texels(resolution);
        let lane = |port: &str| -> Option<Vec<f32>> {
            ports
                .iter()
                .find(|(name, _)| name == port)
                .map(|(_, plane)| (0..texels).map(|index| plane.texel_at(index)[0]).collect())
        };
        let colour = |port: &str| -> Option<Vec<[f32; 3]>> {
            ports
                .iter()
                .find(|(name, _)| name == port)
                .map(|(_, plane)| (0..texels).map(|index| plane.texel_at(index)).collect())
        };
        let height = lane("height");
        // The one pass that is per-texel and still on the CPU: it is four taps
        // of one plane rather than a walk of the whole expression, and running
        // it here is what makes a GPU bake's normal *identical* to a CPU bake's
        // rather than merely inside a budget.
        let normal =
            bake::derive_normal(height.as_deref(), resolution, plan.normal_strength, threads);
        Ok(Planes {
            resolution,
            normal_strength: plan.normal_strength,
            base_color: colour("base_color").unwrap_or_else(|| vec![[0.0; 3]; texels]),
            roughness: lane("roughness").unwrap_or_else(|| vec![0.0; texels]),
            metallic: lane("metallic").unwrap_or_else(|| vec![0.0; texels]),
            occlusion: lane("occlusion").unwrap_or_else(|| vec![0.0; texels]),
            normal,
            height,
            emissive: colour("emissive"),
        })
    }

    /// Every plane of a split, dispatched in order with the filters run
    /// between.
    ///
    /// `uploaded` is the bake's image cache, handed in rather than made here
    /// so that the outputs dispatch binds the textures these dispatches
    /// uploaded instead of packing them all a second time.
    fn rasterise_planes(
        &self,
        split: &Partition,
        needed: &[bool],
        threads: Option<NonZeroUsize>,
        uploaded: &mut Uploaded,
    ) -> Result<Vec<Plane>> {
        let wanted = split.runtime().buffers().to_vec();
        let mut planes: Vec<Plane> = Vec::with_capacity(wanted.len());
        for (index, plan) in wanted.iter().enumerate() {
            if needed.get(index).copied() != Some(true) {
                // A one-texel placeholder, exactly as the CPU pass leaves for a
                // plan nobody asked for: the list stays parallel to
                // `Ir::buffers`, and the closure has already said that nothing
                // rasterised here reads this slot.
                planes.push(Plane::new(
                    1,
                    plan.value_type,
                    vec![0.0; plan.value_type.components()],
                ));
                continue;
            }
            let size = plan.resolution.unwrap_or(split.resolution()).max(1);
            // A strand plane is scattered and splatted rather than evaluated
            // per texel, and neither half is a kernel: the scatter is sixty-five
            // thousand point evaluations of a small expression and the splat is
            // a scatter-gather over overlapping footprints. So this one takes
            // the CPU path on both backends, which is also what makes them
            // agree — there is one implementation rather than two.
            if plan.strands.is_some() {
                planes.push(
                    planes::strand_plane(
                        plan,
                        size,
                        threads,
                        &mut ashlar_material::planes::BakeCache::default(),
                    )
                    .map_err(anyhow::Error::from)
                    .with_context(|| format!("splatting plane {index} of {}", plan.path))?,
                );
                continue;
            }
            let port = plane_port(index);
            let kernel = wgsl::emit_compute_ports(split, size, std::slice::from_ref(&port))
                .map_err(anyhow::Error::from)
                .with_context(|| {
                    format!("emitting the kernel for plane {index} of {}", plan.path)
                })?;
            let mut written = self
                .dispatch(&kernel, split, &planes, needed, size, uploaded)
                .with_context(|| format!("dispatching plane {index} of {}", plan.path))?;
            let raw = written
                .pop()
                .with_context(|| format!("plane {index} of {} wrote no port", plan.path))?
                .1;
            // The planes before this one, because a slope blur's filter reads
            // one of them: the walk down a height is a CPU pass over two
            // planes, and a dispatch has only computed the field to smear.
            let filtered = planes::filter(plan.filter, raw, &planes, threads);
            planes.push(filtered);
        }
        Ok(planes)
    }

    /// Every bound output of a split, over the planes it samples.
    ///
    /// One dispatch where the whole answer fits the device's storage budget and
    /// as few as it takes otherwise: the kernel writes a `vec4<f32>` per port
    /// per texel, so six ports at 2048 squared is four hundred megabytes and
    /// past what wgpu's default limits bind at once.
    ///
    /// `uploaded` is the bake's, so a batch binds the textures the plane
    /// dispatches already wrote. Every plane exists by the time this runs, so
    /// every image is complete and every one of them is a hit unless nothing
    /// before this read it.
    fn rasterise_outputs(
        &self,
        split: &Partition,
        resolution: u32,
        planes: &[Plane],
        uploaded: &mut Uploaded,
    ) -> Result<Vec<(String, Plane)>> {
        let ports: Vec<String> = split
            .runtime()
            .roots()
            .keys()
            .filter(|port| !is_plane_port(port))
            .cloned()
            .collect();
        let budget = self.budget();
        let bytes_per_port = (texels(resolution) * OUTPUT_STRIDE) as u64;
        ensure!(
            bytes_per_port <= budget,
            "one output of a {resolution}-texel bake is {bytes_per_port} bytes and this device \
             binds {budget} at once; bake it on the CPU or at fewer texels"
        );
        let per_batch = usize::try_from(budget / bytes_per_port.max(1))
            .unwrap_or(usize::MAX)
            .max(1);
        let mut written = Vec::with_capacity(ports.len());
        // Every plane exists by the time the outputs run, which is what makes
        // every image complete and every bind of one a cache hit.
        let made = vec![true; planes.len()];
        for batch in ports.chunks(per_batch) {
            let kernel = wgsl::emit_compute_ports(split, resolution, batch)
                .map_err(anyhow::Error::from)
                .context("emitting the kernel for a bake's outputs")?;
            written.extend(
                self.dispatch(&kernel, split, planes, &made, resolution, uploaded)
                    .context("dispatching a bake's outputs")?,
            );
        }
        Ok(written)
    }

    /// The largest storage buffer this device will bind, with the whole-buffer
    /// limit taken into account.
    fn budget(&self) -> u64 {
        let limits = self.device.limits();
        limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size)
    }

    /// Compile one emitted kernel into a pipeline, and answer the bind group
    /// layout the dispatch has to fill.
    ///
    /// The refusal lives here because this is where a device that cannot run
    /// compute would otherwise take the process down with it:
    /// [`RenderDevice::create_shader_module`] hands a module error to wgpu's
    /// uncaptured-error handler rather than answering a `Result`, and
    /// `create_compute_pipeline` does the same, so by the time either of them
    /// has failed there is nothing left to fall back from. [`GpuBakePlugin`]
    /// already keeps a [`GpuBaker`] out of the world on such a device, but
    /// [`GpuBaker::new`] and [`GpuBaker::headless`] are public and a content
    /// step opens whichever adapter it is handed.
    fn pipeline(
        &self,
        kernel: &ComputeShader,
        label: &str,
    ) -> Result<(ComputePipeline, BindGroupLayout)> {
        let device = &self.device;
        if let Some(missing) = unsupported(device) {
            bail!(
                "this render device {missing}, so a bake cannot be dispatched on it at all; bake \
                 it on the CPU"
            );
        }
        let module = device.create_and_validate_shader_module(ShaderModuleDescriptor {
            label: Some(label),
            source: ShaderSource::Wgsl(kernel.module().into()),
        });
        let entries = layout_entries(kernel);
        let layout = device.create_bind_group_layout(label, &entries);
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&RawComputePipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: PipelineCompilationOptions::default(),
            cache: None,
        });
        Ok((pipeline, layout))
    }

    /// Run one emitted kernel and read its ports back as planes.
    fn dispatch(
        &self,
        kernel: &ComputeShader,
        split: &Partition,
        planes: &[Plane],
        made: &[bool],
        resolution: u32,
        uploaded: &mut Uploaded,
    ) -> Result<Vec<(String, Plane)>> {
        let label = "ashlar gpu bake";
        let device = &self.device;
        let (pipeline, layout) = self.pipeline(kernel, label)?;

        let mut views = Vec::with_capacity(kernel.bindings().len());
        for binding in kernel.bindings() {
            views.push(self.image_of(binding, split, planes, made, uploaded)?);
        }
        let sampler = self.sampler(label);
        let size = (kernel.output_len() * OUTPUT_STRIDE) as u64;
        let output = device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&BufferDescriptor {
            label: Some("ashlar gpu bake readback"),
            size,
            usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // A bake has no frame and no mesh, so the four runtime inputs are zero
        // — which is what `Target::Bake` already folded them to — and no
        // parameter survives to the block, so it is zeros the width the layout
        // declares.
        let inputs = device.create_buffer(&BufferDescriptor {
            label: Some("AshlarInputs"),
            size: INPUTS.len() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&inputs, 0, &INPUTS);
        let block_size = u64::from(kernel.layout().size().max(16));
        let block = device.create_buffer(&BufferDescriptor {
            label: Some("AshlarParams"),
            size: block_size,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(
            &block,
            0,
            &vec![0_u8; usize::try_from(block_size).unwrap_or_default()],
        );

        let mut bind_entries = vec![
            BindGroupEntry {
                binding: wgsl::OUTPUT_BINDING,
                resource: output.as_entire_binding(),
            },
            BindGroupEntry {
                binding: wgsl::INPUTS_BINDING,
                resource: inputs.as_entire_binding(),
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
        let bind_group = device.create_bind_group(label, &layout, &bind_entries);

        let groups = resolution.div_ceil(wgsl::WORKGROUP);
        let mut encoder =
            device.create_command_encoder(&CommandEncoderDescriptor { label: Some(label) });
        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
                label: Some(label),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &*bind_group, &[]);
            pass.dispatch_workgroups(groups, groups, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, size);
        self.queue.submit([encoder.finish()]);
        let written = self.read_back(&readback)?;
        Ok(unpack(kernel, resolution, &written))
    }

    /// The repeating bilinear sampler a plane is read through.
    ///
    /// What [`image`](crate::runtime_bake::image) asks the loader for on a baked
    /// map, and what
    /// [`Plane::sample`](ashlar_material::interp::Plane::sample) does on the
    /// CPU. No mip filtering, because a plane has no chain: a bake's own sample
    /// is of level zero and so is the kernel's `textureSampleLevel`.
    fn sampler(&self, label: &str) -> bevy::render::render_resource::Sampler {
        self.device.create_sampler(&SamplerDescriptor {
            label: Some(label),
            address_mode_u: AddressMode::Repeat,
            address_mode_v: AddressMode::Repeat,
            address_mode_w: AddressMode::Repeat,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Nearest,
            ..SamplerDescriptor::default()
        })
    }

    /// One bound image as a texture, uploaded once per bake rather than once
    /// per dispatch.
    ///
    /// Every stage after the first binds planes earlier stages already
    /// uploaded, and turning a megatexel plane into texture bytes costs more
    /// than the dispatch that reads it does. An image is cached as soon as
    /// every lane it holds exists, because a [`Plane`] is never edited after it
    /// is made: what is in the texture is then what will always be in it. An
    /// image with a lane still to come is uploaded for this dispatch alone,
    /// which only a packed one can be.
    ///
    /// "Every lane it holds" is the *image's* lanes, read out of the split,
    /// rather than the lanes the kernel being dispatched happens to sample. The
    /// two are the same thing for an unpacked image, and were one expression
    /// here until a study graph packed. They are not the same for a packed one:
    /// the kernel for an early plane samples one lane of a four-lane image, and
    /// having every lane it names is no statement at all about the three it
    /// does not. Caching on that put a texture with three zeroed channels under
    /// the image's id and handed it to every later dispatch — the outputs
    /// dispatch reads all four — which was a rusted-steel graph's roughness
    /// 0.61 wrong over most of the tile and a weathered-paint graph's height
    /// wrong over all of it, with nothing but `just conformance` to say so.
    ///
    /// `made` is which planes exist at all, so that a
    /// [`planes_wanted`](Self::planes_wanted) over part of a graph does not
    /// count the one-texel placeholder standing in for a skipped plane as a
    /// lane. An image with a skipped lane is never complete and so is uploaded
    /// per dispatch, holding the lanes that are real — which is every lane any
    /// kernel in that rasterisation may read, because the closure said so.
    ///
    /// "Per bake" is the whole bake, the outputs dispatch included, because
    /// [`Self::rasterise`] makes one `uploaded` and hands it to both passes.
    /// [`Self::uploads`] counts the misses, and `just conformance` holds that
    /// count to what a cache sharing every finished image makes.
    fn image_of(
        &self,
        binding: &wgsl::TextureBinding,
        split: &Partition,
        planes: &[Plane],
        made: &[bool],
        uploaded: &mut Uploaded,
    ) -> Result<TextureView> {
        let lanes: Vec<BufferId> = split
            .textures()
            .iter()
            .filter(|texture| texture.id == Some(binding.image))
            .map(|texture| texture.buffer)
            .collect();
        let complete = lanes.iter().all(|buffer| {
            buffer.index() < planes.len() && made.get(buffer.index()).copied() == Some(true)
        });
        if complete && let Some(view) = uploaded.get(&binding.image) {
            return Ok(view.clone());
        }
        let view = self.upload(binding, &lanes, split, planes, made)?;
        if complete {
            uploaded.insert(binding.image, view.clone());
        }
        Ok(view)
    }

    /// One bound image as an `Rgba32Float` texture, from the planes its lanes
    /// came from.
    ///
    /// An image is not a plane: where a split packs its scalar planes four to
    /// an image, this writes each into the channel its binding owns and leaves
    /// the rest of them zero. A lane whose plane is not made yet cannot be read
    /// by this dispatch — a plan only samples planes earlier than itself — so a
    /// zero there is a channel nothing looks at rather than a wrong number.
    ///
    /// `lanes` is every lane of the image, handed down by [`Self::image_of`]
    /// rather than taken from the binding, because what goes on the device is
    /// the image and the dispatch after this one has to find the whole of it
    /// there. `made` says which of those planes are real, since a partial
    /// rasterisation leaves a one-texel placeholder in the slots it skipped and
    /// a placeholder is not a lane.
    fn upload(
        &self,
        binding: &wgsl::TextureBinding,
        lanes: &[BufferId],
        split: &Partition,
        planes: &[Plane],
        made: &[bool],
    ) -> Result<TextureView> {
        let mut resolution = 0;
        // The texture's own bytes, written in place: a megatexel image is
        // sixteen megabytes and this runs once per plane of every bake, so the
        // intermediate vector of four-lane texels that reads more nicely is a
        // copy of all of it for nothing.
        let mut bytes: Vec<u8> = Vec::new();
        for buffer in lanes {
            let texture = split
                .textures()
                .get(buffer.index())
                .with_context(|| format!("bound texture {buffer} has no plan"))?;
            if made.get(buffer.index()).copied() != Some(true) {
                // A plane this rasterisation was told to skip. Nothing binding
                // this image reads it, and the placeholder standing in for it
                // is one texel rather than this image's square.
                continue;
            }
            let Some(plane) = planes.get(buffer.index()) else {
                // A plane the sequence has not reached. Only a lane of a packed
                // image can be one, and no instruction in this dispatch reads
                // it; the channel stays zero.
                continue;
            };
            if bytes.is_empty() {
                resolution = plane.resolution();
                bytes = vec![0; plane.texels() * LANES * 4];
            }
            ensure!(
                plane.resolution() == resolution,
                "bound image {} packs planes of {resolution} and {} texels",
                binding.image,
                plane.resolution()
            );
            let width = texture.value_type.components();
            let first = usize::from(texture.lane);
            let source = plane.lanes();
            for texel in 0..plane.texels() {
                for lane in 0..width {
                    let value = source
                        .get(texel * width + lane)
                        .copied()
                        .unwrap_or_default();
                    let at = (texel * LANES + first + lane) * 4;
                    if let Some(slot) = bytes.get_mut(at..at + 4) {
                        slot.copy_from_slice(&value.to_le_bytes());
                    }
                }
            }
        }
        ensure!(
            !bytes.is_empty(),
            "bound image {} holds no plane this dispatch can read",
            binding.image
        );
        // `over_planes` gives every bound texture the one format the emitter
        // decodes nothing for, and this is the upload that goes with it: the
        // numbers the plane holds, at the precision it holds them.
        ensure!(
            binding.format == PlaneFormat::Rgba16Float,
            "a GPU bake uploads its planes raw, and image {} was cut for {:?}",
            binding.image,
            binding.format
        );
        let extent = Extent3d {
            width: resolution,
            height: resolution,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&TextureDescriptor {
            label: Some(&binding.texture_name),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba32Float,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            &bytes,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(resolution * 16),
                rows_per_image: Some(resolution),
            },
            extent,
        );
        self.uploads.fetch_add(1, Ordering::Relaxed);
        Ok(texture.create_view(&TextureViewDescriptor::default()))
    }

    /// Wait for the queue and take a copy of one mapped buffer.
    ///
    /// Bytes rather than `f32`s: a kernel writes four lanes per port per texel
    /// and a port owns one to three of them, so decoding the whole buffer would
    /// decode a quarter to three quarters of it for nothing. [`unpack`] reads
    /// the lanes it wants.
    fn read_back(&self, readback: &Buffer) -> Result<Vec<u8>> {
        let slice = readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        self.device
            .map_buffer(&slice, MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.device
            .poll(PollType::wait_indefinitely())
            .map_err(|error| anyhow::anyhow!("{error:?}"))
            .context("waiting for a bake dispatch")?;
        receiver
            .recv()
            .context("the readback map callback never ran")?
            .map_err(|error| anyhow::anyhow!("{error:?}"))
            .context("mapping a bake's readback buffer")?;
        let view = slice.get_mapped_range();
        let bytes = view.to_vec();
        drop(view);
        readback.unmap();
        Ok(bytes)
    }
}

/// Channels one uploaded image holds. `Rgba32Float`, so four.
const LANES: usize = 4;

/// The textures a bake has already put on the device, by image id.
///
/// One bake's worth, and no more than that. A [`Plane`] is immutable once
/// made, so an image whose every lane exists is an image that will not change,
/// and every later dispatch of the same bake — the outputs included — binds the
/// one already there. It cannot outlive the bake, because the key is an index
/// into one [`Partition`]'s bound textures and two splits number their images
/// independently; what identifies a plane across bakes is
/// [`planes::PlaneKey`](ashlar_material::planes::PlaneKey), and the reason
/// nothing here is keyed by one is in the module's own "What is uploaded".
type Uploaded = std::collections::HashMap<u32, TextureView>;

/// Texels in a square of this many per side.
fn texels(resolution: u32) -> usize {
    (resolution as usize).saturating_mul(resolution as usize)
}

/// The kernel's output buffer as one plane per port.
///
/// Texel `(x, y)` writes one `vec4<f32>` per port at
/// `((y * n + x) * ports + slot)`, which is what
/// [`ComputeShader::ports`](ashlar_material::wgsl::ComputeShader::ports) says;
/// a plane takes the lanes its own width owns and leaves the rest.
fn unpack(kernel: &ComputeShader, resolution: u32, written: &[u8]) -> Vec<(String, Plane)> {
    let count = kernel.ports().len();
    let texels = texels(resolution);
    kernel
        .ports()
        .iter()
        .enumerate()
        .map(|(slot, (port, value_type))| {
            let components = value_type.components();
            let mut lanes = vec![0.0_f32; texels * components];
            for texel in 0..texels {
                let base = (texel * count + slot) * LANES * 4;
                for lane in 0..components {
                    let at = base + lane * 4;
                    if let (Some(slot), Some(word)) = (
                        lanes.get_mut(texel * components + lane),
                        written
                            .get(at..at + 4)
                            .and_then(|bytes| bytes.first_chunk()),
                    ) {
                        *slot = f32::from_le_bytes(*word);
                    }
                }
            }
            (port.clone(), Plane::new(resolution, *value_type, lanes))
        })
        .collect()
}

/// The bind group layout the generated kernel declares.
///
/// The output at zero, the runtime inputs and the parameter block just below
/// the textures, and one texture-and-sampler pair per bound image from a
/// hundred and one up — read off the emitted kernel rather than written down
/// twice.
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

/// Bake a graph on the GPU and answer what it cost.
///
/// [`bake_with_report`](ashlar_material::bake::bake_with_report) on a device,
/// and shaped as an [`ashlar_material::bake::Backend`] so that a content step
/// can be handed one without knowing what a render device is:
///
/// ```no_run
/// use ashlar_bevy::gpu::{GpuBaker, bake_with_report};
/// use ashlar_material::bake::{BakeRequest, Backend};
///
/// # fn write_materials(_: Backend<'_>) {}
/// let baker = GpuBaker::headless();
/// let on_the_device = move |request: &BakeRequest<'_>| bake_with_report(&baker, request);
/// // For instance `ashlar_content::Content::write_materials_on`.
/// write_materials(&on_the_device);
/// ```
///
/// The report is the CPU bake's own, built from the same lowering, so the two
/// backends print comparable lines; `elapsed` is this call's wall clock.
pub fn bake_with_report(
    gpu: &GpuBaker,
    request: &ashlar_material::bake::BakeRequest<'_>,
) -> Result<
    (
        ashlar_material::bake::TextureSet,
        ashlar_material::bake::BakeReport,
    ),
    ashlar_material::bake::BakeError,
> {
    use ashlar_material::bake::{BakeError, BakeReport, Dither};

    let started = std::time::Instant::now();
    let plan = bake::plan(request)?;
    let planes = gpu
        .rasterise(&plan, request.resolution, request.threads)
        .map_err(|error| BakeError::Backend(format!("{error:#}")))?;
    let set = if request.mips {
        planes.encode_mips(Dither::Ordered)
    } else {
        planes.encode(Dither::Ordered)
    };
    let report = BakeReport::of(&plan.ir, &planes, request.mips, started.elapsed());
    Ok((set, report))
}

/// Whether the GPU can dispatch this expression's planes at all.
///
/// A plane pass binds the planes it reads as textures, and a compute pass here
/// binds at most [`MAX_BOUND_TEXTURES`](ashlar_material::partition::MAX_BOUND_TEXTURES)
/// of them. A graph whose last plane reads more than that — the grass, whose
/// canopy reads eleven strand reliefs — is a graph the device cannot split, and
/// its bake belongs to the CPU, which is the reference and has no such limit.
/// Callers ask this before they dispatch rather than treating the refusal as a
/// failure, because it is a capability of the device and not a fault in either
/// the graph or the backend.
#[must_use]
pub fn dispatchable(ir: &Ir, resolution: u32) -> bool {
    over_planes(ir, resolution).is_ok()
}

/// Bake one surface into images on the GPU.
///
/// [`bake_images`](crate::runtime_bake::bake_images)' twin, and the same answer:
/// the same five maps with the same mip chains, inside the tolerance
/// `just conformance` states between the two backends. What differs is where
/// the expression runs — a compute dispatch per plane and one for the outputs,
/// through the WGSL the fragment shader is printed from — and what it costs.
///
/// The CPU bake stays the reference. A file a content step ships is written by
/// it, and this is for the bakes a game pays for while somebody is looking: a
/// per-building seed, a slider in a preview, a wall that wanted its own tint.
/// A graph the device cannot split ([`dispatchable`]) is baked on the CPU.
pub fn bake_images_gpu(
    bake: &ashlar::Bake,
    graphs: &ashlar_material::MaterialGraphLibrary,
    gpu: &GpuBaker,
    threads: Option<NonZeroUsize>,
) -> Result<crate::runtime_bake::GraphTextures> {
    let graph = crate::runtime_bake::graph_of(&bake.graph, graphs)?;
    let plan = bake::plan(&ashlar_material::bake::BakeRequest {
        graph,
        library: graphs,
        params: &bake.params,
        resolution: bake.resolution,
        mips: true,
        threads,
    })
    .with_context(|| format!("planning material graph {:?}", bake.graph))?;
    if !dispatchable(&plan.ir, bake.resolution) {
        return crate::runtime_bake::bake_images(bake, graphs, threads);
    }
    let planes = gpu
        .rasterise(&plan, bake.resolution, threads)
        .with_context(|| format!("baking material graph {:?} on the GPU", bake.graph))?;
    let set = planes.encode_mips(ashlar_material::bake::Dither::Ordered);
    Ok(crate::runtime_bake::GraphTextures::from(&set))
}
