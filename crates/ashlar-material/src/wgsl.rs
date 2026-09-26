//! The runtime partition as WGSL text.
//!
//! [`partition`](crate::partition) hands this module an [`Ir`](crate::ir::Ir) over
//! [`Op::Param`], the four runtime inputs and samples of bound textures. This
//! prints it: one `let` per instruction, in the order the interpreter runs
//! them, inside one function that takes a coordinate and the runtime inputs and
//! answers the material's PBR ports. Everything else here is packaging around
//! that function — a uniform block, a binding pair per bound image, and an
//! entry point for the stage being compiled.
//!
//! # The interpreter is the reference
//!
//! Every [`Op`] is written as the arithmetic
//! [`interp::apply`](crate::interp) does, not as the WGSL built-in that is
//! nearly it. [`Op::Mix`] is `a + (b - a) * t` rather than `mix`, because WGSL
//! defines `mix` as `a * (1 - t) + b * t` and the two round differently;
//! [`Op::Clamp`] is `min(max(x, low), high)` rather than `clamp`, whose answer
//! WGSL leaves indeterminate when the bounds cross and which a graph may well
//! write crossed; [`Op::Div`], [`Op::Smoothstep`] and [`Op::Normalize`] go
//! through helper functions because each has a guarded case — a division by
//! zero that answers zero, edges that meet, a vector of length zero — that the
//! built-in does not have. [`Op::Hash2`] and [`Op::Hash3`] are the same integer
//! multiply-xorshift, in `u32`, whose arithmetic WGSL defines as wrapping, over
//! the same modulo-period lattice reduction; that is what makes a noise the
//! same noise on both backends and it is spelled out in [`HASH`].
//!
//! One op is narrower here than in the interpreter and is written down rather
//! than hidden: [`Op::Pow`] is WGSL's `pow`, which is `exp2(b * log2(a))` and
//! says nothing about a negative base, while `f32::powf` answers for one. A
//! graph that raises a negative field to a power is the one place the two
//! backends may part company.
//!
//! # What is validated, and what is not
//!
//! Two texts come out of one emitter. [`Shader::module`] is the Bevy-flavoured
//! one: it opens with `#import bevy_pbr::…` lines and writes its bind group as
//! `#{MATERIAL_BIND_GROUP}`. Both are `naga_oil` preprocessor directives, and
//! plain `naga` parses neither, so that text is what the Bevy step assembles
//! and registers and is *not* what the tests here parse.
//!
//! [`Shader::core`] is the same generated code with the group written as a
//! literal and no imports at all: the uniform block, the bindings, the helpers
//! and the SSA function, which between them are everything this module decides.
//! [`Shader::standalone`] appends a stub entry point so the function is
//! reachable from the stage it was written for, and that is the text `naga`
//! parses and validates in the tests. What is therefore *not* checked by a
//! parser here is the wrapper: the import list, the `#ifdef`s, and the field
//! names of Bevy's own `PbrInput`. Those are checked by the Bevy step
//! compiling one, and by the conformance test the plan puts after it.
//!
//! [`emit_compute`] has no such split. A compute kernel imports nothing, so
//! what it emits is what is validated, and it is the same surface function over
//! the same bindings evaluated at texel centres — which is what the conformance
//! test dispatches and what a GPU bake would.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::{
    GraphError,
    bake::PlaneFormat,
    ir::{BufferId, Filter, Inst, IrType, Op, ParamBinding, ValueId},
    partition::Partition,
    require,
};

/// The binding the `AshlarParams` uniform block occupies.
///
/// A hundred, because Bevy's `StandardMaterial` owns everything below it and a
/// material extension's own bindings start there by convention.
pub const PARAMS_BINDING: u32 = 100;

/// The binding the first bound texture occupies.
///
/// Image `n` is at `FIRST_TEXTURE_BINDING + 2 * n` and its sampler at one
/// past that, so a material that binds six images reaches binding 112.
pub const FIRST_TEXTURE_BINDING: u32 = 101;

/// The binding the compute kernel's runtime inputs occupy.
///
/// A fragment reads time and the mesh from Bevy's own view and vertex
/// bindings; a compute dispatch has neither, so [`emit_compute`] declares one
/// small uniform of its own just below the parameter block.
pub const INPUTS_BINDING: u32 = 99;

/// The binding the compute kernel writes its answers into.
pub const OUTPUT_BINDING: u32 = 0;

/// The binding a compute kernel reads a world position *per texel* from.
///
/// Just below [`INPUTS_BINDING`], and only declared where the kernel was asked
/// for [`MeshInputs::Planes`].
pub const WORLD_POS_BINDING: u32 = 97;

/// The binding a compute kernel reads a world normal per texel from.
pub const WORLD_NORMAL_BINDING: u32 = 98;

/// The binding a compute kernel reads the mesh's cut flag per texel from.
///
/// The third of the mesh planes, and declared under the same condition as the
/// other two. The flag is the first lane of each `vec4<f32>`.
pub const CUT_FLAG_BINDING: u32 = 96;

/// The bind group the standalone core writes, where the module writes
/// `#{MATERIAL_BIND_GROUP}`.
///
/// Two is what Bevy assigns a material bind group, and the only reason to name
/// it is that `naga` cannot parse the preprocessor substitution the real module
/// carries. Nothing reads this number at run time.
pub const MATERIAL_BIND_GROUP: u32 = 2;

/// Texels one compute workgroup covers on a side.
pub const WORKGROUP: u32 = 8;

/// The name of the generated surface function, in every flavour.
pub const SURFACE_FN: &str = "ashlar_surface";

/// The name of the struct that function answers.
pub const SURFACE_STRUCT: &str = "AshlarSurface";

/// The name of the uniform block holding the live parameters.
pub const PARAMS_STRUCT: &str = "AshlarParams";

/// The name of the variable that block is bound to.
pub const PARAMS_VAR: &str = "ashlar";

/// The lattice hash, in WGSL, exactly as every emitted module carries it.
///
/// The one piece of generated text with a number in it that has to agree with
/// another backend bit for bit, so it is a constant rather than a builder: the
/// primes, the shift amounts and the multiply are
/// [`interp::hash2_bits`](crate::interp::hash2_bits)'s, the reduction is
/// [`interp`](crate::interp)'s `cell`, and `u32` arithmetic wraps in WGSL by
/// definition, which is what `wrapping_mul` and `wrapping_add` say on the other
/// side. The divisor is written as `4294967296.0` and not as `4294967295.0`
/// because the interpreter divides by `u32::MAX as f32`, and `u32::MAX` is not
/// an `f32`: the cast rounds up to the next power of two, and a WGSL module
/// that divided by the exact integer would be a different function.
pub const HASH: &str = r"const ASHLAR_PRIME_0: u32 = 374761393u;
const ASHLAR_PRIME_1: u32 = 668265263u;
const ASHLAR_PRIME_2: u32 = 3266489917u;
const ASHLAR_PRIME_3: u32 = 2654435761u;
const ASHLAR_AVALANCHE: u32 = 1274126177u;
const ASHLAR_HASH_SCALE: f32 = 4294967296.0;

fn ashlar_cell(coordinate: f32, period: f32) -> u32 {
    let floored = floor(coordinate);
    var reduced = floored;
    if period >= 1.0 {
        var remainder = floored % period;
        if remainder < 0.0 {
            remainder = remainder + abs(period);
        }
        reduced = remainder;
    }
    return bitcast<u32>(i32(reduced));
}

fn ashlar_avalanche(mixed: u32) -> u32 {
    let shifted = (mixed ^ (mixed >> 13u)) * ASHLAR_AVALANCHE;
    return shifted ^ (shifted >> 16u);
}

fn ashlar_hash2(coordinate: vec2<f32>, period: vec2<f32>, seed: u32) -> f32 {
    let x = ashlar_cell(coordinate.x, period.x);
    let y = ashlar_cell(coordinate.y, period.y);
    let bits = ashlar_avalanche(x * ASHLAR_PRIME_0 + y * ASHLAR_PRIME_1 + seed * ASHLAR_PRIME_3);
    return f32(bits) / ASHLAR_HASH_SCALE;
}

fn ashlar_hash3(coordinate: vec3<f32>, period: vec3<f32>, seed: u32) -> f32 {
    let x = ashlar_cell(coordinate.x, period.x);
    let y = ashlar_cell(coordinate.y, period.y);
    let z = ashlar_cell(coordinate.z, period.z);
    let bits = ashlar_avalanche(
        x * ASHLAR_PRIME_0 + y * ASHLAR_PRIME_1 + z * ASHLAR_PRIME_2 + seed * ASHLAR_PRIME_3
    );
    return f32(bits) / ASHLAR_HASH_SCALE;
}
";

/// Where a compute kernel gets the three inputs that come from the mesh: the
/// world position, the world normal and the cut flag.
///
/// A fragment reads all three off its own `VertexOutput`, which is the whole
/// point of them; a compute dispatch has no mesh, so it has to be told. Two
/// ways, because two things dispatch one:
///
/// - [`Self::Uniform`] is one position, one normal and one flag for the whole
///   square, from the `AshlarInputs` block. What a
///   [GPU bake](crate::partition::over_planes) wants — a bake is a texture and a
///   texture has no mesh, so the expression it evaluates cannot depend on any of
///   them — and what the conformance test uses wherever a case does not read
///   them.
/// - [`Self::Planes`] is a position, a normal and a flag **per texel**, from
///   three storage buffers indexed by the invocation. A field over the mesh is
///   only itself where the mesh varies under it: a triplanar read at one point
///   is one sample of its source repeated across the square, and a cut flag of
///   one everywhere is a surface with no boundary on it — either would compare
///   equal between two backends that disagreed everywhere else. So the
///   conformance harness lays a world position, a world normal and a cut flag
///   over the same square it compares — planes, in this crate's own sense of the
///   word — and feeds them to both sides.
///
/// It changes the kernel's bindings and nothing else: the surface function
/// takes the same arguments either way, which is what keeps one emitter
/// answering for the fragment, the prepass and both flavours of dispatch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MeshInputs {
    /// One position, normal and cut flag for the whole dispatch.
    #[default]
    Uniform,
    /// One position, normal and cut flag per texel, from three storage buffers.
    Planes,
}

