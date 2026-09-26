//! Wind: the one thing about a strand layer that moves.
//!
//! [`strands`](crate::strands) answers geometry that does not change between
//! frames, and it says why at length: a strand set is scattered from graph
//! fields, the fields are static by decision, and a
//! `Live` parameter reaching one is folded.
//! That leaves exactly one thing a lawn needs and the scatter cannot give it,
//! and this module is that thing. It is a vertex stage: the strands stay where
//! they were planted, and what moves is where their tips are drawn.
//!
//! # What it does
//!
//! Every strand mesh carries
//! [`crate::strands::ATTRIBUTE_STRAND_WIND`] -
//! `[phase, v, stiffness]`, written since phase 2 and read by nothing until
//! now. The vertex stage bends a vertex along the wind direction by
//! `strength * v² * stiffness * gust`:
//!
//! - `v²`, because a blade is a cantilever clamped at the root. The root does
//!   not move, the middle moves a little and the tip moves most, and a square
//!   is the shape of that with no cost.
//! - `stiffness`, which is `1 - lean`: a strand already lying over has less
//!   upright length left to bend.
//! - `gust`, a scrolled wave over the world plus a per-strand sine phased by
//!   `phase`. The wave is what makes a field of grass move in one direction at
//!   once; the phase is what keeps the blades of a tuft from moving in step,
//!   which is the difference between grass and a flag.
//!
//! # The prepass gets the same displacement
//!
//! [`MaterialExtension::prepass_vertex_shader`] is overridden with the same
//! bend written against the prepass's own vertex layout, because a depth
//! buffer, a shadow map and a normal prepass drawn from the *undisplaced*
//! geometry are a blade whose shadow does not move with it. The two texts are
//! generated from one template, which is what keeps them from drifting.
//!
//! # What it deliberately does not do
//!
//! It does not rotate the normal. A bent blade's shading normal is a little
//! wrong, by about the angle it bent, and at the widths a strand is drawn at
//! that is below what anything can see; rotating it costs a cross product and
//! a normalise per vertex on the most vertex-heavy thing in the scene.
//!
//! It does not shrink the last level-of-detail band continuously with
//! distance, which [`LAST_BAND_LENGTH`](crate::strands::LAST_BAND_LENGTH)
//! still does in one step at the bake. A continuous shrink has to scale a
//! vertex *towards its own root*, and the root is not something a vertex
//! carries: `v` says how far along the strand the vertex is and nothing says
//! where the strand began. Carrying it would be a second three-float attribute
//! on every strand vertex, which is about 17 MB over one full-detail repeat of
//! the benchmark lawn. We did not take that.
//!
//! # Zero wind is the picture without this module
//!
//! [`StrandWind::still`] is strength zero, and a strength of zero multiplies
//! the whole displacement by zero before anything else is computed. A capture
//! meant to be reproducible sets it, which is what the reference gallery does.

use bevy::{
    asset::uuid::Uuid,
    mesh::{MeshVertexBufferLayoutRef, VertexAttributeDescriptor},
    pbr::{ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline},
    prelude::*,
    render::render_resource::{
        AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
    },
    shader::Shader,
};

use crate::strands::ATTRIBUTE_STRAND_WIND;

/// Where the wind attribute is bound in the generated vertex stages.
///
/// Eight, which is past every location Bevy's own `Vertex` structs use in
/// either the main pass or the prepass: the forward layout stops at seven with
/// the skinning weights and the prepass layout stops at seven with the colour.
/// It is the same number in both, which costs nothing and means one constant
/// rather than two that have to be kept apart.
const WIND_LOCATION: u32 = 8;

/// The high bits this module's shader ids share.
///
/// A random constant, chosen the way
/// [`shader`](crate::shader)'s is and for the same reason: an id derived here
/// must not collide with Bevy's own embedded shaders, with a compiled graph's,
/// or with another crate's.
const WIND_NAMESPACE: u128 = 0x0000_57a5_11d0_4e3b_9c61_0f28;

