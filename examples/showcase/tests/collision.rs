//! The kits used to carry hand-typed collision tables next to the geometry, and
//! the two drifted apart whenever a dimension moved. The proxies are derived
//! from the evaluated solids now; these fixtures are the last version of those
//! tables, kept so the derived set can be shown to cover what they covered.
use ashlar::{Building, ConvexSolid, Pose};
use ashlar_manifold::{ManifoldMesher, mesh_building};
use ashlar_showcase::{Scene, building, corporate};
use glam::DVec3;

const STOREY: f64 = 3.8;
const PODIUM: f64 = 6.45;

/// A box of the hand tables, as size and minimum corner in part space.
type Box3 = ([f64; 3], [f64; 3]);

fn outpost_table(part: &str) -> Vec<Box3> {
    match part {
        "study:facade" => vec![
            ([4.0, 1.8, 0.85], [0.0, 0.0, 0.0]),
            ([4.0, 1.35, 0.85], [0.0, 3.45, 0.0]),
            ([0.6, 1.65, 0.85], [0.0, 1.8, 0.0]),
            ([0.6, 1.65, 0.85], [3.4, 1.8, 0.0]),
            ([2.8, 1.65, 0.045], [0.6, 1.8, 0.69]),
        ],
        "study:entrance" => vec![
            ([0.65, 3.35, 0.85], [0.0, 0.0, 0.0]),
            ([0.65, 3.35, 0.85], [3.35, 0.0, 0.0]),
            ([4.0, 1.45, 0.85], [0.0, 3.35, 0.0]),
            ([2.7, 3.15, 0.14], [0.65, 0.12, 0.69]),
            ([2.85, 0.12, 1.15], [0.575, -0.02, -0.48]),
        ],
        "foundation" | "roof" => vec![([12.65, 0.3, 8.65], [-0.325, 0.0, -0.325])],
        "canopy" => vec![([4.6, 0.24, 2.3], [0.0, 0.0, 0.0])],
        "study:parapet" => vec![([3.97, 0.94, 0.42], [0.015, 0.0, -0.075])],
        "study:plant" => vec![([3.95, 2.07, 3.0], [0.025, 0.0, 0.0])],
        _ => Vec::new(),
    }
}

fn corporate_table(part: &str) -> Vec<Box3> {
    match part {
        "metro:podium" | "metro:curtain" => {
            let (h, bottom, opening) = if part == "metro:podium" {
                (6.0, 0.55, 4.5)
            } else {
                (STOREY, 0.4, 2.95)
            };
            vec![
                ([0.42, h, 0.8], [0.0, 0.0, 0.0]),
                ([0.42, h, 0.8], [3.58, 0.0, 0.0]),
                ([3.16, bottom, 0.8], [0.42, 0.0, 0.0]),
                (
                    [3.16, h - bottom - opening, 0.8],
                    [0.42, bottom + opening, 0.0],
                ),
                ([3.12, opening - 0.04, 0.065], [0.44, bottom + 0.02, 0.64]),
            ]
        }
        "metro:lobby" => vec![
            ([1.2, 6.0, 1.6], [0.0, 0.0, 0.0]),
            ([1.2, 6.0, 1.6], [6.8, 0.0, 0.0]),
            ([5.6, 1.22, 1.6], [1.2, 4.78, 0.0]),
            ([5.6, 4.5, 0.12], [1.2, 0.13, 1.25]),
            ([5.8, 0.15, 1.95], [1.1, -0.02, -0.6]),
        ],
        "metro:base" => vec![([16.65, 0.45, 12.65], [-0.325, -0.47, -0.325])],
        "metro:podium-cap" => vec![([17.2, 0.45, 13.2], [-0.6, 0.0, -0.6])],
        "metro:tower-cap" => vec![([12.9, 0.5, 8.9], [-0.45, 0.0, -0.45])],
        "metro:core-base" => vec![([2.6, PODIUM, 6.7], [0.0, 0.0, 0.0])],
        "metro:service-riser" => vec![([2.6, STOREY, 6.7], [0.0, 0.0, 0.0])],
        "metro:core-crown" => vec![([2.95, 2.6, 7.1], [-0.175, 0.0, -0.2])],
        "metro:plant" => vec![([4.0, 2.4, 3.0], [0.0, 0.0, 0.0])],
        _ => Vec::new(),
    }
}

