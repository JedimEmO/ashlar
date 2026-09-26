//! Levels of detail as simplified recipes.
//!
//! A coarser level of a building is not a decimated mesh. The recipe is
//! procedural, so a simpler building is a simpler recipe: fewer segments round
//! every cylinder and revolve, the small cutters — a window reveal, a porthole,
//! a vent slot — left out so the facade texture carries them, the thin
//! elements — trim, rails, sills, props, furniture — dropped, and the interior
//! side dropped entirely. An element that glows declares itself
//! [`far`](Element::far) and stays. A [`LodPolicy`] says which and from what distance, and the mesher
//! evaluates the simplified parts exactly as it evaluates the authored ones, so
//! every level is a closed, correctly shaded and correctly mapped solid, and a
//! level's texture lines up with the next one's. See ADR 0006.
//!
//! ```
//! use ashlar::{Element, Geometry, LodPolicy, Part, Pose};
//! let wall = Part::builder("example:wall")
//!     .element(Element::new(
//!         "body",
//!         Geometry::cuboid([4.0, 3.0, 0.3])
//!             .subtract(Geometry::cuboid([0.2, 0.2, 0.5]).placed(Pose::at([1.0, 1.0, -0.1]))),
//!         "wall",
//!     ))
//!     .element(Element::new("sign", Geometry::cuboid([0.3, 0.1, 0.02]), "sign"))
//!     .build()?;
//! let far = LodPolicy { min_feature: 0.5, ..LodPolicy::default() };
//! let simple = wall.simplified(&far).expect("the wall survives");
//! assert_eq!(simple.elements.len(), 1, "the sign is gone");
//! assert!(simple.elements[0].geometry.cutters().is_empty(), "so is the vent");
//! # Ok::<(), ashlar::ValidationError>(())
//! ```
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{Building, Element, Geometry, Part, Shape, Side, ValidationError};

/// How one level of detail simplifies a building, and where it takes over.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LodPolicy {
    /// Distance in metres from the camera at which this level stops drawing
    /// and the next one starts. `None` draws to any distance, which is what
    /// the last level of a ladder usually wants.
    #[serde(default)]
    pub until: Option<f32>,
    /// Metres. An element or a union member thinner than this is left out,
    /// and so is a cutter whose opening is narrower than this. Thin is judged
    /// by the middle of a solid's three extents: the narrower side of its
    /// broadest face, which is how wide it looks from where it looks
    /// widest. A sill three metres long and twenty centimetres deep is twenty
    /// centimetres of anything at a distance, and a cutter's depth runs past
    /// the faces it opens, so a hole is its narrower side too. A dropped
    /// cutter is a hole that is no longer cut; at the distance this level
    /// draws, the material's texture carries it. An element marked
    /// [`far`](Element::far) is never dropped, nor are its union members.
    #[serde(default)]
    pub min_feature: f64,
    /// Multiplies the segment count of every cylinder and revolve, in
    /// `(0, 1]`. Never below [`MIN_SEGMENTS`], and never above what was
    /// authored.
    #[serde(default = "one")]
    pub segment_scale: f64,
    /// Leave out every [`Side::Interior`] element: nothing inside a building
    /// is seen from the distance a coarse level draws at.
    #[serde(default)]
    pub drop_interior: bool,
}

/// The fewest segments a simplified cylinder or revolve keeps round a turn.
///
/// Eight: a column at eight sides still reads as round in silhouette from the
/// distances a coarse level draws at, where six reads as a hexagon.
pub const MIN_SEGMENTS: u32 = 8;

fn one() -> f64 {
    1.0
}

impl Default for LodPolicy {
    /// The authored building, to any distance.
    fn default() -> Self {
        Self {
            until: None,
            min_feature: 0.0,
            segment_scale: 1.0,
            drop_interior: false,
        }
    }
}

impl LodPolicy {
    /// The level as authored, drawn out to `until` metres.
    #[must_use]
    pub fn full(until: Option<f32>) -> Self {
        Self {
            until,
            ..Self::default()
        }
    }