/// Which stage an emitted module is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// The main pass: `pbr_input_from_standard_material`, the PBR slots, the
    /// tangent-space normal, and the lighting.
    Fragment,
    /// The prepass: the same normal, half-and-half encoded, so screen-space
    /// occlusion and the deferred path see the procedural relief.
    Prepass,
}

/// Which text is being written, which decides how a texture is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flavour {
    /// A fragment stage: `textureSample`, with the mip chosen from the
    /// coordinate's own derivatives.
    Fragment,
    /// A compute stage, where there are no derivatives, so the level is named
    /// rather than chosen: zero for every dispatch that is a bake or a
    /// comparison of one, and a level further down for a harness asking what
    /// this material looks like from further away.
    Compute {
        /// The mip level every bound texture is read at.
        level: u32,
    },
}

/// Where one live parameter sits in the uniform block's bytes.
///
/// The Rust-side description of the std140 layout, so the Bevy step fills the
/// block without re-deriving it. [`Self::offset`] and [`Self::size`] are bytes;
/// the padding between one entry and the next belongs to nobody and is written
/// as zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamLayout {
    /// The parameter's name in the graph, which is what a caller looks it up
    /// by.
    pub name: String,
    /// The struct member's name in the shader, which is the graph's name unless
    /// that was not a WGSL identifier or collided with one already used.
    pub member: String,
    /// Bytes from the start of the block.
    pub offset: u32,
    /// Bytes the value itself occupies: four for a scalar, twelve for a colour.
    pub size: u32,
    /// The width the expression reads it at.
    pub value_type: IrType,
}

/// The `AshlarParams` block: what is in it, where, and how big it is.
///
/// The layout is WGSL's own, which for these types is std140: a scalar is
/// aligned to four bytes and a `vec3<f32>` to sixteen, members keep the order
/// the graph declares its parameters in, and the block's size is rounded up to
/// sixteen. **Consecutive floats pack**, four to sixteen bytes, rather than
/// each taking a sixteen-byte slot of its own — that is the choice this module
/// makes and the reason it can be made is that the order is fixed by the graph
/// and never by the emitter, so an author adding a slider moves the tail of the
/// block and nothing before it.
///
/// A block with no parameters is still a block: it carries one padding member
/// and is sixteen bytes, so the bind group layout does not change shape from
/// one graph to the next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniformLayout {
    entries: Vec<ParamLayout>,
    size: u32,
}

impl UniformLayout {
    /// The layout of one parameter list, in binding order.
    fn of(params: &[ParamBinding]) -> Self {
        let mut entries = Vec::with_capacity(params.len());
        let mut used: BTreeSet<String> = BTreeSet::new();
        let mut offset = 0_u32;
        for (index, binding) in params.iter().enumerate() {
            let (align, size) = shape(binding.value_type);
            offset = round_up(offset, align);
            let member = member_name(&binding.name, index, &mut used);
            entries.push(ParamLayout {
                name: binding.name.clone(),
                member,
                offset,
                size,
                value_type: binding.value_type,
            });
            offset = offset.saturating_add(size);
        }
        Self {
            size: round_up(offset, 16).max(16),
            entries,
        }
    }

    /// Every parameter, in the order [`Ir::params`](crate::ir::Ir::params) holds them.
    pub fn entries(&self) -> &[ParamLayout] {
        &self.entries
    }

    /// One parameter by its name in the graph.
    pub fn entry(&self, name: &str) -> Option<&ParamLayout> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    /// The whole block's size in bytes, rounded up to sixteen.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// The block's bytes, for values given in [`Ir::params`](crate::ir::Ir::params) order.
    ///
    /// A value is three lanes wide whatever its type, the way
    /// [`Inputs::params`](crate::interp::Inputs::params) carries one, and only
    /// the lanes the parameter's own width owns are written. A value list
    /// shorter than the block leaves the rest zero, which is what an unbound
    /// slider is.
    pub fn bytes(&self, values: &[[f32; 3]]) -> Vec<u8> {
        let mut block = vec![0_u8; self.size as usize];
        for (entry, value) in self.entries.iter().zip(values) {
            let lanes = value.iter().enumerate().take(entry.value_type.components());
            for (lane, component) in lanes {
                let at = entry.offset as usize + lane * 4;
                let Some(slot) = block.get_mut(at..at + 4) else {
                    continue;
                };
                slot.copy_from_slice(&component.to_le_bytes());
            }
        }
        block
    }
}

/// One bound image and the pair of bindings it occupies.
///
/// An image, not a plane: a graph whose scalar planes were packed four to an
/// RGBA image has one of these per image and several
/// [`BoundTexture`](crate::partition::BoundTexture)s reading lanes of it, which
/// is what [`Self::buffers`] lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureBinding {
    /// The image's index, which is
    /// [`BoundTexture::id`](crate::partition::BoundTexture::id).
    pub image: u32,
    /// The binding the `texture_2d<f32>` occupies.
    pub texture: u32,
    /// The binding the `sampler` occupies.
    pub sampler: u32,
    /// The texture's name in the shader.
    pub texture_name: String,
    /// The sampler's name in the shader.
    pub sampler_name: String,
    /// The format the image is uploaded in.
    pub format: PlaneFormat,
    /// Which of the partition's bound textures read lanes of this image, in
    /// lane order.
    pub buffers: Vec<BufferId>,
}

/// One emitted shader: the Bevy module, the standalone core, and what they
/// bind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shader {
    stage: Stage,
    core: String,
    module: String,
    layout: UniformLayout,
    bindings: Vec<TextureBinding>,
    ports: Vec<(String, IrType)>,
}

impl Shader {
    /// Which stage this was written for.
    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// The Bevy module: `#import` lines, `#{MATERIAL_BIND_GROUP}`, the core,
    /// and the stage's entry point.
    ///
    /// This is what `Shader::from_wgsl` is handed. `naga` alone does not parse
    /// it; `naga_oil`, which Bevy preprocesses every shader through, does.
    pub fn module(&self) -> &str {
        &self.module
    }

    /// The generated code with no imports and a literal bind group: the
    /// uniform block, the bindings, the helpers, and the surface function.
    pub fn core(&self) -> &str {
        &self.core
    }

    /// [`Self::core`] with a stub entry point for its stage, which is a module
    /// `naga` parses and validates on its own.
    pub fn standalone(&self) -> String {
        let mut text = self.core.clone();
        text.push_str(&probe(&self.ports));
        text.push_str(STUB_FRAGMENT);
        text
    }

    /// Where each live parameter sits in the uniform block.
    pub fn layout(&self) -> &UniformLayout {
        &self.layout
    }

    /// The images the fragment reads, in binding order.
    pub fn bindings(&self) -> &[TextureBinding] {
        &self.bindings
    }

    /// The PBR ports the surface function answers, with the width each is
    /// carried at.
    pub fn ports(&self) -> &[(String, IrType)] {
        &self.ports
    }
}

/// One emitted compute kernel over the same expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComputeShader {
    module: String,
    layout: UniformLayout,
    bindings: Vec<TextureBinding>,
    ports: Vec<(String, IrType)>,
    resolution: u32,
    mesh: MeshInputs,
    level: u32,
}

impl ComputeShader {
    /// The whole module. It imports nothing, so this is also what is validated.
    pub fn module(&self) -> &str {
        &self.module
    }

    /// Where each live parameter sits in the uniform block.
    pub fn layout(&self) -> &UniformLayout {
        &self.layout
    }

    /// The images the kernel reads, in binding order.
    pub fn bindings(&self) -> &[TextureBinding] {
        &self.bindings
    }

    /// The ports the kernel writes, in the order they appear in the output
    /// buffer.
    ///
    /// Texel `(x, y)` writes `ports().len()` `vec4<f32>`s starting at
    /// `(y * resolution + x) * ports().len()`. A colour port fills the first
    /// three lanes and a scalar port the first, with the rest zero.
    pub fn ports(&self) -> &[(String, IrType)] {
        &self.ports
    }

    /// Texels per side the kernel evaluates.
    pub fn resolution(&self) -> u32 {
        self.resolution
    }

    /// Where the kernel reads the mesh's three inputs from, and so which
    /// bindings a caller has to fill.
    ///
    /// [`MeshInputs::Planes`] declares three storage buffers at
    /// [`CUT_FLAG_BINDING`], [`WORLD_POS_BINDING`] and [`WORLD_NORMAL_BINDING`],
    /// each [`Self::mesh_len`] `vec4<f32>`s — one per texel, in the same
    /// row-major order the output buffer is written in. A position and a normal
    /// leave the fourth lane unread; a cut flag is the first lane and leaves
    /// three.
    pub fn mesh(&self) -> MeshInputs {
        self.mesh
    }

    /// The mip level every bound texture is read at.
    ///
    /// Zero for a dispatch that is a bake or a comparison against the planes a
    /// bake made, which is every caller but one. A level further down is what
    /// asks the other question a texture set exists to answer — *what does this
    /// material look like from further away* — and a caller that asks it has to
    /// upload the chain to read it from, and to dispatch a square that small.
    pub fn level(&self) -> u32 {
        self.level
    }

    /// How many `vec4<f32>`s one of those planes holds: one per texel.
    pub fn mesh_len(&self) -> usize {
        (self.resolution as usize).saturating_mul(self.resolution as usize)
    }

    /// How many `vec4<f32>`s the output buffer holds.
    pub fn output_len(&self) -> usize {
        let texels = (self.resolution as usize).saturating_mul(self.resolution as usize);
        texels.saturating_mul(self.ports.len())
    }
}

