//! The showcase's content step: every scene baked to a `.ashlar` building at
//! every level of detail, and the material library exported beside them.
//!
//! `content [asset-root] [resolution] [scene...]`
//!
//! Writes under `<asset-root>` (default `assets`, which git ignores):
//! `buildings/<scene>.ashlar` for each scene named, or all of them, and
//! `materials/` with the KTX2 maps, strand sets and `library.materials.ron`.
//! `ashlar-bevy`'s `baked` example loads what this writes.
use std::time::Instant;

use anyhow::{Context, Result};
use ashlar::LodPolicy;
use ashlar_content::Content;
use ashlar_showcase::{Scene, building, library, materials};

/// Bytes as megabytes, for a progress line.
#[expect(
    clippy::cast_precision_loss,
    reason = "a progress line, to one decimal"
)]
fn megabytes(bytes: usize) -> f64 {
    bytes as f64 / 1e6
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let root = args.next().unwrap_or_else(|| "assets".into());
    let resolution = args
        .next()
        .map(|value| value.parse::<u32>())
        .transpose()
        .context("the second argument is a resolution")?;
    let wanted: Vec<String> = args.collect();
    let content = Content::new(&root).resolution(resolution);

    let buildings = Scene::ALL
        .into_iter()
        // The primitive fixture binds no library and draws in diagnostic
        // colours, so there is nothing for a game to ship of it.
        .filter(|(_, scene, _)| scene.materials().is_some())
        .filter(|(name, _, _)| wanted.is_empty() || wanted.iter().any(|wanted| wanted == name))
        .map(|(name, scene, _)| {
            building(scene)
                .map(|recipe| (format!("buildings/{name}.ashlar"), recipe))
                .with_context(|| format!("building {name}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let started = Instant::now();
    let shipped = content.ship(
        &materials::graphs(),
        &library::materials(),
        "materials",
        &buildings,
        &LodPolicy::ladder(),
    )?;
    eprintln!(
        "materials: {} sets, {:.1} MB",
        shipped.materials.sets.len(),
        megabytes(shipped.materials.bytes)
    );
    for (path, baked) in &shipped.buildings {
        eprintln!(
            "{path}: {:?} triangles by level, {:.1} MB, {:.2?}",
            baked.triangles,
            megabytes(baked.bytes),
            baked.elapsed
        );
    }
    eprintln!("content step: {:.1?}", started.elapsed());
    Ok(())
}
