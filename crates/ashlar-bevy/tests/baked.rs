//! The game path for baked content, headless: a `.ashlar` file and a
//! `.materials.ron` library loaded through the asset server, spawned by
//! `AshlarPlugin` with every level in its band, shared meshes and materials,
//! collision proxies and spaces. No render device and no feature.
#![allow(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "fixtures built in the test, and bands the plugin computes from the same literals"
)]
use std::{collections::BTreeMap, path::Path};

use ashlar::{
    BakedBuilding, BakedLevel, Building, Collision, Element, Geometry, Instance,
    MaterialDefinition, MaterialLibrary, Part, Pose, Room, Surface,
};
use ashlar_bevy::baked::{
    AshlarBuilding, AshlarBuildingSpawned, AshlarCache, AshlarCollider, AshlarFailed, AshlarPiece,
    AshlarPlugin, AshlarSpaces, AshlarSpawned, Bands,
};
use ashlar_manifold::{ManifoldMesher, mesh_building};
use bevy::{asset::AssetPlugin, camera::visibility::VisibilityRange, prelude::*};
use tempfile::TempDir;

/// A bay: a wall with a doorway marked as a portal and a bounds proxy, and a
/// small standalone light that the coarse level leaves out.
fn bay() -> Part {
    Part::builder("test:bay")
        .element(
            Element::new(
                "wall",
                Geometry::cuboid([4.0, 3.0, 0.3]).subtract(
                    Geometry::cuboid([1.0, 2.1, 0.5])
                        .placed(Pose::at([1.5, -0.05, -0.1]))
                        .portal("door"),
                ),
                "wall",
            )
            .cut_material("reveal")
            .collision(Collision::Bounds),
        )
        .element(
            Element::new(
                "lamp",
                Geometry::cuboid([0.3, 0.1, 0.1]).placed(Pose::at([1.85, 2.4, -0.1])),
                "light",
            )
            .standalone(),
        )
        .build()
        .unwrap()
}

fn building() -> Building {
    Building::builder("test:row")
        .part(bay())
        .material("wall", "test:plaster")
        .material("reveal", "test:steel")
        .material("light", "test:light")
        .instance(Instance::new("a", "test:bay"))
        .instance(Instance::new("b", "test:bay").placed(Pose::at([4.0, 0.0, 0.0])))
        .room(
            Room::new("inside", [8.0, 3.0, 3.0])
                .placed(Pose::at([0.0, 0.0, 0.3]))
                .portal("door"),
        )
        .build()
        .unwrap()
}

/// Two levels: the building as meshed until 50 m, then the same without its
/// lamps to any distance.
fn baked() -> BakedBuilding {
    let meshed = mesh_building(&building(), &ManifoldMesher::default()).unwrap();
    let coarse = meshed
        .parts
        .iter()
        .map(|(id, elements)| {
            let kept = elements
                .iter()
                .filter(|element| element.id != "lamp")
                .cloned()
                .collect();
            (id.clone(), kept)
        })
        .collect();
    BakedBuilding::new(
        meshed.building.clone(),
        vec![
            BakedLevel {
                until: Some(50.0),
                parts: meshed.parts.clone(),
                groups: Vec::new(),
            },
            BakedLevel {
                until: None,
                parts: coarse,
                groups: Vec::new(),
            },
        ],
        meshed.part_colliders.clone(),
        meshed.part_portals.clone(),
    )
}

fn plain(color: [f32; 3]) -> MaterialDefinition {
    MaterialDefinition {
        base_color: color,
        surface: Surface::Plain,
        ..MaterialDefinition::default()
    }
}

fn library() -> MaterialLibrary {
    MaterialLibrary {
        materials: BTreeMap::from([
            ("test:plaster".to_owned(), plain([0.8, 0.8, 0.75])),
            ("test:steel".to_owned(), plain([0.5, 0.5, 0.5])),
            ("test:light".to_owned(), plain([1.0, 1.0, 0.9])),
        ]),
    }
}

