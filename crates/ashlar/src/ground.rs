//! Pure fitting against a triangulated height patch, independent of world sampling.
use crate::{ConvexSolid, Pose, Socket};
use glam::{DQuat, DVec2, DVec3};
use serde::{Deserialize, Serialize};

/// Axis-aligned horizontal bounds in the frame named by their consumer.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroundRect {
    /// Minimum X/Z coordinates.
    pub min: DVec2,
    /// Maximum X/Z coordinates.
    pub max: DVec2,
}
impl GroundRect {
    /// Construct horizontal bounds.
    pub fn new(min: [f64; 2], max: [f64; 2]) -> Self {
        Self {
            min: min.into(),
            max: max.into(),
        }
    }
    fn valid(self) -> bool {
        self.min.is_finite() && self.max.is_finite() && self.max.cmpgt(self.min).all()
    }
    /// Rectangle corners, counterclockwise in X/Z.
    pub fn corners(self) -> [DVec2; 4] {
        [
            self.min,
            DVec2::new(self.max.x, self.min.y),
            self.max,
            DVec2::new(self.min.x, self.max.y),
        ]
    }
}

/// Placement failures are explicit; unsuitable sites are never silently forced.
///
/// Non-exhaustive: a caller reports or retries, and a new failure mode must not
/// break the match.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum GroundError {
    /// Invalid authored parameters or non-finite sampled heights.
    #[error("invalid grounding input: {0}")]
    Invalid(String),
    /// The supplied patch does not cover the footprint or approach search.
    #[error("terrain patch does not cover the requested region")]
    OutsidePatch,
    /// Foundations would exceed the configured vertical limit.
    #[error("required support depth {required:.3} exceeds limit {limit:.3}")]
    SupportDepth {
        /// Required metres.
        required: f64,
        /// Configured metres.
        limit: f64,
    },
    /// A straight, grade-limited approach cannot reach this threshold.
    #[error("no traversable approach within the configured length and grade")]
    NoApproach,
}

/// Which corner pair each grid cell is split along. A patch must be triangulated
/// the same way the collision it stands for is, or a building sits on a surface
/// the player does not walk on.
///
/// Non-exhaustive: another terrain may split cells by a rule of its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Diagonal {
    /// Split from the low-X high-Z corner to the high-X low-Z corner. The
    /// default, and what the terrain this solver grew against uses.
    #[default]
    Anti,
    /// Split from the low-X low-Z corner to the high-X high-Z corner.
    Main,
}

/// A checked row-major height patch and the triangulation it is read under.
#[derive(Clone, Debug)]
pub struct TerrainPatch {
    origin: DVec2,
    step: f64,
    size: [usize; 2],
    heights: Vec<f64>,
    diagonal: Diagonal,
}
impl TerrainPatch {
    /// Samples are absolute heights; `size` counts vertices, X fastest. Cells
    /// are split along [`Diagonal::Anti`].
    pub fn new(
        origin: [f64; 2],
        step: f64,
        size: [usize; 2],
        heights: Vec<f64>,
    ) -> Result<Self, GroundError> {
        Self::with_diagonal(origin, step, size, heights, Diagonal::Anti)
    }

    /// The triangulation the caller's own collision uses.
    pub fn diagonal(&self) -> Diagonal {
        self.diagonal
    }

    /// Samples as in [`Self::new`], split along the given diagonal.
    pub fn with_diagonal(
        origin: [f64; 2],
        step: f64,
        size: [usize; 2],
        heights: Vec<f64>,
        diagonal: Diagonal,
    ) -> Result<Self, GroundError> {
        if !DVec2::from(origin).is_finite()
            || !step.is_finite()
            || step <= 0.0
            || size.iter().any(|v| !(2..=1025).contains(v))
            || heights.len() != size[0] * size[1]
            || heights.iter().any(|v| !v.is_finite())
        {
            return Err(GroundError::Invalid(
                "terrain patch dimensions or heights".into(),
            ));
        }
        Ok(Self {
            origin: origin.into(),
            step,
            size,
            heights,
            diagonal,
        })
    }

