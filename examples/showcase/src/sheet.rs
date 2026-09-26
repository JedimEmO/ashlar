//! The material sheet: every material in the default library, on a ball and a
//! cube, at three texel densities, baked beside live.
//!
//! A material is judged by looking at it, and the thing it has to survive is
//! not one look but the whole range of them: close enough to see a mortar joint
//! and far enough that the joint is a third of a texel. A sheet is where that
//! is checked deliberately rather than noticed on a wall three commits later.
//!
//! # Why the three distances are three tilings
//!
//! The gallery frames a scene to its own bounds and stands the camera off by a
//! fixed multiple of the diagonal, so a row of specimens laid out in depth can
//! only ever span about two to one in real distance — the row *is* the bounds,
//! and the camera backs off as the row gets longer. Two to one is one mip
//! level, which is not a test of anything.
//!
//! What a renderer actually selects a mip level from is texels per screen
//! pixel, and that is `resolution * distance / tile_metres`. Dividing the
//! tiling by three is exactly the same number as standing three times further
//! away, and it leaves the specimen at a size somebody can look at instead of
//! shrinking it to a dozen pixels. So the three column pairs of a sheet are the
//! same material at one, three and nine times the distance, drawn the same size.
//! [`DISTANCES`] is that factor, and the only thing the fiction costs is that
//! the far pair's bricks are physically small rather than far away, which no
//! capture can tell apart.
//!
//! # The ball
//!
//! There is no sphere in [`ashlar::Geometry`] and this is not the place to add
//! one. The round specimen is a chamfered cube at a bevel close to the largest
//! the kernel allows, which is the convex hull of twenty-four points: six
//! square faces, eight triangles and twelve rectangles, at forty-five degrees
//! to one another. The mesher creases every edge at that angle, so it is a
//! faceted ball rather than a smooth one — twenty-six flat faces catching the
//! light at twenty-six angles, which is what the specimen is for.
//!
//! # Baked beside live
//!
//! Every column is a pair, and the two halves are the same material delivered
//! two ways: on the left the graph baked when the material is registered, on
//! the right the graph itself, partitioned and compiled. A graph with no live parameter should be
//! the same picture both ways — the compiler even says so, in the cost report's
//! own words, `no live inputs: bake this` — and the point of putting the two
//! touching is that "should" is then a claim somebody can check by looking. The
//! conformance test checks it per op on the GPU; this checks it on a lit
//! surface, which is where a difference in encoding, in the mip chain or in the
//! sampler would show and an arithmetic comparison would not.
//!
//! One difference is expected rather than a bug, and the far pair is where to
//! look for it: the compiled half binds occlusion, roughness and metallic as
//! three images where the bake packs them into one ORM, so the two quantise
//! independently.
//!
//! A second one used to stand here — a compiled material's roughness did not
//! widen with distance the way a baked one's does — and is closed. The
//! coherence `|n_avg|` that `mips::encode_mips` widens a bake's chain by now
//! rides in the alpha of the bound normal map's own chain, and the fragment
//! applies the same Toksvig term to whatever level the hardware chose. So the
//! far pair is a comparison again rather than a known difference, and a
//! specimen that turns to glass at nine times the distance while its twin stays
//! matt is a bug to be reported.
//!
//! The halves are laid out left to right rather than stacked because the
//! comparison is the point, and the eye is very good at two things touching and
//! very bad at two things a screen apart. What it costs is size: twelve
//! specimens in a frame the gallery scales to their own bounds, so each is
//! about half the width it was when the sheet was three columns of two.
use ashlar::{
    Building, Collision, Element, Geometry, Instance, MaterialDefinition, MaterialLibrary, Part,
    Pose, Surface, ValidationError,
};

/// The namespace of every material key a sheet binds.
const PREFIX: &str = "sheet";

/// Texels per repeat a sheet bakes at: a millimetre a texel over the two-metre
/// repeat most of the library is drawn at, which is the density the library's
/// materials were reviewed at, and fine enough for a strand relief to splat.
pub(crate) const RESOLUTION: u32 = 2048;

/// The library materials a sheet can be made of, by name: every
/// `library:<name>` definition whose surface is baked.
#[must_use]
pub fn names() -> Vec<String> {
    ashlar_material::stdlib::materials()
        .materials
        .into_iter()
        .filter(|(_, definition)| matches!(definition.surface, Surface::Graph(_)))
        .filter_map(|(key, _)| key.strip_prefix("library:").map(str::to_owned))
        .collect()
}

/// The three column pairs: a name, and how many times further away the pair
/// stands for.
///
/// One, three and nine, which is three mip levels and a bit over — far enough
/// apart that a surface which is going to turn to glitter or to mush has done
/// it by the third pair.
pub const DISTANCES: [(&str, f32); 3] = [("near", 1.0), ("mid", 3.0), ("far", 9.0)];

