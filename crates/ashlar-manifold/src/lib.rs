//! Solid evaluation adapter. The recipe domain and returned meshes have no Bevy
//! or native-kernel types. Evaluation is synchronous for offline authoring tools.
//!
//! This crate sits between [`ashlar`], which describes buildings, and a
//! renderer such as `ashlar-bevy`, which uploads them. It is the only member
//! that carries a native dependency, so a consumer that only reads fitted
//! results never compiles C++.
//!
//! # Meshing a building
//!
//! Each used part is evaluated once however many times it is placed, and the
//! collision proxies its elements declared come back from the same evaluated
//! solid the triangles did.
//!
//! ```
//! use ashlar::{Building, Collision, Element, Geometry, Instance, Part, Pose};
//! use ashlar_manifold::{ManifoldMesher, mesh_building};
//!
//! let bay = Part::builder("example:bay")
//!     .element(
//!         Element::new(
//!             "shell",
//!             Geometry::cuboid([4.0, 3.0, 0.3]).subtract(
//!                 Geometry::cuboid([1.5, 2.2, 0.5]).placed(Pose::at([1.25, 0.0, -0.1])),
//!             ),
//!             "surface",
//!         )
//!         .collision(Collision::Bounds),
//!     )
//!     .build()?;
//! let building = Building::builder("example:wall")
//!     .part(bay)
//!     .material("surface", "example:painted_metal")
//!     .instance(Instance::new("first", "example:bay"))
//!     .instance(Instance::new("second", "example:bay").placed(Pose::at([4.0, 0.0, 0.0])))
//!     .build()?;
//!
//! let meshed = mesh_building(&building, &ManifoldMesher::default())?;
//!
//! // One shared part, evaluated once, with the opening cut out of it.
//! assert_eq!(meshed.parts.len(), 1);
//! let shell = &meshed.parts["example:bay"][0];
//! assert!(shell.mesh.triangle_count() > 12, "a plain box would be twelve");
//! assert!(shell.mesh.cut_faces().any(|cut| cut), "the reveal is marked");
//!
//! // One declared proxy, placed once per instance.
//! assert_eq!(meshed.part_colliders["example:bay"].len(), 1);
//! let placed = meshed.colliders();
//! assert_eq!(placed.len(), 2);
//! assert_eq!(placed[0].solid.vertices.len(), 8, "an axis-aligned box");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! A recipe that declares `merged` changes the shape of the answer, not the
//! parts: each element is still evaluated alone because its collision proxy
//! comes from that mesh, but only standalone elements are drawn from `parts`,
//! and every merge group is unioned once in building space. Its surface comes
//! back in [`MeshedBuilding::groups`] as batches of faces that share a binding,
//! each face still naming the element it came from through its operand.
//!
//! # Meshing a group
//!
//! A group is several placed solids unioned into one surface in the group's
//! frame, with each damage cut subtracted after them.
//! [`mesh_group`](ashlar::GeometryMesher::mesh_group) is stateless: the same
//! placements and cuts give the same solid, so a load, a late-joining client
//! and a server replaying a damage list all derive it. Every face names the
//! operand it came from.
//!
//! ```
//! use ashlar::{Geometry, GeometryMesher, PlacedGeometry, Pose};
//! use ashlar_manifold::ManifoldMesher;
//!
//! let cube = Geometry::cuboid([1.0; 3]);
//! let first = PlacedGeometry {
//!     geometry: &cube,
//!     pose: Pose::default(),
//! };
//! let second = PlacedGeometry {
//!     geometry: &cube,
//!     pose: Pose::at([1.0, 0.0, 0.0]),
//! };
//!
//! let group = ManifoldMesher::default().mesh_group(&[first, second], &[])?;
//!
//! let operands: std::collections::BTreeSet<u32> =
//!     group.mesh.sources.iter().map(|source| source.operand).collect();
//! assert!(operands.contains(&0) && operands.contains(&1));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Building this crate
//!
//! The kernel is native. A first build clones and compiles upstream Manifold,
//! which needs Git, `cmake` and a C++ compiler and is not covered by
//! `cargo fetch`. Point `MANIFOLD_CSG_LIB_DIR` at a prebuilt install to skip
//! both the clone and the compile. Documentation is the exception: the sys
//! crate skips its build script under `DOCS_RS` and rustdoc links nothing, so
//! docs.rs builds this crate as it stands.

use std::collections::BTreeMap;

use ashlar::{
    Binding, Building, Collision, ConvexSolid, DamageLog, Element, ElementCollider, ElementMesh,
    FaceOrigin, FaceSource, Geometry, GeometryMesher, GroupBatch, GroupMembers, GroupMesh,
    MergedGroup, MeshError, MeshedBuilding, Operand, PlacedCut, PlacedGeometry, PortalShape, Pose,
    Shape, Side, TriangleCollider, TriangleMesh, UvMode,
};
use glam::{DMat3, DVec3};
use manifold_csg::{CrossSection, FillRule, Manifold, MeshGL64, MeshGL64Options};

mod bake;
mod solids;

pub use bake::bake;
pub use solids::{Debris, DebrisKind, GroupHit, GroupSolids};

/// Tessellation policy, independent of architectural style.
#[derive(Clone, Copy, Debug)]
pub struct ManifoldMesher {
    /// Edges at or above this angle retain a shading crease, in degrees. An
    /// edge at exactly the angle is a crease.
    pub sharp_angle: f64,
    /// Maximum authored cylinder subdivision count.
    pub max_segments: u32,
    /// Maximum triangles after each solid operation, before copying vertex data.
    pub max_triangles: usize,
    /// Distance within which output vertices are shared. Zero keeps one vertex
    /// per triangle corner.
    pub weld_tolerance: f64,
}

impl Default for ManifoldMesher {
    fn default() -> Self {
        Self {
            sharp_angle: 45.0,
            max_segments: 4096,
            max_triangles: 1_000_000,
            weld_tolerance: ashlar::WELD_TOLERANCE,
        }
    }
}

fn error(path: &str, reason: impl ToString) -> MeshError {
    MeshError {
        path: path.into(),
        reason: reason.to_string(),
    }
}

fn matrix(pose: Pose) -> [f64; 12] {
    let rotation = DMat3::from_quat(pose.rotation).to_cols_array();
    let mut matrix = [0.0; 12];
    matrix[..9].copy_from_slice(&rotation);
    matrix[9..].copy_from_slice(&pose.translation.to_array());
    matrix
}

/// Check a group placement without the domain's private pose validator:
/// translation and rotation finite, and the rotation's length within 1e-6 of 1.
fn check_pose(pose: Pose, path: &str) -> Result<(), MeshError> {
    if pose.translation.is_finite()
        && pose.rotation.is_finite()
        && (pose.rotation.length() - 1.0).abs() < 1e-6
    {
        Ok(())
    } else {
        Err(error(path, "pose must be finite with a unit rotation"))
    }
}

/// How far beyond the core a piercing probe reaches, in metres.
///
/// It has to be small: a door facing another wall a metre away must not see
/// that wall, or the doorway would read as a recess.
const PROBE: f64 = 1e-2;

/// How far inside the core's cross-section a piercing probe stops, in metres.
///
/// Shrinking the probe off the opening's own jambs keeps their faces from
/// counting as material beyond the axis under test.
const JAMB_CLEARANCE: f64 = 1e-6;

