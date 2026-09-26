use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{
    GraphError, GraphWarning, MAX_PERIOD, Period, finite,
    library::{Bindings, InstanceKey, Resolver},
    name,
    nodes::{Accepts, GraphInput, InputPort, Node, Output, Ports, Resolved},
    period, require,
};

/// The type of a port. Three of them, with the two implicit conversions a
/// Material Maker author expects and no others.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ValueType {
    /// One channel, nominally `0..=1` but never clamped. Broadcasts to [`Self::Color`].
    Float,
    /// Linear RGB. Converts to [`Self::Float`] by Rec. 709 luminance.
    Color,
    /// A UV or an offset. Converts to nothing.
    Vec2,
}

impl ValueType {
    /// Whether a value of this type reaches `target` through an implicit conversion.
    pub fn converts_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (Self::Float | Self::Color, Self::Float | Self::Color)
        ) || self == target
    }

    /// The type of a pointwise result over two operands: colour wins over float.
    ///
    /// `None` when one side is a [`Self::Vec2`] and the other is not, which is
    /// the one combination that has no meaning.
    pub fn join(self, other: Self) -> Option<Self> {
        match (self, other) {
            (Self::Color, Self::Float | Self::Color) | (Self::Float, Self::Color) => {
                Some(Self::Color)
            }
            (Self::Float, Self::Float) => Some(Self::Float),
            (Self::Vec2, Self::Vec2) => Some(Self::Vec2),
            (Self::Vec2, _) | (_, Self::Vec2) => None,
        }
    }
}

impl std::fmt::Display for ValueType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Float => "Float",
            Self::Color => "Color",
            Self::Vec2 => "Vec2",
        })
    }
}

/// Rec. 709 luminance, the one direction colour takes to reach a float.
pub const LUMINANCE: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// A literal on a node input. Colours are linear here, never sRGB.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    /// One unitless channel.
    Float(f32),
    /// Linear RGB, allowed to exceed one where it feeds emission.
    Color([f32; 3]),
    /// A UV or an offset in UV space.
    Vec2([f32; 2]),
}

impl Value {
    /// The port type this literal carries.
    pub fn value_type(&self) -> ValueType {
        match self {
            Self::Float(_) => ValueType::Float,
            Self::Color(_) => ValueType::Color,
            Self::Vec2(_) => ValueType::Vec2,
        }
    }

    /// Whether every component is finite. A NaN literal poisons every texel it reaches.
    pub fn is_finite(&self) -> bool {
        match self {
            Self::Float(value) => value.is_finite(),
            Self::Color(value) => value.iter().all(|c| c.is_finite()),
            Self::Vec2(value) => value.iter().all(|c| c.is_finite()),
        }
    }

    /// This literal as one channel, taking luminance where it is a colour.
    ///
    /// A [`Self::Vec2`] has no float form and answers `None`, which is the
    /// runtime half of the rule validation enforces statically.
    pub fn as_float(&self) -> Option<f32> {
        match self {
            Self::Float(value) => Some(*value),
            Self::Color(rgb) => Some(luminance(*rgb)),
            Self::Vec2(_) => None,
        }
    }

    /// This literal as linear RGB, broadcasting where it is one channel.
    pub fn as_color(&self) -> Option<[f32; 3]> {
        match self {
            Self::Float(value) => Some([*value; 3]),
            Self::Color(rgb) => Some(*rgb),
            Self::Vec2(_) => None,
        }
    }
}

/// Rec. 709 luminance of a linear colour.
pub fn luminance(color: [f32; 3]) -> f32 {
    color[0] * LUMINANCE[0] + color[1] * LUMINANCE[1] + color[2] * LUMINANCE[2]
}

pub use ashlar_surface::ParamValue;

/// What a graph makes of a [`ParamValue`].
///
/// The value itself is `ashlar-surface`'s, because a material definition and a
/// slot binding write one too. What it *means* inside a graph is here: `Int`
/// and `Bool` are authoring conveniences, and both reach the graph as a
/// [`ValueType::Float`], so a `Bool` parameter drives a
/// [`crate::nodes::Switch`] and an `Int` drives a `Math` operand.
pub trait GraphParam {
    /// The port type this parameter presents to the graph.
    fn value_type(&self) -> ValueType;

    /// This parameter as a graph literal, with integers and switches widened.
    fn value(&self) -> Value;
}

impl GraphParam for ParamValue {
    fn value_type(&self) -> ValueType {
        match self {
            Self::Color(_) => ValueType::Color,
            Self::Float(_) | Self::Int(_) | Self::Bool(_) => ValueType::Float,
        }
    }

    fn value(&self) -> Value {
        match self {
            Self::Float(value) => Value::Float(*value),
            Self::Color(rgb) => Value::Color(*rgb),
            #[expect(
                clippy::cast_precision_loss,
                reason = "an authored count, not a bit pattern; f32 is what the IR runs on"
            )]
            Self::Int(value) => Value::Float(*value as f32),
            Self::Bool(value) => Value::Float(if *value { 1.0 } else { 0.0 }),
        }
    }
}

/// When a parameter is resolved: at compile time, or per frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Exposure {
    /// Folded into constants when the graph is lowered. The default, and free.
    #[default]
    Bake,
    /// Kept as a uniform, so it can move without a rebake. Costs a shader.
    Live,
}

/// An exposed parameter: a value an author or a game sets without editing nodes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct Param {
    /// Unique name within the graph; what [`Input::Param`] refers to.
    pub name: String,
    /// The default, and the type this parameter presents to the graph.
    pub value: ParamValue,
    /// Inclusive slider bounds for a numeric parameter, for tooling.
    #[serde(default)]
    pub range: Option<(f32, f32)>,
    /// Whether a backend may fold this away.
    #[serde(default)]
    pub exposure: Exposure,
}

impl Param {
    /// A unitless float parameter.
    pub fn float(name: impl Into<String>, value: f32) -> Self {
        Self::of(name, ParamValue::Float(value))
    }

    /// A linear RGB parameter.
    pub fn color(name: impl Into<String>, value: [f32; 3]) -> Self {
        Self::of(name, ParamValue::Color(value))
    }

    /// An integer parameter: a count, a seed or a period.
    pub fn int(name: impl Into<String>, value: i32) -> Self {
        Self::of(name, ParamValue::Int(value))
    }

    /// A switch parameter, reaching the graph as zero or one.
    pub fn bool(name: impl Into<String>, value: bool) -> Self {
        Self::of(name, ParamValue::Bool(value))
    }

    fn of(name: impl Into<String>, value: ParamValue) -> Self {
        Self {
            name: name.into(),
            value,
            range: None,
            exposure: Exposure::default(),
        }
    }

    /// Bound a numeric parameter. Colours and switches have no range.
    pub fn range(mut self, low: f32, high: f32) -> Self {
        self.range = Some((low, high));
        self
    }

    /// Keep this parameter live: a uniform in a shader, never folded.
    pub fn live(mut self) -> Self {
        self.exposure = Exposure::Live;
        self
    }

    fn check(&self, path: &str) -> Result<(), GraphError> {
        name(&self.name, &format!("{path}.name"))?;
        require(
            self.value.is_finite(),
            &format!("{path}.value"),
            "parameter value must be finite",
        )?;
        let Some((low, high)) = self.range else {
            return Ok(());
        };
        let path = format!("{path}.range");
        require(
            matches!(self.value, ParamValue::Float(_) | ParamValue::Int(_)),
            &path,
            "only a float or integer parameter has a range",
        )?;
        require(
            low.is_finite() && high.is_finite() && low <= high,
            &path,
            "range must be finite and ordered",
        )?;
        let value = match self.value.value() {
            Value::Float(value) => value,
            Value::Color(_) | Value::Vec2(_) => return Ok(()),
        };
        require(
            (low..=high).contains(&value),
            &path,
            "default value lies outside the range",
        )
    }
}

/// A node input: a literal, a parameter, another node's output, or a node
/// written where the input is.
///
/// An inline node is an authoring convenience with no runtime form:
/// [`MaterialGraph::build`] hoists it into the graph under the id
/// `parent.port`, so a validated [`Material`] never holds one and a backend
/// never sees one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Input {
    /// A literal value.
    Const(Value),
    /// The name of an exposed parameter.
    Param(String),
    /// The id of another node in the same graph.
    Node(String),
    /// A node written where its output was wanted; hoisted away at build.
    Inline(Box<Node>),
}

impl Default for Input {
    fn default() -> Self {
        Self::Const(Value::Float(0.0))
    }
}

impl Input {
    /// Read an exposed parameter by name.
    pub fn param(name: impl Into<String>) -> Self {
        Self::Param(name.into())
    }

    /// Read another node's output by id.
    pub fn node(id: impl Into<String>) -> Self {
        Self::Node(id.into())
    }

    /// A one-channel literal.
    pub fn float(value: f32) -> Self {
        Self::Const(Value::Float(value))
    }

    /// A linear RGB literal.
    pub fn color(value: [f32; 3]) -> Self {
        Self::Const(Value::Color(value))
    }

    /// A UV-space literal.
    pub fn vec2(value: [f32; 2]) -> Self {
        Self::Const(Value::Vec2(value))
    }
}

/// A node id is the ordinary way to name an input, so a bare string is one.
impl From<&str> for Input {
    fn from(id: &str) -> Self {
        Self::Node(id.to_owned())
    }
}

impl From<String> for Input {
    fn from(id: String) -> Self {
        Self::Node(id)
    }
}

impl From<&String> for Input {
    fn from(id: &String) -> Self {
        Self::Node(id.clone())
    }
}

impl From<f32> for Input {
    fn from(value: f32) -> Self {
        Self::float(value)
    }
}

impl From<[f32; 3]> for Input {
    fn from(value: [f32; 3]) -> Self {
        Self::color(value)
    }
}

impl From<[f32; 2]> for Input {
    fn from(value: [f32; 2]) -> Self {
        Self::vec2(value)
    }
}

impl From<Value> for Input {
    fn from(value: Value) -> Self {
        Self::Const(value)
    }
}

impl From<Node> for Input {
    fn from(node: Node) -> Self {
        Self::Inline(Box::new(node))
    }
}

