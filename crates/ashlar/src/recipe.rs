use std::{
    collections::{BTreeMap, BTreeSet},
    f64::consts::PI,
};

use glam::{DQuat, DVec3};
use serde::{Deserialize, Serialize};

use crate::geometry::Scope;
use crate::{
    Binding, BoundSlots, Collision, FaceOrigin, Geometry, Pose, UvMode, ValidationError, name,
    require,
};

/// Which side of the building's envelope an element's faces are seen from.
/// Declared, not detected: a liner over the inside of a wall is an element, so
/// its faces already know what they are, and the answer does not change when a
/// blast opens a room to the sky.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub enum Side {
    /// Seen from outside. The default.
    #[default]
    Exterior,
    /// Seen from inside: floors, partitions, liners, ceilings.
    Interior,
}

/// A separately addressable solid within a part.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Element {
    /// Stable name within the part, for diagnostics and future overrides.
    pub id: String,
    /// Solid description in the part's frame.
    pub geometry: Geometry,
    /// Semantic slot resolved by the recipe or an instance override.
    pub material_slot: String,
    /// Optional slot for faces created by subtraction, otherwise the main slot.
    #[serde(default)]
    pub cut_material_slot: Option<String>,
    /// Texture projection for this element's surface.
    #[serde(default)]
    pub uv: UvMode,
    /// What this element contributes to static collision.
    #[serde(default)]
    pub collision: Collision,
    /// Which side of the envelope this element's faces are seen from.
    #[serde(default, skip_serializing_if = "is_exterior")]
    pub side: Side,
    /// Keeps this element its own mesh whatever group its instance is in; for
    /// glass, lights, doors and anything else that moves or is swapped.
    #[serde(default, skip_serializing_if = "is_false")]
    pub standalone: bool,
    /// Kept at every level of detail whatever its size: a light, a sign, a
    /// neon line, anything thin that still reads from afar because it glows.
    /// Its segments are still scaled.
    #[serde(default, skip_serializing_if = "is_false")]
    pub far: bool,
}

impl Element {
    /// Assign a solid to a named surface slot without choosing its material.
    pub fn new(id: impl Into<String>, geometry: Geometry, slot: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            geometry,
            material_slot: slot.into(),
            cut_material_slot: None,
            uv: UvMode::default(),
            collision: Collision::default(),
            side: Side::default(),
            standalone: false,
            far: false,
        }
    }

    /// Declare this element's faces seen from inside the envelope.
    #[must_use]
    pub fn interior(mut self) -> Self {
        self.side = Side::Interior;
        self
    }

    /// Keep this element its own mesh whatever group its instance is in.
    #[must_use]
    pub fn standalone(mut self) -> Self {
        self.standalone = true;
        self
    }

    /// Keep this element at every level of detail, however thin it is: see
    /// [`LodPolicy::min_feature`](crate::LodPolicy::min_feature).
    #[must_use]
    pub fn far(mut self) -> Self {
        self.far = true;
        self
    }

    /// Declare a static collision proxy for this element.
    ///
    /// The default is [`Collision::None`], so trim, slats, lights and signage
    /// cost nothing and say so. A proxy is convex and conservative: an opening
    /// subtracted inside this element is not subtracted from its proxy.
    ///
    /// ```
    /// use ashlar::{Collision, Element, Geometry};
    /// let wall = Element::new("shell", Geometry::cuboid([4.0, 3.0, 0.3]), "surface")
    ///     .collision(Collision::Bounds);
    /// let sign = Element::new("sign", Geometry::cuboid([1.0, 0.4, 0.05]), "accent");
    /// assert_eq!(wall.collision, Collision::Bounds);
    /// assert_eq!(sign.collision, Collision::None);
    /// ```
    #[must_use]
    pub fn collision(mut self, collision: Collision) -> Self {
        self.collision = collision;
        self
    }

    /// Choose how this element's surface is projected into texture space.
    #[must_use]
    pub fn uv(mut self, mode: UvMode) -> Self {
        self.uv = mode;
        self
    }

    /// Give exposed cut faces their own material binding.
    #[must_use]
    pub fn cut_material(mut self, slot: impl Into<String>) -> Self {
        self.cut_material_slot = Some(slot.into());
        self
    }

    pub(crate) fn slots(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.material_slot.as_str())
            .chain(self.cut_material_slot.as_deref())
            .chain(
                self.geometry
                    .cutters()
                    .into_iter()
                    .filter_map(|cutter| cutter.cut_slot.as_deref()),
            )
    }

    /// The slot a face of this element wears, given where the face came from.
    ///
    /// A body face wears the element's main slot. A cutter face wears the
    /// cutter's own slot when it names one, then the element's cut slot, then
    /// the main slot; an index past the end of the element's cutters takes the
    /// same fallback. A damage face wears the main slot: damage names its own
    /// slot on the damage record and is resolved there, so this arm is only the
    /// answer for a caller that has nothing better.
    pub fn slot_for(&self, origin: FaceOrigin) -> &str {
        match origin {
            FaceOrigin::Body | FaceOrigin::Damage(_) => self.material_slot.as_str(),
            FaceOrigin::Cutter(index) => self
                .geometry
                .cutters()
                .get(index as usize)
                .and_then(|cutter| cutter.cut_slot.as_deref())
                .or(self.cut_material_slot.as_deref())
                .unwrap_or(self.material_slot.as_str()),
        }
    }
}

