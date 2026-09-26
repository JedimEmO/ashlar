//! Buffered filters: the nodes that need a neighbourhood, and so a plane.
//!
//! In a bake each of these is an intermediate plane at the bake resolution,
//! filtered with wrap so the result tiles as its input did, and read back
//! bilinearly: the pipeline is in [`planes`](crate::planes) and the plan a node
//! leaves behind is a [`BufferPlan`](crate::ir::BufferPlan). In a shader they
//! cannot be evaluated per fragment, so they are where the compiler cuts the
//! graph and bakes what is upstream into a bound texture.
//!
//! Every radius here is a reach in UV, not a count of texels, so a graph
//! describes the same surface at every resolution. Two nodes that ask for the
//! same filter over the same input share one plane, and a node whose output
//! nothing reads leaves none.

use serde::{Deserialize, Serialize};

use crate::{
    GraphError, Input, ValueType, finite,
    ir::{Filter, Lower, Lowering, NodeInputs, Op, ValueId},
    nodes::{Check, Output, generator::check_unit, ports},
    require,
};

/// Which blur.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum BlurKind {
    /// Isotropic, separable, wrapped.
    #[default]
    Gaussian,
    /// Along [`Blur::angle`] only, which is what makes a brushed metal.
    Directional,
    /// Along the slope of [`Blur::height`], which smears wear along the relief.
    Slope,
}

/// How samples along a slope walk combine, including the starting sample.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlopeMode {
    /// Smooth by averaging all samples (the historical behavior).
    #[default]
    Average,
    /// Erode ridges and chip silhouettes without softening surviving faces.
    Min,
    /// Grow deposits along the guide's flow.
    Max,
}

/// How many displacements a slope blur may take.
///
/// Each step reads the guide gradient and the source at every texel. The cap
/// permits 32-step authored damage filters while bounding their linear cost.
pub const MAX_SLOPE_STEPS: u32 = 32;

/// A wrapped blur over a rasterised plane.
///
/// [`Self::radius`] is how far the kernel reaches, in UV units: the Gaussian is
/// truncated there and its standard deviation is a third of it, so a radius of
/// `0.01` is a blur whose weight is all but three parts in a thousand inside a
/// hundredth of the repeat, at every resolution. A radius under half a texel of
/// the plane is an identity, because there is nothing for it to reach.
///
/// The three kinds read the same radius and differ in where they spend it:
///
/// - [`BlurKind::Gaussian`] spends it over both axes, separably.
/// - [`BlurKind::Directional`] spends it along [`Self::angle`] alone. A grain
///   through one of these is a brushed or rolled metal, and the anisotropy is
///   the whole point: what the eye reads as "brushed" is highlight that is
///   smeared one way and sharp the other.
/// - [`BlurKind::Slope`] spends it walking *down* [`Self::height`],
///   [`Self::steps`] displacements of `radius / steps` each, averaging the
///   source at every place it stopped. Flat ground does not move at all, so
///   what this does is drag one field along the shape of another — and since a
///   texel reads what lies *below* it, what travels is uphill: rust out of a
///   chip and onto the paint, damp out of a joint and onto the brick, a stain
///   up the wall from where it pooled. Inverting the height walks the other
///   way. It is the node that turns a clean mask into a worn one.
///   Set [`Self::slope_mode`] to [`SlopeMode::Min`] to chip the source, or
///   [`SlopeMode::Max`] to grow it. Both include the original sample, so
///   minimum never raises a texel and maximum never lowers one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Blur {
    /// The field to blur.
    pub input: Input,
    /// The height a slope blur walks down. Read by [`BlurKind::Slope`] alone,
    /// exactly as a unary [`Math`](super::Math) never reads its second operand.
    pub height: Input,
    /// Radius in UV, so it is resolution-independent.
    pub radius: f32,
    /// Which blur.
    pub kind: BlurKind,
    /// Direction in degrees, for a directional blur.
    pub angle: f32,
    /// How many displacements a slope blur takes, up to [`MAX_SLOPE_STEPS`].
    pub steps: u32,
    /// Sample reduction for slope blur; ignored by other blur kinds.
    pub slope_mode: SlopeMode,
}

