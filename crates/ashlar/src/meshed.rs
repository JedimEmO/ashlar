//! What a backend makes of a whole building, and the pieces it is drawn as.
//! The declared output of [`GeometryMesher`](crate::GeometryMesher), beside the
//! port itself: these types name nothing a kernel owns, so the crate that holds
//! the kernel is not the crate that has to define them, and a consumer reading
//! meshed results never compiles one.
use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;

use crate::geometry::boxed;
use crate::{
    Binding, Building, ConvexSolid, Damage, DamageLog, PortalShape, Pose, Side, TriangleMesh,
};

/// Closed-interval overlap of two axis-aligned boxes on all three axes.
fn overlaps(a: [DVec3; 2], b: [DVec3; 2]) -> bool {
    (a[0].cmple(b[1]) & b[0].cmple(a[1])).all()
}

/// An element mesh retains its slot so all instances can share its geometry.
#[derive(Clone, Debug)]
pub struct ElementMesh {
    /// Element identity within its part.
    pub id: String,
    /// Slot resolved by each instance's palette.
    pub material_slot: String,
    /// Whether this batch is cut through: non-empty and every face exposed by a
    /// subtraction. An element may now yield several cut batches, one per slot,
    /// and `material_slot` is what tells them apart.
    pub is_cut: bool,
    /// The declared side of the element this batch came from.
    pub side: Side,
    /// Part-local surface geometry.
    pub mesh: TriangleMesh,
}

/// One element's collision proxy, in the frame of the part that owns it.
#[derive(Clone, Debug)]
pub struct ElementCollider {
    /// Element identity within its part.
    pub id: String,
    /// Convex proxy derived from the same evaluated solid the mesh came from.
    pub solid: ConvexSolid,
}

/// One placed opening, in building space.
#[derive(Clone, Debug, PartialEq)]
pub struct Portal {
    /// The cutter's portal id.
    pub id: String,
    /// The placement it belongs to.
    pub instance: String,
    /// The element whose cutter made it.
    pub element: String,
    /// The rectangle's corners, in building space, in order around it.
    pub corners: [DVec3; 4],
    /// Unit normal of the rectangle, in building space.
    pub normal: DVec3,
    /// The storey of the instance's group, if that group declares one.
    pub storey: Option<i32>,
}

/// One placed collision proxy, in building space.
#[derive(Clone, Debug)]
pub struct InstanceCollider {
    /// Placement the proxy belongs to.
    pub instance: String,
    /// Element within that placement's part.
    pub element: String,
    /// Convex proxy, already posed.
    pub solid: ConvexSolid,
}

/// One placed element of a merge group: what a
/// [`FaceSource::operand`](crate::FaceSource::operand) of the group's meshes
/// indexes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Operand {
    /// The instance placing the element.
    pub instance: String,
    /// The element within that instance's part.
    pub element: String,
}

/// The faces of one merge group that share a binding and a side.
#[derive(Clone, Debug)]
pub struct GroupBatch {
    /// The binding in force on every face here, resolved per face from the
    /// face's own source exactly as [`Building::binding`] resolves it per
    /// element.
    pub binding: Binding,
    /// Every face here came from an element declared on this side; a group is
    /// split by binding AND side, so a wall and its liner wearing one plaster
    /// are still two batches a game can hide separately.
    ///
    /// A face exposed by damage belongs to no element, and such faces are
    /// classed [`Side::Interior`]: what a hole exposes is the inside of
    /// something.
    pub side: Side,
    /// Building-space triangles. Each face keeps its
    /// [`FaceSource`](crate::FaceSource), whose operand indexes
    /// [`MergedGroup::operands`].
    pub mesh: TriangleMesh,
}

/// A triangle-mesh collider, for a group a convex proxy can no longer
/// describe: a convex proxy cannot have a hole in it.
#[derive(Clone, Debug, PartialEq)]
pub struct TriangleCollider {
    /// Building-space vertices, shared between triangles.
    pub positions: Vec<DVec3>,
    /// Three indices per outward-wound triangle.
    pub indices: Vec<u32>,
}

/// One merge group, unioned and meshed once in building space.
#[derive(Clone, Debug)]
pub struct MergedGroup {
    /// The group's id; the building's own id for the default group.
    pub id: String,
    /// The group's declared storey.
    pub storey: Option<i32>,
    /// The placed elements that were unioned, in operand order.
    pub operands: Vec<Operand>,
    /// The surface, split by binding, ordered by binding.
    pub batches: Vec<GroupBatch>,
    /// Axis-aligned bounds of the surface in building space, or `None` for a
    /// group whose union is empty.
    pub bounds: Option<[DVec3; 2]>,
    /// `None` while the group is undamaged and its elements' convex proxies
    /// still stand for it; `Some` once a hit has changed it, built from the
    /// same evaluated solid the batches came from. It keeps that mesh's own
    /// welded vertices, so a cut seam may duplicate a vertex; collision does
    /// not care.
    pub collider: Option<TriangleCollider>,
}

