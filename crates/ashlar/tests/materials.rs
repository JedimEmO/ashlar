//! Material validation and independent palette interchange.
#![expect(
    clippy::float_cmp,
    reason = "a constant carried through an override has to arrive as exactly the number \
              the definition wrote"
)]
use std::collections::BTreeMap;

use ashlar::{
    Bake, Binding, Building, Element, Geometry, Instance, MaterialDefinition, MaterialLibrary,
    ParamValue, Part, Surface,
};

/// A `Files` surface naming only the keys a case cares about.
fn textures(keys: [Option<&str>; 5]) -> Surface {
    let [base_color, normal, orm, height, emissive] = keys.map(|k| k.map(str::to_owned));
    Surface::Files {
        base_color,
        normal,
        orm,
        height,
        emissive,
        baked_from: None,
    }
}

/// A graph request with no parameters, at a resolution the bake accepts.
fn bake(resolution: u32) -> Bake {
    Bake {
        graph: "study:concrete".into(),
        params: BTreeMap::new(),
        resolution,
    }
}

#[test]
fn material_libraries_validate_cut_bindings_and_round_trip() {
    let part = Part::builder("wall")
        .element(Element::new("panel", Geometry::cuboid([1.0; 3]), "outer").cut_material("cut"))
        .build()
        .expect("part");
    let building = Building::builder("test")
        .part(part)
        .material("outer", "paint")
        .material("cut", "bare")
        .instance(Instance::new("wall", "wall"))
        .build()
        .expect("building");
    let mut library = MaterialLibrary::default();
    library
        .materials
        .insert("paint".into(), MaterialDefinition::default());
    assert!(
        library
            .check_for(&building)
            .expect_err("cut missing")
            .path
            .contains("cut")
    );
    library.materials.insert(
        "bare".into(),
        MaterialDefinition {
            metallic: 0.8,
            tile_metres: [2.0, 3.0],
            ..Default::default()
        },
    );
    library.check_for(&building).expect("complete palette");
    let encoded = ron::to_string(&library).expect("serialize");
    let decoded: MaterialLibrary = ron::from_str(&encoded).expect("decode");
    assert_eq!(library, decoded);
}

#[test]
fn invalid_material_numbers_and_empty_texture_keys_are_rejected() {
    for definition in [
        MaterialDefinition {
            roughness: f32::NAN,
            ..Default::default()
        },
        MaterialDefinition {
            metallic: 1.1,
            ..Default::default()
        },
        MaterialDefinition {
            tile_metres: [0.0, 1.0],
            ..Default::default()
        },
        MaterialDefinition {
            emissive: [-0.1; 3],
            ..Default::default()
        },
        MaterialDefinition {
            surface: textures([None, Some(" "), None, None, None]),
            ..Default::default()
        },
        MaterialDefinition {
            surface: textures([None, None, None, None, Some("")]),
            ..Default::default()
        },
    ] {
        assert!(definition.check("material").is_err());
    }
    assert!(
        MaterialDefinition {
            emissive: [8.0; 3],
            ..Default::default()
        }
        .check("light")
        .is_ok()
    );
}

#[test]
fn texture_keys_name_every_map_of_a_textured_surface_and_nothing_of_the_others() {
    let definition = MaterialDefinition {
        surface: textures([
            Some("base.png"),
            Some("normal.png"),
            Some("orm.png"),
            Some("height.png"),
            Some("emissive.png"),
        ]),
        ..Default::default()
    };
    assert_eq!(
        definition.texture_keys().collect::<Vec<_>>(),
        [
            "base.png",
            "normal.png",
            "orm.png",
            "height.png",
            "emissive.png"
        ]
    );
    definition.check("material").expect("named keys");

    for surface in [
        Surface::Plain,
        Surface::Graph(bake(1024)),
        Surface::Shader {
            graph: "study:concrete".into(),
            params: BTreeMap::new(),
        },
    ] {
        let definition = MaterialDefinition {
            surface,
            ..Default::default()
        };
        assert_eq!(
            definition.texture_keys().count(),
            0,
            "a graph names no files"
        );
        definition.check("material").expect("valid surface");
    }
}

#[test]
fn a_bake_resolution_must_be_a_power_of_two_the_bake_pipeline_accepts() {
    for resolution in [0, 1, 128, 1000, 8192] {
        let error = MaterialDefinition {
            surface: Surface::Graph(bake(resolution)),
            ..Default::default()
        }
        .check("material")
        .expect_err("out of range or not a power of two");
        assert_eq!(error.path, "material.surface.resolution");
    }
    for resolution in [256, 512, 1024, 2048, 4096] {
        MaterialDefinition {
            surface: Surface::Graph(bake(resolution)),
            ..Default::default()
        }
        .check("material")
        .expect("accepted resolution");
    }
}