impl Default for Blur {
    fn default() -> Self {
        Self {
            input: Input::default(),
            height: Input::default(),
            radius: 0.01,
            kind: BlurKind::default(),
            angle: 0.0,
            steps: 8,
            slope_mode: SlopeMode::Average,
        }
    }
}

impl Blur {
    /// Blur a field.
    pub fn new(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            ..Self::default()
        }
    }

    /// Blur a field along one direction, in degrees.
    pub fn directional(input: impl Into<Input>, degrees: f32) -> Self {
        Self::new(input).kind(BlurKind::Directional).angle(degrees)
    }

    /// Smear a field downhill on a height field.
    pub fn slope(input: impl Into<Input>, height: impl Into<Input>) -> Self {
        Self {
            height: height.into(),
            ..Self::new(input).kind(BlurKind::Slope)
        }
    }

    /// Set the radius in UV.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    /// Choose the blur.
    pub fn kind(mut self, kind: BlurKind) -> Self {
        self.kind = kind;
        self
    }

    /// Set the direction of a directional blur, in degrees.
    pub fn angle(mut self, degrees: f32) -> Self {
        self.angle = degrees;
        self
    }

    /// Set how many displacements a slope blur takes.
    pub fn steps(mut self, steps: u32) -> Self {
        self.steps = steps;
        self
    }

    /// Choose how the slope walk combines its samples.
    pub fn slope_mode(mut self, mode: SlopeMode) -> Self {
        self.slope_mode = mode;
        self
    }

    /// Which of the two inputs this kind actually reads.
    ///
    /// Period inference asks, for the reason a unary [`Math`](super::Math)
    /// makes it ask: an input a lowering never emits cannot move the lattice
    /// the answer lands on, and taking the multiple of one would inflate the
    /// period of every Gaussian blur in a graph that also holds a height.
    pub(crate) fn reads(&self, port: &str) -> bool {
        port == "input" || (port == "height" && self.kind == BlurKind::Slope)
    }
}

ports!(
    Blur,
    Output::SameAs("input"),
    "input": Field => input,
    "height": Float => height,
);

impl Check for Blur {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        check_unit(self.radius, "radius", path)?;
        finite(self.angle, &format!("{path}.angle"), "angle")?;
        require(
            (1..=MAX_SLOPE_STEPS).contains(&self.steps),
            &format!("{path}.steps"),
            &format!("a slope blur takes between one and {MAX_SLOPE_STEPS} steps"),
        )
    }
}

/// Which neighbourhood a [`Curvature`] compares a texel against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum CurvatureKind {
    /// Four taps at the radius: the discrete Laplacian, negated so that a crest
    /// is positive. Cheap, and sharp enough to find a bevel.
    #[default]
    Laplacian,
    /// The height less its own Gaussian blur at the radius. Smoother, and what
    /// to reach for when the height is noisy.
    Blurred,
}

impl CurvatureKind {
    /// A stable number for the cache key, written out as every other code is.
    pub(crate) fn code(self) -> u32 {
        match self {
            Self::Laplacian => 0,
            Self::Blurred => 1,
        }
    }
}

/// Which side of the curvature a [`Curvature`] presents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum CurvatureOutput {
    /// The whole signed field, centred on a half: flat is `0.5`, a crest is
    /// above it and a hollow below.
    #[default]
    Signed,
    /// The crests alone, as a mask in `0..=1`.
    Peaks,
    /// The hollows alone, as a mask in `0..=1`.
    Cavity,
}

