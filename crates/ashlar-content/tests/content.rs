//! The content step writes what the game path reads.
use ashlar::{
    BakedBuilding, Building, Element, Geometry, Instance, LodPolicy, MaterialLibrary, Part, Surface,
};
use ashlar_content::{Content, LIBRARY_FILE};
use ashlar_material::stdlib;

fn building() -> Building {
    Building::builder("game:shed")
        .part(
            Part::builder("game:shed")
                .element(Element::new(
                    "walls",
                    Geometry::chamfered_cuboid([4.0, 3.0, 3.0], 0.1),
                    "wall",
                ))
                .element(Element::new(
                    "vent",
                    Geometry::cylinder(0.1, 0.2, 24).placed(ashlar::Pose::at([2.0, 3.0, 1.5])),
                    "wall",
                ))
                .build()
                .expect("a shed"),
        )
        .instance(Instance::new("shed", "game:shed"))
        .material("wall", "library:plaster")
        .build()
        .expect("a shed building")
}

#[test]
fn materials_and_a_building_land_where_the_game_path_reads_them() {
    let root = tempfile::tempdir().expect("a temp dir");
    let content = Content::new(root.path()).resolution(Some(256));
    let mut definitions = stdlib::materials();
    definitions
        .materials
        .retain(|key, _| key == "library:paving-slabs");
    let written = content
        .write_materials(&stdlib::graphs(), &definitions, "materials")
        .expect("materials write");
    assert_eq!(written.sets.len(), 1);
    let text = std::fs::read_to_string(root.path().join("materials").join(LIBRARY_FILE))
        .expect("the library is written");
    let library: MaterialLibrary = ron::from_str(&text).expect("the library parses");
    let Surface::Files {
        base_color,
        baked_from,
        ..
    } = &library.materials["library:paving-slabs"].surface
    else {
        panic!("a baked definition becomes files");
    };
    assert!(baked_from.is_some());
    assert!(
        root.path()
            .join(base_color.as_ref().expect("a base map"))
            .is_file()
    );

    let baked = content
        .write_building("buildings/shed.ashlar", &building(), &LodPolicy::ladder())
        .expect("the building writes");
    assert_eq!(baked.triangles.len(), LodPolicy::ladder().len());
    let bytes = std::fs::read(root.path().join("buildings/shed.ashlar")).expect("written");
    let read = BakedBuilding::read(&bytes).expect("reads back");
    assert_eq!(read.levels.len(), baked.triangles.len());
    // No temporary file is left behind.
    assert!(!root.path().join("buildings/shed.ashlar.partial").exists());
}

#[test]
fn per_instance_overrides_become_definitions_of_their_own() {
    use ashlar::{Binding, ParamValue};
    use ashlar_content::flatten_overrides;

    let tinted = |id: &str, colour: [f32; 3]| {
        Instance::new(id, "game:shed").binding(
            "wall",
            Binding::new("library:plaster").param("color", ParamValue::Color(colour)),
        )
    };
    let recipe = building().into_recipe();
    let mut builder = Building::builder("game:street").part(recipe.parts[0].clone());
    builder = builder
        .material("wall", "library:plaster")
        .instance(tinted("a", [0.6, 0.5, 0.4]))
        .instance(tinted("b", [0.6, 0.5, 0.4]).placed(ashlar::Pose::at([5.0, 0.0, 0.0])))
        .instance(tinted("c", [0.3, 0.4, 0.5]).placed(ashlar::Pose::at([10.0, 0.0, 0.0])))
        .instance(Instance::new("d", "game:shed").placed(ashlar::Pose::at([15.0, 0.0, 0.0])));
    let street = builder.build().expect("a street");
    let (flattened, library) =
        flatten_overrides(&[street], &stdlib::materials()).expect("flattens");
    let street = &flattened[0];
    let key = |id: &str| street.material(id, "wall").expect("bound").to_owned();
    assert_eq!(key("a"), key("b"), "one override, one definition");
    assert_ne!(key("a"), key("c"));
    assert_eq!(key("d"), "library:plaster", "a plain binding is left alone");
    for id in ["a", "b", "c", "d"] {
        assert!(street.binding(id, "wall").expect("bound").params.is_empty());
        assert!(library.materials.contains_key(&key(id)), "{id}");
    }
    let Surface::Graph(bake) = &library.materials[&key("c")].surface else {
        panic!("an override is a baked definition until the export");
    };
    assert_eq!(
        bake.params["color"],
        ashlar::ParamValue::Color([0.3, 0.4, 0.5])
    );
}