#[test]
fn a_graph_surface_is_rejected_by_path_for_a_blank_key_or_a_parameter_that_is_not_finite() {
    let error = MaterialDefinition {
        surface: Surface::Shader {
            graph: "  ".into(),
            params: BTreeMap::new(),
        },
        ..Default::default()
    }
    .check("material")
    .expect_err("blank graph key");
    assert_eq!(error.path, "material.surface.graph");

    for value in [
        ParamValue::Float(f32::NAN),
        ParamValue::Color([0.0, f32::INFINITY, 0.0]),
    ] {
        let error = MaterialDefinition {
            surface: Surface::Graph(Bake {
                graph: "study:concrete".into(),
                params: BTreeMap::from([("wear".to_owned(), value)]),
                resolution: 1024,
            }),
            ..Default::default()
        }
        .check("material")
        .expect_err("non-finite parameter");
        assert_eq!(error.path, "material.surface.params[wear]");
    }

    // A blank parameter name is reported with the name in the path, so it can
    // be found in a map of forty.
    let error = MaterialDefinition {
        surface: Surface::Shader {
            graph: "study:concrete".into(),
            params: BTreeMap::from([(" ".to_owned(), ParamValue::Float(0.5))]),
        },
        ..Default::default()
    }
    .check("material")
    .expect_err("blank parameter name");
    assert_eq!(error.path, "material.surface.params[ ]");

    // The provenance a file bake records is a bake, and is checked like one.
    let error = MaterialDefinition {
        surface: Surface::Files {
            base_color: Some("base.png".into()),
            normal: None,
            orm: None,
            height: None,
            emissive: None,
            baked_from: Some(bake(300)),
        },
        ..Default::default()
    }
    .check("material")
    .expect_err("bad provenance resolution");
    assert_eq!(error.path, "material.surface.baked_from.resolution");
}

#[test]
fn every_surface_variant_round_trips_through_ron_and_a_plain_one_is_the_default() {
    assert_eq!(MaterialDefinition::default().surface, Surface::Plain);
    let mut library = MaterialLibrary::default();
    for (key, surface) in [
        ("plain", Surface::Plain),
        (
            "files",
            textures([Some("base.png"), None, None, None, Some("glow.png")]),
        ),
        (
            "baked",
            Surface::Graph(Bake {
                params: BTreeMap::from([
                    ("seed".to_owned(), ParamValue::Int(7)),
                    ("tint".to_owned(), ParamValue::Color([0.6, 0.5, 0.4])),
                ]),
                ..bake(512)
            }),
        ),
        (
            "live",
            Surface::Shader {
                graph: "study:concrete".into(),
                params: BTreeMap::from([
                    ("wear".to_owned(), ParamValue::Float(0.25)),
                    ("wet".to_owned(), ParamValue::Bool(true)),
                ]),
            },
        ),
    ] {
        library.materials.insert(
            key.to_owned(),
            MaterialDefinition {
                surface,
                ..Default::default()
            },
        );
    }
    let encoded = ron::to_string(&library).expect("serialize");
    assert_eq!(
        ron::from_str::<MaterialLibrary>(&encoded).expect("decode"),
        library
    );
}

#[test]
fn a_textured_surface_may_omit_the_maps_it_does_not_have() {
    let library: MaterialLibrary =
        ron::from_str("(materials: {\"paint\": (surface: Files(base_color: Some(\"base.png\")))})")
            .expect("decode");
    assert_eq!(
        library.materials["paint"].surface,
        textures([Some("base.png"), None, None, None, None])
    );
}