/// The rectangle one marked cutter opens through the solid it cuts.
///
/// Both manifolds are in the difference's frame. Working in the cutter's own
/// frame keeps its box tight however the cutter was rotated into the wall, so
/// the solid is carried there too. The opening is the core, `solid ∩ cutter`:
/// the material the cutter removes, not the whole solid. An axis is pierced
/// when the solid stops at the core at both of its ends, and the rectangle is
/// the core's extent on the other two axes at its mid-point there. An empty
/// core or no pierced axis is the same refusal: a cutter that does not pass
/// through the solid is not an opening.
fn portal_shape(
    id: &str,
    solid: &Manifold,
    cutter: &Manifold,
    cutter_geometry: &Geometry,
    path: &str,
) -> Result<PortalShape, MeshError> {
    let to_cutter = matrix(cutter_geometry.pose.inverse());
    let solid = solid.transform(&to_cutter);
    let cutter = cutter.transform(&to_cutter);
    // Whatever else the solid is, its bounds around the opening are the core's.
    let core = solid.intersection(&cutter);
    let Some(core_box) = core.bounding_box() else {
        return Err(error(path, "portal cutter does not pass through the solid"));
    };
    let core_min = DVec3::from_array(core_box.min());
    let core_max = DVec3::from_array(core_box.max());
    // An axis is pierced when the solid carries on at neither end of the core.
    // A probe holds the core's cross-section, a hair small so it cannot graze
    // the opening's own jambs, and sticks out a little past both ends on the
    // axis; the intersection then reaches further than the core on that axis
    // exactly when there is material beyond it. Several axes can be pierced
    // where the solid is a thin fin; the thinnest core is the wall's thickness.
    let mut pierced = None;
    for axis in 0..3 {
        let mut lo = core_min;
        let mut hi = core_max;
        lo[axis] -= PROBE;
        hi[axis] += PROBE;
        for other in 0..3 {
            if other != axis {
                lo[other] += JAMB_CLEARANCE;
                hi[other] -= JAMB_CLEARANCE;
            }
        }
        let size = hi - lo;
        let probe = Manifold::cube(size.x, size.y, size.z, false).translate(lo.x, lo.y, lo.z);
        let Some(beyond) = solid.intersection(&probe).bounding_box() else {
            continue;
        };
        let extent = core_max[axis] - core_min[axis];
        let reached = beyond.max()[axis] - beyond.min()[axis];
        // No further than the core: a probe held a hair inside a curved wall's
        // jambs meets that wall's face a hair short of where the core's box
        // ends, so the probe reaching *less* is still a wall that stops.
        if reached <= extent + 1e-9 && pierced.is_none_or(|(_, best): (usize, f64)| extent < best) {
            pierced = Some((axis, extent));
        }
    }
    let Some((axis, _)) = pierced else {
        return Err(error(path, "portal cutter does not pass through the solid"));
    };
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (2, 0),
        _ => (0, 1),
    };
    let lo_u = core_min[u];
    let hi_u = core_max[u];
    let lo_v = core_min[v];
    let hi_v = core_max[v];
    if hi_u <= lo_u || hi_v <= lo_v {
        return Err(error(path, "portal cutter does not pass through the solid"));
    }
    // The core's mid-plane in the cutter's frame, mapped back with the cutter.
    let plane = f64::midpoint(core_min[axis], core_max[axis]);
    let corner = |a: f64, b: f64| {
        let mut local = [0.0; 3];
        local[u] = a;
        local[v] = b;
        local[axis] = plane;
        cutter_geometry
            .pose
            .transform_point(DVec3::from_array(local))
    };
    let corners = [
        corner(lo_u, lo_v),
        corner(hi_u, lo_v),
        corner(hi_u, hi_v),
        corner(lo_u, hi_v),
    ];
    let mut axis_unit = [0.0; 3];
    axis_unit[axis] = 1.0;
    Ok(PortalShape {
        id: id.to_owned(),
        corners,
        normal: cutter_geometry.pose.rotation * DVec3::from_array(axis_unit),
    })
}

/// Which origin each kernel original ID stands for, and the next cutter index
/// to hand out. An ID absent from the map is the body.
#[derive(Default)]
struct Origins {
    by_id: BTreeMap<u32, FaceOrigin>,
    next_cutter: u32,
    /// Portals contributed so far, in the frame of the geometry currently being
    /// evaluated. An ancestor applies its own pose to each in turn.
    portals: Vec<PortalShape>,
}

/// Whether the geometry being evaluated is the operand's own solid or lies
/// under cutter root `k`.
#[derive(Clone, Copy)]
enum Role {
    Body,
    Cutter(u32),
}

impl ManifoldMesher {
    /// Subtract each cutter in turn, publishing a portal for every marked
    /// cutter root. A portal is measured against the solid the cutter opens,
    /// before that cutter is subtracted.
    fn evaluate_difference(
        &self,
        path: &str,
        solid: &Geometry,
        cutters: &[Geometry],
        origins: &mut Origins,
        role: Role,
    ) -> Result<Manifold, MeshError> {
        let mut result = self.evaluate(solid, &format!("{path}.solid"), origins, role)?;
        for (index, cutter) in cutters.iter().enumerate() {
            let cutter_path = format!("{path}.cutters[{index}]");
            let cutter_role = match role {
                Role::Body => {
                    let k = origins.next_cutter;
                    origins.next_cutter += 1;
                    Role::Cutter(k)
                }
                Role::Cutter(k) => Role::Cutter(k),
            };
            let cutter_solid = self.evaluate(cutter, &cutter_path, origins, cutter_role)?;
            // A portal belongs to a cutter root, so only a difference met as an
            // ordinary solid publishes one.
            if matches!(role, Role::Body)
                && let Some(id) = &cutter.portal
            {
                origins.portals.push(portal_shape(
                    id,
                    &result,
                    &cutter_solid,
                    cutter,
                    &cutter_path,
                )?);
            }
            result = result.difference(&cutter_solid);
            self.check_solid(&result, path)?;
        }
        Ok(result)
    }

    /// A profile turned about Y. The kernel revolves the X/Y plane about Y
    /// and lays the profile's Y along Z, as its cylinder does; the same quarter
    /// turn stands it back up on Y.
    /// A cylinder on Y, within the segment budget.
    fn cylinder(
        &self,
        radius: f64,
        height: f64,
        segments: u32,
        path: &str,
    ) -> Result<Manifold, MeshError> {
        if segments > self.max_segments {
            return Err(error(path, "cylinder exceeds segment budget"));
        }
        let count = i32::try_from(segments).map_err(|e| error(path, e))?;
        Ok(Manifold::cylinder(height, radius, radius, count, false).rotate(-90.0, 0.0, 0.0))
    }

    fn revolve(
        &self,
        profile: &[[f64; 2]],
        segments: u32,
        sweep: f64,
        path: &str,
    ) -> Result<Manifold, MeshError> {
        if segments > self.max_segments {
            return Err(error(path, "revolve exceeds segment budget"));
        }
        let count = i32::try_from(segments).map_err(|e| error(path, e))?;
        let section =
            CrossSection::from_polygons_with_fill_rule(&[profile.to_vec()], FillRule::EvenOdd);
        Ok(Manifold::revolve(&section, count, sweep).rotate(-90.0, 0.0, 0.0))
    }