/// Named local frame for later attachment and entrance tooling.
/// Sockets are metadata; they do not automatically connect or snap instances.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Socket {
    /// Unique name within the part.
    pub id: String,
    /// Position and orientation relative to the part origin.
    pub pose: Pose,
}

impl Socket {
    /// Describe an attachment frame without imposing an architectural role.
    pub fn new(id: impl Into<String>, pose: Pose) -> Self {
        Self {
            id: id.into(),
            pose,
        }
    }
}

/// Reusable geometry, independent of any assembly or material palette.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Part {
    /// Recipe-local identity, suitable for a namespaced content key.
    pub id: String,
    /// One or more solids. Elements remain separate, not implicitly unioned.
    pub elements: Vec<Element>,
    /// Attachment frames owned by this definition.
    pub sockets: Vec<Socket>,
}

impl Part {
    /// Start a reusable part definition.
    pub fn builder(id: impl Into<String>) -> PartBuilder {
        PartBuilder(Self {
            id: id.into(),
            elements: Vec::new(),
            sockets: Vec::new(),
        })
    }

    fn validate(&self, path: &str) -> Result<(), ValidationError> {
        name(&self.id, &format!("{path}.id"))?;
        require(
            !self.elements.is_empty(),
            path,
            "part requires at least one element",
        )?;
        let mut ids = BTreeSet::new();
        for element in &self.elements {
            let path = format!("{path}.elements[{}]", element.id);
            unique(&mut ids, &element.id, &path)?;
            name(&element.material_slot, &format!("{path}.material_slot"))?;
            if let Some(slot) = &element.cut_material_slot {
                name(slot, &format!("{path}.cut_material_slot"))?;
            }
            let mut portals = BTreeSet::new();
            element
                .geometry
                .validate(&format!("{path}.geometry"), &mut Scope::new(&mut portals))?;
        }
        let mut ids = BTreeSet::new();
        for socket in &self.sockets {
            let path = format!("{path}.sockets[{}]", socket.id);
            unique(&mut ids, &socket.id, &path)?;
            socket.pose.validate(&path)?;
        }
        Ok(())
    }
}

/// Incremental authoring helper. Finalization validates local geometry and names.
#[derive(Clone, Debug)]
#[must_use]
pub struct PartBuilder(Part);

impl PartBuilder {
    /// Add a separately named solid and its material slot.
    pub fn element(mut self, element: Element) -> Self {
        self.0.elements.push(element);
        self
    }

    /// Add an attachment frame.
    pub fn socket(mut self, socket: Socket) -> Self {
        self.0.sockets.push(socket);
        self
    }

    /// Finish a part. Assembly validation rechecks it after any data edits.
    pub fn build(self) -> Result<Part, ValidationError> {
        self.0.validate(&format!("parts[{}]", self.0.id))?;
        Ok(self.0)
    }
}

/// Which way an attached socket faces relative to the one it lands on.
///
/// Non-exhaustive: a further convention would not break an existing match.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub enum Facing {
    /// The two sockets face each other. Sockets point outward along +Z, so the
    /// attached part is turned half a turn about the socket's Y axis, the way
    /// two modules meet at a shared edge. This is the default.
    #[default]
    Opposed,
    /// The two socket frames coincide exactly, for a socket that marks a
    /// direction to continue in rather than a face to meet.
    Aligned,
}

/// Derive one instance's placement from another instance's socket.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    /// Socket on this instance's own part.
    pub socket: String,
    /// ID of the instance to attach to.
    pub target: String,
    /// Socket on that instance's part.
    pub target_socket: String,
    /// How the two socket frames are brought together.
    #[serde(default)]
    pub facing: Facing,
}

