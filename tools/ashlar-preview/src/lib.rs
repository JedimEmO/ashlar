//! A preview workbench for [`ashlar`] building recipes, with no content of its own.
//!
//! The tool is a library first so a game can point the same viewer and the same
//! reference gallery at its own buildings. Register a [`Catalog`] of named
//! scenes, parse [`Options`] against it and call [`run`]:
//!
//! ```no_run
//! use ashlar_preview::{Catalog, Options, Scene, run};
//!
//! # fn my_tower() -> anyhow::Result<ashlar::Building> { unimplemented!() }
//! fn main() -> anyhow::Result<()> {
//!     let catalog = Catalog::new()
//!         .scene(Scene::new("tower", || my_tower()).material_library("buildings/city.ron"))
//!         .default_scene("tower");
//!     run(&catalog, Options::parse_for(&catalog))
//! }
//! ```
//!
//! The gallery (`--references <dir>`) writes one PNG per scene and angle plus an
//! `index.html` and a `manifest.tsv`. It is for looking at, not for asserting on:
//! there is no pixel baseline comparison.
mod blast;
mod catalog;
mod options;
mod panel;
mod references;
mod reload;
mod viewer;

pub use catalog::{Catalog, Scene};
pub use options::Options;

use ashlar::MaterialLibrary;
use ashlar_material::MaterialGraphLibrary;
use bevy::prelude::Resource;

/// The scene's material library, preflighted, or `None` for diagnostic colours.
#[derive(Resource, Default)]
pub(crate) struct Definitions(pub(crate) Option<MaterialLibrary>);

/// The material graph library beside [`Definitions`], validated.
///
/// A `Surface::Graph` names a key in here, so whatever creates materials needs
/// it as well as the definitions; the default is the empty library a set of
/// file and constant surfaces preflights against.
#[derive(Resource, Default)]
pub(crate) struct Graphs(pub(crate) MaterialGraphLibrary);

/// Run the preview: the reference gallery if `--references` was given, the
/// interactive viewer or a one-shot export otherwise.
pub fn run(catalog: &Catalog, options: Options) -> anyhow::Result<()> {
    if options.references.is_some() {
        return references::run(catalog, &options);
    }
    viewer::run(catalog, options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ashlar::{Building, Element, Geometry, Instance, Part};

    fn cube(id: &str) -> anyhow::Result<Building> {
        let part = Part::builder("test:block")
            .element(Element::new(
                "body",
                Geometry::cuboid([1.0, 1.0, 1.0]),
                "shell",
            ))
            .build()?;
        Ok(Building::builder(id)
            .part(part)
            .material("shell", "test:grey")
            .instance(Instance::new("only", "test:block"))
            .build()?)
    }

    /// A content-free catalog, so the command-line tests do not need the showcase.
    pub(crate) fn catalog() -> Catalog {
        Catalog::new()
            .scene(Scene::new("one", || cube("test:one")))
            .scene(Scene::new("two", || cube("test:two")).display_name("Two"))
            .default_scene("one")
    }

    #[test]
    fn a_catalog_resolves_names_and_reports_the_alternatives() {
        let catalog = catalog();
        assert_eq!(catalog.default_name(), Some("one"));
        assert_eq!(catalog.names().collect::<Vec<_>>(), ["one", "two"]);
        assert_eq!(catalog.get("two").expect("scene").display(), "Two");
        let error = catalog
            .require("three")
            .err()
            .expect("unknown scene")
            .to_string();
        assert!(error.contains("one, two"), "{error}");
        assert!(catalog.require("one").expect("scene").build().is_ok());
    }

    #[test]
    fn a_catalog_can_move_the_default_asset_root_to_its_own_game() {
        let catalog = catalog().asset_root("/game/assets");
        let options =
            Options::try_parse_for(&catalog, ["preview"]).expect("the catalog's own default");
        assert_eq!(options.asset_root, std::path::Path::new("/game/assets"));
        let options = Options::try_parse_for(&catalog, ["preview", "--asset-root", "/elsewhere"])
            .expect("an explicit root still wins");
        assert_eq!(options.asset_root, std::path::Path::new("/elsewhere"));
    }
}