/// Shared part meshes alongside validated assembly placement and socket metadata.
#[derive(Clone, Debug)]
pub struct MeshedBuilding {
    /// Original immutable description; instance transforms are not baked into meshes.
    pub building: Building,
    /// Used parts only, evaluated once per part ID. In a merged building this
    /// holds the standalone elements only, because the rest are in `groups`.
    pub parts: BTreeMap<String, Vec<ElementMesh>>,
    /// Proxies for the elements that declared one, per part ID and in part space.
    pub part_colliders: BTreeMap<String, Vec<ElementCollider>>,
    /// Portals declared by element cutters, per part ID and in part space, each
    /// beside the id of the element that owns the cutter.
    pub part_portals: BTreeMap<String, Vec<(String, PortalShape)>>,
    /// Merged groups, in the order [`Building::merge_groups`] gives. Empty for
    /// an unmerged building, and for a merged one every group with at least one
    /// member is present.
    pub groups: Vec<MergedGroup>,
    /// The log this meshed building reflects; empty for an undamaged one.
    pub damage: DamageLog,
}

/// What a piece is a piece of.
///
/// The identity that cannot stay flat on [`Piece`] once a group is drawn: a
/// merged batch has no single instance, part or element to name, so what it has
/// instead is the group and the batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PieceOrigin<'a> {
    /// One batch of one element of one instance: the unmerged path, and a
    /// standalone element of a merged building.
    Element {
        /// Placement this piece belongs to.
        instance: &'a str,
        /// Part that placement names, and therefore which meshes it shares.
        part: &'a str,
        /// Element within that part.
        element: &'a str,
        /// The slot this batch was resolved to. Two cut batches of one element
        /// differ in this and in nothing else a key could be built from.
        slot: &'a str,
        /// Whether every face in this batch was exposed by a subtraction, so an
        /// element with several cut batches marks each one for itself.
        is_cut: bool,
    },
    /// One batch of a merge group: every face of the group wearing one
    /// binding, whichever instances and elements they came from. The faces
    /// still say which, through their `FaceSource` and `MergedGroup::operands`.
    Group {
        /// The group's id.
        group: &'a str,
        /// Position of the batch in `MergedGroup::batches`.
        batch: usize,
    },
}

/// What uploaded geometry may be shared by.
///
/// Two element pieces over the same part, element and slot are the same
/// triangles; a group batch is shared with nothing, because its geometry exists
/// only in building space and belongs to no reusable part.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MeshKey<'a> {
    /// Geometry of one element batch, keyed by what a part could be placed
    /// again as.
    Element {
        /// Part that names the geometry.
        part: &'a str,
        /// Element within that part.
        element: &'a str,
        /// The slot this batch was resolved to, which is what tells two cut
        /// batches of one element apart.
        slot: &'a str,
    },
    /// Geometry of one merge batch, unique to the building it was unioned in.
    Group {
        /// The group's id.
        group: &'a str,
        /// Position of the batch in `MergedGroup::batches`.
        batch: usize,
    },
}

/// One drawable piece of a meshed building: a batch of triangles, where it
/// stands, and the material binding in force on it.
///
/// Not `Surface`, which in this crate is what a material is *made of*. A piece
/// is a batch of faces that share a binding and a placement — the unit a
/// renderer spawns — and [`PieceOrigin`] says what that batch is a piece of. An
/// element whose cutters name slots of their own is several pieces.
///
/// Borrowed rather than owned throughout, including the mesh: a part is
/// evaluated once however many times it is placed, so two instances of one part
/// yield two pieces over the same triangles, and copying them per placement is
/// the cost this whole shape exists to avoid.
#[derive(Clone, Copy, Debug)]
pub struct Piece<'a> {
    /// What this batch is a piece of.
    pub origin: PieceOrigin<'a>,
    /// Storey of the group the piece belongs to, if it has one and that group
    /// declares one.
    pub storey: Option<i32>,
    /// Which side of the envelope the piece's faces are seen from, from the
    /// batch or the element mesh it came from.
    pub side: Side,
    /// Where the piece stands. An element piece takes its instance's pose over
    /// part-local triangles; a group is meshed in building space, so its
    /// placement is the identity.
    pub pose: Pose,
    /// Geometry, part-local for an element piece and building-space for a group
    /// batch. An element batch is shared with every other placement of its
    /// part; a group batch is shared with nothing.
    pub mesh: &'a TriangleMesh,
    /// The binding in force on every face here.
    pub binding: &'a Binding,
}

