//! The showcase's material library: the default library's definitions, and
//! the few that are particular to the showcase's own scenes.
//!
//! Everything here is Rust, as the graphs are. A scene hands this library and
//! [`crate::materials::graphs`] to the renderer as they are, and every
//! `Surface::Graph` in it is baked when the material is registered; the
//! content step (`ashlar_content::Content`, run by the `content` example) turns
//! the same library into files.
use ashlar::{MaterialDefinition, MaterialLibrary, ParamValue, Surface};
use std::collections::BTreeMap;

/// The default surfaces, the four compiled demonstrations, the smooth interior
/// plaster, the corporate kit's customizations and the dark city's.
pub fn materials() -> MaterialLibrary {
    let mut library = ashlar_material::stdlib::materials();
    for (key, base, graph) in [
        (
            "showcase:concrete-wet",
            "library:formed-concrete",
            "showcase:concrete-wet",
        ),
        (
            "showcase:concrete-cut-aware",
            "library:formed-concrete",
            "showcase:concrete-cut-aware",
        ),
        (
            "showcase:brick-weathered",
            "library:brick",
            "showcase:brick-weathered",
        ),
        ("showcase:light", "library:emissive-strip", "showcase:strip"),
    ] {
        let mut definition = library.materials[base].clone();
        definition.surface = Surface::Shader {
            graph: graph.into(),
            params: if key == "showcase:concrete-wet" {
                BTreeMap::from([(
                    "wetness".into(),
                    ParamValue::Float(crate::materials::CONCRETE_WETNESS),
                )])
            } else {
                BTreeMap::new()
            },
        };
        if key == "showcase:light" {
            definition.tile_metres = crate::materials::STRIP_TILE_METRES;
        }
        library.materials.insert(key.into(), definition);
    }
    let mut smooth = library.materials["library:plaster"].clone();
    if let Surface::Graph(bake) = &mut smooth.surface {
        bake.params
            .insert("smoothness".into(), ParamValue::Float(1.0));
    }
    library
        .materials
        .insert("showcase:interior-plaster".into(), smooth);
    library.materials.extend(corporate_materials().materials);
    // A clipped hedge or a tree's crown: the lawn's surface without the
    // strands, which a crown seen from the street has no use for.
    let mut hedge = library.materials["library:grass"].clone();
    hedge.strands = None;
    library.materials.insert("showcase:hedge".into(), hedge);
    library.materials.extend(dark_city_materials().materials);
    library
}

/// Corporate customizations: a repeat that closes across 3.8 m storeys and
/// two lit facade accents. All textured surfaces instance the default graphs.
pub fn corporate_materials() -> MaterialLibrary {
    let defaults = ashlar_material::stdlib::materials();
    let mut materials = BTreeMap::new();
    for (key, base) in [
        ("showcase:corporate-stone", "library:stone-cladding"),
        ("showcase:corporate-concrete", "library:formed-concrete"),
    ] {
        let mut definition = defaults.materials[base].clone();
        definition.tile_scale = Some(1.9 / definition.tile_metres[0]);
        definition.tile_metres = [1.9; 2];
        materials.insert(key.into(), definition);
    }
    let mut occupied = defaults.materials["library:glass"].clone();
    occupied.emissive = [0.06, 0.1, 0.12];
    materials.insert("showcase:occupied-glass".into(), occupied);
    materials.insert(
        "showcase:signal".into(),
        MaterialDefinition {
            base_color: [0.55, 0.24, 0.045],
            roughness: 0.55,
            emissive: [0.8, 0.3, 0.05],
            ..MaterialDefinition::default()
        },
    );
    MaterialLibrary { materials }
}