    // Grid coordinates are checked and bounded to 1025 before integer conversion.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    fn vertices(&self, rect: GroundRect, pose: Pose) -> Result<Vec<DVec3>, GroundError> {
        let corners = rect
            .corners()
            .map(|p| pose.transform_point(DVec3::new(p.x, 0.0, p.y)));
        let min = corners.iter().fold(DVec2::splat(f64::INFINITY), |a, p| {
            a.min(DVec2::new(p.x, p.z))
        });
        let max = corners
            .iter()
            .fold(DVec2::splat(f64::NEG_INFINITY), |a, p| {
                a.max(DVec2::new(p.x, p.z))
            });
        let low = (min - self.origin) / self.step;
        let high = (max - self.origin) / self.step;
        if low.min_element() < -1e-7
            || high.x > (self.size[0] - 1) as f64 + 1e-7
            || high.y > (self.size[1] - 1) as f64 + 1e-7
        {
            return Err(GroundError::OutsidePatch);
        }
        let inverse = pose.rotation.inverse();
        let vertex = |x: usize, z: usize| {
            inverse
                * (DVec3::new(
                    self.origin.x + x as f64 * self.step,
                    self.heights[z * self.size[0] + x],
                    self.origin.y + z as f64 * self.step,
                ) - pose.translation)
        };
        let mut result = Vec::new();
        for z in (low.y.floor().max(0.0) as usize)..(high.y.ceil() as usize).min(self.size[1] - 1) {
            for x in
                (low.x.floor().max(0.0) as usize)..(high.x.ceil() as usize).min(self.size[0] - 1)
            {
                let [a, b, c, d] = [
                    vertex(x, z),
                    vertex(x + 1, z),
                    vertex(x, z + 1),
                    vertex(x + 1, z + 1),
                ];
                let split = match self.diagonal {
                    Diagonal::Anti => [[a, b, c], [d, c, b]],
                    Diagonal::Main => [[a, b, d], [a, d, c]],
                };
                for triangle in split {
                    let mut polygon = triangle.to_vec();
                    for (axis, bound, direction) in [
                        (0, rect.min.x, 1.0),
                        (0, rect.max.x, -1.0),
                        (2, rect.min.y, 1.0),
                        (2, rect.max.y, -1.0),
                    ] {
                        polygon = clip(&polygon, axis, bound, direction);
                    }
                    result.extend(polygon);
                }
            }
        }
        if result.is_empty() {
            return Err(GroundError::OutsidePatch);
        }
        Ok(result)
    }
    /// Exact extrema of the piecewise-linear ground clipped to a yawed rectangle.
    pub fn range(&self, rect: GroundRect, pose: Pose) -> Result<[f64; 2], GroundError> {
        if !rect.valid() || !yaw_pose(pose) {
            return Err(GroundError::Invalid("sample rectangle or pose".into()));
        }
        let points = self.vertices(rect, pose)?;
        Ok(points
            .iter()
            .fold([f64::INFINITY, f64::NEG_INFINITY], |[lo, hi], p| {
                [
                    lo.min(p.y + pose.translation.y),
                    hi.max(p.y + pose.translation.y),
                ]
            }))
    }
}

fn clip(points: &[DVec3], axis: usize, bound: f64, direction: f64) -> Vec<DVec3> {
    let mut output = Vec::new();
    if points.is_empty() {
        return output;
    }
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        let da = (a[axis] - bound) * direction;
        let db = (b[axis] - bound) * direction;
        if da >= 0.0 {
            output.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            output.push(a.lerp(b, da / (da - db)));
        }
    }
    output
}
fn yaw_pose(pose: Pose) -> bool {
    pose.translation.is_finite()
        && pose.rotation.is_finite()
        && (pose.rotation.length_squared() - 1.0).abs() < 1e-9
        && (pose.rotation * DVec3::Y - DVec3::Y).length() < 1e-9
}

