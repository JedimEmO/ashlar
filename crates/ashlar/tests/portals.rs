//! A cutter marked as a portal: where the mark is legal, and how a backend that
//! cannot derive a rectangle answers for it.
use ashlar::*;

fn doorway() -> Geometry {
    Geometry::cuboid([4.0, 3.0, 0.3]).subtract(
        Geometry::cuboid([1.0, 2.2, 0.5])
            .placed(Pose::at([1.5, -0.2, -0.1]))
            .portal("front-door"),
    )
}

#[test]
fn a_portal_is_only_valid_on_a_cutter() {
    let lone = Geometry::cuboid([1.0; 3]).portal("front-door");
    let error = lone.check().expect_err("a solid is not an opening");
    assert!(error.path.ends_with(".portal"), "{}", error.path);
    assert_eq!(error.reason, "only a cutter is a portal");

    doorway().check().expect("a cutter root is an opening");

    let blank = Geometry::cuboid([4.0, 3.0, 0.3]).subtract(
        Geometry::cuboid([1.0, 2.2, 0.5])
            .placed(Pose::at([1.5, -0.2, -0.1]))
            .portal("  "),
    );
    let error = blank.check().expect_err("a blank portal id is not a name");
    assert!(error.path.ends_with(".portal"), "{}", error.path);
}

#[test]
fn a_portal_under_an_array_is_refused() {
    let geometry = doorway().arrayed(3, Pose::at([5.0, 0.0, 0.0]));
    let error = geometry
        .check()
        .expect_err("an array has no one rigid frame");
    assert!(error.path.ends_with(".portal"), "{}", error.path);
    assert_eq!(
        error.reason,
        "a portal cannot sit under an array or a mirror"
    );
}

#[test]
fn a_portal_under_a_mirror_is_refused() {
    let geometry = doorway().mirrored(MirrorPlane::new(Axis::X, 0.0));
    let error = geometry
        .check()
        .expect_err("a mirror has no one rigid frame");
    assert!(error.path.ends_with(".portal"), "{}", error.path);
    assert_eq!(
        error.reason,
        "a portal cannot sit under an array or a mirror"
    );
}

#[test]
fn duplicate_portal_ids_are_refused() {
    let geometry = Geometry::cuboid([6.0, 3.0, 0.3])
        .subtract(
            Geometry::cuboid([1.0, 2.0, 0.5])
                .placed(Pose::at([1.0, 0.0, -0.1]))
                .portal("door"),
        )
        .subtract(
            Geometry::cuboid([1.0, 2.0, 0.5])
                .placed(Pose::at([4.0, 0.0, -0.1]))
                .portal("door"),
        );
    let error = geometry.check().expect_err("a portal id is a key");
    assert!(error.path.ends_with(".portal"), "{}", error.path);
    assert_eq!(error.reason, "duplicate portal");
}

#[test]
fn an_unmarked_geometry_round_trips_without_the_field() {
    let plain = Geometry::cuboid([4.0, 3.0, 0.3])
        .subtract(Geometry::cuboid([1.0, 2.2, 0.5]).placed(Pose::at([1.5, -0.2, -0.1])));
    let encoded = ron::to_string(&plain).expect("serialize");
    assert!(!encoded.contains("portal"), "{encoded}");
    let decoded: Geometry = ron::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded, plain);

    let marked = doorway();
    let encoded = ron::to_string(&marked).expect("serialize");
    assert!(encoded.contains("portal"), "{encoded}");
    let decoded: Geometry = ron::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded, marked);
}

struct MeshOnly;

impl GeometryMesher for MeshOnly {
    fn mesh(&self, _geometry: &Geometry) -> Result<TriangleMesh, MeshError> {
        Ok(TriangleMesh::default())
    }
}

#[test]
fn a_backend_without_portals_answers_none_for_plain_geometry_and_refuses_marked() {
    let plain = Geometry::cuboid([4.0, 3.0, 0.3])
        .subtract(Geometry::cuboid([1.0, 2.2, 0.5]).placed(Pose::at([1.5, -0.2, -0.1])));
    assert!(
        MeshOnly
            .portals(&plain)
            .expect("no portal marked")
            .is_empty()
    );

    let error = MeshOnly
        .portals(&doorway())
        .expect_err("a marked cutter is refused");
    assert_eq!(error.path, "geometry");
    assert_eq!(error.reason, "this backend cannot derive portals");
}