/// The dark city's surfaces: stained and dark concrete, wet streets, neon,
/// windows lit from inside and sodium street light.
///
/// Every textured one instances a default graph with a handful of bindings,
/// because each distinct binding is its own bake; the lit ones differ only in
/// the definition's `emissive`, which costs nothing.
#[expect(clippy::too_many_lines, reason = "one library, key by key")]
pub fn dark_city_materials() -> MaterialLibrary {
    let defaults = ashlar_material::stdlib::materials();
    let bound = |base: &str, params: &[(&str, ParamValue)]| {
        let mut definition = defaults.materials[base].clone();
        if let Surface::Graph(bake) = &mut definition.surface {
            for (name, value) in params {
                bake.params.insert((*name).into(), *value);
            }
        }
        definition
    };
    let mut materials = BTreeMap::new();
    let concrete = |color: [f32; 3]| {
        let mut definition = bound(
            "library:formed-concrete",
            &[("color", ParamValue::Color(color))],
        );
        definition.tile_scale = Some(1.9 / definition.tile_metres[0]);
        definition.tile_metres = [1.9; 2];
        definition
    };
    materials.insert(
        "showcase:stained-concrete".into(),
        defaults.materials["library:stained-concrete"].clone(),
    );
    materials.insert(
        "showcase:dark-concrete".into(),
        concrete([0.024, 0.024, 0.026]),
    );
    // Rain on everything underfoot: the same three ground graphs, wet.
    let wet = ("wet", ParamValue::Float(1.0));
    materials.insert(
        "showcase:dark-paving".into(),
        bound(
            "library:paving-slabs",
            &[
                ("moss", ParamValue::Float(0.0)),
                ("tint", ParamValue::Color([0.2, 0.24, 0.5])),
                wet,
            ],
        ),
    );
    // Wet asphalt is far darker than dry: the definition's multiplier takes
    // the rest of the way the rain's own darkening does not.
    for (key, base) in [
        ("showcase:wet-road", "library:road"),
        ("showcase:wet-asphalt", "library:asphalt"),
    ] {
        let mut definition = bound(base, &[wet]);
        definition.base_color = [0.7; 3];
        materials.insert(key.into(), definition);
    }
    // A tenement's wall: the same stained concrete, warmer, with every
    // streak and all the grime at its foot.
    materials.insert(
        "showcase:alley-wall".into(),
        bound(
            "library:stained-concrete",
            &[
                ("color", ParamValue::Color([0.03, 0.027, 0.024])),
                ("streaks", ParamValue::Float(1.0)),
                ("grime", ParamValue::Float(1.0)),
            ],
        ),
    );
    materials.insert(
        "showcase:dark-metal".into(),
        bound(
            "library:painted-metal",
            &[
                ("color", ParamValue::Color([0.03, 0.035, 0.04])),
                ("wear", ParamValue::Float(0.4)),
            ],
        ),
    );
    for (key, emissive) in [
        ("showcase:neon-magenta", [8.0, 0.5, 5.0]),
        ("showcase:neon-cyan", [0.4, 6.0, 8.0]),
        ("showcase:neon-amber", [8.0, 3.2, 0.4]),
        ("showcase:neon-green", [1.0, 8.0, 2.0]),
    ] {
        let mut neon = defaults.materials["library:emissive-strip"].clone();
        neon.base_color = [0.12; 3];
        neon.emissive = emissive;
        materials.insert(key.into(), neon);
    }
    // Every pane is one office bake: dark ones with the faintest light left
    // on behind them, lit ones in three colours, each shifted a quarter repeat
    // so a warm storey's blinds are not a cold one's.
    for (key, emissive, shift) in [
        ("showcase:window-dark", [0.012, 0.014, 0.02], 0.0),
        ("showcase:window-dim", [0.16, 0.18, 0.24], 0.25),
        ("showcase:window-warm", [0.95, 0.52, 0.2], 0.5),
        ("showcase:window-cold", [0.42, 0.66, 1.0], 0.75),
    ] {
        let mut window = defaults.materials["library:office-window"].clone();
        window.emissive = emissive;
        window.uv_offset = [shift, 0.0];
        materials.insert(key.into(), window);
    }
    // A lobby's shop, lit warm; the colours are in the bake.
    let mut shop = defaults.materials["library:shopfront"].clone();
    shop.emissive = [1.0, 0.9, 0.78];
    materials.insert("showcase:shopfront".into(), shop);
    materials.insert(
        "showcase:sodium".into(),
        MaterialDefinition {
            base_color: [0.6, 0.36, 0.1],
            roughness: 0.5,
            emissive: [10.0, 4.2, 0.8],
            ..MaterialDefinition::default()
        },
    );
    for (key, color) in [
        ("showcase:holo-magenta", [1.0, 0.08, 0.62]),
        ("showcase:holo-cyan", [0.08, 0.72, 1.0]),
    ] {
        materials.insert(
            key.into(),
            bound("library:holo-sign", &[("color", ParamValue::Color(color))]),
        );
    }
    MaterialLibrary { materials }
}
