#![allow(clippy::unwrap_used, reason = "test fixtures")]
//! A solid of revolution stands on Y, closes on its axis and is watertight.
use ashlar::{Building, Element, Geometry, GeometryMesher, Instance, Part};
use ashlar_manifold::{ManifoldMesher, mesh_building};

fn dome() -> Geometry {
    let mut profile = vec![[0.0, 0.0], [2.0, 0.0], [2.0, 1.0]];
    for step in 1..8 {
        let angle = std::f64::consts::FRAC_PI_2 * f64::from(step) / 8.0;
        profile.push([2.0 * angle.cos(), 1.0 + 2.0 * angle.sin()]);
    }
    profile.push([0.0, 3.0]);
    Geometry::revolve(profile, 24)
}

#[test]
fn a_dome_on_a_drum_meshes_upright_and_closed() {
    let part = Part::builder("test:dome")
        .element(Element::new("body", dome(), "shell"))
        .build()
        .expect("a dome validates");
    let building = Building::builder("test:dome")
        .part(part)
        .instance(Instance::new("dome", "test:dome"))
        .material("shell", "test:shell")
        .build()
        .expect("a building validates");
    let meshed = mesh_building(&building, &ManifoldMesher::default()).expect("a dome meshes");
    let mesh = &meshed.parts["test:dome"][0].mesh;
    let (mut low, mut high) = ([f64::MAX; 3], [f64::MIN; 3]);
    for p in &mesh.positions {
        for axis in 0..3 {
            low[axis] = low[axis].min(p[axis]);
            high[axis] = high[axis].max(p[axis]);
        }
    }
    assert!(
        (low[1] - 0.0).abs() < 1e-4 && (high[1] - 3.0).abs() < 1e-4,
        "{low:?} {high:?}"
    );
    assert!(
        (high[0] - 2.0).abs() < 1e-3 && (low[0] + 2.0).abs() < 1e-3,
        "{low:?} {high:?}"
    );
    assert!(
        mesh.indices.len() / 3 > 24 * 8,
        "{}",
        mesh.indices.len() / 3
    );
}

#[test]
fn a_revolve_past_the_segment_budget_is_refused_by_path() {
    let part = Part::builder("test:dense")
        .element(Element::new(
            "body",
            Geometry::revolve([[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]], 100_000),
            "shell",
        ))
        .build()
        .expect("validation allows it; the mesher's budget does not");
    let building = Building::builder("test:dense")
        .part(part)
        .instance(Instance::new("dense", "test:dense"))
        .material("shell", "test:shell")
        .build()
        .unwrap();
    let error = mesh_building(&building, &ManifoldMesher::default()).unwrap_err();
    assert!(error.to_string().contains("segment budget"), "{error}");
}

#[test]
fn a_partial_sweep_turns_from_plus_x_towards_minus_z() {
    let wall = [[2.8, 0.0], [3.0, 0.0], [3.0, 2.5], [2.8, 2.5]];
    let mut mesh = ManifoldMesher::default()
        .mesh(&Geometry::revolve_arc(wall, 48, 90.0))
        .expect("a quarter wall meshes");
    mesh.unweld();
    let (mut low, mut high) = ([f64::MAX; 3], [f64::MIN; 3]);
    for p in &mesh.positions {
        for axis in 0..3 {
            low[axis] = low[axis].min(p[axis]);
            high[axis] = high[axis].max(p[axis]);
        }
    }
    // A quarter from +X to -Z: X and -Z both reach the outer radius, and
    // neither -X nor +Z is entered.
    assert!(high[0] > 2.99 && low[2] < -2.99, "{low:?} {high:?}");
    assert!(low[0] > -1e-6 && high[2] < 1e-6, "{low:?} {high:?}");
}