    fn evaluate(
        &self,
        geometry: &Geometry,
        path: &str,
        origins: &mut Origins,
        role: Role,
    ) -> Result<Manifold, MeshError> {
        let first = origins.portals.len();
        let solid = match &geometry.shape {
            Shape::ChamferedCuboid { size, bevel } => chamfered_box(*size, *bevel),
            Shape::Cuboid { size: [x, y, z] } => Manifold::cube(*x, *y, *z, false),
            Shape::Cylinder {
                radius,
                height,
                segments,
            } => self.cylinder(*radius, *height, *segments, path)?,
            Shape::Extrusion { profile, depth } => {
                // Map X/Z to the kernel's X/Y plane, then rotate Z extrusion to +Y.
                // Negating the second profile coordinate preserves authored Z.
                let ring = profile.iter().map(|[x, z]| [*x, -*z]).collect();
                CrossSection::from_polygons_with_fill_rule(&[ring], FillRule::EvenOdd)
                    .extrude(*depth)
                    .rotate(-90.0, 0.0, 0.0)
            }
            Shape::Revolve {
                profile,
                segments,
                sweep,
            } => self.revolve(profile, *segments, *sweep, path)?,
            Shape::Hull { points } => Manifold::hull_pts(points),
            Shape::Difference { solid, cutters } => {
                self.evaluate_difference(path, solid, cutters, origins, role)?
            }
            Shape::Union { solids } => {
                let mut operands = Vec::with_capacity(solids.len());
                for (index, solid) in solids.iter().enumerate() {
                    operands.push(self.evaluate(
                        solid,
                        &format!("{path}.solids[{index}]"),
                        origins,
                        role,
                    )?);
                }
                let result = Manifold::batch_union(&operands);
                self.check_solid(&result, path)?;
                result
            }
            Shape::Array { solid, count, step } => {
                let source = self.evaluate(solid, &format!("{path}.solid"), origins, role)?;
                // Validation refuses a portal under an array, whose copies would
                // not share one rigid frame; this is the backend's backstop.
                if origins.portals.len() != first {
                    return Err(error(
                        path,
                        "a portal cannot sit under an array or a mirror",
                    ));
                }
                let mut copies = Vec::with_capacity(*count as usize);
                let mut placement = Pose::default();
                for _ in 0..*count {
                    copies.push(source.transform(&matrix(placement)));
                    placement = placement.compose(*step);
                }
                // Copies of one solid share an original ID, so a cutter inside
                // the source marks the cut faces of every copy at once.
                let result = match copies.len() {
                    1 => copies.remove(0),
                    _ => Manifold::batch_union(&copies),
                };
                self.check_solid(&result, path)?;
                result
            }
            Shape::Mirror { solid, plane } => {
                let source = self.evaluate(solid, &format!("{path}.solid"), origins, role)?;
                if origins.portals.len() != first {
                    return Err(error(
                        path,
                        "a portal cannot sit under an array or a mirror",
                    ));
                }
                let normal = plane.axis.unit();
                let to_plane = normal * plane.offset;
                // The kernel mirrors through the origin, and flips the winding
                // itself because the reflection has a negative determinant.
                let reflected = source
                    .translate(-to_plane.x, -to_plane.y, -to_plane.z)
                    .mirror(normal.to_array())
                    .translate(to_plane.x, to_plane.y, to_plane.z);
                self.check_solid(&reflected, path)?;
                // The kernel flips the winding for us, but it was the kernel's
                // own normals that needed the rebuild: a reflection leaves its
                // property association crossed, so normals calculated directly
                // on the result belong to the wrong faces. We derive normals
                // ourselves now, but the rebuild stays, because it keeps the
                // surface IDs the cut slots are matched against and the
                // cut-provenance tests were written against it.
                let result = Manifold::from_meshgl64(&reflected.to_meshgl64())
                    .map_err(|e| error(path, e))?;
                self.check_solid(&result, path)?;
                result
            }
            // `Shape` is non-exhaustive so the domain can grow a node before
            // every backend implements it; refusing by path beats a panic.
            other => return Err(error(path, format!("unsupported shape {other:?}"))),
        }
        .transform(&matrix(geometry.pose));
        self.check_solid(&solid, path)?;
        // A portal this subtree contributed is still in the child's frame, so it
        // gets the same pose the solid did. An ancestor transforms it again,
        // which is how each level of nesting composes.
        let pose = geometry.pose;
        for portal in &mut origins.portals[first..] {
            portal.corners = portal.corners.map(|corner| pose.transform_point(corner));
            portal.normal = pose.rotation * portal.normal;
        }
        if let Role::Cutter(k) = role
            && !matches!(geometry.shape, Shape::Difference { .. })
        {
            for id in solid.to_meshgl64().run_original_id() {
                origins.by_id.insert(id, FaceOrigin::Cutter(k));
            }
        }
        Ok(solid)
    }

