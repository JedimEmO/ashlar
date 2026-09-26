use std::collections::BTreeSet;
use std::f64::consts::{PI, TAU};

use glam::{DQuat, DVec3};
use serde::{Deserialize, Serialize};

use crate::{ValidationError, name, require};

/// The eight corners of an axis-aligned box, in no particular order.
pub(crate) fn corners([min, max]: [DVec3; 2]) -> [DVec3; 8] {
    [
        DVec3::new(min.x, min.y, min.z),
        DVec3::new(max.x, min.y, min.z),
        DVec3::new(min.x, max.y, min.z),
        DVec3::new(max.x, max.y, min.z),
        DVec3::new(min.x, min.y, max.z),
        DVec3::new(max.x, min.y, max.z),
        DVec3::new(min.x, max.y, max.z),
        DVec3::new(max.x, max.y, max.z),
    ]
}

/// A box around `points`, or `None` when there are none.
pub(crate) fn boxed(points: impl Iterator<Item = DVec3>) -> Option<[DVec3; 2]> {
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    let mut seen = false;
    for point in points {
        seen = true;
        min = min.min(point);
        max = max.max(point);
    }
    seen.then_some([min, max])
}

/// Rigid local placement. Dimensions belong to geometry, not hidden scale.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct Pose {
    /// Translation in metres in the parent's coordinate system.
    pub translation: DVec3,
    /// Unit quaternion rotating local axes into the parent frame.
    pub rotation: DQuat,
}

impl Default for Pose {
    fn default() -> Self {
        Self {
            translation: DVec3::ZERO,
            rotation: DQuat::IDENTITY,
        }
    }
}

impl Pose {
    /// Place an object without rotating its axes.
    pub fn at(translation: [f64; 3]) -> Self {
        Self {
            translation: translation.into(),
            ..Self::default()
        }
    }

    /// Set an absolute local rotation. Validation rejects non-unit quaternions.
    pub fn rotated(mut self, rotation: DQuat) -> Self {
        self.rotation = rotation;
        self
    }

    /// Convert a point from this local frame to its parent's frame.
    pub fn transform_point(self, point: DVec3) -> DVec3 {
        self.translation + self.rotation * point
    }

    /// Express a pose given in this frame in this frame's parent instead.
    /// Composition is `self` then `local`, the same order as nesting geometry.
    pub fn compose(self, local: Self) -> Self {
        Self {
            translation: self.transform_point(local.translation),
            rotation: self.rotation * local.rotation,
        }
    }

    /// The placement that undoes this one. Poses are rigid, so this is exact
    /// apart from rounding.
    pub fn inverse(self) -> Self {
        let rotation = self.rotation.inverse();
        Self {
            translation: rotation * -self.translation,
            rotation,
        }
    }

    pub(crate) fn validate(self, path: &str) -> Result<(), ValidationError> {
        require(
            self.translation.is_finite(),
            path,
            "translation must be finite",
        )?;
        require(
            self.rotation.is_finite() && (self.rotation.length_squared() - 1.0).abs() < 1e-9,
            path,
            "rotation must be a finite unit quaternion",
        )
    }
}

/// One coordinate axis of a local frame. Right-handed and Y-up, as everywhere else.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Axis {
    /// Local X.
    X,
    /// Local Y, the up axis.
    #[default]
    Y,
    /// Local Z.
    Z,
}

impl Axis {
    /// The unit vector along this axis.
    pub fn unit(self) -> DVec3 {
        match self {
            Self::X => DVec3::X,
            Self::Y => DVec3::Y,
            Self::Z => DVec3::Z,
        }
    }

    /// Index into an `[f64; 3]` or a `DVec3`.
    pub fn index(self) -> usize {
        match self {
            Self::X => 0,
            Self::Y => 1,
            Self::Z => 2,
        }
    }

    /// The other two axes, in the order that keeps the frame right-handed:
    /// X gives (Y, Z), Y gives (Z, X) and Z gives (X, Y).
    pub fn tangents(self) -> (Self, Self) {
        match self {
            Self::X => (Self::Y, Self::Z),
            Self::Y => (Self::Z, Self::X),
            Self::Z => (Self::X, Self::Y),
        }
    }
}

/// A reflection plane perpendicular to one axis of the enclosing local frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorPlane {
    /// Axis the plane is perpendicular to.
    pub axis: Axis,
    /// Where the plane crosses that axis, in metres.
    pub offset: f64,
}

impl MirrorPlane {
    /// A plane perpendicular to `axis`, crossing it at `offset` metres.
    pub fn new(axis: Axis, offset: f64) -> Self {
        Self { axis, offset }
    }
}

/// A solid expression and its placement within the enclosing expression or part.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct Geometry {
    /// Unevaluated shape. A backend must explicitly support each operation it uses.
    pub shape: Shape,
    /// Applied after evaluating the shape.
    pub pose: Pose,
    /// Material slot worn by the faces this geometry exposes when it is used as
    /// a cutter.
    ///
    /// It means something only on a *cutter root*, an entry of a
    /// [`Shape::Difference`] list that is not itself inside another cutter
    /// root, and validation refuses it anywhere else. `None` falls back to the
    /// enclosing element's `cut_material_slot`, then to its `material_slot`. It
    /// is skipped when empty, so a recipe written before the field existed
    /// reads and writes unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_slot: Option<String>,
    /// Id marking this cutter as a portal, an opening a game walks through.
    ///
    /// It means something only on a *cutter root*, exactly as `cut_slot` does,
    /// and validation refuses it anywhere else. A backend publishes one
    /// [`PortalShape`](crate::PortalShape) per marked cutter, and a portal may
    /// not sit under an [`Shape::Array`] or a [`Shape::Mirror`] because its
    /// frame would not then be one rigid pose. It is skipped when empty, so a
    /// recipe written before the field existed reads and writes unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portal: Option<String>,
}

