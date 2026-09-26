//! Simplifying a recipe for a coarser level of detail.
#![allow(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test fixtures; the compared corners are exact translations"
)]
use ashlar::{
    Building, Element, Geometry, Instance, LodPolicy, MIN_SEGMENTS, Part, Pose, Shape, Socket,
};

fn far(min_feature: f64) -> LodPolicy {
    LodPolicy {
        min_feature,
        segment_scale: 0.25,
        drop_interior: true,
        ..LodPolicy::default()
    }
}

#[test]
fn segments_scale_down_to_a_floor_and_never_up() {
    let column = Geometry::cylinder(0.5, 3.0, 48);
    let Shape::Cylinder { segments, .. } = column.simplified(&far(0.0)).unwrap().shape else {
        panic!("a cylinder stays a cylinder");
    };
    assert_eq!(segments, 12);
    let Shape::Cylinder { segments, .. } = Geometry::cylinder(0.5, 3.0, 12)
        .simplified(&far(0.0))
        .unwrap()
        .shape
    else {
        panic!();
    };
    assert_eq!(segments, MIN_SEGMENTS);
    let Shape::Cylinder { segments, .. } = Geometry::cylinder(0.5, 3.0, 6)
        .simplified(&far(0.0))
        .unwrap()
        .shape
    else {
        panic!();
    };
    assert_eq!(segments, 6, "an authored count below the floor is kept");
    let Shape::Revolve { segments, .. } =
        Geometry::revolve([[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]], 64)
            .simplified(&far(0.0))
            .unwrap()
            .shape
    else {
        panic!();
    };
    assert_eq!(segments, 16);
}

#[test]
fn small_cutters_go_and_large_ones_stay_even_when_they_are_portals() {
    let wall = Geometry::cuboid([4.0, 3.0, 0.3])
        .subtract(
            Geometry::cuboid([1.0, 2.1, 0.5])
                .placed(Pose::at([1.5, 0.0, -0.1]))
                .portal("door")
                .cut_material("frame"),
        )
        .subtract(Geometry::cuboid([0.2, 0.2, 0.5]).placed(Pose::at([0.3, 2.5, -0.1])));
    let simple = wall.simplified(&far(0.5)).unwrap();
    let cutters = simple.cutters();
    assert_eq!(cutters.len(), 1, "the vent is gone and the door is not");
    assert_eq!(cutters[0].portal.as_deref(), Some("door"));
    // With every cutter gone the solid is what is left, placed where it was.
    let bare = Geometry::cuboid([4.0, 3.0, 0.3])
        .subtract(Geometry::cuboid([0.2, 0.2, 0.5]))
        .placed(Pose::at([1.0, 2.0, 3.0]));
    let simple = bare.simplified(&far(0.5)).unwrap();
    assert!(matches!(simple.shape, Shape::Cuboid { .. }));
    assert_eq!(simple.bounds().unwrap()[0].to_array(), [1.0, 2.0, 3.0]);
}

#[test]
fn a_union_keeps_its_large_members_and_collapses_to_one() {
    let union = Geometry::union_all([
        Geometry::cuboid([2.0, 2.0, 2.0]),
        Geometry::cuboid([0.1, 0.1, 0.1]).placed(Pose::at([2.0, 0.0, 0.0])),
    ])
    .placed(Pose::at([0.0, 5.0, 0.0]));
    let simple = union.simplified(&far(0.5)).unwrap();
    assert!(matches!(simple.shape, Shape::Cuboid { .. }));
    assert_eq!(simple.bounds().unwrap()[0].to_array(), [0.0, 5.0, 0.0]);
    let tiny = Geometry::union_all([Geometry::cuboid([0.1; 3]), Geometry::cuboid([0.2; 3])]);
    assert!(tiny.simplified(&far(0.5)).is_none());
}

fn house() -> Part {
    Part::builder("test:house")
        .element(Element::new(
            "walls",
            Geometry::cuboid([6.0, 3.0, 6.0]),
            "wall",
        ))
        .element(Element::new("liner", Geometry::cuboid([5.0, 2.5, 5.0]), "liner").interior())
        .element(Element::new(
            "lamp",
            Geometry::cuboid([0.2, 0.3, 0.2]),
            "light",
        ))
        .socket(Socket::new("door", Pose::default()))
        .build()
        .unwrap()
}

#[test]
fn a_part_drops_its_interior_and_its_small_elements_and_keeps_its_sockets() {
    let simple = house().simplified(&far(0.5)).unwrap();
    let ids: Vec<&str> = simple.elements.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["walls"]);
    assert_eq!(simple.sockets.len(), 1);
    assert_eq!(house().simplified(&LodPolicy::default()).unwrap(), house());
    let lamp = Part::builder("test:lamp")
        .element(Element::new("lamp", Geometry::cuboid([0.2; 3]), "light"))
        .build()
        .unwrap();
    assert!(lamp.simplified(&far(0.5)).is_none());
}