/// What a material presents to a renderer. Normals come from height, never
/// authored directly, so relief and parallax cannot disagree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct PbrOutput {
    /// Linear base colour.
    pub base_color: Input,
    /// Perceptual roughness.
    pub roughness: Input,
    /// Metalness.
    pub metallic: Input,
    /// Ambient occlusion; one means unoccluded.
    pub occlusion: Input,
    /// Height in `0..=1`, the field the normal map is derived from.
    pub height: Option<Input>,
    /// Metres of relief per unit height, per repeat. Zero is a flat normal.
    pub normal_strength: f32,
    /// Linear emitted RGB, allowed to exceed one.
    pub emissive: Option<Input>,
    /// Named float masks this graph exports beside its PBR channels.
    ///
    /// A bake writes no map for one and a shader binds nothing for one: they
    /// are roots only inside an inlined instance, where whatever instanced this
    /// graph reads one through
    /// [`SurfaceOutput::Extra`](crate::nodes::SurfaceOutput::Extra). What they
    /// are for is a compound that decided something — where the wear took,
    /// where the water sat — whose caller wants to layer on that decision
    /// rather than build it a second time and get an answer a texel off.
    ///
    /// They are held to the tiling rule every other bound output is held to and
    /// they count towards the material's repeat, because a mask that does not
    /// meet itself at the seam is as wrong as a colour that does not.
    pub extra: BTreeMap<String, Input>,
}

impl Default for PbrOutput {
    fn default() -> Self {
        Self {
            base_color: Input::color([0.5; 3]),
            roughness: Input::float(0.8),
            metallic: Input::float(0.0),
            occlusion: Input::float(1.0),
            height: None,
            normal_strength: 0.0,
            emissive: None,
            extra: BTreeMap::new(),
        }
    }
}

impl PbrOutput {
    /// A mid-grey dielectric, which every setter below replaces a piece of.
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind linear base colour.
    pub fn base_color(mut self, input: impl Into<Input>) -> Self {
        self.base_color = input.into();
        self
    }

    /// Bind perceptual roughness.
    pub fn roughness(mut self, input: impl Into<Input>) -> Self {
        self.roughness = input.into();
        self
    }

    /// Bind metalness.
    pub fn metallic(mut self, input: impl Into<Input>) -> Self {
        self.metallic = input.into();
        self
    }

    /// Bind ambient occlusion.
    pub fn occlusion(mut self, input: impl Into<Input>) -> Self {
        self.occlusion = input.into();
        self
    }

    /// Bind the height field the normal map is derived from.
    pub fn height(mut self, input: impl Into<Input>) -> Self {
        self.height = Some(input.into());
        self
    }

    /// Set the relief in metres per unit height, over one repeat.
    pub fn normal_strength(mut self, metres: f32) -> Self {
        self.normal_strength = metres;
        self
    }

    /// Bind linear emission.
    pub fn emissive(mut self, input: impl Into<Input>) -> Self {
        self.emissive = Some(input.into());
        self
    }

    /// Export a named float mask beside the PBR channels.
    ///
    /// The name is what a [`Subgraph`](crate::nodes::Subgraph) reads through
    /// [`SurfaceOutput::Extra`](crate::nodes::SurfaceOutput::Extra), and it may
    /// not be one of the six PBR port names, which mean something here already.
    pub fn extra(mut self, name: impl Into<String>, input: impl Into<Input>) -> Self {
        self.extra.insert(name.into(), input.into());
        self
    }

    /// The names the PBR channels take, which an extra may not take.
    const PORTS: [&'static str; 6] = [
        "base_color",
        "roughness",
        "metallic",
        "occlusion",
        "height",
        "emissive",
    ];

    /// What can be said about the output before anything is inferred.
    ///
    /// Only the extras' names, and only because they share one namespace with
    /// the PBR channels: an extra called `roughness` would be two ports of one
    /// name in [`Self::ports`], in the material's output map and in the lowered
    /// roots, and whichever landed last would win. It runs before the graph is
    /// flattened so that the message is about the name the author wrote rather
    /// than about the id an inline node written under it generated.
    pub(crate) fn check(&self) -> Result<(), GraphError> {
        for name in self.extra.keys() {
            let path = format!("output.extra[{name}]");
            crate::name(name, &path)?;
            require(
                !Self::PORTS.contains(&name.as_str()),
                &path,
                &format!(
                    "{name:?} is a PBR channel of this output; an extra needs a name of its own"
                ),
            )?;
        }
        Ok(())
    }

    /// Every bound input, with the port name and the type that port needs.
    pub(crate) fn ports(&self) -> Vec<(Cow<'static, str>, Accepts, &Input)> {
        let mut ports = vec![
            (
                Cow::Borrowed("base_color"),
                Accepts::Color,
                &self.base_color,
            ),
            (Cow::Borrowed("roughness"), Accepts::Float, &self.roughness),
            (Cow::Borrowed("metallic"), Accepts::Float, &self.metallic),
            (Cow::Borrowed("occlusion"), Accepts::Float, &self.occlusion),
        ];
        if let Some(height) = &self.height {
            ports.push((Cow::Borrowed("height"), Accepts::Float, height));
        }
        if let Some(emissive) = &self.emissive {
            ports.push((Cow::Borrowed("emissive"), Accepts::Color, emissive));
        }
        // An extra is a port like any other here, which is what holds it to the
        // same type, the same tiling rule and the same multiple. What it is not
        // is a channel a bake writes, and that difference is made at lowering.
        for (name, input) in &self.extra {
            ports.push((Cow::Owned(name.clone()), Accepts::Float, input));
        }
        ports
    }

    fn ports_mut(&mut self) -> Vec<(Cow<'static, str>, &mut Input)> {
        let mut ports = vec![
            (Cow::Borrowed("base_color"), &mut self.base_color),
            (Cow::Borrowed("roughness"), &mut self.roughness),
            (Cow::Borrowed("metallic"), &mut self.metallic),
            (Cow::Borrowed("occlusion"), &mut self.occlusion),
        ];
        if let Some(height) = &mut self.height {
            ports.push((Cow::Borrowed("height"), height));
        }
        if let Some(emissive) = &mut self.emissive {
            ports.push((Cow::Borrowed("emissive"), emissive));
        }
        for (name, input) in &mut self.extra {
            ports.push((Cow::Owned(name.clone()), input));
        }
        ports
    }
}

// `StrandProfile` and `MAX_SEGMENTS` used to be declared here, beside the
// layer that carries them. They live in `ashlar-strands` now and are
// re-exported from the crate root, because the mesh builder that reads them is
// the half a game links and this half is the graph engine. Nothing a caller
// writes changes: `ashlar_material::StrandProfile` is still the path, and it is
// still the type a `StrandLayer` serialises.
pub use ashlar_strands::{MAX_SEGMENTS, StrandProfile};

/// The most strands one cell of the lattice may stand.
///
/// A cell's strands are the cost the period does not see: `count` is bounded by
/// [`MAX_PERIOD`] because a repeat has to carry it, and this is bounded here
/// instead, because nothing downstream would refuse a layer that asked for a
/// thousand blades per cell until the mesh builder ran out of memory. Eight
/// over a 256-cell lattice is half a million strands over one repeat, which is
/// already past what the plan budgets for a lawn.
pub const MAX_PER_CELL: u32 = 8;