impl<'a> Piece<'a> {
    /// What uploaded geometry may be shared by; see [`MeshKey`].
    pub fn mesh_key(&self) -> MeshKey<'a> {
        match self.origin {
            PieceOrigin::Element {
                part,
                element,
                slot,
                ..
            } => MeshKey::Element {
                part,
                element,
                slot,
            },
            PieceOrigin::Group { group, batch } => MeshKey::Group { group, batch },
        }
    }

    /// What every piece drawn from the same geometry *and* the same binding
    /// shares.
    ///
    /// Anything derived from the geometry *and* the material — an uploaded
    /// mesh, a scattered strand layer — is one answer for all of them, because
    /// the placement is the only thing that differs and the placement is a
    /// transform applied afterwards. Growing a lawn once per instance rather
    /// than once per key is the performance cliff this exists to name.
    pub fn shared_key(&self) -> (MeshKey<'a>, &'a Binding) {
        (self.mesh_key(), self.binding)
    }

    /// A readable name for diagnostics and entity names.
    ///
    /// `"{instance}/{element}"` for an element piece, and
    /// `"{group}/{binding material}"` for a group batch.
    pub fn label(&self) -> String {
        match self.origin {
            PieceOrigin::Element {
                instance, element, ..
            } => format!("{instance}/{element}"),
            PieceOrigin::Group { group, .. } => format!("{group}/{}", self.binding.material),
        }
    }
}

impl MeshedBuilding {
    /// Every declared proxy, placed once per instance of its part. A part is
    /// evaluated once however many times it is placed, so this is a transform
    /// per proxy and no further kernel work.
    ///
    /// A proxy whose `(instance, element)` is an operand of a group with a
    /// triangle collider is left out: that group has been hit, its convex proxy
    /// cannot describe the hole, and [`mesh_colliders`](Self::mesh_colliders)
    /// answers for it instead.
    pub fn colliders(&self) -> Vec<InstanceCollider> {
        let replaced: BTreeSet<(&str, &str)> = self
            .groups
            .iter()
            .filter(|group| group.collider.is_some())
            .flat_map(|group| {
                group
                    .operands
                    .iter()
                    .map(|operand| (operand.instance.as_str(), operand.element.as_str()))
            })
            .collect();
        let mut result = Vec::new();
        for instance in &self.building.recipe().instances {
            let Some(colliders) = self.part_colliders.get(&instance.part) else {
                continue;
            };
            result.extend(
                colliders
                    .iter()
                    .filter(|collider| {
                        !replaced.contains(&(instance.id.as_str(), collider.id.as_str()))
                    })
                    .map(|collider| InstanceCollider {
                        instance: instance.id.clone(),
                        element: collider.id.clone(),
                        solid: collider.solid.placed(instance.pose),
                    }),
            );
        }
        result
    }

    /// Group id and collider, for the groups that have a triangle collider.
    ///
    /// A group appears here once a hit has changed it; an undamaged group
    /// answers through [`colliders`](Self::colliders) instead.
    pub fn mesh_colliders(&self) -> impl Iterator<Item = (&str, &TriangleCollider)> {
        self.groups.iter().filter_map(|group| {
            group
                .collider
                .as_ref()
                .map(|collider| (group.id.as_str(), collider))
        })
    }

    /// Every declared portal, placed once per instance of its part. A part is
    /// evaluated once however many times it is placed, so this is a transform
    /// per portal and no further kernel work.
    ///
    /// The corners go through the instance pose; the normal goes through the
    /// pose's rotation alone, because it is a direction and not a point. The
    /// storey is the one the instance's group declares.
    pub fn portals(&self) -> Vec<Portal> {
        let mut result = Vec::new();
        for instance in &self.building.recipe().instances {
            let Some(portals) = self.part_portals.get(&instance.part) else {
                continue;
            };
            let storey = self.building.storey_of(&instance.id);
            result.extend(portals.iter().map(|(element, shape)| {
                Portal {
                    id: shape.id.clone(),
                    instance: instance.id.clone(),
                    element: element.clone(),
                    corners: shape
                        .corners
                        .map(|corner| instance.pose.transform_point(corner)),
                    normal: instance.pose.rotation * shape.normal,
                    storey,
                }
            }));
        }
        result
    }

