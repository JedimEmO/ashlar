//! The web demo's content step: the demo's scenes baked to `.ashlar` files,
//! the materials they wear, and the gallery's maps, sized for a download.
//!
//! `cargo run --release -p ashlar-web --features content --example content -- <asset-root> [scene-floor] [gallery-floor]`
//!
//! Writes under `<asset-root>`, which `just site` points at a staging
//! directory under `target/` — never the repository:
//!
//! - `buildings/<scene>.ashlar.gz` for every scene in [`catalog::SCENES`]: the
//!   baked building, gzipped, which the app inflates as it loads;
//! - `materials/`: the maps of every material those scenes bind and nothing
//!   else, with `library.materials.ron`;
//! - `gallery/`: every `library:*` material of the default library, with its
//!   own `library.materials.ron`.
//!
//! Each material bakes at the floor — [`catalog::SCENE_RESOLUTION`] and
//! [`catalog::GALLERY_RESOLUTION`] unless the arguments say otherwise — or at
//! its graph's finest lattice where that is finer.
//!
//! This is `examples/showcase/examples/content.rs` narrowed to what a page
//! shows. The whole showcase at each definition's own resolution is well over a
//! hundred megabytes; a visitor downloads the scene they pick.
use std::{collections::BTreeSet, io::Write, num::NonZeroUsize, time::Instant};

use anyhow::{Context, Result, bail};
use ashlar::{Building, LodPolicy, MaterialLibrary, Surface};
use ashlar_content::{Content, flatten_overrides};
use ashlar_material::MaterialGraphLibrary;
use ashlar_material::stdlib;
use ashlar_showcase::{Scene, building, library, materials};
use ashlar_web::catalog;

/// Bake threads. Four, not one per core: this machine and CI runners alike are
/// better served by a bounded bake.
const THREADS: Option<NonZeroUsize> = NonZeroUsize::new(4);

/// Bytes as megabytes, for a progress line.
#[expect(
    clippy::cast_precision_loss,
    reason = "a progress line, to one decimal"
)]
fn megabytes(bytes: usize) -> f64 {
    bytes as f64 / 1e6
}

/// Every material key a building binds, on its palette or on an instance.
fn bound(building: &Building) -> BTreeSet<String> {
    let recipe = building.recipe();
    recipe
        .materials
        .values()
        .chain(
            recipe
                .instances
                .iter()
                .flat_map(|instance| instance.materials.values()),
        )
        .map(|binding| binding.material.clone())
        .collect()
}

/// A library narrowed to `keys` and baked at `floor` or each graph's lattice,
/// with strand layers dropped: the demo draws the relief a strand layer leaves
/// in the maps and grows no blades, so a strand set would be bytes nobody
/// reads.
fn narrowed(
    library: &MaterialLibrary,
    keys: &BTreeSet<String>,
    graphs: &MaterialGraphLibrary,
    floor: u32,
) -> MaterialLibrary {
    MaterialLibrary {
        materials: library
            .materials
            .iter()
            .filter(|(key, _)| keys.contains(*key))
            .map(|(key, definition)| {
                let mut definition = definition.clone();
                definition.strands = None;
                if let Surface::Graph(bake) = &mut definition.surface {
                    bake.resolution = catalog::resolution(graphs, &bake.graph, &bake.params, floor);
                }
                (key.clone(), definition)
            })
            .collect(),
    }
}

/// Drop what the demo never reads from a written materials directory: each
/// set's height map, which reaches no material slot and whose sixteen-bit
/// format WebGL2 could not upload anyway, and the graph library, since the app
/// builds the default library's graphs itself. The file-backed library is
/// written again without the height paths, so it names no file that is not
/// there. About two fifths of the bytes.
fn trim(content: &Content, directory: &str, library: &MaterialLibrary) -> Result<MaterialLibrary> {
    let mut library = library.clone();
    for definition in library.materials.values_mut() {
        if let Surface::Files { height, .. } = &mut definition.surface
            && let Some(path) = height.take()
        {
            let on_disk = content.root().join(&path);
            if on_disk.exists() {
                std::fs::remove_file(&on_disk)
                    .with_context(|| format!("removing {}", on_disk.display()))?;
            }
        }
    }
    let graphs = content
        .root()
        .join(directory)
        .join(ashlar_content::GRAPHS_FILE);
    if graphs.exists() {
        std::fs::remove_file(&graphs)?;
    }
    let text = ron::ser::to_string_pretty(&library, ron::ser::PrettyConfig::default())?;
    content.write_file(
        &format!("{directory}/{}", ashlar_content::LIBRARY_FILE),
        text.as_bytes(),
    )?;
    Ok(library)
}