/// Grounding metadata belongs to content, not inferred from render bounds.
#[derive(Clone, Debug)]
pub struct GroundingSpec {
    /// Load-bearing footprint in building space.
    pub footprint: GroundRect,
    /// Walking surface elevation in building space.
    pub floor: f64,
    /// Bottom of the retained slab in building space.
    pub slab_bottom: f64,
    /// Threshold frame: +Z points outward and translation.y is walking height.
    pub entrance: Socket,
}
/// Site fitting limits; no material or architectural style assumptions.
#[derive(Clone, Copy, Debug)]
pub struct GroundingPolicy {
    /// Floor clearance above highest occupied ground.
    pub clearance: f64,
    /// Burial below minimum ground beneath each support.
    pub embed: f64,
    /// Maximum vertical support depth.
    pub max_depth: f64,
    /// Thickness of the perimeter support wall.
    pub support_width: f64,
    /// Maximum length of one support segment.
    pub max_span: f64,
    /// Landing length outward from the threshold.
    pub landing_length: f64,
    /// Clear width of landing and ramp.
    pub ramp_width: f64,
    /// Maximum rise/run, in either direction.
    pub max_grade: f64,
    /// Maximum straight ramp length beyond the landing.
    pub max_ramp_length: f64,
    /// Search step and minimum ramp length.
    pub search_step: f64,
    /// Extra vegetation clearance around building and approach bounds.
    pub vegetation_margin: f64,
    /// Largest tolerated ground-to-toe step across the full ramp width.
    pub max_toe_step: f64,
}

/// Eight-corner convex prism, shared by rendering and static collision adapters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroundSolid {
    /// Bottom ring then top ring, each in increasing X/Z rectangle order.
    pub vertices: [DVec3; 8],
}
impl GroundSolid {
    /// Box with a minimum-corner origin.
    pub fn cuboid(size: [f64; 3], pose: Pose) -> Self {
        let mut solid = Self::prism(
            GroundRect::new([0.0, 0.0], [size[0], size[2]]),
            0.0,
            size[1],
            size[1],
        );
        for point in &mut solid.vertices {
            *point = pose.transform_point(*point);
        }
        solid
    }
    fn prism(rect: GroundRect, bottom: f64, near: f64, far: f64) -> Self {
        let corners = rect.corners();
        Self {
            vertices: std::array::from_fn(|i| {
                let p = corners[i % 4];
                DVec3::new(
                    p.x,
                    if i < 4 {
                        bottom
                    } else if i % 4 < 2 {
                        near
                    } else {
                        far
                    },
                    p.y,
                )
            }),
        }
    }
    /// Outward-wound triangles for this prism, with identical collision vertices.
    pub fn triangles(&self) -> impl Iterator<Item = [DVec3; 3]> + '_ {
        [
            [0, 1, 2],
            [0, 2, 3],
            [4, 7, 6],
            [4, 6, 5],
            [0, 4, 5],
            [0, 5, 1],
            [1, 5, 6],
            [1, 6, 2],
            [2, 6, 7],
            [2, 7, 3],
            [3, 7, 4],
            [3, 4, 0],
        ]
        .into_iter()
        .map(|ids| ids.map(|i| self.vertices[i]))
    }
    /// Apply a rigid placement without changing shape.
    #[must_use]
    pub fn placed(mut self, pose: Pose) -> Self {
        for p in &mut self.vertices {
            *p = pose.transform_point(*p);
        }
        self
    }
}

impl From<&GroundSolid> for ConvexSolid {
    /// A fitted prism is already convex, so the collision proxies a game builds
    /// out of a grounding and out of its elements are one kind of thing.
    fn from(solid: &GroundSolid) -> Self {
        Self {
            vertices: solid.vertices.to_vec(),
        }
    }
}

/// Immutable fitting output consumed by rendering, collision and vegetation.
#[derive(Clone, Debug, PartialEq)]
pub struct Grounding {
    /// World origin of the level building; rotation is around Y only.
    pub pose: Pose,
    /// Site-specific supports, then landing and ramp, in building-local space.
    pub solids: Vec<GroundSolid>,
    /// World-space conservative occupancy rectangles, including access.
    pub exclusions: Vec<GroundRect>,
    /// Ground extrema under the support footprint.
    pub terrain_range: [f64; 2],
    /// Selected ramp length in metres.
    pub ramp_length: f64,
    /// Absolute selected rise/run.
    pub ramp_grade: f64,
    /// World-space position at the ramp toe's centre, on its walking surface.
    pub toe: DVec3,
}

