//! The join between a meshed building and its materials, without a kernel.
//!
//! `MeshedBuilding` is plain data, so a case can state exactly which batches
//! came back and assert what the walk resolves on them. What a real mesher
//! produces from a real solid is `ashlar-manifold`'s own test.
use std::collections::BTreeMap;

use ashlar::{
    Binding, Building, DamageLog, Element, ElementMesh, FaceSource, Geometry, Instance,
    MeshedBuilding, ParamValue, Part, Piece, PieceOrigin, Pose, Side, TriangleMesh,
};
use glam::DVec3;

/// One triangle, enough to be a non-empty batch.
fn triangle() -> TriangleMesh {
    let mut mesh = TriangleMesh::default();
    mesh.push_triangle(
        [DVec3::ZERO, DVec3::X, DVec3::Y],
        [DVec3::Z; 3],
        [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        FaceSource::BODY,
    );
    mesh
}

/// The `(instance, element, slot, is_cut)` of an element piece, panicking when a
/// test that wants one meets a group batch instead.
fn element_of<'a>(piece: &Piece<'a>) -> (&'a str, &'a str, &'a str, bool) {
    match piece.origin {
        PieceOrigin::Element {
            instance,
            element,
            slot,
            is_cut,
            ..
        } => (instance, element, slot, is_cut),
        _ => panic!("expected an element piece, found {:?}", piece.origin),
    }
}

/// A part whose one element dresses its cut faces separately, placed twice: the
/// second instance overrides the outer slot with a key and a parameter value,
/// and leaves the cut slot to the building's palette.
fn building() -> Building {
    let bay = Part::builder("bay")
        .element(
            Element::new("shell", Geometry::cuboid([4.0, 3.0, 0.3]), "outer")
                .cut_material("reveal"),
        )
        .element(Element::new(
            "sign",
            Geometry::cuboid([1.0, 0.4, 0.05]),
            "outer",
        ))
        .build()
        .expect("valid reusable bay");
    Building::builder("wall")
        .part(bay)
        .material("outer", "brick")
        .material("reveal", "plaster")
        .instance(Instance::new("first", "bay"))
        .instance(
            Instance::new("second", "bay")
                .placed(Pose::at([4.0, 0.0, 0.0]))
                .binding(
                    "outer",
                    Binding::new("brick").param("variation", ParamValue::Int(7)),
                ),
        )
        .build()
        .expect("valid assembly")
}

/// The batches a mesher would hand back for that part: an outer and a cut batch
/// for the shell, one batch for the sign, and the empty batch a fully
/// subtracted element leaves behind.
fn meshed() -> MeshedBuilding {
    let batch = |id: &str, slot: &str, is_cut: bool, mesh: TriangleMesh| ElementMesh {
        id: id.into(),
        material_slot: slot.into(),
        is_cut,
        side: Side::Exterior,
        mesh,
    };
    MeshedBuilding {
        building: building(),
        parts: BTreeMap::from([(
            "bay".to_owned(),
            vec![
                batch("shell", "outer", false, triangle()),
                batch("shell", "reveal", true, triangle()),
                batch("sign", "outer", false, TriangleMesh::default()),
            ],
        )]),
        part_colliders: BTreeMap::new(),
        part_portals: BTreeMap::new(),
        groups: Vec::new(),
        damage: DamageLog::default(),
    }
}

#[test]
fn surfaces_walk_instances_then_elements_and_keep_empty_batches() {
    let meshed = meshed();
    let order: Vec<(&str, &str, bool)> = meshed
        .pieces()
        .map(|piece| {
            let (instance, element, _, is_cut) = element_of(&piece);
            (instance, element, is_cut)
        })
        .collect();
    assert_eq!(
        order,
        [
            ("first", "shell", false),
            ("first", "shell", true),
            ("first", "sign", false),
            ("second", "shell", false),
            ("second", "shell", true),
            ("second", "sign", false),
        ],
        "every batch of every instance, in the order the recipe declares them"
    );
    assert_eq!(
        meshed
            .pieces()
            .filter(|piece| piece.mesh.positions.is_empty())
            .count(),
        2,
        "the empty batch is yielded rather than skipped, once per instance"
    );
    // The pose is the instance's, and the mesh is the part's: two placements of
    // one part borrow the same triangles rather than a copy each.
    let poses: Vec<DVec3> = meshed
        .pieces()
        .map(|piece| piece.pose.translation)
        .collect();
    assert_eq!(poses[0], DVec3::ZERO);
    assert_eq!(poses[3], DVec3::new(4.0, 0.0, 0.0));
    assert!(std::ptr::eq(
        meshed.pieces().next().expect("a first piece").mesh,
        meshed.pieces().nth(3).expect("a fourth piece").mesh,
    ));
}

#[test]
fn a_cut_batch_resolves_the_cut_slot_and_an_instance_override_wins() {
    let meshed = meshed();
    let bindings: Vec<(&str, bool, &str)> = meshed
        .pieces()
        .map(|piece| {
            let (instance, _, _, is_cut) = element_of(&piece);
            (instance, is_cut, piece.binding.material.as_str())
        })
        .collect();
    assert_eq!(
        bindings,
        [
            ("first", false, "brick"),
            ("first", true, "plaster"),
            ("first", false, "brick"),
            ("second", false, "brick"),
            ("second", true, "plaster"),
            ("second", false, "brick"),
        ],
        "a cut batch wears the cut slot's binding and an outer batch its own"
    );
    // The second instance overrode the outer slot only, so its cut faces still
    // come from the building's palette and are the same binding the first
    // instance's are.
    let overridden: Vec<&Binding> = meshed
        .pieces()
        .filter(|piece| element_of(piece).0 == "second")
        .map(|piece| piece.binding)
        .collect();
    assert_eq!(
        overridden[0].params.get("variation"),
        Some(&ParamValue::Int(7)),
        "the instance's own values ride on the binding the walk resolves"
    );
    assert!(
        overridden[1].params.is_empty(),
        "the cut slot was not touched"
    );
}

#[test]
fn the_shared_key_separates_an_override_and_not_a_placement() {
    let meshed = meshed();
    let keys: Vec<_> = meshed
        .pieces()
        .map(|piece| {
            let (key, binding) = piece.shared_key();
            (key, binding.clone())
        })
        .collect();
    assert_eq!(
        keys[1], keys[4],
        "two placements of one part over one binding share their scatter, and \
         the cut slot is a binding neither instance touched"
    );
    assert_ne!(
        keys[0], keys[3],
        "an override is a different binding and so a different key, even though \
         it is the same element of the same part"
    );
    assert_eq!(
        keys.iter().collect::<std::collections::BTreeSet<_>>().len(),
        5,
        "the shell's cut batch is the only one of the six the two instances share"
    );
}

#[test]
fn a_piece_carries_the_slot_of_the_batch_it_came_from() {
    let meshed = meshed();
    let batches = &meshed.parts["bay"];
    let expected: Vec<&str> = meshed
        .building
        .recipe()
        .instances
        .iter()
        .flat_map(|_| batches.iter().map(|batch| batch.material_slot.as_str()))
        .collect();
    let actual: Vec<&str> = meshed.pieces().map(|piece| element_of(&piece).2).collect();
    assert_eq!(
        actual, expected,
        "each piece reports the slot of the batch the walk took it from"
    );
}