/// Initial solid vocabulary; independent of building function and visual style.
///
/// Non-exhaustive: a backend matches on it and must reject the operations it
/// does not implement, so adding a node is not a breaking change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub enum Shape {
    /// Axis-aligned box from the origin to `size`, in X/Y/Z metres.
    Cuboid {
        /// Positive extent on each axis.
        size: [f64; 3],
    },
    /// Box with planar edge and corner chamfers, preserving its outer bounds.
    ChamferedCuboid {
        /// Positive X/Y/Z extent in metres.
        size: [f64; 3],
        /// Inset along each face, strictly less than half the smallest extent.
        bevel: f64,
    },
    /// Cylinder centred on the Y axis, from Y=0 to Y=height.
    Cylinder {
        /// Positive radius in metres.
        radius: f64,
        /// Positive height in metres.
        height: f64,
        /// Circumference subdivision count, at least three.
        segments: u32,
    },
    /// Simple polygon in X/Z, extruded from Y=0 to Y=depth.
    Extrusion {
        /// Unclosed boundary in either winding; no holes or self-intersections.
        profile: Vec<[f64; 2]>,
        /// Positive extrusion distance in metres.
        depth: f64,
    },
    /// A profile in the half-plane `x >= 0` of X/Y, turned a full circle about
    /// the Y axis: a dome, a silo, a flared plinth, a ring collar.
    Revolve {
        /// Unclosed simple polygon of `[radius, height]` pairs, radius never
        /// negative. Points on the axis close the solid there.
        profile: Vec<[f64; 2]>,
        /// Subdivision count of a full turn, at least three. A partial sweep
        /// uses its share of them.
        segments: u32,
        /// How far round the profile turns, in degrees, `0 < sweep <= 360`.
        /// A partial sweep starts on the +X axis and turns towards -Z, which
        /// is a positive rotation about Y; a wedge of a round wall is placed by
        /// rotating it about Y.
        #[serde(default = "full_turn", skip_serializing_if = "is_full_turn")]
        sweep: f64,
    },
    /// The convex hull of a set of points, in this expression's frame.
    Hull {
        /// At least four points that are not all in one plane.
        points: Vec<[f64; 3]>,
    },
    /// Subtract cutters in this expression's local frame. Cut faces use the
    /// enclosing element's cut slot when present, otherwise its main slot.
    Difference {
        /// Original solid, including its local placement.
        solid: Box<Geometry>,
        /// One or more independently positioned cutters.
        cutters: Vec<Geometry>,
    },
    /// Fuse solids in this expression's local frame into one. A union creates no
    /// cut faces of its own: every face keeps the provenance it had in the
    /// operand it came from, so a reveal inside one operand stays a reveal.
    Union {
        /// Two or more independently positioned solids.
        solids: Vec<Geometry>,
    },
    /// Repeat one solid `count` times, fused. Copy zero is the solid itself and
    /// copy `n` has `step` applied `n` times, so a translation gives a row and a
    /// rotation gives a ring around the local origin. Every copy carries the
    /// provenance of the source expression, so a cutter inside it marks cut
    /// faces on all of them.
    Array {
        /// Solid to repeat, including its local placement.
        solid: Box<Geometry>,
        /// Total number of copies, including the original.
        count: u32,
        /// Placement difference between one copy and the next.
        step: Pose,
    },
    /// Reflect one solid across an axis-aligned plane. The result is the
    /// reflection alone, not the pair: wrap both in a [`Shape::Union`] for a
    /// symmetric solid. Reflected faces keep the provenance of their originals,
    /// and the winding is corrected so normals still point outward.
    Mirror {
        /// Solid to reflect, including its local placement.
        solid: Box<Geometry>,
        /// Reflection plane in this expression's local frame.
        plane: MirrorPlane,
    },
}

/// Upper bound on one array node's copies. An array is a convenience over a
/// loop in the authoring language, not a way to ask a kernel for a city.
pub const MAX_ARRAY_COUNT: u32 = 1024;

/// Upper bound on one hull node's points. A hull is a convenience over a finite
/// point set, not a way to ask a kernel for an arbitrary-detail surface.
pub const MAX_HULL_POINTS: usize = 4096;

impl Geometry {
    /// Check an editable expression before passing it to a geometry backend.
    pub fn check(&self) -> Result<(), ValidationError> {
        let mut portals = BTreeSet::new();
        self.validate("geometry", &mut Scope::new(&mut portals))
    }

    fn new(shape: Shape) -> Self {
        Self {
            shape,
            pose: Pose::default(),
            cut_slot: None,
            portal: None,
        }
    }

