//! Every building resolves its slots and overrides through the unified library.
use ashlar::{BoundSlots, ParamValue};
use ashlar_showcase::{Scene, building, library, materials};

#[test]
fn every_building_slot_and_parameter_resolves_in_the_shared_library() {
    let definitions = library::materials();
    let graphs = materials::graphs();
    for scene in [
        Scene::Facade,
        Scene::Entrance,
        Scene::Outpost,
        Scene::Office,
        Scene::Corporate,
        Scene::CorporateMerged,
        Scene::CorporateAnnex,
        Scene::CorporateBlock,
        Scene::CorporateBlockMerged,
        Scene::Interior,
        Scene::ScifiKit,
        Scene::ScifiOutpost,
        Scene::ScifiColony,
        Scene::CityKit,
        Scene::CityBlock,
        Scene::Metropolis,
        Scene::CityTower,
        Scene::CityLandmark,
    ] {
        let building = building(scene).expect("building");
        definitions.check_for(&building).expect("all slots resolve");
        for (path, binding) in building.bound_slots() {
            let definition = &definitions.materials[&binding.material];
            if let Some(key) = definition.surface.graph() {
                let mut params = definition.surface.params().cloned().unwrap_or_default();
                params.extend(binding.params.clone());
                graphs
                    .get(key)
                    .expect("graph")
                    .with_params(&params)
                    .unwrap_or_else(|error| panic!("{path}: {error}"))
                    .build_in(&graphs)
                    .unwrap_or_else(|error| panic!("{path}: {error}"));
            }
        }
    }
}

#[test]
fn house_finishes_are_timber_roof_clay_and_smooth_interior_render() {
    let house = building(Scene::Interior).expect("house");
    let recipe = house.recipe();
    assert_eq!(recipe.materials["floor"].material, "library:wood-floor");
    assert_eq!(recipe.materials["roof"].material, "library:clay-roof-tiles");
    assert_eq!(recipe.materials["stone"].material, "library:plaster");
    let definitions = library::materials();
    let smooth = &definitions.materials[&recipe.materials["spine"].material];
    assert_eq!(smooth.surface.graph(), Some("library:plaster"));
    assert_eq!(
        smooth.surface.params().expect("params")["smoothness"],
        ParamValue::Float(1.0)
    );
    let floor = recipe
        .parts
        .iter()
        .find(|p| p.id == "house:floor")
        .expect("floor");
    assert!(floor.elements.iter().any(|e| e.material_slot == "floor"));
}
