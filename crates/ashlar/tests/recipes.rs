//! Authoring and interchange invariants, independent of a renderer or asset files.
use ashlar::{
    Binding, Building, BuildingRecipe, Element, Geometry, Instance, ParamValue, Part, Pose, Shape,
    Socket,
};
use glam::{DQuat, DVec3};

fn part() -> Part {
    Part::builder("bay")
        .element(Element::new(
            "wall",
            Geometry::cuboid([4.0, 4.0, 0.4]),
            "surface",
        ))
        .socket(Socket::new("edge", Pose::at([4.0, 0.0, 0.0])))
        .build()
        .expect("valid reusable bay")
}

fn recipe() -> BuildingRecipe {
    Building::builder("assembly")
        .part(part())
        .material("surface", "metal")
        .instance(Instance::new("a", "bay"))
        .instance(Instance::new("b", "bay").material("surface", "timber"))
        .build()
        .expect("valid assembly")
        .into_recipe()
}

#[test]
fn palette_swaps_and_instance_overrides_preserve_geometry() {
    let original = recipe().build().expect("valid");
    let mut edited = original.clone().into_recipe();
    edited.materials.insert("surface".into(), "ceramic".into());
    let edited = edited.build().expect("palette swap");
    assert_eq!(original.recipe().parts, edited.recipe().parts);
    assert_eq!(edited.material("a", "surface"), Some("ceramic"));
    assert_eq!(edited.material("b", "surface"), Some("timber"));
    assert_eq!(edited.material("b", "unknown"), None);
    assert_eq!(edited.material("unknown", "surface"), None);
}

#[test]
fn interchange_roundtrip_revalidates_and_rejects_unknown_fields() {
    let building = recipe().build().expect("valid");
    let encoded = ron::to_string(&building).expect("serialize");
    let decoded: BuildingRecipe = ron::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded.build().expect("validate loaded recipe"), building);
    let corrupted = encoded.replacen("id:", "typo:", 1);
    assert!(ron::from_str::<BuildingRecipe>(&corrupted).is_err());
    let mut invalid = recipe();
    invalid.instances[0].part = "missing".into();
    let encoded = ron::to_string(&invalid).expect("raw invalid data is serializable");
    let decoded: BuildingRecipe = ron::from_str(&encoded).expect("raw data");
    assert_eq!(
        decoded.build().expect_err("bad reference").path,
        "instances[a].part"
    );
}

#[test]
fn duplicate_ids_and_unbound_or_misspelled_slots_are_errors() {
    let mut data = recipe();
    data.parts.push(part());
    assert!(
        data.build()
            .expect_err("duplicate part")
            .reason
            .contains("duplicate")
    );
    let mut data = recipe();
    data.instances[1].id = "a".into();
    assert!(
        data.build()
            .expect_err("duplicate placement")
            .reason
            .contains("duplicate")
    );
    let mut data = recipe();
    data.materials.clear();
    assert_eq!(
        data.build().expect_err("missing binding").path,
        "instances[a].materials[surface]"
    );
    let mut data = recipe();
    data.instances[0]
        .materials
        .insert("surafce".into(), "metal".into());
    assert!(
        data.build()
            .expect_err("misspelled slot")
            .reason
            .contains("absent")
    );
}

#[test]
fn raw_edits_cannot_bypass_geometry_and_socket_validation() {
    let mut data = recipe();
    data.parts[0].elements[0].geometry = Geometry::cuboid([1.0, f64::NAN, 1.0]);
    assert!(
        data.build()
            .expect_err("nonfinite dimension")
            .path
            .contains("parts[bay].elements[wall]")
    );
    let mut data = recipe();
    let duplicate = data.parts[0].sockets[0].clone();
    data.parts[0].sockets.push(duplicate);
    assert!(
        data.build()
            .expect_err("duplicate socket")
            .reason
            .contains("duplicate")
    );
    let mut data = recipe();
    data.instances[0].pose.rotation = DQuat::from_xyzw(0.0, 0.0, 0.0, 2.0);
    assert!(
        data.build()
            .expect_err("nonunit rotation")
            .reason
            .contains("unit quaternion")
    );
    for grid in [0.0, -1.0, f64::INFINITY] {
        let mut data = recipe();
        data.grid = Some(grid);
        assert_eq!(data.build().expect_err("bad grid").path, "grid");
    }
}