    /// Describe a box with an origin at its minimum corner.
    ///
    /// ```
    /// use ashlar::Geometry;
    /// // Four metres wide, three tall, a quarter deep, from the origin.
    /// Geometry::cuboid([4.0, 3.0, 0.25]).check()?;
    /// // A zero extent is not a degenerate solid, it is an authoring mistake.
    /// assert!(Geometry::cuboid([4.0, 0.0, 0.25]).check().is_err());
    /// # Ok::<(), ashlar::ValidationError>(())
    /// ```
    pub fn cuboid(size: [f64; 3]) -> Self {
        Self::new(Shape::Cuboid { size })
    }

    /// Make physical planar bevels on a box before applying later solid cuts.
    /// This is not a general edge modifier on arbitrary boolean results.
    ///
    /// The bevel is an inset along every face, so it has to stay under half the
    /// smallest extent or opposite chamfers would meet.
    ///
    /// ```
    /// use ashlar::Geometry;
    /// Geometry::chamfered_cuboid([4.0, 3.0, 0.25], 0.03).check()?;
    /// assert!(Geometry::chamfered_cuboid([4.0, 3.0, 0.25], 0.2).check().is_err());
    /// # Ok::<(), ashlar::ValidationError>(())
    /// ```
    pub fn chamfered_cuboid(size: [f64; 3], bevel: f64) -> Self {
        Self::new(Shape::ChamferedCuboid { size, bevel })
    }

    /// Describe a cylinder with explicit tessellation intent.
    ///
    /// Subdivision is authored rather than derived from a tolerance, so the
    /// same recipe gives the same triangles on every machine.
    ///
    /// ```
    /// use ashlar::Geometry;
    /// Geometry::cylinder(0.6, 4.0, 24).check()?;
    /// assert!(Geometry::cylinder(0.6, 4.0, 2).check().is_err());
    /// # Ok::<(), ashlar::ValidationError>(())
    /// ```
    pub fn cylinder(radius: f64, height: f64, segments: u32) -> Self {
        Self::new(Shape::Cylinder {
            radius,
            height,
            segments,
        })
    }

    /// Describe an extrusion, preserving authored profile winding.
    ///
    /// The profile is an unclosed simple polygon in X/Z. Concave is fine;
    /// duplicate points, collinear neighbours and self-intersections are not.
    ///
    /// ```
    /// use ashlar::Geometry;
    /// // A gabled section, extruded two metres along Y.
    /// let gable = [[0.0, 0.0], [3.0, 0.0], [1.5, 2.0]];
    /// Geometry::extrude(gable, 2.0).check()?;
    /// let bowtie = [[0.0, 0.0], [2.0, 2.0], [2.0, 0.0], [0.0, 2.0]];
    /// assert!(Geometry::extrude(bowtie, 2.0).check().is_err());
    /// # Ok::<(), ashlar::ValidationError>(())
    /// ```
    pub fn extrude(profile: impl IntoIterator<Item = [f64; 2]>, depth: f64) -> Self {
        Self::new(Shape::Extrusion {
            profile: profile.into_iter().collect(),
            depth,
        })
    }

    /// Describe a solid of revolution: a `[radius, height]` profile turned a
    /// full circle about the Y axis, `segments` steps round.
    ///
    /// The shape language of domes, pods, tanks and flared towers, which no
    /// extrusion reaches. Like a cylinder it stands on the local Y axis; unlike
    /// one, the profile may be concave, so a dome can carry a lip and a tower a
    /// waist. A profile vertex on the axis (radius zero) closes the solid there,
    /// as the crown of a dome does.
    ///
    /// ```
    /// use ashlar::Geometry;
    /// // A dome on a drum: straight wall to 1 m, then a quarter circle.
    /// let mut profile = vec![[0.0, 0.0], [2.0, 0.0], [2.0, 1.0]];
    /// for step in 1..8 {
    ///     let angle = std::f64::consts::FRAC_PI_2 * f64::from(step) / 8.0;
    ///     profile.push([2.0 * angle.cos(), 1.0 + 2.0 * angle.sin()]);
    /// }
    /// profile.push([0.0, 3.0]);
    /// Geometry::revolve(profile, 24).check()?;
    /// assert!(Geometry::revolve([[-1.0, 0.0], [1.0, 0.0], [0.0, 1.0]], 24).check().is_err());
    /// # Ok::<(), ashlar::ValidationError>(())
    /// ```
    pub fn revolve(profile: impl IntoIterator<Item = [f64; 2]>, segments: u32) -> Self {
        Self::revolve_arc(profile, segments, 360.0)
    }

    /// Describe a part of a solid of revolution: the profile turned `sweep`
    /// degrees from the +X axis towards -Z.
    ///
    /// What a round wall is cut into when each run of it needs its own convex
    /// collision proxy: the hull of a whole drum would fill the room inside it,
    /// and the hull of a twelfth of one is a thin, tight slab. `segments` is
    /// still the count for a full turn, so wedges of one wall meet facet to
    /// facet.
    ///
    /// ```
    /// use ashlar::Geometry;
    /// let wall = [[2.8, 0.0], [3.0, 0.0], [3.0, 2.5], [2.8, 2.5]];
    /// Geometry::revolve_arc(wall, 48, 30.0).check()?;
    /// assert!(Geometry::revolve_arc(wall, 48, 0.0).check().is_err());
    /// assert!(Geometry::revolve_arc(wall, 48, 400.0).check().is_err());
    /// # Ok::<(), ashlar::ValidationError>(())
    /// ```
    pub fn revolve_arc(
        profile: impl IntoIterator<Item = [f64; 2]>,
        segments: u32,
        sweep: f64,
    ) -> Self {
        Self::new(Shape::Revolve {
            profile: profile.into_iter().collect(),
            segments,
            sweep,
        })
    }

