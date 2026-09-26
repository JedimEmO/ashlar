//! The showcase's material libraries are Rust, and the preview hands them to
//! the renderer as they are. This is the check a run of the preview makes,
//! without a window: every scene's library and every sheet's validates against
//! the building that binds its keys, and every graph it names lowers.
use ashlar::{MaterialLibrary, Surface};
use ashlar_bevy::runtime_bake::{BakeCache, BakeContext, Baker, create_graph_material};
use ashlar_material::MaterialGraphLibrary;
use ashlar_showcase as showcase;
use bevy::{MinimalPlugins, asset::AssetPlugin, prelude::*};

fn asset_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

/// A scene's library and graphs, preflighted against its building.
fn preflighted(
    scene: showcase::Scene,
    building: &ashlar::Building,
) -> (MaterialLibrary, MaterialGraphLibrary) {
    let library = scene.materials().expect("a scene with materials");
    let graphs = scene.graphs().expect("a scene with materials");
    ashlar_bevy::check_library_with_graphs(
        &format!("{scene:?}"),
        &library,
        &asset_root(),
        building,
        &graphs,
    )
    .unwrap_or_else(|error| panic!("{scene:?}: {error:#}"));
    (library, graphs)
}

#[test]
fn every_scene_library_preflights_against_its_building() {
    let mut checked = 0;
    for scene in [
        showcase::Scene::Basic,
        showcase::Scene::Facade,
        showcase::Scene::Entrance,
        showcase::Scene::Outpost,
        showcase::Scene::Office,
        showcase::Scene::Corporate,
        showcase::Scene::CorporateAnnex,
        showcase::Scene::CorporateBlock,
        showcase::Scene::Interior,
        showcase::Scene::ScifiKit,
        showcase::Scene::ScifiOutpost,
        showcase::Scene::ScifiColony,
        showcase::Scene::CityKit,
        showcase::Scene::CityBlock,
        showcase::Scene::Metropolis,
        showcase::Scene::CityTower,
        showcase::Scene::CityLandmark,
    ] {
        if scene.materials().is_none() {
            continue;
        }
        let building = showcase::building(scene).expect("showcase");
        let (library, _) = preflighted(scene, &building);
        assert!(
            !library.materials.is_empty(),
            "{scene:?} names no materials"
        );
        checked += 1;
    }
    assert!(
        checked >= 8,
        "all material-bearing scenes should be covered"
    );
}

#[test]
fn buildings_use_default_graphs_and_keep_scene_customizations() {
    let scene = showcase::Scene::CorporateBlock;
    let block = showcase::building(scene).expect("showcase");
    let (library, _) = preflighted(scene, &block);
    for (key, graph) in [
        ("library:formed-concrete", "library:formed-concrete"),
        ("showcase:corporate-stone", "library:stone-cladding"),
        ("library:painted-metal", "library:painted-metal"),
        ("library:glass", "library:glass"),
    ] {
        let Surface::Graph(bake) = &library.materials[key].surface else {
            panic!("{key} is baked");
        };
        assert_eq!(bake.graph, graph);
        assert_eq!(bake.resolution, 512);
    }
    let bound: std::collections::BTreeSet<_> = block
        .bindings()
        .filter(|(_, slot, _)| *slot == "stone")
        .map(|(_, _, binding)| binding.clone())
        .collect();
    assert_eq!(bound.len(), 3, "one stone variation per building");
    assert!(
        bound
            .iter()
            .all(|b| b.material == "showcase:corporate-stone")
    );
}

#[test]
fn every_sheet_preflights_against_its_own_specimens_and_close_up() {
    let graphs = showcase::materials::graphs();
    let names = showcase::sheet::names();
    assert!(names.len() >= 24, "{names:?}");
    for name in &names {
        let library = showcase::sheet::definitions(name);
        for building in [
            showcase::sheet::building(name).expect("a sheet builds"),
            showcase::sheet::detail(name).expect("a close-up builds"),
        ] {
            ashlar_bevy::check_library_with_graphs(
                name,
                &library,
                &asset_root(),
                &building,
                &graphs,
            )
            .unwrap_or_else(|error| panic!("{name}: {error:#}"));
        }
    }
}

