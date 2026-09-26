//! Damage as data: the solids a building has had subtracted from it.
//!
//! A hit is one solid, where it struck, and the slot its exposed faces wear.
//! A building's damage is an ordered list of them, and that list is both the
//! save format and the wire format: a server sends "this solid, here", and
//! every client derives the same hole because a [`Geometry`] and a [`Pose`]
//! evaluate the same everywhere.
use glam::DVec3;
use serde::{Deserialize, Serialize};

use crate::geometry::{boxed, corners};
use crate::{Building, Geometry, Pose, ValidationError, require};

/// One hit: a solid, where it struck, and what the faces it exposes wear.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Damage {
    /// What is subtracted, in its own frame.
    pub solid: Geometry,
    /// Where that frame stands in building space.
    #[serde(default)]
    pub pose: Pose,
    /// The slot the exposed faces wear, resolved against the building's own
    /// palette: a hole belongs to no instance, so no instance override applies.
    pub slot: String,
    /// Whether what this hit leaves unattached falls with it.
    ///
    /// When set, after the solid is subtracted, a connected piece that does not
    /// reach the ground is removed as well. A piece reaches the ground when it
    /// touches it, or touches another supported piece of any group, so cutting
    /// the storey under another drops that one too. A piece that was already
    /// loose in the undamaged building, and that no hit has changed, stays
    /// where its author put it. A backend that meshes one group alone applies
    /// the base rule instead, because within one group reaching the base and
    /// being supported are the same thing; the building-wide rule is
    /// `GroupSolids`' business. This is on the record rather than a setting of
    /// the mesher, because the log is the wire format and every client must
    /// derive the same building.
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapse: bool,
}

impl Damage {
    /// Record a hit on the solid's own frame, wearing `slot`.
    #[must_use]
    pub fn new(solid: Geometry, slot: impl Into<String>) -> Self {
        Self {
            solid,
            pose: Pose::default(),
            slot: slot.into(),
            collapse: false,
        }
    }

    /// Place the solid's frame in building space.
    #[must_use]
    pub fn placed(mut self, pose: Pose) -> Self {
        self.pose = pose;
        self
    }

    /// Also drop whatever this hit leaves unattached; see [`Damage::collapse`].
    #[must_use]
    pub fn collapsing(mut self) -> Self {
        self.collapse = true;
        self
    }

    /// A blast: a ball of `radius` centred on `centre`, wearing `slot`.
    ///
    /// The ball is [`Geometry::ball`], so its point set is ours and no kernel
    /// sphere is evaluated.
    #[must_use]
    pub fn blast(centre: [f64; 3], radius: f64, slot: impl Into<String>) -> Self {
        Self::new(Geometry::ball(radius, 8), slot).placed(Pose::at(centre))
    }

    /// A conservative box around the hit, in building space. `None` when the
    /// solid has no bounds of its own.
    pub fn bounds(&self) -> Option<[DVec3; 2]> {
        let local = self.solid.bounds()?;
        boxed(
            corners(local)
                .into_iter()
                .map(|corner| self.pose.transform_point(corner)),
        )
    }

    /// Check the hit against the building it will be subtracted from.
    ///
    /// The building must be merged, the solid must be valid geometry, the pose
    /// finite with a unit rotation, and the slot must be bound in the building's
    /// own palette.
    pub fn check(&self, building: &Building, path: &str) -> Result<(), ValidationError> {
        require(
            building.recipe().merged,
            path,
            "damage needs a merged building",
        )?;
        self.solid.check().map_err(|error| ValidationError {
            path: format!(
                "{path}.solid{}",
                error.path.strip_prefix("geometry").unwrap_or("")
            ),
            reason: error.reason,
        })?;
        self.pose.validate(&format!("{path}.pose"))?;
        require(
            building.recipe().materials.contains_key(&self.slot),
            &format!("{path}.slot"),
            "damage slot has no binding in the building's palette",
        )
    }
}

/// A building's damage, in the order it happened. This is the save format and
/// the wire format: a server sends "this solid, here", and every client derives
/// the same hole.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DamageLog(pub Vec<Damage>);

impl DamageLog {
    /// Append one hit to the end of the log.
    pub fn push(&mut self, damage: Damage) {
        self.0.push(damage);
    }

    /// The number of hits recorded.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the building has taken no damage.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The hits, in the order they happened.
    pub fn iter(&self) -> impl Iterator<Item = &Damage> {
        self.0.iter()
    }

    /// Check every hit against the building, at its own path in the log.
    pub fn check(&self, building: &Building) -> Result<(), ValidationError> {
        for (index, damage) in self.0.iter().enumerate() {
            damage.check(building, &format!("damage[{index}]"))?;
        }
        Ok(())
    }
}

impl<'a> IntoIterator for &'a DamageLog {
    type Item = &'a Damage;
    type IntoIter = std::slice::Iter<'a, Damage>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde dictates the signature"
)]
fn is_false(value: &bool) -> bool {
    !*value
}