    /// Describe the convex hull of a set of points, in this expression's frame.
    ///
    /// The points stay in the recipe, so the solid is the same on every machine
    /// and its evaluation needs none of the kernel's static tables.
    ///
    /// ```
    /// use ashlar::Geometry;
    /// let tetra = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    /// Geometry::hull(tetra).check()?;
    /// let flat = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]];
    /// assert!(Geometry::hull(flat).check().is_err());
    /// # Ok::<(), ashlar::ValidationError>(())
    /// ```
    pub fn hull(points: impl IntoIterator<Item = [f64; 3]>) -> Self {
        Self::new(Shape::Hull {
            points: points.into_iter().collect(),
        })
    }

    /// Describe a ball centred on the origin, as the hull of a point set.
    ///
    /// Unlike [`cuboid`](Self::cuboid), whose origin is its minimum corner, the
    /// origin is the ball's centre, because a blast is placed by its centre.
    /// `rings` is the latitude count: the south pole, then `rings - 1` rings of
    /// `2 * rings` points, then the north pole, generated in that exact order so
    /// every client derives the same solid.
    ///
    /// This is not a kernel sphere. The point set is ours, so it is identical
    /// on every machine, and a hull reaches none of the kernel's smoothing
    /// calls, which crash on some toolchains.
    ///
    /// ```
    /// use ashlar::Geometry;
    /// Geometry::ball(1.5, 8).check()?;
    /// assert!(Geometry::ball(1.5, 1).check().is_err());
    /// assert!(Geometry::ball(0.0, 8).check().is_err());
    /// # Ok::<(), ashlar::ValidationError>(())
    /// ```
    pub fn ball(radius: f64, rings: u32) -> Self {
        // Too many rings would exceed the hull point limit, so refuse here and
        // let `check` report it, rather than building a point set no hull takes.
        let count = u64::from(rings)
            .checked_sub(1)
            .and_then(|rings| rings.checked_mul(2)?.checked_mul(rings))
            .and_then(|rings| rings.checked_add(2));
        let Some(count) = count.filter(|count| *count <= MAX_HULL_POINTS as u64) else {
            return Self::hull(Vec::new());
        };
        if !radius.is_finite() || radius <= 0.0 || rings < 2 {
            return Self::hull(Vec::new());
        }
        let mut points = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
        points.push([0.0, -radius, 0.0]);
        for ring in 1..rings {
            let latitude = -PI / 2.0 + PI * f64::from(ring) / f64::from(rings);
            for segment in 0..2 * rings {
                let longitude = TAU * f64::from(segment) / f64::from(2 * rings);
                points.push([
                    radius * latitude.cos() * longitude.cos(),
                    radius * latitude.sin(),
                    radius * latitude.cos() * longitude.sin(),
                ]);
            }
        }
        points.push([0.0, radius, 0.0]);
        Self::hull(points)
    }

    /// A conservative axis-aligned box around the solid, in the frame of this
    /// geometry's parent, without evaluating it. Tight for an unrotated
    /// primitive; never smaller than the solid. `None` for a geometry that
    /// fails [`check`](Self::check).
    pub fn bounds(&self) -> Option<[DVec3; 2]> {
        self.check().ok()?;
        let local = self.shape_bounds()?;
        boxed(
            corners(local)
                .into_iter()
                .map(|corner| self.pose.transform_point(corner)),
        )
    }

    /// [`bounds`](Self::bounds) without validating this node first: for a
    /// cutter root carrying a portal or a cut slot, which validate only in
    /// place and not as a bare expression. The caller holds a validated tree.
    pub(crate) fn placed_bounds(&self) -> Option<[DVec3; 2]> {
        let local = self.shape_bounds()?;
        boxed(
            corners(local)
                .into_iter()
                .map(|corner| self.pose.transform_point(corner)),
        )
    }

