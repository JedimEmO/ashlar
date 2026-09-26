//! Mesh upload, without a GPU. Nothing here opens a window or a render device:
//! `Mesh` is plain data until something extracts it, and tangent generation is
//! CPU work, so the whole f64-to-f32 boundary is testable headlessly.
#![allow(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "exact analytic fixtures; a failure is a test failure"
)]
use std::collections::BTreeMap;

use ashlar::{
    Binding, Diagonal, FaceSource, GroundRect, GroundSolid, GroundingPolicy, GroundingSpec,
    GroupBatch, MaterialDefinition, MaterialLibrary, Pose, Side, Socket, Surface, TerrainPatch,
    TriangleMesh, WELD_TOLERANCE, fit_ground,
};
use bevy::{
    MinimalPlugins,
    app::App,
    asset::{AssetApp, AssetPlugin, AssetServer, Assets, Handle},
    ecs::change_detection::Mut,
    math::{DVec3, Vec3},
    mesh::{Indices, Mesh, VertexAttributeValues},
    pbr::StandardMaterial,
};

/// Two triangles of one 2 by 3 metre quad in the XY plane, facing +Z, already
/// indexed: four vertices for six corners.
fn quad() -> TriangleMesh {
    TriangleMesh {
        positions: vec![
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(2.0, 0.0, 0.0),
            DVec3::new(2.0, 3.0, 0.0),
            DVec3::new(0.0, 3.0, 0.0),
        ],
        normals: vec![DVec3::Z; 4],
        uvs: vec![[0.0, 0.0], [2.0, 0.0], [2.0, 3.0], [0.0, 3.0]],
        indices: vec![0, 1, 2, 0, 2, 3],
        sources: vec![FaceSource::BODY, FaceSource::BODY],
    }
}

/// The same quad as unshared corners, the shape a mesher emits before welding.
fn quad_soup() -> TriangleMesh {
    let indexed = quad();
    let mut soup = TriangleMesh::default();
    for corners in indexed.corners() {
        soup.push_triangle(
            corners.map(|i| indexed.positions[i]),
            corners.map(|i| indexed.normals[i]),
            corners.map(|i| indexed.uvs[i]),
            FaceSource::BODY,
        );
    }
    soup
}

fn indices(mesh: &Mesh) -> Vec<usize> {
    match mesh.indices().expect("uploaded meshes are indexed") {
        Indices::U32(values) => values.iter().map(|i| *i as usize).collect(),
        Indices::U16(values) => values.iter().map(|i| *i as usize).collect(),
    }
}

fn positions(mesh: &Mesh) -> Vec<[f32; 3]> {
    mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        .and_then(VertexAttributeValues::as_float3)
        .expect("positions")
        .to_vec()
}

fn normals(mesh: &Mesh) -> Vec<[f32; 3]> {
    mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
        .and_then(VertexAttributeValues::as_float3)
        .expect("normals")
        .to_vec()
}

fn uvs(mesh: &Mesh) -> Vec<[f32; 2]> {
    match mesh.attribute(Mesh::ATTRIBUTE_UV_0).expect("uvs") {
        VertexAttributeValues::Float32x2(values) => values.clone(),
        other => panic!("unexpected UV attribute {other:?}"),
    }
}

fn tangents(mesh: &Mesh) -> Vec<[f32; 4]> {
    match mesh.attribute(Mesh::ATTRIBUTE_TANGENT).expect("tangents") {
        VertexAttributeValues::Float32x4(values) => values.clone(),
        other => panic!("unexpected tangent attribute {other:?}"),
    }
}

/// Every corner of every triangle, as the triple the renderer will read.
fn corners(mesh: &Mesh) -> Vec<([f32; 3], [f32; 3], [f32; 2])> {
    let (positions, normals, uvs) = (positions(mesh), normals(mesh), uvs(mesh));
    indices(mesh)
        .into_iter()
        .map(|i| (positions[i], normals[i], uvs[i]))
        .collect()
}

#[test]
fn an_indexed_upload_keeps_its_indices_positions_normals_and_metre_uvs() {
    let source = quad();
    assert!(source.is_consistent());
    let mesh = ashlar_bevy::mesh(&source).expect("upload");

    assert_eq!(indices(&mesh), vec![0, 1, 2, 0, 2, 3]);
    assert_eq!(positions(&mesh).len(), 4, "vertices are still shared");
    assert_eq!(
        positions(&mesh),
        vec![
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 3.0, 0.0],
            [0.0, 3.0, 0.0]
        ]
    );
    assert_eq!(normals(&mesh), vec![[0.0, 0.0, 1.0]; 4]);
    // Metres, not a zero-to-one rectangle: the material's tile size is what
    // turns them into repeats.
    assert_eq!(
        uvs(&mesh),
        vec![[0.0, 0.0], [2.0, 0.0], [2.0, 3.0], [0.0, 3.0]]
    );
}