/// A reference to a reusable part, with assembly-local placement and palette.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct Instance {
    /// Unique identity within the building, independent of its part identity.
    pub id: String,
    /// ID of a definition in the same recipe.
    pub part: String,
    /// Placement relative to the building origin. Derived at build time when
    /// [`Self::attach`] is set, and whatever was authored here is replaced.
    pub pose: Pose,
    /// Optional socket attachment. Building resolves it into `pose`.
    #[serde(default)]
    pub attach: Option<Attachment>,
    /// Slot to material binding; takes precedence over recipe bindings.
    ///
    /// A [`Binding`] is an opaque material key, and optionally the graph
    /// parameters this instance overrides on it — which is how two instances
    /// wear two seeds of one material without a second key in the library. In
    /// RON a binding is the key alone where it overrides nothing, so a recipe
    /// written before overrides existed reads and writes unchanged.
    pub materials: BTreeMap<String, Binding>,
    /// The declared group this placement belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

impl Instance {
    /// Reference a part without copying its geometry.
    pub fn new(id: impl Into<String>, part: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            part: part.into(),
            pose: Pose::default(),
            attach: None,
            materials: BTreeMap::new(),
            group: None,
        }
    }

    /// Place this instance in a declared merge group.
    pub fn group(mut self, id: impl Into<String>) -> Self {
        self.group = Some(id.into());
        self
    }

    /// Derive this instance's placement by bringing its own socket onto another
    /// instance's socket, facing it. Replaces any pose set here, and is
    /// replaced by a later [`Self::placed`].
    ///
    /// `build()` resolves attachments in dependency order, so a target may
    /// appear later in the recipe, and reports unknown sockets, unknown
    /// targets, self-attachment and cycles by path.
    ///
    /// ```
    /// use ashlar::{Building, Element, Geometry, Instance, Part, Pose, Socket, glam::DQuat};
    /// use std::f64::consts::FRAC_PI_2;
    ///
    /// let bay = Part::builder("bay")
    ///     .element(Element::new("shell", Geometry::cuboid([4.0, 3.0, 0.3]), "surface"))
    ///     .socket(Socket::new("left", Pose::default().rotated(DQuat::from_rotation_y(-FRAC_PI_2))))
    ///     .socket(Socket::new(
    ///         "right",
    ///         Pose::at([4.0, 0.0, 0.0]).rotated(DQuat::from_rotation_y(FRAC_PI_2)),
    ///     ))
    ///     .build()?;
    /// let wall = Building::builder("wall")
    ///     .part(bay)
    ///     .material("surface", "paint")
    ///     .instance(Instance::new("a", "bay"))
    ///     .instance(Instance::new("b", "bay").attach("left", "a", "right"))
    ///     .build()?;
    ///
    /// let placed = wall.instance("b").expect("resolved").pose.translation;
    /// assert!((placed.x - 4.0).abs() < 1e-9, "one bay along, not stacked");
    /// # Ok::<(), ashlar::ValidationError>(())
    /// ```
    pub fn attach(
        self,
        socket: impl Into<String>,
        target: impl Into<String>,
        target_socket: impl Into<String>,
    ) -> Self {
        self.attached(socket, target, target_socket, Facing::Opposed)
    }

    /// Attach with the two socket frames coinciding instead of facing.
    pub fn attach_aligned(
        self,
        socket: impl Into<String>,
        target: impl Into<String>,
        target_socket: impl Into<String>,
    ) -> Self {
        self.attached(socket, target, target_socket, Facing::Aligned)
    }

    fn attached(
        mut self,
        socket: impl Into<String>,
        target: impl Into<String>,
        target_socket: impl Into<String>,
        facing: Facing,
    ) -> Self {
        self.attach = Some(Attachment {
            socket: socket.into(),
            target: target.into(),
            target_socket: target_socket.into(),
            facing,
        });
        self
    }

    /// Set placement directly; arbitrary unit rotations are accepted. Clears an
    /// attachment, because two sources for one pose is an authoring mistake.
    pub fn placed(mut self, pose: Pose) -> Self {
        self.pose = pose;
        self.attach = None;
        self
    }

    /// Bind or replace one slot on this instance.
    pub fn material(mut self, slot: impl Into<String>, material: impl Into<String>) -> Self {
        self.materials
            .insert(slot.into(), Binding::new(material.into()));
        self
    }

    /// Bind or replace one slot with a binding that may override graph
    /// parameters.
    ///
    /// The per-instance half of a procedural material: the same key on every
    /// wall, and a value of its own per instance.
    ///
    /// ```
    /// use ashlar::{Binding, Instance, ParamValue};
    ///
    /// let annex = Instance::new("annex", "kit:bay").binding(
    ///     "stone",
    ///     Binding::new("metro:stone").param("variation", ParamValue::Float(0.37)),
    /// );
    /// assert_eq!(annex.materials["stone"].material, "metro:stone");
    /// ```
    pub fn binding(mut self, slot: impl Into<String>, binding: Binding) -> Self {
        self.materials.insert(slot.into(), binding);
        self
    }
}