    /// The box around this expression in its own local frame: children already
    /// placed, this expression's own pose not yet applied.
    fn shape_bounds(&self) -> Option<[DVec3; 2]> {
        match &self.shape {
            Shape::Cuboid { size } | Shape::ChamferedCuboid { size, .. } => {
                Some([DVec3::ZERO, DVec3::from_array(*size)])
            }
            Shape::Cylinder { radius, height, .. } => Some([
                DVec3::new(-radius, 0.0, -radius),
                DVec3::new(*radius, *height, *radius),
            ]),
            Shape::Extrusion { profile, depth } => boxed(
                profile
                    .iter()
                    .flat_map(|[x, z]| [DVec3::new(*x, 0.0, *z), DVec3::new(*x, *depth, *z)]),
            ),
            Shape::Revolve { profile, .. } => {
                let radius = profile.iter().map(|[r, _]| *r).fold(0.0, f64::max);
                let low = profile
                    .iter()
                    .map(|[_, y]| *y)
                    .fold(f64::INFINITY, f64::min);
                let high = profile
                    .iter()
                    .map(|[_, y]| *y)
                    .fold(f64::NEG_INFINITY, f64::max);
                (low.is_finite() && high.is_finite()).then(|| {
                    [
                        DVec3::new(-radius, low, -radius),
                        DVec3::new(radius, high, radius),
                    ]
                })
            }
            Shape::Hull { points } => boxed(points.iter().map(|point| DVec3::from_array(*point))),
            Shape::Difference { solid, .. } => solid.bounds(),
            Shape::Union { solids } => {
                boxed(solids.iter().filter_map(Geometry::bounds).flat_map(corners))
            }
            Shape::Array { solid, count, step } => {
                let child = solid.bounds()?;
                let mut placement = Pose::default();
                let mut points = Vec::with_capacity(*count as usize * 8);
                for _ in 0..*count {
                    points.extend(
                        corners(child)
                            .into_iter()
                            .map(|corner| placement.transform_point(corner)),
                    );
                    placement = placement.compose(*step);
                }
                boxed(points.into_iter())
            }
            Shape::Mirror { solid, plane } => {
                let child = solid.bounds()?;
                let normal = plane.axis.unit();
                boxed(
                    corners(child).into_iter().map(|corner| {
                        corner - normal * (2.0 * (corner.dot(normal) - plane.offset))
                    }),
                )
            }
        }
    }

    /// Set placement relative to this geometry's parent.
    pub fn placed(mut self, pose: Pose) -> Self {
        self.pose = pose;
        self
    }

    /// Name the material slot worn by the faces this geometry exposes when it is
    /// used as a cutter.
    ///
    /// The slot is meaningful only on a cutter root; validation refuses it
    /// anywhere else. Where it is absent, the enclosing element's cut slot then
    /// its main slot are used instead.
    ///
    /// ```
    /// use ashlar::{Geometry, Pose};
    /// // One wall, two openings, each cut face finished differently.
    /// let wall = Geometry::cuboid([4.0, 3.0, 0.3])
    ///     .subtract(
    ///         Geometry::cuboid([1.0, 1.0, 0.5])
    ///             .placed(Pose::at([1.0, 1.0, -0.1]))
    ///             .cut_material("reveal"),
    ///     )
    ///     .subtract(
    ///         Geometry::cuboid([0.8, 0.8, 0.5])
    ///             .placed(Pose::at([3.0, 1.0, -0.1]))
    ///             .cut_material("sill"),
    ///     );
    /// wall.check()?;
    /// # Ok::<(), ashlar::ValidationError>(())
    /// ```
    pub fn cut_material(mut self, slot: impl Into<String>) -> Self {
        self.cut_slot = Some(slot.into());
        self
    }

    /// Mark a cutter as a portal, the opening a game walks through.
    ///
    /// The mark is meaningful only on a cutter root; validation refuses it
    /// anywhere else and refuses a portal under an array or a mirror, whose
    /// frame is not one rigid pose. The backend publishes one rectangle per
    /// marked cutter, at the middle of the solid's thickness; an arched or
    /// round opening publishes its bounding rectangle in v1.
    ///
    /// ```
    /// use ashlar::{Geometry, Pose};
    /// // A doorway cutter runs past the wall on both sides, so it is an opening.
    /// let wall = Geometry::cuboid([4.0, 3.0, 0.3]).subtract(
    ///     Geometry::cuboid([1.0, 2.2, 0.5])
    ///         .placed(Pose::at([1.5, -0.2, -0.1]))
    ///         .portal("front-door"),
    /// );
    /// wall.check()?;
    /// # Ok::<(), ashlar::ValidationError>(())
    /// ```
    pub fn portal(mut self, id: impl Into<String>) -> Self {
        self.portal = Some(id.into());
        self
    }

    /// Wrap this solid in a subtraction. Existing solid and cutter placements
    /// share the new expression's local frame; neither is silently reset.
    ///
    /// A cutter that runs past the solid on both sides is what makes an opening
    /// rather than a recess; the backend marks the faces it created.
    ///
    /// ```
    /// use ashlar::{Geometry, Pose};
    /// let wall = Geometry::cuboid([4.0, 3.0, 0.3])
    ///     .subtract(Geometry::cuboid([1.5, 2.2, 0.5]).placed(Pose::at([1.25, 0.0, -0.1])));
    /// wall.check()?;
    /// # Ok::<(), ashlar::ValidationError>(())
    /// ```
    pub fn subtract(self, cutter: Self) -> Self {
        Self::new(Shape::Difference {
            solid: Box::new(self),
            cutters: vec![cutter],
        })
    }

    /// Fuse this solid with another. Both keep their own placements, in the new
    /// expression's frame.
    pub fn union(self, other: Self) -> Self {
        Self::new(Shape::Union {
            solids: vec![self, other],
        })
    }

    /// Fuse a whole group at once, which is one kernel operation rather than a
    /// chain of them. Validation rejects fewer than two solids.
    ///
    /// ```
    /// use ashlar::{Geometry, Pose};
    /// let posts = (0..4).map(|i| {
    ///     Geometry::cuboid([0.2, 3.0, 0.2]).placed(Pose::at([f64::from(i) * 1.5, 0.0, 0.0]))
    /// });
    /// Geometry::union_all(posts).check()?;
    /// assert!(Geometry::union_all([Geometry::cuboid([1.0; 3])]).check().is_err());
    /// # Ok::<(), ashlar::ValidationError>(())
    /// ```
    pub fn union_all(solids: impl IntoIterator<Item = Self>) -> Self {
        Self::new(Shape::Union {
            solids: solids.into_iter().collect(),
        })
    }