#[test]
fn generated_tangents_are_finite_unit_vectors_across_the_surface() {
    let mesh = ashlar_bevy::mesh(&quad()).expect("upload");
    let tangents = tangents(&mesh);
    assert_eq!(tangents.len(), 4);
    for tangent in tangents {
        assert!(tangent.iter().all(|v| v.is_finite()), "{tangent:?}");
        let direction = Vec3::new(tangent[0], tangent[1], tangent[2]);
        assert!((direction.length() - 1.0).abs() < 1e-3, "{tangent:?}");
        // A +Z face mapped by its own metre frame has a tangent in the plane.
        assert!(direction.z.abs() < 1e-3, "{tangent:?}");
        assert!(tangent[3].abs() == 1.0, "handedness sign");
    }
}

#[test]
fn welding_a_soup_shares_vertices_without_changing_the_uploaded_surface() {
    let soup = quad_soup();
    let mut welded = soup.clone();
    welded.weld(WELD_TOLERANCE);
    assert_eq!(soup.positions.len(), 6, "one vertex per corner");
    assert_eq!(welded.positions.len(), 4, "the diagonal pair is shared");

    let (soup, welded) = (
        ashlar_bevy::mesh(&soup).expect("upload"),
        ashlar_bevy::mesh(&welded).expect("upload"),
    );
    assert_eq!(indices(&soup).len(), indices(&welded).len());
    assert_eq!(corners(&soup), corners(&welded));
    assert!(
        positions(&welded).len() < positions(&soup).len(),
        "welding is what saves the vertices"
    );
}

/// The flag a compiled material reads off a vertex, and the seam that makes it
/// one answer per face.
#[test]
fn a_mesh_with_a_cut_face_uploads_the_cut_flag_and_one_without_uploads_nothing() {
    let plain = ashlar_bevy::mesh(&quad()).expect("upload");
    assert!(
        plain.attribute(ashlar_bevy::ATTRIBUTE_ASHLAR_CUT).is_none(),
        "a surface with nothing cut out of it carries no flag and no vertex bytes"
    );

    // The same quad with its second triangle cut, welded the way every mesher
    // in this workspace welds: the diagonal is a seam, so six vertices, three
    // of them flagged.
    let mut sawn = quad_soup();
    sawn.sources = vec![FaceSource::BODY, FaceSource::cutter(0)];
    sawn.weld(WELD_TOLERANCE);
    assert_eq!(sawn.positions.len(), 6, "the cut boundary is a weld seam");

    let uploaded = ashlar_bevy::mesh(&sawn).expect("upload");
    let flags = match uploaded
        .attribute(ashlar_bevy::ATTRIBUTE_ASHLAR_CUT)
        .expect("a cut surface carries the flag")
    {
        VertexAttributeValues::Float32x2(values) => values.clone(),
        other => panic!("the cut flag is a two-lane float attribute, not {other:?}"),
    };
    assert_eq!(flags.len(), positions(&uploaded).len(), "one per vertex");
    assert!(
        flags.iter().all(|flag| flag[0] == 0.0 || flag[0] == 1.0),
        "a welded mesh has no vertex on both sides of a cut: {flags:?}"
    );
    assert_eq!(
        flags.iter().filter(|flag| flag[0] == 1.0).count(),
        3,
        "the three corners of the one cut triangle"
    );
    assert!(
        flags.iter().all(|flag| flag[1] == 0.0),
        "the second lane is reserved and written zero"
    );
    // Every triangle's three corners agree, which is what makes the value a
    // fragment interpolates exactly one or exactly zero.
    for (face, corners) in sawn.corners().enumerate() {
        let wanted = f32::from(u8::from(sawn.is_cut(face)));
        for corner in corners {
            assert_eq!(flags[corner][0], wanted, "face {face}, vertex {corner}");
        }
    }
}

/// An unwelded soup is the one shape a vertex can be on both sides of a cut in,
/// and the upload takes the maximum rather than refusing it. It is a mesh to
/// weld, not a mesh to drop.
#[test]
fn a_vertex_shared_across_a_cut_takes_the_flag_of_the_cut_face() {
    let mut shared = quad();
    shared.sources = vec![FaceSource::BODY, FaceSource::cutter(0)];
    let uploaded = ashlar_bevy::mesh(&shared).expect("upload");
    let flags = match uploaded
        .attribute(ashlar_bevy::ATTRIBUTE_ASHLAR_CUT)
        .expect("a cut surface carries the flag")
    {
        VertexAttributeValues::Float32x2(values) => values.clone(),
        other => panic!("the cut flag is a two-lane float attribute, not {other:?}"),
    };
    // Vertices 0 and 2 are the shared diagonal; both belong to the cut face.
    assert_eq!(
        flags.iter().map(|flag| flag[0]).collect::<Vec<_>>(),
        vec![1.0, 0.0, 1.0, 1.0]
    );
}