/// Which of the two vertex stages a shader is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindStage {
    /// The main pass, against `bevy_pbr::forward_io`.
    Vertex,
    /// The prepass, the deferred prepass and the shadow pass, against
    /// `bevy_pbr::prepass_io`.
    Prepass,
}

impl WindStage {
    /// The asset id this stage's shader is registered under.
    fn id(self) -> AssetId<Shader> {
        let stage = match self {
            Self::Vertex => 0_u128,
            Self::Prepass => 1,
        };
        AssetId::Uuid {
            uuid: Uuid::from_u128((WIND_NAMESPACE << 8) | stage),
        }
    }

    /// A weak handle to it. Nothing here owns the asset; the plugin's insert
    /// does, and a pipeline built from one keeps drawing either way.
    fn handle(self) -> Handle<Shader> {
        Handle::Uuid(
            match self.id() {
                AssetId::Uuid { uuid } => uuid,
                AssetId::Index { .. } => unreachable!("built from a uuid"),
            },
            std::marker::PhantomData,
        )
    }
}

/// How the wind blows, for every strand layer in the world.
///
/// One resource rather than one per material, because wind is weather: two
/// lawns in one scene that disagreed about which way it was blowing would be
/// the most obvious thing in the frame. A scene that wants a sheltered patch
/// wants a second [`StrandWindMaterial`] with its own extension, which this
/// leaves possible and does not do for you.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct StrandWind {
    /// Which way it blows, in world XZ. Normalised on the way into the
    /// uniform, so a caller may write a direction rather than a unit vector; a
    /// zero vector is no wind whatever the strength says.
    pub direction: Vec2,
    /// How far the tip of an upright, full-length strand is pushed, in metres.
    ///
    /// Metres rather than a fraction, because a blade and a bristle bend by
    /// very different amounts under the same air and the layer's own length is
    /// not what decides it.
    pub strength: f32,
    /// How many times a second a single strand completes its own sway.
    pub gust_hertz: f32,
    /// How long the travelling gust is, in metres from crest to crest.
    ///
    /// This is the part that reads as *wind* rather than as vibration: a wave
    /// running across the field at a scale a good deal larger than a blade.
    pub gust_metres: f32,
}

impl Default for StrandWind {
    /// A modest breeze: 12 mm of tip travel, a strand swaying about twice a
    /// second, under a gust two metres long. It is deliberately small. Grass
    /// that visibly waves is grass in a gale, and the first thing anyone does
    /// with a wind control is turn it up.
    fn default() -> Self {
        Self {
            direction: Vec2::new(1.0, 0.35),
            strength: 0.012,
            gust_hertz: 2.0,
            gust_metres: 2.0,
        }
    }
}

impl StrandWind {
    /// No wind at all, which renders exactly what a strand layer rendered
    /// before this module existed.
    ///
    /// What a capture meant to be compared against another capture sets: a
    /// gallery that let the grass move would answer a different image every
    /// run, however many frames it waited.
    pub fn still() -> Self {
        Self {
            strength: 0.0,
            ..Self::default()
        }
    }

    /// This wind as the two rows the shader reads.
    fn uniform(self) -> WindUniform {
        let direction = self.direction.normalize_or_zero();
        WindUniform {
            flow: Vec4::new(
                direction.x,
                direction.y,
                self.strength.max(0.0),
                self.gust_hertz.max(0.0),
            ),
            // The wavelength becomes its reciprocal here rather than in the
            // shader, so the per-vertex cost is a multiply and the divide by
            // zero is dealt with once, in Rust, where it can be read.
            gust: Vec4::new(
                if self.gust_metres.abs() > f32::EPSILON {
                    self.gust_metres.recip()
                } else {
                    0.0
                },
                0.0,
                0.0,
                0.0,
            ),
        }
    }
}

