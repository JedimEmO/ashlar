//! Cutter numbering, cut slots and the fallback that resolves a face's slot.
use ashlar::*;

fn wall() -> Geometry {
    Geometry::cuboid([4.0, 3.0, 0.3])
}

#[test]
fn cutters_are_numbered_depth_first() {
    let a = Geometry::cuboid([1.0, 2.0, 3.0]);
    let b = Geometry::cuboid([0.5, 1.0, 1.5]);
    let c = Geometry::cuboid([2.0, 0.5, 0.25]);
    // The inner difference is the `solid` of the outer one, so `a` is found
    // before `b`, and the union's second solid contributes `c` last.
    let whole = wall()
        .subtract(a.clone())
        .subtract(b.clone())
        .union(Geometry::cuboid([0.4, 3.0, 0.4]).subtract(c.clone()));

    let roots = whole.cutters();
    assert_eq!(roots.len(), 3);
    assert_eq!(*roots[0], a);
    assert_eq!(*roots[1], b);
    assert_eq!(*roots[2], c);
}

#[test]
fn a_cutter_inside_a_cutter_is_not_a_root() {
    let plug = Geometry::cuboid([1.0, 1.0, 1.0]);
    let hole = Geometry::cuboid([0.4, 0.4, 0.4]);
    let whole = wall().subtract(plug.subtract(hole));
    assert_eq!(whole.cutters().len(), 1);
}

#[test]
fn an_arrays_cutters_are_counted_once() {
    let railing = wall()
        .subtract(Geometry::cuboid([1.0, 1.0, 0.5]))
        .arrayed(5, Pose::at([0.5, 0.0, 0.0]));
    assert_eq!(railing.cutters().len(), 1);
}

#[test]
fn a_cut_slot_is_only_valid_on_a_cutter() {
    let lone = Geometry::cuboid([1.0, 1.0, 1.0]).cut_material("x");
    let error = lone.check().expect_err("a solid cannot name a cut slot");
    assert!(error.path.ends_with(".cut_slot"), "{}", error.path);

    let used = wall().subtract(Geometry::cuboid([1.0, 1.0, 0.5]).cut_material("x"));
    assert!(used.check().is_ok());

    let blank = wall().subtract(Geometry::cuboid([1.0, 1.0, 0.5]).cut_material("  "));
    let error = blank
        .check()
        .expect_err("a blank cutter slot is not a name");
    assert!(error.path.ends_with(".cut_slot"), "{}", error.path);

    let nested = wall().subtract(
        Geometry::cuboid([2.0, 2.0, 2.0])
            .subtract(Geometry::cuboid([1.0, 1.0, 1.0]).cut_material("x")),
    );
    let error = nested
        .check()
        .expect_err("a slot inside a cutter root is refused");
    assert!(error.path.ends_with(".cut_slot"), "{}", error.path);
}

#[test]
fn slot_for_prefers_the_cutter_then_the_element() {
    let geometry = wall()
        .subtract(Geometry::cuboid([1.0, 1.0, 0.5]))
        .subtract(Geometry::cuboid([0.5, 0.5, 0.5]).cut_material("sill"));
    let element = Element::new("shell", geometry.clone(), "wall").cut_material("reveal");

    assert_eq!(element.slot_for(FaceOrigin::Body), "wall");
    assert_eq!(element.slot_for(FaceOrigin::Cutter(0)), "reveal");
    assert_eq!(element.slot_for(FaceOrigin::Cutter(1)), "sill");
    assert_eq!(element.slot_for(FaceOrigin::Cutter(9)), "reveal");

    let plain = Element::new("shell", geometry, "wall");
    assert_eq!(plain.slot_for(FaceOrigin::Cutter(0)), "wall");
}

#[test]
fn a_cutter_slot_needs_a_binding() {
    let wall = wall().subtract(Geometry::cuboid([1.0, 1.0, 0.5]).cut_material("sill"));
    let part = Part::builder("bay")
        .element(Element::new("shell", wall, "wall"))
        .build()
        .expect("part with a cutter slot");

    let unbound = Building::builder("wall")
        .part(part.clone())
        .material("wall", "paint")
        .instance(Instance::new("shell", "bay"))
        .build()
        .expect_err("the cutter slot is not bound");
    assert!(unbound.path.contains("materials[sill]"), "{}", unbound.path);

    let building = Building::builder("wall")
        .part(part)
        .material("wall", "paint")
        .material("sill", "stone")
        .instance(Instance::new("shell", "bay"))
        .build()
        .expect("bound cutter slot");
    assert!(
        building
            .bindings()
            .any(|(instance, slot, _)| instance == "shell" && slot == "sill")
    );
}

#[test]
fn an_old_recipe_round_trips_unchanged() {
    let plain = wall().subtract(Geometry::cuboid([1.0, 1.0, 0.5]));
    let encoded = ron::to_string(&plain).expect("serialize");
    assert!(!encoded.contains("cut_slot"), "{encoded}");
    let decoded: Geometry = ron::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded, plain);

    let dressed = wall().subtract(Geometry::cuboid([1.0, 1.0, 0.5]).cut_material("sill"));
    let encoded = ron::to_string(&dressed).expect("serialize");
    assert!(encoded.contains("cut_slot"), "{encoded}");
    let decoded: Geometry = ron::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded, dressed);
}