#[test]
fn a_path_outside_the_root_is_refused() {
    let root = tempfile::tempdir().expect("a temp dir");
    let content = Content::new(root.path().join("assets"));
    for path in [
        "",
        ".",
        "../escape.ashlar",
        "/etc/escape.ashlar",
        "buildings/../../x",
    ] {
        let error = content.write_file(path, b"x").expect_err(path);
        assert!(
            matches!(error, ashlar_content::ContentError::OutsideRoot(_)),
            "{path}: {error}"
        );
    }
    assert!(!root.path().join("escape.ashlar").exists());
}

#[test]
fn replacing_an_asset_preserves_neighbouring_files_and_cleans_up_on_failure() {
    let root = tempfile::tempdir().expect("a temp dir");
    let content = Content::new(root.path());
    let neighbour = root.path().join("wall.ashlar.partial");
    std::fs::write(&neighbour, b"another writer's file").expect("neighbour");
    content
        .write_file("wall.ashlar", b"old")
        .expect("first write");
    content
        .write_file("wall.ashlar", b"new")
        .expect("replacement");
    assert_eq!(
        std::fs::read(root.path().join("wall.ashlar")).expect("asset"),
        b"new"
    );
    assert_eq!(
        std::fs::read(neighbour).expect("neighbour survives"),
        b"another writer's file"
    );

    // Replacing a directory must fail without leaving a temporary file behind.
    std::fs::create_dir(root.path().join("occupied")).expect("directory");
    assert!(content.write_file("occupied", b"data").is_err());
    assert_eq!(std::fs::read_dir(root.path()).expect("root").count(), 3);
}

#[test]
fn an_override_whose_key_is_taken_is_refused() {
    use ashlar::{Binding, ParamValue};
    use ashlar_content::flatten_overrides;

    let override_of =
        |colour| Binding::new("library:plaster").param("color", ParamValue::Color(colour));
    let street = |colour| {
        let recipe = building().into_recipe();
        Building::builder("game:street")
            .part(recipe.parts[0].clone())
            .material("wall", "library:plaster")
            .instance(Instance::new("a", "game:shed").binding("wall", override_of(colour)))
            .build()
            .expect("a street")
    };
    let (flattened, _) =
        flatten_overrides(&[street([0.6, 0.5, 0.4])], &stdlib::materials()).expect("flattens");
    let key = flattened[0]
        .material("a", "wall")
        .expect("bound")
        .to_owned();
    // A library that already holds that key as something else.
    let mut definitions = stdlib::materials();
    let other = definitions.materials["library:adobe"].clone();
    definitions.materials.insert(key, other);
    let error =
        flatten_overrides(&[street([0.6, 0.5, 0.4])], &definitions).expect_err("the key is taken");
    assert!(error.path.starts_with("instances[a]"), "{error}");
}

#[test]
fn review_pngs_land_beside_the_maps_and_the_library_still_names_the_ktx2() {
    let root = tempfile::tempdir().expect("a temp dir");
    let mut definitions = stdlib::materials();
    definitions
        .materials
        .retain(|key, _| key == "library:plaster" || key == "library:emissive-strip");
    let written = Content::new(root.path())
        .resolution(Some(512))
        .review_pngs(true)
        .write_materials(&stdlib::graphs(), &definitions, "materials")
        .expect("materials write");
    assert_eq!(written.sets.len(), 2);
    for (name, colour) in [
        ("plaster/base.png", image::ColorType::Rgba8),
        ("plaster/height.png", image::ColorType::L16),
        ("emissive-strip/emissive.png", image::ColorType::Rgba8),
    ] {
        let path = root.path().join("materials/library").join(name);
        let png = image::open(&path).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!((png.width(), png.height()), (512, 512), "{name}");
        assert_eq!(png.color(), colour, "{name}");
    }
    let text = std::fs::read_to_string(root.path().join("materials").join(LIBRARY_FILE))
        .expect("the library is written");
    assert!(!text.contains(".png"), "the library names the KTX2 files");
}