/// A flat site, so the fitted supports and ramp are exact and the test asserts
/// on orientation rather than on the solver's search.
fn flat_site() -> Vec<GroundSolid> {
    let heights = vec![10.0; 81 * 81];
    let terrain =
        TerrainPatch::with_diagonal([-40.0, -40.0], 1.0, [81, 81], heights, Diagonal::Anti)
            .expect("patch");
    let spec = GroundingSpec {
        footprint: GroundRect::new([-4.0, 0.0], [4.0, 6.0]),
        floor: 0.0,
        slab_bottom: -0.3,
        entrance: Socket::new(
            "entrance",
            Pose::at([0.0, 0.0, 0.0])
                .rotated(bevy::math::DQuat::from_rotation_y(std::f64::consts::PI)),
        ),
    };
    let policy = GroundingPolicy {
        clearance: 0.4,
        embed: 0.3,
        max_depth: 5.0,
        support_width: 0.3,
        max_span: 2.0,
        landing_length: 1.5,
        ramp_width: 2.0,
        max_grade: 0.3,
        max_ramp_length: 18.0,
        search_step: 0.5,
        vegetation_margin: 0.75,
        max_toe_step: 0.2,
    };
    fit_ground(&spec, policy, &terrain, [0.0, 0.0], 0.0)
        .expect("flat ground fits")
        .solids
}

#[test]
fn a_fitted_site_uploads_with_outward_normals() {
    let solids = flat_site();
    assert!(solids.len() > 4, "perimeter supports plus landing and ramp");
    let mesh = ashlar_bevy::ground_mesh(&solids).expect("upload");
    let (positions, normals, indices) = (positions(&mesh), normals(&mesh), indices(&mesh));

    let mut volume = 0.0;
    for triangle in indices.as_chunks::<3>().0 {
        let [a, b, c] = [0, 1, 2].map(|i| Vec3::from_array(positions[triangle[i]]));
        let winding = (b - a).cross(c - a);
        assert!(winding.length() > 0.0, "degenerate uploaded triangle");
        for corner in triangle {
            let normal = Vec3::from_array(normals[*corner]);
            assert!(
                normal.normalize().dot(winding.normalize()) > 0.999,
                "shading normal disagrees with the winding it was built from"
            );
        }
        // Divergence theorem: with every face wound outward the signed volume
        // of the closed prisms is positive, and inward faces would cancel it.
        volume += a.dot(b.cross(c)) / 6.0;
    }
    assert!(
        volume > 1.0,
        "enclosed volume {volume} is not outward-wound"
    );
}

#[test]
fn an_empty_site_uploads_an_empty_mesh_rather_than_failing() {
    let mesh = ashlar_bevy::ground_mesh(&[]).expect("upload");
    assert_eq!(indices(&mesh).len(), 0);
    assert_eq!(positions(&mesh).len(), 0);
}

/// The `strands` feature pulls no graph engine, said by the manifest.
///
/// The claim the whole 2026-09-20 split rests on: a game that grows grass links
/// `ashlar-strands` and not `ashlar-material`, so `cargo tree -e normal
/// --features strands` has no graph engine in it. Nothing else in the suite
/// would notice if that stopped being true — every build would still compile,
/// and the only symptom would be a game's binary quietly gaining a bake
/// engine — so it is asserted here, against the one file that decides it.
///
/// Read out of `Cargo.toml` by hand rather than through a TOML parser: this
/// crate has no parser in its tree and a dependency added to check a
/// dependency boundary is a poor trade. The two lines below are the whole of
/// what makes the boundary hold, and the test fails with the line it found
/// when either of them moves.
#[test]
fn the_strands_feature_pulls_no_graph_engine() {
    let manifest = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
    )
    .unwrap();
    let line = |prefix: &str| {
        manifest
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with(prefix))
            .unwrap_or_else(|| panic!("no line starting {prefix:?} in ashlar-bevy's Cargo.toml"))
            .to_owned()
    };

    // `strands` enables exactly one thing, and it is the graph-free crate.
    assert_eq!(
        line("strands ="),
        r#"strands = ["dep:ashlar-strands"]"#,
        "the `strands` feature enables something other than `ashlar-strands`, so a game that \
         grows grass no longer gets the game half alone"
    );
    // And `ashlar-material` is reachable only through `runtime-bake`, which is
    // the tool feature `strands` does not imply.
    assert_eq!(
        line("runtime-bake ="),
        r#"runtime-bake = ["dep:ashlar-material"]"#,
        "the feature that pulls the graph engine has moved; check that `strands` still does not \
         reach it"
    );
    assert!(
        line("ashlar-material =").contains("optional = true"),
        "`ashlar-material` is no longer optional, so every build of this crate links the graph \
         engine whatever its features say: {}",
        line("ashlar-material =")
    );
}