    /// Repeat this solid `count` times, each copy one further `step` along.
    ///
    /// A translating step gives a row and a rotating one gives a ring, and the
    /// kernel fuses the copies rather than the recipe listing them.
    ///
    /// ```
    /// use ashlar::{Geometry, Pose, MAX_ARRAY_COUNT};
    /// let railing = Geometry::cuboid([0.05, 1.0, 0.05]).arrayed(20, Pose::at([0.25, 0.0, 0.0]));
    /// railing.check()?;
    /// let city = Geometry::cuboid([1.0; 3]).arrayed(MAX_ARRAY_COUNT + 1, Pose::at([2.0, 0.0, 0.0]));
    /// assert!(city.check().is_err());
    /// # Ok::<(), ashlar::ValidationError>(())
    /// ```
    pub fn arrayed(self, count: u32, step: Pose) -> Self {
        Self::new(Shape::Array {
            solid: Box::new(self),
            count,
            step,
        })
    }

    /// Reflect this solid across a plane in the new expression's frame.
    ///
    /// The result is the reflection alone, so a symmetric part unions a solid
    /// with its own mirror.
    ///
    /// ```
    /// use ashlar::{Axis, Geometry, MirrorPlane, Pose};
    /// let wing = Geometry::cuboid([2.0, 3.0, 0.3]).placed(Pose::at([0.5, 0.0, 0.0]));
    /// let symmetric = wing.clone().union(wing.mirrored(MirrorPlane::new(Axis::X, 0.0)));
    /// symmetric.check()?;
    /// # Ok::<(), ashlar::ValidationError>(())
    /// ```
    pub fn mirrored(self, plane: MirrorPlane) -> Self {
        Self::new(Shape::Mirror {
            solid: Box::new(self),
            plane,
        })
    }

    /// Every cutter root of this geometry, in index order, so `cutters()[k]` is
    /// cutter `k`.
    ///
    /// A *cutter root* is an entry of a [`Shape::Difference`] list that is not
    /// itself inside another cutter root. Roots are numbered from zero in this
    /// depth-first order:
    ///
    /// - `Difference { solid, cutters }`: first everything inside `solid`, then
    ///   each entry of `cutters` in list order. Each entry is one root, and the
    ///   search does not descend into it, so a difference nested inside a
    ///   cutter belongs to that cutter rather than becoming a root of its own.
    /// - `Union { solids }`: each solid in list order.
    /// - `Array { solid, .. }` and `Mirror { solid, .. }`: inside `solid`. An
    ///   array's copies all share the numbers of the one authored solid.
    /// - The primitives (`Cuboid`, `ChamferedCuboid`, `Cylinder`, `Extrusion`,
    ///   `Hull`) hold nothing.
    ///
    /// This function *is* the definition of that numbering, so a backend that
    /// meshes cutters and a resolver that looks one up by a
    /// [`FaceOrigin::Cutter`](crate::FaceOrigin::Cutter) index must both call
    /// it, and cannot disagree.
    pub fn cutters(&self) -> Vec<&Geometry> {
        let mut roots = Vec::new();
        self.collect_cutters(&mut roots);
        roots
    }

    fn collect_cutters<'a>(&'a self, roots: &mut Vec<&'a Geometry>) {
        match &self.shape {
            Shape::Difference { solid, cutters } => {
                solid.collect_cutters(roots);
                roots.extend(cutters.iter());
            }
            Shape::Union { solids } => {
                for solid in solids {
                    solid.collect_cutters(roots);
                }
            }
            Shape::Array { solid, .. } | Shape::Mirror { solid, .. } => {
                solid.collect_cutters(roots);
            }
            Shape::Cuboid { .. }
            | Shape::ChamferedCuboid { .. }
            | Shape::Cylinder { .. }
            | Shape::Extrusion { .. }
            | Shape::Revolve { .. }
            | Shape::Hull { .. } => {}
        }
    }

    pub(crate) fn validate(
        &self,
        path: &str,
        scope: &mut Scope<'_>,
    ) -> Result<(), ValidationError> {
        let (context, under_repeat) = (scope.context, scope.under_repeat);
        require(
            scope.depth <= 64,
            path,
            "geometry nesting exceeds 64 levels",
        )?;
        self.pose.validate(&format!("{path}.pose"))?;
        if let Some(slot) = &self.cut_slot {
            require(
                context == CutContext::CutterRoot,
                &format!("{path}.cut_slot"),
                "only a cutter names a cut slot",
            )?;
            name(slot, &format!("{path}.cut_slot"))?;
        }
        if let Some(portal) = &self.portal {
            validate_portal(portal, path, context, under_repeat, scope.portals)?;
        }
        match &self.shape {
            Shape::ChamferedCuboid { size, bevel } => require(
                size.iter().all(|n| n.is_finite() && *n > 0.0)
                    && bevel.is_finite()
                    && *bevel > 0.0
                    && size.iter().all(|n| *bevel < *n * 0.5),
                path,
                "chamfer must be finite, positive and smaller than half every box extent",
            ),
            Shape::Cuboid { size } => require(
                size.iter().all(|n| n.is_finite() && *n > 0.0),
                path,
                "box dimensions must be finite and positive",
            ),
            Shape::Cylinder {
                radius,
                height,
                segments,
            } => validate_cylinder(*radius, *height, *segments, path),
            Shape::Extrusion { profile, depth } => {
                require(
                    depth.is_finite() && *depth > 0.0,
                    path,
                    "extrusion depth must be finite and positive",
                )?;
                validate_profile(profile, path)
            }
            Shape::Revolve {
                profile,
                segments,
                sweep,
            } => validate_revolve(profile, *segments, *sweep, path),
            Shape::Hull { points } => validate_hull_shape(points, path),
            Shape::Difference { solid, cutters } => {
                require(!cutters.is_empty(), path, "difference requires a cutter")?;
                solid.validate(
                    &format!("{path}.solid"),
                    &mut scope.child(context.descend(), under_repeat),
                )?;
                validate_list(
                    cutters,
                    &format!("{path}.cutters"),
                    scope,
                    context.within_cutter(),
                )
            }
            Shape::Union { solids } => {
                require(solids.len() >= 2, path, "union requires two solids")?;
                validate_list(solids, &format!("{path}.solids"), scope, context.descend())
            }
            Shape::Array { solid, count, step } => {
                require(*count >= 1, path, "array requires at least one copy")?;
                require(
                    *count <= MAX_ARRAY_COUNT,
                    path,
                    "array exceeds the copy limit",
                )?;
                step.validate(&format!("{path}.step"))?;
                solid.validate(
                    &format!("{path}.solid"),
                    &mut scope.child(context.descend(), true),
                )
            }
            Shape::Mirror { solid, plane } => {
                require(
                    plane.offset.is_finite(),
                    &format!("{path}.plane"),
                    "mirror plane offset must be finite",
                )?;
                solid.validate(
                    &format!("{path}.solid"),
                    &mut scope.child(context.descend(), true),
                )
            }
        }
    }
}