/// A strand layer: geometry scattered from the same graph fields the surface
/// is made of.
///
/// A [`PbrOutput`] is one field over a continuous surface, and there are
/// surfaces that are not one — grass, fur, moss fibre, carpet, thatch — because
/// what they are made of stands *off* the surface and has a silhouette. A
/// strand layer is the second kind of output a graph carries: a deterministic
/// list of strands over one repeat, each rooted on a jittered lattice of
/// [`Self::count`] cells that wraps at the seam, each carrying the fields below
/// as read **once at its own root** rather than once per texel.
///
/// That is the whole difference from the PBR half, and it is what makes the two
/// halves agree: a `"clumps"` mask and an `Input::param("lushness")` mean the
/// same thing to a blade as they do to the colour under it, because they are
/// the same nodes. [`crate::strands::scatter`] is what turns a layer into the
/// [`StrandSet`](crate::strands::StrandSet) a mesh builder reads.
///
/// The fields are held to the tiling rule every bound output is held to, and
/// they count towards the material's repeat: a strand set that does not meet
/// itself at the seam is as wrong as a colour that does not. They are also
/// *static* — a field that reaches [`Time`](crate::nodes::Time),
/// [`WorldPos`](crate::nodes::WorldPos),
/// [`WorldNormal`](crate::nodes::WorldNormal),
/// [`CutFlag`](crate::nodes::CutFlag), a
/// [`Triplanar`](crate::nodes::Triplanar) or a
/// [`WorldMask`](crate::nodes::WorldMask) is refused by path when the set is
/// scattered, because a strand is placed before there is a mesh or a frame to
/// ask. Wind belongs to the renderer.
///
/// The `_variation` amounts are the other axis of randomness, and they are not
/// the same as wiring a noise: a noise varies a field *across the surface*, so
/// neighbouring strands agree, and a variation varies it *per strand*, so they
/// do not. A lawn wants both.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct StrandLayer {
    /// Where strands stand, as a keep threshold in `0..=1` against each
    /// strand's own hash.
    ///
    /// A threshold rather than a multiply, for the reason
    /// [`Tile::mask`](crate::nodes::Tile::mask) gives at length: strands that
    /// thin out read as a sparse patch, and strands that shrink towards nothing
    /// read as a mistake.
    pub density: Input,
    /// Multiplies [`Self::length_metres`].
    pub length: Input,
    /// Multiplies [`Self::width_metres`].
    pub width: Input,
    /// Which way the strand leans, in the UV plane. A zero vector is upright.
    pub direction: Input,
    /// How far the root tilts away from the surface normal, in `0..=1` of a
    /// quarter turn.
    pub lean: Input,
    /// How much the strand droops between root and tip: the curvature of it.
    pub bend: Input,
    /// Linear colour at the root, which the geometry carries as vertex colour.
    pub root_color: Input,
    /// Linear colour at the tip.
    pub tip_color: Input,
    /// Perceptual roughness, per strand.
    pub roughness: Input,
    /// A displacement added to the root after the fields are read, for pulling
    /// the strands of a cell into one tuft.
    ///
    /// [`VoronoiOutput::Offset`](crate::nodes::VoronoiOutput::Offset) is what
    /// this port was added for: it answers the vector from a texel to its cell
    /// point, so wiring it here gathers a cell's roots onto that point.
    pub clump: Input,
    /// Cells across the repeat, in u and v. One strand per cell before
    /// [`Self::density`] takes any away.
    pub count: [u32; 2],
    /// How far a root may stray from its cell centre, in `0..=1` of a cell.
    pub jitter: f32,
    /// Varies every per-strand hash without varying the lattice.
    pub seed: u32,
    /// How long a strand of unit [`Self::length`] is, in metres.
    pub length_metres: f32,
    /// How wide a strand of unit [`Self::width`] is at the root, in metres.
    pub width_metres: f32,
    /// How much of a strand's length its own hash may take away, in `0..=1`.
    pub length_variation: f32,
    /// How much of a strand's width its own hash may take away, in `0..=1`.
    pub width_variation: f32,
    /// How far a strand's own hash may turn its direction, in `0..=1` of a
    /// half turn either way.
    pub direction_variation: f32,
    /// How much of a strand's lean its own hash may take away, in `0..=1`.
    pub lean_variation: f32,
    /// How much of a strand's bend its own hash may take away, in `0..=1`.
    ///
    /// The last of the five variations, and the one the Ghost of Tsushima
    /// accounts put squarely under the clump's control: "each blade checks
    /// which clump point it is closest to. That clump controls the blade's
    /// height. Direction. Color. Bend." Like the other four it is mixed with
    /// the clump's own hash by [`Self::clump_share`], so a tuft curls together
    /// and the tuft beside it curls differently.
    pub bend_variation: f32,
    /// How many segments the curve is built from, in `1..=`[`MAX_SEGMENTS`].
    pub segments: u32,
    /// How much of the root width the tip gives up, in `0..=1`. One is a point.
    pub taper: f32,
    /// How much the vertex colour is darkened at the root, in `0..=1`.
    ///
    /// It may look like a cheat beside a shadow map, and it is one: a blade is
    /// thinner than a shadow texel, so nothing else is going to put the dark at
    /// the bottom of a lawn, and that gradient is most of what reads as depth.
    pub root_occlusion: f32,
    /// Which shape a strand is built as.
    pub profile: StrandProfile,
    /// How many strands stand in each cell of the lattice.
    ///
    /// Density without touching the period. `count` is a lattice the repeat has
    /// to carry, so raising it raises the material's own multiple and is
    /// bounded by [`MAX_PERIOD`]; this is not, because every strand of a cell
    /// shares that cell's coordinate and differs only in the salt its hashes
    /// are taken under. Material Maker's tiler calls the same parameter
    /// *Overlap*, and Substance never raises its instance count either — a
    /// grass material there is a few thousand instances of a pattern that is
    /// itself a handful of blades.
    pub per_cell: u32,
    /// Where along the strand it is widest, in `0..=1` from root to tip.
    ///
    /// Zero is a blade that is widest at the root and tapers to its tip, which
    /// is what [`Self::taper`] alone describes and what every layer authored
    /// before this existed gets. Above zero the blade narrows towards the root
    /// as well, at the same rate, so the widest part is somewhere up the leaf —
    /// which is what gives the *bright* end of a colour ramp any area at all. A
    /// fully tapered blade at midpoint zero is a triangle whose tip is one texel
    /// wide, and a splat of it is weighted almost entirely towards the dark
    /// root.
    pub midpoint: f32,
    /// How far a strand's own hash may turn it about its own curve, in `0..=1`
    /// of a half turn either way.
    ///
    /// The width axis is otherwise perpendicular to the lean plane — one cross
    /// product for the whole strand, and no twist along it — so a patch of
    /// blades that lean alike is also *presented* alike, with no edge-on
    /// members among the flat-on ones. This is the control Unreal's landscape
    /// grass calls Random Rotation and gives the same reason for: so that the
    /// same side is not seen all the time.
    pub facing_variation: f32,
    /// How far a strand's own hash may sink its root below the surface, in
    /// metres.
    ///
    /// Every root otherwise sits exactly on the surface, so the only variation
    /// in how tall a blade stands is [`Self::length_variation`], which shortens
    /// the whole blade rather than burying its base. Substance's shape splatter
    /// carries the same pair under Height Offset and Height Offset Random, and
    /// it is how a pile gets short, half-buried blades without shortening the
    /// distribution.
    pub height_offset_metres: f32,
    /// The lattice clumps are laid on, in u and v. `[0, 0]` is no clumping.
    ///
    /// A clump is a cell of *this* lattice, coarser than [`Self::count`]: every
    /// strand belongs to the clump cell its root falls in, and the strands of
    /// one clump share an identity the three fields below read. A tuft in the
    /// reference is several blades that grew together, and what makes it read
    /// as one is that they agree — about how long they are and which way they
    /// lie — while their roots stay apart.
    pub clump_count: [u32; 2],
    /// How much of each per-strand variation is taken from the strand's clump
    /// rather than from the strand itself, in `0..=1`.
    ///
    /// Blender splits its child roughness the same way and says why: one kind is
    /// "based on children location so it varies the paths in a similar way" and
    /// another is "based on a random vector so it is not the same for nearby
    /// children". At one every blade of a clump is the same length and lies the
    /// same way; at zero they are independent and the clump is only a position.
    pub clump_share: f32,
    /// How far a strand's tip is pulled towards its clump's centre, in `0..=1`.
    ///
    /// The tip and not the root, which is the whole difference between a clump
    /// and a huddle: Blender's Clump ranges from tips meeting at one to roots
    /// meeting at minus one, and a tuft of grass is the first of those. Roots
    /// stay where the lattice put them.
    pub clump_tips: f32,
    /// How far a per-clump hash may darken a strand's colours, in `0..=1`.
    ///
    /// Substance drives per-instance luminance from the instance's own index
    /// for the same reason: a field varies a lawn over metres and a per-strand
    /// hash varies it per blade, and neither of them makes one *tuft* read
    /// darker than the tuft beside it.
    pub clump_tint: f32,
}

impl Default for StrandLayer {
    fn default() -> Self {
        Self {
            density: Input::float(1.0),
            length: Input::float(1.0),
            width: Input::float(1.0),
            direction: Input::vec2([0.0, 0.0]),
            lean: Input::float(0.0),
            bend: Input::float(0.0),
            root_color: Input::color([0.5; 3]),
            tip_color: Input::color([0.5; 3]),
            roughness: Input::float(0.8),
            clump: Input::vec2([0.0, 0.0]),
            count: [64, 64],
            jitter: 1.0,
            seed: 0,
            length_metres: 0.1,
            width_metres: 0.004,
            length_variation: 0.0,
            width_variation: 0.0,
            direction_variation: 0.0,
            lean_variation: 0.0,
            bend_variation: 0.0,
            segments: 3,
            taper: 1.0,
            root_occlusion: 0.0,
            profile: StrandProfile::Blade,
            // Every one of these is the behaviour a layer had before it
            // existed, so an old RON file reads unchanged and scatters the set
            // it always scattered.
            per_cell: 1,
            midpoint: 0.0,
            facing_variation: 0.0,
            height_offset_metres: 0.0,
            clump_count: [0, 0],
            clump_share: 0.0,
            clump_tips: 0.0,
            clump_tint: 0.0,
        }
    }
}

impl StrandLayer {
    /// A layer of 64 by 64 upright blades, which every setter below replaces a
    /// piece of.
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind where strands stand.
    pub fn density(mut self, input: impl Into<Input>) -> Self {
        self.density = input.into();
        self
    }

    /// Bind the length multiplier.
    pub fn length(mut self, input: impl Into<Input>) -> Self {
        self.length = input.into();
        self
    }

    /// Bind the width multiplier.
    pub fn width(mut self, input: impl Into<Input>) -> Self {
        self.width = input.into();
        self
    }

    /// Bind the lean direction in the UV plane.
    pub fn direction(mut self, input: impl Into<Input>) -> Self {
        self.direction = input.into();
        self
    }

    /// Bind the root tilt.
    pub fn lean(mut self, input: impl Into<Input>) -> Self {
        self.lean = input.into();
        self
    }

    /// Bind the droop from root to tip.
    pub fn bend(mut self, input: impl Into<Input>) -> Self {
        self.bend = input.into();
        self
    }

    /// Bind the two ends of the vertex-colour gradient.
    pub fn colors(mut self, root: impl Into<Input>, tip: impl Into<Input>) -> Self {
        self.root_color = root.into();
        self.tip_color = tip.into();
        self
    }

    /// Bind the per-strand roughness.
    pub fn roughness(mut self, input: impl Into<Input>) -> Self {
        self.roughness = input.into();
        self
    }

    /// Bind the displacement that gathers a cell's roots into a tuft.
    pub fn clump(mut self, input: impl Into<Input>) -> Self {
        self.clump = input.into();
        self
    }

    /// Set the same cell count in both axes.
    pub fn count(mut self, count: u32) -> Self {
        self.count = [count; 2];
        self
    }

    /// Set the cell count per axis.
    pub fn counts(mut self, u: u32, v: u32) -> Self {
        self.count = [u, v];
        self
    }

    /// Set how far a root may stray from its cell centre.
    pub fn jitter(mut self, jitter: f32) -> Self {
        self.jitter = jitter;
        self
    }

    /// Vary how far each strand curls, against its neighbours and its clump.
    pub fn bend_variation(mut self, amount: f32) -> Self {
        self.bend_variation = amount;
        self
    }

    /// Vary the strands.
    pub fn seed(mut self, seed: u32) -> Self {
        self.seed = seed;
        self
    }

    /// Set the length and width of a strand of unit length and width.
    pub fn metres(mut self, length: f32, width: f32) -> Self {
        self.length_metres = length;
        self.width_metres = width;
        self
    }

    /// Vary strand length per strand.
    pub fn length_variation(mut self, variation: f32) -> Self {
        self.length_variation = variation;
        self
    }

    /// Vary strand width per strand.
    pub fn width_variation(mut self, variation: f32) -> Self {
        self.width_variation = variation;
        self
    }

    /// Vary strand direction per strand.
    pub fn direction_variation(mut self, variation: f32) -> Self {
        self.direction_variation = variation;
        self
    }

    /// Vary strand lean per strand.
    pub fn lean_variation(mut self, variation: f32) -> Self {
        self.lean_variation = variation;
        self
    }

    /// Set how many segments the curve is built from.
    pub fn segments(mut self, segments: u32) -> Self {
        self.segments = segments;
        self
    }

    /// Set how much of the root width the tip gives up.
    pub fn taper(mut self, taper: f32) -> Self {
        self.taper = taper;
        self
    }

    /// Set how much the vertex colour darkens at the root.
    pub fn root_occlusion(mut self, occlusion: f32) -> Self {
        self.root_occlusion = occlusion;
        self
    }

