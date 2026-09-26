//! The pluggable content boundary. The tool knows scene names, not buildings.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use ashlar::{Building, MaterialLibrary};
use ashlar_bevy as materials;
use ashlar_material::MaterialGraphLibrary;

type Builder = Box<dyn Fn() -> Result<Building> + Send + Sync>;

/// Where a scene's material or graph library comes from.
#[derive(Clone)]
enum Source<T> {
    /// A RON file, relative to `--asset-root`. The preview watches a graph
    /// library read this way and reloads it when it changes.
    File(String),
    /// A library the content builds in Rust, asked for when the scene opens.
    Built(Arc<dyn Fn() -> T + Send + Sync>),
}

/// One named building in a [`Catalog`].
///
/// The building is produced on demand rather than stored, so a catalog can be
/// declared once at startup without meshing or validating anything.
pub struct Scene {
    name: String,
    display: String,
    library: Option<Source<MaterialLibrary>>,
    graphs: Option<Source<MaterialGraphLibrary>>,
    build: Builder,
}

impl Scene {
    /// A scene called `name`, built by `build`.
    ///
    /// `name` is what `--scene` and `--reference-scenes` accept and what the
    /// gallery uses for its file names, so keep it lowercase and hyphenated.
    pub fn new<F>(name: impl Into<String>, build: F) -> Self
    where
        F: Fn() -> Result<Building> + Send + Sync + 'static,
    {
        let name = name.into();
        Self {
            display: name.clone(),
            name,
            library: None,
            graphs: None,
            build: Box::new(build),
        }
    }

    /// Override the human-readable heading shown in the gallery.
    #[must_use]
    pub fn display_name(mut self, display: impl Into<String>) -> Self {
        self.display = display.into();
        self
    }

    /// The material library for this scene, as a RON file relative to
    /// `--asset-root`.
    ///
    /// Without one the preview falls back to stable diagnostic colours.
    #[must_use]
    pub fn material_library(mut self, key: impl Into<String>) -> Self {
        self.library = Some(Source::File(key.into()));
        self
    }

    /// The material library for this scene, built in Rust when the scene opens.
    #[must_use]
    pub fn materials<F>(mut self, build: F) -> Self
    where
        F: Fn() -> MaterialLibrary + Send + Sync + 'static,
    {
        self.library = Some(Source::Built(Arc::new(build)));
        self
    }

    /// The material graph library for this scene, as a RON file relative to
    /// `--asset-root`.
    ///
    /// A scene whose surfaces are all files or constants needs none; a scene
    /// with a `Surface::Graph` is preflighted against this, so a graph that
    /// stopped validating is a startup error rather than an untextured wall.
    #[must_use]
    pub fn graph_library(mut self, key: impl Into<String>) -> Self {
        self.graphs = Some(Source::File(key.into()));
        self
    }

    /// The material graph library for this scene, built in Rust when the scene
    /// opens.
    #[must_use]
    pub fn graphs<F>(mut self, build: F) -> Self
    where
        F: Fn() -> MaterialGraphLibrary + Send + Sync + 'static,
    {
        self.graphs = Some(Source::Built(Arc::new(build)));
        self
    }

    /// The catalog key, as accepted on the command line.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The heading shown in the reference gallery.
    pub fn display(&self) -> &str {
        &self.display
    }

    /// Load and validate this scene's graph library, and the file it came from
    /// when it came from one.
    ///
    /// # Errors
    ///
    /// A file that does not read, or a library that does not validate.
    pub fn load_graphs(
        &self,
        asset_root: &Path,
    ) -> Result<(MaterialGraphLibrary, Option<PathBuf>)> {
        match &self.graphs {
            None => Ok((MaterialGraphLibrary::default(), None)),
            Some(Source::File(key)) => {
                let path = asset_root.join(key);
                Ok((materials::read_graphs(&path)?, Some(path)))
            }
            Some(Source::Built(build)) => {
                let graphs = build();
                graphs
                    .check()
                    .with_context(|| format!("validating {}'s graph library", self.name))?;
                Ok((graphs, None))
            }
        }
    }

    /// Load and preflight this scene's material library against its building
    /// and its graphs.
    ///
    /// # Errors
    ///
    /// A file that does not read, or a definition, a texture or a graph that
    /// does not preflight.
    pub fn load_materials(
        &self,
        asset_root: &Path,
        building: &Building,
        graphs: &MaterialGraphLibrary,
    ) -> Result<Option<MaterialLibrary>> {
        match &self.library {
            None => Ok(None),
            Some(Source::File(key)) => Ok(Some(materials::read_library_with_graphs(
                &asset_root.join(key),
                asset_root,
                building,
                graphs,
            )?)),
            Some(Source::Built(build)) => {
                let library = build();
                materials::check_library_with_graphs(
                    &format!("{}'s material library", self.name),
                    &library,
                    asset_root,
                    building,
                    graphs,
                )?;
                Ok(Some(library))
            }
        }
    }

    /// Produce the building.
    pub fn build(&self) -> Result<Building> {
        (self.build)()
    }
}

/// The set of scenes a preview binary offers.
///
/// A downstream game builds one over its own content and calls
/// [`crate::run`]; nothing here depends on the bundled showcase.
#[derive(Default)]
pub struct Catalog {
    scenes: Vec<Scene>,
    default_scene: Option<String>,
    asset_root: Option<PathBuf>,
}

impl Catalog {
    /// An empty catalog.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a scene.
    ///
    /// # Panics
    ///
    /// If the name is already taken. Two scenes with one name would overwrite
    /// each other's gallery images, so this is a programming error rather than
    /// something the command line should report.
    #[must_use]
    pub fn scene(mut self, scene: Scene) -> Self {
        assert!(
            self.get(scene.name()).is_none(),
            "duplicate scene name: {}",
            scene.name()
        );
        self.scenes.push(scene);
        self
    }

    /// The scene `--scene` selects when it is not given.
    #[must_use]
    pub fn default_scene(mut self, name: impl Into<String>) -> Self {
        self.default_scene = Some(name.into());
        self
    }

    /// Where `--asset-root` points when it is not given.
    ///
    /// A downstream binary lives in its own repository, and the built-in
    /// default is this workspace's assets folder, which is not where that
    /// game's textures are.
    #[must_use]
    pub fn asset_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.asset_root = Some(root.into());
        self
    }

    /// The default `--asset-root`, if this catalog names one.
    pub fn default_asset_root(&self) -> Option<&Path> {
        self.asset_root.as_deref()
    }

    /// Every scene, in declaration order.
    pub fn scenes(&self) -> &[Scene] {
        &self.scenes
    }

    /// Every scene name, in declaration order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.scenes.iter().map(Scene::name)
    }

    /// The scene called `name`, if there is one.
    pub fn get(&self, name: &str) -> Option<&Scene> {
        self.scenes.iter().find(|scene| scene.name() == name)
    }

    /// The scene called `name`, with the available names in the error.
    pub fn require(&self, name: &str) -> Result<&Scene> {
        match self.get(name) {
            Some(scene) => Ok(scene),
            None => bail!(
                "unknown scene {name:?}; available: {}",
                self.names().collect::<Vec<_>>().join(", ")
            ),
        }
    }

    /// The scene to show when `--scene` is absent: the declared default, else
    /// the first one registered.
    pub fn default_name(&self) -> Option<&str> {
        self.default_scene
            .as_deref()
            .or_else(|| self.scenes.first().map(Scene::name))
    }
}