/// Fit a level building, segmented foundations and a straight access ramp.
///
/// The solver is pure: it reads the height patch the caller gathered and never
/// samples a world, chooses a style or creates a renderer or physics handle.
/// Unsuitable sites return a typed [`GroundError`] rather than being forced.
///
/// ```
/// use ashlar::{
///     GroundRect, GroundingPolicy, GroundingSpec, Pose, Socket, TerrainPatch, fit_ground,
///     glam::DQuat,
/// };
///
/// // A level 81 by 81 metre patch at ten metres above sea level.
/// let terrain = TerrainPatch::new([-40.0, -40.0], 1.0, [81, 81], vec![10.0; 81 * 81])?;
/// let spec = GroundingSpec {
///     footprint: GroundRect::new([-4.0, 0.0], [4.0, 6.0]),
///     floor: 0.0,
///     slab_bottom: -0.3,
///     entrance: Socket::new(
///         "entrance",
///         Pose::default().rotated(DQuat::from_rotation_y(std::f64::consts::PI)),
///     ),
/// };
/// let policy = GroundingPolicy {
///     clearance: 0.4,
///     embed: 0.3,
///     max_depth: 5.0,
///     support_width: 0.3,
///     max_span: 2.0,
///     landing_length: 1.5,
///     ramp_width: 2.0,
///     max_grade: 0.3,
///     max_ramp_length: 18.0,
///     search_step: 0.5,
///     vegetation_margin: 0.75,
///     max_toe_step: 0.2,
/// };
///
/// let site = fit_ground(&spec, policy, &terrain, [0.0, 0.0], 0.0)?;
/// // The floor sits one clearance above the highest ground under the footprint.
/// assert!((site.pose.translation.y - 10.4).abs() < 1e-9);
/// assert!(site.ramp_grade <= policy.max_grade);
/// assert!(!site.solids.is_empty(), "perimeter supports, landing and ramp");
/// # Ok::<(), ashlar::GroundError>(())
/// ```
pub fn fit_ground(
    spec: &GroundingSpec,
    policy: GroundingPolicy,
    terrain: &TerrainPatch,
    at: [f64; 2],
    yaw: f64,
) -> Result<Grounding, GroundError> {
    validate(spec, policy, at, yaw)?;
    let rotation = DQuat::from_rotation_y(yaw);
    let horizontal = Pose::at([at[0], 0.0, at[1]]).rotated(rotation);
    let terrain_range = terrain.range(spec.footprint, horizontal)?;
    let elevation = terrain_range[1] + policy.clearance - spec.floor;
    let pose = Pose::at([at[0], elevation, at[1]]).rotated(rotation);
    let mut solids = Vec::new();
    let mut rects = Vec::new();
    let r = spec.footprint;
    let w = policy.support_width;
    for rect in [
        GroundRect::new(r.min.to_array(), [r.max.x, r.min.y + w]),
        GroundRect::new([r.min.x, r.max.y - w], r.max.to_array()),
        GroundRect::new([r.min.x, r.min.y + w], [r.min.x + w, r.max.y - w]),
        GroundRect::new([r.max.x - w, r.min.y + w], [r.max.x, r.max.y - w]),
    ] {
        let axis = usize::from((rect.max - rect.min).x <= (rect.max - rect.min).y);
        let mut start = rect.min[axis];
        while start < rect.max[axis] - 1e-9 {
            let end = (start + policy.max_span).min(rect.max[axis]);
            let mut segment = rect;
            segment.min[axis] = start;
            segment.max[axis] = end;
            let range = terrain.range(segment, horizontal)?;
            let bottom = (range[0] - policy.embed - elevation).min(spec.slab_bottom - 0.01);
            check_depth(spec.slab_bottom - bottom, policy)?;
            solids.push(GroundSolid::prism(
                segment,
                bottom,
                spec.slab_bottom,
                spec.slab_bottom,
            ));
            start = end;
        }
    }
    rects.push(world_bounds(r, pose));
    let entrance = Pose {
        translation: pose.transform_point(spec.entrance.pose.translation),
        rotation: rotation * spec.entrance.pose.rotation,
    };
    let landing = GroundRect::new(
        [-policy.ramp_width / 2.0, 0.0],
        [policy.ramp_width / 2.0, policy.landing_length],
    );
    let (length, end_height) = approach(terrain, entrance, policy)?;
    let ramp = GroundRect::new(
        [-policy.ramp_width / 2.0, policy.landing_length],
        [policy.ramp_width / 2.0, policy.landing_length + length],
    );
    for (rect, near, far) in [(landing, 0.0_f64, 0.0), (ramp, 0.0, end_height)] {
        let range = terrain.range(rect, entrance)?;
        let bottom = range[0] - policy.embed - entrance.translation.y;
        check_depth(near.max(far) - bottom, policy)?;
        let local = Pose {
            translation: pose.rotation.inverse() * (entrance.translation - pose.translation),
            rotation: spec.entrance.pose.rotation,
        };
        solids.push(GroundSolid::prism(rect, bottom, near, far).placed(local));
        rects.push(world_bounds(rect, entrance));
    }
    for rect in &mut rects {
        rect.min -= DVec2::splat(policy.vegetation_margin);
        rect.max += DVec2::splat(policy.vegetation_margin);
    }
    Ok(Grounding {
        pose,
        solids,
        exclusions: rects,
        terrain_range,
        ramp_length: length,
        ramp_grade: end_height.abs() / length,
        toe: entrance.transform_point(DVec3::new(0.0, end_height, policy.landing_length + length)),
    })
}