/// A stub entry point, so the surface function is reachable from the stage it
/// was written for and `naga` checks it in that context.
///
/// A `textureSample` is legal only inside a fragment stage, and a validator
/// that never entered one would not say so. It is appended by
/// [`Shader::standalone`] rather than carried in [`Shader::core`], because the
/// real module has an entry point of its own.
const STUB_FRAGMENT: &str = r"
@fragment
fn ashlar_validate(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let surface = ashlar_surface(position.xy, position.w, position.xyz, position.xyz, position.w);
    return ashlar_probe(surface);
}
";

/// Emit one stage of a partition.
///
/// ```
/// use ashlar_material::{
///     MaterialGraph, PbrOutput,
///     ir::Target,
///     nodes::{Math, MathOp, Noise, Time},
///     partition::partition,
///     wgsl::{Stage, emit},
/// };
///
/// let material = MaterialGraph::builder("test:strip")
///     .node("glow", Noise::value().period(8))
///     .node("clock", Time::new())
///     .node("pulse", Math::new(MathOp::Sin, "clock", 0.0))
///     .node("lit", Math::new(MathOp::Mul, "glow", "pulse"))
///     .output(PbrOutput::new().emissive("lit"))
///     .build()?;
/// let split = partition(&material, &Target::shader_for(&material), 256)?;
/// let shader = emit(&split, Stage::Fragment)?;
///
/// // One image, one binding pair, and an empty block that is still sixteen
/// // bytes wide so every graph's bind group has the same shape.
/// assert_eq!(shader.bindings().len(), 1);
/// assert_eq!(shader.bindings()[0].texture, 101);
/// assert_eq!(shader.bindings()[0].sampler, 102);
/// assert_eq!(shader.layout().size(), 16);
/// assert!(shader.module().starts_with("#import bevy_pbr::"));
/// assert!(shader.core().contains("fn ashlar_surface("));
/// # Ok::<(), ashlar_material::GraphError>(())
/// ```
pub fn emit(partition: &Partition, stage: Stage) -> Result<Shader, GraphError> {
    let layout = UniformLayout::of(partition.runtime().params());
    let ports: Vec<String> = partition.outputs().keys().cloned().collect();
    let body = Body::build(partition, &layout, Flavour::Fragment, &ports)?;
    let bindings = images(partition, &body.reads)?;
    let core = core_text(&body, &layout, &bindings, &MATERIAL_BIND_GROUP.to_string());
    let mut module = imports(stage, &body);
    module.push_str(&core_text(
        &body,
        &layout,
        &bindings,
        "#{MATERIAL_BIND_GROUP}",
    ));
    module.push_str(&entry(stage, &body));
    Ok(Shader {
        stage,
        core,
        module,
        layout,
        bindings,
        ports: body.ports,
    })
}

/// Emit a compute kernel that evaluates the partition at the texel centres of a
/// `resolution` square.
///
/// The same surface function over the same bindings, dispatched
/// [`WORKGROUP`] by [`WORKGROUP`], writing one `vec4<f32>` per port per texel
/// into a storage buffer. [`emit_compute_mesh`] and [`emit_compute_ports`] are
/// the variants the GPU bake and `ashlar-bevy`'s conformance test dispatch.
///
/// Two things differ from the fragment flavour, and both are forced. A compute
/// stage has no derivatives, so a bound texture is read with
/// `textureSampleLevel` at level zero — which is the level the CPU's own
/// [`Plane`](crate::interp::Plane) is. And it has neither a view nor a mesh, so
/// the four runtime inputs come from a small uniform of its own at
/// [`INPUTS_BINDING`] instead of from Bevy's bindings.
pub fn emit_compute(partition: &Partition, resolution: u32) -> Result<ComputeShader, GraphError> {
    require(
        crate::bake::in_range(resolution),
        "resolution",
        &format!("{resolution} is not a power of two a bake takes"),
    )?;
    let ports: Vec<String> = partition.outputs().keys().cloned().collect();
    emit_compute_ports(partition, resolution, &ports)
}

/// Emit a compute kernel over some of a partition's ports.
///
/// [`emit_compute`] with the port list written out, which is what a backend
/// that evaluates an expression *in pieces* needs: a GPU bake dispatches one
/// kernel per plane — the ports
/// [`partition::plane_port`](crate::partition::plane_port) names on a
/// [`partition::over_planes`](crate::partition::over_planes) split — runs each
/// filter on the plane it read back, and then dispatches the material's own
/// outputs over the finished planes.
///
/// Only what the named ports reach is written, and only the images they
/// actually sample are declared, so an early stage neither computes nor binds
/// the planes that do not exist yet. A port the partition does not carry is
/// skipped rather than refused, the way [`emit_compute`] skips a root a
/// partition has no expression for.
///
/// `resolution` is the square the kernel evaluates, which for a plane that
/// pinned one is that pin rather than the bake's own — so it is not held to a
/// bake's own range here, only to something a dispatch can cover.
pub fn emit_compute_ports(
    partition: &Partition,
    resolution: u32,
    ports: &[String],
) -> Result<ComputeShader, GraphError> {
    emit_compute_mesh(partition, resolution, ports, MeshInputs::Uniform, 0)
}

/// [`emit_compute_ports`] with the mesh inputs' source and the mip level
/// written out.
///
/// The two things a caller of that function cannot say, and a conformance
/// harness has to say both.
///
/// With [`MeshInputs::Planes`] the kernel reads a world position, a world
/// normal and a cut flag per texel out of three storage buffers instead of out
/// of its uniform block, so a graph whose surface depends on where the fragment
/// *is*, or on which side of a cut it is on, can be compared against the
/// interpreter over a mesh that varies. [`MeshInputs`] says why that is worth a
/// second shape of kernel.
///
/// `level` is the mip level every bound texture is read at. Zero is what a bake
/// and every comparison against one wants — it is the level the CPU's own
/// [`Plane`](crate::interp::Plane) is. A level further down is the only way to
/// ask a *compiled* material the question a chain exists to answer: a fragment
/// shader picks its level from derivatives a dispatch does not have, so a
/// harness that wants to see the surface from forty metres names the level
/// instead, uploads the chain, and dispatches the square that level is. That is
/// how the roughness widening is measured against the bake's own, and there is
/// no other way to see it — at level 0 the widening is exactly nothing.
///
/// `resolution` is the square being dispatched, so it is the size of the level
/// being read and not of level 0; `resolution << level` is therefore the
/// material's own resolution, and asking for a level past the end of that chain
/// is refused here rather than silently clamped by the view.
pub fn emit_compute_mesh(
    partition: &Partition,
    resolution: u32,
    ports: &[String],
    mesh: MeshInputs,
    level: u32,
) -> Result<ComputeShader, GraphError> {
    require(
        (1..=crate::bake::MAX_RESOLUTION).contains(&resolution),
        "resolution",
        &format!("{resolution} texels is not a square a kernel evaluates"),
    )?;
    // A level the chain does not have compiles perfectly well and reads
    // whatever the view clamps to, which is a silent wrong answer in the one
    // place — a comparison against the bake — where the whole point is that the
    // answer is right. The level is only meaningful beside the square being
    // dispatched, so this is the pairing rather than the level alone.
    // In `u64`, and not with `checked_shl`: that checks the shift amount and not
    // the value, so a wide level would wrap a `u32` to zero and pass.
    let level_zero = if level < u32::BITS {
        u64::from(resolution) << level
    } else {
        u64::MAX
    };
    require(
        level_zero <= u64::from(crate::bake::MAX_RESOLUTION),
        "level",
        &format!(
            "level {level} of a {resolution}-texel square is level 0 of a chain larger than              {} texels, which no material has",
            crate::bake::MAX_RESOLUTION
        ),
    )?;
    let layout = UniformLayout::of(partition.runtime().params());
    let body = Body::build(partition, &layout, Flavour::Compute { level }, ports)?;
    let bindings = images(partition, &body.reads)?;
    let mut module = String::new();
    module.push_str(COMPUTE_BINDINGS);
    let _ = writeln!(
        module,
        "@group(0) @binding({OUTPUT_BINDING}) var<storage, read_write> ashlar_output: \
         array<vec4<f32>>;"
    );
    let _ = writeln!(
        module,
        "@group(0) @binding({INPUTS_BINDING}) var<uniform> ashlar_inputs: AshlarInputs;"
    );
    if mesh == MeshInputs::Planes {
        let _ = writeln!(
            module,
            "@group(0) @binding({WORLD_POS_BINDING}) var<storage, read> ashlar_world_pos: \
             array<vec4<f32>>;"
        );
        let _ = writeln!(
            module,
            "@group(0) @binding({WORLD_NORMAL_BINDING}) var<storage, read> ashlar_world_normal: \
             array<vec4<f32>>;"
        );
        let _ = writeln!(
            module,
            "@group(0) @binding({CUT_FLAG_BINDING}) var<storage, read> ashlar_cut_flag: \
             array<vec4<f32>>;"
        );
    }
    module.push('\n');
    module.push_str(&core_text(&body, &layout, &bindings, "0"));
    module.push_str(&compute_entry(&body, resolution, mesh));
    Ok(ComputeShader {
        module,
        layout,
        bindings,
        ports: body.ports,
        resolution,
        mesh,
        level,
    })
}

/// The runtime inputs a compute dispatch has to be told, since it has neither a
/// view nor a mesh to read them from.
///
/// Laid out so that nothing pads: two `vec3<f32>`s each followed by the scalar
/// that fits in the fourth lane, which is thirty-two bytes exactly.
const COMPUTE_BINDINGS: &str = r"struct AshlarInputs {
    world_pos: vec3<f32>,
    time: f32,
    world_normal: vec3<f32>,
    cut_flag: f32,
};

";

