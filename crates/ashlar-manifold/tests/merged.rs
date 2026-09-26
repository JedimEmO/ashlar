//! Merged buildings: unioned groups, per-face bindings and building-space UVs.

use ashlar::{
    Axis, Building, Collision, Element, FaceOrigin, Geometry, GeometryMesher, Instance, MergeGroup,
    MeshError, Part, PieceOrigin, Pose, Side, TriangleMesh, UvMode,
};
use ashlar_manifold::{ManifoldMesher, mesh_building};
use glam::DVec3;

fn volume(mesh: &TriangleMesh) -> f64 {
    mesh.triangles()
        .map(|triangle| triangle[0].dot(triangle[1].cross(triangle[2])) / 6.0)
        .sum()
}

fn triangle_area([a, b, c]: [DVec3; 3]) -> f64 {
    (b - a).cross(c - a).length() * 0.5
}

fn area(mesh: &TriangleMesh) -> f64 {
    mesh.triangles().map(triangle_area).sum()
}

/// The part every wall test places: a wall with one opening whose reveal wears
/// its own slot, and a pane that stays its own mesh.
fn bay() -> Part {
    Part::builder("bay")
        .element(
            Element::new(
                "shell",
                Geometry::cuboid([4.0, 3.0, 0.3])
                    .subtract(Geometry::cuboid([1.0, 2.0, 0.5]).placed(Pose::at([1.5, 0.0, -0.1]))),
                "surface",
            )
            .cut_material("reveal")
            .uv(UvMode::Box)
            .collision(Collision::Bounds),
        )
        .element(
            Element::new(
                "glass",
                Geometry::cuboid([1.0, 2.0, 0.02]).placed(Pose::at([1.5, 0.0, 0.14])),
                "glass",
            )
            .standalone(),
        )
        .build()
        .expect("valid bay")
}

/// Three bays in a row, the middle one wearing a brick face instead of paint.
fn wall(merged: bool) -> Building {
    let mut builder = Building::builder("wall")
        .part(bay())
        .material("surface", "paint")
        .material("reveal", "steel")
        .material("glass", "glass")
        .instance(Instance::new("a", "bay"))
        .instance(
            Instance::new("b", "bay")
                .placed(Pose::at([4.0, 0.0, 0.0]))
                .material("surface", "brick"),
        )
        .instance(Instance::new("c", "bay").placed(Pose::at([8.0, 0.0, 0.0])));
    if merged {
        builder = builder.merged();
    }
    builder.build().expect("valid wall")
}

#[test]
fn an_unmerged_building_is_meshed_as_before() {
    let meshed = mesh_building(&wall(false), &ManifoldMesher::default()).expect("meshes");
    assert!(meshed.groups.is_empty());
    let elements: Vec<&str> = meshed.parts["bay"]
        .iter()
        .map(|element| element.id.as_str())
        .collect();
    assert_eq!(
        elements,
        ["shell", "shell", "glass"],
        "shell outer, shell cut and the standalone pane"
    );
}

#[test]
fn a_merged_building_has_one_group_and_keeps_its_standalone_elements() {
    let meshed = mesh_building(&wall(true), &ManifoldMesher::default()).expect("meshes");
    assert_eq!(meshed.groups.len(), 1);
    let group = &meshed.groups[0];
    assert_eq!(group.id, "wall", "the default group is the building");
    let operands: Vec<(&str, &str)> = group
        .operands
        .iter()
        .map(|operand| (operand.instance.as_str(), operand.element.as_str()))
        .collect();
    assert_eq!(operands, [("a", "shell"), ("b", "shell"), ("c", "shell")]);
    let elements: Vec<&str> = meshed.parts["bay"]
        .iter()
        .map(|element| element.id.as_str())
        .collect();
    assert_eq!(
        elements,
        ["glass"],
        "the merged shells are drawn as a group"
    );
}