#[test]
fn a_misspelled_key_inside_a_surface_is_a_parse_error_rather_than_a_dropped_map() {
    // `Surface` carries `#[serde(deny_unknown_fields)]`, which serde applies to
    // its struct variants, and `Bake` carries its own. Without them a typo
    // would deserialise to a surface with no maps at all, pass `check`,
    // preflight clean and render as untextured constants.
    for (surface, misspelling, expected) in [
        (
            "Files(basecolour: Some(\"a.png\"))",
            "basecolour",
            "base_color",
        ),
        ("Graph((graph: \"g\", res: 1024))", "res", "resolution"),
        ("Shader(graph: \"g\", parms: {})", "parms", "params"),
    ] {
        let error = ron::from_str::<MaterialLibrary>(&format!(
            "(materials: {{\"paint\": (surface: {surface})}})"
        ))
        .expect_err("an unknown field inside a surface must not be ignored")
        .to_string();
        assert!(error.contains(misspelling), "{error}");
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn a_blank_texture_key_is_reported_against_the_map_it_belongs_to() {
    for (index, field) in ["base_color", "normal", "orm", "height", "emissive"]
        .into_iter()
        .enumerate()
    {
        let mut keys = [None; 5];
        keys[index] = Some(" ");
        let error = MaterialDefinition {
            surface: textures(keys),
            ..Default::default()
        }
        .check("material")
        .expect_err("blank key");
        assert_eq!(error.path, format!("material.surface.{field}"));
    }
}

#[test]
fn an_instance_may_override_the_parameters_of_a_graph_surface_and_of_nothing_else() {
    let part = Part::builder("wall")
        .element(Element::new("panel", Geometry::cuboid([1.0; 3]), "outer"))
        .build()
        .expect("part");
    let dressed = |binding: Binding| {
        Building::builder("test")
            .part(part.clone())
            .material("outer", "paint")
            .instance(Instance::new("wall", "wall").binding("outer", binding))
            .build()
            .expect("building")
    };
    let mut library = MaterialLibrary::default();
    library.materials.insert(
        "paint".into(),
        MaterialDefinition {
            surface: Surface::Graph(bake(512)),
            ..Default::default()
        },
    );
    let building = dressed(Binding::new("paint").param("wear", ParamValue::Float(0.5)));
    library
        .check_for(&building)
        .expect("a Graph surface takes parameters");

    // A file surface's `baked_from` is provenance rather than a request, so
    // there is nothing there to override and saying so is a validation error
    // at the binding rather than a value that quietly does nothing.
    library.materials.insert(
        "paint".into(),
        MaterialDefinition {
            surface: textures([Some("a.png"), None, None, None, None]),
            ..Default::default()
        },
    );
    let error = library
        .check_for(&building)
        .expect_err("a Files surface takes none");
    assert_eq!(error.path, "instances[wall].materials[outer]");
    assert!(error.reason.contains("Files"), "{}", error.reason);

    library
        .materials
        .insert("paint".into(), MaterialDefinition::default());
    assert!(
        library
            .check_for(&building)
            .expect_err("a Plain surface takes none")
            .reason
            .contains("Plain")
    );
    // And a binding that overrides nothing is what it always was, whatever the
    // surface under it is.
    library
        .check_for(&dressed(Binding::new("paint")))
        .expect("a plain binding on a plain surface");
}

#[test]
fn an_override_replaces_one_value_of_a_surface_and_leaves_the_rest_of_it_alone() {
    let definition = MaterialDefinition {
        base_color: [0.5, 0.6, 0.7],
        surface: Surface::Graph(Bake {
            params: [
                ("variation".to_owned(), ParamValue::Float(0.0)),
                ("cracking".to_owned(), ParamValue::Float(0.62)),
            ]
            .into(),
            ..bake(512)
        }),
        ..Default::default()
    };
    let binding = Binding::new("stone").param("variation", ParamValue::Float(0.37));
    let dressed = definition.overridden(&binding).expect("a graph surface");
    let Surface::Graph(bake) = &dressed.surface else {
        panic!("an overridden bake is a bake");
    };
    assert_eq!(bake.graph, "study:concrete");
    assert_eq!(bake.resolution, 512);
    assert_eq!(bake.params["variation"], ParamValue::Float(0.37));
    // The value the binding said nothing about is the definition's own, which
    // is what makes an override a change of one number rather than a second
    // definition written out per instance.
    assert_eq!(bake.params["cracking"], ParamValue::Float(0.62));
    assert_eq!(dressed.base_color, [0.5, 0.6, 0.7]);

    // Nothing overridden is the definition itself, borrowed rather than cloned.
    let plain = definition
        .overridden(&Binding::new("stone"))
        .expect("a plain binding");
    assert!(matches!(plain, std::borrow::Cow::Borrowed(_)));
    assert_eq!(*plain, definition);

    // A `Shader` surface takes the same treatment; a `Plain` one takes none.
    let compiled = MaterialDefinition {
        surface: Surface::Shader {
            graph: "study:light".into(),
            params: BTreeMap::new(),
        },
        ..Default::default()
    };
    let dressed = compiled
        .overridden(&Binding::new("light").param("rate", ParamValue::Float(2.0)))
        .expect("a shader surface");
    assert_eq!(
        dressed.surface.params().expect("a graph surface")["rate"],
        ParamValue::Float(2.0)
    );
    assert!(
        MaterialDefinition::default()
            .overridden(&Binding::new("plain").param("rate", ParamValue::Float(2.0)))
            .is_none()
    );
}

#[test]
fn a_strand_setting_round_trips_and_a_definition_without_one_is_the_file_it_was() {
    // The compatibility claim in one line: a definition that grows nothing
    // writes back exactly the text it was written from, with no `strands` key
    // in it at all, so every shipped library is unchanged by this field
    // existing.
    let plain = MaterialDefinition {
        surface: Surface::Graph(bake(1024)),
        ..Default::default()
    };
    let text = ron::ser::to_string_pretty(&plain, ron::ser::PrettyConfig::default())
        .expect("a definition serialises");
    assert!(!text.contains("strands"), "{text}");
    assert_eq!(
        ron::from_str::<MaterialDefinition>(&text).expect("and reads back"),
        plain
    );

    let grown = MaterialDefinition {
        surface: Surface::Graph(bake(1024)),
        strands: Some(
            ashlar::StrandSettings::new(["blades", "stems"])
                .lod_metres([4.0, 12.0])
                .density(0.5)
                .cast_shadows(false),
        ),
        ..Default::default()
    };
    let text = ron::ser::to_string_pretty(&grown, ron::ser::PrettyConfig::default())
        .expect("a definition with strands serialises");
    assert_eq!(
        ron::from_str::<MaterialDefinition>(&text).expect("and reads back"),
        grown
    );
    // And the defaults are what a setting that says only which layers gets.
    let bare = ashlar::StrandSettings::new(["blades"]);
    assert_eq!(bare.density, 1.0);
    assert!(bare.cast_shadows);
    assert!(bare.lod_metres.is_empty());
    assert!(bare.card_metres.is_none());

    // A card distance is skipped where there is none, so a library written
    // before cards existed is still byte for byte the library it was, and it
    // round-trips where there is one.
    let text = ron::ser::to_string_pretty(&grown, ron::ser::PrettyConfig::default())
        .expect("a setting with no cards serialises");
    assert!(!text.contains("card_metres"), "{text}");
    let carded = MaterialDefinition {
        strands: Some(
            ashlar::StrandSettings::new(["blades"])
                .lod_metres([4.0, 12.0])
                .card_metres(40.0),
        ),
        ..grown.clone()
    };
    let text = ron::ser::to_string_pretty(&carded, ron::ser::PrettyConfig::default())
        .expect("a setting with cards serialises");
    assert!(text.contains("card_metres"), "{text}");
    assert_eq!(
        ron::from_str::<MaterialDefinition>(&text).expect("and reads back"),
        carded
    );
}

#[test]
fn a_strand_setting_is_refused_by_path_where_it_could_not_be_grown() {
    let grow = |surface: Surface, strands: ashlar::StrandSettings| {
        MaterialDefinition {
            surface,
            strands: Some(strands),
            ..Default::default()
        }
        .check("materials[turf]")
    };
    let blades = || ashlar::StrandSettings::new(["blades"]);

    // A surface that names no graph names no layer either, and the message says
    // which of the two surfaces that do would have worked.
    let error = grow(Surface::Plain, blades()).expect_err("a plain surface grows nothing");
    assert_eq!(error.path, "materials[turf].strands");
    assert!(error.reason.contains("names no material graph"), "{error}");
    assert!(
        grow(
            textures([Some("base.ktx2"), None, None, None, None]),
            blades()
        )
        .is_err(),
        "files alone are not a graph"
    );
    // Files that record the bake they came from are, which is what lets a
    // shipped library grow a layer at all.
    assert!(
        grow(
            Surface::Files {
                base_color: Some("base.ktx2".into()),
                normal: None,
                orm: None,
                height: None,
                emissive: None,
                baked_from: Some(bake(1024)),
            },
            blades()
        )
        .is_ok()
    );
    assert!(grow(Surface::Graph(bake(1024)), blades()).is_ok());

    // And the setting's own numbers, each at the path that carries it.
    let baked = || Surface::Graph(bake(1024));
    for (strands, path) in [
        (ashlar::StrandSettings::new([] as [&str; 0]), "layers"),
        (
            ashlar::StrandSettings::new(["blades", "blades"]),
            "layers[1]",
        ),
        (ashlar::StrandSettings::new([" "]), "layers[0]"),
        (blades().density(1.5), "density"),
        (blades().density(f32::NAN), "density"),
        // Distances grow away from the camera; a level that ends where the one
        // before it did is a band nothing is ever drawn in.
        (blades().lod_metres([12.0, 4.0]), "lod_metres[1]"),
        (blades().lod_metres([0.0]), "lod_metres[0]"),
        // A card is the level *after* the thinnest strands, so its distance is
        // past every band rather than inside one.
        (
            blades().lod_metres([4.0, 12.0]).card_metres(8.0),
            "card_metres",
        ),
        (blades().card_metres(0.0), "card_metres"),
        (blades().card_metres(f32::INFINITY), "card_metres"),
    ] {
        let error = grow(baked(), strands).expect_err("{path} is refused");
        assert_eq!(error.path, format!("materials[turf].strands.{path}"));
    }
}
