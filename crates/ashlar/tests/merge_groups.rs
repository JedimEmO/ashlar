//! Merge group declaration, validation and the queries a backend walks.

use ashlar::{Building, BuildingRecipe, Element, Geometry, Instance, MergeGroup, Part, Side};

fn bay() -> Part {
    Part::builder("bay")
        .element(Element::new(
            "shell",
            Geometry::cuboid([4.0, 3.0, 0.3]),
            "surface",
        ))
        .element(Element::new("glass", Geometry::cuboid([1.0, 1.0, 0.1]), "surface").standalone())
        .build()
        .expect("valid bay")
}

fn shell_bay() -> Part {
    Part::builder("bay")
        .element(Element::new(
            "shell",
            Geometry::cuboid([4.0, 3.0, 0.3]),
            "surface",
        ))
        .build()
        .expect("valid bay")
}

#[test]
fn an_unmerged_building_has_no_merge_groups() {
    let building = Building::builder("tower")
        .part(shell_bay())
        .material("surface", "paint")
        .instance(Instance::new("a", "bay"))
        .build()
        .expect("valid");
    assert!(building.merge_groups().is_empty());
    assert_eq!(building.group_of("a"), Some("tower"));
}

#[test]
fn the_default_group_is_the_building() {
    let building = Building::builder("tower")
        .part(bay())
        .material("surface", "paint")
        .merged()
        .instance(Instance::new("a", "bay"))
        .instance(Instance::new("b", "bay"))
        .build()
        .expect("valid");
    let groups = building.merge_groups();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].id, "tower");
    assert_eq!(groups[0].storey, None);
    let members: Vec<(&str, &str)> = groups[0]
        .members
        .iter()
        .map(|(instance, element)| (instance.id.as_str(), element.id.as_str()))
        .collect();
    assert_eq!(members, [("a", "shell"), ("b", "shell")]);
}

#[test]
fn declared_groups_come_after_the_default_in_declaration_order() {
    let building = Building::builder("tower")
        .part(bay())
        .material("surface", "paint")
        .merged()
        .group(MergeGroup::new("upper").storey(2))
        .group(MergeGroup::new("lower").storey(1))
        .instance(Instance::new("a", "bay").group("lower"))
        .instance(Instance::new("b", "bay").group("upper"))
        .instance(Instance::new("c", "bay"))
        .build()
        .expect("valid");
    let groups = building.merge_groups();
    let ids: Vec<&str> = groups.iter().map(|group| group.id).collect();
    assert_eq!(ids, ["tower", "upper", "lower"]);
    let storeys: Vec<Option<i32>> = groups.iter().map(|group| group.storey).collect();
    assert_eq!(storeys, [None, Some(2), Some(1)]);
}

#[test]
fn a_group_with_only_standalone_members_is_not_returned() {
    let pane = Part::builder("pane")
        .element(Element::new("glass", Geometry::cuboid([1.0, 1.0, 0.1]), "surface").standalone())
        .build()
        .expect("valid pane");
    let building = Building::builder("tower")
        .part(pane)
        .material("surface", "glazing")
        .merged()
        .group(MergeGroup::new("glazing"))
        .instance(Instance::new("a", "pane").group("glazing"))
        .build()
        .expect("valid");
    assert!(building.merge_groups().is_empty());
}

#[test]
fn a_group_under_the_buildings_own_id_is_the_default_group() {
    let building = Building::builder("tower")
        .part(bay())
        .material("surface", "paint")
        .merged()
        .group(MergeGroup::new("tower").storey(3))
        .instance(Instance::new("a", "bay"))
        .instance(Instance::new("b", "bay").group("tower"))
        .build()
        .expect("valid");
    let groups = building.merge_groups();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].id, "tower");
    assert_eq!(groups[0].storey, Some(3));
    assert_eq!(groups[0].members.len(), 2);
    assert_eq!(building.storey_of("a"), Some(3));
}

#[test]
fn an_unknown_group_is_refused_by_path() {
    let error = Building::builder("tower")
        .part(bay())
        .material("surface", "paint")
        .merged()
        .instance(Instance::new("a", "bay").group("nowhere"))
        .build()
        .expect_err("unknown group");
    assert_eq!(error.path, "instances[a].group");
    assert_eq!(error.reason, "unknown group \"nowhere\"");
}

#[test]
fn a_duplicate_group_is_refused_by_path() {
    let error = Building::builder("tower")
        .part(bay())
        .material("surface", "paint")
        .merged()
        .group(MergeGroup::new("upper"))
        .group(MergeGroup::new("upper"))
        .instance(Instance::new("a", "bay").group("upper"))
        .build()
        .expect_err("duplicate group");
    assert_eq!(error.path, "groups[upper]");
}

#[test]
fn storey_of_follows_the_group() {
    let building = Building::builder("tower")
        .part(bay())
        .material("surface", "paint")
        .group(MergeGroup::new("upper").storey(2))
        .instance(Instance::new("a", "bay").group("upper"))
        .instance(Instance::new("b", "bay"))
        .build()
        .expect("valid");
    assert_eq!(building.storey_of("a"), Some(2));
    assert_eq!(building.storey_of("b"), None);
    assert_eq!(building.storey_of("missing"), None);
}

#[test]
fn an_old_recipe_serialises_to_the_same_text() {
    let building = Building::builder("tower")
        .part(shell_bay())
        .material("surface", "paint")
        .instance(Instance::new("a", "bay"))
        .build()
        .expect("valid");
    let text = ron::to_string(building.recipe()).expect("serialize");
    for word in ["merged", "groups", "group", "standalone"] {
        assert!(!text.contains(word), "old recipe wrote {word:?}: {text}");
    }

    let merged = Building::builder("tower")
        .part(bay())
        .material("surface", "paint")
        .merged()
        .group(MergeGroup::new("upper").storey(2))
        .instance(Instance::new("a", "bay").group("upper"))
        .instance(Instance::new("b", "bay"))
        .build()
        .expect("valid");
    let text = ron::to_string(merged.recipe()).expect("serialize");
    let decoded: BuildingRecipe = ron::from_str(&text).expect("deserialize");
    assert_eq!(decoded, *merged.recipe());
}

#[test]
fn side_defaults_to_exterior_and_is_not_serialised() {
    let exterior = Element::new("shell", Geometry::cuboid([4.0, 3.0, 0.3]), "surface");
    assert_eq!(exterior.side, Side::Exterior);
    let text = ron::to_string(&exterior).expect("serialize");
    assert!(
        !text.contains("side"),
        "an exterior element wrote it: {text}"
    );

    let interior = exterior.clone().interior();
    assert_eq!(interior.side, Side::Interior);
    let text = ron::to_string(&interior).expect("serialize");
    assert!(
        text.contains("side"),
        "an interior element omitted it: {text}"
    );
    let decoded: Element = ron::from_str(&text).expect("deserialize");
    assert_eq!(decoded, interior, "interior round-trips through RON");
}