    /// Choose which shape a strand is built as.
    pub fn profile(mut self, profile: StrandProfile) -> Self {
        self.profile = profile;
        self
    }

    /// Stand several strands in every cell, without touching the lattice the
    /// repeat has to carry.
    pub fn per_cell(mut self, strands: u32) -> Self {
        self.per_cell = strands;
        self
    }

    /// Move the widest part of the strand up towards its tip.
    pub fn midpoint(mut self, midpoint: f32) -> Self {
        self.midpoint = midpoint;
        self
    }

    /// Turn each strand about its own curve by its own hash.
    pub fn facing_variation(mut self, amount: f32) -> Self {
        self.facing_variation = amount;
        self
    }

    /// Sink each root below the surface by up to this many metres.
    pub fn height_offset(mut self, metres: f32) -> Self {
        self.height_offset_metres = metres;
        self
    }

    /// Gather the strands into clumps on a lattice of this many cells, and say
    /// how much they share, how far their tips converge and how much a clump's
    /// own hash tints it.
    pub fn clumps(mut self, cells: u32, share: f32, tips: f32) -> Self {
        self.clump_count = [cells, cells];
        self.clump_share = share;
        self.clump_tips = tips;
        self
    }

    /// Vary the colour of a whole clump against its neighbours.
    pub fn clump_tint(mut self, amount: f32) -> Self {
        self.clump_tint = amount;
        self
    }

    /// The lattice a strand set is laid on, as a period.
    ///
    /// A layer's cells wrap at the repeat, so the set meets itself at the seam
    /// the way a [`Tile`](crate::nodes::Tile)'s instances do. The count joins
    /// the material's own multiple all the same, because it is a lattice laid
    /// over the repeat by an output of the graph, and a repeat that does not
    /// carry it is a repeat the strands do not land on the same way twice.
    pub(crate) fn period(&self) -> Period {
        let lattice = Period::Tiled {
            u: self.count[0],
            v: self.count[1],
        };
        // A clump lattice wraps at the repeat exactly as the root lattice does,
        // and is laid over the repeat by the same output, so the repeat has to
        // carry it too or the tufts do not meet themselves at the seam.
        if self.clump_count[0] == 0 || self.clump_count[1] == 0 {
            return lattice;
        }
        lattice.lcm(Period::Tiled {
            u: self.clump_count[0],
            v: self.clump_count[1],
        })
    }

    /// Whether this layer gathers its strands into clumps at all.
    pub(crate) fn clumped(&self) -> bool {
        self.clump_count[0] > 0 && self.clump_count[1] > 0
    }

    /// The constants a layer carries, checked before anything is inferred.
    pub(crate) fn check(&self, path: &str) -> Result<(), GraphError> {
        for (axis, count) in ["u", "v"].into_iter().zip(self.count) {
            require(
                (1..=MAX_PERIOD).contains(&count),
                &format!("{path}.count"),
                &format!("count in {axis} must be in 1..={MAX_PERIOD}"),
            )?;
        }
        for (axis, count) in ["u", "v"].into_iter().zip(self.clump_count) {
            require(
                count <= MAX_PERIOD,
                &format!("{path}.clump_count"),
                &format!("clump count in {axis} must be at most {MAX_PERIOD}"),
            )?;
        }
        require(
            (1..=MAX_PER_CELL).contains(&self.per_cell),
            &format!("{path}.per_cell"),
            &format!("per_cell must be in 1..={MAX_PER_CELL}"),
        )?;
        {
            let path = format!("{path}.height_offset_metres");
            finite(self.height_offset_metres, &path, "height_offset_metres")?;
            require(
                self.height_offset_metres >= 0.0,
                &path,
                "height_offset_metres must not be negative",
            )?;
        }
        for (field, value) in [
            ("jitter", self.jitter),
            ("length_variation", self.length_variation),
            ("width_variation", self.width_variation),
            ("direction_variation", self.direction_variation),
            ("lean_variation", self.lean_variation),
            ("bend_variation", self.bend_variation),
            ("taper", self.taper),
            ("root_occlusion", self.root_occlusion),
            ("midpoint", self.midpoint),
            ("facing_variation", self.facing_variation),
            ("clump_share", self.clump_share),
            ("clump_tips", self.clump_tips),
            ("clump_tint", self.clump_tint),
        ] {
            let path = format!("{path}.{field}");
            finite(value, &path, field)?;
            require(
                (0.0..=1.0).contains(&value),
                &path,
                &format!("{field} must be in 0..=1"),
            )?;
        }
        for (field, value) in [
            ("length_metres", self.length_metres),
            ("width_metres", self.width_metres),
        ] {
            let path = format!("{path}.{field}");
            finite(value, &path, field)?;
            require(value > 0.0, &path, &format!("{field} must be above zero"))?;
        }
        require(
            (1..=MAX_SEGMENTS).contains(&self.segments),
            &format!("{path}.segments"),
            &format!("segments must be in 1..={MAX_SEGMENTS}"),
        )
    }

    /// Every bound field, with the port name and the type that port needs.
    ///
    /// The same shape [`PbrOutput::ports`] answers, and read by the same three
    /// passes: validation resolves them, flattening hoists what was written
    /// inline in one, and [`crate::strands`] lowers them as its roots.
    pub(crate) fn ports(&self) -> Vec<(&'static str, Accepts, &Input)> {
        vec![
            ("density", Accepts::Float, &self.density),
            ("length", Accepts::Float, &self.length),
            ("width", Accepts::Float, &self.width),
            ("direction", Accepts::Vec2, &self.direction),
            ("lean", Accepts::Float, &self.lean),
            ("bend", Accepts::Float, &self.bend),
            ("root_color", Accepts::Color, &self.root_color),
            ("tip_color", Accepts::Color, &self.tip_color),
            ("roughness", Accepts::Float, &self.roughness),
            ("clump", Accepts::Vec2, &self.clump),
        ]
    }

    fn ports_mut(&mut self) -> Vec<(&'static str, &mut Input)> {
        vec![
            ("density", &mut self.density),
            ("length", &mut self.length),
            ("width", &mut self.width),
            ("direction", &mut self.direction),
            ("lean", &mut self.lean),
            ("bend", &mut self.bend),
            ("root_color", &mut self.root_color),
            ("tip_color", &mut self.tip_color),
            ("roughness", &mut self.roughness),
            ("clump", &mut self.clump),
        ]
    }
}

/// A material graph: parameters, nodes and one PBR output.
///
/// Editable interchange data, the way `ashlar`'s recipe is. Serde reads and
/// writes exactly what the builder writes, and [`Self::build`] is the only way
/// to the validated [`Material`] a backend consumes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialGraph {
    /// Content identity, under the same name rules as a part.
    pub id: String,
    /// Exposed parameters, in authored order.
    #[serde(default)]
    pub params: Vec<Param>,
    /// Node id to node. The map is the graph; edges are ids inside the nodes.
    #[serde(default)]
    pub nodes: BTreeMap<String, Node>,
    /// What the material presents to a renderer.
    #[serde(default)]
    pub output: PbrOutput,
    /// Named strand layers this graph scatters beside its surface.
    ///
    /// A second kind of output, read once per strand at its root rather than
    /// once per texel; [`StrandLayer`] says what one is and why it lives in the
    /// graph rather than in a document of its own. Skipped when empty, so a
    /// graph that scatters nothing reads and writes exactly what it always did.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub strands: BTreeMap<String, StrandLayer>,
    /// How many metres of wall one repeat of this graph was authored to cover,
    /// where its author said.
    ///
    /// Advisory, and deliberately not a texel: nothing in a build, a lowering,
    /// a bake or a key reads it, so declaring it changes no byte any graph ever
    /// baked. What it records is the world scale the graph was *authored*
    /// against, which every graph has and none could state until now.
    /// [`PbrOutput::normal_strength`] is metres of relief per unit height per
    /// repeat and a [`StrandLayer`]'s lengths are absolute metres, so both are
    /// only true at one repeat size; that size lives on the other side of the
    /// seam, in a material definition's `tile_metres`, which this crate cannot
    /// see. Written here, the number a reader of a shipped graph would
    /// otherwise have to guess is in the graph, and the layer that holds both
    /// libraries can say when the two disagree.
    ///
    /// It is advice and not a rule because a definition is allowed to disagree
    /// on purpose: the same graph at a third of its repeat is how a specimen
    /// fakes three times the distance, and nothing here should refuse that.
    ///
    /// Skipped when absent, so every library written before this reads and
    /// writes exactly what it always did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tile_metres: Option<[f32; 2]>,
}

impl MaterialGraph {
    /// Start authoring a graph.
    pub fn builder(id: impl Into<String>) -> MaterialGraphBuilder {
        MaterialGraphBuilder {
            graph: Self {
                id: id.into(),
                params: Vec::new(),
                nodes: BTreeMap::new(),
                output: PbrOutput::default(),
                strands: BTreeMap::new(),
                tile_metres: None,
            },
            added: Vec::new(),
        }
    }

    /// Validate this graph and infer the type and period of every port.
    ///
    /// A graph that instances another through [`crate::nodes::Subgraph`] needs
    /// the library that holds it; use [`Self::build_in`] for those.
    pub fn build(self) -> Result<Material, GraphError> {
        let library = crate::MaterialGraphLibrary::default();
        let mut resolver = Resolver::new(&library);
        resolver.build(self)
    }

    /// Validate this graph against a library, resolving subgraphs as it goes.
    pub fn build_in(self, library: &crate::MaterialGraphLibrary) -> Result<Material, GraphError> {
        let mut resolver = Resolver::new(library);
        resolver.build(self)
    }

    /// Look up an exposed parameter by name.
    pub fn param(&self, name: &str) -> Option<&Param> {
        self.params.iter().find(|param| param.name == name)
    }

    /// Every signal this graph takes from whatever instances it, by name.
    ///
    /// An input is a node, so this is a reading of the node map rather than a
    /// table beside it. A [`Subgraph`](crate::nodes::Subgraph) binds these by
    /// name, and the resolver reads them here to say what a graph takes before
    /// it has built it — which it must, because what a caller binds is what the
    /// graph is then built under. Two inputs of one name are refused by
    /// [`Self::build`], so for any graph that builds this is as long as the
    /// declarations.
    ///
    /// An input written inline inside another node is not in the node map until
    /// a build hoists it there, so a caller that wants every input of an
    /// unbuilt graph reads this after [`Self::build`] or on the graph a
    /// [`Material`] carries.
    pub fn inputs(&self) -> BTreeMap<&str, &GraphInput> {
        self.nodes
            .values()
            .filter_map(|node| match node {
                Node::GraphInput(input) => Some((input.name.as_str(), input)),
                _ => None,
            })
            .collect()
    }

