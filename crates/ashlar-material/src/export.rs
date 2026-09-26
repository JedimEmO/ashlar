//! The content step: a library of definitions turned into files a game ships.
//!
//! A [`MaterialDefinition`](ashlar_surface::MaterialDefinition) whose surface is [`Surface::Graph`] can be
//! delivered two ways, and both start from the same definition. Registered as
//! it is, the renderer bakes it when the material is registered and nothing
//! touches disk. Passed through [`export`] first, it comes back as a
//! [`Surface::Files`] naming maps under a directory, with the bake it was
//! made from recorded in `baked_from`, and the maps and strand sets come back
//! beside it for the caller to write. A game picks per material; the
//! definition it started from is the same one either way.
//!
//! This module does no IO, like the rest of the crate: it answers the bytes
//! and the paths they belong at, relative to the asset root the definitions are
//! resolved against, and the caller writes them. [`ExportedSet::files`] is the
//! whole of what a writer needs.
//!
//! ```
//! use ashlar_material::{export, stdlib};
//!
//! let graphs = stdlib::graphs();
//! let mut definitions = stdlib::materials();
//! definitions.materials.retain(|key, _| key == "library:paving-slabs");
//! let exported = export::export(&export::ExportRequest {
//!     graphs: &graphs,
//!     definitions: &definitions,
//!     directory: "materials",
//!     resolution: Some(256),
//!     threads: std::num::NonZeroUsize::new(2),
//!     backend: &ashlar_material::bake::bake_with_report,
//! })?;
//! let set = &exported.sets[0];
//! assert_eq!(set.directory, "materials/library/paving-slabs");
//! let files = set.files(ashlar_material::ktx2::Supercompression::None)?;
//! assert!(files.iter().any(|(path, _)| path == "materials/library/paving-slabs/base.ktx2"));
//! # Ok::<(), export::ExportError>(())
//! ```
use crate::{
    MaterialGraphLibrary,
    bake::{Backend, BakeError, BakeReport, BakeRequest, Encoded, TextureSet},
    ktx2::{self, Ktx2Error, Supercompression},
    strands::{self, StrandError, StrandRequest},
};
use ashlar_surface::{Bake, MaterialLibrary, Surface};
use std::num::NonZeroUsize;

/// What an export is asked for.
#[derive(Clone, Copy)]
pub struct ExportRequest<'a> {
    /// The graphs every definition's bake names, and whatever they instance.
    pub graphs: &'a MaterialGraphLibrary,
    /// The definitions to export. Every one comes back; only the
    /// [`Surface::Graph`] ones are baked.
    pub definitions: &'a MaterialLibrary,
    /// Where the files go, relative to the asset root the returned library is
    /// resolved against. A definition keyed `namespace:name` is written under
    /// `<directory>/<namespace>/<name>/`.
    pub directory: &'a str,
    /// Texels per repeat for every bake, in place of each [`Bake`]'s own.
    /// `None` keeps the definition's; the recorded `baked_from` is always what
    /// was actually baked.
    pub resolution: Option<u32>,
    /// Threads each bake and scatter divides its rows across. `None` is one
    /// per available core, as it is for a [`BakeRequest`].
    pub threads: Option<NonZeroUsize>,
    /// What rasterises each bake: [`bake_with_report`](crate::bake::bake_with_report) on the CPU, which is the
    /// reference, or a device baker a tool supplies.
    pub backend: Backend<'a>,
}

/// What an export answers: the library to ship, and what to write for it.
#[derive(Debug)]
pub struct Exported {
    /// Every definition of the request. The baked ones now name files, and a
    /// definition that grows strands names the set that was scattered for it.
    pub library: MaterialLibrary,
    /// One per distinct bake. Two definitions that bake the same graph with the
    /// same parameters at the same resolution share the first one's files.
    pub sets: Vec<ExportedSet>,
}

/// One bake's maps, its strands, and where they go.
#[derive(Debug)]
pub struct ExportedSet {
    /// The first definition that asked for this bake.
    pub key: String,
    /// The directory the files are named under, relative to the asset root.
    pub directory: String,
    /// What was baked.
    pub bake: Bake,
    /// The maps, with their full mip chains.
    pub maps: TextureSet,
    /// The strand set file, when the definition grows strands.
    pub strands: Option<Vec<u8>>,
    /// The strand layers that file holds, when it holds any.
    pub layers: Option<Vec<String>>,
    /// The bake's report: ops, planes, height range and warnings.
    pub report: BakeReport,
}

impl ExportedSet {
    /// Every file of this set as a path relative to the asset root and its
    /// bytes, KTX2 containers first and the strand set last.
    ///
    /// # Errors
    ///
    /// [`ExportError::Ktx2`] if a map cannot be written, which for a map a bake
    /// produced means the supercompression asked for is not compiled in.
    pub fn files(
        &self,
        supercompression: Supercompression,
    ) -> Result<Vec<(String, Vec<u8>)>, ExportError> {
        let mut files = Vec::new();
        for (name, map) in maps(&self.maps) {
            files.push((
                format!("{}/{name}.ktx2", self.directory),
                ktx2::write_with(map, self.maps.resolution, supercompression)?,
            ));
        }
        if let Some(bytes) = &self.strands {
            files.push((format!("{}/set.strands", self.directory), bytes.clone()));
        }
        Ok(files)
    }
}

/// The maps of a set under the file names an exported definition names them
/// by, in the order they are written.
#[must_use]
pub fn maps(set: &TextureSet) -> Vec<(&'static str, &Encoded)> {
    let mut maps = vec![
        ("base", &set.base_color),
        ("normal", &set.normal),
        ("orm", &set.orm),
    ];
    if let Some(height) = &set.height {
        maps.push(("height", height));
    }
    if let Some(emissive) = &set.emissive {
        maps.push(("emissive", emissive));
    }
    maps
}