    /// Whether this policy changes nothing.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.min_feature <= 0.0 && self.segment_scale >= 1.0 && !self.drop_interior
    }

    /// A default ladder of three levels, for buildings at the scale of the
    /// showcase's houses, kits and towers. A game tunes its own.
    ///
    /// - **Level 0, to 60 m:** as authored, inside and out.
    /// - **Level 1, to 250 m:** no interior; nothing smaller than 35 cm, which
    ///   takes the furniture, props, rivets, sills and reveals; half the
    ///   segments round.
    /// - **Level 2, beyond:** nothing smaller than 1.5 m, which leaves walls,
    ///   slabs, roofs, towers and the openings a storey is made of; a quarter
    ///   of the segments.
    ///
    /// The distances are where each level's dropped detail falls under a
    /// couple of pixels at a 60-degree field of view on a 1080p screen: a
    /// 35 cm feature is about two pixels at 250 m, and 1.5 m is about two at a
    /// kilometre.
    #[must_use]
    pub fn ladder() -> Vec<Self> {
        vec![
            Self::full(Some(60.0)),
            Self {
                until: Some(250.0),
                min_feature: 0.35,
                segment_scale: 0.5,
                drop_interior: true,
            },
            Self {
                until: None,
                min_feature: 1.5,
                segment_scale: 0.25,
                drop_interior: true,
            },
        ]
    }

    fn segments(&self, authored: u32) -> u32 {
        if self.segment_scale >= 1.0 {
            return authored;
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a positive count scaled by at most one"
        )]
        let scaled = (f64::from(authored) * self.segment_scale.max(0.0)).round() as u32;
        scaled.max(MIN_SEGMENTS).min(authored)
    }

    /// Whether a solid is thinner than the feature size: the middle of its
    /// three extents. For a solid that is the narrower side of its broadest
    /// face — a rail's five centimetres, a slab's width — and for a cutter
    /// through a wall, which runs past both faces, the opening's narrower side:
    /// a vent's twenty centimetres, a door's metre.
    fn thin(&self, geometry: &Geometry) -> bool {
        self.min_feature > 0.0
            && geometry.placed_bounds().is_none_or(|[low, high]| {
                let mut extents = (high - low).to_array();
                extents.sort_by(f64::total_cmp);
                extents[1] < self.min_feature
            })
    }
}

impl Geometry {
    /// This expression with `policy` applied: segment counts scaled, and every
    /// union member and cutter thinner than the policy's feature size left out.
    /// `None` when nothing survives.
    ///
    /// The expression's own size is not judged here — an element's is, by
    /// [`Part::simplified`] — because a small solid on its own is what its
    /// caller decides to keep or drop.
    #[must_use]
    pub fn simplified(&self, policy: &LodPolicy) -> Option<Self> {
        let shape = match &self.shape {
            Shape::Cylinder {
                radius,
                height,
                segments,
            } => Shape::Cylinder {
                radius: *radius,
                height: *height,
                segments: policy.segments(*segments),
            },
            Shape::Revolve {
                profile,
                segments,
                sweep,
            } => Shape::Revolve {
                profile: profile.clone(),
                segments: policy.segments(*segments),
                sweep: *sweep,
            },
            Shape::Difference { solid, cutters } => {
                let solid = solid.simplified(policy)?;
                let cutters: Vec<Self> = cutters
                    .iter()
                    .filter(|cutter| !policy.thin(cutter))
                    .filter_map(|cutter| cutter.simplified(policy))
                    .collect();
                if cutters.is_empty() {
                    return Some(solid.replacing(self));
                }
                Shape::Difference {
                    solid: Box::new(solid),
                    cutters,
                }
            }
            Shape::Union { solids } => {
                let mut solids: Vec<Self> = solids
                    .iter()
                    .filter(|solid| !policy.thin(solid))
                    .filter_map(|solid| solid.simplified(policy))
                    .collect();
                match solids.len() {
                    0 => return None,
                    1 => return solids.pop().map(|solid| solid.replacing(self)),
                    _ => Shape::Union { solids },
                }
            }
            Shape::Array { solid, count, step } => {
                if policy.thin(solid) {
                    return None;
                }
                Shape::Array {
                    solid: Box::new(solid.simplified(policy)?),
                    count: *count,
                    step: *step,
                }
            }
            Shape::Mirror { solid, plane } => Shape::Mirror {
                solid: Box::new(solid.simplified(policy)?),
                plane: *plane,
            },
            other => other.clone(),
        };
        Some(Self {
            shape,
            ..self.clone()
        })
    }

