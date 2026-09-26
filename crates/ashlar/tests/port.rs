//! The port's default group path: a backend that only meshes refuses one.

use ashlar::{Geometry, GeometryMesher, MeshError, TriangleMesh};

struct MeshOnly;

impl GeometryMesher for MeshOnly {
    fn mesh(&self, _geometry: &Geometry) -> Result<TriangleMesh, MeshError> {
        Ok(TriangleMesh::default())
    }
}

#[test]
fn a_backend_that_only_meshes_refuses_a_group() {
    let error = MeshOnly
        .mesh_group(&[], &[])
        .expect_err("the default must refuse");
    assert_eq!(error.path, "group");
}