#[test]
fn the_group_is_one_solid() {
    let meshed = mesh_building(&wall(true), &ManifoldMesher::default()).expect("meshes");
    let group = &meshed.groups[0];
    let total: f64 = group.batches.iter().map(|batch| volume(&batch.mesh)).sum();
    assert!((total - 9.0).abs() < 1e-9, "volume {total}");
    let total_area: f64 = group.batches.iter().map(|batch| area(&batch.mesh)).sum();
    assert!(
        (total_area - 72.6).abs() < 1e-6,
        "union area {total_area}, three shells less the two former joints"
    );
    for junction in [4.0, 8.0] {
        for batch in &group.batches {
            for triangle in batch.mesh.triangles() {
                assert!(
                    !triangle
                        .iter()
                        .all(|point| (point.x - junction).abs() < 1e-9),
                    "a triangle lies in the former joint at x = {junction}"
                );
            }
        }
    }
}

#[test]
fn every_face_wears_the_binding_its_own_instance_resolves() {
    let meshed = mesh_building(&wall(true), &ManifoldMesher::default()).expect("meshes");
    let group = &meshed.groups[0];
    assert_eq!(group.batches.len(), 3);
    let batch = |material: &str| {
        group
            .batches
            .iter()
            .find(|batch| batch.binding.material == material)
            .expect("a batch for the material")
    };

    for source in &batch("brick").mesh.sources {
        assert_eq!(source.operand, 1, "the override is the middle instance");
        assert_eq!(source.origin, FaceOrigin::Body);
    }

    let steel = batch("steel");
    let mut operands: Vec<u32> = steel
        .mesh
        .sources
        .iter()
        .map(|source| source.operand)
        .collect();
    operands.sort_unstable();
    operands.dedup();
    for source in &steel.mesh.sources {
        assert_eq!(source.origin, FaceOrigin::Cutter(0), "{source:?}");
    }
    assert_eq!(operands, [0, 1, 2], "every reveal is present");

    let paint = batch("paint");
    let mut operands: Vec<u32> = paint
        .mesh
        .sources
        .iter()
        .map(|source| source.operand)
        .collect();
    operands.sort_unstable();
    operands.dedup();
    assert_eq!(operands, [0, 2], "the middle instance is not painted");
}

#[test]
fn box_uvs_are_continuous_across_a_former_joint() {
    let meshed = mesh_building(&wall(true), &ManifoldMesher::default()).expect("meshes");
    let group = &meshed.groups[0];
    let mut checked = 0;
    for batch in group
        .batches
        .iter()
        .filter(|batch| batch.binding.material == "paint" || batch.binding.material == "brick")
    {
        for ((position, normal), uv) in batch
            .mesh
            .positions
            .iter()
            .zip(&batch.mesh.normals)
            .zip(&batch.mesh.uvs)
        {
            if (position.z - 0.3).abs() > 1e-9 || (normal - DVec3::Z).length() > 1e-6 {
                continue;
            }
            checked += 1;
            assert!(
                (uv[0] - position.x).abs() < 1e-9 && (uv[1] - position.y).abs() < 1e-9,
                "uv {uv:?} is not the building position {position}"
            );
        }
    }
    assert!(checked > 0, "the front face was found");
}

#[test]
fn colliders_do_not_change_when_a_building_is_merged() {
    let unmerged = mesh_building(&wall(false), &ManifoldMesher::default()).expect("meshes");
    let merged = mesh_building(&wall(true), &ManifoldMesher::default()).expect("meshes");
    let left = unmerged.colliders();
    let right = merged.colliders();
    assert_eq!(left.len(), right.len());
    for (before, after) in left.iter().zip(&right) {
        assert_eq!(before.instance, after.instance);
        assert_eq!(before.element, after.element);
        assert_eq!(before.solid.vertices, after.solid.vertices);
    }
}

