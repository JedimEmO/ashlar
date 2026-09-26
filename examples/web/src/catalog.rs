//! What the demo shows: the scenes, where their files are, and a line about
//! each material in the gallery.
//!
//! Plain data with no Bevy in it, because the content step reads the same list
//! to decide what to bake: a scene listed here and not baked is a 404 in the
//! browser, and one baked and not listed is download size nobody sees.

/// The directory under the asset root the scene materials are exported to.
pub const SCENE_MATERIALS: &str = "materials";

/// The file-backed library every scene wears, relative to the asset root.
pub const SCENE_LIBRARY: &str = "materials/library.materials.ron";

/// The directory under the asset root the gallery's maps are exported to.
pub const GALLERY_MATERIALS: &str = "gallery";

/// The file-backed library the gallery shows, relative to the asset root.
pub const GALLERY_LIBRARY: &str = "gallery/library.materials.ron";

/// The least resolution a scene material is baked at for the web, in texels
/// per repeat. A graph whose finest lattice is finer is baked at its lattice,
/// which a bake has to resolve; see [`resolution`]. A quarter of the texels of
/// the library's own 512, because a map is a download here. Most library
/// graphs lay a 512 lattice and bake at 512 regardless; the floor is what the
/// plainer ones save.
pub const SCENE_RESOLUTION: u32 = 256;

/// The least resolution the gallery's files are baked at, and that a slider
/// re-bakes at. The same number, so a slider moved and moved back is the
/// picture the page opened on.
pub const GALLERY_RESOLUTION: u32 = 256;

/// The resolution a graph bakes at under `params`: `floor`, or the graph's
/// finest lattice where that is finer, since a bake refuses to sample a
/// lattice it cannot resolve. A graph that does not build answers `floor`
/// and lets the bake say why.
#[must_use]
pub fn resolution(
    graphs: &ashlar_material::MaterialGraphLibrary,
    key: &str,
    params: &std::collections::BTreeMap<String, ashlar::ParamValue>,
    floor: u32,
) -> u32 {
    graphs
        .get(key)
        .and_then(|graph| graph.with_params(params).ok())
        .and_then(|graph| graph.build_in(graphs).ok())
        .map_or(floor, |material| {
            let [u, v] = material.finest_lattice();
            u.max(v).next_power_of_two().max(floor)
        })
}

/// One scene of the demo.
#[derive(Clone, Copy, Debug)]
pub struct DemoScene {
    /// The showcase's name for it, which is also its file name.
    pub name: &'static str,
    /// The heading in the picker.
    pub title: &'static str,
    /// One sentence under the heading.
    pub blurb: &'static str,
    /// Whether it opens by night, which is how the dark city was lit to be
    /// seen.
    pub night: bool,
    /// How far the opening view stands off, as a share of the building's
    /// diagonal. A city is framed from inside itself; a house from across the
    /// garden.
    pub distance: f32,
    /// How far above the horizon the opening view looks down, in radians.
    pub pitch: f32,
}

impl DemoScene {
    /// The baked building, relative to the asset root: a `.ashlar` file
    /// gzipped, because a static host serves it as an opaque binary and does
    /// not compress it, and a baked building is mostly repeated floats that
    /// deflate to an eighth.
    #[must_use]
    pub fn path(&self) -> String {
        format!("buildings/{}.ashlar.gz", self.name)
    }
}

/// Every scene the demo offers, in picker order. The showcase's other scenes
/// are kit sheets and test fixtures: a study bay, one facade, a sci-fi kit laid
/// out in rows.
pub const SCENES: [DemoScene; 8] = [
    DemoScene {
        name: "metropolis",
        title: "Metropolis",
        blurb: "A seeded dark city of four by four blocks: towers, back alleys, neon.",
        night: true,
        distance: 0.55,
        pitch: 0.32,
    },
    DemoScene {
        name: "city-block",
        title: "City block",
        blurb: "One block of four towers, their back alleys and its streets.",
        night: true,
        distance: 0.75,
        pitch: 0.35,
    },
    DemoScene {
        name: "city-alley",
        title: "Back alleys",
        blurb: "A block of low towers, close enough to walk between.",
        night: true,
        distance: 0.8,
        pitch: 0.3,
    },
    DemoScene {
        name: "city-landmark",
        title: "Landmark tower",
        blurb: "One hundred and ten storeys on its block.",
        night: true,
        distance: 0.7,
        pitch: 0.2,
    },
    DemoScene {
        name: "corporate-block",
        title: "Corporate courtyard",
        blurb: "Three structures of one kit around a paved forecourt.",
        night: false,
        distance: 1.5,
        pitch: 0.3,
    },
    DemoScene {
        name: "interior",
        title: "House with an interior",
        blurb: "Two storeys with rooms, floors and stairs: cut a storey to look in.",
        night: false,
        distance: 2.2,
        pitch: 0.4,
    },
    DemoScene {
        name: "scifi-colony",
        title: "Sci-fi colony",
        blurb: "A corporate colony: hab tower, pods, walkway tubes and prefab modules.",
        night: false,
        distance: 0.8,
        pitch: 0.3,
    },
    DemoScene {
        name: "scifi-outpost",
        title: "Desert outpost",
        blurb: "A walled homestead of adobe domes and vaporators.",
        night: false,
        distance: 0.8,
        pitch: 0.32,
    },
];