/// One building-space group batch wearing `binding`, with [`quad`] for geometry.
fn batch(binding: &str) -> GroupBatch {
    GroupBatch {
        binding: Binding::new(binding),
        side: Side::Exterior,
        mesh: quad(),
    }
}

/// A library of two `Plain` definitions, one per binding the batches wear.
///
/// Constants rather than files, so no material this test creates asks the asset
/// server to load anything and the app needs no image loader.
fn batch_library() -> MaterialLibrary {
    let materials = [
        (
            "test:plain".to_owned(),
            MaterialDefinition {
                base_color: [0.85, 0.05, 0.05],
                roughness: 1.0,
                metallic: 0.0,
                surface: Surface::Plain,
                ..MaterialDefinition::default()
            },
        ),
        (
            "test:brick".to_owned(),
            MaterialDefinition {
                base_color: [1.0, 1.0, 1.0],
                roughness: 1.0,
                metallic: 1.0,
                surface: Surface::Plain,
                ..MaterialDefinition::default()
            },
        ),
    ]
    .into_iter()
    .collect();
    MaterialLibrary { materials }
}

/// One `batch_drawables` call against an app that has the two asset stores and
/// a server and no render device.
fn draw_batches<'a>(
    app: &mut App,
    server: &AssetServer,
    batches: &'a [GroupBatch],
    library: &'a MaterialLibrary,
    cache: &mut BTreeMap<Binding, Handle<StandardMaterial>>,
) -> Vec<ashlar_bevy::BatchDrawable<'a>> {
    app.world_mut()
        .resource_scope(|world, mut meshes: Mut<Assets<Mesh>>| {
            world.resource_scope(|_, mut materials: Mut<Assets<StandardMaterial>>| {
                let mut cx = ashlar_bevy::UploadContext {
                    server,
                    meshes: &mut meshes,
                    materials: &mut materials,
                };
                ashlar_bevy::batch_drawables(batches, library, cache, &mut cx)
                    .expect("the fixture batches name definitions the fixture library holds")
            })
        })
}

/// `batch_drawables` uploads every batch, and shares a material between two
/// batches wearing one binding.
///
/// The claim a game pays for after a hit: a wall hit repeatedly does not create
/// a material per hit. This needs no renderer — `AssetPlugin` is the whole of
/// what an `AssetServer` needs and `Assets<Mesh>` and `Assets<StandardMaterial>`
/// are plain data until something extracts them — so three batches, three
/// uploads, two materials and a cache of two are checkable here.
#[test]
fn batch_drawables_reuses_a_material_and_uploads_every_batch() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>();
    let server = app.world().resource::<AssetServer>().clone();

    let batches = vec![
        batch("test:plain"),
        batch("test:brick"),
        batch("test:brick"),
    ];
    let library = batch_library();
    let mut cache = BTreeMap::new();

    let first = draw_batches(&mut app, &server, &batches, &library, &mut cache);
    assert_eq!(first.len(), 3, "none of these three batches is empty");
    // One binding, one material: the two brick batches share the handle the
    // first of them created, and the plain batch has one of its own.
    assert_ne!(first[0].material, first[1].material);
    assert_eq!(first[1].material, first[2].material);
    // A group batch is shared with nothing, so three batches mean three
    // uploads, even where two of them wear the same binding.
    assert_ne!(first[0].mesh, first[1].mesh);
    assert_ne!(first[1].mesh, first[2].mesh);
    assert_ne!(first[0].mesh, first[2].mesh);
    assert_eq!(cache.len(), 2, "one cache entry per distinct binding");
    assert_eq!(
        app.world()
            .resource::<Assets<StandardMaterial>>()
            .iter()
            .count(),
        2,
        "the app holds one material per binding, not one per batch"
    );

    // The second hit: the same batches against the same cache. Nothing new is
    // created, which is what makes re-hitting a wall cheap.
    let second = draw_batches(&mut app, &server, &batches, &library, &mut cache);
    assert_eq!(second.len(), first.len());
    assert_eq!(second[0].material, first[0].material);
    assert_eq!(second[1].material, first[1].material);
    assert_eq!(cache.len(), 2);
    assert_eq!(
        app.world()
            .resource::<Assets<StandardMaterial>>()
            .iter()
            .count(),
        2,
        "a second call with the same cache created a material"
    );
}