fn write(root: &Path, key: &str, bytes: &[u8]) {
    let path = root.join(key);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn app(root: &Path) -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin {
            file_path: root.to_string_lossy().into_owned(),
            ..default()
        },
        TransformPlugin,
    ))
    .init_asset::<Mesh>()
    .init_asset::<Image>()
    .init_asset::<StandardMaterial>()
    .add_plugins(AshlarPlugin::default());
    app
}

fn spawn(app: &mut App, building: &str, library: &str) -> Entity {
    let server = app.world().resource::<AssetServer>().clone();
    app.world_mut()
        .spawn(AshlarBuilding {
            building: server.load(building.to_owned()),
            materials: server.load(library.to_owned()),
        })
        .id()
}

/// Run frames until every root is spawned or failed.
fn settle(app: &mut App, roots: &[Entity]) {
    for _ in 0..3000 {
        app.update();
        if roots.iter().all(|root| {
            let entity = app.world().entity(*root);
            entity.contains::<AshlarSpawned>() || entity.contains::<AshlarFailed>()
        }) {
            return;
        }
    }
    panic!("the buildings never finished spawning");
}

fn fixture() -> TempDir {
    let root = TempDir::new().unwrap();
    write(
        root.path(),
        "buildings/row.ashlar",
        &baked().write().unwrap(),
    );
    write(
        root.path(),
        "materials/library.materials.ron",
        ron::to_string(&library()).unwrap().as_bytes(),
    );
    root
}

#[test]
fn a_baked_building_spawns_every_level_in_its_band() {
    let root = fixture();
    let mut app = app(root.path());
    let one = spawn(
        &mut app,
        "buildings/row.ashlar",
        "materials/library.materials.ron",
    );
    settle(&mut app, &[one]);
    assert!(
        !app.world().entity(one).contains::<AshlarFailed>(),
        "{:?}",
        app.world().entity(one).get::<AshlarFailed>()
    );

    let baked = baked();
    let expected: Vec<usize> = (0..2)
        .map(|level| {
            baked
                .level(level)
                .unwrap()
                .pieces()
                .filter(|piece| !piece.mesh.positions.is_empty())
                .count()
        })
        .collect();
    // A piece a coarser level did not change is one entity whose band runs on
    // through that level, so what draws at a level is every piece whose band
    // covers it.
    let world = app.world_mut();
    let mut pieces = world.query::<(&AshlarPiece, &VisibilityRange, &ChildOf)>();
    let mut counts = [0_usize; 2];
    for (piece, range, parent) in pieces.iter(world) {
        assert_eq!(parent.parent(), one);
        let open_ended = range.end_margin.start == f32::MAX;
        if piece.level == 0 {
            assert_eq!(range.start_margin, 0.0..0.0);
            counts[0] += 1;
            if open_ended {
                counts[1] += 1;
            } else {
                assert_eq!(range.end_margin, 50.0..55.0, "a tenth of the boundary");
            }
        } else {
            assert_eq!(range.start_margin, 50.0..55.0, "the crossfade is shared");
            assert!(open_ended, "the last level is open-ended");
            counts[1] += 1;
        }
    }
    assert_eq!(counts.to_vec(), expected);
    assert!(counts[1] < counts[0], "the coarse level drops the lamps");
    let mut levels = world.query::<&AshlarPiece>();
    assert!(
        levels.iter(world).all(|piece| piece.level == 0),
        "the walls did not change, so no piece starts at the coarse level"
    );

    // Two instances of one part at one level are one mesh, and one binding is
    // one material.
    let mut meshes = world.query::<(&AshlarPiece, &Mesh3d, &MeshMaterial3d<StandardMaterial>)>();
    // A label is `instance/element`, and an element with a cut slot is two
    // batches told apart by their materials.
    let mut by_label: BTreeMap<(usize, String, AssetId<StandardMaterial>), Handle<Mesh>> =
        BTreeMap::new();
    let mut instances = std::collections::BTreeSet::new();
    for (piece, mesh, material) in meshes.iter(world) {
        let (instance, element) = piece.label.split_once('/').unwrap();
        instances.insert(instance.to_owned());
        let key = (piece.level, element.to_owned(), material.0.id());
        let previous = by_label.insert(key, mesh.0.clone());
        if let Some(previous) = previous {
            assert_eq!(
                previous, mesh.0,
                "{element} at level {} is shared",
                piece.level
            );
        }
    }
    assert_eq!(instances.len(), 2, "both instances drew");
    let cache = world.resource::<AshlarCache>();
    assert_eq!(cache.materials(), 3, "three bindings, three materials");

    // Proxies and spaces.
    let mut colliders = world.query::<&AshlarCollider>();
    let colliders: Vec<_> = colliders.iter(world).collect();
    assert_eq!(colliders.len(), 2, "one wall proxy per instance");
    assert!(colliders.iter().all(|c| c.vertices.len() == 8));
    let spaces = world.entity(one).get::<AshlarSpaces>().unwrap();
    assert_eq!(spaces.rooms.len(), 1);
    assert_eq!(spaces.portals.len(), 2, "one doorway per instance");

    let messages = world.resource::<Messages<AshlarBuildingSpawned>>();
    assert_eq!(messages.len(), 1);
}