/// The state threaded down one geometry tree while it is validated: how deep
/// the node is, what consumes it, whether an array or a mirror encloses it, and
/// the portal ids seen so far.
pub(crate) struct Scope<'a> {
    depth: usize,
    context: CutContext,
    under_repeat: bool,
    portals: &'a mut BTreeSet<String>,
}

impl<'a> Scope<'a> {
    /// A scope for an element's root geometry.
    pub(crate) fn new(portals: &'a mut BTreeSet<String>) -> Self {
        Self {
            depth: 0,
            context: CutContext::Solid,
            under_repeat: false,
            portals,
        }
    }

    /// A scope one level down, with a different role or repeat state. The portal
    /// set is shared with the parent.
    fn child(&mut self, context: CutContext, under_repeat: bool) -> Scope<'_> {
        Scope {
            depth: self.depth + 1,
            context,
            under_repeat,
            portals: &mut *self.portals,
        }
    }
}

/// Validate each child of a difference's cutter list or a union's solid list,
/// at `context`, sharing the parent's repeat state and portal set.
fn validate_list(
    children: &[Geometry],
    prefix: &str,
    scope: &mut Scope<'_>,
    context: CutContext,
) -> Result<(), ValidationError> {
    let under_repeat = scope.under_repeat;
    for (i, child) in children.iter().enumerate() {
        child.validate(
            &format!("{prefix}[{i}]"),
            &mut scope.child(context, under_repeat),
        )?;
    }
    Ok(())
}

/// A portal id is a name, meaningful only on a cutter root, and unique within
/// its geometry tree. A cutter under an array or a mirror has no one rigid
/// frame, so it cannot publish a walkable rectangle.
fn validate_portal(
    portal: &str,
    path: &str,
    context: CutContext,
    under_repeat: bool,
    portals: &mut BTreeSet<String>,
) -> Result<(), ValidationError> {
    let portal_path = format!("{path}.portal");
    require(
        context == CutContext::CutterRoot,
        &portal_path,
        "only a cutter is a portal",
    )?;
    name(portal, &portal_path)?;
    require(
        !under_repeat,
        &portal_path,
        "a portal cannot sit under an array or a mirror",
    )?;
    require(
        portals.insert(portal.to_owned()),
        &portal_path,
        "duplicate portal",
    )
}

/// Where a node sits relative to the subtraction that will consume it, which is
/// what decides whether it may name its own cut slot.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CutContext {
    /// An ordinary solid, outside every cutter.
    Solid,
    /// An entry of a [`Shape::Difference`] list met as an ordinary solid.
    CutterRoot,
    /// Anywhere beneath a cutter root.
    InsideCutter,
}

impl CutContext {
    /// The context of a node reached through `solid`, a union entry, an array
    /// entry or a mirror entry.
    fn descend(self) -> Self {
        match self {
            Self::Solid => Self::Solid,
            Self::CutterRoot | Self::InsideCutter => Self::InsideCutter,
        }
    }

    /// The context of an entry of a difference's `cutters` list.
    fn within_cutter(self) -> Self {
        match self {
            Self::Solid => Self::CutterRoot,
            Self::CutterRoot | Self::InsideCutter => Self::InsideCutter,
        }
    }
}