#[test]
fn arbitrary_rotations_and_off_grid_placements_survive_build() {
    let pose = Pose::at([0.13, 2.2, -3.7]).rotated(DQuat::from_rotation_y(0.37));
    let mut data = recipe();
    data.grid = Some(0.5);
    data.instances[0].pose = pose;
    let building = data.build().expect("grid is only a hint");
    assert_eq!(building.recipe().instances[0].pose, pose);
    let point = Pose::at([10.0, 0.0, 0.0])
        .rotated(DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2))
        .transform_point(DVec3::X);
    assert!(point.distance(DVec3::new(10.0, 0.0, -1.0)) < 1e-12);
}

fn accepts(geometry: Geometry) -> bool {
    Part::builder("shape")
        .element(Element::new("solid", geometry, "surface"))
        .build()
        .is_ok()
}

#[test]
fn profiles_accept_concavity_and_both_windings_but_reject_bad_boundaries() {
    let concave = vec![
        [0.0, 0.0],
        [2.0, 0.0],
        [2.0, 1.0],
        [1.0, 1.0],
        [1.0, 2.0],
        [0.0, 2.0],
    ];
    assert!(accepts(Geometry::extrude(concave.clone(), 3.0)));
    assert!(accepts(Geometry::extrude(concave.into_iter().rev(), 3.0)));
    for invalid in [
        vec![[0.0, 0.0], [1.0, 1.0]],
        vec![[0.0, 0.0], [1.0, 1.0], [0.0, 1.0], [1.0, 0.0]],
        vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0]],
        vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]],
        vec![[0.0, 0.0], [1.0, 0.0], [f64::INFINITY, 1.0]],
    ] {
        assert!(!accepts(Geometry::extrude(invalid, 1.0)));
    }
    assert!(!accepts(Geometry::cylinder(1.0, 2.0, 2)));
    assert!(!accepts(Geometry::cylinder(-1.0, 2.0, 16)));
    assert!(accepts(Geometry::cylinder(1.0, 2.0, 16)));
}

#[test]
fn subtraction_preserves_operand_frames_and_checks_nested_cutters() {
    let solid = Geometry::cuboid([4.0, 4.0, 0.4]).placed(Pose::at([1.0, 0.0, 0.0]));
    let cutter = Geometry::cuboid([2.0, 3.0, 1.0]).placed(Pose::at([2.0, 0.0, -0.3]));
    let result = solid.clone().subtract(cutter.clone());
    assert!(accepts(result.clone()));
    let Shape::Difference {
        solid: actual,
        cutters,
    } = result.shape
    else {
        panic!("difference")
    };
    assert_eq!(*actual, solid);
    assert_eq!(cutters, vec![cutter]);
    assert!(!accepts(
        solid.clone().subtract(Geometry::cuboid([0.0, 1.0, 1.0]))
    ));
    let mut deep = solid;
    for _ in 0..66 {
        deep = deep.subtract(Geometry::cuboid([1.0; 3]));
    }
    assert!(!accepts(deep));
}

#[test]
fn an_unknown_schema_version_is_refused_and_an_absent_one_reads_as_the_first() {
    let mut data = recipe();
    data.version = ashlar::SCHEMA_VERSION + 1;
    assert_eq!(data.build().expect_err("future schema").path, "version");
    let encoded = ron::to_string(&recipe().build().expect("valid")).expect("serialize");
    assert!(encoded.contains("version:1"));
    let legacy = encoded.replacen("version:1,", "", 1);
    let decoded: BuildingRecipe = ron::from_str(&legacy).expect("pre-version document");
    assert_eq!(decoded.version, ashlar::SCHEMA_VERSION);
    assert!(decoded.build().is_ok());
}

#[test]
fn parts_and_instances_are_addressable_by_id() {
    let building = recipe().build().expect("valid");
    assert_eq!(building.part("bay").expect("part").id, "bay");
    assert_eq!(building.instance("b").expect("instance").part, "bay");
    assert!(building.part("missing").is_none());
    assert!(building.instance("missing").is_none());
    // The re-exported glam is the one the recipe's own vectors come from.
    let point: ashlar::glam::DVec3 = building.instance("a").expect("a").pose.translation;
    assert_eq!(point, DVec3::ZERO);
}

fn edged() -> Part {
    Part::builder("bay")
        .element(Element::new(
            "wall",
            Geometry::cuboid([4.0, 4.0, 0.4]),
            "surface",
        ))
        .socket(Socket::new(
            "left",
            Pose::default().rotated(DQuat::from_rotation_y(-std::f64::consts::FRAC_PI_2)),
        ))
        .socket(Socket::new(
            "right",
            Pose::at([4.0, 0.0, 0.0]).rotated(DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2)),
        ))
        .build()
        .expect("bay with edges")
}

