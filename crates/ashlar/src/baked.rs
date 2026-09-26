//! A building evaluated ahead of time, at every level of detail, as one file.
//!
//! A game should not link a geometry kernel. [`MeshedBuilding`] is what a
//! backend makes of a recipe, and everything in it is plain data — triangles,
//! convex proxies, portals — so it can be made once, in a content step, and
//! written to disk. [`BakedBuilding`] is that result with its coarser levels
//! beside it: level zero is the building as authored, and each level after it
//! is the same building simplified for a greater distance (see
//! `ashlar_manifold::bake` for how they are made). A renderer draws the level
//! [`BakedBuilding::level`] answers through the same [`MeshedBuilding::pieces`]
//! walk it would use on a freshly meshed building, so nothing downstream knows
//! the difference.
//!
//! [`BakedBuilding::write`] and [`BakedBuilding::read`] are the file. See
//! ADR 0006 for why it looks the way it does.
//!
//! ```
//! use ashlar::{BakedBuilding, BakedLevel, Building, Element, Geometry, Instance, Part};
//! # use std::collections::BTreeMap;
//! let building = Building::builder("example:box")
//!     .part(Part::builder("example:part").element(Element::new("body", Geometry::cuboid([1.0; 3]), "wall")).build()?)
//!     .instance(Instance::new("one", "example:part"))
//!     .material("wall", "example:plaster")
//!     .build()?;
//! let baked = BakedBuilding::new(building, vec![BakedLevel::default()], BTreeMap::new(), BTreeMap::new());
//! let bytes = baked.write()?;
//! let back = BakedBuilding::read(&bytes)?;
//! assert_eq!(back.levels.len(), 1);
//! assert_eq!(back.building().recipe().id, "example:box");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # The container
//!
//! Twelve identifying bytes, a version, then two sections, each length
//! prefixed: a RON *manifest* holding the recipe and every level's structure
//! with each triangle mesh replaced by an index, and a binary *mesh table*
//! holding those meshes as little-endian arrays. Structure stays readable
//! and diffable; bulk stays compact. Positions, normals and UVs narrow to
//! `f32` on disk, exactly as they do on their way to a GPU, and widen again on
//! read; indices and face provenance are stored as written.
use std::collections::BTreeMap;

use glam::DVec3;
use serde::{Deserialize, Serialize};

use crate::{
    Binding, Building, BuildingRecipe, ConvexSolid, DamageLog, ElementCollider, ElementMesh,
    FaceOrigin, FaceSource, GroupBatch, MergedGroup, MeshedBuilding, Operand, PortalShape, Side,
    TriangleCollider, TriangleMesh, ValidationError,
};

/// The twelve bytes every baked building starts with, shaped as KTX2's and the
/// strand set's identifiers are: a high byte, a readable name, and a line-ending
/// trap.
pub const IDENTIFIER: [u8; 12] = [
    0xAB, b'A', b'S', b'H', b'B', b'L', b'D', b'G', 0x0D, 0x0A, 0x1A, 0x0A,
];

/// The format version this crate writes and the only one it reads.
pub const VERSION: u32 = 1;

/// One level of detail: the building's drawable geometry, simplified or not.
#[derive(Clone, Debug, Default)]
pub struct BakedLevel {
    /// Distance in metres from the camera at which this level stops drawing and
    /// the next one starts. `None` on the last level, which draws to any
    /// distance; a game that wants the building culled past it gives its last
    /// level a distance and bakes no level after it.
    pub until: Option<f32>,
    /// Element meshes per part id, in part space, as [`MeshedBuilding::parts`].
    pub parts: BTreeMap<String, Vec<ElementMesh>>,
    /// Merged groups in building space, as [`MeshedBuilding::groups`].
    pub groups: Vec<MergedGroup>,
}

/// A building and every level of detail of it, ready to draw without a kernel.
#[derive(Clone, Debug)]
pub struct BakedBuilding {
    building: Building,
    /// Level zero first, then each coarser level.
    pub levels: Vec<BakedLevel>,
    /// Collision proxies per part id, from level zero: a proxy does not coarsen
    /// with distance, because physics runs at every distance.
    pub part_colliders: BTreeMap<String, Vec<ElementCollider>>,
    /// Portals per part id, from level zero.
    pub part_portals: BTreeMap<String, Vec<(String, PortalShape)>>,
}