/// A set of placed elements that is unioned into one solid and meshed once.
/// Also the unit a hit re-meshes, so its size bounds the cost of one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeGroup {
    /// Unique within the building. Instances name it.
    pub id: String,
    /// Which storey the group belongs to, for a game that hides or shows by
    /// storey. `None` when the group is not on one, such as paving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storey: Option<i32>,
}

impl MergeGroup {
    /// Declare a group with no storey.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            storey: None,
        }
    }

    /// Put this group on a storey.
    #[must_use]
    pub fn storey(mut self, storey: i32) -> Self {
        self.storey = Some(storey);
        self
    }
}

/// A declared interior volume: data for a game, never geometry.
///
/// A room is not meshed and is not subtracted from any solid, so the convex
/// proxies ADR 0002 derives stay correct. It is a volume an author declares so
/// a game can ask which room a point falls in and decide what to draw; the
/// library publishes it and culls nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Room {
    /// Unique within the building.
    pub id: String,
    /// Axis-aligned extent in the room's own frame, from its origin.
    pub size: [f64; 3],
    /// Where the room's minimum corner stands in building space.
    #[serde(default)]
    pub pose: Pose,
    /// The declared merge group the room belongs to, which gives it a storey.
    /// `None` for a room that spans several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Ids of the portals that open onto this room, as marked with
    /// `Geometry::portal`. Not checked against the parts: a portal id is local
    /// to one geometry and a building may place that part many times, so which
    /// placed portal is meant is the game's to resolve by position.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub portals: Vec<String>,
}

impl Room {
    /// Declare a room with no placement, group or portals.
    pub fn new(id: impl Into<String>, size: [f64; 3]) -> Self {
        Self {
            id: id.into(),
            size,
            pose: Pose::default(),
            group: None,
            portals: Vec::new(),
        }
    }

    /// Place the room's minimum corner in building space.
    #[must_use]
    pub fn placed(mut self, pose: Pose) -> Self {
        self.pose = pose;
        self
    }

    /// Put the room in a declared merge group, which gives it a storey.
    #[must_use]
    pub fn group(mut self, id: impl Into<String>) -> Self {
        self.group = Some(id.into());
        self
    }

    /// Record one portal id that opens onto this room.
    #[must_use]
    pub fn portal(mut self, id: impl Into<String>) -> Self {
        self.portals.push(id.into());
        self
    }

    /// Whether a building-space point lies in the room, boundary included.
    pub fn contains(&self, point: DVec3) -> bool {
        let local = self.pose.inverse().transform_point(point);
        (0..3).all(|axis| local[axis] >= 0.0 && local[axis] <= self.size[axis])
    }

    /// The eight corners in building space, for a game that draws the volume.
    pub fn corners(&self) -> [DVec3; 8] {
        let mut corners = [DVec3::ZERO; 8];
        for (index, corner) in corners.iter_mut().enumerate() {
            let local = DVec3::new(
                if index & 1 == 0 { 0.0 } else { self.size[0] },
                if index & 2 == 0 { 0.0 } else { self.size[1] },
                if index & 4 == 0 { 0.0 } else { self.size[2] },
            );
            *corner = self.pose.transform_point(local);
        }
        corners
    }
}

/// The only schema version this build of the crate understands. A recipe written
/// by a newer version is refused rather than silently reinterpreted.
pub const SCHEMA_VERSION: u32 = 1;

const fn schema_version() -> u32 {
    SCHEMA_VERSION
}