/// How far a height field stands above its own neighbourhood: the node that
/// finds the edges of relief without being told where they are.
///
/// This is the mask a worn surface is built out of. A wall's mortar is a
/// hollow and its arrises are crests; a chipped paint film is a plateau with a
/// rim; the difference between "the joint" and "the brick" is a fact about the
/// *height* rather than something a second generator has to be talked into
/// drawing in the same places. So [`CurvatureOutput::Cavity`] is dirt, damp and
/// dust, and [`CurvatureOutput::Peaks`] is polish, chipping and light — the two
/// shortcuts this node carries rather than two nodes over one filter, because
/// they are the two signs of one number and an author who wants both should pay
/// for one plane.
///
/// What the filter answers is a difference of heights rather than a second
/// derivative, which is what keeps it the same field at every resolution and
/// its magnitude the relief of the feature. It is therefore *small* — a crest
/// standing a fiftieth proud of its surroundings answers a fiftieth — and
/// [`Self::strength`] is the gain that turns it into a mask. The default of
/// eight is a starting point for a height field bedded in `0..=1`, not a
/// calibration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Curvature {
    /// The height field.
    pub height: Input,
    /// How far the neighbourhood reaches, in UV.
    pub radius: f32,
    /// Which neighbourhood.
    pub kind: CurvatureKind,
    /// The gain applied before the output is presented.
    pub strength: f32,
    /// Which side of the field to present.
    pub output: CurvatureOutput,
}

impl Default for Curvature {
    fn default() -> Self {
        Self {
            height: Input::default(),
            radius: 0.01,
            kind: CurvatureKind::default(),
            strength: 8.0,
            output: CurvatureOutput::default(),
        }
    }
}

impl Curvature {
    /// The signed curvature of a height field.
    pub fn new(height: impl Into<Input>) -> Self {
        Self {
            height: height.into(),
            ..Self::default()
        }
    }

    /// The hollows of a height field, as a mask.
    pub fn cavity(height: impl Into<Input>) -> Self {
        Self::new(height).output(CurvatureOutput::Cavity)
    }

    /// The crests of a height field, as a mask.
    pub fn peaks(height: impl Into<Input>) -> Self {
        Self::new(height).output(CurvatureOutput::Peaks)
    }

    /// Set the radius in UV.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    /// Choose the neighbourhood.
    pub fn kind(mut self, kind: CurvatureKind) -> Self {
        self.kind = kind;
        self
    }

    /// Set the gain.
    pub fn strength(mut self, strength: f32) -> Self {
        self.strength = strength;
        self
    }

    /// Choose which side of the field to present.
    pub fn output(mut self, output: CurvatureOutput) -> Self {
        self.output = output;
        self
    }
}

ports!(
    Curvature,
    Output::Fixed(ValueType::Float),
    "height": Float => height,
);

impl Check for Curvature {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        check_unit(self.radius, "radius", path)?;
        finite(self.strength, &format!("{path}.strength"), "strength")
    }
}

/// Where a field changes: the length of its two central differences over the
/// radius, as a mask.
///
/// The difference rather than the slope, so the edge of a mask that steps from
/// zero to one is one whatever the radius, and the radius sets how *wide* the
/// line is rather than how bright. A ramp has an edge everywhere and a crest
/// has one on both of its flanks, which is what tells this apart from a
/// [`Curvature`]: an edge detect finds where a field moves, a curvature finds
/// where it turns.
///
/// It is how a chip gets a lip and a tile gets a drawn joint without either
/// being authored twice.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct EdgeDetect {
    /// The field to find the edges of.
    pub input: Input,
    /// Half the span the difference is taken over, in UV.
    pub radius: f32,
    /// The gain applied before the mask is clamped.
    pub strength: f32,
}

impl Default for EdgeDetect {
    fn default() -> Self {
        Self {
            input: Input::default(),
            radius: 0.004,
            strength: 1.0,
        }
    }
}

impl EdgeDetect {
    /// Find the edges of a field.
    pub fn new(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            ..Self::default()
        }
    }

    /// Set the radius in UV.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    /// Set the gain.
    pub fn strength(mut self, strength: f32) -> Self {
        self.strength = strength;
        self
    }
}

ports!(
    EdgeDetect,
    Output::Fixed(ValueType::Float),
    "input": Float => input,
);

impl Check for EdgeDetect {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        check_unit(self.radius, "radius", path)?;
        require(
            self.strength >= 0.0 && self.strength.is_finite(),
            &format!("{path}.strength"),
            "strength must be finite and not negative",
        )
    }
}

