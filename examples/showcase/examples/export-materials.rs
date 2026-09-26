//! The material half of the content step, for reviewing a material.
//!
//! `export-materials [asset-root] [resolution] [name] [png]`
//!
//! The same `ashlar_content::Content` step `content` runs, over the showcase's
//! material library alone: KTX2 maps, strand sets and a file-backed
//! `library.materials.ron` under `<asset-root>/materials/` (default `assets`,
//! which git ignores). A resolution replaces every definition's own; a name
//! exports only the definitions keyed `*:<name>`; `png` also writes level 0 of
//! every map as a PNG, which is what the reference comparisons and the
//! `material-swatch` studio read. `just materials` runs it.
use anyhow::{Result, ensure};
use ashlar_content::Content;
use ashlar_showcase::{library, materials};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let root = args.next().unwrap_or_else(|| "assets".into());
    let resolution = args.next().map(|value| value.parse()).transpose()?;
    let only = args.next();
    let png = match args.next().as_deref() {
        None => false,
        Some("png") => true,
        Some(other) => anyhow::bail!("expected `png`, not {other}"),
    };
    ensure!(
        args.next().is_none(),
        "usage: export-materials [asset-root] [resolution] [name] [png]"
    );
    let mut definitions = library::materials();
    if let Some(only) = &only {
        definitions
            .materials
            .retain(|key, _| key.rsplit(':').next() == Some(only.as_str()));
        ensure!(!definitions.materials.is_empty(), "no material {only}");
    }
    let written = Content::new(&root)
        .resolution(resolution)
        .review_pngs(png)
        .write_materials(&materials::graphs(), &definitions, "materials")?;
    for (key, report) in &written.sets {
        eprintln!(
            "{key}: {} ops, {} buffered planes, height {:?}, {:.2?}",
            report.ops, report.buffers, report.height_range, report.elapsed
        );
        for warning in &report.warnings {
            eprintln!("{key}: warning: {warning}");
        }
    }
    Ok(())
}