/// The uniform block the vertex stage reads.
///
/// Two `vec4` rows rather than six scalars, for the reason
/// `shader::GraphParams` is rows: a uniform block is laid
/// out in sixteen-byte rows whatever the members say, so writing the rows is
/// writing what is actually there.
#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct WindUniform {
    /// `direction.x`, `direction.y`, `strength` in metres, `gust_hertz`.
    pub flow: Vec4,
    /// `1 / gust_metres`, and three rows of nothing yet.
    pub gust: Vec4,
}

/// The wind half of a strand material, as a Bevy material extension.
///
/// One type for every layer of every graph: unlike
/// `shader::GraphExtension` there is nothing here that
/// varies per material, so there is no bind group data and no `specialize`
/// swap of the shader. What `specialize` does do is the one thing a custom
/// vertex attribute needs, which is to put
/// [`ATTRIBUTE_STRAND_WIND`] in the vertex buffer layout.
#[derive(Asset, AsBindGroup, Clone, Copy, Debug, Default, TypePath)]
pub struct StrandWindExtension {
    /// The weather, copied in by [`drive_wind`] every frame it changes.
    #[uniform(100)]
    pub wind: WindUniform,
}

/// A strand layer's material: a `StandardMaterial` with a vertex stage over it.
///
/// The constants, the transmission and the lighting are Bevy's, exactly as they
/// were when [`strand_material`](crate::strands::strand_material) answered a
/// bare `StandardMaterial`; what the extension adds is where the vertices are.
pub type StrandWindMaterial = ExtendedMaterial<StandardMaterial, StrandWindExtension>;

/// The shader-def the prepass pipeline pushes and the main pass does not.
///
/// The same discriminator [`shader`](crate::shader) uses, read off the *vertex*
/// stage's defs rather than the fragment's: a shadow pass has no fragment stage
/// at all, and a shadow of an undisplaced blade is the bug this module exists
/// to avoid.
const PREPASS_DEF: &str = "PREPASS_PIPELINE";

impl MaterialExtension for StrandWindExtension {
    fn vertex_shader() -> bevy::shader::ShaderRef {
        WindStage::Vertex.handle().into()
    }

    fn prepass_vertex_shader() -> bevy::shader::ShaderRef {
        WindStage::Prepass.handle().into()
    }

    fn deferred_vertex_shader() -> bevy::shader::ShaderRef {
        WindStage::Prepass.handle().into()
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let prepass = descriptor.vertex.shader_defs.iter().any(|def| {
            let name = match def {
                bevy::shader::ShaderDefVal::Bool(name, _)
                | bevy::shader::ShaderDefVal::Int(name, _)
                | bevy::shader::ShaderDefVal::UInt(name, _) => name,
            };
            name == PREPASS_DEF
        });
        // The two passes bind the same attributes at different locations —
        // `bevy_pbr::forward_io` and `bevy_pbr::prepass_io` disagree about
        // every one of them — so the layout is built per pass from the same
        // list the generated text declares.
        let mut wanted: Vec<VertexAttributeDescriptor> = if prepass {
            vec![
                Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
                Mesh::ATTRIBUTE_UV_0.at_shader_location(1),
                Mesh::ATTRIBUTE_NORMAL.at_shader_location(3),
                Mesh::ATTRIBUTE_COLOR.at_shader_location(7),
            ]
        } else {
            vec![
                Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
                Mesh::ATTRIBUTE_NORMAL.at_shader_location(1),
                Mesh::ATTRIBUTE_UV_0.at_shader_location(2),
                Mesh::ATTRIBUTE_COLOR.at_shader_location(5),
            ]
        };
        wanted.push(ATTRIBUTE_STRAND_WIND.at_shader_location(WIND_LOCATION));
        descriptor.vertex.buffers = vec![layout.0.get_layout(&wanted)?];
        Ok(())
    }
}

/// The plugin that registers the two shaders and drives the uniform.
///
/// Add it beside `MaterialPlugin::<StrandWindMaterial>::default()`, or take
/// [`StrandPlugin`], which adds both.
pub struct StrandWindPlugin;