/// Horizon-based ambient occlusion from a height field, wrapped.
///
/// One means the texel sees the whole sky and zero means none of it. The filter
/// marches eight directions out to [`Self::radius`] and keeps the steepest rise
/// it finds in each; the mean of the sines of those angles is how much sky the
/// surface took away, and [`Self::strength`] scales it. The height is read as a
/// length in the same units as UV, so a joint a tenth deep and a hundredth wide
/// is a wall and darkens its own foot.
///
/// A field that is flat, or that only falls away from a texel, occludes
/// nothing: a pit darkens its floor and not the ground around it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct OcclusionFromHeight {
    /// The height field.
    pub height: Input,
    /// How far to look for a horizon, in UV.
    pub radius: f32,
    /// How dark the result is allowed to go.
    pub strength: f32,
}

impl Default for OcclusionFromHeight {
    fn default() -> Self {
        Self {
            height: Input::default(),
            radius: 0.02,
            strength: 1.0,
        }
    }
}

impl OcclusionFromHeight {
    /// Occlude from a height field.
    pub fn new(height: impl Into<Input>) -> Self {
        Self {
            height: height.into(),
            ..Self::default()
        }
    }

    /// Set how far to look for a horizon.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    /// Set how dark the result may go.
    pub fn strength(mut self, strength: f32) -> Self {
        self.strength = strength;
        self
    }
}

ports!(
    OcclusionFromHeight,
    Output::Fixed(ValueType::Float),
    "height": Float => height,
);

impl Check for OcclusionFromHeight {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        check_unit(self.radius, "radius", path)?;
        check_unit(self.strength, "strength", path)
    }
}

/// Wrapped distance from a mask, by jump flood.
///
/// The plane holds the distance in UV units to the nearest texel at or above
/// [`Self::threshold`], measured the short way around the wrap; the node
/// presents that distance divided by [`Self::range`] and held at one, so the
/// output is the `0..=1` field a mask wants and `range` is the distance that
/// reaches white. A mask with nothing in it is one everywhere.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Distance {
    /// The mask to measure from.
    pub input: Input,
    /// What counts as inside the mask.
    pub threshold: f32,
    /// The distance in UV that becomes one in the result.
    pub range: f32,
}

impl Default for Distance {
    fn default() -> Self {
        Self {
            input: Input::default(),
            threshold: 0.5,
            range: 0.1,
        }
    }
}

impl Distance {
    /// Measure distance from a mask.
    pub fn new(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            ..Self::default()
        }
    }

    /// Set what counts as inside.
    pub fn threshold(mut self, threshold: f32) -> Self {
        self.threshold = threshold;
        self
    }

    /// Set the distance that becomes one.
    pub fn range(mut self, range: f32) -> Self {
        self.range = range;
        self
    }
}

ports!(
    Distance,
    Output::Fixed(ValueType::Float),
    "input": Float => input,
);

impl Check for Distance {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        finite(self.threshold, &format!("{path}.threshold"), "threshold")?;
        check_unit(self.range, "range", path)?;
        require(
            self.range > 0.0,
            &format!("{path}.range"),
            "range must be positive",
        )
    }
}

/// Shrinks the bright parts of a mask.
///
/// The smallest value within [`Self::radius`], over the square neighbourhood of
/// that reach rather than the disc: the square is separable, so the cost is
/// linear in the radius, and an [`Erode`] followed by a [`Dilate`] of the same
/// radius gives an axis-aligned mask back unchanged where a disc would round
/// its corners. A diagonal edge moves by a little more than the radius.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Erode {
    /// The field to shrink.
    pub input: Input,
    /// Radius in UV.
    pub radius: f32,
}

impl Default for Erode {
    fn default() -> Self {
        Self {
            input: Input::default(),
            radius: 0.01,
        }
    }
}

impl Erode {
    /// Shrink a mask.
    pub fn new(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            ..Self::default()
        }
    }

    /// Set the radius in UV.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }
}

ports!(Erode, Output::SameAs("input"), "input": Field => input);

impl Check for Erode {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        check_unit(self.radius, "radius", path)
    }
}

/// Grows the bright parts of a mask.
///
/// The largest value within [`Self::radius`], over the same square
/// neighbourhood [`Erode`] describes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Dilate {
    /// The field to grow.
    pub input: Input,
    /// Radius in UV.
    pub radius: f32,
}