/// The images the emitted block reads, one per distinct
/// [`BoundTexture::id`](crate::partition::BoundTexture::id).
///
/// `read` is a flag per [`BufferId`], as
/// [`Body::reads`] collects it: a plane the block never samples costs no
/// binding, which is what lets one kernel of a
/// [`over_planes`](crate::partition::over_planes) split bind the planes that
/// already exist and none of the ones it is on the way to making. Two bindings
/// sharing an image are two lanes of it, and the image is declared as soon as
/// either lane is read.
fn images(partition: &Partition, read: &[bool]) -> Result<Vec<TextureBinding>, GraphError> {
    let mut by_image: BTreeMap<u32, TextureBinding> = BTreeMap::new();
    for texture in partition.textures() {
        let Some(id) = texture.id else {
            continue;
        };
        if read.get(texture.buffer.index()).copied() != Some(true) {
            continue;
        }
        let entry = by_image.entry(id).or_insert_with(|| TextureBinding {
            image: id,
            texture: FIRST_TEXTURE_BINDING + 2 * id,
            sampler: FIRST_TEXTURE_BINDING + 2 * id + 1,
            texture_name: format!("ashlar_texture_{id}"),
            sampler_name: format!("ashlar_sampler_{id}"),
            format: texture.format,
            buffers: Vec::new(),
        });
        // One image is one format, so every lane packed into it has to have
        // been given the same one — otherwise the encoder writes the channels
        // of the first plane's format and the shader reads each lane at its
        // own, which is a wrong picture rather than an error. `bind_textures`
        // gives everything that packs one format; this is the check that says
        // so out loud.
        require(
            entry.format == texture.format,
            "output",
            &format!(
                "image {id} would hold a {:?} plane and a {:?} one, and an image is one format",
                entry.format, texture.format
            ),
        )?;
        entry.buffers.push(texture.buffer);
    }
    let bindings: Vec<TextureBinding> = by_image.into_values().collect();
    require(
        bindings.len() <= crate::partition::MAX_BOUND_TEXTURES,
        "output",
        &format!(
            "{} images is past what one compiled material may bind",
            bindings.len()
        ),
    )?;
    Ok(bindings)
}

/// The generated code: the uniform block, the bindings, the helpers, the
/// surface struct and the surface function, with the bind group written as
/// `group`.
fn core_text(
    body: &Body,
    layout: &UniformLayout,
    bindings: &[TextureBinding],
    group: &str,
) -> String {
    let mut text = String::new();
    text.push_str(&params_block(layout, group));
    for binding in bindings {
        let _ = writeln!(
            text,
            "@group({group}) @binding({}) var {}: texture_2d<f32>;",
            binding.texture, binding.texture_name
        );
        let _ = writeln!(
            text,
            "@group({group}) @binding({}) var {}: sampler;",
            binding.sampler, binding.sampler_name
        );
    }
    text.push('\n');
    for helper in &body.helpers {
        text.push_str(&helper.text());
        text.push('\n');
    }
    text.push_str(&body.surface);
    text
}

/// The uniform block, in the order the graph declares its parameters.
fn params_block(layout: &UniformLayout, group: &str) -> String {
    let mut text = format!("struct {PARAMS_STRUCT} {{\n");
    if layout.entries.is_empty() {
        // A struct with no members is not a struct, and the bind group layout
        // must not change shape from one graph to the next, so an empty block
        // is one padded slot.
        text.push_str("    _empty: vec4<f32>,\n");
    } else {
        let mut end = 0;
        for entry in &layout.entries {
            let _ = writeln!(
                text,
                "    {}: {},",
                entry.member,
                wgsl_type(entry.value_type)
            );
            end = entry.offset + entry.size;
        }
        // Round the block to sixteen without a member of its own: `@size` on
        // the tail is the one attribute that says "this member occupies more
        // bytes than its type does" and it is what makes the Rust-side size and
        // the shader's agree.
        let tail = layout.size.saturating_sub(end);
        if tail > 0 {
            let _ = writeln!(text, "    @size({tail}) _tail: f32,");
        }
    }
    text.push_str("};\n");
    let _ = writeln!(
        text,
        "@group({group}) @binding({PARAMS_BINDING}) var<uniform> {PARAMS_VAR}: {PARAMS_STRUCT};"
    );
    text
}

/// A function that folds the surface struct into one colour, so a stub entry
/// point consumes every field whatever the graph bound.
fn probe(ports: &[(String, IrType)]) -> String {
    let mut text = format!("\nfn ashlar_probe(surface: {SURFACE_STRUCT}) -> vec4<f32> {{\n");
    text.push_str("    var total = 0.0;\n");
    for (port, value_type) in ports {
        let term = match value_type {
            IrType::Float => format!("surface.{port}"),
            IrType::Vec2 => format!("surface.{port}.x + surface.{port}.y"),
            IrType::Vec3 => format!("surface.{port}.x + surface.{port}.y + surface.{port}.z"),
        };
        let _ = writeln!(text, "    total = total + {term};");
    }
    text.push_str("    return vec4<f32>(total, total, total, 1.0);\n}\n");
    text
}

/// A generated helper function, emitted once however many instructions call it.
///
/// Each is here because the WGSL built-in that is nearly it differs from the
/// interpreter in a case a graph can reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Helper {
    /// The lattice hash, shared by [`Op::Hash2`] and [`Op::Hash3`].
    Hash,
    /// A division that answers zero rather than an infinity.
    Div(IrType),
    /// A ramp whose meeting edges are a step rather than a division by zero.
    Smoothstep(IrType),
    /// A normalisation that answers zero for a vector of length zero.
    Normalize(IrType),
}

impl Helper {
    fn text(self) -> String {
        match self {
            Self::Hash => HASH.to_owned(),
            Self::Div(width) => {
                let name = wgsl_type(width);
                format!(
                    "fn ashlar_div_{suffix}(a: {name}, b: {name}) -> {name} {{\n    \
                     return select(a / b, {zero}, b == {zero});\n}}\n",
                    suffix = suffix(width),
                    zero = zero(width),
                )
            }
            Self::Smoothstep(width) => {
                let name = wgsl_type(width);
                format!(
                    "fn ashlar_smoothstep_{suffix}(low: {name}, high: {name}, x: {name}) -> \
                     {name} {{\n    \
                     let t = min(max((x - low) / (high - low), {zero}), {one});\n    \
                     let ramp = t * t * ({three} - {two} * t);\n    \
                     return select(ramp, step(low, x), high == low);\n}}\n",
                    suffix = suffix(width),
                    zero = zero(width),
                    one = splat(width, "1.0"),
                    two = splat(width, "2.0"),
                    three = splat(width, "3.0"),
                )
            }
            Self::Normalize(width) => {
                let name = wgsl_type(width);
                let quotient = match width {
                    IrType::Float => "a / magnitude".to_owned(),
                    _ => format!("a / {}(magnitude)", wgsl_type(width)),
                };
                format!(
                    "fn ashlar_normalize_{suffix}(a: {name}) -> {name} {{\n    \
                     let magnitude = sqrt({square});\n    \
                     return select({quotient}, {zero}, magnitude == 0.0);\n}}\n",
                    suffix = suffix(width),
                    square = match width {
                        IrType::Float => "a * a".to_owned(),
                        _ => "dot(a, a)".to_owned(),
                    },
                    zero = zero(width),
                )
            }
        }
    }
}

/// Which of the four runtime inputs an emitted block actually reads.
///
/// Two of the four are worth knowing about and two are not. [`Op::Time`] is the
/// one a fragment cannot get from its own vertex output: a graph with no clock
/// in it should not import Bevy's view bindings for one, and a `0.0` written
/// where the time would go is both smaller and honest. [`Op::CutFlag`] rides in
/// the mesh's second UV set, which is present only on a mesh that has a cut
/// face, so an entry point reading it has to guard the fetch on `VERTEX_UVS_B`
/// — and a graph that never asks should carry neither the guard nor the
/// dependence on a vertex attribute. [`Op::WorldPos`] and [`Op::WorldNormal`]
/// are fields of `VertexOutput` that every mesh has, so they cost nothing to
/// pass and are not tracked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Uses {
    /// The block reads the clock.
    time: bool,
    /// The block reads the mesh's cut flag.
    cut_flag: bool,
}

/// The SSA block, the struct it fills, and what it needed to do it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Body {
    /// The surface struct and the function that answers one.
    surface: String,
    /// The helper functions the block calls, in a stable order.
    helpers: BTreeSet<Helper>,
    /// The ports the struct carries, with the width each is at.
    ports: Vec<(String, IrType)>,
    /// Which runtime inputs the block asked for, so an entry point fetches the
    /// ones it needs and writes `0.0` for the rest.
    uses: Uses,
    /// Which planes the block samples, as a flag per [`BufferId`].
    ///
    /// A block written for one port reads the planes that port reaches and no
    /// others, and only those need a binding — which is what lets one kernel of
    /// a [`over_planes`](crate::partition::over_planes) split bind the planes
    /// that already exist and none of the ones it is on the way to making.
    reads: Vec<bool>,
}