    /// This expression moved by `pose`, after the placement it already has:
    /// what a wrapping node's pose does to the one child that replaces it.
    /// This geometry standing in for `parent`, which simplified down to it:
    /// under the parent's pose, and wearing the parent's cutter slot and portal
    /// when it has none of its own. A door cutter that was a union of a frame
    /// and a leaf and lost the leaf is still the door, in the door's slot.
    fn replacing(mut self, parent: &Self) -> Self {
        self.pose = parent.pose.compose(self.pose);
        if self.cut_slot.is_none() {
            self.cut_slot.clone_from(&parent.cut_slot);
        }
        if self.portal.is_none() {
            self.portal.clone_from(&parent.portal);
        }
        self
    }
}

impl Part {
    /// This part with `policy` applied to every element; see
    /// [`Geometry::simplified`]. An element is dropped when it is interior and
    /// the policy drops the interior, when it is thinner than the
    /// policy's feature size, or when nothing of its geometry survives.
    /// Sockets are kept. `None` when no element survives.
    #[must_use]
    pub fn simplified(&self, policy: &LodPolicy) -> Option<Self> {
        let elements: Vec<Element> = self
            .elements
            .iter()
            .filter(|element| !(policy.drop_interior && element.side == Side::Interior))
            .filter(|element| element.far || !policy.thin(&element.geometry))
            .filter_map(|element| {
                // A far element keeps every member and every hole; only its
                // segments coarsen.
                let geometry = if element.far {
                    element.geometry.simplified(&LodPolicy {
                        min_feature: 0.0,
                        ..*policy
                    })?
                } else {
                    element.geometry.simplified(policy)?
                };
                Some(Element {
                    geometry,
                    ..element.clone()
                })
            })
            .collect();
        (!elements.is_empty()).then(|| Self {
            elements,
            ..self.clone()
        })
    }
}

impl Building {
    /// This building with `policy` applied to every part; see
    /// [`Part::simplified`].
    ///
    /// A part nothing of which survives is removed, and so are the instances
    /// that placed it. Every surviving instance keeps the pose it was resolved
    /// to, with its attachment cleared — the part it was attached to may be
    /// gone — and only the slot overrides its simplified part still uses.
    /// Merge groups, rooms and the palette are kept, so a merged building
    /// stays merged and a room keeps its storey.
    ///
    /// `Ok(None)` when nothing survives: a lamp post has no level past the
    /// distance at which everything in it is smaller than `min_feature`, and a
    /// baked level for it is simply empty.
    ///
    /// # Errors
    ///
    /// If the simplified recipe does not validate, which would be a bug in the
    /// simplification rather than something content can cause.
    pub fn simplified(&self, policy: &LodPolicy) -> Result<Option<Self>, ValidationError> {
        let mut recipe = self.recipe().clone();
        let used: BTreeSet<&str> = recipe.instances.iter().map(|i| i.part.as_str()).collect();
        let parts: Vec<Part> = recipe
            .parts
            .iter()
            .filter(|part| used.contains(part.id.as_str()))
            .filter_map(|part| part.simplified(policy))
            .collect();
        let slots = |part: &str| -> BTreeSet<String> {
            parts
                .iter()
                .find(|p| p.id == part)
                .map(|p| {
                    p.elements
                        .iter()
                        .flat_map(|e| e.slots().map(str::to_owned).collect::<Vec<_>>())
                        .collect()
                })
                .unwrap_or_default()
        };
        let kept: BTreeSet<String> = parts.iter().map(|p| p.id.clone()).collect();
        recipe
            .instances
            .retain(|instance| kept.contains(&instance.part));
        for instance in &mut recipe.instances {
            instance.attach = None;
            let used = slots(&instance.part);
            instance.materials.retain(|slot, _| used.contains(slot));
        }
        recipe.parts = parts;
        if recipe.instances.is_empty() {
            return Ok(None);
        }
        recipe.build().map(Some)
    }
}
