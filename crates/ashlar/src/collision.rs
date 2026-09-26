//! Declared collision proxies. The domain says which elements are solid to walk
//! into; a backend derives the shapes from the same geometry it meshes.
use glam::DVec3;
use serde::{Deserialize, Serialize};

use crate::Pose;

/// What an element contributes to static collision.
///
/// A proxy is convex and conservative. An opening cut out of an element is not
/// cut out of its proxy, so a wall meant to be walked through needs the opening
/// to be the gap between elements rather than a subtraction inside one.
///
/// Non-exhaustive: a tighter derivation is a backend feature, not a break.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub enum Collision {
    /// No proxy. The default, so trim, slats, lights and signage cost nothing.
    #[default]
    None,
    /// Axis-aligned box around the evaluated solid, in part space, then posed
    /// with its instance. The cheapest useful proxy for a rectangular element.
    Bounds,
    /// Convex hull of the evaluated solid. Tighter than [`Self::Bounds`] on a
    /// chamfered or angled element, and more vertices to carry.
    Hull,
}

/// A convex collision proxy, as the points it is the hull of.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConvexSolid {
    /// Hull points, in the frame named by whoever hands the solid over.
    pub vertices: Vec<DVec3>,
}

impl ConvexSolid {
    /// The eight corners of an axis-aligned box.
    pub fn from_bounds(min: DVec3, max: DVec3) -> Self {
        Self {
            vertices: (0..8)
                .map(|corner| {
                    DVec3::new(
                        if corner & 1 == 0 { min.x } else { max.x },
                        if corner & 2 == 0 { min.y } else { max.y },
                        if corner & 4 == 0 { min.z } else { max.z },
                    )
                })
                .collect(),
        }
    }

    /// The tightest axis-aligned box around the points, or `None` when empty.
    pub fn bounds(&self) -> Option<[DVec3; 2]> {
        let mut min = DVec3::splat(f64::INFINITY);
        let mut max = DVec3::splat(f64::NEG_INFINITY);
        for point in &self.vertices {
            min = min.min(*point);
            max = max.max(*point);
        }
        (!self.vertices.is_empty()).then_some([min, max])
    }

    /// Apply a rigid placement. A rotated box is still convex, so the proxy
    /// stays exact rather than being re-fitted to the axes.
    #[must_use]
    pub fn placed(&self, pose: Pose) -> Self {
        Self {
            vertices: self
                .vertices
                .iter()
                .map(|p| pose.transform_point(*p))
                .collect(),
        }
    }
}