impl Body {
    fn build(
        partition: &Partition,
        layout: &UniformLayout,
        flavour: Flavour,
        wanted_ports: &[String],
    ) -> Result<Self, GraphError> {
        let ir = partition.runtime();
        let mut ports: Vec<(String, IrType)> = Vec::new();
        let mut roots: Vec<ValueId> = Vec::new();
        for port in wanted_ports {
            let Some(root) = ir.root(port) else {
                continue;
            };
            let value_type = ir.type_of(root).unwrap_or(IrType::Float);
            // A PBR port is written at the width its slot is, and a graph that
            // lowered another width into one is a graph to fix. A port that is
            // not one of them — a plane root a compute backend dispatches — is
            // whatever width it lowered to, because nothing downstream has an
            // opinion about it.
            if let Some(wanted) = port_width(port) {
                require(
                    value_type == wanted,
                    &format!("output.{port}"),
                    &format!("{port} lowered to a {value_type} where a shader writes a {wanted}"),
                )?;
            }
            ports.push((port.clone(), value_type));
            roots.push(root);
        }
        let widening = Widening::of(partition, &ports);
        // The coherence is read at the coordinate the normal map is read at, so
        // that expression has to survive even when the normal itself is not a
        // port this body was asked for.
        let mut needed = roots.clone();
        needed.extend(widening.map(|widening| widening.uv));
        let live = ir.reaches(&needed);
        let mut reads = vec![false; ir.buffers().len()];
        for (index, inst) in ir.insts().iter().enumerate() {
            if live.get(index).copied() != Some(true) {
                continue;
            }
            if let Op::Sample(buffer) = inst.op
                && let Some(slot) = reads.get_mut(buffer.index())
            {
                *slot = true;
            }
        }
        // The widening fetch is not an `Op::Sample`, so nothing above it would
        // declare the image it reads. It is the same image the normal port
        // reads, in every case but the one where this body has no normal port.
        if let Some(widening) = widening
            && let Some(slot) = reads.get_mut(widening.buffer.index())
        {
            *slot = true;
        }
        let mut helpers = BTreeSet::new();
        let mut uses = Uses::default();
        let mut block = String::new();
        for (index, inst) in ir.insts().iter().enumerate() {
            if live.get(index).copied() != Some(true) {
                continue;
            }
            let rhs = expression(partition, layout, flavour, *inst, &mut helpers, &mut uses)?;
            let _ = writeln!(
                block,
                "    let v{index}: {} = {rhs};",
                wgsl_type(inst.value_type)
            );
        }
        let mut surface = format!("struct {SURFACE_STRUCT} {{\n");
        for (port, value_type) in &ports {
            let _ = writeln!(surface, "    {port}: {},", wgsl_type(*value_type));
        }
        surface.push_str("};\n\n");
        let _ = writeln!(
            surface,
            "fn {SURFACE_FN}(uv: vec2<f32>, time: f32, world_pos: vec3<f32>, \
             world_normal: vec3<f32>, cut_flag: f32) -> {SURFACE_STRUCT} {{"
        );
        surface.push_str(&block);
        let _ = writeln!(surface, "    var surface: {SURFACE_STRUCT};");
        if let Some(widening) = widening {
            surface.push_str(&widening.coherence(flavour, ir));
        }
        for ((port, _), root) in ports.iter().zip(&roots) {
            match widening {
                Some(_) if port == "roughness" => {
                    surface.push_str(&Widening::roughness(*root));
                }
                _ => {
                    let _ = writeln!(surface, "    surface.{port} = v{};", root.index());
                }
            }
        }
        surface.push_str("    return surface;\n}\n");
        Ok(Self {
            surface,
            helpers,
            ports,
            uses,
            reads,
        })
    }
}

/// The name the generated function holds the sampled coherence under.
const COHERENCE_VAR: &str = "ashlar_coherence";

/// How a compiled material's roughness widens with distance.
///
/// The one thing a baked material got for free and a compiled one did not. A
/// bake filters the normal plane and the roughness plane together, so it can
/// widen the second by how short the first's mean got —
/// `r' = sqrt(r^2 + (1 - |n_avg|) * k)`, level by level, over
/// [`TOKSVIG_K`](crate::mips::TOKSVIG_K). A partition binds separate images
/// that know nothing of each other, so a compiled wall used to keep its
/// close-up roughness at every distance and turn to glass where its baked twin
/// stayed matt.
///
/// What closes it is that the coherence rides in the bound normal map's own
/// alpha channel — [`bound_mips`](crate::bake::bound_mips) puts it there, one
/// number per texel per level, and level 0 is one — and the fragment applies the
/// bake's own formula to whatever the hardware's chosen level handed back. The
/// roughness it widens is the bound roughness at that same level, which is the
/// box-filtered *un-widened* roughness, which is exactly what the bake widens.
/// So the two are the same arithmetic over the same numbers, and the answers
/// agree to the eighth bit rather than merely resembling each other.
///
/// That last sentence is earned *at a level centre*, which is what the
/// conformance case dispatches and what the eighth bit is measured at. A
/// shipped fragment picks a fractional level and an anisotropic sampler
/// averages several taps, so what it actually computes between two levels is
/// `sqrt(lerp(r)^2 + (1 - lerp(a)) * k)` where the bake's own chain would have
/// handed the sampler `lerp(sqrt(r^2 + (1 - a) * k))`. The two differ at second
/// order in how far apart the levels are, they agree wherever the level lands
/// on a centre, and no fractional level puts the answer outside the pair it is
/// interpolating between. It is the difference between two filters of the same
/// curve, not between two curves.
///
/// Two more things the phrase "at that same level" is quietly assuming, both
/// reachable and neither wrong enough to refuse. A node that pins one plane's
/// resolution ([`BoundTexture::resolution`](crate::partition::BoundTexture))
/// gives the normal map and the roughness map different chains, so the level
/// the coherence is read at is the normal image's rather than the roughness
/// image's. And a roughness whose expression samples an interior plane at a
/// warped or scaled coordinate describes a different footprint from the one the
/// coherence was measured over. In both the term is approximately right rather
/// than exactly so — it is still the coherence of a neighbourhood of this
/// surface at about this distance — and in both, no study graph is in that
/// position: every bound plane is the partition's own resolution, sampled at
/// the one `uv` the cut hands out.
///
/// Six instructions and no extra fetch: the alpha comes off a read of an image
/// the shader is already sampling at the same coordinate, which every compiler
/// in the stack collapses into one. And it holds whether the roughness is a
/// texture read, a literal or eleven instructions of live parameter — which is
/// the reason it is done here rather than by widening the bound roughness map's
/// own chain at bake time. That would be cheaper and would work for a static
/// roughness; the study's wet concrete, whose roughness is a `Mix` against a
/// uniform, is precisely the material it would do nothing for.
#[derive(Clone, Copy)]
struct Widening {
    /// The bound normal map's image number, whose alpha is `|n_avg|`.
    image: u32,
    /// Which bound texture that is, so the body declares its image.
    buffer: BufferId,
    /// The coordinate the normal map is read at, which is where the coherence
    /// is read too: the same texel, and so the same footprint.
    uv: ValueId,
}

impl Widening {
    /// What this body widens, or `None` where nothing does.
    ///
    /// [`Partition::widening`] has already decided whether the *material* can
    /// widen at all; what is left here is whether this particular body is
    /// writing a roughness, and where in the expression the normal map's
    /// coordinate is. A body emitted for a subset of ports that leaves the
    /// roughness out writes no widening, because it writes no roughness.
    fn of(partition: &Partition, ports: &[(String, IrType)]) -> Option<Self> {
        if !ports.iter().any(|(port, _)| port == "roughness") {
            return None;
        }
        let buffer = partition.widening()?;
        let image = partition.textures().get(buffer.index())?.id?;
        let ir = partition.runtime();
        let inst = ir.inst(ir.root("normal")?)?;
        // A bound normal output is exactly one sample of its own plane; the
        // partition said so, and this reads it back rather than assuming it.
        match inst.op {
            Op::Sample(sampled) if sampled == buffer => Some(Self {
                image,
                buffer,
                uv: *inst.operands.as_slice().first()?,
            }),
            _ => None,
        }
    }

    /// The line that reads `|n_avg|` off the normal map.
    fn coherence(self, flavour: Flavour, ir: &crate::ir::Ir) -> String {
        let uv = self.uv.index();
        let width = ir.type_of(self.uv).unwrap_or(IrType::Vec2);
        // A coordinate is a `vec2`; the narrowing is here so that a lowering
        // that handed over something else is a shader that still compiles.
        let coordinate = match width {
            IrType::Vec2 => format!("v{uv}"),
            _ => format!("v{uv}.xy"),
        };
        format!(
            "    // The coherence of this footprint's normals, which the chain \
             carries\n    // in the alpha of the normal map; one at level 0, and \
             shorter the\n    // more the normals under a texel disagree.\n    \
             let {COHERENCE_VAR}: f32 = {}.a;\n",
            fetch(flavour, self.image, &coordinate)
        )
    }

    /// The roughness the surface ships: the level's own, widened by what that
    /// level's normals lost, and never past one.
    ///
    /// An associated function rather than a method because the line depends on
    /// nothing but the roughness it is widening: which image the coherence came
    /// out of is [`Self::coherence`]'s business, and by here it is a name.
    fn roughness(root: ValueId) -> String {
        let rough = format!("v{}", root.index());
        format!(
            "    surface.roughness = min(sqrt({rough} * {rough} + (1.0 - {COHERENCE_VAR}) * {}), \
             1.0);\n",
            literal(crate::mips::TOKSVIG_K)
        )
    }
}

/// The width a PBR port is written at, or `None` for a port that is not one.
///
/// The PBR slots have widths a graph has to meet. Every other port a partition
/// can carry is a plane root named by
/// [`plane_port`](crate::partition::plane_port), and a plane holds whatever its
/// own expression lowered to.
fn port_width(port: &str) -> Option<IrType> {
    match port {
        "base_color" | "emissive" | "normal" => Some(IrType::Vec3),
        "roughness" | "metallic" | "occlusion" | "height" => Some(IrType::Float),
        _ => None,
    }
}