/// The two halves of a pair: a name, and whether that half is the compiled one.
///
/// The name is part of the material key and of the instance id, so it is what a
/// capture, a test failure and a click in the preview all say.
pub(crate) const DELIVERY: [(&str, bool); 2] = [("baked", false), ("live", true)];

/// Metres across each specimen, the gap inside a pair, and the gap between
/// pairs.
///
/// The inner gap is small and the outer one is not, so that which two columns
/// belong together is visible before anything is read.
const SPECIMEN: f64 = 1.5;
const SPACING: f64 = 0.2;
const PAIR_GAP: f64 = 0.85;

/// The bevel of the ball, as a fraction of its extent.
///
/// The bevel at which every face of the hull is the same size — `1 - 2b` for
/// the squares against `b * sqrt(2)` for the rectangles between them, which
/// meet at `b = 1 / (2 + sqrt(2))`. That is the roundest this construction
/// gets; the half the kernel refuses would collapse it to an octahedron
/// instead, which is a gem and not a ball.
const BALL_BEVEL: f64 = 0.292_893_2;

/// The material key one column of a sheet binds.
#[must_use]
pub fn material_key(name: &str, distance: &str, delivery: &str) -> String {
    format!("{PREFIX}:{name}-{distance}-{delivery}")
}

/// One material's six definitions: the library's own definition at the three
/// tilings [`DISTANCES`] names, each delivered both ways `DELIVERY` names.
///
/// The baked half is the library's `Surface::Graph` at `RESOLUTION`; the live
/// half names the same graph at the same parameters under a
/// [`Surface::Shader`], so the pair is a like-for-like comparison and not two
/// different materials. A material that grows strands grows them on the near
/// column only: the mid and far columns stand for the same lawn at three and
/// nine times the distance, which is exactly where a strand layer's level of
/// detail has already given way to the relief.
///
/// # Panics
///
/// If `name` is not one of [`names`].
#[must_use]
pub fn definitions(name: &str) -> MaterialLibrary {
    let key = format!("library:{name}");
    let library_definition = ashlar_material::stdlib::materials()
        .materials
        .remove(&key)
        .unwrap_or_else(|| panic!("no library material {key}"));
    let Surface::Graph(bake) = &library_definition.surface else {
        panic!("{key} is not baked");
    };
    let mut library = MaterialLibrary::default();
    for (distance, factor) in DISTANCES {
        for (delivery, compiled) in DELIVERY {
            let surface = if compiled {
                Surface::Shader {
                    graph: bake.graph.clone(),
                    params: bake.params.clone(),
                }
            } else {
                Surface::Graph(ashlar::Bake {
                    resolution: RESOLUTION,
                    ..bake.clone()
                })
            };
            library.materials.insert(
                material_key(name, distance, delivery),
                MaterialDefinition {
                    tile_metres: library_definition.tile_metres.map(|metres| metres / factor),
                    // The near column tiles at the graph's own declared repeat
                    // and needs no `tile_scale`; the mid and far columns divide
                    // it by three and by nine on purpose, to fake distance
                    // without moving the camera, and say so rather than reading
                    // as a typo. `>` rather than `!=` on a float.
                    tile_scale: (factor > 1.0).then_some(factor.recip()),
                    strands: library_definition
                        .strands
                        .clone()
                        .filter(|_| distance == DISTANCES[0].0),
                    surface,
                    ..library_definition.clone()
                },
            );
        }
    }
    library
}

fn specimen(id: &str, geometry: Geometry) -> Part {
    Part::builder(id)
        .element(Element::new("body", geometry, "surface").collision(Collision::Bounds))
        .build()
        .expect("a specimen is one validated primitive")
}

/// One sheet: a ball over a cube, six times across — three distances, each
/// baked and live.
///
/// # Errors
///
/// If the assembled recipe does not validate, which would be a bug here rather
/// than something content can cause.
pub fn building(name: &str) -> Result<Building, ValidationError> {
    let size = [SPECIMEN; 3];
    let mut builder = Building::builder(format!("sheet:{name}"))
        .part(specimen("sheet:cube", Geometry::cuboid(size)))
        .part(specimen(
            "sheet:ball",
            Geometry::chamfered_cuboid(size, SPECIMEN * BALL_BEVEL),
        ))
        // A slot the instances all override; a building needs a binding for
        // every slot its parts declare, and this is the near baked one.
        .material("surface", material_key(name, DISTANCES[0].0, DELIVERY[0].0));
    let mut x = 0.0;
    for (index, (distance, _)) in DISTANCES.into_iter().enumerate() {
        if index > 0 {
            x += PAIR_GAP - SPACING;
        }
        for (delivery, _) in DELIVERY {
            for (shape, part, y) in [
                ("cube", "sheet:cube", 0.0),
                ("ball", "sheet:ball", SPECIMEN + 0.3),
            ] {
                builder = builder.instance(
                    Instance::new(format!("{distance}-{delivery}-{shape}"), part)
                        .placed(Pose::at([x, y, 0.0]))
                        .material("surface", material_key(name, distance, delivery)),
                );
            }
            x += SPECIMEN + SPACING;
        }
    }
    builder.build()
}