#[test]
fn declared_groups_are_meshed_separately() {
    let building = Building::builder("wall")
        .part(bay())
        .material("surface", "paint")
        .material("reveal", "steel")
        .material("glass", "glass")
        .merged()
        .group(MergeGroup::new("low"))
        .group(MergeGroup::new("high"))
        .instance(Instance::new("a", "bay").group("low"))
        .instance(
            Instance::new("b", "bay")
                .placed(Pose::at([4.0, 0.0, 0.0]))
                .group("low"),
        )
        .instance(
            Instance::new("c", "bay")
                .placed(Pose::at([8.0, 0.0, 0.0]))
                .group("high"),
        )
        .build()
        .expect("valid wall");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshes");
    assert_eq!(meshed.groups.len(), 2);
    assert_eq!(meshed.groups[0].id, "low");
    assert_eq!(meshed.groups[1].id, "high");
    assert_eq!(meshed.groups[0].operands.len(), 2);
    assert_eq!(meshed.groups[1].operands.len(), 1);
    let low = meshed.groups[0].bounds.expect("low bounds");
    let high = meshed.groups[1].bounds.expect("high bounds");
    assert!(
        low[1].x <= high[0].x + 1e-9 && high[0].x <= low[1].x + 1e-9,
        "low {low:?} and high {high:?} overlap in x"
    );
}

#[test]
fn a_cylindrical_element_keeps_its_wrap_when_merged() {
    let column = Part::builder("column")
        .element(
            Element::new("shaft", Geometry::cylinder(0.5, 3.0, 24), "skin")
                .uv(UvMode::Cylindrical { axis: Axis::Y }),
        )
        .build()
        .expect("valid column");
    let building = Building::builder("court")
        .part(column)
        .material("skin", "stone")
        .merged()
        .instance(Instance::new("column", "column").placed(Pose::at([10.0, 0.0, 5.0])))
        .build()
        .expect("valid court");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshes");
    let group = &meshed.groups[0];
    let mut checked = 0;
    for batch in &group.batches {
        for ((position, normal), uv) in batch
            .mesh
            .positions
            .iter()
            .zip(&batch.mesh.normals)
            .zip(&batch.mesh.uvs)
        {
            if normal.y.abs() > 1e-6 {
                continue;
            }
            checked += 1;
            assert!(
                (uv[1] - position.y).abs() < 1e-9,
                "v {} is not the height {}",
                uv[1],
                position.y
            );
            assert!(
                uv[0].abs() < 0.5 * std::f64::consts::TAU + 1e-6,
                "u {} wraps the building, not the column",
                uv[0]
            );
        }
    }
    assert!(checked > 0, "the side of the column was found");
}

/// A backend that evaluates single geometries but has no union of its own, so
/// it keeps the trait's default `mesh_group`.
struct OnlyMesh(ManifoldMesher);

impl GeometryMesher for OnlyMesh {
    fn mesh(&self, geometry: &Geometry) -> Result<TriangleMesh, MeshError> {
        self.0.mesh(geometry)
    }
}

#[test]
fn a_backend_that_cannot_union_fails_a_merged_building_by_path() {
    let error = mesh_building(&wall(true), &OnlyMesh(ManifoldMesher::default()))
        .expect_err("a backend with no union refuses");
    assert!(error.path.starts_with("groups["), "{}", error.path);
    assert!(
        mesh_building(&wall(false), &OnlyMesh(ManifoldMesher::default())).is_ok(),
        "the unmerged path does not union"
    );
}

#[test]
fn a_merged_building_yields_group_pieces_then_standalone_pieces() {
    let meshed = mesh_building(&wall(true), &ManifoldMesher::default()).expect("meshes");
    let pieces: Vec<_> = meshed.pieces().collect();
    let groups: Vec<usize> = pieces[..3]
        .iter()
        .map(|piece| match piece.origin {
            PieceOrigin::Group { batch, .. } => batch,
            _ => panic!("expected a group batch, found {:?}", piece.origin),
        })
        .collect();
    assert_eq!(groups, [0, 1, 2], "the one group's three batches, in order");
    for piece in &pieces[..3] {
        assert_eq!(
            piece.pose,
            Pose::default(),
            "a group is meshed in building space and stands at the identity"
        );
    }
    let elements: Vec<&str> = pieces[3..]
        .iter()
        .map(|piece| match piece.origin {
            PieceOrigin::Element { element, .. } => element,
            _ => panic!("expected an element piece, found {:?}", piece.origin),
        })
        .collect();
    assert_eq!(
        elements,
        ["glass", "glass", "glass"],
        "the standalone panes follow the group, one per instance"
    );
}

