//! `bake`: a building meshed once per level of a ladder.
#![allow(
    clippy::unwrap_used,
    reason = "test fixtures, where a failure is the test failing"
)]
use ashlar::{
    BakedBuilding, Building, Collision, Element, Geometry, Instance, LodPolicy, MergeGroup, Part,
    Pose, Side,
};
use ashlar_manifold::{ManifoldMesher, bake};

fn room() -> Part {
    Part::builder("test:room")
        .element(
            Element::new(
                "wall",
                Geometry::cuboid([4.0, 3.0, 0.3]).subtract(
                    Geometry::cuboid([1.0, 2.1, 0.5])
                        .placed(Pose::at([1.5, 0.0, -0.1]))
                        .portal("door"),
                ),
                "wall",
            )
            .collision(Collision::Bounds),
        )
        .element(
            Element::new(
                "liner",
                Geometry::cuboid([4.0, 3.0, 0.02]).placed(Pose::at([0.0, 0.0, 0.3])),
                "liner",
            )
            .interior(),
        )
        .element(Element::new(
            "column",
            Geometry::cylinder(0.3, 3.0, 48).placed(Pose::at([-1.0, 0.0, 0.0])),
            "trim",
        ))
        .build()
        .unwrap()
}

fn plain() -> Part {
    Part::builder("test:slab")
        .element(Element::new(
            "slab",
            Geometry::cuboid([4.0, 0.2, 4.0]),
            "floor",
        ))
        .build()
        .unwrap()
}

fn lamp() -> Part {
    Part::builder("test:lamp")
        .element(Element::new("lamp", Geometry::cuboid([0.2; 3]), "light"))
        .build()
        .unwrap()
}

fn building(merged: bool) -> Building {
    let mut builder = Building::builder("test:site")
        .part(room())
        .part(plain())
        .part(lamp())
        .material("wall", "plaster")
        .material("liner", "paint")
        .material("trim", "steel")
        .material("floor", "concrete")
        .material("light", "glow")
        .group(MergeGroup::new("ground").storey(0))
        .instance(Instance::new("room", "test:room").group("ground"))
        .instance(
            Instance::new("slab", "test:slab")
                .placed(Pose::at([0.0, -0.2, 0.0]))
                .group("ground"),
        )
        .instance(Instance::new("lamp", "test:lamp").placed(Pose::at([2.0, 2.5, 1.0])));
    if merged {
        builder = builder.merged();
    }
    builder.build().unwrap()
}

fn ladder() -> Vec<LodPolicy> {
    LodPolicy::ladder()
}

#[test]
fn an_empty_ladder_is_the_building_as_authored_to_any_distance() {
    let baked = bake(&building(false), &[], &ManifoldMesher::default()).unwrap();
    assert_eq!(baked.levels.len(), 1);
    assert_eq!(baked.levels[0].until, None);
}

#[test]
fn each_level_simplifies_and_an_unchanged_part_is_the_level_before() {
    let baked = bake(&building(false), &ladder(), &ManifoldMesher::default()).unwrap();
    assert_eq!(baked.levels.len(), 3);
    let triangles = baked.triangles();
    assert!(
        triangles[1] < triangles[0] && triangles[2] <= triangles[1],
        "{triangles:?}"
    );
    // The slab has nothing to simplify, so its meshes are level zero's.
    assert_eq!(
        baked.levels[0].parts["test:slab"][0].mesh,
        baked.levels[2].parts["test:slab"][0].mesh
    );
    // The lamp is gone past level zero, and the level still draws.
    assert!(!baked.levels[1].parts.contains_key("test:lamp"));
    let pieces = baked.level(1).unwrap().pieces().count();
    assert!(pieces > 0);
    // The liner is interior and gone; the column has fewer segments.
    let room = &baked.levels[1].parts["test:room"];
    assert!(room.iter().all(|element| element.side == Side::Exterior));
    let column = |level: usize| {
        baked.levels[level].parts["test:room"]
            .iter()
            .find(|element| element.id == "column")
            .unwrap()
            .mesh
            .triangle_count()
    };
    assert!(column(1) < column(0), "{} {}", column(0), column(1));
    // Colliders and portals are level zero's at every level.
    assert_eq!(baked.level(2).unwrap().colliders().len(), 1);
    assert_eq!(baked.level(2).unwrap().portals().len(), 1);
}

#[test]
fn a_merged_building_is_merged_at_every_level() {
    let baked = bake(&building(true), &ladder(), &ManifoldMesher::default()).unwrap();
    for (index, level) in baked.levels.iter().enumerate() {
        // The ungrouped lamp is the default group's only member, and it is
        // gone past level zero; the ground group is merged at every level.
        let expected = if index == 0 { 2 } else { 1 };
        assert_eq!(level.groups.len(), expected, "level {index}");
        assert!(level.groups.iter().any(|group| group.id == "ground"));
        if index > 0 {
            assert!(
                level
                    .groups
                    .iter()
                    .flat_map(|group| &group.batches)
                    .all(|batch| batch.side == Side::Exterior),
                "level {index} keeps the interior"
            );
        }
    }
    let bytes = baked.write().unwrap();
    let back = BakedBuilding::read(&bytes).unwrap();
    assert_eq!(back.triangles(), baked.triangles());
}

#[test]
fn a_prop_that_vanishes_at_a_distance_bakes_an_empty_level() {
    use ashlar::{Building, Element, Geometry, Instance, LodPolicy, Part};
    use ashlar_manifold::{ManifoldMesher, bake};
    let post = Building::builder("test:post")
        .part(
            Part::builder("test:post")
                .element(Element::new(
                    "pole",
                    Geometry::cylinder(0.05, 1.2, 12),
                    "metal",
                ))
                .build()
                .expect("a post"),
        )
        .instance(Instance::new("post", "test:post"))
        .material("metal", "test:steel")
        .build()
        .expect("a post building");
    let baked = bake(&post, &LodPolicy::ladder(), &ManifoldMesher::default())
        .expect("a small prop bakes at every level");
    let triangles = baked.triangles();
    assert!(triangles[0] > 0);
    assert_eq!(*triangles.last().expect("levels"), 0, "{triangles:?}");
    let back = ashlar::BakedBuilding::read(&baked.write().expect("writes")).expect("reads");
    assert_eq!(back.triangles(), triangles);
}