    /// This graph with `params` in place of its declared defaults.
    ///
    /// What a [`Bake`](crate::bake::BakeRequest) and a
    /// [`Surface::Shader`](crate::partition::partition) both need before they
    /// build: the values a definition binds are the graph's values, and a name
    /// the graph does not declare would otherwise bind nothing and leave the
    /// default in place, which reads as the backend ignoring what it was asked
    /// for. So an unknown name and a value of the wrong type are both errors at
    /// `params[name]`, the way a subgraph's binding is.
    ///
    /// A *live* parameter is overridden here too, and that is not a
    /// contradiction: the value written in becomes the one a partition folds if
    /// it has to freeze the parameter, and the one a caller should put in the
    /// uniform block if it does not. Either way the graph and the uniform say
    /// the same number.
    ///
    /// ```
    /// use ashlar_material::{MaterialGraph, Param, ParamValue, PbrOutput, nodes::Noise};
    ///
    /// let graph = MaterialGraph::builder("test:grain")
    ///     .param(Param::float("wear", 0.25))
    ///     .node("grain", Noise::value().period(8))
    ///     .output(PbrOutput::new().roughness("grain"))
    ///     .into_graph();
    /// let bound = graph.with_params(&[("wear".to_owned(), ParamValue::Float(0.75))].into())?;
    /// assert_eq!(bound.param("wear").map(|p| p.value), Some(ParamValue::Float(0.75)));
    /// # Ok::<(), ashlar_material::GraphError>(())
    /// ```
    pub fn with_params(
        &self,
        params: &BTreeMap<String, ParamValue>,
    ) -> Result<Self, crate::GraphError> {
        let mut graph = self.clone();
        let id = graph.id.clone();
        for (name, value) in params {
            let path = format!("params[{name}]");
            let Some(param) = graph.params.iter_mut().find(|param| param.name == *name) else {
                return Err(crate::GraphError::new(
                    path,
                    format!("graph {id:?} has no parameter {name:?}"),
                ));
            };
            let expected = param.value.value_type();
            let found = value.value_type();
            crate::require(
                found == expected,
                &path,
                &format!("graph {id:?} takes a {expected} for {name:?}, not a {found}"),
            )?;
            param.value = *value;
        }
        Ok(graph)
    }
}

/// Fluent authoring over the same data Serde reads.
#[derive(Clone, Debug)]
#[must_use]
pub struct MaterialGraphBuilder {
    graph: MaterialGraph,
    /// Node ids in the order they were added, so a duplicate is an error at
    /// build rather than a silent overwrite of the map entry.
    added: Vec<String>,
}

impl MaterialGraphBuilder {
    /// Expose a parameter.
    pub fn param(mut self, param: Param) -> Self {
        self.graph.params.push(param);
        self
    }

    /// Add a node under an id. A node value may be written inline in any input.
    pub fn node(mut self, id: impl Into<String>, node: impl Into<Node>) -> Self {
        let id = id.into();
        self.added.push(id.clone());
        self.graph.nodes.insert(id, node.into());
        self
    }

    /// Instance one library graph under several of its outputs at once.
    ///
    /// A compound answers a surface rather than a field: a wear pass hands back
    /// a colour, a roughness, a height and the mask it decided with, and each of
    /// those is read by a [`Subgraph`](crate::nodes::Subgraph) node of its own.
    /// Written out, that is four near-identical nodes whose bindings have to
    /// agree exactly — and they *must* agree, because an instance is inferred
    /// per binding signature, so a fifth input on one of the four would quietly
    /// make it a second instance of the same graph. This writes them from one
    /// node: the bindings are made once and copied, and the only thing that
    /// differs between the copies is which output each reads.
    ///
    /// Each node is added under `<id>.<port>`, the port being the name
    /// [`SurfaceOutput::port`](crate::nodes::SurfaceOutput::port) answers —
    /// `base_color`, `height`, or an extra's own name — which is the same
    /// spelling the flattener gives a node written inline in a port. Nothing
    /// else happens: the [`PbrOutput`] reads those ids like any others, so a
    /// layer that nothing reads is a layer that costs nothing, and listing an
    /// output twice is a duplicate id, reported at build.
    ///
    /// ```
    /// use ashlar_material::{
    ///     MaterialGraph, MaterialGraphLibrary, PbrOutput,
    ///     nodes::{GraphInput, Invert, Noise, Subgraph, SurfaceOutput},
    /// };
    ///
    /// let mut library = MaterialGraphLibrary::default();
    /// library.insert(
    ///     MaterialGraph::builder("game:tarnish")
    ///         .node("roughness", GraphInput::float("roughness", 0.8))
    ///         .node("dulled", Invert::new("roughness"))
    ///         .output(PbrOutput::new().roughness("dulled").extra("mask", "roughness"))
    ///         .into_graph(),
    /// );
    ///
    /// let material = MaterialGraph::builder("game:panel")
    ///     .node("grain", Noise::value().period(8))
    ///     .layer(
    ///         "tarnish",
    ///         Subgraph::new("game:tarnish").input("roughness", "grain"),
    ///         &[
    ///             SurfaceOutput::Roughness,
    ///             SurfaceOutput::Extra("mask".to_owned()),
    ///         ],
    ///     )
    ///     .output(
    ///         PbrOutput::new()
    ///             .roughness("tarnish.roughness")
    ///             .base_color("tarnish.mask"),
    ///     )
    ///     .build_in(&library)?;
    ///
    /// // One node per output, under the port's own name, and one instance
    /// // between them: the two bind the same field.
    /// assert!(material.port("tarnish.roughness").is_some());
    /// assert!(material.port("tarnish.mask").is_some());
    /// # Ok::<(), ashlar_material::GraphError>(())
    /// ```
    pub fn layer(
        mut self,
        id: impl AsRef<str>,
        subgraph: crate::nodes::Subgraph,
        outputs: &[crate::nodes::SurfaceOutput],
    ) -> Self {
        let id = id.as_ref();
        for output in outputs {
            let node = subgraph.clone().output(output.clone());
            self = self.node(format!("{id}.{}", output.port()), node);
        }
        self
    }

    /// Bind the PBR output.
    pub fn output(mut self, output: PbrOutput) -> Self {
        self.graph.output = output;
        self
    }

    /// Add a strand layer under a name.
    ///
    /// The name is what [`crate::strands::StrandRequest::layer`] asks for and
    /// what an error inside the layer is reported under, as
    /// `strands[name].inputs[port]`. A layer's fields read the same nodes and
    /// the same parameters the [`PbrOutput`] does, which is the whole reason a
    /// layer lives here rather than in a document beside the graph.
    ///
    /// ```
    /// use ashlar_material::{
    ///     MaterialGraph, PbrOutput, StrandLayer, nodes::Noise,
    /// };
    ///
    /// let material = MaterialGraph::builder("test:lawn")
    ///     .node("patches", Noise::value().period(8))
    ///     .output(PbrOutput::new().roughness("patches"))
    ///     .strands("blades", StrandLayer::new().count(64).density("patches"))
    ///     .build()?;
    ///
    /// // The layer's lattice joins the material's own repeat, and its fields
    /// // are resolved like any other bound output.
    /// assert_eq!(material.period(), ashlar_material::Period::square(64));
    /// assert!(material.strand("blades").is_some());
    /// # Ok::<(), ashlar_material::GraphError>(())
    /// ```
    pub fn strands(mut self, name: impl Into<String>, layer: StrandLayer) -> Self {
        self.graph.strands.insert(name.into(), layer);
        self
    }

    /// Declare how many metres of wall one repeat of this graph covers.
    ///
    /// The scale this graph's relief and strand lengths were authored against;
    /// [`MaterialGraph::tile_metres`] says why it is worth writing down and why
    /// nothing enforces it. Both axes must be finite and above zero, which is
    /// checked where every other field of the graph is.
    ///
    /// ```
    /// use ashlar_material::{MaterialGraph, PbrOutput, nodes::Noise};
    ///
    /// let material = MaterialGraph::builder("test:flags")
    ///     .node("stone", Noise::value().period(8))
    ///     .output(PbrOutput::new().height("stone").normal_strength(0.01))
    ///     // Two metres of paving per repeat, so that `normal_strength` reads
    ///     // as the centimetre of relief it is.
    ///     .tile_metres([2.0, 2.0])
    ///     .build()?;
    /// assert_eq!(material.graph().tile_metres, Some([2.0, 2.0]));
    /// # Ok::<(), ashlar_material::GraphError>(())
    /// ```
    pub fn tile_metres(mut self, metres: [f32; 2]) -> Self {
        self.graph.tile_metres = Some(metres);
        self
    }

    /// Validate the graph. Duplicate node ids are reported here.
    pub fn build(self) -> Result<Material, GraphError> {
        self.duplicates()?;
        self.graph.build()
    }

    /// Validate the graph against a library, resolving subgraphs.
    pub fn build_in(self, library: &crate::MaterialGraphLibrary) -> Result<Material, GraphError> {
        self.duplicates()?;
        self.graph.build_in(library)
    }

    /// The unvalidated graph, for putting in a library that validates it later.
    pub fn into_graph(self) -> MaterialGraph {
        self.graph
    }

    fn duplicates(&self) -> Result<(), GraphError> {
        let mut seen = BTreeSet::new();
        for id in &self.added {
            require(
                seen.insert(id.as_str()),
                &format!("nodes[{id}]"),
                "duplicate identity",
            )?;
        }
        Ok(())
    }
}

/// The inferred facts about one port: what it carries, and how it tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Port {
    /// The type the port carries, after any implicit conversion on the way in.
    pub value_type: ValueType,
    /// How the field repeats across UV `[0, 1)`.
    pub period: Period,
}