/// Editable and serializable input, intentionally distinct from validated output.
/// The schema is experimental; this is not the existing runtime GLB manifest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildingRecipe {
    /// Schema version of this document. Files written before the field existed
    /// deserialize as [`SCHEMA_VERSION`] 1, which is what they are.
    #[serde(default = "schema_version")]
    pub version: u32,
    /// Content identity for this assembly.
    pub id: String,
    /// Optional authoring grid in metres. A hint, not a snap or quantization rule.
    pub grid: Option<f64>,
    /// Reusable definitions, including optional unused library parts.
    pub parts: Vec<Part>,
    /// Assembly placements in authored order.
    pub instances: Vec<Instance>,
    /// Whether the elements of this building are unioned by group rather than
    /// drawn as the parts they were authored from. Off unless said: a merged
    /// group is a unique mesh, so merging gives up the instancing a repeated kit
    /// relies on.
    #[serde(default, skip_serializing_if = "is_false")]
    pub merged: bool,
    /// Declared groups. An instance that names none is in the default group,
    /// which is the building itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<MergeGroup>,
    /// Declared interior volumes. Data a game queries; never meshed or carved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rooms: Vec<Room>,
    /// Default slot bindings, allowing a palette to be swapped independently.
    ///
    /// The same [`Binding`] an instance overrides with, so a palette may pin a
    /// graph parameter for every instance that does not say otherwise.
    pub materials: BTreeMap<String, Binding>,
}

impl BuildingRecipe {
    /// Validate raw data, including references, before handing it to a backend.
    pub fn build(self) -> Result<Building, ValidationError> {
        require(
            self.version == SCHEMA_VERSION,
            "version",
            &format!("unknown recipe schema version, expected {SCHEMA_VERSION}"),
        )?;
        name(&self.id, "id")?;
        if let Some(grid) = self.grid {
            require(
                grid.is_finite() && grid > 0.0,
                "grid",
                "grid must be finite and positive",
            )?;
        }
        require(
            !self.instances.is_empty(),
            "instances",
            "building requires at least one instance",
        )?;
        bindings(&self.materials, "materials")?;
        let mut ids = BTreeSet::new();
        for part in &self.parts {
            let path = format!("parts[{}]", part.id);
            unique(&mut ids, &part.id, &path)?;
            part.validate(&path)?;
        }
        let mut ids = BTreeSet::new();
        for group in &self.groups {
            unique(&mut ids, &group.id, &format!("groups[{}]", group.id))?;
        }
        let mut index = Index {
            parts: self
                .parts
                .iter()
                .enumerate()
                .map(|(i, part)| (part.id.clone(), i))
                .collect(),
            slots: self
                .parts
                .iter()
                .map(|part| {
                    part.elements
                        .iter()
                        .flat_map(Element::slots)
                        .map(str::to_owned)
                        .collect()
                })
                .collect(),
            instances: BTreeMap::new(),
            groups: self
                .groups
                .iter()
                .enumerate()
                .map(|(i, group)| (group.id.clone(), i))
                .collect(),
        };
        room_volumes(&self.rooms, &index.groups)?;
        let mut ids = BTreeSet::new();
        for (position, instance) in self.instances.iter().enumerate() {
            let path = format!("instances[{}]", instance.id);
            unique(&mut ids, &instance.id, &path)?;
            index.instances.insert(instance.id.clone(), position);
            if let Some(group) = &instance.group {
                require(
                    index.groups.contains_key(group.as_str()),
                    &format!("{path}.group"),
                    &format!("unknown group {group:?}"),
                )?;
            }
            instance.pose.validate(&format!("{path}.pose"))?;
            bindings(&instance.materials, &format!("{path}.materials"))?;
            let Some(part) = index.parts.get(instance.part.as_str()) else {
                return Err(ValidationError {
                    path: format!("{path}.part"),
                    reason: format!("unknown part {:?}", instance.part),
                });
            };
            let slots = &index.slots[*part];
            for slot in instance.materials.keys() {
                require(
                    slots.contains(slot.as_str()),
                    &format!("{path}.materials[{slot}]"),
                    "override names a slot absent from this part",
                )?;
            }
            for slot in slots {
                require(
                    instance.materials.contains_key(slot) || self.materials.contains_key(slot),
                    &format!("{path}.materials[{slot}]"),
                    "material slot has no binding",
                )?;
            }
        }
        let mut recipe = self;
        attachments(&mut recipe, &index)?;
        for instance in &recipe.instances {
            instance
                .pose
                .validate(&format!("instances[{}].pose", instance.id))?;
        }
        Ok(Building { recipe, index })
    }
}

/// Lookup tables built once by [`BuildingRecipe::build`], so a consumer that
/// resolves a material or a part for every instance does not rescan the recipe.
#[derive(Clone, Debug, Default)]
struct Index {
    parts: BTreeMap<String, usize>,
    instances: BTreeMap<String, usize>,
    /// Slot names used by each part, in the same order as `recipe.parts`.
    slots: Vec<BTreeSet<String>>,
    /// Position of each declared group in `recipe.groups`.
    groups: BTreeMap<String, usize>,
}