fn cross(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn on_segment(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> bool {
    (0..2).all(|axis| p[axis] >= a[axis].min(b[axis]) && p[axis] <= a[axis].max(b[axis]))
}

fn intersects(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let [ac, ad, ca, cb] = [
        cross(a, b, c),
        cross(a, b, d),
        cross(c, d, a),
        cross(c, d, b),
    ];
    let opposite = |x: f64, y: f64| (x < 0.0 && y > 0.0) || (x > 0.0 && y < 0.0);
    (opposite(ac, ad) && opposite(ca, cb))
        || (ac == 0.0 && on_segment(a, b, c))
        || (ad == 0.0 && on_segment(a, b, d))
        || (ca == 0.0 && on_segment(c, d, a))
        || (cb == 0.0 && on_segment(c, d, b))
}

// Authored duplicate coordinates are exact; near-coincident points are left to the mesher.
#[allow(clippy::float_cmp)]
fn validate_profile(points: &[[f64; 2]], path: &str) -> Result<(), ValidationError> {
    require(
        points.len() >= 3,
        path,
        "profile requires at least three vertices",
    )?;
    require(
        points.iter().flatten().all(|n| n.is_finite()),
        path,
        "profile coordinates must be finite",
    )?;
    let n = points.len();
    let mut area = 0.0;
    for i in 0..n {
        let a = points[i];
        let b = points[(i + 1) % n];
        require(a != b, path, "profile contains a zero-length edge")?;
        let turn = cross(a, b, points[(i + 2) % n]);
        require(
            turn.is_finite() && turn != 0.0,
            path,
            "profile has collinear neighbours or overflowing coordinates",
        )?;
        area += cross(points[0], a, b);
        for j in (i + 1)..n {
            if j == i + 1 || (i == 0 && j == n - 1) {
                continue;
            }
            require(
                !intersects(a, b, points[j], points[(j + 1) % n]),
                path,
                "profile self-intersects",
            )?;
        }
    }
    require(
        area.is_finite() && area.abs() > 0.0,
        path,
        "profile must enclose finite nonzero area",
    )
}

/// A hull needs at least four finite points and four of them off one plane.
fn validate_cylinder(
    radius: f64,
    height: f64,
    segments: u32,
    path: &str,
) -> Result<(), ValidationError> {
    require(
        radius.is_finite() && radius > 0.0 && height.is_finite() && height > 0.0,
        path,
        "cylinder dimensions must be finite and positive",
    )?;
    require(
        segments >= 3,
        path,
        "cylinder requires at least three segments",
    )
}

fn full_turn() -> f64 {
    360.0
}

// A `&f64` is only "trivially copyable" by clippy's measure on a 64-bit target.
#[cfg_attr(
    target_pointer_width = "64",
    expect(clippy::trivially_copy_pass_by_ref, reason = "serde's skip predicate")
)]
fn is_full_turn(sweep: &f64) -> bool {
    (*sweep - 360.0).abs() < f64::EPSILON
}

/// A revolve's profile is an extrusion's, confined to the half-plane it turns.
fn validate_revolve(
    profile: &[[f64; 2]],
    segments: u32,
    sweep: f64,
    path: &str,
) -> Result<(), ValidationError> {
    require(
        segments >= 3,
        path,
        "revolve requires at least three segments",
    )?;
    require(
        sweep.is_finite() && sweep > 0.0 && sweep <= 360.0,
        path,
        "revolve sweep must be more than zero and at most 360 degrees",
    )?;
    require(
        profile.iter().all(|[r, _]| *r >= 0.0),
        path,
        "revolve profile radius must not be negative",
    )?;
    validate_profile(profile, path)
}

fn validate_hull_shape(points: &[[f64; 3]], path: &str) -> Result<(), ValidationError> {
    require(
        (4..=MAX_HULL_POINTS).contains(&points.len()),
        path,
        "hull requires between four and the point limit",
    )?;
    require(
        points.iter().flatten().all(|n| n.is_finite()),
        path,
        "hull points must be finite",
    )?;
    validate_hull(points, path)
}

/// A hull spans space only when its first point, the point farthest from it,
/// and the point farthest from that line are each a real distance apart, and
/// some point lies off the plane those three span.
fn validate_hull(points: &[[f64; 3]], path: &str) -> Result<(), ValidationError> {
    let a = DVec3::from_array(points[0]);
    let mut b = a;
    let mut ab_length = 0.0;
    for point in points {
        let point = DVec3::from_array(*point);
        let length = point.distance(a);
        if length > ab_length {
            ab_length = length;
            b = point;
        }
    }
    require(ab_length > 1e-9, path, "hull points must not all coincide")?;
    let axis = (b - a) / ab_length;
    let off_line = |point: DVec3| (point - a) - axis * (point - a).dot(axis);
    let mut c = a;
    let mut off_length = 0.0;
    for point in points {
        let length = off_line(DVec3::from_array(*point)).length();
        if length > off_length {
            off_length = length;
            c = DVec3::from_array(*point);
        }
    }
    require(
        off_length > 1e-9,
        path,
        "hull points must not all be collinear",
    )?;
    let normal = (b - a).cross(c - a).normalize();
    let mut plane_distance = 0.0;
    for point in points {
        let distance = (DVec3::from_array(*point) - a).dot(normal).abs();
        if distance > plane_distance {
            plane_distance = distance;
        }
    }
    require(
        plane_distance > 1e-9,
        path,
        "hull points must not all be coplanar",
    )
}