/// The hand boxes were rounded to the nearest few centimetres, and they ignored
/// projections: a facade's box stopped at the panel face while the cornice above
/// it oversails a quarter of a metre. A third of a metre is that rounding.
const SLACK: f64 = 0.3;

fn bounds(boxes: impl IntoIterator<Item = ([f64; 3], [f64; 3])>, pose: Pose) -> [DVec3; 2] {
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    for (size, at) in boxes {
        for corner in 0..8 {
            let point = pose.transform_point(DVec3::new(
                at[0] + if corner & 1 == 0 { 0.0 } else { size[0] },
                at[1] + if corner & 2 == 0 { 0.0 } else { size[1] },
                at[2] + if corner & 4 == 0 { 0.0 } else { size[2] },
            ));
            min = min.min(point);
            max = max.max(point);
        }
    }
    [min, max]
}

fn inside(point: DVec3, solid: &ConvexSolid) -> bool {
    let [min, max] = solid.bounds().expect("a proxy has vertices");
    (0..3).all(|axis| point[axis] >= min[axis] - 1e-6 && point[axis] <= max[axis] + 1e-6)
}

/// Assert that the proxies the elements declare stand for the same solid the
/// hand table did: the same occupied volume to within its rounding, and no box
/// of the table left without something in the middle of it.
fn covers(building: &Building, table: fn(&str) -> Vec<Box3>, expected: usize) {
    let meshed = mesh_building(building, &ManifoldMesher::default()).expect("meshed");
    let derived = meshed.colliders();
    assert_eq!(derived.len(), expected, "derived proxy count");
    for instance in &building.recipe().instances {
        let proxies: Vec<_> = derived
            .iter()
            .filter(|c| c.instance == instance.id)
            .collect();
        let hand = table(&instance.part);
        assert_eq!(
            hand.is_empty(),
            proxies.is_empty(),
            "{} has {} hand boxes and {} derived proxies",
            instance.id,
            hand.len(),
            proxies.len()
        );
        if hand.is_empty() {
            continue;
        }
        let expected = bounds(hand.clone(), instance.pose);
        let actual = bounds(
            proxies.iter().filter_map(|c| {
                let [min, max] = c.solid.bounds()?;
                Some(((max - min).to_array(), min.to_array()))
            }),
            Pose::default(),
        );
        for axis in 0..3 {
            assert!(
                (actual[0][axis] - expected[0][axis]).abs() < SLACK
                    && (actual[1][axis] - expected[1][axis]).abs() < SLACK,
                "{} occupies {actual:?} where the hand table occupied {expected:?}",
                instance.id
            );
        }
        for (size, at) in hand {
            let centre = instance.pose.transform_point(DVec3::new(
                at[0] + size[0] / 2.0,
                at[1] + size[1] / 2.0,
                at[2] + size[2] / 2.0,
            ));
            assert!(
                proxies.iter().any(|c| inside(centre, &c.solid)),
                "{} leaves the middle of its {size:?} box at {at:?} without a proxy",
                instance.id
            );
        }
    }
}

#[test]
fn derived_proxies_cover_what_the_hand_tables_covered() {
    covers(
        &building(Scene::Outpost).expect("outpost"),
        outpost_table,
        88,
    );
    covers(
        &corporate::building(4).expect("tower"),
        corporate_table,
        117,
    );
    covers(&corporate::block().expect("block"), corporate_table, 246);
}

#[test]
fn decoration_carries_no_proxy_and_a_part_is_evaluated_once() {
    let building = building(Scene::Outpost).expect("outpost");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("meshed");
    let facade = &meshed.part_colliders["study:facade"];
    let named: Vec<&str> = facade.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        named,
        [
            "panel",
            "top-course",
            "left",
            "right",
            "recessed-glass",
            "plinth"
        ]
    );
    // One proxy set per definition, however many times it is placed.
    let placements = building
        .recipe()
        .instances
        .iter()
        .filter(|i| i.part == "study:facade")
        .count();
    assert!(placements > facade.len());
    // The window opening is inside the panel element, so its proxy is the whole
    // panel: a conservative box, not a carved one.
    let panel = &facade[0].solid.bounds().expect("panel bounds");
    assert!(panel[0].y < 0.1 && panel[1].y > 4.7);
}
