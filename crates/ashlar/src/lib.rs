//! Style-independent building descriptions, with a renderer-independent mesh backend port.
//!
//! This is the bottom of the `ashlar` stack: plain data, builders and
//! validation, with no Bevy, no IO and no solid kernel. `ashlar-manifold`
//! evaluates what is described here into triangles, and `ashlar-bevy` turns
//! those into engine assets. A dedicated server can depend on this crate alone
//! and carry neither a renderer nor a C++ toolchain.
//!
//! Recipes use metres, right-handed Y-up coordinates, and `f64` local positions.
//! They contain no world placement, renderer handles, filesystem access or style
//! catalogue. [`BuildingRecipe`] is editable interchange data; [`Building`] is
//! an immutable, validated snapshot. Deserialized recipes must call
//! [`BuildingRecipe::build`] before use.
//!
//! Geometry operations describe intent only. This crate does not tessellate,
//! resolve material assets or evaluate booleans. Collision proxies are declared
//! here and derived by the mesh backend.
//!
//! # A part, and two bays of it
//!
//! A [`Part`] is a reusable solid with named attachment frames. An [`Instance`]
//! places one, either at a pose or by bringing one of its [`Socket`]s onto
//! another instance's. Material slots are opaque keys bound per building and
//! overridable per instance — and an instance's [`Binding`] may carry its own
//! values for the *graph* parameters of the material it names, so a row of
//! buildings wears one key at a seed each rather than one key each.
//!
//! ```
//! use ashlar::{
//!     Building, Collision, Element, Geometry, Instance, Part, Pose, Socket,
//!     glam::{DQuat, DVec3},
//! };
//! use std::f64::consts::FRAC_PI_2;
//!
//! // Sockets point outward along +Z, so a bay's own edges face -X and +X.
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
//!     .socket(Socket::new(
//!         "left",
//!         Pose::default().rotated(DQuat::from_rotation_y(-FRAC_PI_2)),
//!     ))
//!     .socket(Socket::new(
//!         "right",
//!         Pose::at([4.0, 0.0, 0.0]).rotated(DQuat::from_rotation_y(FRAC_PI_2)),
//!     ))
//!     .build()?;
//!
//! let building = Building::builder("example:wall")
//!     .part(bay)
//!     .material("surface", "example:painted_metal")
//!     .instance(Instance::new("first", "example:bay"))
//!     .instance(
//!         Instance::new("second", "example:bay")
//!             .attach("left", "first", "right")
//!             .material("surface", "example:ceramic"),
//!     )
//!     .build()?;
//!
//! // The attachment resolved into a pose one bay along, unrotated.
//! let second = building.instance("second").expect("placed");
//! assert!(second.pose.translation.abs_diff_eq(DVec3::new(4.0, 0.0, 0.0), 1e-9));
//! // The instance override wins over the building's default binding.
//! assert_eq!(building.material("first", "surface"), Some("example:painted_metal"));
//! assert_eq!(building.material("second", "surface"), Some("example:ceramic"));
//! # Ok::<(), ashlar::ValidationError>(())
//! ```
//!
//! # glam
//!
//! Positions are [`glam`] `f64` vectors, and the crate re-exports the exact
//! version it was built against as [`ashlar::glam`](glam). A consumer that also
//! uses Bevy must resolve to that same glam, or `DVec3` in a recipe is a
//! different type from `DVec3` in the renderer and every call converts at the
//! boundary. Depend on the re-export rather than a second `glam` entry when in
//! doubt.

pub use glam;

mod baked;
mod collision;
mod damage;
mod geometry;
mod ground;
mod lod;
mod mesh;
mod meshed;
mod normals;
mod recipe;

pub use ashlar_surface::{
    Bake, Binding, BoundSlots, MaterialDefinition, MaterialLibrary, ParamValue, StrandSettings,
    Surface, ValidationError,
};
pub use baked::{BakedBuilding, BakedError, BakedLevel};
pub use collision::{Collision, ConvexSolid};
pub use lod::{LodPolicy, MIN_SEGMENTS};
/// The baked building file's identifier and version.
pub mod baked_file {
    pub use crate::baked::{IDENTIFIER, VERSION};
}
pub use damage::{Damage, DamageLog};
pub use geometry::{Axis, Geometry, MAX_ARRAY_COUNT, MAX_HULL_POINTS, MirrorPlane, Pose, Shape};
pub use ground::{
    Diagonal, GroundError, GroundRect, GroundSolid, Grounding, GroundingPolicy, GroundingSpec,
    TerrainPatch, fit_ground,
};
pub use mesh::{
    FaceOrigin, FaceSource, GeometryMesher, GroupMesh, MeshError, PlacedCut, PlacedGeometry,
    PortalShape, TriangleMesh, UvMode, WELD_TOLERANCE, box_uvs, cylindrical_uvs, planar_uvs,
    project_uvs,
};
pub use meshed::{
    ElementCollider, ElementMesh, GroupBatch, InstanceCollider, MergedGroup, MeshKey,
    MeshedBuilding, Operand, Piece, PieceOrigin, Portal, TriangleCollider,
};
pub use normals::{SHARP_ANGLE_TIE, shade_normals};
pub use recipe::{
    Building, BuildingBuilder, BuildingRecipe, Element, GroupMembers, Instance, MergeGroup, Part,
    PartBuilder, Room, SCHEMA_VERSION, Side, Socket,
};

pub(crate) use ashlar_surface::{name, require};