/// One full-size vertical patch of a material, two metres square, for close
/// inspection. It binds the near, baked column of the material's sheet, so
/// [`definitions`] is its library too.
///
/// # Errors
///
/// If the assembled recipe does not validate.
pub fn detail(name: &str) -> Result<Building, ValidationError> {
    Building::builder(format!("sheet:detail-{name}"))
        .part(
            Part::builder("sheet:detail")
                .element(Element::new(
                    "surface",
                    Geometry::cuboid([2.0, 2.0, 0.06]),
                    "surface",
                ))
                .build()?,
        )
        .instance(Instance::new("patch", "sheet:detail"))
        .material("surface", material_key(name, DISTANCES[0].0, DELIVERY[0].0))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_baked_library_material_has_a_sheet_and_every_cell_names_its_own_tiling() {
        let graphs = crate::materials::graphs();
        let names = names();
        assert!(names.iter().any(|name| name == "brick"));
        assert!(names.iter().any(|name| name == "grass"));
        for name in &names {
            let building = building(name).expect("a sheet builds");
            let library = definitions(name);
            library
                .check_for(&building)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            detail(name)
                .and_then(|detail| library.check_for(&detail))
                .unwrap_or_else(|error| panic!("{name} detail: {error}"));
            assert_eq!(library.materials.len(), DISTANCES.len() * DELIVERY.len());
            assert_eq!(
                building.recipe().instances.len(),
                2 * DISTANCES.len() * DELIVERY.len()
            );
            let library_definition =
                &ashlar_material::stdlib::materials().materials[&format!("library:{name}")];
            for (distance, factor) in DISTANCES {
                for (delivery, compiled) in DELIVERY {
                    let key = material_key(name, distance, delivery);
                    let definition = &library.materials[&key];
                    for axis in 0..2 {
                        let expected = library_definition.tile_metres[axis] / factor;
                        assert!((definition.tile_metres[axis] - expected).abs() < 1e-6);
                    }
                    // The two halves are the same material two ways: the same
                    // graph at the same parameters, and only the delivery
                    // different.
                    match (&definition.surface, &library_definition.surface) {
                        (Surface::Shader { graph, params }, Surface::Graph(bake)) => {
                            assert!(compiled, "{key} compiles a graph it should have baked");
                            assert_eq!(graph, &bake.graph);
                            assert_eq!(params, &bake.params);
                            assert!(graphs.get(graph).is_some());
                        }
                        (Surface::Graph(sheet), Surface::Graph(bake)) => {
                            assert!(!compiled, "{key} bakes what it should have compiled");
                            assert_eq!(sheet.graph, bake.graph);
                            assert_eq!(sheet.resolution, RESOLUTION);
                        }
                        other => panic!("{key} is a {other:?}"),
                    }
                    assert_eq!(
                        definition.strands.is_some(),
                        library_definition.strands.is_some() && distance == "near",
                        "{key}"
                    );
                    for shape in ["cube", "ball"] {
                        assert_eq!(
                            building.material(&format!("{distance}-{delivery}-{shape}"), "surface"),
                            Some(key.as_str())
                        );
                    }
                }
            }
        }
    }

    /// Inside a pair the two specimens touch; between pairs they do not.
    ///
    /// The layout is what makes the comparison readable, so it is pinned rather
    /// than left to whoever next edits a constant.
    #[test]
    fn a_pair_stands_closer_together_than_two_pairs_do() {
        let building = building("formed-concrete").expect("a sheet builds");
        let at = |id: &str| {
            building
                .recipe()
                .instances
                .iter()
                .find(|instance| instance.id == id)
                .unwrap_or_else(|| panic!("no instance {id}"))
                .pose
                .translation
                .x
        };
        let inside = at("near-live-cube") - at("near-baked-cube");
        let between = at("mid-baked-cube") - at("near-live-cube");
        assert!((inside - (SPECIMEN + SPACING)).abs() < 1e-9, "{inside}");
        assert!((between - (SPECIMEN + PAIR_GAP)).abs() < 1e-9, "{between}");
        assert!(between > inside, "a pair must read as a pair");
    }
}