/// One merge group and the placed elements in it.
#[derive(Clone, Debug)]
pub struct GroupMembers<'a> {
    /// The group's id; the building's id for the default group.
    pub id: &'a str,
    /// The group's storey, if declared.
    pub storey: Option<i32>,
    /// Each placed element: the instance placing it and the element of that
    /// instance's part. Index in this list is the operand index a merged mesh's
    /// `FaceSource` names.
    pub members: Vec<(&'a Instance, &'a Element)>,
}

/// Validated immutable description; construction is only through checked builders
/// or [`BuildingRecipe::build`]. No deserialization path bypasses validation.
#[derive(Clone, Debug, Serialize)]
#[serde(transparent)]
pub struct Building {
    recipe: BuildingRecipe,
    #[serde(skip)]
    index: Index,
}

/// The index is derived from the recipe, so it is not part of a building's identity.
impl PartialEq for Building {
    fn eq(&self, other: &Self) -> bool {
        self.recipe == other.recipe
    }
}

impl Building {
    /// Begin an assembly without prescribing a grid, materials or architecture.
    pub fn builder(id: impl Into<String>) -> BuildingBuilder {
        BuildingBuilder(BuildingRecipe {
            version: SCHEMA_VERSION,
            id: id.into(),
            grid: None,
            parts: Vec::new(),
            instances: Vec::new(),
            merged: false,
            groups: Vec::new(),
            rooms: Vec::new(),
            materials: BTreeMap::new(),
        })
    }

    /// Read the validated recipe without invalidating its guarantees.
    pub fn recipe(&self) -> &BuildingRecipe {
        &self.recipe
    }

    /// Return editable data; call `build` again after making changes.
    pub fn into_recipe(self) -> BuildingRecipe {
        self.recipe
    }

    /// Look up a definition by ID.
    pub fn part(&self, id: &str) -> Option<&Part> {
        self.recipe.parts.get(*self.index.parts.get(id)?)
    }

    /// Look up a placement by ID.
    pub fn instance(&self, id: &str) -> Option<&Instance> {
        self.recipe.instances.get(*self.index.instances.get(id)?)
    }

    /// The group an instance belongs to: the one it names, or the default
    /// group, whose id is the building's own id. `None` for an unknown instance.
    pub fn group_of(&self, instance_id: &str) -> Option<&str> {
        let instance = self.instance(instance_id)?;
        Some(instance.group.as_deref().unwrap_or(self.recipe.id.as_str()))
    }

    /// The storey of the group an instance belongs to, if that group is
    /// declared and has one. The default group has none unless a group is
    /// declared under the building's own id.
    pub fn storey_of(&self, instance_id: &str) -> Option<i32> {
        let group = self.group_of(instance_id)?;
        self.recipe.groups[*self.index.groups.get(group)?].storey
    }

    /// The rooms declared on this building, in authored order.
    pub fn rooms(&self) -> &[Room] {
        &self.recipe.rooms
    }

    /// The first declared room containing a building-space point.
    ///
    /// Rooms may overlap, and declaration order breaks the tie: the earliest
    /// room in the recipe wins.
    pub fn room_at(&self, point: DVec3) -> Option<&Room> {
        self.recipe.rooms.iter().find(|room| room.contains(point))
    }

    /// The storey of the group a room names, if that group is declared and has
    /// one. `None` for an unknown room or a room with no group.
    pub fn room_storey(&self, room_id: &str) -> Option<i32> {
        let room = self.recipe.rooms.iter().find(|room| room.id == room_id)?;
        let group = room.group.as_deref()?;
        self.recipe.groups[*self.index.groups.get(group)?].storey
    }

