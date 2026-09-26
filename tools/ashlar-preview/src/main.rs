//! The bundled preview binary: the showcase catalog in the generic preview tool.
//!
//! Everything specific to this content is the table below. A game writes the
//! same thing over its own buildings and never depends on `ashlar-showcase`.
use anyhow::Result;
use ashlar_preview::{Catalog, Options, Scene, run};
use ashlar_showcase as showcase;

fn catalog() -> Catalog {
    let mut catalog = Catalog::new();
    for (name, scene, display) in showcase::Scene::ALL {
        let mut entry =
            Scene::new(name, move || Ok(showcase::building(scene)?)).display_name(display);
        if scene.materials().is_some() {
            entry = entry
                .materials(move || scene.materials().unwrap_or_default())
                .graphs(move || scene.graphs().unwrap_or_default());
        }
        catalog = catalog.scene(entry);
    }
    // A sheet and a close-up of every baked material in the default library,
    // off the library itself: a material added there gets both without a
    // second list to keep in step.
    for name in showcase::sheet::names() {
        let sheet = name.clone();
        let detail = name.clone();
        let library = name.clone();
        let library_detail = name.clone();
        catalog = catalog
            .scene(
                Scene::new(format!("sheet-{name}"), move || {
                    Ok(showcase::sheet::building(&sheet)?)
                })
                .display_name(format!("Material sheet: {name}"))
                .materials(move || showcase::sheet::definitions(&library))
                .graphs(showcase::materials::graphs),
            )
            .scene(
                Scene::new(format!("detail-{name}"), move || {
                    Ok(showcase::sheet::detail(&detail)?)
                })
                .display_name(format!("Close-up: {name}"))
                .materials(move || showcase::sheet::definitions(&library_detail))
                .graphs(showcase::materials::graphs),
            );
    }
    // One scene per piece of the sci-fi kit, for looking at the piece itself.
    for name in showcase::scifi::piece_names() {
        let piece = name.clone();
        catalog = catalog.scene(
            Scene::new(format!("scifi-{name}"), move || {
                Ok(showcase::scifi::piece(&piece)?)
            })
            .display_name(format!("Sci-fi kit: {name}"))
            .materials(showcase::library::materials)
            .graphs(showcase::materials::graphs),
        );
    }
    // A tenement on its own, and one scene per part of the city kit; the
    // tower and the landmark are scenes of the showcase's own.
    catalog = catalog.scene(
        Scene::new("city-tenement", || {
            Ok(showcase::city::tower_scene(showcase::city::Kind::Tenement)?)
        })
        .display_name("City tenement")
        .materials(showcase::library::materials)
        .graphs(showcase::materials::graphs),
    );
    for name in showcase::city::piece_names() {
        let piece = name.clone();
        catalog = catalog.scene(
            Scene::new(format!("city-{name}"), move || {
                Ok(showcase::city::piece(&piece)?)
            })
            .display_name(format!("City kit: {name}"))
            .materials(showcase::library::materials)
            .graphs(showcase::materials::graphs),
        );
    }
    catalog.default_scene("outpost")
}

fn main() -> Result<()> {
    let catalog = catalog();
    let options = Options::parse_for(&catalog);
    run(&catalog, options)
}