#[test]
fn a_group_piece_borrows_the_batch_and_its_binding() {
    let meshed = mesh_building(&wall(true), &ManifoldMesher::default()).expect("meshes");
    let group = &meshed.groups[0];
    let mut seen = 0;
    for piece in meshed.pieces() {
        let PieceOrigin::Group { batch, .. } = piece.origin else {
            break;
        };
        seen += 1;
        assert_eq!(
            piece.mesh.triangle_count(),
            group.batches[batch].mesh.triangle_count()
        );
        assert_eq!(piece.binding, &group.batches[batch].binding);
    }
    assert_eq!(seen, group.batches.len(), "every batch was visited");
}

#[test]
fn mesh_keys_share_elements_and_never_groups() {
    let meshed = mesh_building(&wall(true), &ManifoldMesher::default()).expect("meshes");
    let pieces: Vec<_> = meshed.pieces().collect();
    let glass: Vec<_> = pieces
        .iter()
        .filter(|piece| {
            matches!(
                piece.origin,
                PieceOrigin::Element {
                    element: "glass",
                    ..
                }
            )
        })
        .collect();
    assert_eq!(glass.len(), 3, "one pane per instance");
    for pair in glass.windows(2) {
        assert_eq!(
            pair[0].mesh_key(),
            pair[1].mesh_key(),
            "every placement of the pane shares its triangles"
        );
    }
    let groups: Vec<_> = pieces
        .iter()
        .filter_map(|piece| match piece.origin {
            PieceOrigin::Group { .. } => Some(piece.mesh_key()),
            _ => None,
        })
        .collect();
    assert_eq!(groups.len(), 3);
    let distinct: std::collections::BTreeSet<_> = groups.iter().collect();
    assert_eq!(
        distinct.len(),
        3,
        "each group batch is its own geometry and is shared with nothing"
    );
}

#[test]
fn labels_name_the_piece() {
    let meshed = mesh_building(&wall(true), &ManifoldMesher::default()).expect("meshes");
    let glass = meshed
        .pieces()
        .find(|piece| {
            matches!(
                piece.origin,
                PieceOrigin::Element {
                    instance: "a",
                    element: "glass",
                    ..
                }
            )
        })
        .expect("the first instance's pane");
    assert_eq!(glass.label(), "a/glass");
    let group = meshed
        .pieces()
        .find(|piece| matches!(piece.origin, PieceOrigin::Group { .. }))
        .expect("a group batch");
    assert_eq!(group.label(), format!("wall/{}", group.binding.material));
}

#[test]
fn a_piece_carries_its_groups_storey() {
    let building = Building::builder("wall")
        .part(bay())
        .material("surface", "paint")
        .material("reveal", "steel")
        .material("glass", "glass")
        .merged()
        .group(MergeGroup::new("low").storey(1))
        .instance(Instance::new("a", "bay").group("low"))
        .instance(
            Instance::new("b", "bay")
                .placed(Pose::at([4.0, 0.0, 0.0]))
                .group("low"),
        )
        .instance(
            Instance::new("c", "bay")
                .placed(Pose::at([8.0, 0.0, 0.0]))
                .group("low"),
        )
        .build()
        .expect("valid wall");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshes");
    let mut pieces = 0;
    for piece in meshed.pieces() {
        pieces += 1;
        assert_eq!(piece.storey, Some(1), "{:?}", piece.origin);
    }
    assert!(pieces > 0, "the building yielded pieces");
}