/// One instruction's right-hand side.
///
/// Every arm is the arithmetic [`interp::apply`](crate::interp) does, and where
/// the two could differ the difference is in a helper rather than in a built-in
/// that is nearly right.
fn expression(
    partition: &Partition,
    layout: &UniformLayout,
    flavour: Flavour,
    inst: Inst,
    helpers: &mut BTreeSet<Helper>,
    uses: &mut Uses,
) -> Result<String, GraphError> {
    let ir = partition.runtime();
    let at = |slot: usize| -> String {
        inst.operands
            .as_slice()
            .get(slot)
            .map_or_else(|| zero(IrType::Float), |id| format!("v{}", id.index()))
    };
    // The width a component-wise op works at, which `Lowering::align` made the
    // width of operand zero.
    let width = inst
        .operands
        .as_slice()
        .first()
        .and_then(|id| ir.type_of(*id))
        .unwrap_or(inst.value_type);
    let (a, b, c) = (at(0), at(1), at(2));
    Ok(match inst.op {
        Op::Const(value) => literal(value),
        Op::Uv => "uv".to_owned(),
        Op::Param(index) => {
            let entry = layout.entries.get(index as usize).ok_or_else(|| {
                GraphError::new(
                    "params",
                    format!("the expression reads uniform {index}, and the block has none"),
                )
            })?;
            format!("{PARAMS_VAR}.{}", entry.member)
        }
        Op::Time => {
            uses.time = true;
            "time".to_owned()
        }
        Op::WorldPos => "world_pos".to_owned(),
        Op::WorldNormal => "world_normal".to_owned(),
        Op::CutFlag => {
            uses.cut_flag = true;
            "cut_flag".to_owned()
        }
        Op::Add => format!("({a} + {b})"),
        Op::Sub => format!("({a} - {b})"),
        Op::Mul => format!("({a} * {b})"),
        Op::Div => {
            helpers.insert(Helper::Div(width));
            format!("ashlar_div_{}({a}, {b})", suffix(width))
        }
        Op::Min => format!("min({a}, {b})"),
        Op::Max => format!("max({a}, {b})"),
        Op::Abs => format!("abs({a})"),
        Op::Floor => format!("floor({a})"),
        Op::Fract => format!("fract({a})"),
        Op::Sqrt => format!("sqrt({a})"),
        Op::Pow => format!("pow({a}, {b})"),
        Op::Exp2 => format!("exp2({a})"),
        Op::Log2 => format!("log2({a})"),
        Op::Sin => format!("sin({a})"),
        Op::Cos => format!("cos({a})"),
        Op::Atan2 => format!("atan2({a}, {b})"),
        // `mix` in WGSL is `a * (1 - t) + b * t`, which rounds differently.
        Op::Mix => format!("({a} + ({b} - {a}) * {c})"),
        Op::Step => format!("step({a}, {b})"),
        Op::Smoothstep => {
            helpers.insert(Helper::Smoothstep(width));
            format!("ashlar_smoothstep_{}({a}, {b}, {c})", suffix(width))
        }
        // `clamp` is this, except that WGSL leaves it indeterminate where the
        // bounds cross and a graph is allowed to cross them.
        Op::Clamp => format!("min(max({a}, {b}), {c})"),
        // The interpreter reads the condition's first lane whatever width the
        // value has, and a WGSL relational operator does not broadcast a scalar
        // against a vector, so a condition that is not already a scalar is
        // narrowed to the lane the interpreter read.
        Op::Select => format!("select({c}, {b}, {} >= 0.5)", extract(&a, width, 0)),
        Op::Hash2(seed) => {
            helpers.insert(Helper::Hash);
            format!("ashlar_hash2({a}, {b}, {seed}u)")
        }
        Op::Hash3(seed) => {
            helpers.insert(Helper::Hash);
            format!("ashlar_hash3({a}, {b}, {seed}u)")
        }
        // The interpreter computes both over three lanes with the lanes the
        // type does not own left zero, so a scalar's length is its magnitude
        // and a scalar's dot product is a multiply.
        Op::Length => match width {
            IrType::Float => format!("abs({a})"),
            _ => format!("sqrt(dot({a}, {a}))"),
        },
        Op::Dot => match width {
            IrType::Float => format!("({a} * {b})"),
            _ => format!("dot({a}, {b})"),
        },
        Op::Normalize => {
            helpers.insert(Helper::Normalize(width));
            format!("ashlar_normalize_{}({a})", suffix(width))
        }
        Op::Sample(buffer) => sample(partition, flavour, buffer, &a)?,
        Op::Compose => match inst.operands.len() {
            3 => format!("vec3<f32>({a}, {b}, {c})"),
            _ => format!("vec2<f32>({a}, {b})"),
        },
        Op::Extract(channel) => extract(&a, width, channel),
    })
}

/// One texture read, swizzled to the lanes this binding owns and decoded if the
/// image it lives in cannot hold what the plane held.
///
/// The one decode is a normal map. A [`Filter::Normal`] plane runs `-1..=1` and
/// the bake writes it into an eight-bit unorm image half and half about zero,
/// so a shader reading that image undoes exactly that. Every other bound plane
/// is already the numbers it was: an sRGB base colour is converted by the
/// sampler, a height is a unorm `0..=1`, and an interior cut is half floats,
/// which clamp nothing.
fn sample(
    partition: &Partition,
    flavour: Flavour,
    buffer: BufferId,
    uv: &str,
) -> Result<String, GraphError> {
    let texture = partition
        .textures()
        .get(buffer.index())
        .ok_or_else(|| GraphError::new("partition", format!("no bound texture for {buffer}")))?;
    let image = texture.id.ok_or_else(|| {
        GraphError::new(
            "partition",
            format!(
                "{buffer} is a plane no fragment was supposed to read, and the shader reads it"
            ),
        )
    })?;
    let read = fetch(flavour, image, uv);
    let lanes = ["r", "g", "b", "a"];
    let swizzle = match texture.value_type {
        IrType::Float => (*lanes.get(usize::from(texture.lane)).ok_or_else(|| {
            GraphError::new("partition", format!("{buffer} has no lane to read"))
        })?)
        .to_owned(),
        IrType::Vec2 => "rg".to_owned(),
        IrType::Vec3 => "rgb".to_owned(),
    };
    let read = format!("{read}.{swizzle}");
    let encoded = matches!(texture.filter, Filter::Normal { .. })
        && matches!(
            texture.format,
            PlaneFormat::Rgba8Unorm | PlaneFormat::Rgba8Srgb | PlaneFormat::R16Unorm
        );
    Ok(if encoded {
        format!("({read} * 2.0 - 1.0)")
    } else {
        read
    })
}

/// One whole `vec4` read of a bound image, before any swizzle or decode.
///
/// The one place a texture fetch is spelled, because two things make one: an
/// [`Op::Sample`], and the coherence the roughness widens by, which is the
/// alpha of a fetch the shader was making anyway. A fragment picks its own
/// level from the coordinate's derivatives; a compute stage has none and is
/// told which level to read.
fn fetch(flavour: Flavour, image: u32, uv: &str) -> String {
    match flavour {
        Flavour::Fragment => {
            format!("textureSample(ashlar_texture_{image}, ashlar_sampler_{image}, {uv})")
        }
        Flavour::Compute { level } => format!(
            "textureSampleLevel(ashlar_texture_{image}, ashlar_sampler_{image}, {uv}, {level}.0)"
        ),
    }
}

/// One lane of a value, or a zero where the value has no such lane.
fn extract(value: &str, width: IrType, channel: u8) -> String {
    let lane = ["x", "y", "z"]
        .get(usize::from(channel))
        .filter(|_| usize::from(channel) < width.components());
    match (lane, width) {
        (None, _) => zero(IrType::Float),
        (Some(_), IrType::Float) => value.to_owned(),
        (Some(lane), _) => format!("{value}.{lane}"),
    }
}

/// The `#import` lines one stage needs.
///
/// Preprocessor directives, not WGSL: `naga_oil` resolves them and plain
/// `naga` does not, which is why [`Shader::core`] carries none of them.
fn imports(stage: Stage, body: &Body) -> String {
    let mut text = String::new();
    match stage {
        Stage::Fragment => text.push_str(
            "#import bevy_pbr::{\n    \
             pbr_fragment::pbr_input_from_standard_material,\n    \
             pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing, \
             calculate_tbn_mikktspace},\n    \
             pbr_types,\n    \
             forward_io::{VertexOutput, FragmentOutput},\n}\n",
        ),
        Stage::Prepass => {
            text.push_str(
                "#import bevy_pbr::{\n    \
                 prepass_io::{VertexOutput, FragmentOutput},\n    \
                 pbr_prepass_functions,\n    \
                 pbr_bindings,\n    \
                 pbr_types,\n    \
                 pbr_functions,\n    \
                 mesh_bindings::mesh,\n}\n",
            );
            text.push_str(
                "#ifdef BINDLESS\n#import bevy_pbr::pbr_bindings::material_indices\n#endif\n",
            );
        }
    }
    // The clock lives in two different places. In the main pass the view bind
    // group is `mesh_view_bindings`, where `globals` is binding 11 and is
    // imported by name. In the *prepass* the view bind group is a smaller one
    // of the prepass's own, where the same buffer is bound at binding 1 and no
    // shipped module declares it — Bevy binds it there and none of its own
    // prepass shaders read it. So the prepass declares it here, from the same
    // struct, or the pipeline is refused with "binding 11 is not available in
    // the pipeline layout".
    if body.uses.time {
        match stage {
            Stage::Fragment => text.push_str("#import bevy_pbr::mesh_view_bindings::globals\n"),
            Stage::Prepass => text.push_str(
                "#import bevy_render::globals::Globals\n\n\
                 @group(0) @binding(1) var<uniform> ashlar_globals: Globals;\n",
            ),
        }
    }
    text.push('\n');
    text
}

/// What the entry point passes the surface function for the clock.
///
/// Two names for one buffer, because the two stages bind it in two different
/// groups; see [`imports`].
fn clock(stage: Stage, body: &Body) -> &'static str {
    match (body.uses.time, stage) {
        (false, _) => "0.0",
        (true, Stage::Fragment) => "globals.time",
        (true, Stage::Prepass) => "ashlar_globals.time",
    }
}