    /// Union placed solids, answering the solid and the table that names an
    /// operand for every kernel original ID in it.
    ///
    /// This is the union half of [`mesh_group`](GeometryMesher::mesh_group),
    /// split out so a kept solid can be unioned once and then cut. Each operand
    /// is evaluated with its own `Origins`: a fresh evaluation builds fresh
    /// primitives, a fresh primitive gets a fresh original ID, and that is what
    /// makes an ID name exactly one operand.
    fn union_operands(
        &self,
        solids: &[PlacedGeometry<'_>],
    ) -> Result<(Manifold, BTreeMap<u32, FaceSource>), MeshError> {
        let mut sources: BTreeMap<u32, FaceSource> = BTreeMap::new();
        let mut operands = Vec::with_capacity(solids.len());
        for (i, placed) in solids.iter().enumerate() {
            let operand_index = u32::try_from(i).map_err(|e| error(&format!("solids[{i}]"), e))?;
            let mut origins = Origins::default();
            let operand = self
                .evaluate(
                    placed.geometry,
                    &format!("solids[{i}]"),
                    &mut origins,
                    Role::Body,
                )?
                .transform(&matrix(placed.pose));
            for id in operand.to_meshgl64().run_original_id() {
                let source = FaceSource {
                    operand: operand_index,
                    origin: origins.by_id.get(&id).copied().unwrap_or_default(),
                };
                if sources
                    .get(&id)
                    .is_some_and(|seen| seen.operand != operand_index)
                {
                    return Err(error(
                        &format!("solids[{i}]"),
                        "kernel reused an original ID across operands",
                    ));
                }
                sources.insert(id, source);
            }
            operands.push(operand);
        }
        let result = match operands.len() {
            0 => Manifold::empty(),
            1 => operands.remove(0),
            _ => Manifold::batch_union(&operands),
        };
        self.check_solid(&result, "group")?;
        Ok((result, sources))
    }

    /// Evaluate one cut and record every face it contributes as
    /// `Damage(index)`, answering the solid and its slice of the ID table.
    ///
    /// This is the subtraction half of
    /// [`mesh_group`](GeometryMesher::mesh_group): a face the cut exposes is a
    /// damage face, and `mesh_group` numbers them by position among the cuts it
    /// was handed.
    fn evaluate_cut(
        &self,
        placed: &PlacedCut<'_>,
        index: u32,
        path: &str,
    ) -> Result<(Manifold, BTreeMap<u32, FaceSource>), MeshError> {
        let mut origins = Origins::default();
        let cut = self
            .evaluate(placed.geometry, path, &mut origins, Role::Body)?
            .transform(&matrix(placed.pose));
        let source = FaceSource {
            operand: FaceSource::NO_OPERAND,
            origin: FaceOrigin::Damage(index),
        };
        let mut sources = BTreeMap::new();
        for id in cut.to_meshgl64().run_original_id() {
            sources.insert(id, source);
        }
        Ok((cut, sources))
    }

    fn check_solid(&self, solid: &Manifold, path: &str) -> Result<(), MeshError> {
        solid.status().map_err(|e| error(path, e))?;
        if solid.num_tri() > self.max_triangles {
            return Err(error(path, "solid exceeds triangle budget"));
        }
        Ok(())
    }
}

/// The loose pieces a solid already holds, as `(volume, bounds)`.
///
/// A piece is loose when the lowest Y of its bounds is more than 1e-3 m above
/// the group's base. This is the one-group rule [`mesh_group`] applies, where
/// reaching the base and being supported are the same thing; a whole building
/// asks the cross-group question in `solids`.
///
/// [`mesh_group`]: GeometryMesher::mesh_group
fn loose_pieces(solid: &Manifold, base: f64) -> Vec<(f64, [[f64; 3]; 2])> {
    let mut loose = Vec::new();
    for piece in solid.decompose() {
        if piece.is_empty() {
            continue;
        }
        if let Some(bounds) = piece.bounding_box()
            && (bounds.min()[1] - base).abs() > 1e-3
        {
            loose.push((piece.volume(), [bounds.min(), bounds.max()]));
        }
    }
    loose
}

/// Whether two `(volume, bounds)` signatures are the same piece: volumes agree
/// within 1e-9 m^3 and every one of the six bounds agrees within 1e-9 m.
fn same_piece(left: (f64, [[f64; 3]; 2]), right: (f64, [[f64; 3]; 2])) -> bool {
    (left.0 - right.0).abs() <= 1e-9
        && left
            .1
            .iter()
            .zip(right.1)
            .all(|(a, b)| a.iter().zip(b).all(|(x, y)| (x - y).abs() <= 1e-9))
}

/// Rebuild a solid with one run per original ID that still has triangles.
///
/// A decomposed piece carries the whole solid's run table, so recomposing the
/// kept pieces after a collapse copies that table once per piece: on the
/// storey-merged tower one kept solid went from 83 runs to 367708 across eight
/// collapsing hits while holding near 8,500 triangles, and every later boolean,
/// `decompose` and extraction walked all of them. One run per ID the solid
/// actually shows keeps the table bounded by the faces that are there.
fn compact_runs(solid: &Manifold) -> Result<Manifold, MeshError> {
    let mesh = solid.to_meshgl64();
    let stride = mesh.num_prop();
    let vertices = mesh.vert_properties();
    let indices = mesh.tri_verts();
    if indices.is_empty() {
        return Ok(solid.clone());
    }
    let mut run_index = mesh.run_index();
    let run_ids = mesh.run_original_id();
    // A mesh without the sentinel would drop its last run, so close the list.
    if run_index.len() == run_ids.len() {
        run_index.push(u64::try_from(indices.len()).map_err(|e| error("group", e))?);
    }
    // One entry per ID, in ID order, so the rebuilt table is deterministic.
    let mut by_id: BTreeMap<u32, Vec<u64>> = BTreeMap::new();
    for (range, id) in run_index.windows(2).zip(&run_ids) {
        let start = usize::try_from(range[0]).map_err(|e| error("group", e))?;
        let end = usize::try_from(range[1]).map_err(|e| error("group", e))?;
        let triangles = indices
            .get(start..end)
            .ok_or_else(|| error("group", "kernel run runs past its triangle list"))?;
        by_id.entry(*id).or_default().extend_from_slice(triangles);
    }
    if by_id.is_empty() {
        return Ok(solid.clone());
    }
    let mut tris = Vec::with_capacity(indices.len());
    let mut new_index = Vec::with_capacity(by_id.len() + 1);
    let mut new_ids = Vec::with_capacity(by_id.len());
    for (id, triangles) in &by_id {
        new_index.push(u64::try_from(tris.len()).map_err(|e| error("group", e))?);
        new_ids.push(*id);
        tris.extend_from_slice(triangles);
    }
    new_index.push(u64::try_from(tris.len()).map_err(|e| error("group", e))?);
    let mut options = MeshGL64Options::new().runs(&new_index, &new_ids);
    // Merge vectors exist only when the source carried extra properties, and
    // passing them through keeps a welded mesh welded.
    let merge_from = mesh.merge_from_vert();
    let merge_to = mesh.merge_to_vert();
    if !merge_from.is_empty() {
        options = options.merge_vertices(&merge_from, &merge_to);
    }
    let rebuilt = MeshGL64::new_with_options(&vertices, stride, &tris, options)
        .map_err(|e| error("group", e))?;
    Manifold::from_meshgl64(&rebuilt).map_err(|e| error("group", e))
}

/// Returns what is kept and the pieces that fell for a collapsing difference,
/// within one group. This is the port's one-group rule; a whole building asks
/// the cross-group version in `solids`, because a piece can be held up by a
/// piece of another group.
///
/// A piece is kept when the lowest Y of its bounds is within 1e-3 m of the
/// group's base, or when `already_loose` names it and the cut left it
/// untouched. `already_loose` is the loose pieces' `(volume, bounds)` list from
/// before the cut, and it is consulted only once a loose piece survives, so a
/// caller can pass its cache and skip decomposing the solid before the cut when
/// nothing is hanging: a piece the cut created, or a loose piece the cut bit
/// into, falls, but a loose piece the cut never touched stays where its author
/// put it. `compose` puts the kept pieces back together without a boolean, so
/// their original IDs, and the face sources that name them, survive. When
/// nothing falls the solid is handed back as it was, because a recomposition
/// only costs something when it changes the answer; a recomposition is
/// compacted, and a failure there is returned rather than quietly falling back
/// to the multiplied table.
fn drop_unattached(
    solid: &Manifold,
    base: f64,
    already_loose: &[(f64, [[f64; 3]; 2])],
) -> Result<(Manifold, Vec<Manifold>), MeshError> {
    let mut kept = Vec::new();
    let mut fallen = Vec::new();
    for piece in solid.decompose() {
        if piece.is_empty() {
            continue;
        }
        match piece.bounding_box() {
            Some(bounds) if (bounds.min()[1] - base).abs() <= 1e-3 => kept.push(piece),
            Some(bounds) => {
                let signature = (piece.volume(), [bounds.min(), bounds.max()]);
                if already_loose
                    .iter()
                    .any(|known| same_piece(*known, signature))
                {
                    kept.push(piece);
                } else {
                    fallen.push(piece);
                }
            }
            None => fallen.push(piece),
        }
    }
    if fallen.is_empty() {
        return Ok((solid.clone(), fallen));
    }
    let kept = match kept.len() {
        0 => Manifold::empty(),
        1 => kept.remove(0),
        _ => Manifold::compose(&kept),
    };
    // Every decomposed piece carried the whole run table, so this is where the
    // multiplication is undone. An empty solid has no runs to rebuild.
    let kept = if kept.is_empty() {
        kept
    } else {
        compact_runs(&kept)?
    };
    Ok((kept, fallen))
}

impl GeometryMesher for ManifoldMesher {
    fn mesh(&self, geometry: &Geometry) -> Result<TriangleMesh, MeshError> {
        geometry.check().map_err(|e| error(&e.path, e.reason))?;
        if !self.sharp_angle.is_finite() || !(0.0..=180.0).contains(&self.sharp_angle) {
            return Err(error(
                "mesher.sharp_angle",
                "must be between 0 and 180 degrees",
            ));
        }
        let mut origins = Origins::default();
        let solid = self.evaluate(geometry, "geometry", &mut origins, Role::Body)?;
        // Positions and run metadata only. The kernel's own normals decided an
        // edge at the sharp angle by floating-point noise, which is the bug the
        // tie in `ashlar::shade_normals` fixes, so they are not asked for.
        extract_solid(
            &solid,
            "geometry",
            |id| {
                Ok(FaceSource {
                    operand: 0,
                    origin: origins.by_id.get(&id).copied().unwrap_or_default(),
                })
            },
            true,
            self,
        )
    }