#[test]
fn a_second_building_from_the_same_files_shares_every_upload() {
    let root = fixture();
    let mut app = app(root.path());
    let one = spawn(
        &mut app,
        "buildings/row.ashlar",
        "materials/library.materials.ron",
    );
    settle(&mut app, &[one]);
    let (meshes, materials) = {
        let cache = app.world().resource::<AshlarCache>();
        (cache.meshes(), cache.materials())
    };
    let two = spawn(
        &mut app,
        "buildings/row.ashlar",
        "materials/library.materials.ron",
    );
    settle(&mut app, &[two]);
    let cache = app.world().resource::<AshlarCache>();
    assert_eq!(cache.meshes(), meshes);
    assert_eq!(cache.materials(), materials);
    assert_eq!(app.world().resource::<Assets<Mesh>>().len(), meshes);
}

#[test]
fn a_malformed_file_fails_the_building_and_panics_nothing() {
    let root = fixture();
    write(
        root.path(),
        "buildings/bad.ashlar",
        b"this is not a building",
    );
    let mut app = app(root.path());
    let bad = spawn(
        &mut app,
        "buildings/bad.ashlar",
        "materials/library.materials.ron",
    );
    settle(&mut app, &[bad]);
    let failed = app.world().entity(bad).get::<AshlarFailed>().unwrap();
    assert!(failed.0.contains("building"), "{}", failed.0);
    assert!(app.world().entity(bad).get::<Children>().is_none());
}

#[test]
fn a_missing_map_fails_the_library_by_its_key() {
    let root = fixture();
    let mut library = library();
    library.materials.insert(
        "test:plaster".to_owned(),
        MaterialDefinition {
            surface: Surface::Files {
                base_color: Some("materials/plaster/base.ktx2".to_owned()),
                normal: None,
                orm: None,
                height: None,
                emissive: None,
                baked_from: None,
            },
            ..MaterialDefinition::default()
        },
    );
    write(
        root.path(),
        "materials/missing.materials.ron",
        ron::to_string(&library).unwrap().as_bytes(),
    );
    let mut app = app(root.path());
    let root_entity = spawn(
        &mut app,
        "buildings/row.ashlar",
        "materials/missing.materials.ron",
    );
    settle(&mut app, &[root_entity]);
    let failed = app
        .world()
        .entity(root_entity)
        .get::<AshlarFailed>()
        .unwrap();
    assert!(
        failed.0.contains("materials/plaster/base.ktx2"),
        "{}",
        failed.0
    );
}