/// A validated graph: immutable, with a type and a period on every port.
///
/// Construction is only through [`MaterialGraph::build`], so no deserialization
/// path reaches a backend unchecked, and every node id a backend reads is known
/// to resolve.
#[derive(Clone, Debug, Serialize)]
#[serde(transparent)]
pub struct Material {
    graph: MaterialGraph,
    #[serde(skip)]
    ports: BTreeMap<String, Port>,
    #[serde(skip)]
    order: Vec<String>,
    #[serde(skip)]
    output: BTreeMap<String, Port>,
    #[serde(skip)]
    strand_ports: BTreeMap<String, BTreeMap<String, Port>>,
    #[serde(skip)]
    period: Period,
    #[serde(skip)]
    lattice: [u32; 2],
    #[serde(skip)]
    lattices: BTreeMap<String, [u32; 2]>,
    #[serde(skip)]
    instances: BTreeMap<InstanceKey, Arc<Material>>,
    #[serde(skip)]
    warnings: Vec<GraphWarning>,
}

/// Everything but the graph is derived from it, so it is not part of identity.
impl PartialEq for Material {
    fn eq(&self, other: &Self) -> bool {
        self.graph == other.graph
    }
}

impl Material {
    /// Read the validated graph without invalidating its guarantees.
    pub fn graph(&self) -> &MaterialGraph {
        &self.graph
    }

    /// Return editable data; build it again after making changes.
    pub fn into_graph(self) -> MaterialGraph {
        self.graph
    }

    /// The inferred port of one node, by id.
    pub fn port(&self, id: &str) -> Option<Port> {
        self.ports.get(id).copied()
    }

    /// Every node's inferred port.
    pub fn ports(&self) -> &BTreeMap<String, Port> {
        &self.ports
    }

    /// The inferred port of one [`PbrOutput`] input, by port name.
    ///
    /// The PBR names are `base_color`, `roughness`, `metallic`, `occlusion`,
    /// `height` and `emissive`; the last two are absent when unbound. A graph
    /// that exports extra masks answers for those under their own names too,
    /// which is how a [`Subgraph`](crate::nodes::Subgraph) reading one learns
    /// what it carries and how it tiles.
    pub fn output_port(&self, port: &str) -> Option<Port> {
        self.output.get(port).copied()
    }

    /// Every [`PbrOutput`] input's inferred port, by port name: the bound PBR
    /// channels and whatever extra masks the graph exports.
    pub fn output_ports(&self) -> &BTreeMap<String, Port> {
        &self.output
    }

    /// One strand layer by name, as the graph declares it.
    pub fn strand(&self, name: &str) -> Option<&StrandLayer> {
        self.graph.strands.get(name)
    }

    /// Every strand layer this material scatters, by name.
    pub fn strands(&self) -> &BTreeMap<String, StrandLayer> {
        &self.graph.strands
    }

    /// The inferred port of one strand field: what it carries and how it tiles.
    ///
    /// The port names are [`StrandLayer`]'s own — `density`, `length`, `width`,
    /// `direction`, `lean`, `bend`, `root_color`, `tip_color`, `roughness` and
    /// `clump` — and every one of them is always bound, because a layer carries
    /// a default for each.
    pub fn strand_port(&self, layer: &str, port: &str) -> Option<Port> {
        self.strand_ports.get(layer)?.get(port).copied()
    }

    /// Node ids in dependency order: every node appears after the nodes it reads.
    ///
    /// This is the order a lowering walks. Nodes nothing reads are included,
    /// as are comments, which have no output at all.
    pub fn order(&self) -> &[String] {
        &self.order
    }

    /// How the finished material tiles: the least common multiple of its outputs.
    ///
    /// Always [`Period::Tiled`]. An output that does not tile, and a set of
    /// outputs whose multiple runs past [`MAX_PERIOD`], are both build errors,
    /// so a material that exists has a repeat count a bake can use.
    pub fn period(&self) -> Period {
        self.period
    }

    /// The finest lattice any node in the graph lays, per axis.
    ///
    /// What a bake needs texels for. A [`Period`] is the count a field comes
    /// back to itself at, and it grows through every least common multiple
    /// downstream; this is the cell count of the finest *generator*, which is
    /// what a resolution has to resolve. A noise counts its finest octave, a
    /// wall its rows and columns rather than its bond, and a node that lays no
    /// lattice of its own counts one.
    ///
    /// Every node in the graph counts, including one no output reads: a lattice
    /// that fine in a graph is a graph to tidy either way, and reading only the
    /// live ones would make a bake's answer depend on which outputs happened to
    /// be bound. A graph a [`Subgraph`](crate::nodes::Subgraph) instanced
    /// counts too, because inlining it lays its lattice here.
    ///
    /// The count is carried *through* the graph rather than read off each node
    /// alone, because a resampler changes how fine a lattice lands: a
    /// [`Transform`](crate::nodes::Transform) scaling a noise by four lays four
    /// times the cells, and a [`Tile`](crate::nodes::Tile) that scatters a
    /// shape sixteen times across the repeat lays sixteen times whatever the
    /// shape laid. Reading the nodes one at a time would answer the noise's own
    /// count and let a bake resolve a quarter of what it wrote.
    ///
    /// ```
    /// use ashlar_material::{MaterialGraph, PbrOutput, nodes::Noise};
    ///
    /// let material = MaterialGraph::builder("test:grain")
    ///     // Four octaves doubling from a lattice of sixteen: the finest is
    ///     // 128, and a bake with fewer texels than that would be sampling it
    ///     // rather than writing it.
    ///     .node("grain", Noise::value().period(16).octaves(4))
    ///     .output(PbrOutput::new().roughness("grain"))
    ///     .build()?;
    /// assert_eq!(material.finest_lattice(), [128, 128]);
    /// # Ok::<(), ashlar_material::GraphError>(())
    /// ```
    pub fn finest_lattice(&self) -> [u32; 2] {
        self.lattice
    }

    /// One graph this material instances with nothing bound to its inputs, by
    /// the key a [`Subgraph`](crate::nodes::Subgraph) named.
    ///
    /// Validated when this material was, which is where recursion was refused.
    /// Lowering inlines these, so a material carries them rather than asking
    /// for the library again: a `Material` is the whole of what a backend
    /// needs, as it was before subgraphs lowered.
    ///
    /// A graph instanced with a field wired into one of its inputs is a
    /// *different* instance of the same key, inferred again under what was
    /// bound, and this answers `None` for it: what identifies one is the whole
    /// binding signature, which only the node that wrote it has.
    pub fn instanced(&self, key: &str) -> Option<&Material> {
        self.instance(&InstanceKey::unbound(key))
    }

    /// One instance this material holds, by the key its build derived.
    pub(crate) fn instance(&self, key: &InstanceKey) -> Option<&Material> {
        self.instances.get(key).map(Arc::as_ref)
    }

    /// One node's inputs as inference resolved them: what each carries, how it
    /// tiles, and how fine a lattice reaches it.
    ///
    /// The build answers this as it infers; the lowering asks it again when it
    /// inlines a [`Subgraph`](crate::nodes::Subgraph), because the instance to
    /// inline is the one the fields on that node's ports name. It is the same
    /// question in both places, so it is the same function over the ports and
    /// lattices the build left behind, and neither side can drift from the
    /// other.
    pub(crate) fn resolved_inputs(
        &self,
        node: &impl Ports,
        path: &str,
    ) -> Result<Vec<Resolved>, GraphError> {
        let params: BTreeMap<&str, &Param> = self
            .graph
            .params
            .iter()
            .map(|param| (param.name.as_str(), param))
            .collect();
        resolve_ports(node.inputs(), &params, &self.ports, &self.lattices, path)
    }

    /// What the graph will probably do wrong. Never a reason a build failed.
    pub fn warnings(&self) -> &[GraphWarning] {
        &self.warnings
    }

    /// Look up an exposed parameter by name.
    pub fn param(&self, name: &str) -> Option<&Param> {
        self.graph.param(name)
    }

    /// The port an input resolves to: a literal's own type at unit period, a
    /// parameter's type at unit period, or the port of the node it names.
    pub fn port_of(&self, input: &Input) -> Option<Port> {
        match input {
            Input::Const(value) => Some(Port {
                value_type: value.value_type(),
                period: Period::UNIT,
            }),
            Input::Param(name) => self.param(name).map(|param| Port {
                value_type: param.value.value_type(),
                period: Period::UNIT,
            }),
            Input::Node(id) => self.port(id),
            Input::Inline(_) => None,
        }
    }

    /// The type an input carries, before the consuming port converts it.
    pub fn type_of(&self, input: &Input) -> Option<ValueType> {
        self.port_of(input).map(|port| port.value_type)
    }
}

/// Move every inline node into the map under `parent.port`, depth first.
fn hoist(node: &mut Node, base: &str, out: &mut Vec<(String, Node)>) {
    for (port, input) in node.inputs_mut() {
        if !matches!(input, Input::Inline(_)) {
            continue;
        }
        let id = format!("{base}.{port}");
        if let Input::Inline(boxed) = std::mem::replace(input, Input::Node(id.clone())) {
            let mut child = *boxed;
            hoist(&mut child, &id, out);
            out.push((id, child));
        }
    }
}

/// Flatten every inline input in a graph, failing on a generated id that
/// collides with an authored one rather than replacing the author's node.
pub(crate) fn flatten(graph: &mut MaterialGraph) -> Result<(), GraphError> {
    let mut hoisted = Vec::new();
    for (id, node) in &mut graph.nodes {
        hoist(node, id, &mut hoisted);
    }
    for (port, input) in graph.output.ports_mut() {
        if !matches!(input, Input::Inline(_)) {
            continue;
        }
        let id = format!("output.{port}");
        if let Input::Inline(boxed) = std::mem::replace(input, Input::Node(id.clone())) {
            let mut child = *boxed;
            hoist(&mut child, &id, &mut hoisted);
            hoisted.push((id, child));
        }
    }
    // A strand field takes a node written inline the way every other port does,
    // under `strands.<layer>.<port>` so that the id says where it came from.
    for (layer, strands) in &mut graph.strands {
        for (port, input) in strands.ports_mut() {
            if !matches!(input, Input::Inline(_)) {
                continue;
            }
            let id = format!("strands.{layer}.{port}");
            if let Input::Inline(boxed) = std::mem::replace(input, Input::Node(id.clone())) {
                let mut child = *boxed;
                hoist(&mut child, &id, &mut hoisted);
                hoisted.push((id, child));
            }
        }
    }
    for (id, node) in hoisted {
        require(
            graph.nodes.insert(id.clone(), node).is_none(),
            &format!("nodes[{id}]"),
            "an inline input generated an id an authored node already uses",
        )?;
    }
    Ok(())
}

/// Why a field does not tile, and which node is answerable for it.
#[derive(Clone, Debug)]
struct FreeCause {
    node: String,
    reason: String,
}