/// A resolution argument, or the default.
fn floor(argument: Option<String>, default: u32) -> Result<u32> {
    argument.map_or(Ok(default), |value| {
        value
            .parse()
            .with_context(|| format!("{value:?} is not a resolution"))
    })
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let root = args
        .next()
        .context("usage: content <asset-root> [scene-floor] [gallery-floor]")?;
    let scene_floor = floor(args.next(), catalog::SCENE_RESOLUTION)?;
    let gallery_floor = floor(args.next(), catalog::GALLERY_RESOLUTION)?;
    let started = Instant::now();

    let mut buildings = Vec::new();
    for demo in &catalog::SCENES {
        let (_, scene, _) = Scene::ALL
            .into_iter()
            .find(|(name, _, _)| *name == demo.name)
            .with_context(|| format!("the showcase has no scene {:?}", demo.name))?;
        let recipe = building(scene).with_context(|| format!("building {}", demo.name))?;
        buildings.push((demo.path(), recipe));
    }
    let recipes: Vec<Building> = buildings.iter().map(|(_, recipe)| recipe.clone()).collect();
    let (flattened, grown) = flatten_overrides(&recipes, &library::materials())?;
    let keys: BTreeSet<String> = flattened.iter().flat_map(bound).collect();
    let graphs = materials::graphs();
    let scene_library = narrowed(&grown, &keys, &graphs, scene_floor);
    // A compiled surface is a shader, and the game path draws files. The
    // study kit is the one that binds any, and it is not in the demo.
    for (key, definition) in &scene_library.materials {
        if matches!(definition.surface, Surface::Shader { .. }) {
            bail!("{key} is a compiled surface, which the web demo cannot draw");
        }
    }

    let scenes = Content::new(&root).threads(THREADS);
    let written = scenes.write_materials(&graphs, &scene_library, catalog::SCENE_MATERIALS)?;
    trim(&scenes, catalog::SCENE_MATERIALS, &written.library)?;
    eprintln!(
        "scene materials: {} definitions, {} sets, {:.1} MB before trimming",
        scene_library.materials.len(),
        written.sets.len(),
        megabytes(written.bytes)
    );
    for ((path, _), recipe) in buildings.iter().zip(&flattened) {
        // Written plain, then gzipped beside itself and the plain one removed:
        // `write_building` is the one place a building is meshed and
        // serialized, and this step only changes how it travels.
        let plain = path
            .strip_suffix(".gz")
            .with_context(|| format!("{path} is not a .gz path"))?;
        let baked = scenes
            .write_building(plain, recipe, &LodPolicy::ladder())
            .with_context(|| format!("baking {path}"))?;
        let on_disk = scenes.root().join(plain);
        let bytes = std::fs::read(&on_disk)?;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(&bytes)?;
        let gzipped = encoder.finish()?;
        scenes.write_file(path, &gzipped)?;
        std::fs::remove_file(&on_disk)?;
        eprintln!(
            "{path}: {:?} triangles by level, {:.1} MB, {:.1} MB gzipped, {:.1?}",
            baked.triangles,
            megabytes(baked.bytes),
            megabytes(gzipped.len()),
            baked.elapsed
        );
    }

    let defaults = stdlib::materials();
    let every: BTreeSet<String> = defaults.materials.keys().cloned().collect();
    let library_graphs = stdlib::graphs();
    let gallery_content = Content::new(&root).threads(THREADS);
    let gallery = gallery_content.write_materials(
        &library_graphs,
        &narrowed(&defaults, &every, &library_graphs, gallery_floor),
        catalog::GALLERY_MATERIALS,
    )?;
    trim(
        &gallery_content,
        catalog::GALLERY_MATERIALS,
        &gallery.library,
    )?;
    eprintln!(
        "gallery: {} sets, {:.1} MB before trimming",
        gallery.sets.len(),
        megabytes(gallery.bytes)
    );
    eprintln!("web content step: {:.1?}", started.elapsed());
    Ok(())
}