    /// The merge groups a hit can change: those whose bounds overlap the
    /// damage solid's. A bounds test and no kernel, so a server that trusts its
    /// clients can still say which chunks a hit invalidates. Conservative: a
    /// group named here may turn out untouched once the solid is actually
    /// subtracted.
    pub fn groups_touched(&self, damage: &Damage) -> Vec<&str> {
        let Some(bounds) = damage.bounds() else {
            return Vec::new();
        };
        self.groups
            .iter()
            .filter(|group| group.bounds.is_some_and(|group| overlaps(group, bounds)))
            .map(|group| group.id.as_str())
            .collect()
    }

    /// The standalone pieces inside a hit's bounds, as (instance, element).
    /// Damage cuts merged groups only; whether glass shatters or a door falls
    /// off is the game's, and this is what it needs to decide.
    pub fn standalone_touched(&self, damage: &Damage) -> Vec<(&str, &str)> {
        let Some(bounds) = damage.bounds() else {
            return Vec::new();
        };
        let mut touched: Vec<(&str, &str)> = Vec::new();
        for piece in self.pieces() {
            let PieceOrigin::Element {
                instance, element, ..
            } = piece.origin
            else {
                continue;
            };
            let Some(piece_bounds) = boxed(
                piece
                    .mesh
                    .positions
                    .iter()
                    .map(|position| piece.pose.transform_point(*position)),
            ) else {
                continue;
            };
            if overlaps(piece_bounds, bounds) && !touched.contains(&(instance, element)) {
                touched.push((instance, element));
            }
        }
        touched
    }

    /// Put a re-meshed group in place of the one with the same id, and record
    /// the hit that caused it. Returns false, changing nothing, when no group
    /// has that id.
    ///
    /// The log is not touched here: a hit that changes three groups is one
    /// record, and [`record`](Self::record) is what adds it, once.
    pub fn replace_group(&mut self, group: MergedGroup) -> bool {
        let Some(existing) = self
            .groups
            .iter_mut()
            .find(|existing| existing.id == group.id)
        else {
            return false;
        };
        *existing = group;
        true
    }

    /// Append one hit to this building's damage log, in the order it happened.
    ///
    /// Separate from [`replace_group`](Self::replace_group) so a hit that
    /// changes several groups joins the log once.
    pub fn record(&mut self, damage: Damage) {
        self.damage.push(damage);
    }

    /// Every drawable piece: every group's batches first, in group order and
    /// batch order, then the standalone element pieces in instance order and
    /// then element order.
    ///
    /// A group is unioned and meshed in building space, so its batches carry
    /// the identity pose and a binding resolved per face; an element batch is
    /// part-local and carries the pose of the instance placing it.
    ///
    /// This is the join between a meshed building and its materials, and the
    /// walk every renderer would otherwise write by hand: the slot an element
    /// declared is resolved against the palette of the instance placing it, so
    /// what comes back is geometry with a [`Binding`] already on it rather than
    /// a slot name to look up.
    ///
    /// Empty batches are yielded like any other. A fully subtracted element
    /// still binds a material, and whether a renderer wants to skip it depends
    /// on what it is doing with it — uploading nothing, but still creating the
    /// material the palette names.
    pub fn pieces(&self) -> impl Iterator<Item = Piece<'_>> {
        let groups = self.groups.iter().flat_map(|group| {
            group
                .batches
                .iter()
                .enumerate()
                .map(move |(batch, batch_mesh)| Piece {
                    origin: PieceOrigin::Group {
                        group: &group.id,
                        batch,
                    },
                    storey: group.storey,
                    side: batch_mesh.side,
                    pose: Pose::default(),
                    mesh: &batch_mesh.mesh,
                    binding: &batch_mesh.binding,
                })
        });
        let elements = self
            .building
            .recipe()
            .instances
            .iter()
            .flat_map(move |instance| {
                // A part with no meshed entry is one nothing placed, which this
                // walk cannot reach, or a hand-assembled value whose parts and
                // recipe disagree. Neither is worth a panic in a draw loop.
                self.parts
                    .get(&instance.part)
                    .map_or(&[][..], Vec::as_slice)
                    .iter()
                    .map(move |element| Piece {
                        origin: PieceOrigin::Element {
                            instance: &instance.id,
                            part: &instance.part,
                            element: &element.id,
                            slot: &element.material_slot,
                            is_cut: element.is_cut,
                        },
                        storey: self.building.storey_of(&instance.id),
                        side: element.side,
                        pose: instance.pose,
                        mesh: &element.mesh,
                        binding: self
                            .building
                            .binding(&instance.id, &element.material_slot)
                            .expect("validated binding"),
                    })
            });
        groups.chain(elements)
    }
}