#[test]
fn a_building_loses_vanished_parts_and_their_instances_and_stays_valid() {
    let lamp = Part::builder("test:lamp")
        .element(Element::new("lamp", Geometry::cuboid([0.2; 3]), "light"))
        .socket(Socket::new("base", Pose::default()))
        .build()
        .unwrap();
    let mut house = house();
    house
        .sockets
        .push(Socket::new("roof", Pose::at([3.0, 3.0, 3.0])));
    let building = Building::builder("test:street")
        .part(house)
        .part(lamp)
        .material("wall", "plaster")
        .material("liner", "plaster")
        .instance(
            Instance::new("house", "test:house")
                .material("light", "glow")
                .material("liner", "paint"),
        )
        .instance(Instance::new("lamp", "test:lamp").attach_aligned("base", "house", "roof"))
        .material("light", "glow")
        .build()
        .unwrap();
    let simple = building
        .simplified(&far(0.5))
        .unwrap()
        .expect("the house survives");
    let recipe = simple.recipe();
    assert_eq!(recipe.instances.len(), 1, "the lamp went with its part");
    assert_eq!(recipe.parts.len(), 1);
    assert!(
        recipe.instances[0].materials.is_empty(),
        "no override for a slot now unused"
    );
    // The survivor keeps the pose it was resolved to.
    assert_eq!(
        simple.instance("house").unwrap().pose,
        building.instance("house").unwrap().pose
    );
}

#[test]
fn the_default_ladder_widens_and_ends_open() {
    let ladder = LodPolicy::ladder();
    assert!(ladder[0].is_identity());
    let bands: Vec<f32> = ladder.iter().filter_map(|p| p.until).collect();
    assert!(bands.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(ladder.last().unwrap().until, None);
    let policy: LodPolicy = ron::from_str("(until: Some(90.0), min_feature: 0.3)").unwrap();
    assert!(
        (policy.segment_scale - 1.0).abs() < f64::EPSILON,
        "a missing scale is one"
    );
}

#[test]
fn a_cutter_that_simplifies_to_one_child_keeps_its_slot_and_portal() {
    use ashlar::{Geometry, LodPolicy, Pose, Shape};
    // A door cutter: the opening and a small latch notch, as one union.
    let door = Geometry::cuboid([1.0, 2.1, 0.6])
        .union(Geometry::cuboid([0.05, 0.05, 0.6]).placed(Pose::at([0.9, 1.0, 0.0])))
        .portal("door")
        .cut_material("frame");
    let wall = Geometry::cuboid([4.0, 3.0, 0.3]).subtract(door.placed(Pose::at([1.5, 0.0, -0.15])));
    let policy = LodPolicy {
        min_feature: 0.3,
        ..LodPolicy::default()
    };
    let simple = wall.simplified(&policy).expect("the wall survives");
    let Shape::Difference { cutters, .. } = &simple.shape else {
        panic!("the door is still cut");
    };
    assert_eq!(cutters.len(), 1);
    assert_eq!(cutters[0].portal.as_deref(), Some("door"));
    assert_eq!(cutters[0].cut_slot.as_deref(), Some("frame"));
}

#[test]
fn a_thin_element_goes_however_long_it_is_and_a_far_one_stays() {
    // A sill three metres long and twenty centimetres deep is twenty
    // centimetres of anything at a distance; a neon line as thin is kept
    // because it glows.
    let part = Part::builder("test:bay")
        .element(Element::new(
            "wall",
            Geometry::cuboid([4.0, 3.8, 0.45]),
            "wall",
        ))
        .element(Element::new(
            "sill",
            Geometry::cuboid([3.2, 0.08, 0.2]).placed(Pose::at([0.4, 0.6, -0.2])),
            "metal",
        ))
        .element(
            Element::new(
                "neon",
                Geometry::union_all([
                    Geometry::cuboid([0.05, 3.8, 0.05]).placed(Pose::at([0.0, 0.0, -0.06])),
                    Geometry::cuboid([4.0, 0.05, 0.05]).placed(Pose::at([0.0, 3.7, -0.06])),
                ]),
                "neon",
            )
            .far(),
        )
        .build()
        .unwrap();
    let simple = part.simplified(&far(0.35)).unwrap();
    let ids: Vec<&str> = simple.elements.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["wall", "neon"]);
    let Shape::Union { solids } = &simple.elements[1].geometry.shape else {
        panic!("a far element keeps every member");
    };
    assert_eq!(solids.len(), 2);
}

#[test]
fn a_thin_union_member_goes_and_a_broad_plate_stays() {
    let geometry = Geometry::union_all([
        // A plate a finger thick but four metres square: broad, kept.
        Geometry::cuboid([4.0, 0.02, 4.0]),
        // A rail four metres long and five centimetres thick: thin, gone.
        Geometry::cuboid([4.0, 0.05, 0.05]).placed(Pose::at([0.0, 1.0, 0.0])),
    ]);
    let simple = geometry.simplified(&far(0.35)).unwrap();
    assert!(
        !matches!(simple.shape, Shape::Union { .. }),
        "one member left, standing in for the union"
    );
    let [low, high] = simple.bounds().unwrap();
    assert!((high - low).y < 0.03, "the plate is what is left");
}
