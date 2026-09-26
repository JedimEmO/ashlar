//! The content step: which definitions share a set, and which keys are refused.
use ashlar_material::{
    bake::bake_with_report,
    export::{ExportError, ExportRequest, export},
    stdlib,
};
use ashlar_surface::{MaterialLibrary, Surface};

fn request<'a>(
    graphs: &'a ashlar_material::MaterialGraphLibrary,
    definitions: &'a MaterialLibrary,
) -> ExportRequest<'a> {
    ExportRequest {
        graphs,
        definitions,
        directory: "materials",
        resolution: Some(256),
        threads: std::num::NonZeroUsize::new(2),
        backend: &bake_with_report,
    }
}

#[test]
fn a_definition_that_only_changes_its_repeat_shares_the_maps() {
    let graphs = stdlib::graphs();
    let mut definitions = stdlib::materials();
    definitions
        .materials
        .retain(|key, _| key == "library:paving-slabs");
    let mut wide = definitions.materials["library:paving-slabs"].clone();
    wide.tile_metres = [4.0, 4.0];
    definitions.materials.insert("game:plaza".into(), wide);
    let exported = export(&request(&graphs, &definitions)).expect("exports");
    assert_eq!(exported.sets.len(), 1, "one bake, one set");
    for definition in exported.library.materials.values() {
        assert!(matches!(definition.surface, Surface::Files { .. }));
    }
}

#[test]
fn a_definition_that_grows_strands_gets_its_own_set_beside_one_that_does_not() {
    let graphs = stdlib::graphs();
    let mut definitions = stdlib::materials();
    definitions
        .materials
        .retain(|key, _| key == "library:moss-carpet");
    // `a:` sorts before `library:`, so the bare definition is seen first — the
    // order that used to leave the strand-growing one without its set.
    let mut bare = definitions.materials["library:moss-carpet"].clone();
    bare.strands = None;
    definitions.materials.insert("a:bare-moss".into(), bare);
    let exported = export(&ExportRequest {
        resolution: Some(512),
        ..request(&graphs, &definitions)
    })
    .expect("exports");
    let grown = &exported.library.materials["library:moss-carpet"];
    let set = grown
        .strands
        .as_ref()
        .and_then(|settings| settings.baked_set.clone())
        .expect("the strand-growing definition names a set");
    assert!(
        exported
            .sets
            .iter()
            .any(|exported| exported.strands.is_some() && set.starts_with(&exported.directory)),
        "{set}"
    );
}

#[test]
fn a_key_that_would_leave_the_directory_is_refused() {
    let graphs = stdlib::graphs();
    let plaster = stdlib::materials().materials["library:plaster"].clone();
    for key in [
        "library:../../x",
        "a:b:c",
        "Library:Plaster",
        "plaster",
        "a:",
    ] {
        let definitions = MaterialLibrary {
            materials: [(key.to_owned(), plaster.clone())].into_iter().collect(),
        };
        assert!(
            matches!(
                export(&request(&graphs, &definitions)),
                Err(ExportError::Key(_))
            ),
            "{key}"
        );
    }
}