fn chain() -> BuildingRecipe {
    Building::builder("row")
        .part(edged())
        .material("surface", "metal")
        .instance(
            Instance::new("first", "bay")
                .placed(Pose::at([10.0, 0.0, 0.0]).rotated(DQuat::from_rotation_y(0.4))),
        )
        .instance(Instance::new("second", "bay").attach("left", "first", "right"))
        .instance(Instance::new("third", "bay").attach("left", "second", "right"))
        .build()
        .expect("attached row")
        .into_recipe()
}

#[test]
fn opposed_sockets_bring_two_parts_edge_to_edge_in_the_targets_frame() {
    let row = chain().build().expect("valid");
    let first = row.instance("first").expect("first").pose;
    for (id, bays) in [("second", 1.0), ("third", 2.0)] {
        let pose = row.instance(id).expect("attached").pose;
        let expected = first.transform_point(DVec3::new(4.0 * bays, 0.0, 0.0));
        assert!(pose.translation.distance(expected) < 1e-12, "{id}");
        // Sockets face outward, so meeting one turns the part half a turn: two
        // edge sockets that oppose each other leave the parts side by side.
        assert!(pose.rotation.angle_between(first.rotation) < 1e-12);
    }
    // Order in the file does not decide resolution order.
    let mut reversed = chain();
    reversed.instances.reverse();
    let built = reversed.build().expect("forward references resolve");
    assert_eq!(
        built.instance("third").expect("third").pose,
        row.instance("third").expect("third").pose
    );
}

#[test]
fn aligned_sockets_land_one_frame_on_the_other() {
    let stacked = Building::builder("stack")
        .part(
            Part::builder("bay")
                .element(Element::new(
                    "wall",
                    Geometry::cuboid([4.0, 4.0, 0.4]),
                    "surface",
                ))
                .socket(Socket::new("base", Pose::default()))
                .socket(Socket::new("top", Pose::at([0.0, 4.0, 0.0])))
                .build()
                .expect("bay"),
        )
        .material("surface", "metal")
        .instance(Instance::new("ground", "bay"))
        .instance(Instance::new("upper", "bay").attach_aligned("base", "ground", "top"))
        .build()
        .expect("stack");
    let pose = stacked.instance("upper").expect("upper").pose;
    assert_eq!(pose.translation, DVec3::new(0.0, 4.0, 0.0));
    assert!(pose.rotation.angle_between(DQuat::IDENTITY) < 1e-12);
}

#[test]
fn unknown_sockets_targets_and_cycles_carry_their_path() {
    let bad = |instance: Instance| {
        let mut data = chain();
        data.instances[1] = instance;
        data.build().expect_err("invalid attachment")
    };
    let error = bad(Instance::new("second", "bay").attach("edge", "first", "right"));
    assert_eq!(error.path, "instances[second].attach.socket");
    let error = bad(Instance::new("second", "bay").attach("left", "nowhere", "right"));
    assert_eq!(error.path, "instances[second].attach.target");
    let error = bad(Instance::new("second", "bay").attach("left", "first", "edge"));
    assert_eq!(error.path, "instances[second].attach.target_socket");
    let error = bad(Instance::new("second", "bay").attach("left", "second", "right"));
    assert!(error.reason.contains("itself"));
    let mut data = chain();
    data.instances[0] = Instance::new("first", "bay").attach("left", "third", "right");
    assert!(data.build().expect_err("cycle").reason.contains("cycle"));
    // A pose set afterwards is the pose, and an attachment set afterwards wins.
    let instance = Instance::new("x", "bay")
        .attach("left", "first", "right")
        .placed(Pose::at([1.0, 0.0, 0.0]));
    assert!(instance.attach.is_none());
}