impl Plugin for StrandWindPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StrandWind>()
            .add_plugins(MaterialPlugin::<StrandWindMaterial>::default())
            .add_systems(Update, drive_wind);
    }

    /// The shaders are inserted here rather than in [`Self::build`] because
    /// `Assets<Shader>` is another plugin's resource and the order plugins are
    /// added in is the caller's. `finish` runs after every `build`.
    fn finish(&self, app: &mut App) {
        let mut shaders = app.world_mut().resource_mut::<Assets<Shader>>();
        for stage in [WindStage::Vertex, WindStage::Prepass] {
            let label = match stage {
                WindStage::Vertex => "vertex",
                WindStage::Prepass => "prepass",
            };
            // An insert under a known id cannot fail for a reason a caller can
            // fix, and a plugin has nowhere to report one, so a failure here is
            // a panic naming the stage.
            shaders
                .insert(
                    stage.id(),
                    Shader::from_wgsl(wgsl(stage), format!("ashlar/strand-wind/{label}.wgsl")),
                )
                .unwrap_or_else(|error| {
                    panic!("registering the strand wind {label} shader: {error}")
                });
        }
    }
}

/// Everything a scene needs to draw strand layers that move.
///
/// The material plugin, the two shaders and the resource. A caller that wants
/// the geometry and not the movement adds this and sets
/// [`StrandWind::still`]; the pipeline is the same either way, which is what
/// makes a still capture and a moving frame the same picture.
pub struct StrandPlugin;

impl Plugin for StrandPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(StrandWindPlugin);
    }
}

/// Copy the weather into every strand material that is not already carrying it.
///
/// Every material rather than one: the uniform is per material and the wind is
/// per world, so this is the join. It writes only on a change, which is what
/// keeps a still scene from re-uploading a uniform block per material per
/// frame.
pub fn drive_wind(wind: Res<StrandWind>, mut materials: ResMut<Assets<StrandWindMaterial>>) {
    let uniform = wind.uniform();
    // `Assets::iter_mut` marks every asset modified, so the guard is here
    // rather than inside the loop.
    if !wind.is_changed() {
        return;
    }
    for (_, material) in materials.iter_mut() {
        material.extension.wind = uniform;
    }
}

/// The generated text of one stage.
///
/// Written as a Rust string rather than a `.wgsl` file on disk, which is what
/// this workspace does everywhere: `shader::register`
/// puts a compiled graph's fragment under a `Handle::Uuid` the same way. The
/// two stages share one `BEND` text so that the main pass and the prepass cannot
/// drift apart; what differs is only the vertex layout they are written
/// against.
#[must_use]
pub fn wgsl(stage: WindStage) -> String {
    match stage {
        WindStage::Vertex => format!("{VERTEX_IMPORTS}{UNIFORM}{BEND}{VERTEX_ENTRY}"),
        WindStage::Prepass => format!("{PREPASS_IMPORTS}{UNIFORM}{BEND}{PREPASS_ENTRY}"),
    }
}

/// The main pass's imports, and the clock where the main pass keeps it.
const VERTEX_IMPORTS: &str = "\
#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
    forward_io::VertexOutput,
    mesh_view_bindings::globals,
}

";

/// The prepass's imports.
///
/// The clock is declared by hand from the same struct, because the prepass's
/// view bind group is a smaller one of its own where the globals buffer sits at
/// binding 1 and no shipped module declares it. `wgsl::imports` in
/// `ashlar-material` says the same thing at more length; this is that fact
/// again, for the vertex stage.
const PREPASS_IMPORTS: &str = "\
#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
    prepass_io::VertexOutput,
}
#import bevy_render::globals::Globals

@group(0) @binding(1) var<uniform> ashlar_globals: Globals;

";

