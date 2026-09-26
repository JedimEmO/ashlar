//! The authoring loop: the graph library watched on disk, and what changed re-baked.
//!
//! A material graph is text, and the point of it being text is that editing it
//! and looking at the result is one motion. This module is the second half of
//! that: the file's timestamp is read a couple of times a second, a change
//! re-reads and re-validates the library, and every material the scene holds is
//! created again from it. The panel in [`crate::panel`] is the same path with
//! the edit coming from a slider instead of an editor.
//!
//! Nothing here touches the entities. A preview material is an asset that the
//! meshes name by handle, so a re-bake writes the new [`StandardMaterial`] into
//! the handle the scene already has and every wall using it changes at once;
//! swapping the handle on each entity would be the same picture and more
//! bookkeeping.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Instant, SystemTime},
};

use anyhow::{Context, Result};
use ashlar_bevy as materials;
use bevy::{prelude::*, time::Real};

use crate::viewer::{Baking, Compiling, Palette};

/// How long between two reads of the graph library's timestamp.
///
/// Half a second is well under the pause between saving a file and looking up,
/// and far over the cost of one `stat`. A poll rather than a watch because both
/// watches cost more than they save here: `notify` is not in this workspace's
/// dependency graph — `cargo tree -i inotify` ends in `gilrs`, not in it — and
/// enabling Bevy's `file_watcher` feature, which is `notify`, was measured to
/// bring a second `windows-sys` family that `cargo deny`'s duplicate rule
/// refuses. It would also watch an asset root, and `--graphs` may name a file
/// anywhere on disk.
pub(crate) const POLL: f32 = 0.5;

/// What a poll compares: when the file was last written, and what is in it.
///
/// The timestamp alone is not enough, and not by a little: writing this file
/// twice in a row and reading `st_mtime_ns` between the writes gives the same
/// nanosecond on 195 runs out of 200 on this machine's filesystem, because an
/// inode's timestamp is only as fine as the kernel's coarse clock. Two saves
/// inside one tick are a minute apart for a person and indistinguishable here,
/// and a test that edits a file twice sees it every time. So the digest of the
/// bytes is in the stamp as well, which costs one read of a few kilobytes twice
/// a second and makes a missed edit impossible rather than unlikely. The
/// timestamp stays in front of it so that a `touch` with no edit still re-bakes,
/// which is the cheapest way to ask for one by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stamp {
    modified: SystemTime,
    len: u64,
    digest: u64,
}

/// The stamp of `path`, or `None` where it cannot be read.
///
/// A file being rewritten is briefly absent on the atomic-rename save path some
/// editors take, and a missing file is not an error here: the next poll finds it
/// again, and until then the library in memory is the one that was valid.
pub(crate) fn stamp(path: &Path) -> Option<Stamp> {
    let data = std::fs::metadata(path).ok()?;
    let bytes = std::fs::read(path).ok()?;
    Some(Stamp {
        modified: data.modified().ok()?,
        len: data.len(),
        digest: digest(&bytes),
    })
}