/// What a fragment entry point fetches the mesh's cut flag with, and the name
/// it passes the surface function.
///
/// `ashlar-bevy` uploads the flag as the mesh's second UV set — its
/// `ATTRIBUTE_ASHLAR_CUT` says why that slot and not a new one — and Bevy's own
/// vertex stages forward `uv_b` in both the main pass and the prepass under
/// `VERTEX_UVS_B`, which the pipeline defines from the mesh's own layout. A
/// mesh with no cut face carries no such attribute, so the fetch is guarded and
/// the unguarded arm is zero: an uncut surface, which is what a mesh with
/// nothing cut out of it is.
///
/// Nothing is emitted at all for a graph that does not read the flag, so a
/// shader only depends on a vertex attribute when its material asked to.
fn cut_flag(body: &Body) -> (&'static str, &'static str) {
    if body.uses.cut_flag {
        (
            "#ifdef VERTEX_UVS_B\n    \
             let ashlar_cut = in.uv_b.x;\n\
             #else\n    \
             let ashlar_cut = 0.0;\n\
             #endif\n",
            "ashlar_cut",
        )
    } else {
        ("", "0.0")
    }
}

/// The entry point for one stage.
///
/// Not validated here: it names Bevy's own structs and functions, which this
/// crate has no declarations for, and it carries `#ifdef`s that only the
/// preprocessor resolves. The Bevy step compiling one is what checks it.
fn entry(stage: Stage, body: &Body) -> String {
    match stage {
        Stage::Fragment => fragment_entry(body),
        Stage::Prepass => prepass_entry(body),
    }
}

/// The main-pass fragment.
///
/// The graph's answers *multiply* into the PBR slots rather than replacing
/// them, because that is what the same material's baked textures do: a base
/// colour map is multiplied by `MaterialDefinition::base_color`, a
/// metallic-roughness map by the two constants, and an occlusion map into
/// `diffuse_occlusion`. A shader material and its baked twin have to be the
/// same picture, and the constants are half of it.
///
/// The coordinate is `in.uv` through the material's own `uv_transform`, which
/// is where `tile_metres` and `uv_offset` live, so a repeat of the graph is a
/// repeat of the wall. It is not wrapped into `0..1`: the graph's period
/// inference already guarantees the field repeats with period one, and a
/// `fract` here would put a derivative discontinuity along every seam and a
/// visible line of the wrong mip with it.
fn fragment_entry(body: &Body) -> String {
    let mut text = String::from(
        "\n@fragment\nfn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> \
         FragmentOutput {\n    \
         var pbr_input = pbr_input_from_standard_material(in, is_front);\n    \
         let ashlar_uv = (pbr_input.material.uv_transform * vec3<f32>(in.uv, 1.0)).xy;\n",
    );
    let (fetch, cut) = cut_flag(body);
    text.push_str(fetch);
    let _ = writeln!(
        text,
        "    let surface = {SURFACE_FN}(ashlar_uv, {}, in.world_position.xyz, in.world_normal, \
         {cut});",
        clock(Stage::Fragment, body)
    );
    for (port, _) in &body.ports {
        let line = match port.as_str() {
            "base_color" => Some(
                "    pbr_input.material.base_color = vec4<f32>(\n        \
                 pbr_input.material.base_color.rgb * surface.base_color,\n        \
                 pbr_input.material.base_color.a,\n    );"
                    .to_owned(),
            ),
            "roughness" => Some(
                "    pbr_input.material.perceptual_roughness = \
                 pbr_input.material.perceptual_roughness * surface.roughness;"
                    .to_owned(),
            ),
            "metallic" => Some(
                "    pbr_input.material.metallic = pbr_input.material.metallic * \
                 surface.metallic;"
                    .to_owned(),
            ),
            "occlusion" => Some(
                "    pbr_input.diffuse_occlusion = pbr_input.diffuse_occlusion * \
                 vec3<f32>(surface.occlusion);"
                    .to_owned(),
            ),
            "emissive" => Some(
                "    pbr_input.material.emissive = vec4<f32>(\n        \
                 pbr_input.material.emissive.rgb * surface.emissive,\n        \
                 pbr_input.material.emissive.a,\n    );"
                    .to_owned(),
            ),
            // UV-derived normals store -dH/dV with rows increasing downward.
            // Convert Y to Bevy's OpenGL/Mikk basis, as the baked adapter does.
            // The back-face flip is what `apply_normal_mapping` would have done, and this
            // shader does not call it: the back face of a double-sided material
            // reads its tangent-space normal the other way up.
            //
            // A tangent-space normal needs a tangent, and `VertexOutput`
            // carries one only under `VERTEX_TANGENTS`. A mesh without the
            // attribute keeps the interpolated geometric normal
            // `pbr_input_from_standard_material` already put in `N`, which is
            // what Bevy's own `pbr_fragment` does — a flat surface rather than
            // a pipeline that will not build.
            "normal" => Some(
                "#ifdef VERTEX_TANGENTS\n    \
                 let ashlar_double_sided = (pbr_input.material.flags\n        \
                 & pbr_types::STANDARD_MATERIAL_FLAGS_DOUBLE_SIDED_BIT) != 0u;\n    \
                 var ashlar_normal_ts = surface.normal * vec3<f32>(1.0, -1.0, 1.0);\n    \
                 if ashlar_double_sided && !is_front {\n        \
                 ashlar_normal_ts = -ashlar_normal_ts;\n    }\n    \
                 let tbn = calculate_tbn_mikktspace(pbr_input.world_normal, \
                 in.world_tangent);\n    \
                 pbr_input.N = normalize(tbn * ashlar_normal_ts);\n\
                 #endif"
                    .to_owned(),
            ),
            // `height` is baked and addressable and bound to no slot, exactly
            // as a baked surface's height map is.
            _ => None,
        };
        if let Some(line) = line {
            let _ = writeln!(text, "{line}");
        }
    }
    text.push_str(
        "    var out: FragmentOutput;\n    \
         out.color = apply_pbr_lighting(pbr_input);\n    \
         out.color = main_pass_post_lighting_processing(pbr_input, out.color);\n    \
         return out;\n}\n",
    );
    text
}

/// The prepass fragment, writing the same normal.
///
/// Shaped like Bevy's own `pbr_prepass.wgsl`: the whole fragment lives under
/// `PREPASS_FRAGMENT`, the normal under `NORMAL_PREPASS`, and the depth-only
/// variant is the alpha discard alone. A graph that derives no normal emits the
/// default prepass and nothing else, because there is nothing for it to say.
fn prepass_entry(body: &Body) -> String {
    let has_normal = body.ports.iter().any(|(port, _)| port == "normal");
    let mut text = String::from(
        "\n#ifdef PREPASS_FRAGMENT\n@fragment\nfn fragment(in: VertexOutput, \
         @builtin(front_facing) is_front: bool) -> FragmentOutput {\n    \
         pbr_prepass_functions::prepass_alpha_discard(in);\n    \
         var out: FragmentOutput;\n\
         #ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION\n    \
         out.frag_depth = in.unclipped_depth;\n\
         #endif\n",
    );
    if has_normal {
        text.push_str(
            "#ifdef NORMAL_PREPASS\n\
             #ifdef BINDLESS\n    \
             let ashlar_slot = mesh[in.instance_index].material_and_lightmap_bind_group_slot \
             & 0xffffu;\n    \
             let ashlar_material =\n        \
             pbr_bindings::material_array[material_indices[ashlar_slot].material];\n\
             #else\n    \
             let ashlar_material = pbr_bindings::material;\n\
             #endif\n    \
             let ashlar_uv = (ashlar_material.uv_transform * vec3<f32>(in.uv, 1.0)).xy;\n    \
             let ashlar_double_sided = (ashlar_material.flags\n        \
             & pbr_types::STANDARD_MATERIAL_FLAGS_DOUBLE_SIDED_BIT) != 0u;\n    \
             let ashlar_world_normal = pbr_functions::prepare_world_normal(\n        \
             in.world_normal, ashlar_double_sided, is_front);\n",
        );
        let (fetch, cut) = cut_flag(body);
        text.push_str(fetch);
        let _ = writeln!(
            text,
            "    let surface = {SURFACE_FN}(ashlar_uv, {}, in.world_position.xyz, \
             ashlar_world_normal, {cut});",
            clock(Stage::Prepass, body)
        );
        // As in the main pass: without the tangent attribute there is no
        // tangent space to put the normal in, so the prepass writes the
        // geometric normal, which is what it would have written for a mesh with
        // no normal map at all.
        text.push_str(
            "#ifdef VERTEX_TANGENTS\n    \
             let tbn = pbr_functions::calculate_tbn_mikktspace(ashlar_world_normal, \
             in.world_tangent);\n    \
             var ashlar_normal_ts = surface.normal * vec3<f32>(1.0, -1.0, 1.0);\n    \
             if ashlar_double_sided && !is_front {\n        \
             ashlar_normal_ts = -ashlar_normal_ts;\n    }\n    \
             let ashlar_normal = normalize(tbn * ashlar_normal_ts);\n\
             #else\n    \
             let ashlar_normal = ashlar_world_normal;\n\
             #endif\n    \
             out.normal = vec4<f32>(ashlar_normal * 0.5 + vec3<f32>(0.5), 1.0);\n\
             #endif\n",
        );
    }
    text.push_str(
        "#ifdef MOTION_VECTOR_PREPASS\n    \
         out.motion_vector =\n        \
         pbr_prepass_functions::calculate_motion_vector(in.world_position, \
         in.previous_world_position);\n\
         #endif\n    \
         return out;\n}\n\
         #else\n@fragment\nfn fragment(in: VertexOutput) {\n    \
         pbr_prepass_functions::prepass_alpha_discard(in);\n}\n\
         #endif\n",
    );
    text
}