/// The uniform block, at the material bind group.
///
/// `#{MATERIAL_BIND_GROUP}` is a `naga_oil` directive Bevy substitutes, and it
/// is how a shader written by hand names the group an `AsBindGroup` derive put
/// it in without hard-coding the number.
const UNIFORM: &str = "\
struct AshlarWind {
    flow: vec4<f32>,
    gust: vec4<f32>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> ashlar_wind: AshlarWind;

";

/// The bend itself, shared by both stages.
///
/// `world` is the vertex in world space and `wind` is the mesh attribute
/// `[phase, v, stiffness]`. What comes back is the same point, moved.
const BEND: &str = "\
fn ashlar_bend(world: vec3<f32>, wind: vec3<f32>, seconds: f32) -> vec3<f32> {
    let direction = ashlar_wind.flow.xy;
    let strength = ashlar_wind.flow.z;
    // Zero strength is zero displacement before anything else is computed, so
    // a still scene draws exactly what this material drew without a wind stage
    // at all.
    if (strength <= 0.0) {
        return world;
    }
    let phase = wind.x;
    let along = wind.y;
    let stiffness = wind.z;
    let hertz = ashlar_wind.flow.w;
    let waves = ashlar_wind.gust.x;
    let tau = 6.2831855;
    // The travelling gust: one wave running across the field along the wind,
    // at a wavelength a good deal longer than a strand. This is the part that
    // reads as weather rather than as vibration.
    let travel = sin(tau * (0.25 * hertz * seconds - dot(world.xz, direction) * waves));
    // The strand's own sway, phased by its own hash so that neighbours do not
    // move in step. A lawn whose blades agree is a flag.
    let sway = sin(tau * (hertz * seconds + phase));
    // A cantilever clamped at the root: the root does not move, the tip moves
    // most, and the square is the shape of that for one multiply.
    let lever = along * along * stiffness;
    let amount = strength * lever * sway * (0.65 + 0.35 * travel);
    return vec3<f32>(
        world.x + direction.x * amount,
        world.y,
        world.z + direction.y * amount,
    );
}

";

/// The main pass entry point.
///
/// `bevy_pbr::mesh.wgsl`'s own vertex stage with two changes: the vertex struct
/// carries the wind attribute, and the world position is bent before it is
/// projected. Every `#ifdef` here is one of Bevy's, copied so that a strand
/// mesh that gains or loses an attribute still compiles.
const VERTEX_ENTRY: &str = "\
struct AshlarStrandVertex {
    @builtin(instance_index) instance_index: u32,
#ifdef VERTEX_POSITIONS
    @location(0) position: vec3<f32>,
#endif
#ifdef VERTEX_NORMALS
    @location(1) normal: vec3<f32>,
#endif
#ifdef VERTEX_UVS_A
    @location(2) uv: vec2<f32>,
#endif
#ifdef VERTEX_COLORS
    @location(5) color: vec4<f32>,
#endif
    @location(8) wind: vec3<f32>,
};

@vertex
fn vertex(vertex: AshlarStrandVertex) -> VertexOutput {
    var out: VertexOutput;
    let mesh_world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);

#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal,
        vertex.instance_index,
    );
#endif

#ifdef VERTEX_POSITIONS
    var world = mesh_functions::mesh_position_local_to_world(
        mesh_world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    world = vec4<f32>(ashlar_bend(world.xyz, vertex.wind, globals.time), world.w);
    out.world_position = world;
    out.position = position_world_to_clip(world.xyz);
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif

#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif

#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif

#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex.instance_index, mesh_world_from_local[3]);
#endif

    return out;
}
";

/// The prepass entry point.
///
/// `bevy_pbr::prepass.wgsl`'s vertex stage, cut down to what a strand mesh
/// has and bent by the same function. The locations are the prepass's own,
/// which are not the main pass's for a single attribute.
///
/// Motion vectors are written from the *bent* previous position as well, which
/// is to say from the same position: a strand's local geometry does not change
/// between frames, so the honest previous world position is the one the
/// previous frame's clock would have bent it to, and reconstructing that needs
/// a second clock nobody binds here. A lawn under a temporal filter therefore
/// reports no motion where it has some, which is a smear on a blade and not on
/// the scene.
const PREPASS_ENTRY: &str = "\
struct AshlarStrandVertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
#ifdef VERTEX_UVS_A
    @location(1) uv: vec2<f32>,