    /// What a backend unions: for a merged building, every group that has at
    /// least one member, with its members in instance order and then element
    /// order. Standalone elements are members of nothing. Empty when the
    /// building is not merged.
    pub fn merge_groups(&self) -> Vec<GroupMembers<'_>> {
        if !self.recipe.merged {
            return Vec::new();
        }
        let building_id = self.recipe.id.as_str();
        let mut result = Vec::new();
        let default = self.group_members(building_id);
        if !default.is_empty() {
            result.push(GroupMembers {
                id: building_id,
                storey: self
                    .index
                    .groups
                    .get(building_id)
                    .and_then(|index| self.recipe.groups[*index].storey),
                members: default,
            });
        }
        for group in &self.recipe.groups {
            if group.id == building_id {
                continue;
            }
            let members = self.group_members(&group.id);
            if members.is_empty() {
                continue;
            }
            result.push(GroupMembers {
                id: &group.id,
                storey: group.storey,
                members,
            });
        }
        result
    }

    /// The placed, non-standalone elements of one group, in instance order and
    /// then element order.
    fn group_members(&self, id: &str) -> Vec<(&Instance, &Element)> {
        self.recipe
            .instances
            .iter()
            .filter(|instance| self.group_of(&instance.id) == Some(id))
            .flat_map(|instance| {
                let part = self.part(&instance.part).expect("validated part");
                part.elements
                    .iter()
                    .filter(|element| !element.standalone)
                    .map(move |element| (instance, element))
            })
            .collect()
    }

    /// Resolve the effective material key. Unknown instances or slots return None.
    pub fn material(&self, instance_id: &str, slot: &str) -> Option<&str> {
        Some(self.binding(instance_id, slot)?.material.as_str())
    }

    /// Resolve the effective binding: the material key and whatever graph
    /// parameters this instance overrides on it.
    ///
    /// The instance's own binding where it has one, the recipe's default
    /// otherwise, and never a mixture of the two — an instance that overrides a
    /// slot replaces the palette's binding rather than adding to it, exactly as
    /// it replaces the palette's key.
    pub fn binding(&self, instance_id: &str, slot: &str) -> Option<&Binding> {
        let instance = self.instance(instance_id)?;
        let part = *self.index.parts.get(instance.part.as_str())?;
        if !self.index.slots.get(part)?.contains(slot) {
            return None;
        }
        instance
            .materials
            .get(slot)
            .or_else(|| self.recipe.materials.get(slot))
    }

    /// Every slot a building actually draws, as instance id, slot and the
    /// binding in force there.
    ///
    /// The cut and cutter slots included, and a slot named by two elements of one
    /// part yielded twice, because what this enumerates is the slots the parts
    /// declare rather than the keys the palette holds. This is what validation
    /// and a material adapter walk: a slot that no element names binds nothing
    /// and is nobody's startup error, and a palette entry nothing uses is
    /// allowed on purpose.
    pub fn bindings(&self) -> impl Iterator<Item = (&str, &str, &Binding)> {
        self.recipe.instances.iter().flat_map(move |instance| {
            let part = self.part(&instance.part).expect("validated part");
            part.elements.iter().flat_map(move |element| {
                element.slots().map(move |slot| {
                    (
                        instance.id.as_str(),
                        slot,
                        self.binding(&instance.id, slot).expect("validated binding"),
                    )
                })
            })
        })
    }
}

impl BoundSlots for Building {
    fn bound_slots(&self) -> impl Iterator<Item = (String, &Binding)> {
        self.bindings().map(|(instance, slot, binding)| {
            (format!("instances[{instance}].materials[{slot}]"), binding)
        })
    }
}

/// Fluent assembly authoring over the same data used for file interchange.
#[derive(Clone, Debug)]
#[must_use]
pub struct BuildingBuilder(BuildingRecipe);

impl BuildingBuilder {
    /// Record an optional grid hint without changing authored positions.
    pub fn grid(mut self, metres: f64) -> Self {
        self.0.grid = Some(metres);
        self
    }

    /// Register a part definition. Duplicate IDs are errors at build time.
    pub fn part(mut self, part: Part) -> Self {
        self.0.parts.push(part);
        self
    }

    /// Place a part. Forward references are allowed until build time.
    pub fn instance(mut self, instance: Instance) -> Self {
        self.0.instances.push(instance);
        self
    }

    /// Union this building's elements by group instead of drawing the parts
    /// they were authored from.
    pub fn merged(mut self) -> Self {
        self.0.merged = true;
        self
    }

    /// Declare a merge group instances may name.
    pub fn group(mut self, group: MergeGroup) -> Self {
        self.0.groups.push(group);
        self
    }

    /// Declare an interior volume a game may query.
    pub fn room(mut self, room: Room) -> Self {
        self.0.rooms.push(room);
        self
    }

    /// Bind or replace a default material slot using an opaque content key.
    pub fn material(mut self, slot: impl Into<String>, material: impl Into<String>) -> Self {
        self.0
            .materials
            .insert(slot.into(), Binding::new(material.into()));
        self
    }

    /// Bind or replace a default material slot with a binding that may override
    /// graph parameters.
    pub fn binding(mut self, slot: impl Into<String>, binding: Binding) -> Self {
        self.0.materials.insert(slot.into(), binding);
        self
    }

