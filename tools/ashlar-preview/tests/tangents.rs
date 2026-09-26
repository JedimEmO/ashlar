//! The showcase kits must survive the whole path to a Bevy mesh with normal maps.
#![allow(
    clippy::float_cmp,
    reason = "a cut flag is written as exactly zero or exactly one, and that is the claim"
)]
use ashlar_manifold::{ManifoldMesher, mesh_building};
use ashlar_showcase as showcase;

#[test]
fn every_showcase_surface_supports_finite_tangents_for_normal_maps() {
    for scene in [showcase::Scene::Outpost, showcase::Scene::CorporateBlock] {
        let building = showcase::building(scene).expect("showcase");
        let output = mesh_building(&building, &ManifoldMesher::default()).expect("mesh");
        for element in output.parts.values().flatten() {
            let render = ashlar_bevy::mesh(&element.mesh).expect("projected UVs");
            let Some(bevy::mesh::VertexAttributeValues::Float32x4(tangents)) =
                render.attribute(bevy::prelude::Mesh::ATTRIBUTE_TANGENT)
            else {
                panic!("missing tangents");
            };
            assert!(tangents.iter().flatten().all(|v| v.is_finite()));
            assert!(
                tangents
                    .iter()
                    .all(|v| bevy::math::Vec3::new(v[0], v[1], v[2]).length() > 0.9)
            );
        }
    }
}

/// The study's own two answers to a cut face, end to end through the mesher.
///
/// The facade bay's panel is one element, one mesh and one material, and its
/// cut faces are told apart by the flag it uploads; the entrance bay's is split
/// into two meshes at mesh time and told apart by the slot each wears. Both
/// have to keep working, which is why the study ships one of each.
#[test]
fn the_facade_panel_carries_its_cut_flag_and_the_entrance_panel_carries_a_slot() {
    let building = showcase::building(showcase::Scene::Outpost).expect("showcase");
    let output = mesh_building(&building, &ManifoldMesher::default()).expect("mesh");

    let facade: Vec<_> = output.parts["study:facade"]
        .iter()
        .filter(|element| element.id == "panel")
        .collect();
    assert_eq!(facade.len(), 1, "the facade's panel is one mesh");
    let panel = facade[0];
    assert_eq!(panel.material_slot, "panel");
    assert!(
        panel.mesh.cut_faces().any(|cut| cut),
        "the window is cut out of the panel"
    );
    let render = ashlar_bevy::mesh(&panel.mesh).expect("projected UVs");
    let Some(bevy::mesh::VertexAttributeValues::Float32x2(flags)) =
        render.attribute(ashlar_bevy::ATTRIBUTE_ASHLAR_CUT)
    else {
        panic!("a mesh with cut faces uploads the cut flag");
    };
    assert!(
        flags.iter().all(|flag| flag[0] == 0.0 || flag[0] == 1.0),
        "welding split the vertices either side of the cut"
    );
    let cut = flags.iter().filter(|flag| flag[0] == 1.0).count();
    assert!(
        cut > 0 && cut < flags.len(),
        "{cut} of {} vertices are on a cut face",
        flags.len()
    );

    // The entrance bay splits instead, and each half is one kind of face — so
    // neither half needs the attribute and neither carries it.
    let entrance: Vec<_> = output.parts["study:entrance"]
        .iter()
        .filter(|element| element.id == "panel")
        .collect();
    assert_eq!(entrance.len(), 2, "the entrance's panel is outer and cut");
    for element in entrance {
        let render = ashlar_bevy::mesh(&element.mesh).expect("projected UVs");
        assert_eq!(element.mesh.cut_faces().any(|cut| cut), element.is_cut);
        assert_eq!(
            render
                .attribute(ashlar_bevy::ATTRIBUTE_ASHLAR_CUT)
                .is_some(),
            element.is_cut,
            "{} carries the flag only where every face is a cut face",
            element.material_slot
        );
    }
}