/// FNV-1a over the file's bytes: not a security hash, and not asked to be one.
///
/// What it has to do is separate two saves of the same graph library with one
/// number changed, and it does that.
fn digest(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Where the graph library came from, and where the panel saves it.
///
/// Absent `path` is a scene with no graphs at all, or a `--recipe` that brought
/// none: there is then nothing to watch and nothing to save, and the panel says
/// so rather than inventing a file.
#[derive(Resource, Default)]
pub(crate) struct GraphSource {
    /// The file the library was read from, and the file that is watched.
    pub(crate) path: Option<PathBuf>,
    /// Where `--graphs-out` sends a save, when it is not back over the source.
    pub(crate) out: Option<PathBuf>,
    /// The stamp the last poll saw. Seeded at startup, so the first poll after
    /// the window opens is not a reload of the file that was just read.
    seen: Option<Stamp>,
    /// Seconds left until the next poll.
    countdown: f32,
}

impl GraphSource {
    /// The source of a library read from `path`, already stamped.
    pub(crate) fn new(path: Option<PathBuf>, out: Option<PathBuf>) -> Self {
        let seen = path.as_deref().and_then(stamp);
        Self {
            path,
            out,
            seen,
            countdown: POLL,
        }
    }

    /// Where a save goes: `--graphs-out` if it was given, else the file the
    /// library was read from.
    pub(crate) fn destination(&self) -> Option<&Path> {
        self.out.as_deref().or(self.path.as_deref())
    }
}

/// Set when the in-memory graph library changed: a file reload, or a panel edit
/// the pointer let go of.
///
/// A flag consumed by [`rebake`] rather than a bake run where the change
/// happened, so that an edit arriving in an observer and a reload arriving in a
/// system coalesce into one bake per frame.
#[derive(Resource, Default)]
pub(crate) struct Rebake(pub(crate) bool);

impl Rebake {
    /// Ask for a bake on the next frame.
    pub(crate) fn request(&mut self) {
        self.0 = true;
    }
}

/// Set when a live parameter may have moved: a panel edit, or a re-bake that
/// rebuilt the compiled materials from the definitions.
///
/// Separate from [`Rebake`] because it is the cheap half and wants to happen on
/// every step of a drag rather than on release. Writing a compiled material's
/// uniform block rasterises nothing, emits no WGSL and rebuilds no pipeline —
/// that is the whole point of a live parameter — so there is no reason to make
/// the pointer let go first.
#[derive(Resource, Default)]
pub(crate) struct Redress(pub(crate) bool);

impl Redress {
    /// Rewrite the compiled materials' uniforms on the next frame.
    pub(crate) fn request(&mut self) {
        self.0 = true;
    }
}

/// Set when the file replaced the library in memory, and taken by the panel.
///
/// Separate from [`Rebake`] because the two have different audiences: a bake is
/// wanted after any edit, and a rebuilt panel is wanted only when the values
/// under it moved without it. The panel's own sliders write the library on every
/// step of a drag, so watching the resource for changes instead would despawn
/// the slider being dragged.
#[derive(Resource, Default)]
pub(crate) struct Reloaded(pub(crate) bool);

/// Read the graph library's timestamp, and reload it when it moved.
///
/// A library that no longer parses or no longer validates is reported and
/// dropped: the preview keeps the graphs it had, which is what a half-written
/// file being saved needs it to do. The stamp is recorded either way, so a
/// broken file is complained about once rather than twice a second.
pub(crate) fn watch(
    // The real clock, not the virtual one: a poll interval is wall time, and the
    // virtual clock is both pausable and clamped to 250 ms a frame, so a frame
    // that took longer than that would under-count towards the next poll.
    time: Res<Time<Real>>,
    mut source: ResMut<GraphSource>,
    mut graphs: ResMut<crate::Graphs>,
    mut rebake: ResMut<Rebake>,
    mut reloaded: ResMut<Reloaded>,
) {
    let Some(path) = source.path.clone() else {
        return;
    };
    source.countdown -= time.delta_secs();
    if source.countdown > 0.0 {
        return;
    }
    source.countdown = POLL;
    let now = stamp(&path);
    if now == source.seen {
        return;
    }
    source.seen = now;
    match materials::read_graphs(&path) {
        Ok(library) => {
            println!(
                "Reloaded {} ({} graphs)",
                path.display(),
                library.graphs.len()
            );
            graphs.0 = library;
            rebake.request();
            reloaded.0 = true;
        }
        Err(error) => eprintln!("Keeping the graphs that were loaded: {error:#}"),
    }
}

/// Create every material of the scene again, from the graph library as it is now.
///
/// All of them rather than the ones whose graph moved: a material is a handful
/// of milliseconds when its surface is files or constants, the cache is cleared
/// anyway — its key names a graph rather than describing one, so an edited
/// library makes every entry a lie — and which graphs an edit reached is a
/// question the answer to is "re-read the file", not something the panel knows.
///
/// Nothing is committed until every material is created, so a graph that
/// validates and then refuses to lower leaves the picture exactly as it was
/// rather than half of it re-baked.
///
/// A *compiled* surface is re-made too, and it is the expensive half: its graph
/// is partitioned again, its static planes are rasterised again and its two
/// shaders are emitted again, which is a pipeline rebuild rather than a texture
/// upload. That is what a node edit or a folded parameter costs, and there is
/// no cheaper way to pay it — the value went into the bound textures and into
/// the generated text. A *live* parameter costs none of it and does not come
/// through here at all; [`redress`] is that path, and it runs on every step of
/// a drag.
///
/// The registry is cleared beside the bake cache, and for the same reason: a
/// [`ShaderKey`](materials::runtime_bake::ShaderKey) names a graph and says
/// nothing about what its nodes do, so after an edit every entry would answer
/// the shader it had before.
///
/// A recompiled material takes its live values from the definitions again,
/// which is right and needs no putting back: the only thing that asks for a
/// re-bake without also asking for a [`Redress`] is a file reload, and a file
/// reload is a new library rather than a value somebody was dragging. When a
/// slider is what asked, the panel set both flags and [`redress`] runs after
/// this in the same frame.
pub(crate) fn rebake(
    mut wanted: ResMut<Rebake>,
    definitions: Res<crate::Definitions>,
    server: Res<AssetServer>,
    mut baking: Baking,
    mut compiling: Compiling,
    mut palette: ResMut<Palette>,
    mut assets: ResMut<Assets<StandardMaterial>>,
) {
    if !std::mem::take(&mut wanted.0) {
        return;
    }
    let Some(library) = definitions.0.as_ref() else {
        return;
    };
    let started = Instant::now();
    baking.clear_cache();
    compiling.forget();
    let mut created = Vec::with_capacity(palette.originals.len());
    for binding in palette.originals.keys() {
        // A palette is keyed by the binding, so a re-bake dresses the fresh
        // definition in the same instance overrides the first bake used. A
        // binding whose material the reloaded library no longer holds is
        // skipped, exactly as one whose key had gone was before.
        let Ok(definition) = materials::definition(library, binding) else {
            continue;
        };
        match materials::runtime_bake::create_graph_material(
            &definition,
            &server,
            &mut baking.context(),
        )
        .with_context(|| format!("material {binding}"))
        {
            Ok(material) => created.push((binding.clone(), material)),
            Err(error) => {
                eprintln!("Re-bake abandoned, materials unchanged: {error:#}");
                return;
            }
        }
    }
    // Nothing is committed until every material of both kinds is made, so a
    // graph that validates and then refuses to lower leaves the picture exactly
    // as it was rather than half of it re-baked and half of it recompiled.
    let mut compiled = Vec::with_capacity(palette.shaders.len());
    {
        let mut cx = baking.shader_context(&mut compiling);
        for binding in palette.shaders.keys() {
            let Ok(definition) = materials::definition(library, binding) else {
                continue;
            };
            match materials::shader::create_shader_material(&definition, &mut cx)
                .with_context(|| format!("material {binding}"))
            {
                Ok(material) => compiled.push((binding.clone(), material)),
                Err(error) => {
                    eprintln!("Recompile abandoned, materials unchanged: {error:#}");
                    return;
                }
            }
        }
    }
    let count = created.len() + compiled.len();
    for (key, material) in created {
        if !palette.alternate
            && let Some(handle) = palette.handles.get(&key)
            && let Some(mut slot) = assets.get_mut(handle)
        {
            *slot = material.clone();
        }
        palette.originals.insert(key, material);
    }
    for (key, material) in compiled {
        if let Some(handle) = palette.shaders.get(&key)
            && let Some(mut slot) = compiling.assets().get_mut(handle)
        {
            *slot = material;
        }
    }
    println!(
        "Re-made {count} materials in {:.0} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
}

/// Write every compiled material's uniform block from the library as it is now.
///
/// The cheap half of the authoring loop, and the one phase three exists for. A
/// live parameter is a uniform: no texture is re-rasterised, no WGSL is
/// re-emitted and no pipeline is rebuilt, because none of them ever saw the
/// value. So this runs on every step of a slider drag rather than on release,
/// and a wall changes under the pointer.
///
/// The value is the one [`create_shader_material`](materials::shader::create_shader_material)
/// would compute: what the definition named, and the graph's own default where
/// it named nothing. A definition that pinned a live parameter has *chosen* a
/// value — `study:concrete-wet` is the study's wet threshold and is wet — and
/// the panel edits graphs rather than materials, so it moves the surfaces that
/// left the choice to the graph. Clicking the dry panel behind the threshold
/// and dragging `wetness` is the demonstration of both halves at once: the
/// compiled sheet specimens follow the pointer, the threshold stays where its
/// definition put it, and the baked wall catches up when the pointer lets go.
pub(crate) fn redress(
    mut wanted: ResMut<Redress>,
    definitions: Res<crate::Definitions>,
    graphs: Res<crate::Graphs>,
    registry: Res<materials::shader::GraphShaders>,
    palette: Res<Palette>,
    mut compiled: ResMut<Assets<materials::shader::ProceduralMaterial>>,
) {
    if !std::mem::take(&mut wanted.0) {
        return;
    }
    let Some(library) = definitions.0.as_ref() else {
        return;
    };
    for (binding, handle) in &palette.shaders {
        // The definition as this instance's binding dresses it, so a value an
        // instance overrode is the value written into the uniform — a live
        // parameter overridden per instance is the same pipeline and the same
        // bound textures with a different block, which is the whole of what an
        // override costs on a compiled surface.
        let Ok(definition) = materials::definition(library, binding) else {
            continue;
        };
        let ashlar::Surface::Shader { graph, params } = &definition.surface else {
            continue;
        };
        let Some(mut material) = compiled.get_mut(handle) else {
            continue;
        };
        let Some(entry) = registry.get(material.extension.graph) else {
            continue;
        };
        let mut values = graphs.0.get(graph).map_or_else(BTreeMap::new, |graph| {
            graph
                .params
                .iter()
                .map(|param| (param.name.clone(), param.value))
                .collect()
        });
        values.extend(params.iter().map(|(name, value)| (name.clone(), *value)));
        material.extension.params = materials::shader::block(&entry.layout, &values);
    }
}

/// Write the in-memory graph library back to RON.
///
/// The destination is `--graphs-out` where it was given and the file the
/// library was read from otherwise, so saving over the source is the default
/// and keeping the shipped file is one flag. Saving moves the source's
/// timestamp, so the stamp is taken again here: without that, [`watch`] would
/// read back what the panel just wrote and re-bake the picture it is already
/// showing.
pub(crate) fn save(graphs: &crate::Graphs, source: &mut GraphSource) -> Result<PathBuf> {
    let path = source
        .destination()
        .context("no graph library to save: this scene brought none, and neither did --graphs")?
        .to_path_buf();
    let text = ron::ser::to_string_pretty(&graphs.0, ron::ser::PrettyConfig::default())
        .context("serialising the graph library")?;
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    if source.path.as_deref() == Some(path.as_path()) {
        source.seen = stamp(&path);
    }
    Ok(path)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{collections::BTreeMap, num::NonZeroUsize, time::Duration};

    use ashlar::{Bake, MaterialDefinition, MaterialLibrary, Surface};
    use ashlar_material::{
        Input, MaterialGraph, MaterialGraphLibrary, MathOp, Param, PbrOutput,
        nodes::{Math, Noise},
    };
    use bevy::{MinimalPlugins, asset::AssetPlugin, time::TimeUpdateStrategy};
    use tempfile::TempDir;

    use super::*;
    use crate::viewer::{BakeThreads, Palette};

    /// The key every fixture binds, and the one material the palette holds.
    pub(crate) const KEY: &str = "wall";
    /// The smallest resolution a bake allows, so a test that bakes four times
    /// costs a few hundred milliseconds rather than a few seconds.
    pub(crate) const RESOLUTION: u32 = 256;

    /// A one-graph library whose noise seed says which version of the file it is.
    ///
    /// The seed reaches every texel, so two seeds are two different texture
    /// sets and "did the re-bake happen" is a question the bytes answer.
    pub(crate) fn library(seed: u32) -> MaterialGraphLibrary {
        let mut library = MaterialGraphLibrary::default();
        library.insert(
            MaterialGraph::builder("test:wall")
                .param(Param::float("wear", 0.25).range(0.0, 1.0))
                .node("grain", Noise::value().period(16).seed(seed))
                .node(
                    "worn",
                    Math::new(MathOp::Mul, "grain", Input::param("wear")),
                )
                .output(
                    PbrOutput::new()
                        .base_color("worn")
                        .roughness("grain")
                        .height("grain")
                        .normal_strength(0.01),
                )
                .into_graph(),
        );
        library
    }

    /// One material that bakes `test:wall`, which is what a re-bake has to reach.
    pub(crate) fn definitions() -> crate::Definitions {
        let mut materials = BTreeMap::new();
        materials.insert(
            KEY.to_owned(),
            MaterialDefinition {
                surface: Surface::Graph(Bake {
                    graph: "test:wall".to_owned(),
                    params: BTreeMap::new(),
                    resolution: RESOLUTION,
                }),
                ..MaterialDefinition::default()
            },
        );
        crate::Definitions(Some(MaterialLibrary { materials }))
    }

    pub(crate) fn write(path: &std::path::Path, library: &MaterialGraphLibrary) {
        std::fs::write(
            path,
            ron::ser::to_string_pretty(library, ron::ser::PrettyConfig::default())
                .expect("serialising a graph library"),
        )
        .expect("writing a graph library");
    }

    /// The viewer's authoring loop without a window: the two systems, the
    /// resources they take, and a clock that steps a whole poll per frame.
    ///
    /// `MinimalPlugins` plus `AssetPlugin` is what supplies the `AssetServer`
    /// that creating a material needs; nothing here renders, and the bake is
    /// ordinary CPU work that answers the same bytes with or without a GPU.
    pub(crate) fn harness(path: &std::path::Path) -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin {
                file_path: path
                    .parent()
                    .expect("a temp file has a directory")
                    .to_string_lossy()
                    .into_owned(),
                ..default()
            },
        ))
        .init_asset::<Image>()
        .init_asset::<StandardMaterial>()
        // A re-bake now also recompiles the scene's compiled surfaces, so the
        // three resources that takes exist even in a harness whose palette
        // holds none of them: a system whose parameters cannot be built does
        // not run at all, which would silently disable the half being tested.
        .init_asset::<bevy::shader::Shader>()
        .init_asset::<materials::shader::ProceduralMaterial>()
        .init_resource::<materials::shader::GraphShaders>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(
            POLL * 1.2,
        )))
        .insert_resource(definitions())
        .insert_resource(crate::Graphs(
            materials::read_graphs(path).expect("the fixture library validates"),
        ))
        .insert_resource(GraphSource::new(Some(path.to_path_buf()), None))
        .insert_resource(BakeThreads(NonZeroUsize::new(3)))
        .init_resource::<materials::runtime_bake::BakeCache>()
        .init_resource::<materials::cards::CardCache>()
        .init_resource::<Rebake>()
        .init_resource::<Redress>()
        .init_resource::<Reloaded>()
        .init_resource::<Palette>()
        .add_systems(Update, (watch, rebake, redress).chain());
        // The palette holds what the scene already created: one key, one
        // handle, and the material that handle points at. A re-bake writes
        // through that handle, which is how every wall using it changes at once.
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let mut palette = app.world_mut().resource_mut::<Palette>();
        palette
            .originals
            .insert(ashlar::Binding::new(KEY), default());
        palette.handles.insert(ashlar::Binding::new(KEY), handle);
        app
    }

    /// The key the compiled fixture binds, and the graph behind it.
    const LIT: &str = "lamp";
    const LIT_GRAPH: &str = "test:lamp";

    /// A one-graph library whose parameter is live: a lamp whose emissive is
    /// driven by `glow`.
    ///
    /// Live rather than baked is the whole of the fixture. The value reaches the
    /// picture through a uniform rather than through a texel, so "did the edit
    /// land" is a question sixteen bytes answer and no plane has to be
    /// rasterised to ask it.
    fn driven(glow: f32, seed: u32) -> MaterialGraphLibrary {
        let mut library = MaterialGraphLibrary::default();
        library.insert(
            MaterialGraph::builder(LIT_GRAPH)
                .param(Param::float("glow", glow).range(0.0, 4.0).live())
                .node("grain", Noise::value().period(16).seed(seed))
                .node("lit", Math::new(MathOp::Mul, "grain", Input::param("glow")))
                .output(
                    PbrOutput::new()
                        .base_color("grain")
                        .roughness("grain")
                        .emissive("lit"),
                )
                .into_graph(),
        );
        library
    }

    /// One compiled material over that graph, pinning nothing.
    fn compiled_definitions() -> crate::Definitions {
        let mut materials = BTreeMap::new();
        materials.insert(
            LIT.to_owned(),
            MaterialDefinition {
                surface: Surface::Shader {
                    graph: LIT_GRAPH.to_owned(),
                    params: BTreeMap::new(),
                },
                ..MaterialDefinition::default()
            },
        );
        crate::Definitions(Some(MaterialLibrary { materials }))
    }

    /// The authoring loop over one compiled surface, with the material already
    /// created the way `setup_materials` would have created it.
    fn compiled_harness(glow: f32, seed: u32) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Image>()
            .init_asset::<StandardMaterial>()
            .init_asset::<bevy::shader::Shader>()
            .init_asset::<materials::shader::ProceduralMaterial>()
            .init_resource::<materials::shader::GraphShaders>()
            .init_resource::<materials::runtime_bake::BakeCache>()
            .init_resource::<materials::cards::CardCache>()
            .insert_resource(compiled_definitions())
            .insert_resource(crate::Graphs(driven(glow, seed)))
            .insert_resource(GraphSource::default())
            .insert_resource(BakeThreads(NonZeroUsize::new(3)))
            .init_resource::<Rebake>()
            .init_resource::<Redress>()
            .init_resource::<Reloaded>()
            .init_resource::<Palette>()
            .add_systems(Update, (rebake, redress).chain());
        let material = {
            let world = app.world_mut();
            let graphs = world.resource::<crate::Graphs>().0.clone();
            let definitions = world.resource::<crate::Definitions>().0.clone();
            let definition = definitions.expect("a library").materials[LIT].clone();
            world.resource_scope(
                |world, mut cache: Mut<materials::runtime_bake::BakeCache>| {
                    world.resource_scope(|world, mut images: Mut<Assets<Image>>| {
                        world.resource_scope(
                            |world, mut shaders: Mut<Assets<bevy::shader::Shader>>| {
                                world.resource_scope(
                                    |_, mut registry: Mut<materials::shader::GraphShaders>| {
                                        materials::shader::create_shader_material(
                                            &definition,
                                            &mut materials::shader::ShaderContext {
                                                graphs: &graphs,
                                                cache: &mut cache,
                                                images: &mut images,
                                                shaders: &mut shaders,
                                                registry: &mut registry,
                                                resolution: Some(RESOLUTION),
                                                threads: NonZeroUsize::new(3),
                                                baker: materials::runtime_bake::Baker::Cpu,
                                            },
                                        )
                                        .expect("the fixture compiles")
                                    },
                                )
                            },
                        )
                    })
                },
            )
        };
        let handle = app
            .world_mut()
            .resource_mut::<Assets<materials::shader::ProceduralMaterial>>()
            .add(material);
        app.world_mut()
            .resource_mut::<Palette>()
            .shaders
            .insert(ashlar::Binding::new(LIT), handle);
        app
    }

    /// The first lane of the compiled fixture's uniform block, which is where
    /// its one live parameter sits.
    fn uniform(app: &App) -> f32 {
        let handle = app.world().resource::<Palette>().shaders[&ashlar::Binding::new(LIT)].clone();
        app.world()
            .resource::<Assets<materials::shader::ProceduralMaterial>>()
            .get(&handle)
            .expect("the compiled material")
            .extension
            .params
            .rows[0]
            .x
    }

    /// The compiled fixture's first bound texture.
    fn bound(app: &App) -> Handle<Image> {
        let handle = app.world().resource::<Palette>().shaders[&ashlar::Binding::new(LIT)].clone();
        app.world()
            .resource::<Assets<materials::shader::ProceduralMaterial>>()
            .get(&handle)
            .expect("the compiled material")
            .extension
            .static_0
            .clone()
            .expect("a bound texture")
    }

    #[test]
    fn a_live_parameter_reaches_a_compiled_surface_without_re_baking_anything() {
        let mut app = compiled_harness(0.5, 11);
        assert!((uniform(&app) - 0.5).abs() < 1e-6, "{}", uniform(&app));
        let texture = bound(&app);

        // A frame with nothing asked for changes nothing.
        app.update();
        assert!((uniform(&app) - 0.5).abs() < 1e-6);

        // The panel's edit: the value in the graph library moves, and a redress
        // is asked for. This is what `dragged` does on every step of a drag,
        // and what it does *without* asking for a bake.
        {
            let mut graphs = app.world_mut().resource_mut::<crate::Graphs>();
            let param = graphs
                .0
                .graphs
                .get_mut(LIT_GRAPH)
                .expect("the fixture graph")
                .params
                .iter_mut()
                .find(|param| param.name == "glow")
                .expect("the live parameter");
            param.value = ashlar_material::ParamValue::Float(2.75);
        }
        app.world_mut().resource_mut::<Redress>().request();
        app.update();

        assert!((uniform(&app) - 2.75).abs() < 1e-6, "{}", uniform(&app));
        // And nothing else moved: the same image, the same cache entry, and no
        // bake asked for. A live parameter costs sixteen bytes of uniform.
        assert_eq!(bound(&app), texture, "a live edit re-rasterised a plane");
        assert_eq!(
            app.world()
                .resource::<materials::runtime_bake::BakeCache>()
                .statics(),
            1
        );
        assert!(
            !app.world().resource::<Rebake>().0,
            "a live edit asked for a bake"
        );
        assert!(
            !app.world().resource::<Redress>().0,
            "the request is consumed"
        );
    }

    #[test]
    fn an_edit_to_a_compiled_graph_recompiles_it_rather_than_leaving_it_stale() {
        // The expensive half, and the one a live parameter exists to avoid. A
        // node moved, so the value went into the bound textures and into the
        // generated text; nothing short of partitioning the graph again can
        // reach it, and the re-bake is where that happens.
        let mut app = compiled_harness(0.5, 11);
        let texture = bound(&app);
        app.world_mut().resource_mut::<crate::Graphs>().0 = driven(0.5, 99);
        app.world_mut().resource_mut::<Rebake>().request();
        app.update();
        assert_ne!(
            bound(&app),
            texture,
            "a recompile left the bound texture the old graph baked"
        );
        assert_eq!(
            app.world()
                .resource::<materials::shader::GraphShaders>()
                .len(),
            1,
            "the registry was cleared and the graph registered again"
        );
        // And the live value survived, because it never depended on the graph's
        // nodes in the first place.
        assert!((uniform(&app) - 0.5).abs() < 1e-6, "{}", uniform(&app));
    }

    #[test]
    fn a_definition_that_pinned_a_live_parameter_keeps_the_value_it_pinned() {
        // The other half of the rule, and the reason `study:concrete-wet` stays
        // wet however far the panel's slider travels: the panel edits graphs,
        // and a definition that named a value has chosen one. A live parameter
        // is still live — a game can drive it — but the thing driving it here
        // is the library, not the slider.
        let mut app = compiled_harness(0.5, 11);
        if let Some(library) = app
            .world_mut()
            .resource_mut::<crate::Definitions>()
            .0
            .as_mut()
        {
            library.materials.get_mut(LIT).expect("the fixture").surface = Surface::Shader {
                graph: LIT_GRAPH.to_owned(),
                params: BTreeMap::from([("glow".to_owned(), ashlar::ParamValue::Float(1.25))]),
            };
        }
        {
            let mut graphs = app.world_mut().resource_mut::<crate::Graphs>();
            let param = graphs
                .0
                .graphs
                .get_mut(LIT_GRAPH)
                .expect("the fixture graph")
                .params
                .iter_mut()
                .find(|param| param.name == "glow")
                .expect("the live parameter");
            param.value = ashlar_material::ParamValue::Float(3.5);
        }
        app.world_mut().resource_mut::<Redress>().request();
        app.update();
        assert!((uniform(&app) - 1.25).abs() < 1e-6, "{}", uniform(&app));
    }

    /// The base colour texture of the one material the palette holds.
    pub(crate) fn texture(app: &App) -> Option<Handle<Image>> {
        app.world().resource::<Palette>().originals[&ashlar::Binding::new(KEY)]
            .base_color_texture
            .clone()
    }

    #[test]
    fn a_touched_graph_library_is_reloaded_and_its_materials_re_baked() {
        let directory = TempDir::new().expect("temp dir");
        let path = directory.path().join("materials.graphs.ron");
        write(&path, &library(1));
        let mut app = harness(&path);

        // The first poll is not a reload: the stamp was taken when the library
        // was read, and the file has not moved since.
        app.update();
        assert!(texture(&app).is_none(), "nothing asked for a bake yet");
        assert!(
            app.world()
                .resource::<materials::runtime_bake::BakeCache>()
                .is_empty()
        );

        // A save. The next poll is past the interval, so this is the frame the
        // reload lands in.
        write(&path, &library(2));
        app.update();
        let baked = texture(&app).expect("the reload re-baked the material");
        assert_eq!(
            app.world()
                .resource::<materials::runtime_bake::BakeCache>()
                .len(),
            1,
            "one distinct bake"
        );
        assert_eq!(
            app.world().resource::<crate::Graphs>().0,
            library(2),
            "the library in memory is the library on disk"
        );
        // The handle the scene holds is the one that changed, which is what
        // makes every wall using this material change without touching an
        // entity.
        let handle = app.world().resource::<Palette>().handles[&ashlar::Binding::new(KEY)].clone();
        assert_eq!(
            app.world()
                .resource::<Assets<StandardMaterial>>()
                .get(&handle)
                .expect("the scene's material")
                .base_color_texture,
            Some(baked.clone())
        );

        // A second save is a second bake, and a different seed is different
        // texels, so the images are not the ones from the first.
        write(&path, &library(3));
        app.update();
        let again = texture(&app).expect("the second reload re-baked too");
        assert_ne!(baked, again, "a different graph is a different texture set");
        assert_eq!(
            app.world()
                .resource::<materials::runtime_bake::BakeCache>()
                .len(),
            1,
            "the cache was cleared rather than grown: the key names the graph"
        );
    }

    #[test]
    fn an_untouched_library_is_not_re_baked_however_long_the_preview_runs() {
        let directory = TempDir::new().expect("temp dir");
        let path = directory.path().join("materials.graphs.ron");
        write(&path, &library(1));
        let mut app = harness(&path);
        for _ in 0..8 {
            app.update();
        }
        assert!(
            texture(&app).is_none(),
            "a poll that finds the same stamp bakes nothing"
        );
        assert!(!app.world().resource::<Rebake>().0);
    }

    #[test]
    fn a_library_that_stopped_parsing_leaves_the_preview_showing_what_it_had() {
        let directory = TempDir::new().expect("temp dir");
        let path = directory.path().join("materials.graphs.ron");
        write(&path, &library(1));
        let mut app = harness(&path);
        app.update();

        // Half a file, which is what a save in progress looks like.
        std::fs::write(&path, "(graphs: {\"test:wall\": (id: \"test:").expect("partial write");
        app.update();
        assert_eq!(
            app.world().resource::<crate::Graphs>().0,
            library(1),
            "the graphs in memory survived a file that does not parse"
        );
        assert!(texture(&app).is_none(), "and nothing was re-baked from it");

        // And the next good save is still picked up: the poll does not give up
        // on a file because it was once broken.
        write(&path, &library(4));
        app.update();
        assert_eq!(app.world().resource::<crate::Graphs>().0, library(4));
        assert!(texture(&app).is_some());
    }

    #[test]
    fn a_save_writes_the_library_and_the_watcher_does_not_read_it_back() {
        let directory = TempDir::new().expect("temp dir");
        let path = directory.path().join("materials.graphs.ron");
        write(&path, &library(1));
        let mut app = harness(&path);
        app.update();

        // An edit the panel would have made, then a save over the source.
        app.world_mut().resource_mut::<crate::Graphs>().0 = library(5);
        let mut source = app.world_mut().resource_mut::<GraphSource>();
        let written = save(&crate::Graphs(library(5)), &mut source).expect("save");
        assert_eq!(written, path);
        assert_eq!(
            materials::read_graphs(&path).expect("what was written validates"),
            library(5)
        );

        // Saving moved the file's timestamp, and the stamp was taken again with
        // it: a preview that re-baked its own save would pay a bake for nothing.
        app.update();
        assert!(
            texture(&app).is_none(),
            "the watcher did not treat the panel's own save as an edit"
        );
    }

    #[test]
    fn graphs_out_sends_a_save_somewhere_else_and_leaves_the_source_alone() {
        let directory = TempDir::new().expect("temp dir");
        let path = directory.path().join("materials.graphs.ron");
        let out = directory.path().join("tuned.graphs.ron");
        write(&path, &library(1));
        let mut source = GraphSource::new(Some(path.clone()), Some(out.clone()));
        assert_eq!(source.destination(), Some(out.as_path()));
        assert_eq!(
            save(&crate::Graphs(library(6)), &mut source).expect("save"),
            out
        );
        assert_eq!(
            materials::read_graphs(&path).expect("the source still parses"),
            library(1),
            "a save with --graphs-out does not touch the file it read"
        );
        assert_eq!(
            materials::read_graphs(&out).expect("the export"),
            library(6)
        );
    }

    #[test]
    fn a_scene_that_brought_no_graph_library_has_nowhere_to_save_and_says_so() {
        let mut source = GraphSource::default();
        assert_eq!(source.destination(), None);
        let error = save(&crate::Graphs::default(), &mut source)
            .expect_err("nothing to save to")
            .to_string();
        assert!(error.contains("--graphs"), "{error}");
    }
}