#[test]
fn a_material_graph_is_refused_by_the_feature_that_would_bake_it() {
    let root = fixture();
    let mut library = library();
    library.materials.insert(
        "test:plaster".to_owned(),
        MaterialDefinition {
            surface: Surface::Graph(ashlar::Bake {
                graph: "library:plaster".to_owned(),
                params: BTreeMap::new(),
                resolution: 512,
            }),
            ..MaterialDefinition::default()
        },
    );
    write(
        root.path(),
        "materials/graph.materials.ron",
        ron::to_string(&library).unwrap().as_bytes(),
    );
    let mut app = app(root.path());
    let root_entity = spawn(
        &mut app,
        "buildings/row.ashlar",
        "materials/graph.materials.ron",
    );
    settle(&mut app, &[root_entity]);
    let failed = app
        .world()
        .entity(root_entity)
        .get::<AshlarFailed>()
        .unwrap();
    assert!(failed.0.contains("material graph"), "{}", failed.0);
}

#[test]
fn a_changed_building_is_spawned_again() {
    let root = fixture();
    let mut app = app(root.path());
    let one = spawn(
        &mut app,
        "buildings/row.ashlar",
        "materials/library.materials.ron",
    );
    settle(&mut app, &[one]);
    let before = app.world().entity(one).get::<Children>().unwrap().len();
    // Whatever changed the asset — a file watcher, a game patching it — the
    // plugin sees one `Modified` and rebuilds from the new value.
    let id = app
        .world()
        .get::<AshlarBuilding>(one)
        .unwrap()
        .building
        .id();
    {
        let mut buildings = app
            .world_mut()
            .resource_mut::<Assets<ashlar_bevy::baked::BakedBuildingAsset>>();
        let mut asset = buildings.get_mut(id).unwrap();
        asset.0.levels.truncate(1);
        asset.0.levels[0].until = None;
    }
    app.update();
    settle(&mut app, &[one]);
    let world = app.world_mut();
    let mut pieces = world.query::<(&AshlarPiece, &ChildOf)>();
    let after: Vec<_> = pieces
        .iter(world)
        .filter(|(_, p)| p.parent() == one)
        .collect();
    assert!(after.iter().all(|(piece, _)| piece.level == 0));
    let lamps = after
        .iter()
        .filter(|(piece, _)| piece.label.ends_with("/lamp"))
        .count();
    assert_eq!(lamps, 2, "the one level keeps the lamps");
    let children = world.entity(one).get::<Children>().unwrap().len();
    assert_eq!(children, before, "the same pieces, now without bands");
    // A single level draws at every distance, so it has no band at all.
    let mut ranged = world.query::<(&AshlarPiece, &VisibilityRange)>();
    assert_eq!(ranged.iter(world).count(), 0);
}