/// The scene called `name`.
#[must_use]
pub fn scene(name: &str) -> Option<&'static DemoScene> {
    SCENES.iter().find(|scene| scene.name == name)
}

/// A line about a library material, for the panel. The library carries no
/// prose of its own, so it lives here, next to the one page that shows it.
#[must_use]
pub fn describe(key: &str) -> &'static str {
    match key.trim_start_matches("library:") {
        "adobe" => "Hand-laid earth render over mud brick, cracked and patched.",
        "ashlar-blocks" => "Squared, dressed stone blocks in coursed rows.",
        "asphalt" => "Bitumen and aggregate, worn and patched.",
        "brick" => "Running-bond brick after ambientCG Bricks097.",
        "brick-moss-light" => "The library brick with a light growth of moss in its joints.",
        "brick-moss-heavy" => "The library brick with moss over the courses as well.",
        "ceramic-tile" => "Glazed square tiles with grout lines.",
        "clay-roof-tiles" => "Lapped clay roof tiles in overlapping courses.",
        "soi-cobblestone" => "Rounded setts after the SOI cobblestone scan.",
        "soi-cobblestone-moss-light" => "The cobblestone with moss between the setts.",
        "soi-cobblestone-moss-heavy" => "The cobblestone mostly under moss.",
        "corrugated-steel" => "Corrugated sheet steel, painted and rusting at the laps.",
        "curtain-wall" => "Glazed curtain wall with mullions and lit floors.",
        "damaged-plaster" => "Plaster fallen away to the brick behind.",
        "dark-recess" => "The near-black of a deep opening.",
        "desert-sand" => "Wind-rippled sand.",
        "emissive-strip" => "A lit strip light.",
        "formed-concrete" => "Board- and panel-formed concrete with tie holes.",
        "glass" => "Clear glazing.",
        "grass" => "A lawn's surface; in the preview it grows blades too.",
        "holo-sign" => "A holographic sign's scanlines.",
        "hull-plating" => "Riveted hull plates in bands, sci-fi.",
        "interior-panelling" => "Lit interior wall panels, sci-fi.",
        "moss-carpet" => "A dense moss carpet.",
        "office-window" => "An office window with blinds, lit or dark by its emissive.",
        "painted-boards" => "Painted timber boards, weathered at the edges.",
        "painted-metal" => "Painted steel, chipped at the edges.",
        "paving-slabs" => "Concrete paving slabs, with moss in the joints.",
        "plaster" => "Painted plaster over an uneven wall.",
        "road" => "A road surface with lane markings.",
        "rubble" => "Loose broken stone.",
        "rusted-steel" => "Steel far gone to rust.",
        "shopfront" => "A lit shop window.",
        "signage-ink" => "Printed signage.",
        "stained-concrete" => "Concrete streaked and stained by weather.",
        "steel" => "Bare brushed steel.",
        "stone-cladding" => "Stone cladding panels on a grid.",
        "tread-plate" => "Diamond tread plate.",
        "window-band" => "A band of lit windows.",
        "wood-floor" => "Wood floor boards after ambientCG WoodFloor043.",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_library_material_has_a_line_in_the_gallery() {
        let missing: Vec<String> = ashlar_material::stdlib::materials()
            .materials
            .into_keys()
            .filter(|key| describe(key).is_empty())
            .collect();
        assert!(missing.is_empty(), "no description for {missing:?}");
    }

    #[test]
    fn scene_names_are_unique_and_found_by_name() {
        for demo in &SCENES {
            assert_eq!(scene(demo.name).map(|found| found.title), Some(demo.title));
        }
    }

    #[test]
    fn a_count_that_lays_a_finer_lattice_raises_the_resolution() {
        let graphs = ashlar_material::stdlib::graphs();
        let brick = resolution(
            &graphs,
            "library:brick",
            &std::collections::BTreeMap::new(),
            256,
        );
        assert!(brick >= 256 && brick.is_power_of_two());
        assert_eq!(
            resolution(
                &graphs,
                "library:no-such-graph",
                &std::collections::BTreeMap::new(),
                256
            ),
            256
        );
    }
}