/// What every shipped library's materials should get from a comparison with
/// their own graphs, once [`ashlar::MaterialDefinition::tile_scale`] says
/// which factors are on purpose.
///
/// The companion to the preflight tests above: those check each library
/// through `check_library_with_graphs`, which logs through `tracing` when a definition
/// tiles at a repeat its graph was not drawn for and never fails a build over
/// it. This checks the content of that log without a subscriber to catch it,
/// by calling the same two pure functions `preflight_tiling` itself calls —
/// [`ashlar_bevy::tiling`] and [`ashlar_bevy::scale_verdict`] — over every
/// definition that names a graph. A definition this fails on is exactly the
/// one that would warn on a real load.
#[test]
fn every_shipped_library_declares_the_scale_it_tiles_its_graphs_at() {
    let graphs = showcase::materials::graphs();
    let mut libraries = vec![("showcase".to_owned(), showcase::library::materials())];
    for name in showcase::sheet::names() {
        libraries.push((format!("sheet-{name}"), showcase::sheet::definitions(&name)));
    }

    let mut checked = 0;
    for (key, library) in libraries {
        for (material, definition) in &library.materials {
            let Some(graph_key) = definition.surface.graph() else {
                continue;
            };
            let Some(declared) = graphs.get(graph_key).and_then(|graph| graph.tile_metres) else {
                continue;
            };
            match ashlar_bevy::tiling(Some(declared), definition.tile_metres) {
                ashlar_bevy::Tiling::Agreed => {}
                ashlar_bevy::Tiling::Scaled(factor) => {
                    let verdict = ashlar_bevy::scale_verdict(
                        factor,
                        definition.tile_scale,
                        definition.strands.is_some(),
                    );
                    assert_eq!(
                        verdict,
                        ashlar_bevy::ScaleVerdict::Declared,
                        "{key}/{material}: tiles graph {graph_key:?} at a factor of \
                         {factor:.3} against declared tile_scale {:?} and strands {:?} — a \
                         real load would warn ({verdict:?})",
                        definition.tile_scale,
                        definition.strands.is_some(),
                    );
                }
                ashlar_bevy::Tiling::Stretched(factors) => {
                    panic!(
                        "{key}/{material}: tiles graph {graph_key:?} at {factors:?} of its \
                         declared repeat, a different shape rather than a different size"
                    );
                }
            }
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "no shipped definition named a graph with a declared repeat"
    );
}

/// Rows a bake divides across. Explicit and small, for the reason every other
/// bake in this workspace states.
const THREADS: Option<std::num::NonZeroUsize> = std::num::NonZeroUsize::new(8);

#[test]
fn the_block_bakes_a_wall_per_building_out_of_one_material_key() {
    // The claim per-instance overrides exist for, measured on the content that
    // uses them: three buildings, one `showcase:corporate-stone` in the
    // library, one graph behind it, three texture sets in the cache — and the
    // hundred and more bays between them paying for none of it, because a
    // binding they share is a cache entry they share.
    let scene = showcase::Scene::CorporateBlock;
    let block = showcase::building(scene).expect("showcase");
    let (library, graphs) = preflighted(scene, &block);

    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<Image>();
    let mut cache = BakeCache::new();
    let mut walls = 0;
    for (instance, slot, binding) in block.bindings() {
        if slot != "stone" {
            continue;
        }
        walls += 1;
        let definition = ashlar_bevy::definition(&library, binding)
            .unwrap_or_else(|error| panic!("{instance}: {error:#}"));
        app.world_mut()
            .resource_scope(|world, mut images: Mut<Assets<Image>>| {
                let server = world.resource::<AssetServer>().clone();
                create_graph_material(
                    &definition,
                    &server,
                    &mut BakeContext {
                        graphs: &graphs,
                        cache: &mut cache,
                        images: &mut images,
                        threads: THREADS,
                        baker: Baker::Cpu,
                    },
                )
            })
            .unwrap_or_else(|error| panic!("{instance}: {error:#}"));
    }

    assert!(
        walls > 30,
        "the block is more than a handful of bays: {walls}"
    );
    assert_eq!(cache.len(), 3, "one texture set per building, and no more");
    assert_eq!(
        cache
            .keys()
            .map(ashlar_bevy::runtime_bake::BakeKey::graph)
            .collect::<std::collections::BTreeSet<_>>(),
        ["library:stone-cladding"].into_iter().collect(),
        "three sets, one graph: an override changes values and never the graph"
    );
    // And a fourth building at a seed one of them already wears is free.
    let shared = block
        .bindings()
        .find(|(_, slot, _)| *slot == "stone")
        .map(|(_, _, binding)| binding.clone())
        .expect("the block clads something");
    let definition = ashlar_bevy::definition(&library, &shared).expect("a bound wall");
    app.world_mut()
        .resource_scope(|world, mut images: Mut<Assets<Image>>| {
            let server = world.resource::<AssetServer>().clone();
            create_graph_material(
                &definition,
                &server,
                &mut BakeContext {
                    graphs: &graphs,
                    cache: &mut cache,
                    images: &mut images,
                    threads: THREADS,
                    baker: Baker::Cpu,
                },
            )
        })
        .expect("a repeated binding");
    assert_eq!(cache.len(), 3);
}