#[test]
fn a_merged_group_is_drawn_about_the_centre_of_its_bounds() {
    // A level switches on the distance to the piece's origin. A merged group's
    // mesh is in building space, so drawn from the building origin a long row
    // would switch level by its distance to one end.
    let mut recipe = building().into_recipe();
    for index in 2..8 {
        recipe.instances.push(
            Instance::new(format!("i{index}"), "test:bay").placed(Pose::at([
                4.0 * f64::from(index),
                0.0,
                0.0,
            ])),
        );
    }
    recipe.merged = true;
    let row = recipe.build().unwrap();
    let baked = ashlar_manifold::bake(
        &row,
        &ashlar::LodPolicy::ladder(),
        &ManifoldMesher::default(),
    )
    .unwrap();
    let [low, high] = baked.levels[0].groups[0].bounds.unwrap();
    let centre = ((low + high) * 0.5).as_vec3();

    let root = fixture();
    write(
        root.path(),
        "buildings/long.ashlar",
        &baked.write().unwrap(),
    );
    let mut app = app(root.path());
    let one = spawn(
        &mut app,
        "buildings/long.ashlar",
        "materials/library.materials.ron",
    );
    settle(&mut app, &[one]);
    assert!(!app.world().entity(one).contains::<AshlarFailed>());
    let world = app.world_mut();
    let mut pieces = world.query::<(&AshlarPiece, &Transform, &Mesh3d)>();
    let mut groups = 0;
    for (piece, transform, mesh) in pieces.iter(world) {
        if piece.label.ends_with("/lamp") {
            continue;
        }
        groups += 1;
        assert!(
            transform.translation.distance(centre) < 1e-4,
            "{} is drawn from {} rather than {centre}",
            piece.label,
            transform.translation
        );
        let meshes = world.resource::<Assets<Mesh>>();
        let aabb =
            bevy::camera::primitives::MeshAabb::compute_aabb(meshes.get(&mesh.0).unwrap()).unwrap();
        assert!(
            Vec3::from(aabb.center).length() < 16.0,
            "the mesh is shifted to its pivot"
        );
    }
    assert!(groups > 0, "the row drew as a merged group");
}

#[test]
fn a_building_nothing_holds_leaves_the_cache() {
    let root = fixture();
    let mut app = app(root.path());
    let one = spawn(
        &mut app,
        "buildings/row.ashlar",
        "materials/library.materials.ron",
    );
    settle(&mut app, &[one]);
    assert!(app.world().resource::<AshlarCache>().meshes() > 0);
    app.world_mut().entity_mut(one).despawn();
    for _ in 0..10 {
        app.update();
    }
    let cache = app.world().resource::<AshlarCache>();
    assert_eq!(cache.meshes(), 0);
    assert_eq!(cache.materials(), 0);
    assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 0);
}

#[test]
fn abrupt_bands_cut_each_level_at_the_middle_of_its_crossfade() {
    // What a browser gets: Bevy 0.19.1 cannot crossfade a range on WebGL2, so
    // `Bands::Auto` cuts there instead. Forced here so a native test checks
    // the ranges; Bevy culls to them on the CPU either way.
    let root = fixture();
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin {
            file_path: root.path().to_string_lossy().into_owned(),
            ..default()
        },
        TransformPlugin,
    ))
    .init_asset::<Mesh>()
    .init_asset::<Image>()
    .init_asset::<StandardMaterial>()
    .add_plugins(AshlarPlugin {
        bands: Bands::Abrupt,
        ..AshlarPlugin::default()
    });
    let one = spawn(
        &mut app,
        "buildings/row.ashlar",
        "materials/library.materials.ron",
    );
    settle(&mut app, &[one]);
    let world = app.world_mut();
    let mut pieces = world.query::<(&AshlarPiece, &VisibilityRange)>();
    let mut cut = 0;
    for (piece, range) in pieces.iter(world) {
        assert!(range.is_abrupt(), "{} has nothing to dither", piece.label);
        // The fixture's boundary is at 50 m with a 5 m margin: the cut is at
        // its middle, where a crossfade would be half one level, half the
        // next.
        if range.end_margin.start < f32::MAX {
            assert_eq!(range.start_margin, 0.0..0.0);
            assert_eq!(range.end_margin, 52.5..52.5, "{}", piece.label);
            assert!(piece.label.ends_with("/lamp"), "only the lamps stop");
            cut += 1;
        }
    }
    assert_eq!(cut, 2, "both lamps stop at the cut");
}

#[test]
fn auto_bands_crossfade_natively_without_a_render_device() {
    // A render device decides `Auto` when one exists; a headless app has none,
    // and a native one crossfades as it always did.
    assert!(!Bands::Auto.abrupt_on(None));
    assert!(!Bands::Crossfade.abrupt_on(None));
    assert!(Bands::Abrupt.abrupt_on(None));
    assert_eq!(AshlarPlugin::default().bands, Bands::Auto);
}
