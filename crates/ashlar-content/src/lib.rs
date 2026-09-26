//! The content step: bake a game's buildings and materials into the files it
//! ships.
//!
//! A game at play links `ashlar`, `ashlar-surface` and the default-feature
//! `ashlar-bevy`, and loads files. Something has to write those files, and it
//! is the only place the geometry kernel and the material graph engine are
//! needed: a small binary in the game's own repository, run when content
//! changes, that calls this crate. Its outputs are the three kinds of file the
//! game path reads:
//!
//! - `<directory>/<namespace>/<name>/*.ktx2` and `set.strands`: every baked
//!   material's maps, with their mip chains, and its strand set;
//! - `<directory>/library.materials.ron`: the material library naming those
//!   files, which `ashlar_bevy::prelude::AshlarPlugin` loads as an asset;
//! - `<path>.ashlar`: a building meshed at every level of detail, which the
//!   same plugin loads and spawns.
//!
//! ```no_run
//! use ashlar::LodPolicy;
//! use ashlar_content::Content;
//! use ashlar_material::stdlib;
//!
//! # fn my_buildings() -> Vec<(String, ashlar::Building)> { Vec::new() }
//! let content = Content::new("assets");
//! let materials = content.write_materials(&stdlib::graphs(), &stdlib::materials(), "materials")?;
//! println!("{} material sets", materials.sets.len());
//! for (name, building) in my_buildings() {
//!     let baked = content.write_building(&format!("buildings/{name}.ashlar"), &building, &LodPolicy::ladder())?;
//!     println!("{name}: {:?} triangles by level", baked.triangles);
//! }
//! # Ok::<(), ashlar_content::ContentError>(())
//! ```
//!
//! # Per-instance overrides
//!
//! A building may bind a slot with graph parameters of its own — a hull colour
//! per faction, a stone variation per building — and a file cannot take a
//! parameter at runtime. [`flatten_overrides`] resolves every distinct override
//! into a definition of its own before anything bakes, and rewrites the
//! buildings to bind those plainly; [`Content::ship`] does that, the
//! materials and the buildings in one call, in that order.
//!
//! Every write goes through a temporary file and a rename, so a game reading
//! the asset directory while the step runs never sees half a file.
//!
//! # Looking at the maps
//!
//! A KTX2 file is what a game should load and not what an image viewer opens.
//! [`Content::review_pngs`] also writes level 0 of every map as a PNG beside
//! it, for reviewing a material flat or comparing it with a reference. The
//! library names the KTX2 files either way, so the PNGs are for people.
use std::{
    io::Write,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use ashlar::{
    BakedError, Binding, Building, LodPolicy, MaterialLibrary, MeshError, ValidationError,
};
use ashlar_manifold::ManifoldMesher;
use ashlar_material::{
    MaterialGraphLibrary,
    bake::{Backend, BakeReport, Encoded, PlaneFormat, bake_with_report, linear_to_srgb},
    export::{ExportError, ExportRequest, export, maps},
    ktx2::{Ktx2Error, Supercompression},
};

/// The file name of the exported material library, inside the materials
/// directory. The two-part extension is what `ashlar-bevy`'s loader matches.
pub const LIBRARY_FILE: &str = "library.materials.ron";

/// The file name of the graph library the maps were baked from, inside the
/// materials directory: what a tool re-bakes from, and what a reviewer reads.
pub const GRAPHS_FILE: &str = "graphs.ron";

/// Why the content step failed.
#[derive(Debug, thiserror::Error)]
pub enum ContentError {
    /// A material would not bake or export.
    #[error(transparent)]
    Export(#[from] ExportError),
    /// A map would not encode.
    #[error(transparent)]
    Ktx2(#[from] Ktx2Error),
    /// A building would not mesh.
    #[error(transparent)]
    Mesh(#[from] MeshError),
    /// A baked building would not serialize.
    #[error(transparent)]
    Baked(#[from] BakedError),
    /// A library would not serialize.
    #[error("serializing {what}: {message}")]
    Serialize {
        /// Which file.
        what: String,
        /// Why.
        message: String,
    },
    /// A building binds something the library cannot dress, or a rewritten
    /// building no longer validates.
    #[error(transparent)]
    Binding(#[from] ValidationError),
    /// One building of [`Content::ship`] failed; the path it was going to.
    #[error("{path}: {source}")]
    Building {
        /// The building's path under the root.
        path: String,
        /// Why.
        source: Box<ContentError>,
    },
    /// A path to write is absolute or climbs out of the root.
    #[error("{0:?} is not a path under the asset root")]
    OutsideRoot(String),
    /// A review PNG would not encode.
    #[error("encoding {}: {message}", path.display())]
    Png {
        /// The file.
        path: PathBuf,
        /// Why.
        message: String,
    },
    /// A file would not write.
    #[error("writing {}: {source}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
}

/// The content step over one asset root.
#[derive(Clone, Debug)]
pub struct Content {
    root: PathBuf,
    threads: Option<NonZeroUsize>,
    resolution: Option<u32>,
    supercompression: Supercompression,
    review_pngs: bool,
}

/// What [`Content::write_materials`] wrote.
#[derive(Debug)]
pub struct MaterialsWritten {
    /// The library it wrote, naming the files; every `Surface::Graph` of the
    /// input is a `Surface::Files` here, recording what it was baked from.
    pub library: MaterialLibrary,
    /// One report per distinct bake: the first definition that asked for it,
    /// and the bake's own report.
    pub sets: Vec<(String, BakeReport)>,
    /// Bytes written, maps and strand sets and both libraries.
    pub bytes: usize,
}

/// What [`Content::ship`] wrote.
#[derive(Debug)]
pub struct Shipped {
    /// The materials.
    pub materials: MaterialsWritten,
    /// Each building by path.
    pub buildings: Vec<(String, BuildingWritten)>,
}

/// What [`Content::write_building`] wrote.
#[derive(Debug)]
pub struct BuildingWritten {
    /// Unique triangles per level, level zero first.
    pub triangles: Vec<usize>,
    /// Bytes written.
    pub bytes: usize,
    /// How long the meshing took.
    pub elapsed: Duration,
}

impl Content {
    /// The content step writing under `root`, the directory a game's asset
    /// server reads. Eight bake threads, each definition's own resolution, and
    /// zstd-supercompressed maps.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            threads: NonZeroUsize::new(8),
            resolution: None,
            supercompression: Supercompression::Zstd,
            review_pngs: false,
        }
    }

    /// Bake with this many threads; `None` is one per core.
    #[must_use]
    pub fn threads(mut self, threads: Option<NonZeroUsize>) -> Self {
        self.threads = threads;
        self
    }

    /// Bake every material at this resolution instead of its definition's own:
    /// a lower one for a quick iteration, a higher one for a release build.
    #[must_use]
    pub fn resolution(mut self, resolution: Option<u32>) -> Self {
        self.resolution = resolution;
        self
    }

    /// Also write level 0 of every map as `<map>.png` beside its KTX2: 8-bit
    /// maps as they are, the 16-bit height as 16-bit grey, and the half-float
    /// emissive through the sRGB transfer, clamped at one.
    #[must_use]
    pub fn review_pngs(mut self, review_pngs: bool) -> Self {
        self.review_pngs = review_pngs;
        self
    }

    /// The asset root this writes under.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Bake every `Surface::Graph` definition into maps and strand sets under
    /// `directory`, and write the file-backed library and the graph library
    /// beside them. Definitions that are not baked are written through as
    /// they are.
    ///
    /// # Errors
    ///
    /// The first definition that does not bake, or the first file that does
    /// not write.
    pub fn write_materials(
        &self,
        graphs: &MaterialGraphLibrary,
        definitions: &MaterialLibrary,
        directory: &str,
    ) -> Result<MaterialsWritten, ContentError> {
        self.write_materials_on(graphs, definitions, directory, &bake_with_report)
    }

    /// [`write_materials`](Self::write_materials) on another backend, such as a GPU baker
    /// a tool supplies. The CPU is the reference.
    ///
    /// # Errors
    ///
    /// As [`write_materials`](Self::write_materials).
    pub fn write_materials_on(
        &self,
        graphs: &MaterialGraphLibrary,
        definitions: &MaterialLibrary,
        directory: &str,
        backend: Backend<'_>,
    ) -> Result<MaterialsWritten, ContentError> {
        let exported = export(&ExportRequest {
            graphs,
            definitions,
            directory,
            resolution: self.resolution,
            threads: self.threads,
            backend,
        })?;
        let mut bytes = 0;
        let mut sets = Vec::with_capacity(exported.sets.len());
        for set in &exported.sets {
            for (path, data) in set.files(self.supercompression)? {
                bytes += data.len();
                self.write_file(&path, &data)?;
            }
            if self.review_pngs {
                for (name, map) in maps(&set.maps) {
                    let path = format!("{}/{name}.png", set.directory);
                    let data =
                        png(map, set.maps.resolution).map_err(|message| ContentError::Png {
                            path: self.root.join(&path),
                            message,
                        })?;
                    bytes += data.len();
                    self.write_file(&path, &data)?;
                }
            }
            sets.push((set.key.clone(), set.report.clone()));
        }
        let directory = directory.trim_end_matches('/');
        let library = ron_pretty(&exported.library, LIBRARY_FILE)?;
        let graphs_text = ron_pretty(graphs, GRAPHS_FILE)?;
        bytes += library.len() + graphs_text.len();
        self.write_file(&format!("{directory}/{LIBRARY_FILE}"), library.as_bytes())?;
        self.write_file(
            &format!("{directory}/{GRAPHS_FILE}"),
            graphs_text.as_bytes(),
        )?;
        Ok(MaterialsWritten {
            library: exported.library,
            sets,
            bytes,
        })
    }

    /// Mesh `building` at every level of `ladder` and write it to `path`,
    /// relative to the root. Pass `&[]` for the building as authored alone.
    ///
    /// # Errors
    ///
    /// The first element that does not mesh, or a file that does not write.
    pub fn write_building(
        &self,
        path: &str,
        building: &Building,
        ladder: &[LodPolicy],
    ) -> Result<BuildingWritten, ContentError> {
        let started = Instant::now();
        let baked = ashlar_manifold::bake(building, ladder, &ManifoldMesher::default())?;
        let elapsed = started.elapsed();
        let data = baked.write()?;
        self.write_file(path, &data)?;
        Ok(BuildingWritten {
            triangles: baked.triangles(),
            bytes: data.len(),
            elapsed,
        })
    }

    /// The whole content step for a set of buildings: resolve their
    /// per-instance overrides ([`flatten_overrides`]), bake and write the
    /// materials under `directory`, and bake and write each building to its
    /// path.
    ///
    /// # Errors
    ///
    /// The first binding, material or building that fails, by name.
    pub fn ship(
        &self,
        graphs: &MaterialGraphLibrary,
        definitions: &MaterialLibrary,
        directory: &str,
        buildings: &[(String, Building)],
        ladder: &[LodPolicy],
    ) -> Result<Shipped, ContentError> {
        let in_building = |path: &str, source: ContentError| ContentError::Building {
            path: path.to_owned(),
            source: Box::new(source),
        };
        // One building at a time, so a binding that fails is named by the
        // building's path as well as by its own; the library accumulates, so
        // an override two buildings share is still one definition.
        let mut library = definitions.clone();
        let mut flattened = Vec::with_capacity(buildings.len());
        for (path, building) in buildings {
            let (mut one, grown) = flatten_overrides(std::slice::from_ref(building), &library)
                .map_err(|error| in_building(path, error.into()))?;
            library = grown;
            flattened.append(&mut one);
        }
        let materials = self.write_materials(graphs, &library, directory)?;
        let mut written = Vec::with_capacity(buildings.len());
        for ((path, _), building) in buildings.iter().zip(&flattened) {
            let building = self
                .write_building(path, building, ladder)
                .map_err(|error| in_building(path, error))?;
            written.push((path.clone(), building));
        }
        Ok(Shipped {
            materials,
            buildings: written,
        })
    }

    /// Write one file under the root, through a temporary file and a rename.
    ///
    /// # Errors
    ///
    /// [`ContentError::Io`] naming the file.
    /// A path that is absolute or has a `..` in it is refused rather than
    /// written somewhere the root does not hold.
    pub fn write_file(&self, path: &str, data: &[u8]) -> Result<(), ContentError> {
        let relative = Path::new(path);
        if relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
        {
            return Err(ContentError::OutsideRoot(path.to_owned()));
        }
        let target = self.root.join(relative);
        let io = |source| ContentError::Io {
            path: target.clone(),
            source,
        };
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        // Create beside the target so persist is a rename on the same
        // filesystem. Each writer owns its temporary file, including cleanup
        // on failure; concurrent writers never truncate one another's data.
        let parent = target.parent().unwrap_or(&self.root);
        let mut partial = tempfile::NamedTempFile::new_in(parent).map_err(io)?;
        partial.write_all(data).map_err(io)?;
        partial.persist(&target).map_err(|error| io(error.error))?;
        Ok(())
    }
}

/// Level 0 of one map as PNG bytes.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a clamped, normalised display channel"
)]
fn png(map: &Encoded, resolution: u32) -> Result<Vec<u8>, String> {
    use image::{ColorType, ImageEncoder, codecs::png::PngEncoder};
    let level = map.mips.first().ok_or("a bake writes level 0")?;
    let (bytes, colour) = match map.format {
        // The encoder takes native-endian samples and the map is little-endian.
        PlaneFormat::R16Unorm => (
            level
                .as_chunks::<2>()
                .0
                .iter()
                .flat_map(|sample| u16::from_le_bytes(*sample).to_ne_bytes())
                .collect(),
            ColorType::L16,
        ),
        PlaneFormat::Rgba16Float => (
            level
                .as_chunks::<8>()
                .0
                .iter()
                .flat_map(|texel| {
                    let mut rgba = [255_u8; 4];
                    for (channel, out) in rgba.iter_mut().take(3).enumerate() {
                        let linear =
                            half::f16::from_le_bytes([texel[channel * 2], texel[channel * 2 + 1]])
                                .to_f32();
                        *out = (linear_to_srgb(linear.clamp(0.0, 1.0)) * 255.0).round() as u8;
                    }
                    rgba
                })
                .collect(),
            ColorType::Rgba8,
        ),
        PlaneFormat::Rgba8Srgb | PlaneFormat::Rgba8Unorm => (level.clone(), ColorType::Rgba8),
    };
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(&bytes, resolution, resolution, colour.into())
        .map_err(|error| error.to_string())?;
    Ok(out)
}

fn ron_pretty(value: &impl serde::Serialize, what: &str) -> Result<String, ContentError> {
    ron::ser::to_string_pretty(value, ron::ser::PrettyConfig::default()).map_err(|error| {
        ContentError::Serialize {
            what: what.to_owned(),
            message: error.to_string(),
        }
    })
}

/// Resolve every per-instance graph override into a definition of its own.
///
/// Answers the buildings rewritten to bind those definitions by key alone, and
/// the library with one definition added per distinct overriding binding. An
/// override's definition is its base's with the parameters written in, keyed
/// `<namespace>:<name>--<hash>` where the hash is of the parameters, so the same
/// override from any number of buildings is one definition and one bake.
/// Bindings without parameters are left as they are, and so is one the
/// library cannot dress, such as a palette entry no element uses; a binding
/// that is used is checked before anything is rewritten.
///
/// # Errors
///
/// A binding naming a material the library does not hold, or overriding
/// parameters of a surface that has none, by the building's own path; and an
/// override whose key the library already holds as a definition of its own.
pub fn flatten_overrides(
    buildings: &[Building],
    definitions: &MaterialLibrary,
) -> Result<(Vec<Building>, MaterialLibrary), ValidationError> {
    let mut library = definitions.clone();
    let mut flattened = Vec::with_capacity(buildings.len());
    for building in buildings {
        definitions.check_for(building)?;
        let mut recipe = building.recipe().clone();
        let mut plain = |binding: &mut Binding, path: String| {
            if binding.params.is_empty() {
                return Ok(());
            }
            let key = override_key(binding);
            let Some(dressed) = definitions
                .materials
                .get(&binding.material)
                .and_then(|base| base.overridden(binding))
            else {
                return Ok(());
            };
            match library.materials.get(&key) {
                Some(existing) if *existing != *dressed => {
                    return Err(ValidationError {
                        path,
                        reason: format!(
                            "the override's key {key:?} is already a different definition"
                        ),
                    });
                }
                Some(_) => {}
                None => {
                    library.materials.insert(key.clone(), dressed.into_owned());
                }
            }
            *binding = Binding::new(key);
            Ok(())
        };
        for (slot, binding) in &mut recipe.materials {
            plain(binding, format!("materials[{slot}]"))?;
        }
        for instance in &mut recipe.instances {
            for (slot, binding) in &mut instance.materials {
                plain(
                    binding,
                    format!("instances[{}].materials[{slot}]", instance.id),
                )?;
            }
        }
        flattened.push(recipe.build()?);
    }
    Ok((flattened, library))
}

/// `<material>--<16 hex digits of an FNV-1a hash of the parameters' RON>`.
fn override_key(binding: &Binding) -> String {
    let text = ron::to_string(&binding.params).unwrap_or_default();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{}--{hash:016x}", binding.material)
}