impl Default for Dilate {
    fn default() -> Self {
        Self {
            input: Input::default(),
            radius: 0.01,
        }
    }
}

impl Dilate {
    /// Grow a mask.
    pub fn new(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            ..Self::default()
        }
    }

    /// Set the radius in UV.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }
}

ports!(Dilate, Output::SameAs("input"), "input": Field => input);

impl Check for Dilate {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        check_unit(self.radius, "radius", path)
    }
}

/// An explicit cache point with no filter.
///
/// Everything upstream is rasterised once into a plane, which is how a
/// sub-expression that three nodes read is paid for once, and how a graph pins
/// the resolution a buffered filter runs at.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct Buffer {
    /// The field to rasterise.
    pub input: Input,
    /// Edge length of the plane in texels; the bake's own resolution if absent.
    pub resolution: Option<u32>,
}

impl Buffer {
    /// Rasterise a field at the bake's resolution.
    pub fn new(input: impl Into<Input>) -> Self {
        Self {
            input: input.into(),
            resolution: None,
        }
    }

    /// Pin the plane's resolution.
    pub fn resolution(mut self, texels: u32) -> Self {
        self.resolution = Some(texels);
        self
    }
}

ports!(Buffer, Output::SameAs("input"), "input": Any => input);

impl Check for Buffer {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        let Some(resolution) = self.resolution else {
            return Ok(());
        };
        require(
            resolution.is_power_of_two() && (16..=4096).contains(&resolution),
            &format!("{path}.resolution"),
            "buffer resolution must be a power of two in 16..=4096",
        )
    }
}

/// Which field of a strand layer a [`StrandRelief`] reads back off the plane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum StrandReliefOutput {
    /// How much of the texel the strands cover, antialiased, in `0..=1`.
    #[default]
    Coverage,
    /// How far the topmost strand over the texel stands off the surface, in
    /// units of the layer's `length_metres`: one is a full-length strand
    /// standing straight up, and zero is bare surface.
    Height,
    /// A hash of the topmost strand, in `0..=1`. The per-strand id a graph
    /// tints or roughens by, and the reason no generator needs a "strand id"
    /// output of its own.
    Id,
    /// How far along the topmost strand the texel is: zero at the root and one
    /// at the tip.
    Along,
    /// The topmost strand's own colour at that point, which is the gradient the
    /// geometry carries as a vertex colour, root occlusion and all.
    Color,
    /// How much strand stands over the texel, *accumulated* and unsaturated.
    ///
    /// [`Self::Coverage`] is a union and stops at one, which is the right
    /// answer to "is there a blade here" and the wrong one to "how much grass
    /// is here": two blades crossing read the same as one. This is the sum
    /// instead, so it keeps rising with every blade, and it is what a graph
    /// builds the *pile* out of — the mat of shorter and fallen blades that
    /// fills the ground between the standing ones.
    ///
    /// Blurring the coverage is the same idea hand-rolled and the wrong shape,
    /// because a blur spreads the mass sideways instead of summing it. Ten
    /// overlapping blades read ten here, so a graph almost always wants a
    /// [`Levels`](crate::nodes::Levels) over it.
    Mass,
    /// The same accumulation, weighted by how high each contribution stood.
    ///
    /// How much grass is over the texel *and* how far above it — so a texel
    /// down in the pile with blades crossing over it reads high, and one under
    /// a single blade lying on bare ground reads low. Inverted, it is the dark
    /// between the blades, which is where a real lawn's shadow is; the
    /// alternative is darkening the blade itself, which puts the shadow on the
    /// brightest thing in the frame.
    ///
    /// Unsaturated, like [`Self::Mass`], and in units of the layer's own
    /// `length_metres`, like [`Self::Height`].
    Occlusion,
}

impl StrandReliefOutput {
    /// A stable number for the cache key, written out as every other code is.
    pub(crate) fn code(self) -> u32 {
        match self {
            Self::Coverage => 0,
            Self::Height => 1,
            Self::Id => 2,
            Self::Along => 3,
            Self::Color => 4,
            Self::Mass => 5,
            Self::Occlusion => 6,
        }
    }