    fn portals(&self, geometry: &Geometry) -> Result<Vec<PortalShape>, MeshError> {
        geometry.check().map_err(|e| error(&e.path, e.reason))?;
        let mut origins = Origins::default();
        self.evaluate(geometry, "geometry", &mut origins, Role::Body)?;
        Ok(origins.portals)
    }

    fn mesh_group(
        &self,
        solids: &[PlacedGeometry<'_>],
        cuts: &[PlacedCut<'_>],
    ) -> Result<GroupMesh, MeshError> {
        if !self.sharp_angle.is_finite() || !(0.0..=180.0).contains(&self.sharp_angle) {
            return Err(error(
                "mesher.sharp_angle",
                "must be between 0 and 180 degrees",
            ));
        }
        for (i, placed) in solids.iter().enumerate() {
            placed
                .geometry
                .check()
                .map_err(|e| error(&format!("solids[{i}].{}", e.path), e.reason))?;
            check_pose(placed.pose, &format!("solids[{i}].pose"))?;
        }
        for (j, placed) in cuts.iter().enumerate() {
            placed
                .geometry
                .check()
                .map_err(|e| error(&format!("cuts[{j}].{}", e.path), e.reason))?;
            check_pose(placed.pose, &format!("cuts[{j}].pose"))?;
        }

        if solids.is_empty() {
            return Ok(GroupMesh::default());
        }
        let (mut result, mut sources) = self.union_operands(solids)?;
        let base = result.bounding_box().map(|bounds| bounds.min()[1]);
        for (j, placed) in cuts.iter().enumerate() {
            let damage_index = u32::try_from(j).map_err(|e| error(&format!("cuts[{j}]"), e))?;
            let (cut, cut_sources) =
                self.evaluate_cut(placed, damage_index, &format!("cuts[{j}]"))?;
            // Keep the union before the cut when it may collapse, so the loose
            // pieces the group already had can be told from the ones the cut
            // creates. The clone shares the kernel's data until either side
            // changes.
            let before = (placed.collapse && base.is_some()).then(|| result.clone());
            sources.extend(cut_sources);
            result = result.difference(&cut);
            self.check_solid(&result, "group")?;
            if let (Some(base), Some(before)) = (base, before) {
                // Cheap first: only decompose the union before the cut when the
                // cut left something hanging. Most hits free nothing.
                if !loose_pieces(&result, base).is_empty() {
                    let already_loose = loose_pieces(&before, base);
                    let (kept, _fallen) = drop_unattached(&result, base, &already_loose)?;
                    result = kept;
                    self.check_solid(&result, "group")?;
                }
            }
        }

        let output = extract_group(&result, &sources, self)?;
        Ok(GroupMesh::new(output))
    }
}

/// Turn a group's solid and the ID table built while evaluating it into a mesh.
/// This is [`mesh`](GeometryMesher::mesh)'s extraction with a group's own source
/// lookup and its own policy for a degenerate triangle.
fn extract_group(
    solid: &Manifold,
    sources: &BTreeMap<u32, FaceSource>,
    mesher: &ManifoldMesher,
) -> Result<TriangleMesh, MeshError> {
    extract_solid(
        solid,
        "group",
        |id| {
            sources
                .get(&id)
                .copied()
                .ok_or_else(|| error("group", "surface run names no operand"))
        },
        false,
        mesher,
    )
}

/// Turn an evaluated solid into a mesh. Positions and run metadata come from
/// the kernel; corner normals and UVs are derived here, so the kernel's
/// superlinear `calculate_normals` is never called.
///
/// `path` names every error this raises, so a lone geometry reports `geometry`
/// and a group reports `group`. `source_of` resolves the face source of a
/// kernel original ID, which lets each caller answer an ID it does not know its
/// own way. `refuse_degenerate` is the one behavioural difference between the
/// two callers: `mesh` refuses a non-finite or degenerate kernel triangle,
/// while `extract_group` drops it. A boolean near a coincident plane leaves
/// slivers whose edges are far below the weld tolerance, and `shade_normals`
/// refuses the whole mesh over one of them, so dropping leaves the surface
/// unchanged and one sliver cannot cost a storey. A lone element has no such
/// scale to save, so a bad triangle there is a real error. Positions are
/// compacted so a dropped triangle's stray non-finite vertex cannot fail the
/// whole mesh from outside the indices.
fn extract_solid(
    solid: &Manifold,
    path: &str,
    source_of: impl Fn(u32) -> Result<FaceSource, MeshError>,
    refuse_degenerate: bool,
    mesher: &ManifoldMesher,
) -> Result<TriangleMesh, MeshError> {
    let output = solid.to_meshgl64();
    let vertices = output.vert_properties();
    let stride = output.num_prop();
    let kernel_indices = output.tri_verts();
    let run_indices = output.run_index();
    let run_ids = output.run_original_id();
    if kernel_indices.is_empty() {
        return Ok(TriangleMesh::default());
    }
    if stride < 3 {
        return Err(error(path, "kernel omitted positions"));
    }
    let positions: Vec<DVec3> = vertices
        .chunks_exact(stride)
        .map(|v| DVec3::new(v[0], v[1], v[2]))
        .collect();
    let mut indices = Vec::with_capacity(kernel_indices.len());
    for &index in &kernel_indices {
        indices.push(u32::try_from(index).map_err(|e| error(path, e))?);
    }
    let mut faces = vec![FaceSource::default(); indices.len() / 3];
    for (range, id) in run_indices.windows(2).zip(&run_ids) {
        let start = usize::try_from(range[0] / 3).map_err(|e| error(path, e))?;
        let end = usize::try_from(range[1] / 3).map_err(|e| error(path, e))?;
        let source = source_of(*id)?;
        faces
            .get_mut(start..end)
            .ok_or_else(|| error(path, "invalid surface run"))?
            .fill(source);
    }
    let mut kept_positions: Vec<DVec3> = Vec::new();
    let mut remap = vec![u32::MAX; positions.len()];
    let mut kept_indices = Vec::with_capacity(indices.len());
    let mut kept_faces = Vec::with_capacity(faces.len());
    for (face, triangle) in indices.as_chunks::<3>().0.iter().enumerate() {
        let mut points = [DVec3::ZERO; 3];
        let mut finite = true;
        for (corner, &index) in triangle.iter().enumerate() {
            let point = *positions
                .get(index as usize)
                .ok_or_else(|| error(path, "invalid kernel vertex index"))?;
            finite &= point.is_finite();
            points[corner] = point;
        }
        if !finite
            || (points[1] - points[0])
                .cross(points[2] - points[0])
                .try_normalize()
                .is_none()
        {
            if refuse_degenerate {
                return Err(error(path, "non-finite or degenerate output triangle"));
            }
            continue;
        }
        for &index in triangle {
            let slot = &mut remap[index as usize];
            if *slot == u32::MAX {
                *slot = u32::try_from(kept_positions.len()).map_err(|e| error(path, e))?;
                kept_positions.push(positions[index as usize]);
            }
            kept_indices.push(*slot);
        }
        kept_faces.push(
            *faces
                .get(face)
                .ok_or_else(|| error(path, "invalid surface run"))?,
        );
    }
    if kept_indices.is_empty() {
        return Err(error(path, "non-finite or degenerate output triangle"));
    }
    let normals = ashlar::shade_normals(&kept_positions, &kept_indices, mesher.sharp_angle)
        .ok_or_else(|| error(path, "non-finite or degenerate output triangle"))?;
    let mut mesh = TriangleMesh::default();
    for (face, triangle) in kept_indices.as_chunks::<3>().0.iter().enumerate() {
        let mut points = [DVec3::ZERO; 3];
        for (corner, &index) in triangle.iter().enumerate() {
            points[corner] = *kept_positions
                .get(index as usize)
                .ok_or_else(|| error(path, "invalid kernel vertex index"))?;
        }
        let uvs =
            ashlar::planar_uvs(points).ok_or_else(|| error(path, "invalid surface UV frame"))?;
        mesh.push_triangle(
            points,
            *normals
                .get(face)
                .ok_or_else(|| error(path, "invalid surface run"))?,
            uvs,
            *kept_faces
                .get(face)
                .ok_or_else(|| error(path, "invalid surface run"))?,
        );
    }
    mesh.weld(mesher.weld_tolerance);
    Ok(mesh)
}

/// Derive a convex proxy from an evaluated surface. Returns `None` for an empty
/// solid, which is what a fully subtracted element leaves behind.
fn collider(
    mesh: &TriangleMesh,
    collision: Collision,
    path: &str,
) -> Result<Option<ConvexSolid>, MeshError> {
    if mesh.positions.is_empty() {
        return Ok(None);
    }
    match collision {
        Collision::Bounds => {
            let mut min = glam::DVec3::splat(f64::INFINITY);
            let mut max = glam::DVec3::splat(f64::NEG_INFINITY);
            for point in &mesh.positions {
                min = min.min(*point);
                max = max.max(*point);
            }
            Ok(Some(ConvexSolid::from_bounds(min, max)))
        }
        Collision::Hull => {
            let points: Vec<[f64; 3]> = mesh.positions.iter().map(DVec3::to_array).collect();
            let hull = Manifold::hull_pts(&points);
            hull.status().map_err(|e| error(path, e))?;
            let surface = hull.to_meshgl64();
            let stride = surface.num_prop();
            let values = surface.vert_properties();
            let vertices = values
                .chunks_exact(stride)
                .map(|v| DVec3::new(v[0], v[1], v[2]))
                .collect();
            Ok(Some(ConvexSolid { vertices }))
        }
        // Non-exhaustive in the domain: an unknown declaration is not a silent
        // absence of collision.
        Collision::None => Ok(None),
        other => Err(error(path, format!("unsupported collision {other:?}"))),
    }
}

/// Evaluate used definitions once, retaining element-level error locations.
///
/// This is the no-damage path, so it keeps taking a port: with no cuts to
/// subtract, meshing each group alone is the whole of it.
pub fn mesh_building(
    building: &Building,
    mesher: &dyn GeometryMesher,
) -> Result<MeshedBuilding, MeshError> {
    let weld_tolerance = ashlar::WELD_TOLERANCE;
    let merging = !building.merge_groups().is_empty();
    let (parts, part_colliders, part_portals) =
        mesh_parts(building, mesher, merging, weld_tolerance)?;
    let groups = if merging {
        mesh_groups(building, mesher, weld_tolerance)?
    } else {
        Vec::new()
    };
    Ok(MeshedBuilding {
        building: building.clone(),
        parts,
        part_colliders,
        part_portals,
        groups,
        damage: DamageLog::default(),
    })
}

/// Mesh a building and apply a damage log to it, from nothing.
///
/// The log is checked against the building first: every record must name a
/// bound slot, be valid geometry, sit at a finite pose, and the building must
/// be merged. The answer is what [`GroupSolids::build`] answers, because
/// support crosses groups: a collapsing hit can drop a piece of a group it
/// never touched, and only the whole building knows what holds what up. The
/// mesher is concrete for that reason, since a port meshes one group at a
/// time. An empty log is exactly [`mesh_building`].
pub fn mesh_building_with_damage(
    building: &Building,
    log: &DamageLog,
    mesher: &ManifoldMesher,
) -> Result<MeshedBuilding, MeshError> {
    GroupSolids::build(building, log, *mesher).map(|(_, meshed)| meshed)
}

/// What the per-part half of meshing a building answers: element batches,
/// collision proxies and portals, all keyed by part id.
type PartsOutput = (
    BTreeMap<String, Vec<ElementMesh>>,
    BTreeMap<String, Vec<ElementCollider>>,
    BTreeMap<String, Vec<(String, PortalShape)>>,
);

/// Evaluate every used part's elements: the standalone (or unmerged) batches
/// that are drawn from `parts`, the collision proxies, and the portals.
///
/// A merged element is still evaluated here because its collision proxy is the
/// mesh it would have been drawn as; whether its batches are kept is the
/// `merging` flag's business.
fn mesh_parts(
    building: &Building,
    mesher: &dyn GeometryMesher,
    merging: bool,
    weld_tolerance: f64,
) -> Result<PartsOutput, MeshError> {
    let mut parts = BTreeMap::new();
    let mut part_colliders: BTreeMap<String, Vec<ElementCollider>> = BTreeMap::new();
    let mut part_portals: BTreeMap<String, Vec<(String, PortalShape)>> = BTreeMap::new();
    for part in &building.recipe().parts {
        if !building
            .recipe()
            .instances
            .iter()
            .any(|i| i.part == part.id)
        {
            continue;
        }
        let mut elements = Vec::new();
        for element in &part.elements {
            let mesh = mesh_element(&part.id, element, mesher, weld_tolerance)?;
            if element.collision != Collision::None
                && let Some(solid) = collider(
                    &mesh,
                    element.collision,
                    &format!("parts[{}].elements[{}]", part.id, element.id),
                )?
            {
                part_colliders
                    .entry(part.id.clone())
                    .or_default()
                    .push(ElementCollider {
                        id: element.id.clone(),
                        solid,
                    });
            }
            // An element with no portal cutter is not handed to the mesher
            // again: a portal costs a solid evaluation only when one is marked.
            if element
                .geometry
                .cutters()
                .iter()
                .any(|cutter| cutter.portal.is_some())
            {
                let shapes = mesher.portals(&element.geometry).map_err(|e| {
                    error(
                        &format!("parts[{}].elements[{}].{}", part.id, element.id, e.path),
                        e.reason,
                    )
                })?;
                part_portals
                    .entry(part.id.clone())
                    .or_default()
                    .extend(shapes.into_iter().map(|shape| (element.id.clone(), shape)));
            }
            // A merged element is drawn from its group rather than here, but it
            // was evaluated above because its collision proxy is this mesh.
            if !merging || element.standalone {
                elements.extend(batches(element, mesh, weld_tolerance));
            }
        }
        parts.insert(part.id.clone(), elements);
    }
    Ok((parts, part_colliders, part_portals))
}

/// One element's surface, projected into its UV mode and checked: the part of
/// meshing a part that does not depend on what else the part declares.
fn mesh_element(
    part: &str,
    element: &Element,
    mesher: &dyn GeometryMesher,
    weld_tolerance: f64,
) -> Result<TriangleMesh, MeshError> {
    let mut mesh = mesher.mesh(&element.geometry).map_err(|e| {
        error(
            &format!("parts[{part}].elements[{}].{}", element.id, e.path),
            e.reason,
        )
    })?;
    if element.uv != ashlar::UvMode::default() {
        if !mesh.project_uvs(element.uv) {
            return Err(error(
                &format!("parts[{part}].elements[{}]", element.id),
                "surface cannot be projected under this UV mode",
            ));
        }
        // Projecting splits every vertex, and a new UV seam is not always
        // where the old one was.
        mesh.weld(weld_tolerance);
    }
    if !mesh.is_consistent() {
        return Err(error(
            &element.id,
            "mesher returned inconsistent surface metadata",
        ));
    }
    Ok(mesh)
}

/// Union each merge group once in building space and split its surface into
/// batches that share a binding. One entry per group, in the order
/// [`Building::merge_groups`] gives.
///
/// No cut is subtracted and no collapse applies: this is the no-damage path.
/// [`GroupSolids::build`] is the damaged one, in `solids`.
fn mesh_groups(
    building: &Building,
    mesher: &dyn GeometryMesher,
    weld_tolerance: f64,
) -> Result<Vec<MergedGroup>, MeshError> {
    let log = DamageLog::default();
    let mut groups = Vec::new();
    for group in building.merge_groups() {
        let path = format!("groups[{}]", group.id);
        let solids: Vec<PlacedGeometry<'_>> = group
            .members
            .iter()
            .map(|(instance, element)| PlacedGeometry {
                geometry: &element.geometry,
                pose: instance.pose,
            })
            .collect();
        let mesh = mesher
            .mesh_group(&solids, &[])
            .map_err(|e| error(&format!("{path}.{}", e.path), e.reason))?
            .mesh;
        groups.push(assemble_group(
            building,
            &log,
            &group,
            &mesh,
            false,
            weld_tolerance,
            &path,
        )?);
    }
    Ok(groups)
}

/// Everything one group's evaluated surface becomes, whichever path evaluated
/// it: batches split by binding and side, building-space bounds, and a triangle
/// collider when a cut has been subtracted from the solid.
///
/// A convex proxy cannot have a hole in it, so a group a record touched answers
/// collision with its own triangles. The collider is taken from the whole
/// evaluated surface, before it is split into batches.
fn assemble_group(
    building: &Building,
    log: &DamageLog,
    group: &GroupMembers<'_>,
    mesh: &TriangleMesh,
    cut: bool,
    weld_tolerance: f64,
    path: &str,
) -> Result<MergedGroup, MeshError> {
    let collider = if cut {
        Some(TriangleCollider {
            positions: mesh.positions.clone(),
            indices: mesh.indices.clone(),
        })
    } else {
        None
    };
    let batches = group_batches(building, log, group, mesh, weld_tolerance, path)?;
    Ok(MergedGroup {
        id: group.id.to_owned(),
        storey: group.storey,
        operands: group
            .members
            .iter()
            .map(|(instance, element)| Operand {
                instance: instance.id.clone(),
                element: element.id.clone(),
            })
            .collect(),
        batches,
        bounds: bounds_of(&mesh.positions),
        collider,
    })
}

/// The binding and side each source of a group's surface resolves to.
enum Resolved<'a> {
    /// A face of a placed element, resolved through its instance.
    Element {
        uv: UvMode,
        pose: Pose,
        binding: &'a Binding,
        side: Side,
    },
    /// A face a damage record exposed; it belongs to no element, so it wears
    /// the record's own slot from the building palette and reads as interior.
    Damage { binding: &'a Binding },
}

/// Resolve each distinct source once. An operand names the placed element and
/// the slot rule is the element's own; a damage index names the record and the
/// binding is the one its slot resolves to in the building's palette.
fn resolve_sources<'a>(
    building: &'a Building,
    log: &DamageLog,
    group: &GroupMembers<'_>,
    mesh: &TriangleMesh,
    path: &str,
) -> Result<BTreeMap<FaceSource, Resolved<'a>>, MeshError> {
    let mut resolved: BTreeMap<FaceSource, Resolved<'a>> = BTreeMap::new();
    for source in &mesh.sources {
        if resolved.contains_key(source) {
            continue;
        }
        let entry = if let FaceOrigin::Damage(global) = source.origin {
            let damage = log.0.get(global as usize).ok_or_else(|| {
                error(
                    path,
                    format!("mesher named damage {global} the log does not have"),
                )
            })?;
            let binding = building
                .recipe()
                .materials
                .get(&damage.slot)
                .ok_or_else(|| {
                    error(
                        path,
                        format!("no binding for damage slot {:?}", damage.slot),
                    )
                })?;
            Resolved::Damage { binding }
        } else {
            let (instance, element) = group
                .members
                .get(source.operand as usize)
                .ok_or_else(|| error(path, "mesher named an operand the group does not have"))?;
            let slot = element.slot_for(source.origin);
            let binding = building.binding(&instance.id, slot).ok_or_else(|| {
                error(
                    path,
                    format!("no binding for slot {slot:?} on instance {:?}", instance.id),
                )
            })?;
            Resolved::Element {
                uv: element.uv,
                pose: instance.pose,
                binding,
                side: element.side,
            }
        };
        resolved.insert(*source, entry);
    }
    Ok(resolved)
}

/// Split a group's evaluated surface into drawable batches, one per binding
/// and side, resolving each face from where it came from.
///
/// Box mapping is what Planar and Box elements get here: its coordinates come
/// from the building frame alone, so they are continuous across every former
/// joint and do not move when a later cut re-triangulates a wall. A damage
/// face is box-mapped in building space for the same reason.
fn group_batches(
    building: &Building,
    log: &DamageLog,
    group: &GroupMembers<'_>,
    mesh: &TriangleMesh,
    weld_tolerance: f64,
    path: &str,
) -> Result<Vec<GroupBatch>, MeshError> {
    let resolved = resolve_sources(building, log, group, mesh, path)?;
    let mut batches: BTreeMap<(&Binding, Side), TriangleMesh> = BTreeMap::new();
    for (face, corners) in mesh.corners().enumerate() {
        let source = mesh.sources[face];
        let points = corners.map(|index| mesh.positions[index]);
        // A face with no area has no surface frame, and one sliver must not
        // cost the group. A sound face that cannot be projected is a real
        // error: the projection itself is wrong.
        let degenerate = (points[1] - points[0])
            .cross(points[2] - points[0])
            .try_normalize()
            .is_none();
        let (binding, side, uvs) = match resolved
            .get(&source)
            .ok_or_else(|| error(path, "mesher named an operand the group does not have"))?
        {
            Resolved::Element {
                uv,
                pose,
                binding,
                side,
            } => {
                let Some(uvs) = group_uvs(*uv, *pose, points) else {
                    if degenerate {
                        continue;
                    }
                    return Err(error(path, "surface cannot be projected in building space"));
                };
                (*binding, *side, uvs)
            }
            Resolved::Damage { binding } => {
                let Some(uvs) = ashlar::box_uvs(points) else {
                    if degenerate {
                        continue;
                    }
                    return Err(error(path, "surface cannot be projected in building space"));
                };
                (*binding, Side::Interior, uvs)
            }
        };
        batches.entry((binding, side)).or_default().push_triangle(
            points,
            corners.map(|index| mesh.normals[index]),
            uvs,
            source,
        );
    }
    Ok(batches
        .into_iter()
        .map(|((binding, side), mut batch)| {
            batch.weld(weld_tolerance);
            GroupBatch {
                binding: binding.clone(),
                side,
                mesh: batch,
            }
        })
        .collect())
}

/// The axis-aligned bounds of a surface, or `None` when it has no positions.
fn bounds_of(positions: &[DVec3]) -> Option<[DVec3; 2]> {
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    for point in positions {
        min = min.min(*point);
        max = max.max(*point);
    }
    (!positions.is_empty()).then_some([min, max])
}

/// Project one building-space face under the element's own UV mode. `Planar`
/// and `Box` both become the building-frame box mapping, and a cylinder wraps
/// about its own axis, reached through the inverse of the pose that placed it.
fn group_uvs(uv: UvMode, pose: Pose, points: [DVec3; 3]) -> Option<[[f64; 2]; 3]> {
    match uv {
        UvMode::Cylindrical { axis } => {
            let local = points.map(|point| pose.inverse().transform_point(point));
            ashlar::cylindrical_uvs(local, axis)
        }
        // `Planar`, `Box` and, because `UvMode` is non-exhaustive, any unknown
        // projection all fall back to the box mapping rather than refusing the
        // whole building.
        _ => ashlar::box_uvs(points),
    }
}

/// Split one element's evaluated surface into drawable batches, one per
/// resolved slot. With nothing to tell faces apart it stays a single batch,
/// cut faces and all.
fn batches(element: &Element, mesh: TriangleMesh, weld_tolerance: f64) -> Vec<ElementMesh> {
    let cutters = element.geometry.cutters();
    // Nothing can separate this element's faces into slots, so it stays one
    // batch even when a cutter marked some of its faces.
    if element.cut_material_slot.is_none() && cutters.iter().all(|cutter| cutter.cut_slot.is_none())
    {
        return vec![ElementMesh {
            id: element.id.clone(),
            material_slot: element.material_slot.clone(),
            is_cut: false,
            side: element.side,
            mesh,
        }];
    }
    // slot_for walks the geometry to allocate, so each distinct origin is
    // resolved once rather than once per face.
    let mut resolved: BTreeMap<FaceOrigin, &str> = BTreeMap::new();
    for source in &mesh.sources {
        resolved
            .entry(source.origin)
            .or_insert_with(|| element.slot_for(source.origin));
    }
    // Batches come out in slot declaration order: the element's main slot, then
    // its cut slot, then each cutter root's own slot.
    let mut candidates = vec![element.material_slot.as_str()];
    candidates.extend(element.cut_material_slot.as_deref());
    candidates.extend(
        cutters
            .iter()
            .filter_map(|cutter| cutter.cut_slot.as_deref()),
    );
    let mut slots: Vec<&str> = Vec::new();
    for slot in candidates {
        if !slots.contains(&slot) {
            slots.push(slot);
        }
    }
    let mut batches = Vec::new();
    for slot in slots {
        let mut batch = TriangleMesh::default();
        for (face, corners) in mesh.corners().enumerate() {
            if resolved.get(&mesh.sources[face].origin).copied() != Some(slot) {
                continue;
            }
            batch.push_triangle(
                corners.map(|i| mesh.positions[i]),
                corners.map(|i| mesh.normals[i]),
                corners.map(|i| mesh.uvs[i]),
                mesh.sources[face],
            );
        }
        if batch.positions.is_empty() {
            continue;
        }
        batch.weld(weld_tolerance);
        batches.push(ElementMesh {
            id: element.id.clone(),
            material_slot: slot.to_owned(),
            is_cut: batch.sources.iter().all(|source| source.is_cut()),
            side: element.side,
            mesh: batch,
        });
    }
    batches
}

// The convex hull of 24 inset corner points gives twelve planar edge chamfers
// and eight triangular corner chamfers, retaining the authored box bounds.
fn chamfered_box(size: [f64; 3], bevel: f64) -> Manifold {
    let mut points = Vec::with_capacity(24);
    for x in [false, true] {
        for y in [false, true] {
            for z in [false, true] {
                for axis in 0..3 {
                    let inset: [f64; 3] =
                        std::array::from_fn(|i| if i == axis { 0.0 } else { bevel });
                    points.push(std::array::from_fn(|i| {
                        if [x, y, z][i] {
                            size[i] - inset[i]
                        } else {
                            inset[i]
                        }
                    }));
                }
            }
        }
    }
    Manifold::hull_pts(&points)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three disjoint unit cubes and a shell: four connected pieces with
    /// several kernel original IDs and no boolean joining them.
    fn grouped_solid() -> Manifold {
        let cube = Manifold::cube(1.0, 1.0, 1.0, false);
        let shell = Manifold::cube(2.0, 2.0, 2.0, false)
            .difference(&Manifold::cube(1.0, 1.0, 1.0, false).translate(0.5, 0.5, 0.5));
        Manifold::batch_union(&[
            cube.clone(),
            cube.translate(3.0, 0.0, 0.0),
            cube.translate(6.0, 0.0, 0.0),
            shell,
        ])
    }

    /// The number of triangles each kernel original ID shows, in ID order.
    fn id_triangle_counts(solid: &Manifold) -> BTreeMap<u32, usize> {
        let mesh = solid.to_meshgl64();
        let runs = mesh.run_index();
        let ids = mesh.run_original_id();
        let mut counts = BTreeMap::new();
        for (range, id) in runs.windows(2).zip(&ids) {
            let triangles = usize::try_from((range[1] - range[0]) / 3).expect("triangle count");
            *counts.entry(*id).or_default() += triangles;
        }
        counts
    }

    #[test]
    fn compacting_keeps_the_solid_and_its_ids() {
        let solid = grouped_solid();
        let volume = solid.volume();
        let triangles = solid.num_tri();
        let ids = id_triangle_counts(&solid);
        let compacted = compact_runs(&solid).expect("compaction");
        assert!((compacted.volume() - volume).abs() < 1e-12);
        assert_eq!(compacted.num_tri(), triangles);
        assert_eq!(id_triangle_counts(&compacted), ids);
    }

    #[test]
    fn decompose_and_compose_no_longer_multiply_runs() {
        let solid = grouped_solid();
        let distinct = id_triangle_counts(&solid).len();
        let mut compacted = solid.clone();
        for _ in 0..10 {
            let pieces = compacted.decompose();
            compacted = compact_runs(&Manifold::compose(&pieces)).expect("compaction");
            assert_eq!(compacted.to_meshgl64().num_run(), distinct);
        }
        // Document the bug the compaction exists for: without it each compose
        // copies the whole run table once per piece.
        let first = Manifold::compose(&solid.decompose())
            .to_meshgl64()
            .num_run();
        let mut plain = solid;
        for _ in 0..10 {
            plain = Manifold::compose(&plain.decompose());
        }
        let grown = plain.to_meshgl64().num_run();
        assert!(
            grown >= first * 100,
            "the uncompacted run table grew from {first} to {grown}"
        );
    }
}