/// Validate a graph and infer every port. The resolver is what makes a
/// subgraph's type and period knowable, and what detects recursion.
///
/// `bindings` is what an instance wired into this graph's inputs, and is
/// `None` for a graph built on its own — which is every top-level build, and
/// every instance of a graph nothing binds anything to. It is why a graph is
/// built once per binding signature rather than once per key: an input is a
/// leaf whose period and lattice are the bound field's, and everything
/// downstream of it is inferred from those.
pub(crate) fn build(
    mut graph: MaterialGraph,
    resolver: &mut Resolver<'_>,
    bindings: Option<&Bindings>,
) -> Result<Material, GraphError> {
    // Before flattening, because flattening generates an id per inline output
    // input and two ports of one name would generate one id twice. A layer's
    // own constants are checked here for the same reason the extras' names are:
    // the message should be about what the author wrote rather than about an id
    // generated under it.
    graph.output.check()?;
    for (name, layer) in &graph.strands {
        let path = format!("strands[{name}]");
        crate::name(name, &format!("{path}.name"))?;
        layer.check(&path)?;
    }
    flatten(&mut graph)?;
    name(&graph.id, "id")?;
    // A declared repeat decides nothing here — no texel reads it — but a
    // nonsense one would be read by the layer that compares it with a material
    // definition's own tiling, and a zero there is a division by zero in
    // somebody else's crate.
    if let Some(metres) = graph.tile_metres {
        for (axis, metres) in metres.into_iter().enumerate() {
            let path = format!("tile_metres[{axis}]");
            finite(metres, &path, "a repeat size")?;
            require(metres > 0.0, &path, "a repeat size must be above zero")?;
        }
    }
    let mut params: BTreeMap<&str, &Param> = BTreeMap::new();
    for param in &graph.params {
        let path = format!("params[{}]", param.name);
        param.check(&path)?;
        require(
            params.insert(&param.name, param).is_none(),
            &path,
            "duplicate identity",
        )?;
    }
    // Two inputs of one name would leave a caller binding one of them and the
    // other quietly reading its default, with nothing in the graph to say
    // which got the field.
    let mut inputs: BTreeMap<&str, &str> = BTreeMap::new();
    for (id, node) in &graph.nodes {
        name(id, &format!("nodes[{id}]"))?;
        node.check(&format!("nodes[{id}]"))?;
        if let Node::GraphInput(input) = node
            && let Some(first) = inputs.insert(&input.name, id)
        {
            return Err(GraphError::new(
                format!("nodes[{id}].name"),
                format!(
                    "input {:?} is already declared by node {first:?}",
                    input.name
                ),
            ));
        }
    }
    references(&graph, &params)?;
    let order = topological_order(&graph)?;
    let Inference {
        ports,
        lattices,
        instances,
        causes,
        mut warnings,
    } = infer(&graph, &order, &params, resolver, bindings)?;
    let lattice = lattices.values().fold([1, 1], |finest, cell| {
        [finest[0].max(cell[0]), finest[1].max(cell[1])]
    });
    // Whatever the graphs this one instanced warned about is the author's to
    // read as well, rooted at the key it came from.
    warnings.extend(resolver.take_warnings());
    let (output, mut period) = output_ports(&graph, &params, &ports, &causes)?;
    let (strand_ports, with_strands) = strand_ports(&graph, &params, &ports, &causes, period)?;
    period = with_strands;
    Ok(Material {
        graph,
        ports,
        order,
        output,
        strand_ports,
        period,
        lattice,
        lattices,
        instances,
        warnings,
    })
}

/// The inference half of a build: a port and a lattice per node, the instances
/// its subgraphs asked for, and what went free or looked suspicious on the way.
///
/// Its own function because a build is two passes over the same graph and only
/// this one walks it in dependency order: everything above is about the graph
/// as written, and everything below is about the output, which reads what this
/// left behind.
struct Inference {
    ports: BTreeMap<String, Port>,
    lattices: BTreeMap<String, [u32; 2]>,
    instances: BTreeMap<InstanceKey, Arc<Material>>,
    causes: BTreeMap<String, FreeCause>,
    warnings: Vec<GraphWarning>,
}

fn infer(
    graph: &MaterialGraph,
    order: &[String],
    params: &BTreeMap<&str, &Param>,
    resolver: &mut Resolver<'_>,
    bindings: Option<&Bindings>,
) -> Result<Inference, GraphError> {
    let mut ports: BTreeMap<String, Port> = BTreeMap::new();
    let mut lattices: BTreeMap<String, [u32; 2]> = BTreeMap::new();
    // Shared rather than cloned: a graph carries the instances it holds, and
    // those carry theirs, so a library where two graphs instance one third
    // would otherwise copy that third once per path to it.
    let mut instances: BTreeMap<InstanceKey, Arc<Material>> = BTreeMap::new();
    let mut causes: BTreeMap<String, FreeCause> = BTreeMap::new();
    let mut warnings = Vec::new();
    // What an instance wired into one of this graph's inputs, for the node that
    // declares it. Nothing outside an instanced graph binds anything, and an
    // input an instance left alone is its default there too.
    let bound = |node: &Node| match node {
        Node::GraphInput(input) => bindings.and_then(|bindings| bindings.get(&input.name)),
        _ => None,
    };
    for id in order {
        let Some(node) = graph.nodes.get(id) else {
            continue;
        };
        let path = format!("nodes[{id}]");
        let inputs = resolve_inputs(node, params, &ports, &lattices, &path)?;
        let Some(value_type) = output_type(node, &inputs, &path)? else {
            continue;
        };
        let mut instance = None;
        let inferred = match node {
            Node::Subgraph(subgraph) => {
                let (key, period) = resolver.subgraph_instance(subgraph, &inputs, &path)?;
                // The instance a subgraph asked for is validated by the line
                // above, so it is here to be carried away: lowering inlines it,
                // and a node holds its graph only by key.
                if let Some(material) = resolver.built(&key)
                    && !instances.contains_key(&key)
                {
                    instances.insert(key.clone(), Arc::clone(material));
                }
                instance = Some(key);
                period::passthrough(period)
            }
            // An input is the field bound to it, and what that field does is
            // knowledge this build has and the node does not: the instance is
            // built again under its bindings, which is what makes the period of
            // an input a question with an answer at all.
            Node::GraphInput(input) => match bound(node) {
                Some(binding) => period::bound_input(&input.name, binding.period),
                None => period::node_period(node, &inputs),
            },
            _ => period::node_period(node, &inputs),
        };
        if let Some(message) = inferred.warning {
            warnings.push(GraphWarning::new(&path, message));
        }
        if inferred.period == Period::Free {
            causes.insert(
                id.clone(),
                cause(id, node, &inputs, &causes, inferred.reason),
            );
        }
        let lattice = if let Some(key) = &instance {
            // What the instance lays is what inlining it lays here, the fields
            // bound into it included: it was inferred under them.
            instances
                .get(key)
                .map_or([1, 1], |material| material.finest_lattice())
        } else if let Some(binding) = bound(node) {
            binding.lattice
        } else {
            period::node_lattice(node, &inputs)
        };
        lattices.insert(id.clone(), lattice);
        ports.insert(
            id.clone(),
            Port {
                value_type,
                period: inferred.period,
            },
        );
    }
    Ok(Inference {
        ports,
        lattices,
        instances,
        causes,
        warnings,
    })
}