    /// Resolve references and validate the complete description.
    pub fn build(self) -> Result<Building, ValidationError> {
        self.0.build()
    }
}

/// Resolve every attachment into a pose, targets first. Attachments form a
/// forest, so one pass per remaining instance is enough to detect a cycle.
fn attachments(recipe: &mut BuildingRecipe, index: &Index) -> Result<(), ValidationError> {
    let socket = |part: usize, id: &str| -> Option<Pose> {
        recipe.parts[part]
            .sockets
            .iter()
            .find(|socket| socket.id == id)
            .map(|socket| socket.pose)
    };
    let mut pending = Vec::new();
    for (position, instance) in recipe.instances.iter().enumerate() {
        let Some(attach) = &instance.attach else {
            continue;
        };
        let path = format!("instances[{}].attach", instance.id);
        let own_part = index.parts[instance.part.as_str()];
        let own = socket(own_part, &attach.socket).ok_or_else(|| ValidationError {
            path: format!("{path}.socket"),
            reason: format!("part has no socket {:?}", attach.socket),
        })?;
        let Some(&target) = index.instances.get(attach.target.as_str()) else {
            return Err(ValidationError {
                path: format!("{path}.target"),
                reason: format!("unknown instance {:?}", attach.target),
            });
        };
        require(target != position, &path, "instance attaches to itself")?;
        let target_part = index.parts[recipe.instances[target].part.as_str()];
        let other = socket(target_part, &attach.target_socket).ok_or_else(|| ValidationError {
            path: format!("{path}.target_socket"),
            reason: format!("part has no socket {:?}", attach.target_socket),
        })?;
        let facing = match attach.facing {
            Facing::Aligned => Pose::default(),
            Facing::Opposed => Pose::default().rotated(DQuat::from_rotation_y(PI)),
        };
        pending.push((position, target, own.inverse(), other.compose(facing)));
    }
    let mut resolved: BTreeSet<usize> = (0..recipe.instances.len())
        .filter(|i| recipe.instances[*i].attach.is_none())
        .collect();
    while !pending.is_empty() {
        let mut progress = false;
        pending.retain(|(position, target, own, other)| {
            if !resolved.contains(target) {
                return true;
            }
            recipe.instances[*position].pose =
                recipe.instances[*target].pose.compose(*other).compose(*own);
            resolved.insert(*position);
            progress = true;
            false
        });
        if !progress {
            let (position, ..) = pending[0];
            return Err(ValidationError {
                path: format!("instances[{}].attach", recipe.instances[position].id),
                reason: "attachment cycle".into(),
            });
        }
    }
    Ok(())
}

fn unique<'a>(ids: &mut BTreeSet<&'a str>, id: &'a str, path: &str) -> Result<(), ValidationError> {
    name(id, path)?;
    require(ids.insert(id), path, "duplicate identity")
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde dictates the signature"
)]
fn is_false(value: &bool) -> bool {
    !*value
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde dictates the signature"
)]
fn is_exterior(side: &Side) -> bool {
    *side == Side::Exterior
}

fn room_volumes(rooms: &[Room], groups: &BTreeMap<String, usize>) -> Result<(), ValidationError> {
    let mut ids = BTreeSet::new();
    for room in rooms {
        let path = format!("rooms[{}]", room.id);
        unique(&mut ids, &room.id, &path)?;
        require(
            room.size
                .iter()
                .all(|dimension| dimension.is_finite() && *dimension > 0.0),
            &format!("{path}.size"),
            "room dimensions must be finite and positive",
        )?;
        room.pose.validate(&format!("{path}.pose"))?;
        if let Some(group) = &room.group {
            require(
                groups.contains_key(group.as_str()),
                &format!("{path}.group"),
                &format!("unknown group {group:?}"),
            )?;
        }
        for portal in &room.portals {
            name(portal, &format!("{path}.portals"))?;
        }
    }
    Ok(())
}

fn bindings(materials: &BTreeMap<String, Binding>, path: &str) -> Result<(), ValidationError> {
    for (slot, binding) in materials {
        name(slot, path)?;
        let path = format!("{path}[{slot}]");
        name(&binding.material, &path)?;
        // An override's names and values are checked here, where they are
        // written, and against the graph itself by the adapter that holds one:
        // this crate can see that a value is finite and that a name is a name,
        // and cannot see whether the graph declares it.
        for (parameter, value) in &binding.params {
            let path = format!("{path}.params[{parameter}]");
            name(parameter, &path)?;
            require(value.is_finite(), &path, "parameter value must be finite")?;
        }
    }
    Ok(())
}