#[test]
fn a_liner_is_its_own_interior_batch_with_no_seam() {
    let part = Part::builder("wall")
        .element(Element::new(
            "shell",
            Geometry::cuboid([4.0, 3.0, 0.3]),
            "plaster",
        ))
        .element(
            Element::new(
                "liner",
                Geometry::cuboid([4.0, 3.0, 0.02]).placed(Pose::at([0.0, 0.0, -0.02])),
                "plaster",
            )
            .interior(),
        )
        .build()
        .expect("valid part");
    let building = Building::builder("wall")
        .part(part)
        .material("plaster", "plaster")
        .merged()
        .instance(Instance::new("a", "wall"))
        .build()
        .expect("valid building");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshes");
    assert_eq!(meshed.groups.len(), 1);
    let group = &meshed.groups[0];
    assert_eq!(group.batches.len(), 2, "one plaster, two declared sides");
    assert_eq!(group.batches[0].binding, group.batches[1].binding);
    assert_eq!(group.batches[0].side, Side::Exterior);
    assert_eq!(group.batches[1].side, Side::Interior);
    let total: f64 = group.batches.iter().map(|batch| volume(&batch.mesh)).sum();
    assert!((total - 3.84).abs() < 1e-9, "volume {total}");
    for batch in &group.batches {
        for triangle in batch.mesh.triangles() {
            assert!(
                !triangle.iter().all(|point| point.z.abs() < 1e-9),
                "a triangle lies in the former internal wall"
            );
        }
    }
    for source in &group.batches[1].mesh.sources {
        assert_eq!(source.operand, 1, "the interior faces are the liner");
    }
    for source in &group.batches[0].mesh.sources {
        assert_eq!(source.operand, 0, "the exterior faces are the shell");
    }
}

#[test]
fn a_standalone_interior_element_says_so_on_its_piece() {
    let part = Part::builder("wall")
        .element(Element::new(
            "shell",
            Geometry::cuboid([4.0, 3.0, 0.3]),
            "plaster",
        ))
        .element(
            Element::new("liner", Geometry::cuboid([1.0, 1.0, 0.1]), "plaster")
                .standalone()
                .interior(),
        )
        .build()
        .expect("valid part");
    let building = Building::builder("wall")
        .part(part)
        .material("plaster", "plaster")
        .merged()
        .instance(Instance::new("a", "wall"))
        .build()
        .expect("valid building");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshes");
    let mut saw_interior_element = false;
    let mut saw_exterior_group = false;
    for piece in meshed.pieces() {
        match piece.origin {
            PieceOrigin::Element { element, .. } => {
                assert_eq!(element, "liner");
                assert_eq!(piece.side, Side::Interior);
                saw_interior_element = true;
            }
            PieceOrigin::Group { .. } => {
                assert_eq!(piece.side, Side::Exterior);
                saw_exterior_group = true;
            }
            _ => panic!("unexpected piece origin {:?}", piece.origin),
        }
    }
    assert!(saw_interior_element, "the standalone liner was drawn");
    assert!(saw_exterior_group, "the shell was unioned into the group");
}

#[test]
fn an_unmerged_building_carries_side_on_its_element_meshes() {
    let part = Part::builder("wall")
        .element(Element::new(
            "shell",
            Geometry::cuboid([4.0, 3.0, 0.3]),
            "plaster",
        ))
        .element(Element::new("liner", Geometry::cuboid([4.0, 3.0, 0.02]), "plaster").interior())
        .build()
        .expect("valid part");
    let building = Building::builder("wall")
        .part(part)
        .material("plaster", "plaster")
        .instance(Instance::new("a", "wall"))
        .build()
        .expect("valid building");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshes");
    let sides: Vec<Side> = meshed.parts["wall"]
        .iter()
        .map(|element| element.side)
        .collect();
    assert_eq!(sides, [Side::Exterior, Side::Interior]);
}