/// The compute entry point: one invocation per texel, writing every port.
fn compute_entry(body: &Body, resolution: u32, mesh: MeshInputs) -> String {
    let ports = body.ports.len();
    let mut text = String::new();
    let _ = writeln!(text, "\nconst ASHLAR_RESOLUTION: u32 = {resolution}u;");
    let _ = writeln!(text, "const ASHLAR_PORTS: u32 = {ports}u;\n");
    let _ = writeln!(
        text,
        "@compute @workgroup_size({WORKGROUP}, {WORKGROUP}, 1)\nfn main(@builtin(global_invocation_id) \
         id: vec3<u32>) {{"
    );
    text.push_str(
        "    if id.x >= ASHLAR_RESOLUTION || id.y >= ASHLAR_RESOLUTION {\n        return;\n    }\n",
    );
    // The centre of the texel's cell, which is where the rasteriser evaluates
    // it and where a plane reads back exactly.
    text.push_str(
        "    let uv = (vec2<f32>(f32(id.x), f32(id.y)) + vec2<f32>(0.5, 0.5)) / \
         f32(ASHLAR_RESOLUTION);\n",
    );
    text.push_str("    let texel = id.y * ASHLAR_RESOLUTION + id.x;\n");
    // The same row-major index the output is written at, so a caller lays a
    // mesh plane out the way it reads the answers back.
    let (position, normal, cut) = match mesh {
        MeshInputs::Uniform => (
            "ashlar_inputs.world_pos",
            "ashlar_inputs.world_normal",
            "ashlar_inputs.cut_flag",
        ),
        MeshInputs::Planes => (
            "ashlar_world_pos[texel].xyz",
            "ashlar_world_normal[texel].xyz",
            "ashlar_cut_flag[texel].x",
        ),
    };
    let _ = writeln!(
        text,
        "    let surface = {SURFACE_FN}(uv, ashlar_inputs.time, {position}, {normal}, {cut});"
    );
    text.push_str("    let base = texel * ASHLAR_PORTS;\n");
    for (slot, (port, value_type)) in body.ports.iter().enumerate() {
        let value = match value_type {
            IrType::Float => format!("vec4<f32>(surface.{port}, 0.0, 0.0, 0.0)"),
            IrType::Vec2 => format!("vec4<f32>(surface.{port}, 0.0, 0.0)"),
            IrType::Vec3 => format!("vec4<f32>(surface.{port}, 0.0)"),
        };
        let _ = writeln!(text, "    ashlar_output[base + {slot}u] = {value};");
    }
    text.push_str("}\n");
    text
}

/// The WGSL spelling of an IR width.
fn wgsl_type(value_type: IrType) -> &'static str {
    match value_type {
        IrType::Float => "f32",
        IrType::Vec2 => "vec2<f32>",
        IrType::Vec3 => "vec3<f32>",
    }
}

/// The suffix a per-width helper is named by.
fn suffix(value_type: IrType) -> &'static str {
    match value_type {
        IrType::Float => "f32",
        IrType::Vec2 => "vec2",
        IrType::Vec3 => "vec3",
    }
}

/// A zero of one width.
fn zero(value_type: IrType) -> String {
    splat(value_type, "0.0")
}

/// One value at every lane of a width.
fn splat(value_type: IrType, value: &str) -> String {
    match value_type {
        IrType::Float => value.to_owned(),
        _ => format!("{}({value})", wgsl_type(value_type)),
    }
}

/// The alignment and size, in bytes, one width occupies in a uniform block.
///
/// WGSL's own rules, which for these three types are std140's: a scalar is four
/// and four, a `vec2<f32>` eight and eight, and a `vec3<f32>` **sixteen** and
/// twelve — aligned as if it were four components and sized as if it were
/// three, which is the rule the padding in every such block comes from.
fn shape(value_type: IrType) -> (u32, u32) {
    match value_type {
        IrType::Float => (4, 4),
        IrType::Vec2 => (8, 8),
        IrType::Vec3 => (16, 12),
    }
}

/// The next multiple of `align` at or above `value`.
fn round_up(value: u32, align: u32) -> u32 {
    let align = align.max(1);
    value.div_ceil(align).saturating_mul(align)
}

/// One `f32` as WGSL source.
///
/// Finite values are printed at the shortest decimal that reads back as the
/// same `f32`, with the `f` suffix that makes the literal concrete rather than
/// an abstract float the compiler converts. A value that is not finite has no
/// WGSL literal at all — there is no `inf` and no `nan` — and folding a graph
/// can produce one, `log2(0)` being the ordinary way, so those are written as
/// the bits they are. Exact either way: a constant that changed on the way into
/// the shader would be a different material.
fn literal(value: f32) -> String {
    if value.is_finite() {
        format!("{value:?}f")
    } else {
        format!("bitcast<f32>({:#010x}u)", value.to_bits())
    }
}

/// A parameter's name as a WGSL struct member.
///
/// The graph's own name wherever it is an identifier WGSL will take, because
/// `ashlar.wear` is what an author reading the generated shader is looking for.
/// A name that is not — one with a space or a dash in it, one that starts with
/// a digit, one that is a WGSL keyword or a reserved word or a predeclared type
/// — is sanitised and, if that still collides, suffixed with its binding index.
/// The graph's name stays in [`ParamLayout::name`], which is what a caller
/// looks a parameter up by, so nothing downstream has to know this happened.
fn member_name(name: &str, index: usize, used: &mut BTreeSet<String>) -> String {
    let mut sanitised: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if sanitised
        .chars()
        .next()
        .is_none_or(|c| !c.is_ascii_alphabetic() && c != '_')
    {
        sanitised.insert(0, '_');
    }
    // WGSL reserves `__` as a prefix for the implementation.
    if sanitised.starts_with("__") {
        sanitised.insert(0, 'p');
    }
    if RESERVED.contains(&sanitised.as_str()) || used.contains(&sanitised) {
        sanitised = format!("{sanitised}_{index}");
    }
    used.insert(sanitised.clone());
    sanitised
}

/// The names a WGSL struct member may not have: the keywords, the reserved
/// words and the predeclared type names, as the specification lists them.
///
/// Written out rather than guessed at. A parameter called `filter` or `mix` or
/// `set` is an ordinary thing for an author to write, and a shader that failed
/// to compile because of one would fail a long way from the graph that caused
/// it.
const RESERVED: &[&str] = &[
    // Keywords.
    "alias",
    "break",
    "case",
    "const",
    "const_assert",
    "continue",
    "continuing",
    "default",
    "diagnostic",
    "discard",
    "else",
    "enable",
    "false",
    "fn",
    "for",
    "if",
    "let",
    "loop",
    "override",
    "requires",
    "return",
    "struct",
    "switch",
    "true",
    "var",
    "while",
    // Predeclared types and type-generators.
    "array",
    "atomic",
    "bool",
    "f16",
    "f32",
    "i32",
    "mat2x2",
    "mat2x3",
    "mat2x4",
    "mat3x2",
    "mat3x3",
    "mat3x4",
    "mat4x2",
    "mat4x3",
    "mat4x4",
    "ptr",
    "sampler",
    "sampler_comparison",
    "texture_1d",
    "texture_2d",
    "texture_2d_array",
    "texture_3d",
    "texture_cube",
    "texture_cube_array",
    "texture_depth_2d",
    "texture_depth_2d_array",
    "texture_depth_cube",
    "texture_depth_cube_array",
    "texture_depth_multisampled_2d",
    "texture_external",
    "texture_multisampled_2d",
    "texture_storage_1d",
    "texture_storage_2d",
    "texture_storage_2d_array",
    "texture_storage_3d",
    "u32",
    "vec2",
    "vec3",
    "vec4",
    // Reserved words.
    "NULL",
    "Self",
    "abstract",
    "active",
    "alignas",
    "alignof",
    "as",
    "asm",
    "asm_fragment",
    "async",
    "attribute",
    "auto",
    "await",
    "become",
    "binding_array",
    "cast",
    "catch",
    "class",
    "co_await",
    "co_return",
    "co_yield",
    "coherent",
    "column_major",
    "common",
    "compile",
    "compile_fragment",
    "concept",
    "const_cast",
    "consteval",
    "constexpr",
    "constinit",
    "crate",
    "debugger",
    "decltype",
    "delete",
    "demote",
    "demote_to_helper",
    "do",
    "dynamic_cast",
    "enum",
    "explicit",
    "export",
    "extends",
    "extern",
    "external",
    "fallthrough",
    "filter",
    "final",
    "finally",
    "friend",
    "from",
    "fxgroup",
    "get",
    "goto",
    "groupshared",
    "highp",
    "impl",
    "implements",
    "import",
    "inline",
    "instanceof",
    "interface",
    "layout",
    "lowp",
    "macro",
    "macro_rules",
    "match",
    "mediump",
    "meta",
    "mod",
    "module",
    "move",
    "mut",
    "mutable",
    "namespace",
    "new",
    "nil",
    "noexcept",
    "noinline",
    "nointerpolation",
    "non_coherent",
    "noncoherent",
    "noperspective",
    "null",
    "nullptr",
    "of",
    "operator",
    "package",
    "packoffset",
    "partition",
    "pass",
    "patch",
    "pixelfragment",
    "precise",
    "precision",
    "premerge",
    "priv",
    "protected",
    "pub",
    "public",
    "readonly",
    "ref",
    "regardless",
    "register",
    "reinterpret_cast",
    "require",
    "resource",
    "restrict",
    "self",
    "set",
    "shared",
    "sizeof",
    "smooth",
    "snorm",
    "static",
    "static_assert",
    "static_cast",
    "std",
    "subroutine",
    "super",
    "target",
    "template",
    "this",
    "thread_local",
    "throw",
    "trait",
    "try",
    "type",
    "typedef",
    "typeid",
    "typename",
    "typeof",
    "union",
    "unless",
    "unorm",
    "unsafe",
    "unsized",
    "use",
    "using",
    "varying",
    "virtual",
    "volatile",
    "wgsl",
    "where",
    "with",
    "writeonly",
    "yield",
];