#[test]
fn a_binding_reads_and_writes_as_a_key_alone_or_as_a_key_with_overrides() {
    // The compatibility claim, both ways round. A recipe that overrides
    // nothing is the document it always was, character for character, and one
    // that overrides something reads back as the same overrides.
    let plain = recipe().build().expect("valid");
    let encoded = ron::to_string(&plain).expect("serialize");
    assert!(
        encoded.contains(r#""surface":"metal""#),
        "a binding that overrides nothing is the key alone: {encoded}"
    );
    let decoded: BuildingRecipe = ron::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded.build().expect("revalidate"), plain);

    let mut data = recipe();
    data.instances[1].materials.insert(
        "surface".into(),
        Binding::new("metal")
            .param("variation", ParamValue::Float(0.37))
            .param("tint", ParamValue::Color([0.5, 0.25, 0.125])),
    );
    let dressed = data.build().expect("an override validates");
    let encoded = ron::to_string(&dressed).expect("serialize");
    assert!(
        encoded.contains("material:\"metal\"") && encoded.contains("variation"),
        "an override writes itself out: {encoded}"
    );
    let decoded: BuildingRecipe = ron::from_str(&encoded).expect("deserialize");
    let decoded = decoded.build().expect("revalidate");
    assert_eq!(decoded, dressed);
    let binding = decoded.binding("b", "surface").expect("bound");
    assert_eq!(binding.material, "metal");
    assert_eq!(binding.params["variation"], ParamValue::Float(0.37));
    // The key alone is still what a caller asking for a key gets.
    assert_eq!(decoded.material("b", "surface"), Some("metal"));
    assert!(decoded.binding("a", "surface").expect("default").is_plain());

    // Hand-written RON of both forms in one map, which is what a library being
    // edited toward overrides looks like halfway through.
    let mixed: BuildingRecipe = ron::from_str(
        &ron::to_string(&dressed)
            .expect("serialize")
            .replace(r#""surface":"metal""#, r#""surface":(material:"metal")"#),
    )
    .expect("a struct with no params is a binding too");
    assert!(
        mixed
            .build()
            .expect("validate")
            .binding("a", "surface")
            .expect("bound")
            .is_plain()
    );
}

#[test]
fn an_override_is_checked_where_it_is_written_and_names_its_own_path() {
    let mut data = recipe();
    data.instances[0].materials.insert(
        "surface".into(),
        Binding::new("metal").param("variation", ParamValue::Float(f32::NAN)),
    );
    let error = data.build().expect_err("a non-finite override");
    assert_eq!(
        error.path,
        "instances[a].materials[surface].params[variation]"
    );
    assert!(error.reason.contains("finite"), "{}", error.reason);

    let mut data = recipe();
    data.instances[0].materials.insert(
        "surface".into(),
        Binding::new("metal").param("", ParamValue::Int(1)),
    );
    assert_eq!(
        data.build().expect_err("a blank parameter name").path,
        "instances[a].materials[surface].params[]"
    );

    // A palette default may carry overrides too, and is checked the same way.
    let mut data = recipe();
    data.materials.insert(
        "surface".into(),
        Binding::new("metal").param("variation", ParamValue::Float(f32::INFINITY)),
    );
    assert_eq!(
        data.build().expect_err("a non-finite default").path,
        "materials[surface].params[variation]"
    );
}

#[test]
fn two_bindings_are_one_binding_when_they_name_one_material_at_one_value() {
    let one = Binding::new("metro:stone").param("variation", ParamValue::Float(0.37));
    let same = Binding::new("metro:stone").param("variation", ParamValue::Float(0.37));
    let other = Binding::new("metro:stone").param("variation", ParamValue::Float(0.71));
    assert_eq!(one, same);
    assert_ne!(one, other);
    assert_ne!(one, Binding::new("metro:stone"));
    // A set of bindings is what a renderer keys its materials by, so the three
    // walls of a block have to come to three entries and not to one or six.
    let held: std::collections::BTreeSet<_> =
        [one.clone(), same, other, Binding::new("metro:stone")]
            .into_iter()
            .collect();
    assert_eq!(held.len(), 3);
    // Bit equality, so the two zeroes are two bindings: a duplicate texture
    // set, never somebody else's texels.
    assert_ne!(
        Binding::new("m").param("v", ParamValue::Float(0.0)),
        Binding::new("m").param("v", ParamValue::Float(-0.0))
    );
}

#[test]
fn a_building_enumerates_every_slot_its_parts_declare() {
    let part = Part::builder("bay")
        .element(
            Element::new("wall", Geometry::cuboid([4.0, 4.0, 0.4]), "surface").cut_material("cut"),
        )
        .build()
        .expect("part");
    let building = Building::builder("assembly")
        .part(part)
        .material("surface", "metal")
        .material("cut", "bare")
        .instance(Instance::new("a", "bay"))
        .instance(Instance::new("b", "bay").binding(
            "surface",
            Binding::new("metal").param("variation", ParamValue::Float(0.5)),
        ))
        .build()
        .expect("building");
    let seen: Vec<_> = building
        .bindings()
        .map(|(instance, slot, binding)| (instance, slot, binding.material.as_str()))
        .collect();
    assert_eq!(
        seen,
        [
            ("a", "surface", "metal"),
            ("a", "cut", "bare"),
            ("b", "surface", "metal"),
            ("b", "cut", "bare"),
        ]
    );
    // The overriding instance is the only one carrying a value, and the cut
    // slot beside it is untouched: an override is per slot, not per instance.
    let dressed: Vec<_> = building
        .bindings()
        .filter(|(_, _, binding)| !binding.is_plain())
        .map(|(instance, slot, _)| (instance, slot))
        .collect();
    assert_eq!(dressed, [("b", "surface")]);
}