fn check_depth(required: f64, policy: GroundingPolicy) -> Result<(), GroundError> {
    if required > policy.max_depth {
        Err(GroundError::SupportDepth {
            required,
            limit: policy.max_depth,
        })
    } else {
        Ok(())
    }
}
fn world_bounds(rect: GroundRect, pose: Pose) -> GroundRect {
    let points = rect
        .corners()
        .map(|p| pose.transform_point(DVec3::new(p.x, 0.0, p.y)));
    GroundRect {
        min: points.iter().fold(DVec2::splat(f64::INFINITY), |a, p| {
            a.min(DVec2::new(p.x, p.z))
        }),
        max: points.iter().fold(DVec2::splat(f64::NEG_INFINITY), |a, p| {
            a.max(DVec2::new(p.x, p.z))
        }),
    }
}
fn approach(
    terrain: &TerrainPatch,
    entrance: Pose,
    p: GroundingPolicy,
) -> Result<(f64, f64), GroundError> {
    let mut length = p.search_step;
    while length <= p.max_ramp_length + 1e-9 {
        let end = p.landing_length + length;
        let toe = GroundRect::new(
            [-p.ramp_width / 2.0, end - 0.05],
            [p.ramp_width / 2.0, end + 0.05],
        );
        let range = terrain.range(toe, entrance)?;
        let height = range[1] + 0.015 - entrance.translation.y;
        if range[1] - range[0] + 0.015 <= p.max_toe_step && height.abs() / length <= p.max_grade {
            let mut clear = true;
            for (start, finish) in [(0.0, p.landing_length), (p.landing_length, end)] {
                let rect =
                    GroundRect::new([-p.ramp_width / 2.0, start], [p.ramp_width / 2.0, finish]);
                clear &= terrain.vertices(rect, entrance)?.iter().all(|v| {
                    let top = height * ((v.z - p.landing_length) / length).clamp(0.0, 1.0);
                    v.y <= top + 1e-8
                });
            }
            if clear {
                return Ok((length, height));
            }
        }
        length += p.search_step;
    }
    Err(GroundError::NoApproach)
}
fn validate(
    s: &GroundingSpec,
    p: GroundingPolicy,
    at: [f64; 2],
    yaw: f64,
) -> Result<(), GroundError> {
    let positive = [
        p.clearance,
        p.embed,
        p.max_depth,
        p.support_width,
        p.max_span,
        p.landing_length,
        p.ramp_width,
        p.max_grade,
        p.max_ramp_length,
        p.search_step,
        p.max_toe_step,
        p.vegetation_margin,
    ];
    if !s.footprint.valid()
        || !s.floor.is_finite()
        || !s.slab_bottom.is_finite()
        || s.slab_bottom >= s.floor
        || !yaw_pose(s.entrance.pose)
        || !yaw.is_finite()
        || !DVec2::from(at).is_finite()
        || positive.iter().any(|v| !v.is_finite() || *v <= 0.0)
        || p.support_width * 2.0 >= (s.footprint.max - s.footprint.min).min_element()
        || p.max_span < 0.1
        || p.max_ramp_length / p.search_step > 1024.0
        || (s.footprint.max - s.footprint.min).max_element() / p.max_span > 1024.0
    {
        return Err(GroundError::Invalid(
            "footprint, entrance or fitting policy".into(),
        ));
    }
    Ok(())
}
