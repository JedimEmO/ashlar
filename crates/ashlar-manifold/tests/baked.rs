//! A meshed building written as a baked file and read back is the same building
//! to within the `f32` the file stores, merged or not.
use ashlar::{
    BakedBuilding, BakedError, BakedLevel, Building, Collision, Element, Geometry, Instance,
    MergeGroup, Part, Pose, UvMode,
};
use ashlar_manifold::{ManifoldMesher, mesh_building};

fn bay() -> Part {
    Part::builder("bay")
        .element(
            Element::new(
                "shell",
                Geometry::cuboid([4.0, 3.0, 0.3]).subtract(
                    Geometry::cuboid([1.0, 2.0, 0.5])
                        .placed(Pose::at([1.5, 0.0, -0.1]))
                        .portal("door"),
                ),
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

fn wall(merged: bool) -> Building {
    let mut builder = Building::builder("wall")
        .part(bay())
        .material("surface", "paint")
        .material("reveal", "steel")
        .material("glass", "glass")
        .group(MergeGroup::new("ground").storey(0))
        .instance(Instance::new("a", "bay").group("ground"))
        .instance(
            Instance::new("b", "bay")
                .placed(Pose::at([4.0, 0.0, 0.0]))
                .group("ground"),
        );
    if merged {
        builder = builder.merged();
    }
    builder.build().expect("valid wall")
}

fn round_trip(merged: bool) {
    let meshed = mesh_building(&wall(merged), &ManifoldMesher::default()).expect("meshes");
    let baked = BakedBuilding::new(
        meshed.building.clone(),
        vec![BakedLevel {
            until: Some(80.0),
            parts: meshed.parts.clone(),
            groups: meshed.groups.clone(),
        }],
        meshed.part_colliders.clone(),
        meshed.part_portals.clone(),
    );
    let bytes = baked.write().expect("writes");
    let back = BakedBuilding::read(&bytes).expect("reads");
    assert_eq!(back.building().recipe(), meshed.building.recipe());
    assert_eq!(back.levels[0].until, Some(80.0));
    assert_eq!(back.triangles(), baked.triangles());
    let level = back.level(0).expect("level zero");
    assert_eq!(level.portals().len(), meshed.portals().len());
    assert_eq!(level.colliders().len(), meshed.colliders().len());
    let original: Vec<_> = meshed.pieces().collect();
    let read: Vec<_> = level.pieces().collect();
    assert_eq!(original.len(), read.len());
    for (a, b) in original.iter().zip(&read) {
        assert_eq!(a.binding, b.binding);
        assert_eq!(a.mesh.indices, b.mesh.indices);
        assert_eq!(a.mesh.sources, b.mesh.sources);
        for (p, q) in a.mesh.positions.iter().zip(&b.mesh.positions) {
            assert!(p.distance(*q) < 1e-5, "{p} {q}");
        }
    }
    assert_eq!(merged, !back.levels[0].groups.is_empty());
}

#[test]
fn an_unmerged_building_round_trips() {
    round_trip(false);
}

#[test]
fn a_merged_building_round_trips_with_its_group_collider() {
    round_trip(true);
}

#[test]
fn the_reader_refuses_what_it_cannot_account_for() {
    let meshed = mesh_building(&wall(false), &ManifoldMesher::default()).expect("meshes");
    let baked = BakedBuilding::new(
        meshed.building.clone(),
        vec![BakedLevel {
            until: None,
            parts: meshed.parts.clone(),
            groups: Vec::new(),
        }],
        meshed.part_colliders.clone(),
        meshed.part_portals.clone(),
    );
    let bytes = baked.write().expect("writes");
    assert!(matches!(
        BakedBuilding::read(b"not a building"),
        Err(BakedError::Identifier | BakedError::Truncated(_))
    ));
    let mut version = bytes.clone();
    version[12] = 99;
    assert!(matches!(
        BakedBuilding::read(&version),
        Err(BakedError::Version(99))
    ));
    for cut in [20, bytes.len() / 2, bytes.len() - 1] {
        assert!(BakedBuilding::read(&bytes[..cut]).is_err(), "cut at {cut}");
    }
}

#[test]
fn a_corrupt_count_is_refused_without_reserving_it() {
    let meshed = mesh_building(&wall(false), &ManifoldMesher::default()).expect("meshes");
    let baked = BakedBuilding::new(
        meshed.building.clone(),
        vec![BakedLevel {
            until: None,
            parts: meshed.parts.clone(),
            groups: Vec::new(),
        }],
        meshed.part_colliders.clone(),
        meshed.part_portals.clone(),
    );
    let mut bytes = baked.write().expect("writes");
    // The first mesh record's vertex count sits right after the table's two
    // length words; claim four billion vertices there.
    let manifest = usize::try_from(u64::from_le_bytes(
        bytes[16..24].try_into().expect("an eight-byte length"),
    ))
    .expect("a manifest length that fits");
    let first_record = 24 + manifest + 16;
    bytes[first_record..first_record + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        BakedBuilding::read(&bytes),
        Err(BakedError::Truncated(_))
    ));
}

#[test]
fn a_level_naming_a_slot_the_recipe_does_not_declare_is_refused() {
    let meshed = mesh_building(&wall(false), &ManifoldMesher::default()).expect("meshes");
    let mut parts = meshed.parts.clone();
    for elements in parts.values_mut() {
        elements[0].material_slot = "nowhere".into();
    }
    let baked = BakedBuilding::new(
        meshed.building.clone(),
        vec![BakedLevel {
            until: None,
            parts,
            groups: Vec::new(),
        }],
        meshed.part_colliders.clone(),
        meshed.part_portals.clone(),
    );
    // Refused on the way out, by the same check a read makes.
    assert!(matches!(baked.write(), Err(BakedError::Mesh(_))));
}

#[test]
fn only_the_last_level_may_draw_to_any_distance() {
    let meshed = mesh_building(&wall(false), &ManifoldMesher::default()).expect("meshes");
    let level = |until| BakedLevel {
        until,
        parts: meshed.parts.clone(),
        groups: Vec::new(),
    };
    for levels in [
        vec![level(None), level(Some(100.0))],
        vec![level(Some(100.0)), level(Some(50.0))],
    ] {
        let baked = BakedBuilding::new(
            meshed.building.clone(),
            levels,
            meshed.part_colliders.clone(),
            meshed.part_portals.clone(),
        );
        assert!(baked.write().is_err());
    }
}