/// Why an export failed, named by the definition that failed it.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// A definition's bake names a graph the library does not hold.
    #[error("{key}: no graph {graph} in the library")]
    Graph {
        /// The definition.
        key: String,
        /// The graph it named.
        graph: String,
    },
    /// A definition's key is not `namespace:name`, both halves of lowercase
    /// ASCII letters, digits, `-` and `_`: the halves become directory names,
    /// so anything else could leave the export directory or not be a portable
    /// path.
    #[error("{0}: a definition key is `namespace:name` in [a-z0-9_-]")]
    Key(String),
    /// The bake refused.
    #[error("{key}: {source}")]
    Bake {
        /// The definition.
        key: String,
        /// Why.
        source: BakeError,
    },
    /// A strand layer would not scatter.
    #[error("{key}: {source}")]
    Strands {
        /// The definition.
        key: String,
        /// Why.
        source: StrandError,
    },
    /// The strand file would not write.
    #[error("{key}: {source}")]
    StrandFile {
        /// The definition.
        key: String,
        /// Why.
        source: strands::file::StrandFileError,
    },
    /// A map would not encode as KTX2.
    #[error(transparent)]
    Ktx2(#[from] Ktx2Error),
}

/// Bake every [`Surface::Graph`] definition of a library and answer the
/// library that names the results as files.
///
/// Definitions that are not baked — plain, shader, already textures — come back
/// unchanged. A definition that grows strands and names no `baked_set` has its
/// layers scattered from the same graph at the same parameters and gets one;
/// one that already names a set keeps it.
///
/// # Errors
///
/// The first definition that cannot be baked or scattered, by key.
pub fn export(request: &ExportRequest<'_>) -> Result<Exported, ExportError> {
    let mut library = request.definitions.clone();
    let mut sets: Vec<ExportedSet> = Vec::new();
    for (key, definition) in &mut library.materials {
        let Surface::Graph(bake) = &definition.surface else {
            continue;
        };
        let bake = Bake {
            resolution: request.resolution.unwrap_or(bake.resolution),
            ..bake.clone()
        };
        // Two definitions share a set when they bake the same graph the same
        // way and grow the same strand layers; a definition that differs only
        // in its repeat or its constants shares the maps.
        let layers = definition
            .strands
            .as_ref()
            .filter(|settings| settings.baked_set.is_none())
            .map(|settings| settings.layers.clone());
        let index = if let Some(index) = sets
            .iter()
            .position(|set| set.bake == bake && set.layers == layers)
        {
            index
        } else {
            sets.push(bake_set(request, key, &bake, definition.strands.as_ref())?);
            sets.len() - 1
        };
        let set = &sets[index];
        let path = |map: &str| Some(format!("{}/{map}.ktx2", set.directory));
        definition.surface = Surface::Files {
            base_color: path("base"),
            normal: path("normal"),
            orm: path("orm"),
            height: set.maps.height.as_ref().and_then(|_| path("height")),
            emissive: set.maps.emissive.as_ref().and_then(|_| path("emissive")),
            baked_from: Some(bake),
        };
        if let Some(settings) = &mut definition.strands
            && settings.baked_set.is_none()
            && set.strands.is_some()
        {
            settings.baked_set = Some(format!("{}/set.strands", set.directory));
        }
    }
    Ok(Exported { library, sets })
}

fn bake_set(
    request: &ExportRequest<'_>,
    key: &str,
    bake: &Bake,
    strands: Option<&ashlar_surface::StrandSettings>,
) -> Result<ExportedSet, ExportError> {
    let portable = |half: &str| {
        !half.is_empty()
            && half
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    };
    let (namespace, name) = key
        .split_once(':')
        .filter(|(namespace, name)| portable(namespace) && portable(name))
        .ok_or_else(|| ExportError::Key(key.to_owned()))?;
    let graph = request
        .graphs
        .get(&bake.graph)
        .ok_or_else(|| ExportError::Graph {
            key: key.to_owned(),
            graph: bake.graph.clone(),
        })?;
    let (maps, report) = (request.backend)(&BakeRequest {
        graph,
        library: request.graphs,
        params: &bake.params,
        resolution: bake.resolution,
        mips: true,
        threads: request.threads,
    })
    .map_err(|source| ExportError::Bake {
        key: key.to_owned(),
        source,
    })?;
    let settings_layers = strands
        .filter(|settings| settings.baked_set.is_none())
        .map(|settings| settings.layers.clone());
    let strands = match strands {
        Some(settings) if settings.baked_set.is_none() => {
            let sets = settings
                .layers
                .iter()
                .map(|layer| {
                    strands::scatter(&StrandRequest {
                        graph,
                        library: request.graphs,
                        params: &bake.params,
                        layer,
                        field_resolution: strands::FIELD_RESOLUTION,
                        threads: request.threads,
                    })
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(|source| ExportError::Strands {
                    key: key.to_owned(),
                    source,
                })?;
            Some(
                strands::file::write(&sets).map_err(|source| ExportError::StrandFile {
                    key: key.to_owned(),
                    source,
                })?,
            )
        }
        _ => None,
    };
    Ok(ExportedSet {
        key: key.to_owned(),
        directory: format!(
            "{}/{namespace}/{name}",
            request.directory.trim_end_matches('/')
        ),
        bake: bake.clone(),
        maps,
        layers: strands.is_some().then(|| settings_layers.clone()).flatten(),
        strands,
        report,
    })
}