/// Every reference in the graph names something that exists and is readable.
fn references(graph: &MaterialGraph, params: &BTreeMap<&str, &Param>) -> Result<(), GraphError> {
    for (id, node) in &graph.nodes {
        for port in node.inputs() {
            let path = format!("nodes[{id}].inputs[{}]", port.name);
            reference(port.input, graph, params, &path, Some(id.as_str()))?;
        }
        // The one node that names something of the graph other than a node or
        // a parameter: a strand layer, by the name it is declared under.
        if let crate::Node::StrandRelief(relief) = node {
            require(
                graph.strands.contains_key(&relief.layer),
                &format!("nodes[{id}].layer"),
                &format!(
                    "this graph declares no strand layer {:?}; it declares {}",
                    relief.layer,
                    if graph.strands.is_empty() {
                        "none at all".to_owned()
                    } else {
                        graph
                            .strands
                            .keys()
                            .map(String::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                ),
            )?;
        }
    }
    for (port, _, input) in graph.output.ports() {
        reference(input, graph, params, &format!("output.{port}"), None)?;
    }
    for (layer, strands) in &graph.strands {
        for (port, _, input) in strands.ports() {
            let path = format!("strands[{layer}].inputs[{port}]");
            reference(input, graph, params, &path, None)?;
        }
    }
    Ok(())
}

fn reference(
    input: &Input,
    graph: &MaterialGraph,
    params: &BTreeMap<&str, &Param>,
    path: &str,
    owner: Option<&str>,
) -> Result<(), GraphError> {
    match input {
        Input::Const(value) => require(
            value.is_finite(),
            path,
            "a literal must be finite; NaN poisons every texel it reaches",
        ),
        Input::Param(name) => require(
            params.contains_key(name.as_str()),
            path,
            &format!("unknown parameter {name:?}"),
        ),
        Input::Node(target) => {
            require(
                Some(target.as_str()) != owner,
                path,
                "node references itself",
            )?;
            let Some(node) = graph.nodes.get(target) else {
                return Err(GraphError::new(path, format!("unknown node {target:?}")));
            };
            require(
                !matches!(node.output(), Output::None),
                path,
                &format!("node {target:?} is a {} and has no output", node.kind()),
            )
        }
        Input::Inline(_) => Err(GraphError::new(
            path,
            "an inline node survived flattening; build the graph rather than validating it by hand",
        )),
    }
}

/// How far the depth-first walk has got with one node.
#[derive(Clone, Copy, PartialEq)]
enum Mark {
    /// Not visited.
    White,
    /// On the stack: reaching it again is a cycle.
    Grey,
    /// Finished, and in the order.
    Black,
}

/// Dependencies first, with the node that closes a cycle named by input path.
fn topological_order(graph: &MaterialGraph) -> Result<Vec<String>, GraphError> {
    let ids: Vec<&str> = graph.nodes.keys().map(String::as_str).collect();
    let index: BTreeMap<&str, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let edges: Vec<Vec<(usize, Cow<'static, str>)>> = graph
        .nodes
        .values()
        .map(|node| {
            node.inputs()
                .iter()
                .filter_map(|port| match port.input {
                    Input::Node(target) => {
                        index.get(target.as_str()).map(|i| (*i, port.name.clone()))
                    }
                    _ => None,
                })
                .collect()
        })
        .collect();
    let mut marks = vec![Mark::White; ids.len()];
    let mut order = Vec::with_capacity(ids.len());
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for root in 0..ids.len() {
        if marks[root] != Mark::White {
            continue;
        }
        marks[root] = Mark::Grey;
        stack.push((root, 0));
        while let Some((node, edge)) = stack.pop() {
            if edge == edges[node].len() {
                marks[node] = Mark::Black;
                order.push(ids[node].to_owned());
                continue;
            }
            stack.push((node, edge + 1));
            let (target, port) = &edges[node][edge];
            let target = *target;
            match marks[target] {
                Mark::Black => {}
                Mark::White => {
                    marks[target] = Mark::Grey;
                    stack.push((target, 0));
                }
                Mark::Grey => {
                    let start = stack
                        .iter()
                        .position(|(open, _)| *open == target)
                        .unwrap_or(0);
                    let mut cycle: Vec<&str> =
                        stack[start..].iter().map(|(open, _)| ids[*open]).collect();
                    cycle.push(ids[target]);
                    return Err(GraphError::new(
                        format!("nodes[{}].inputs[{port}]", ids[node]),
                        format!("cycle: {}", cycle.join(" -> ")),
                    ));
                }
            }
        }
    }
    Ok(order)
}

/// The type and period on each of a node's inputs, checked against what the
/// port accepts and against what the node makes of them together. Every
/// reference is already known to resolve.
fn resolve_inputs(
    node: &Node,
    params: &BTreeMap<&str, &Param>,
    ports: &BTreeMap<String, Port>,
    lattices: &BTreeMap<String, [u32; 2]>,
    path: &str,
) -> Result<Vec<Resolved>, GraphError> {
    let inputs = resolve_ports(node.inputs(), params, ports, lattices, path)?;
    node.check_types(&inputs, path)?;
    Ok(inputs)
}

/// The same, over ports alone: what a validated material answers again when the
/// lowering asks what reached a node, where the node's own checks have already
/// been made and nothing is left to check twice.
fn resolve_ports(
    ports_of: Vec<InputPort<'_>>,
    params: &BTreeMap<&str, &Param>,
    ports: &BTreeMap<String, Port>,
    lattices: &BTreeMap<String, [u32; 2]>,
    path: &str,
) -> Result<Vec<Resolved>, GraphError> {
    let mut inputs = Vec::new();
    for port in ports_of {
        let path = format!("{path}.inputs[{}]", port.name);
        let Some(found) = resolve(port.input, params, ports) else {
            return Err(GraphError::new(path, "input does not resolve to a value"));
        };
        require(
            port.accepts.admits(found.value_type),
            &path,
            &mismatch(port.accepts, found.value_type),
        )?;
        // A literal and a parameter lay no lattice at all; a node lays whatever
        // reached it, scaled by whatever it did to the frame.
        let lattice = match port.input {
            Input::Node(id) => lattices.get(id.as_str()).copied().unwrap_or([1, 1]),
            _ => [1, 1],
        };
        inputs.push(Resolved {
            name: port.name,
            value_type: found.value_type,
            period: found.period,
            lattice,
        });
    }
    Ok(inputs)
}

/// What an input carries: a literal's own type at unit period, a parameter's
/// type at unit period, or the port of the node it names.
fn resolve(
    input: &Input,
    params: &BTreeMap<&str, &Param>,
    ports: &BTreeMap<String, Port>,
) -> Option<Port> {
    match input {
        Input::Const(value) => Some(Port {
            value_type: value.value_type(),
            period: Period::UNIT,
        }),
        Input::Param(name) => params.get(name.as_str()).map(|param| Port {
            value_type: param.value.value_type(),
            period: Period::UNIT,
        }),
        Input::Node(id) => ports.get(id.as_str()).copied(),
        Input::Inline(_) => None,
    }
}

/// Who to blame for a field that does not tile: the first input that was
/// already free, or this node itself.
fn cause(
    id: &str,
    node: &Node,
    inputs: &[Resolved],
    causes: &BTreeMap<String, FreeCause>,
    reason: Option<String>,
) -> FreeCause {
    let inherited = inputs
        .iter()
        .filter(|input| input.period == Period::Free)
        .find_map(|input| {
            let port = node
                .inputs()
                .into_iter()
                .find(|port| port.name == input.name)?;
            match port.input {
                Input::Node(target) => causes.get(target.as_str()).cloned(),
                _ => None,
            }
        });
    inherited.unwrap_or(FreeCause {
        node: id.to_owned(),
        reason: reason.unwrap_or_else(|| format!("{} does not tile", node.kind())),
    })
}

fn mismatch(accepts: Accepts, found: ValueType) -> String {
    if found == ValueType::Vec2 {
        format!(
            "expected {}, found a Vec2, which converts to nothing",
            accepts.describe()
        )
    } else {
        format!("expected {}, found a {found}", accepts.describe())
    }
}

/// The type a node's output carries, or `None` for a node that has no output.
fn output_type(
    node: &Node,
    inputs: &[Resolved],
    path: &str,
) -> Result<Option<ValueType>, GraphError> {
    let of = |name: &str| {
        inputs
            .iter()
            .find(|input| input.name == name)
            .map(|input| input.value_type)
    };
    Ok(match node.output() {
        Output::None => None,
        Output::Fixed(value_type) => Some(value_type),
        Output::SameAs(port) => Some(of(port).unwrap_or(ValueType::Float)),
        Output::Join(left, right) => {
            let left = of(left).unwrap_or(ValueType::Float);
            let right = of(right).unwrap_or(ValueType::Float);
            match left.join(right) {
                Some(value_type) => Some(value_type),
                None => {
                    return Err(GraphError::new(
                        path,
                        format!("cannot combine a {left} with a {right}"),
                    ));
                }
            }
        }
    })
}

/// Check the output's own inputs, and take the least common multiple of their
/// periods. A free input is an error that names the node that broke it, and so
/// is a multiple of otherwise tiled outputs that no bake could hold.
fn output_ports(
    graph: &MaterialGraph,
    params: &BTreeMap<&str, &Param>,
    ports: &BTreeMap<String, Port>,
    causes: &BTreeMap<String, FreeCause>,
) -> Result<(BTreeMap<String, Port>, Period), GraphError> {
    finite(
        graph.output.normal_strength,
        "output.normal_strength",
        "relief",
    )?;
    require(
        graph.output.normal_strength >= 0.0,
        "output.normal_strength",
        "relief must not be negative; invert the height field instead",
    )?;
    let mut resolved = BTreeMap::new();
    let mut period = Period::UNIT;
    for (name, accepts, input) in graph.output.ports() {
        let path = format!("output.{name}");
        let Some(found) = resolve(input, params, ports) else {
            return Err(GraphError::new(path, "input does not resolve to a value"));
        };
        require(
            accepts.admits(found.value_type),
            &path,
            &mismatch(accepts, found.value_type),
        )?;
        if found.period == Period::Free {
            let cause = match input {
                Input::Node(id) => causes.get(id.as_str()),
                _ => None,
            };
            return Err(match cause {
                Some(cause) => GraphError::new(
                    format!("nodes[{}]", cause.node),
                    format!("{}, and the material output must tile", cause.reason),
                ),
                None => GraphError::new(path, "the material output must tile"),
            });
        }
        let combined = period.lcm(found.period);
        // Both sides tile here, so the only way the multiple is free is that it
        // ran past what a bake can hold. The material would then have no repeat
        // count to bake at, which is the free-output error by another route.
        require(
            combined.is_tiled(),
            &path,
            &format!(
                "a period of {} and the {period} of the outputs before it have no common multiple \
                 within {MAX_PERIOD} repeats, so the material has no period to bake at",
                found.period
            ),
        )?;
        period = combined;
        resolved.insert((*name).to_owned(), found);
    }
    Ok((resolved, period))
}

/// The same, over the strand layers: a port per field, and the material's
/// repeat grown by what each layer contributes to it.
///
/// A strand field is a root like a bound PBR channel, and it is held to the
/// same tiling rule for the same reason — a set of strands that does not meet
/// itself at the seam is as wrong as a colour that does not, and a scatter is
/// worse, because what shows there is a line of blades leaning the other way.
/// The layer's own `count` joins the multiple beside the fields: the lattice
/// the roots sit on is laid over the repeat by the graph, so the repeat has to
/// carry it.
fn strand_ports(
    graph: &MaterialGraph,
    params: &BTreeMap<&str, &Param>,
    ports: &BTreeMap<String, Port>,
    causes: &BTreeMap<String, FreeCause>,
    output_period: Period,
) -> Result<(BTreeMap<String, BTreeMap<String, Port>>, Period), GraphError> {
    let mut resolved: BTreeMap<String, BTreeMap<String, Port>> = BTreeMap::new();
    let mut period = output_period;
    for (layer, strands) in &graph.strands {
        let mut fields = BTreeMap::new();
        let join = |path: &str, found: Period, period: &mut Period| {
            let combined = period.lcm(found);
            require(
                combined.is_tiled(),
                path,
                &format!(
                    "a period of {found} and the {period} of the outputs before it have no common \
                     multiple within {MAX_PERIOD} repeats, so the material has no period to bake at"
                ),
            )?;
            *period = combined;
            Ok::<(), GraphError>(())
        };
        join(
            &format!("strands[{layer}].count"),
            strands.period(),
            &mut period,
        )?;
        for (name, accepts, input) in strands.ports() {
            let path = format!("strands[{layer}].inputs[{name}]");
            let Some(found) = resolve(input, params, ports) else {
                return Err(GraphError::new(path, "input does not resolve to a value"));
            };
            require(
                accepts.admits(found.value_type),
                &path,
                &mismatch(accepts, found.value_type),
            )?;
            if found.period == Period::Free {
                let cause = match input {
                    Input::Node(id) => causes.get(id.as_str()),
                    _ => None,
                };
                return Err(match cause {
                    Some(cause) => GraphError::new(
                        format!("nodes[{}]", cause.node),
                        format!("{}, and a strand field must tile", cause.reason),
                    ),
                    None => GraphError::new(path, "a strand field must tile"),
                });
            }
            join(&path, found.period, &mut period)?;
            fields.insert(name.to_owned(), found);
        }
        resolved.insert(layer.clone(), fields);
    }
    Ok((resolved, period))
}