/// Why a baked building could not be written or read.
#[derive(Debug, thiserror::Error)]
pub enum BakedError {
    /// The bytes do not start with [`IDENTIFIER`].
    #[error("not a baked ashlar building")]
    Identifier,
    /// A version this crate does not read.
    #[error("baked building version {0}; this crate reads version {VERSION}")]
    Version(u32),
    /// A length or offset points past the end of the file.
    #[error("baked building is truncated at {0}")]
    Truncated(&'static str),
    /// The manifest is not valid RON for this version.
    #[error("baked building manifest: {0}")]
    Manifest(String),
    /// A mesh index in the manifest names no mesh in the table, or a mesh's
    /// arrays disagree with one another.
    #[error("baked building mesh table: {0}")]
    Mesh(String),
    /// The recipe inside no longer validates.
    #[error("baked building recipe: {0}")]
    Recipe(#[from] ValidationError),
}

impl BakedBuilding {
    /// Assemble a baked building from its parts. Level zero must come first.
    ///
    /// Nothing is checked here, because a mesher assembles it from what it
    /// just made; [`write`](Self::write) checks it against its recipe exactly
    /// as [`read`](Self::read) does, so a file that writes also reads.
    #[must_use]
    pub fn new(
        building: Building,
        levels: Vec<BakedLevel>,
        part_colliders: BTreeMap<String, Vec<ElementCollider>>,
        part_portals: BTreeMap<String, Vec<(String, PortalShape)>>,
    ) -> Self {
        Self {
            building,
            levels,
            part_colliders,
            part_portals,
        }
    }

    /// The validated recipe this was baked from.
    #[must_use]
    pub fn building(&self) -> &Building {
        &self.building
    }

    /// One level as a [`MeshedBuilding`], so every consumer of a freshly meshed
    /// building — [`MeshedBuilding::pieces`], the colliders, the portals —
    /// draws a baked one unchanged. Colliders and portals are level zero's at
    /// every level. `None` past the last level.
    #[must_use]
    pub fn level(&self, index: usize) -> Option<MeshedBuilding> {
        let level = self.levels.get(index)?;
        Some(MeshedBuilding {
            building: self.building.clone(),
            parts: level.parts.clone(),
            part_colliders: self.part_colliders.clone(),
            part_portals: self.part_portals.clone(),
            groups: level.groups.clone(),
            damage: DamageLog::default(),
        })
    }

    /// Unique triangles per level: every part mesh once and every group batch.
    #[must_use]
    pub fn triangles(&self) -> Vec<usize> {
        self.levels
            .iter()
            .map(|level| {
                level
                    .parts
                    .values()
                    .flatten()
                    .map(|element| element.mesh.triangle_count())
                    .sum::<usize>()
                    + level
                        .groups
                        .iter()
                        .flat_map(|group| &group.batches)
                        .map(|batch| batch.mesh.triangle_count())
                        .sum::<usize>()
            })
            .collect()
    }

    /// What `read` would refuse is refused before a byte is written: a mesh
    /// whose arrays disagree, and a face origin whose index would collide with
    /// the two bits that store its kind.
    fn check_writable(&self) -> Result<(), BakedError> {
        check_against_recipe(
            &self.building,
            &self.levels,
            &self.part_colliders,
            &self.part_portals,
        )?;
        for mesh in self.levels.iter().flat_map(|level| {
            level
                .parts
                .values()
                .flatten()
                .map(|element| &element.mesh)
                .chain(
                    level
                        .groups
                        .iter()
                        .flat_map(|group| group.batches.iter().map(|b| &b.mesh)),
                )
        }) {
            if !mesh.is_consistent() {
                return Err(BakedError::Mesh("a mesh's arrays disagree".into()));
            }
            if mesh.sources.iter().any(|source| {
                matches!(source.origin, FaceOrigin::Cutter(k) | FaceOrigin::Damage(k) if k & ORIGIN_KIND != 0)
            }) {
                return Err(BakedError::Mesh("a face origin index past 2^30".into()));
            }
        }
        Ok(())
    }

    /// The file: identifier, version, a RON manifest and a binary mesh table.
    ///
    /// # Errors
    ///
    /// [`BakedError::Mesh`] for a mesh whose arrays disagree or whose face
    /// provenance does not fit the file, and [`BakedError::Manifest`] if the
    /// manifest will not serialize, which a validated building does not do.
    pub fn write(&self) -> Result<Vec<u8>, BakedError> {
        self.check_writable()?;
        let mut table = MeshTable::default();
        let manifest = Manifest {
            recipe: self.building.recipe().clone(),
            levels: self
                .levels
                .iter()
                .map(|level| ManifestLevel {
                    until: level.until,
                    parts: level
                        .parts
                        .iter()
                        .map(|(id, elements)| {
                            let elements = elements
                                .iter()
                                .map(|element| ManifestElement {
                                    id: element.id.clone(),
                                    material_slot: element.material_slot.clone(),
                                    is_cut: element.is_cut,
                                    side: element.side,
                                    mesh: table.push(&element.mesh),
                                })
                                .collect();
                            (id.clone(), elements)
                        })
                        .collect(),
                    groups: level
                        .groups
                        .iter()
                        .map(|group| ManifestGroup {
                            id: group.id.clone(),
                            storey: group.storey,
                            operands: group
                                .operands
                                .iter()
                                .map(|o| (o.instance.clone(), o.element.clone()))
                                .collect(),
                            batches: group
                                .batches
                                .iter()
                                .map(|batch| ManifestBatch {
                                    binding: batch.binding.clone(),
                                    side: batch.side,
                                    mesh: table.push(&batch.mesh),
                                })
                                .collect(),
                            bounds: group.bounds.map(|[a, b]| [a.to_array(), b.to_array()]),
                            collider: group.collider.as_ref().map(|collider| {
                                table.push_collider(&collider.positions, &collider.indices)
                            }),
                        })
                        .collect(),
                })
                .collect(),
            colliders: self
                .part_colliders
                .iter()
                .map(|(id, colliders)| {
                    let colliders = colliders
                        .iter()
                        .map(|c| {
                            (
                                c.id.clone(),
                                c.solid.vertices.iter().map(DVec3::to_array).collect(),
                            )
                        })
                        .collect();
                    (id.clone(), colliders)
                })
                .collect(),
            portals: self
                .part_portals
                .iter()
                .map(|(id, portals)| {
                    let portals = portals
                        .iter()
                        .map(|(element, shape)| ManifestPortal {
                            element: element.clone(),
                            id: shape.id.clone(),
                            corners: shape.corners.map(|c| c.to_array()),
                            normal: shape.normal.to_array(),
                        })
                        .collect();
                    (id.clone(), portals)
                })
                .collect(),
        };
        let manifest = manifest_text(&manifest)?;
        let mut bytes = Vec::with_capacity(32 + manifest.len() + table.bytes.len());
        bytes.extend_from_slice(&IDENTIFIER);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&(manifest.len() as u64).to_le_bytes());
        bytes.extend_from_slice(manifest.as_bytes());
        bytes.extend_from_slice(&u64::from(table.count).to_le_bytes());
        bytes.extend_from_slice(&(table.bytes.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&table.bytes);
        Ok(bytes)
    }

    /// Read a file [`write`](Self::write) produced, revalidating the recipe.
    ///
    /// # Errors
    ///
    /// Whatever the bytes cannot account for, by kind: the identifier, the
    /// version, a length past the end, a manifest that does not parse, a mesh
    /// index or array that does not fit, a recipe that does not validate.
    pub fn read(bytes: &[u8]) -> Result<Self, BakedError> {
        let mut cursor = Cursor { bytes, at: 0 };
        if cursor.take(IDENTIFIER.len(), "identifier")? != IDENTIFIER {
            return Err(BakedError::Identifier);
        }
        let version = cursor.u32("version")?;
        if version != VERSION {
            return Err(BakedError::Version(version));
        }
        let manifest_len = cursor.len("manifest length")?;
        let manifest = cursor.take(manifest_len, "manifest")?;
        let manifest = std::str::from_utf8(manifest)
            .map_err(|error| BakedError::Manifest(error.to_string()))?;
        let manifest: Manifest =
            ron::from_str(manifest).map_err(|error| BakedError::Manifest(error.to_string()))?;
        let count = cursor.len("mesh count")?;
        let table_len = cursor.len("mesh table length")?;
        let table = cursor.take(table_len, "mesh table")?;
        let meshes = read_table(table, count)?;
        let mesh = |index: u32| -> Result<TriangleMesh, BakedError> {
            meshes
                .get(index as usize)
                .cloned()
                .ok_or_else(|| BakedError::Mesh(format!("no mesh {index}")))
        };
        let building = manifest.recipe.build()?;
        let levels = manifest
            .levels
            .into_iter()
            .map(|level| decode_level(level, &mesh))
            .collect::<Result<Vec<_>, _>>()?;
        let part_colliders = decode_colliders(manifest.colliders);
        let part_portals = decode_portals(manifest.portals);
        check_against_recipe(&building, &levels, &part_colliders, &part_portals)?;
        Ok(Self {
            building,
            levels,
            part_colliders,
            part_portals,
        })
    }
}

/// The manifest as text. Indented, so it reads and diffs as a recipe does;
/// arrays stay on one line, and the bulk is in the table anyway.
fn manifest_text(manifest: &Manifest) -> Result<String, BakedError> {
    let pretty = ron::ser::PrettyConfig::default()
        .indentor(" ")
        .compact_arrays(true);
    ron::ser::to_string_pretty(manifest, pretty)
        .map_err(|error| BakedError::Manifest(error.to_string()))
}

/// Everything a level names must be something the recipe declares: a part
/// the recipe holds, and on each of its element meshes a slot one of that
/// part's elements names. The recipe validated binds every such slot on every
/// instance, so a file that passes this cannot make a drawing walk meet a slot
/// it has no binding for. A level may leave a part out: a coarse level drops
/// what it simplified away.
fn check_against_recipe(
    building: &Building,
    levels: &[BakedLevel],
    colliders: &BTreeMap<String, Vec<ElementCollider>>,
    portals: &BTreeMap<String, Vec<(String, PortalShape)>>,
) -> Result<(), BakedError> {
    let part = |id: &str| {
        building
            .part(id)
            .ok_or_else(|| BakedError::Mesh(format!("part {id} is not in the recipe")))
    };
    if levels.is_empty() {
        return Err(BakedError::Mesh("a baked building has no levels".into()));
    }
    for (index, level) in levels.iter().enumerate() {
        if level
            .until
            .is_some_and(|until| !(until.is_finite() && until > 0.0))
        {
            return Err(BakedError::Mesh(format!(
                "level {index} ends at a distance that is not a positive number"
            )));
        }
        for (id, elements) in &level.parts {
            let declared = part(id)?;
            for element in elements {
                let known = declared.elements.iter().any(|authored| {
                    authored.id == element.id
                        && authored.slots().any(|slot| slot == element.material_slot)
                });
                if !known {
                    return Err(BakedError::Mesh(format!(
                        "level {index}: part {id} has no element {} on slot {}",
                        element.id, element.material_slot
                    )));
                }
            }
        }
        if level.until.is_none() && index + 1 < levels.len() {
            return Err(BakedError::Mesh(format!(
                "level {index} draws to any distance but is not the last"
            )));
        }
        if index > 0 && level.until.is_some() && levels[index - 1].until >= level.until {
            return Err(BakedError::Mesh(format!(
                "level {index} does not reach further than level {}",
                index - 1
            )));
        }
    }
    for id in colliders.keys().chain(portals.keys()) {
        part(id)?;
    }
    Ok(())
}

type MeshOf<'a> = dyn Fn(u32) -> Result<TriangleMesh, BakedError> + 'a;

/// A mesh a renderer can draw: normals and UVs for every position, whole
/// triangles, one face source each, every UV finite. The table can hold a
/// collider's bare positions and indices, and an element or batch that points
/// at one would reach a tangent generator with no normals to read.
fn drawable(mesh: TriangleMesh) -> Result<TriangleMesh, BakedError> {
    if !mesh.is_consistent() {
        return Err(BakedError::Mesh(
            "a drawn mesh's normals, UVs, indices or face sources disagree".into(),
        ));
    }
    if mesh.uvs.iter().flatten().any(|uv| !uv.is_finite()) {
        return Err(BakedError::Mesh("a non-finite value in uvs".into()));
    }
    Ok(mesh)
}

fn decode_level(level: ManifestLevel, mesh: &MeshOf<'_>) -> Result<BakedLevel, BakedError> {
    let mut parts = BTreeMap::new();
    for (id, elements) in level.parts {
        let elements = elements
            .into_iter()
            .map(|element| {
                Ok(ElementMesh {
                    id: element.id,
                    material_slot: element.material_slot,
                    is_cut: element.is_cut,
                    side: element.side,
                    mesh: drawable(mesh(element.mesh)?)?,
                })
            })
            .collect::<Result<Vec<_>, BakedError>>()?;
        parts.insert(id, elements);
    }
    let mut groups = Vec::with_capacity(level.groups.len());
    for group in level.groups {
        let batches = group
            .batches
            .into_iter()
            .map(|batch| {
                Ok(GroupBatch {
                    binding: batch.binding,
                    side: batch.side,
                    mesh: drawable(mesh(batch.mesh)?)?,
                })
            })
            .collect::<Result<Vec<_>, BakedError>>()?;
        let collider = group
            .collider
            .map(|index| {
                mesh(index).and_then(|mesh| {
                    if mesh.indices.len() % 3 != 0 {
                        return Err(BakedError::Mesh(
                            "a collider's indices are not whole triangles".into(),
                        ));
                    }
                    Ok(TriangleCollider {
                        positions: mesh.positions,
                        indices: mesh.indices,
                    })
                })
            })
            .transpose()?;
        groups.push(MergedGroup {
            id: group.id,
            storey: group.storey,
            operands: group
                .operands
                .into_iter()
                .map(|(instance, element)| Operand { instance, element })
                .collect(),
            batches,
            bounds: group
                .bounds
                .map(|[a, b]| [DVec3::from_array(a), DVec3::from_array(b)]),
            collider,
        });
    }
    Ok(BakedLevel {
        until: level.until,
        parts,
        groups,
    })
}

fn decode_colliders(
    colliders: BTreeMap<String, Vec<(String, Vec<[f64; 3]>)>>,
) -> BTreeMap<String, Vec<ElementCollider>> {
    colliders
        .into_iter()
        .map(|(id, colliders)| {
            let colliders = colliders
                .into_iter()
                .map(|(element, vertices)| ElementCollider {
                    id: element,
                    solid: ConvexSolid {
                        vertices: vertices.into_iter().map(DVec3::from_array).collect(),
                    },
                })
                .collect();
            (id, colliders)
        })
        .collect()
}

fn decode_portals(
    portals: BTreeMap<String, Vec<ManifestPortal>>,
) -> BTreeMap<String, Vec<(String, PortalShape)>> {
    portals
        .into_iter()
        .map(|(id, portals)| {
            let portals = portals
                .into_iter()
                .map(|portal| {
                    (
                        portal.element,
                        PortalShape {
                            id: portal.id,
                            corners: portal.corners.map(DVec3::from_array),
                            normal: DVec3::from_array(portal.normal),
                        },
                    )
                })
                .collect();
            (id, portals)
        })
        .collect()
}

// ------------------------------------------------------------------ manifest

#[derive(Serialize, Deserialize)]
struct Manifest {
    recipe: BuildingRecipe,
    levels: Vec<ManifestLevel>,
    colliders: BTreeMap<String, Vec<(String, Vec<[f64; 3]>)>>,
    portals: BTreeMap<String, Vec<ManifestPortal>>,
}

#[derive(Serialize, Deserialize)]
struct ManifestLevel {
    until: Option<f32>,
    parts: BTreeMap<String, Vec<ManifestElement>>,
    groups: Vec<ManifestGroup>,
}

#[derive(Serialize, Deserialize)]
struct ManifestElement {
    id: String,
    material_slot: String,
    is_cut: bool,
    side: Side,
    mesh: u32,
}

#[derive(Serialize, Deserialize)]
struct ManifestGroup {
    id: String,
    storey: Option<i32>,
    operands: Vec<(String, String)>,
    batches: Vec<ManifestBatch>,
    bounds: Option<[[f64; 3]; 2]>,
    collider: Option<u32>,
}

#[derive(Serialize, Deserialize)]
struct ManifestBatch {
    binding: Binding,
    side: Side,
    mesh: u32,
}

#[derive(Serialize, Deserialize)]
struct ManifestPortal {
    element: String,
    id: String,
    corners: [[f64; 3]; 4],
    normal: [f64; 3],
}

// ------------------------------------------------------------------ mesh table

/// One mesh record: four `u32` counts — vertices, indices, faces, and whether
/// normals and UVs are present — then positions, normals and UVs as `f32`,
/// indices as `u32`, and one face source per triangle as an operand `u32`
/// and an origin `u32` whose top two bits are the kind.
#[derive(Default)]
struct MeshTable {
    bytes: Vec<u8>,
    count: u32,
    /// Every record written so far, by a hash of its bytes, with where it
    /// starts and ends: a part a coarser level did not change is the same mesh
    /// at every level, and the file stores it once.
    seen: std::collections::HashMap<u64, Vec<(u32, usize, usize)>>,
}

const ORIGIN_BODY: u32 = 0;
const ORIGIN_CUTTER: u32 = 1 << 30;
const ORIGIN_DAMAGE: u32 = 2 << 30;
const ORIGIN_KIND: u32 = 3 << 30;

impl MeshTable {
    fn push(&mut self, mesh: &TriangleMesh) -> u32 {
        self.record(
            &mesh.positions,
            Some((&mesh.normals, &mesh.uvs)),
            &mesh.indices,
            &mesh.sources,
        )
    }

    fn push_collider(&mut self, positions: &[DVec3], indices: &[u32]) -> u32 {
        self.record(positions, None, indices, &[])
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "a mesh's counts are u32 by the index type, and positions narrow to f32 by design"
    )]
    fn record(
        &mut self,
        positions: &[DVec3],
        shading: Option<(&Vec<DVec3>, &Vec<[f64; 2]>)>,
        indices: &[u32],
        sources: &[FaceSource],
    ) -> u32 {
        let start = self.bytes.len();
        let out = &mut self.bytes;
        for count in [
            positions.len() as u32,
            indices.len() as u32,
            sources.len() as u32,
            u32::from(shading.is_some()),
        ] {
            out.extend_from_slice(&count.to_le_bytes());
        }
        let floats = |out: &mut Vec<u8>, values: &mut dyn Iterator<Item = f64>| {
            for value in values {
                out.extend_from_slice(&(value as f32).to_le_bytes());
            }
        };
        floats(out, &mut positions.iter().flat_map(DVec3::to_array));
        if let Some((normals, uvs)) = shading {
            floats(out, &mut normals.iter().flat_map(DVec3::to_array));
            floats(out, &mut uvs.iter().flatten().copied());
        }
        for index in indices {
            out.extend_from_slice(&index.to_le_bytes());
        }
        for source in sources {
            out.extend_from_slice(&source.operand.to_le_bytes());
            let origin = match source.origin {
                FaceOrigin::Cutter(k) => ORIGIN_CUTTER | k,
                FaceOrigin::Damage(k) => ORIGIN_DAMAGE | k,
                FaceOrigin::Body => ORIGIN_BODY,
            };
            out.extend_from_slice(&origin.to_le_bytes());
        }
        let end = self.bytes.len();
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in &self.bytes[start..end] {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let earlier = self.seen.get(&hash).and_then(|records| {
            records
                .iter()
                .find(|(_, from, to)| self.bytes[*from..*to] == self.bytes[start..end])
        });
        if let Some((index, _, _)) = earlier {
            let index = *index;
            self.bytes.truncate(start);
            return index;
        }
        self.seen
            .entry(hash)
            .or_default()
            .push((self.count, start, end));
        self.count += 1;
        self.count - 1
    }
}

fn read_table(bytes: &[u8], count: usize) -> Result<Vec<TriangleMesh>, BakedError> {
    let mut cursor = Cursor { bytes, at: 0 };
    // A record is at least its sixteen bytes of counts.
    let mut meshes = Vec::with_capacity(count.min(bytes.len() / 16));
    for _ in 0..count {
        let vertices = cursor.u32("mesh vertex count")? as usize;
        let indices = cursor.u32("mesh index count")? as usize;
        let faces = cursor.u32("mesh face count")? as usize;
        let shaded = cursor.u32("mesh shading flag")? != 0;
        // Every array's bytes are checked against what is left before anything
        // is reserved for it, so a corrupt count is a refusal and never an
        // allocation the size of the count.
        let per_vertex = if shaded { 32 } else { 12 };
        let needed = vertices
            .checked_mul(per_vertex)
            .and_then(|bytes| bytes.checked_add(indices.checked_mul(4)?))
            .and_then(|bytes| bytes.checked_add(faces.checked_mul(8)?))
            .ok_or(BakedError::Truncated("mesh arrays"))?;
        if needed > cursor.remaining() {
            return Err(BakedError::Truncated("mesh arrays"));
        }
        let positions = cursor.vec3s(vertices, "positions")?;
        let (normals, uvs) = if shaded {
            let normals = cursor.vec3s(vertices, "normals")?;
            let mut uvs = Vec::with_capacity(vertices);
            for _ in 0..vertices {
                uvs.push([f64::from(cursor.f32("uvs")?), f64::from(cursor.f32("uvs")?)]);
            }
            (normals, uvs)
        } else {
            (Vec::new(), Vec::new())
        };
        let mut index_list = Vec::with_capacity(indices);
        for _ in 0..indices {
            let index = cursor.u32("indices")?;
            if index as usize >= vertices {
                return Err(BakedError::Mesh(format!(
                    "index {index} past {vertices} vertices"
                )));
            }
            index_list.push(index);
        }
        let mut sources = Vec::with_capacity(faces);
        for _ in 0..faces {
            let operand = cursor.u32("face sources")?;
            let origin = cursor.u32("face sources")?;
            let index = origin & !ORIGIN_KIND;
            let origin = match origin & ORIGIN_KIND {
                ORIGIN_CUTTER => FaceOrigin::Cutter(index),
                ORIGIN_DAMAGE => FaceOrigin::Damage(index),
                _ => FaceOrigin::Body,
            };
            sources.push(FaceSource { operand, origin });
        }
        if shaded && faces * 3 != indices {
            return Err(BakedError::Mesh(format!(
                "{faces} face sources for {indices} indices"
            )));
        }
        meshes.push(TriangleMesh {
            positions,
            normals,
            uvs,
            indices: index_list,
            sources,
        });
    }
    Ok(meshes)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }

    fn take(&mut self, len: usize, what: &'static str) -> Result<&'a [u8], BakedError> {
        let end = self
            .at
            .checked_add(len)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(BakedError::Truncated(what))?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn u32(&mut self, what: &'static str) -> Result<u32, BakedError> {
        let bytes = self.take(4, what)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn f32(&mut self, what: &'static str) -> Result<f32, BakedError> {
        Ok(f32::from_bits(self.u32(what)?))
    }

    fn len(&mut self, what: &'static str) -> Result<usize, BakedError> {
        let bytes = self.take(8, what)?;
        let mut word = [0; 8];
        word.copy_from_slice(bytes);
        usize::try_from(u64::from_le_bytes(word)).map_err(|_| BakedError::Truncated(what))
    }

    fn vec3s(&mut self, count: usize, what: &'static str) -> Result<Vec<DVec3>, BakedError> {
        let mut out = Vec::with_capacity(count.min(self.bytes.len() / 12 + 1));
        for _ in 0..count {
            let x = self.f32(what)?;
            let y = self.f32(what)?;
            let z = self.f32(what)?;
            if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                return Err(BakedError::Mesh(format!("a non-finite value in {what}")));
            }
            out.push(DVec3::new(f64::from(x), f64::from(y), f64::from(z)));
        }
        Ok(out)
    }
}