    /// What a plane of this output holds.
    pub(crate) fn value_type(self) -> ValueType {
        match self {
            Self::Color => ValueType::Color,
            _ => ValueType::Float,
        }
    }
}

/// The strands of one of this graph's own layers, seen from directly above.
///
/// The node that closes the loop between the two halves of a strand material.
/// A [`StrandLayer`](crate::StrandLayer) is scattered into geometry, and this
/// splats *the same* scatter — the same roots, the same lengths, the same
/// colours — back into a plane the PBR half reads. So the relief the camera
/// sees once the blades have faded out is the blades, rather than a second
/// drawing of them that has to be kept in step by hand.
///
/// It is buffered rather than pointwise because a strand is long: one crosses
/// many lattice cells, so a per-texel answer would have to search its
/// neighbours and re-evaluate every field once per neighbour. Splatting each
/// strand's footprint once into a plane is the same answer for a bounded cost,
/// and it is wrapped at the seam like every other plane in
/// [`planes`](crate::planes).
///
/// # Metres, and why the node has to be told
///
/// A strand is authored in metres and a plane is addressed in UV, and a graph
/// does not know how large a repeat of it is: the same graph dresses a
/// two-metre flagstone and a half-metre one, and `tile_metres` is a fact about
/// the *material definition* rather than about the graph. So
/// [`Self::repeat_metres`] is the conversion, declared here, and a relief
/// authored for the wrong repeat draws blades of the wrong size rather than
/// silently of the right one.
///
/// # What is refused
///
/// A strand field that reaches a `StrandRelief` is refused by path when the
/// layer is lowered. A relief is built *from* the layer's fields, so a field
/// that read one would be asking for itself; the refusal is made where the
/// cycle is, at the node inside the field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[must_use]
pub struct StrandRelief {
    /// The layer of *this* graph to splat, by the name it is declared under.
    pub layer: String,
    /// Which field of it to read back.
    pub output: StrandReliefOutput,
    /// How many metres one repeat of the material covers.
    ///
    /// What turns the layer's metres into UV. Square repeats only: a
    /// non-square one would make a strand's footprint an ellipse and its length
    /// depend on which way it leans, which is not a lawn anybody authored.
    pub repeat_metres: f32,
}

impl Default for StrandRelief {
    fn default() -> Self {
        Self {
            layer: String::new(),
            output: StrandReliefOutput::default(),
            repeat_metres: 1.0,
        }
    }
}

impl StrandRelief {
    /// The coverage of one of this graph's strand layers, over a repeat of
    /// `repeat_metres` metres.
    pub fn new(layer: impl Into<String>, repeat_metres: f32) -> Self {
        Self {
            layer: layer.into(),
            repeat_metres,
            ..Self::default()
        }
    }

    /// Read a different field of the layer back.
    pub fn output(mut self, output: StrandReliefOutput) -> Self {
        self.output = output;
        self
    }
}

ports!(StrandRelief, |node| Output::Fixed(node.output.value_type()),);

impl Check for StrandRelief {
    fn check(&self, path: &str) -> Result<(), GraphError> {
        require(
            !self.layer.is_empty(),
            &format!("{path}.layer"),
            "a strand relief names the layer it splats",
        )?;
        require(
            self.repeat_metres.is_finite() && self.repeat_metres > 0.0,
            &format!("{path}.repeat_metres"),
            "repeat_metres is how many metres one repeat covers, and must be finite and positive",
        )
    }
}

impl Lower for StrandRelief {
    fn lower(&self, cx: &mut Lowering, _inputs: &NodeInputs) -> ValueId {
        cx.reject(
            "a strand relief is splatted by the lowering driver, which is where the layer it \
             names is resolved; this backend lowered the node on its own instead",
        )
    }
}

/// Rasterise an input into a plane, run a filter over it, and read it back at
/// the texel's own coordinate.
///
/// Every buffered node is this and then whatever it does with the answer. The
/// plane is sampled at the coordinate the texel is being evaluated at, which at
/// the resolution the plane was rasterised at is that texel exactly, so a
/// buffered filter costs one plane and not one lookup's worth of blur.
fn buffered(cx: &mut Lowering, input: ValueId, filter: Filter, resolution: Option<u32>) -> ValueId {
    let buffer = cx.buffer(input, filter, resolution);
    let uv = cx.uv();
    cx.sample(buffer, uv)
}