#endif
#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
#ifdef VERTEX_NORMALS
    @location(3) normal: vec3<f32>,
#endif
#endif
#ifdef VERTEX_COLORS
    @location(7) color: vec4<f32>,
#endif
    @location(8) wind: vec3<f32>,
};

@vertex
fn vertex(vertex: AshlarStrandVertex) -> VertexOutput {
    var out: VertexOutput;
    let mesh_world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);

    var world = mesh_functions::mesh_position_local_to_world(
        mesh_world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    world = vec4<f32>(ashlar_bend(world.xyz, vertex.wind, ashlar_globals.time), world.w);
    out.world_position = world;
    out.position = position_world_to_clip(world.xyz);

#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif

#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal,
        vertex.instance_index,
    );
#endif
#endif

#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif

#ifdef MOTION_VECTOR_PREPASS
    out.previous_world_position = world;
#endif

#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif

#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex.instance_index, mesh_world_from_local[3]);
#endif

    return out;
}
";

#[cfg(test)]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "the constants above, written out; a margin would pass a wind \
                  that had quietly stopped being still"
    )]
    use super::*;

    #[test]
    fn still_wind_is_zero_strength_and_bends_nothing() {
        let still = StrandWind::still().uniform();
        assert_eq!(still.flow.z, 0.0, "strength");
        // And the shader's first statement is the early return that makes that
        // an exact zero rather than a very small number.
        assert!(BEND.contains("if (strength <= 0.0) {\n        return world;"));
    }

    #[test]
    fn a_direction_is_normalised_and_a_zero_one_stays_zero() {
        let wind = StrandWind {
            direction: Vec2::new(3.0, 4.0),
            ..StrandWind::default()
        };
        let flow = wind.uniform().flow;
        assert!((flow.x - 0.6).abs() < 1e-6, "{flow:?}");
        assert!((flow.y - 0.8).abs() < 1e-6, "{flow:?}");
        let calm = StrandWind {
            direction: Vec2::ZERO,
            ..StrandWind::default()
        };
        assert_eq!(calm.uniform().flow.xy(), Vec2::ZERO);
    }

    #[test]
    fn a_gust_length_becomes_its_reciprocal_and_zero_does_not_divide() {
        let wind = StrandWind {
            gust_metres: 4.0,
            ..StrandWind::default()
        };
        assert!((wind.uniform().gust.x - 0.25).abs() < 1e-6);
        let flat = StrandWind {
            gust_metres: 0.0,
            ..StrandWind::default()
        };
        assert_eq!(flat.uniform().gust.x, 0.0, "a wavelength of nothing");
    }

    #[test]
    fn the_two_stages_share_one_bend_and_differ_only_in_their_layout() {
        let vertex = wgsl(WindStage::Vertex);
        let prepass = wgsl(WindStage::Prepass);
        assert!(vertex.contains(BEND) && prepass.contains(BEND), "one bend");
        // The clock is bound in two different places, and each stage names the
        // one its own bind group has.
        assert!(vertex.contains("globals.time") && !vertex.contains("ashlar_globals"));
        assert!(prepass.contains("ashlar_globals.time"));
        // The wind attribute is at the same location in both, and it is past
        // every location either of Bevy's own layouts uses.
        assert!(vertex.contains("@location(8) wind: vec3<f32>"));
        assert!(prepass.contains("@location(8) wind: vec3<f32>"));
        assert_eq!(WIND_LOCATION, 8);
        // And the two ids are two ids.
        assert_ne!(WindStage::Vertex.id(), WindStage::Prepass.id());
    }
}