impl Lower for Blur {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let input = inputs.value("input");
        let filter = match self.kind {
            BlurKind::Gaussian => Filter::Blur {
                radius: self.radius,
            },
            BlurKind::Directional => Filter::Directional {
                radius: self.radius,
                radians: self.angle.to_radians(),
            },
            // The one filter that reads two planes: the source it smears and
            // the height it walks down. The height is a plane of its own, with
            // no filter over it, and the slope filter names it — which is why
            // a `Filter` can carry a `BufferId` at all.
            BlurKind::Slope => {
                let guide = cx.buffer(inputs.value("height"), Filter::None, None);
                Filter::Slope {
                    radius: self.radius,
                    steps: self.steps,
                    mode: self.slope_mode,
                    guide,
                }
            }
        };
        buffered(cx, input, filter, None)
    }
}

/// The curvature plane, and then the side of it the node presents.
///
/// The gain is applied here rather than in the filter, per texel, for the
/// reason [`Distance`]'s range is: two nodes that read the same height at the
/// same radius share one plane however differently they present it, so a
/// cavity and a peaks mask over one surface cost one plane and two multiplies.
impl Lower for Curvature {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let curvature = buffered(
            cx,
            inputs.value("height"),
            Filter::Curvature {
                radius: self.radius,
                kind: self.kind,
            },
            None,
        );
        let gain = cx.constant(match self.output {
            // A hollow is a negative curvature, so the mask that finds one is
            // the field turned over rather than a second filter.
            CurvatureOutput::Cavity => -self.strength,
            CurvatureOutput::Signed | CurvatureOutput::Peaks => self.strength,
        });
        let scaled = cx.emit(Op::Mul, [curvature, gain]);
        let zero = cx.constant(0.0);
        let one = cx.constant(1.0);
        match self.output {
            // Centred on a half, because a signed field that a texture has to
            // carry has nowhere else to put its zero.
            CurvatureOutput::Signed => {
                let half = cx.constant(0.5);
                cx.emit(Op::Add, [scaled, half])
            }
            CurvatureOutput::Peaks | CurvatureOutput::Cavity => {
                cx.emit(Op::Clamp, [scaled, zero, one])
            }
        }
    }
}

impl Lower for EdgeDetect {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let edge = buffered(
            cx,
            inputs.value("input"),
            Filter::Edge {
                radius: self.radius,
            },
            None,
        );
        let strength = cx.constant(self.strength);
        let scaled = cx.emit(Op::Mul, [edge, strength]);
        let zero = cx.constant(0.0);
        let one = cx.constant(1.0);
        cx.emit(Op::Clamp, [scaled, zero, one])
    }
}

impl Lower for OcclusionFromHeight {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        buffered(
            cx,
            inputs.value("height"),
            Filter::Occlusion {
                radius: self.radius,
                strength: self.strength,
            },
            None,
        )
    }
}

impl Lower for Distance {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        let distance = buffered(
            cx,
            inputs.value("input"),
            Filter::Distance {
                threshold: self.threshold,
            },
            None,
        );
        // The plane holds a distance in UV, which is the honest thing for a
        // plane to hold and what a second node reading the same mask shares.
        // The range is applied here, per texel, where it costs one divide and
        // does not make a plane of its own.
        let range = cx.constant(self.range);
        let scaled = cx.emit(Op::Div, [distance, range]);
        let one = cx.constant(1.0);
        cx.emit(Op::Min, [scaled, one])
    }
}

impl Lower for Erode {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        buffered(
            cx,
            inputs.value("input"),
            Filter::Erode {
                radius: self.radius,
            },
            None,
        )
    }
}

impl Lower for Dilate {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        buffered(
            cx,
            inputs.value("input"),
            Filter::Dilate {
                radius: self.radius,
            },
            None,
        )
    }
}

impl Lower for Buffer {
    fn lower(&self, cx: &mut Lowering, inputs: &NodeInputs) -> ValueId {
        buffered(cx, inputs.value("input"), Filter::None, self.resolution)
    }
}
