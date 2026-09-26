//! The Bevy adapter for `ashlar`: load baked buildings and materials into a
//! game, or upload a meshed building yourself.
//!
//! This is the top of the `ashlar` stack and the only member that sees Bevy.
//! It pairs with one Bevy release at a time; this is the 0.19 adapter.
//!
//! # Start here: the game path
//!
//! A game ships files a content step baked (`ashlar-content`): `.ashlar`
//! buildings with every level of detail, and a `library.materials.ron` naming
//! KTX2 texture sets. Add [`AshlarPlugin`](prelude::AshlarPlugin), spawn an
//! [`AshlarBuilding`](prelude::AshlarBuilding), and the plugin does the rest:
//! every level with its distance band, collision proxies, rooms and portals.
//! Everything a game names is in the [`prelude`].
//!
//! ```no_run
//! use ashlar_bevy::prelude::*;
//! use bevy::prelude::*;
//!
//! fn setup(mut commands: Commands, server: Res<AssetServer>) {
//!     commands.spawn((
//!         AshlarBuilding {
//!             building: server.load("buildings/outpost.ashlar"),
//!             materials: server.load("materials/library.materials.ron"),
//!         },
//!         Transform::default(),
//!     ));
//! }
//!
//! App::new()
//!     .add_plugins((DefaultPlugins, AshlarPlugin::default()))
//!     .add_systems(Startup, setup)
//!     .run();
//! ```
//!
//! [`baked`] documents the plugin in full, and `docs/guide/integration.md` in
//! the repository walks the whole path from a content crate to a game.
//!
//! # What a game compiles, and what a tool compiles
//!
//! Everything above, and the building blocks below, is what the crate does
//! with **no cargo features at all**, which is the shape a game should link.
//! With default features this crate does not depend on `ashlar-material`, so
//! there is no graph engine in a game's tree, and no geometry kernel either.
//!
//! One more feature is a game feature: `strands` grows grass and moss from a
//! baked strand set (`strands`, `cards`, `wind`), and pulls only
//! `ashlar-strands`.
//!
//! The *graph* half is a tool half, behind four features that are all off by
//! default. `runtime-bake` bakes a [`ashlar::Surface::Graph`] into `Image`
//! assets when the material is created (the `runtime_bake` module); `gpu-bake`
//! runs that same bake on the render device (`gpu`); `shader` compiles a
//! [`ashlar::Surface::Shader`] into a generated fragment instead of baking it
//! (`shader`); and `strand-scatter` scatters a strand set out of a graph rather
//! than reading one off disk. Those modules are named as text rather than
//! linked, because a default build has nothing for a link to point at. What
//! turns them on is `tools/ashlar-preview`, a content step that writes assets,
//! and this crate's own tests — not a game at play, where a bake costs hundreds
//! of milliseconds in front of a frame.
//!
//! The crate's own README has the table.
//!
//! # Building blocks: uploading a meshed building yourself
//!
//! A tool, or a game that meshes at runtime (damage, say), holds a
//! [`ashlar::MeshedBuilding`] rather than a file. These functions are what the
//! plugin is written on.
//!
//! [`read_library`] validates a material library and opens every texture it names
//! before a window exists, so a missing or miscoloured map is a startup error
//! rather than a black surface. [`create_material`] then makes the PBR material, and
//! [`mesh`] uploads the triangles. [`drawables`] is all three over a whole
//! meshed building, deduplicated.
//!
//! A definition's [`ashlar::Surface`] decides what those two do: `Plain` is
//! constants, `Files` names files on disk, and `Graph` and `Shader` name a
//! material graph. A `Graph` surface is baked into `Image` assets when the
//! material is created — that is `runtime_bake::create_graph_material`, which takes the
//! graph library, a `runtime_bake::BakeCache` and `Assets<Image>` beside what
//! [`create_material`] takes, and shares one texture set between every definition asking
//! for the same [`ashlar::Bake`]. A `Shader` surface is *partitioned* instead:
//! everything that does not move is baked into the same cache, the rest is
//! emitted as WGSL, and what comes out is a `shader::ProceduralMaterial` —
//! a `StandardMaterial` with a generated fragment over it. That is
//! `shader::create_shader_material`, and `shader::ProceduralMaterialPlugin` is what
//! teaches an app to draw one. All of that is behind the features above.
//!
//! An *instance* may put its own parameter values on the graph surface a slot
//! binds — `ashlar::Binding` is what carries them — and [`definition`] is where
//! the two meet: it answers the definition a binding names with those values
//! written into it, so one material key wears a seed per building. A `Graph`
//! override is a distinct `runtime_bake::BakeKey` and so a distinct texture set,
//! shared by every instance that asked for the same one; a `Shader` override of
//! a *live* parameter is the same pipeline and the same bound textures with a
//! different uniform block, and of a folded one is a second compiled graph.
//!
//! A whole *building* is [`dressed_pieces`], which walks what a mesher produced and
//! hands back each drawable piece with the definition it wears already
//! resolved. That is the join between the two halves — geometry declares a
//! slot, an instance's palette binds it, and a library answers what the binding
//! names — and writing it out by hand is how a renderer ends up keying its
//! materials by the key rather than by the binding.
//!
//! A definition may also grow *geometry* out of the graph it names.
//! [`ashlar::StrandSettings`] is what asks for it — which of the graph's strand
//! layers, how densely, how far out — and `strands` is what answers, behind the
//! feature of that name: it scatters the layer, plants the roots on the
//! surface's own triangles, and hands back one [`Mesh`] per UV repeat plus the
//! `StandardMaterial` that draws them. `read_library_with_graphs` preflights the layer
//! names against the graph, as it preflights everything else a definition names.
//!
//! A texture file is a PNG or a KTX2, and the difference matters: a KTX2 map
//! carries its own mip chain, which is what a baked material needs to survive
//! being looked at from far away, and a PNG cannot. Both are preflighted and
//! both load through Bevy's own loader, which reads the colour space from the
//! settings [`create_material`] sets per slot.
//!
//! ```no_run
//! use ashlar::{MaterialLibrary, MeshedBuilding};
//! use bevy::prelude::*;
//!
//! /// Whatever the mesher produced, kept out of the render layer until now.
//! #[derive(Resource)]
//! struct Shell {
//!     meshed: MeshedBuilding,
//!     library: MaterialLibrary,
//! }
//!
//! fn spawn(
//!     mut commands: Commands,
//!     shell: Res<Shell>,
//!     server: Res<AssetServer>,
//!     mut meshes: ResMut<Assets<Mesh>>,
//!     mut materials: ResMut<Assets<StandardMaterial>>,
//! ) -> Result {
//!     let mut cx = ashlar_bevy::UploadContext {
//!         server: &server,
//!         meshes: &mut meshes,
//!         materials: &mut materials,
//!     };
//!     for drawable in ashlar_bevy::drawables(&shell.meshed, &shell.library, &mut cx)? {
//!         commands.spawn((
//!             Mesh3d(drawable.mesh),
//!             MeshMaterial3d(drawable.material),
//!             drawable.transform,
//!         ));
//!     }
//!     Ok(())
//! }
//!
//! App::new()
//!     .add_plugins(DefaultPlugins)
//!     .add_systems(Startup, spawn)
//!     .run();
//! ```
// `doc_cfg` is what puts "Available on crate feature `shader`" on a gated item,
// so the documentation of a crate whose whole tool half is optional reads as
// one page rather than as four builds. Nightly-only, and docs.rs is where it is
// turned on; a stable build ignores the attribute and loses only the badge.
// It was `doc_auto_cfg` until nightly folded that into `doc_cfg` and made the
// old name a hard error (checked on nightly 2026-06-27).
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod baked;
#[cfg(feature = "strands")]
pub mod cards;

/// What a game names: the plugin, the component that asks for a building, and
/// what a spawned building carries.
pub mod prelude {
    pub use crate::baked::{
        AshlarBuilding, AshlarBuildingSpawned, AshlarCollider, AshlarFailed, AshlarPiece,
        AshlarPlugin, AshlarSpaces, AshlarSpawned, BakedBuildingAsset, Bands, MaterialLibraryAsset,
    };
}

/// The recipe crate, re-exported so a game names `ashlar_bevy::ashlar::…` and
/// needs no second dependency pinned to the same version.
pub use ashlar;
#[cfg(feature = "gpu-bake")]
pub mod gpu;
#[cfg(feature = "runtime-bake")]
pub mod runtime_bake;
#[cfg(feature = "shader")]
pub mod shader;
#[cfg(feature = "strands")]
pub mod strands;
#[cfg(feature = "strands")]
pub mod wind;

use std::{borrow::Cow, collections::BTreeMap, path::Path};

use anyhow::{Context, Result, ensure};
use ashlar::MaterialLibrary;
#[cfg(feature = "runtime-bake")]
use ashlar_material::{MaterialGraphLibrary, bake::BakeRequest};
use bevy::{
    image::{
        ImageAddressMode, ImageFilterMode, ImageLoaderSettings, ImageSampler,
        ImageSamplerDescriptor,
    },
    prelude::*,
};

/// Read a material graph library from RON and validate every graph in it.
///
/// The companion to [`read_library_with_graphs`]: a library of surfaces that names
/// graphs wants the graphs beside it, and both are checked before the window
/// opens. Validation here is the whole library rather than the graphs some
/// surface happens to name, because a graph library is authored as a unit and a
/// graph nothing names today is named tomorrow.
#[cfg(feature = "runtime-bake")]
pub fn read_graphs(path: &Path) -> Result<MaterialGraphLibrary> {
    let graphs: MaterialGraphLibrary = ron::from_str(
        &std::fs::read_to_string(path)
            .with_context(|| format!("reading material graph library {}", path.display()))?,
    )?;
    graphs
        .check()
        .with_context(|| format!("validating material graph library {}", path.display()))?;
    Ok(graphs)
}

/// Validate and lower one bake against `graphs`, without rasterising a texel.
///
/// This is what makes a broken `Surface::Graph` a startup error the way a
/// missing PNG already is: an unknown graph key, a parameter the graph does not
/// declare, a cycle, a free field or a node this backend cannot lower are all
/// refused here, at the same cost as compiling the graph and none of the cost
/// of baking it.
#[cfg(feature = "runtime-bake")]
fn preflight_graph(bake: &ashlar::Bake, graphs: &MaterialGraphLibrary) -> Result<()> {
    let graph = runtime_bake::graph_of(&bake.graph, graphs)?;
    ashlar_material::bake::preflight(&BakeRequest {
        graph,
        library: graphs,
        params: &bake.params,
        resolution: bake.resolution,
        mips: false,
        threads: None,
    })
    .with_context(|| format!("material graph {:?}", bake.graph))?;
    Ok(())
}

/// What a material definition's tiling says about the repeat the graph it names
/// was authored for.
///
/// The two halves of the seam carry one number twice: a graph is drawn per
/// repeat and may say how many metres that repeat is
/// (`ashlar_material::MaterialGraph::tile_metres`), and a definition says how
/// many metres the repeat is stretched over on a wall
/// ([`ashlar::MaterialDefinition::tile_metres`]). This is the comparison, and
/// it has three answers rather than two because only one kind of disagreement
/// is a mistake.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tiling {
    /// Nothing to report: the graph declares no repeat, or the definition
    /// tiles at the one it declares.
    Agreed,
    /// The same repeat at a different size, by this one factor on both axes.
    ///
    /// Not a fault in itself. Everything the graph draws scales with the
    /// repeat, relief included, so a definition at a third of the declared
    /// tiling is that material drawn a third of the size — which is exactly how
    /// a specimen sheet fakes three times the distance without moving the
    /// camera. The one thing that does *not* scale with it is a strand layer,
    /// whose lengths are absolute metres. Whether a scale this raw needs a
    /// warning still depends on whether the definition's own
    /// [`ashlar::MaterialDefinition::tile_scale`] declares the factor on
    /// purpose — this variant only says the axes agree, it does not read that
    /// field; `preflight_tiling` is where the two meet.
    Scaled(f32),
    /// A repeat of a different shape: this factor on each axis, and the two do
    /// not agree.
    ///
    /// The disagreement that is nearly always a mistake. A graph lays a fixed
    /// number of bricks, threads or flagstones across its repeat, and its
    /// relief is metres of that repeat; a repeat that is wider in one axis than
    /// the graph was drawn for stretches all of it.
    Stretched([f32; 2]),
}

/// Compare what a definition tiles at with the repeat its graph was drawn for.
///
/// Pure, so that the rule can be read and tested without a library, a renderer
/// or a log: `declared` is what the graph says, or `None` for the graph that
/// says nothing, and `definition` is the material definition's own tiling. The
/// answer is the factor the definition is off by, per axis, classified into the
/// three cases [`Tiling`] names.
///
/// The tolerance is a thousandth, relative: a repeat is a number an author
/// types in metres, and two of them that agree to a millimetre in a metre are
/// the same number written twice rather than a disagreement. Anything the
/// arithmetic cannot make sense of — a factor that comes back infinite or NaN
/// from data neither side validated — falls out as [`Tiling::Stretched`], which
/// is the answer that gets looked at.
#[must_use]
pub fn tiling(declared: Option<[f32; 2]>, definition: [f32; 2]) -> Tiling {
    let Some(declared) = declared else {
        return Tiling::Agreed;
    };
    let factors = [definition[0] / declared[0], definition[1] / declared[1]];
    if near(factors[0], 1.0) && near(factors[1], 1.0) {
        Tiling::Agreed
    } else if near(factors[0], factors[1]) {
        // The axes agree to within the tolerance, so either one is the factor.
        Tiling::Scaled(factors[0])
    } else {
        Tiling::Stretched(factors)
    }
}

/// Whether two repeat sizes are one number written twice.
///
/// The finite test is what keeps a division by zero from reading as agreement:
/// an infinite difference is not greater than an infinite tolerance, so the
/// comparison alone would answer yes.
fn near(a: f32, b: f32) -> bool {
    let difference = (a - b).abs();
    difference.is_finite() && difference <= 1e-3 * a.abs().max(b.abs())
}

/// What a uniform [`Tiling::Scaled`] factor means once a definition's own
/// declared intent, and whether it grows strands, are read alongside it.
///
/// Pure and separate from `preflight_tiling` so the three ways a scale can
/// be judged are a fact about three plain values rather than about a graph
/// library — a factor, the [`ashlar::MaterialDefinition::tile_scale`] the
/// definition declared if any, and whether it grows strands. Strands are
/// checked first and unconditionally: a blade's length is absolute metres, so
/// a `tile_scale` that excuses the texture cannot excuse the geometry too.
/// Public and pure for the same reason [`Tiling`] is: a caller checking a
/// library's own content — without a graph library, a renderer or a log —
/// reads exactly the rule `preflight_tiling` warns by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScaleVerdict {
    /// The factor matches what the definition declared on purpose, and
    /// nothing grows from the graph that a declared texture scale cannot
    /// excuse.
    Declared,
    /// Whatever the definition declares, a strand's length does not scale
    /// with the repeat its texture is laid at.
    Strands,
    /// No `tile_scale` excuses this factor, or the one declared does not
    /// match it: the shipped case is an honest typo, indistinguishable from a
    /// deliberate scale until somebody reads it.
    Undeclared,
}

/// Classify a uniform scale, at the same relative tolerance [`tiling`] itself
/// compares repeats with.
#[must_use]
pub fn scale_verdict(factor: f32, declared_scale: Option<f32>, has_strands: bool) -> ScaleVerdict {
    if has_strands {
        ScaleVerdict::Strands
    } else if declared_scale.is_some_and(|scale| near(factor, scale)) {
        ScaleVerdict::Declared
    } else {
        ScaleVerdict::Undeclared
    }
}

/// Say where a definition tiles at a repeat its graph was not drawn for.
///
/// A warning except where the disagreement is declared on purpose. The
/// material sheet and the benchmark both divide the tiling to fake distance,
/// and the corporate library sizes a few walls to a whole number of repeats a
/// storey rather than the graph's own — both write
/// [`ashlar::MaterialDefinition::tile_scale`] to say the factor is
/// deliberate, and a uniform scale that matches it is silent, not even at
/// debug. What no declaration excuses is changing the *shape* of the repeat,
/// which stretches relief the graph authored in metres, or scaling a repeat
/// it grows strands from, because a blade's length is absolute metres and
/// stays the length it was authored at while everything around it changes
/// size — that warns regardless of what `tile_scale` says. And a uniform
/// scale with no `tile_scale`, or one that does not match the factor, is
/// exactly the honest typo the field exists to catch: 2.4 typed for 1.72
/// looks exactly like the sheet's own deliberate ninth until somebody reads
/// it, so that warns too, naming the fix. A graph that declares no repeat at
/// all says nothing whatever a definition does — no shipped library declared
/// one before the field existed, and a warning per definition on every one of
/// them would be noise with no fix in it.
#[cfg(feature = "runtime-bake")]
fn preflight_tiling(
    material: &str,
    definition: &ashlar::MaterialDefinition,
    graphs: &MaterialGraphLibrary,
) {
    let Some(key) = definition.surface.graph() else {
        return;
    };
    // An unknown key is the error `preflight_graph` and `preflight_strands`
    // raise with the context this has no business duplicating.
    let Some(declared) = graphs.get(key).and_then(|graph| graph.tile_metres) else {
        return;
    };
    let [u, v] = definition.tile_metres;
    let [du, dv] = declared;
    match tiling(Some(declared), definition.tile_metres) {
        Tiling::Agreed => {}
        Tiling::Scaled(factor) => {
            match scale_verdict(factor, definition.tile_scale, definition.strands.is_some()) {
                ScaleVerdict::Declared => {}
                ScaleVerdict::Strands => {
                    tracing::warn!(
                        "material {material:?} grows strands from graph {key:?} and tiles it at \
                     {u} x {v} m rather than the {du} x {dv} m it was authored for, a factor \
                     of {factor:.3}. A strand's length is absolute metres, so its blades stay \
                     the size the graph drew them while the surface under them changes size."
                    );
                }
                ScaleVerdict::Undeclared => {
                    let says = match definition.tile_scale {
                        Some(scale) => {
                            format!("declares `tile_scale: Some({scale})`, which does not match")
                        }
                        None => "declares no `tile_scale`".to_owned(),
                    };
                    tracing::warn!(
                        "material {material:?} tiles graph {key:?} at {u} x {v} m rather than the \
                     {du} x {dv} m it was authored for, a factor of {factor:.3}, and the \
                     definition {says} it. Fix `tile_metres` if this is a typo, or set \
                     `tile_scale: Some({factor:.3})` if the graph is meant to lay at this \
                     size on purpose."
                    );
                }
            }
        }
        Tiling::Stretched([fu, fv]) => {
            tracing::warn!(
                "material {material:?} tiles graph {key:?} at {u} x {v} m against the \
                 {du} x {dv} m it was authored for: {fu:.3} across and {fv:.3} down, so \
                 the repeat is a different shape rather than a different size. Relief and \
                 strand lengths in that graph are metres of one repeat and cannot follow \
                 that."
            );
        }
    }
}

/// The definition a binding names, dressed in that binding's own parameter
/// overrides.
///
/// The one call a caller makes between "which material does this slot bind"
/// and "make me that material": the definition itself for the ordinary slot
/// that names a key and nothing else, and a copy carrying the instance's
/// values otherwise. Everything below this — [`create_material`],
/// `runtime_bake::create_graph_material`, `shader::create_shader_material` and the two caches
/// behind them — deals in definitions and never learns that an override
/// happened, which is what makes a per-instance value a distinct texture set
/// or a distinct uniform block without a distinct material key.
///
/// Both failures are ones [`read_library`] already refused, so a caller past preflight
/// is reporting a bug rather than bad content: a binding naming no definition,
/// and an override on a surface that binds no parameters.
pub fn definition<'a>(
    library: &'a MaterialLibrary,
    binding: &ashlar::Binding,
) -> Result<Cow<'a, ashlar::MaterialDefinition>> {
    let definition = library
        .materials
        .get(&binding.material)
        .with_context(|| format!("no material definition {:?}", binding.material))?;
    definition.overridden(binding).with_context(|| {
        format!(
            "material {:?} binds no graph parameters, so an instance cannot override {}",
            binding.material,
            binding
                .params
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

/// One drawable piece of a meshed building with the material it wears already
/// resolved.
///
/// The two halves of the seam in one value: [`ashlar::Piece`] is what the
/// mesher and the recipe answer between them — which element of which instance,
/// or which group batch, where it stands, which triangles, which binding — and
/// the definition is what only a crate holding the library can answer.
#[derive(Clone, Debug)]
pub struct DressedPiece<'a> {
    /// Geometry and binding, as [`ashlar::MeshedBuilding::pieces`] walks them.
    pub piece: ashlar::Piece<'a>,
    /// The definition that binding names, dressed in the binding's own
    /// parameter values: exactly what [`definition`] answers.
    pub definition: Cow<'a, ashlar::MaterialDefinition>,
}

/// Every drawable piece of a meshed building, each with its definition resolved.
///
/// The whole join, so a game writes one loop rather than rediscovering the
/// assembly order: instances, the elements of the part each one places, the
/// binding the palette resolves for the slot that element declared — the cut
/// slot for a cut batch — and the definition that binding names with the
/// instance's own values written into it.
///
/// Lazy, and cheap per piece only because it is: a definition overriding
/// nothing is borrowed from the library, so the copies are the pieces an
/// instance actually varied. What a caller keys its *assets* by is still the
/// binding rather than the material key, because two values of one parameter
/// are two texture sets — [`ashlar::Piece::shared_key`] says which pieces share
/// the work derived from both halves.
///
/// Fallible per piece rather than up front, because a failure is a definition a
/// binding names and [`read_library`] already refused: a caller past preflight is
/// reporting a bug rather than handling bad content.
pub fn dressed_pieces<'a>(
    meshed: &'a ashlar::MeshedBuilding,
    library: &'a MaterialLibrary,
) -> impl Iterator<Item = Result<DressedPiece<'a>>> {
    meshed.pieces().map(move |piece| {
        let definition = definition(library, piece.binding)
            .with_context(|| format!("material {}", piece.binding))?;
        Ok(DressedPiece { piece, definition })
    })
}

/// The borrows [`drawables`] needs to turn a meshed building into assets.
///
/// A plain bundle of the three things a Bevy system already has, in the shape
/// [`create_material`] takes its own context: an [`AssetServer`] to load texture files
/// through, and the two asset collections the meshes and the materials go into.
/// Not a [`SystemParam`](bevy::ecs::system::SystemParam) and not a plugin,
/// because a caller that wants to spawn a building at some other moment —
/// a loading screen, a streaming system, a test with no schedule at all —
/// should not have to be in a system shaped a particular way to do it.
pub struct UploadContext<'a> {
    /// Where a `Surface::Files` map is loaded from, with the colour space
    /// and the sampler [`create_material`] sets per slot.
    pub server: &'a AssetServer,
    /// Where the uploaded geometry goes: one entry per distinct
    /// [`ashlar::MeshKey`], so one for every element batch however many
    /// instances wear it, and one for every group batch.
    pub meshes: &'a mut Assets<Mesh>,
    /// Where the materials go: one entry per distinct [`ashlar::Binding`].
    pub materials: &'a mut Assets<StandardMaterial>,
}

/// One drawable piece of a building with its assets resolved: what a caller
/// spawns.
///
/// The end of the join [`dressed_pieces`] begins. `piece` is the identity — which
/// element of which instance, or which group batch — kept so that a caller can
/// name the entity, attach its own components, or find the piece again later;
/// the other three are what Bevy needs to draw it.
#[derive(Clone, Debug)]
pub struct Drawable<'a> {
    /// Which piece this is, as [`ashlar::MeshedBuilding::pieces`] walks them.
    pub piece: ashlar::Piece<'a>,
    /// The uploaded geometry, shared with every other piece over the same
    /// [`ashlar::MeshKey`]: an element batch across every placement of its part,
    /// and a group batch with nothing, because it is meshed in building space
    /// and belongs to no reusable part.
    pub mesh: Handle<Mesh>,
    /// The material, shared with every other piece wearing the same binding.
    pub material: Handle<StandardMaterial>,
    /// Where the piece stands, as its pose in building-local metres: an
    /// instance's pose over part-local triangles, and the identity over a group
    /// batch already meshed in building space.
    pub transform: Transform,
}

/// What to say when a definition names a graph and this build cannot bake one.
///
/// Two messages rather than one because the reader is in two different
/// situations: without the feature the fix is a line in `Cargo.toml`, and with
/// it the fix is a different call — [`drawables`] deliberately takes neither
/// `Assets<Image>` nor a bake cache, so it could not bake a graph even where
/// the code to do it is compiled in.
#[cfg(not(feature = "runtime-bake"))]
const GRAPH_HELP: &str = "this is the baked-files path, and a graph is baked by a tool: ship the \
                          baked texture set as a `Files` surface, or enable `ashlar-bevy`'s \
                          `runtime-bake` feature (`shader`, for a `Shader` surface) and create \
                          the material through `runtime_bake::create_graph_material`";
/// The same thing said to a build that *has* the bake path. See above.
#[cfg(feature = "runtime-bake")]
const GRAPH_HELP: &str = "this is the baked-files path and takes no bake cache: create the \
                          material through `runtime_bake::create_graph_material`, which takes the graph \
                          library, a `BakeCache` and `Assets<Image>` beside what this takes";

/// Refuse a definition whose surface this path does not draw.
///
/// The check [`drawables`] and [`batch_drawables`] both make and must not
/// disagree on: `Plain` and `Files` are the baked-files path, and a `Graph`
/// or `Shader` surface names a material graph this call has no cache or image
/// store to bake. `label` names the piece the binding was found on, so the
/// error says where rather than only what.
pub(crate) fn files_only(
    definition: &ashlar::MaterialDefinition,
    binding: &ashlar::Binding,
    label: &str,
) -> Result<()> {
    match definition.surface {
        ashlar::Surface::Plain | ashlar::Surface::Files { .. } => Ok(()),
        ashlar::Surface::Graph(_) | ashlar::Surface::Shader { .. } => {
            anyhow::bail!("material {binding} on {label} names a material graph: {GRAPH_HELP}")
        }
    }
}

/// Resolve the definition a binding names and refuse a surface this path cannot
/// draw.
///
/// The resolve-and-refuse half of the material step [`drawables`] and
/// [`batch_drawables`] share: [`definition`] for the definition a binding names
/// with its own parameter values, then [`files_only`] for the surface. What
/// comes back is what [`create_material`] takes.
fn checked<'a>(
    library: &'a MaterialLibrary,
    binding: &ashlar::Binding,
    label: &str,
) -> Result<Cow<'a, ashlar::MaterialDefinition>> {
    let definition = definition(library, binding)?;
    files_only(&definition, binding, label)?;
    Ok(definition)
}

/// The material a binding wears on the baked-files path, created on a miss of
/// the caller's cache.
///
/// The whole step [`drawables`] and [`batch_drawables`] share: resolve the
/// definition a binding names, refuse a surface that names a material graph,
/// and create the [`StandardMaterial`] once per distinct binding. `created` is
/// the cache from binding to handle, so a wall drawn once and hit ten times
/// creates one material rather than eleven; keeping it across a hit is the
/// caller's, because a hit is a second call rather than a second building.
fn material_for(
    library: &MaterialLibrary,
    binding: &ashlar::Binding,
    label: &str,
    created: &mut BTreeMap<ashlar::Binding, Handle<StandardMaterial>>,
    cx: &mut UploadContext<'_>,
) -> Result<Handle<StandardMaterial>> {
    if let Some(material) = created.get(binding) {
        return Ok(material.clone());
    }
    let definition = checked(library, binding, label)?;
    let material = cx.materials.add(create_material(&definition, cx.server));
    created.insert(binding.clone(), material.clone());
    Ok(material)
}

/// Every drawable piece of a meshed building, uploaded and deduplicated.
///
/// The whole featureless path in one call, and the one a game writes: it walks
/// [`dressed_pieces`], uploads each distinct mesh once with [`mesh`], creates each
/// distinct material once with [`create_material`], and hands back what to spawn. What a
/// caller does with the list is its own business — spawn each as an entity,
/// parent them, or keep them.
///
/// Deduplicated the two ways that matter, and they are not the same way.
/// Geometry is shared by [`ashlar::MeshKey`]: an element batch is one upload for
/// the whole part however many times it is placed, because a part is evaluated
/// once and the placement is a transform applied afterwards, and a group batch —
/// meshed in building space and belonging to no reusable part — is one upload
/// shared with nothing. Materials are shared by the whole [`ashlar::Binding`]
/// rather than by the material key, because an instance may write its own
/// parameter values over the definition a key names, and two values of one
/// parameter are two materials. Keying materials by the key is the bug this
/// function exists to stop a caller writing.
///
/// A batch with no triangles is not in the list. An element a cutter removed
/// entirely still binds a material and [`dressed_pieces`] still yields it — that is
/// the documented behaviour and it is the right one, because whether an empty
/// batch is worth anything depends on what the caller is doing — but there is
/// nothing to draw and nothing to upload, so it is dropped here. Its binding is
/// still resolved, so a definition an empty batch names is still an error
/// rather than a silently skipped one.
///
/// Fallible rather than panicking, and the errors are the two a preflighted
/// library cannot produce: a binding naming no definition, which [`read_library`]
/// already refused, and a definition naming a material *graph*, which this path
/// does not bake. The second names the feature that would.
pub fn drawables<'a>(
    meshed: &'a ashlar::MeshedBuilding,
    library: &'a MaterialLibrary,
    cx: &mut UploadContext<'_>,
) -> Result<Vec<Drawable<'a>>> {
    let mut uploaded: BTreeMap<ashlar::MeshKey, Handle<Mesh>> = BTreeMap::new();
    let mut created: BTreeMap<ashlar::Binding, Handle<StandardMaterial>> = BTreeMap::new();
    let mut drawn = Vec::new();
    for surface in dressed_pieces(meshed, library) {
        let DressedPiece { piece, definition } = surface?;
        // Before the empty check, so that a batch with nothing to draw still
        // reports a surface this path cannot take.
        files_only(&definition, piece.binding, &piece.label())?;
        // After the definition and not before it, so that a fully subtracted
        // element still reports a binding nothing answers.
        if piece.mesh.positions.is_empty() {
            continue;
        }
        let mesh =
            match uploaded.entry(piece.mesh_key()) {
                std::collections::btree_map::Entry::Occupied(entry) => entry.get().clone(),
                std::collections::btree_map::Entry::Vacant(entry) => entry
                    .insert(cx.meshes.add(
                        mesh(piece.mesh).with_context(|| format!("uploading {}", piece.label()))?,
                    ))
                    .clone(),
            };
        let material = material_for(library, piece.binding, &piece.label(), &mut created, cx)?;
        drawn.push(Drawable {
            piece,
            mesh,
            material,
            transform: Transform {
                translation: piece.pose.translation.as_vec3(),
                rotation: piece.pose.rotation.as_quat(),
                ..default()
            },
        });
    }
    Ok(drawn)
}

/// One batch of a re-meshed group, or of a piece of debris, with its assets
/// resolved: what a caller spawns after a hit.
///
/// The post-hit twin of [`Drawable`], and smaller by the one field a group
/// batch does not have: a batch is meshed in building space and belongs to no
/// reusable part, so where it stands is the building's own transform and not a
/// placement this can carry.
#[derive(Clone, Debug)]
pub struct BatchDrawable<'a> {
    /// The batch this came from: the binding and side every face wears, and the
    /// building-space triangles, kept so a caller can name the entity or find
    /// the batch again.
    pub batch: &'a ashlar::GroupBatch,
    /// The uploaded geometry, fresh for this batch. Unique: a group batch is
    /// meshed in building space and shared with nothing, so there is no key for
    /// two of them to collide on and no upload a hit could reuse.
    pub mesh: Handle<Mesh>,
    /// The material for the batch's binding, shared with every other batch
    /// wearing that binding through the caller's cache.
    pub material: Handle<StandardMaterial>,
}

/// Upload the batches a hit produced, resolving each binding's material.
///
/// The half of a hit this crate owns, and this crate computes no hit: a game
/// computes one wherever it chose to — a task linking `ashlar-manifold`, or a
/// server — and hands the re-meshed group's batches, or a debris piece's, here.
/// What comes back is what to spawn in place of the entities the group stood
/// as.
///
/// Geometry is uploaded fresh, because a hit changed it: a group batch is
/// meshed in building space and belongs to no reusable part, so there is
/// nothing to share a cache with and a cached handle to pre-hit triangles would
/// draw the wrong surface. Materials are *not* fresh. `materials` is the
/// caller's cache from binding to handle, read first and filled on a miss, so a
/// wall hit ten times creates one material rather than ten; remembering the
/// cache between hits is what makes that true, and a hit is one cache kept
/// beside the building it damaged.
///
/// The batches are in building space, so this returns no transform: where the
/// building stands is the only placement a batch has. An empty batch is
/// skipped, because there is nothing to upload and nothing to draw.
///
/// Fallible for the same two reasons [`drawables`] is, which a preflighted
/// library cannot produce: a binding naming no definition, and a definition
/// naming a material *graph*, which this path does not bake.
pub fn batch_drawables<'a>(
    batches: &'a [ashlar::GroupBatch],
    library: &'a MaterialLibrary,
    materials: &mut BTreeMap<ashlar::Binding, Handle<StandardMaterial>>,
    cx: &mut UploadContext<'_>,
) -> Result<Vec<BatchDrawable<'a>>> {
    let mut drawn = Vec::new();
    for batch in batches {
        if batch.mesh.positions.is_empty() {
            continue;
        }
        let material = material_for(library, &batch.binding, "a group batch", materials, cx)?;
        let mesh = cx.meshes.add(
            mesh(&batch.mesh)
                .with_context(|| format!("uploading a batch wearing {}", batch.binding))?,
        );
        drawn.push(BatchDrawable {
            batch,
            mesh,
            material,
        });
    }
    Ok(drawn)
}

/// Preflight the graph parameters one instance overrides on one slot.
///
/// The half of an override this layer owns, and the reason it is here rather
/// than in `ashlar`: [`ashlar::MaterialLibrary::check_for`] can see that a
/// binding names a `Graph` or `Shader` surface, and only a crate holding the
/// *graph* library can see whether `variation` is a parameter that graph
/// declares, or takes a colour rather than a float. So the overridden surface
/// is lowered exactly as the definition's own was, and what comes back is the
/// same error at the path of the binding that asked.
#[cfg(feature = "runtime-bake")]
fn preflight_override(
    path: &str,
    definition: &ashlar::MaterialDefinition,
    binding: &ashlar::Binding,
    graphs: &MaterialGraphLibrary,
) -> Result<()> {
    // `check_for` has already refused an override on a surface that binds no
    // parameters, and it ran before this.
    let dressed = definition
        .overridden(binding)
        .with_context(|| format!("{path}: this surface binds no graph parameters"))?;
    match &dressed.surface {
        ashlar::Surface::Graph(bake) => preflight_graph(bake, graphs),
        // Only the crate compiled with `shader` holds the partitioner that
        // would lower this one, so a build without it lets the definition load
        // and refuses it where it would be *created* instead — which is
        // [`drawables`], naming the feature. Preflight is allowed to check less
        // than it could; it is never allowed to pass something that will then
        // draw wrong.
        #[cfg(feature = "shader")]
        ashlar::Surface::Shader { graph, params } => {
            shader::preflight_shader(graph, params, graphs)
        }
        #[cfg(not(feature = "shader"))]
        ashlar::Surface::Shader { .. } => Ok(()),
        ashlar::Surface::Plain | ashlar::Surface::Files { .. } => Ok(()),
    }
    .with_context(|| format!("material {}", binding.material))
    .with_context(|| format!("instance override {path}"))
}

/// Validate a material library and open every texture file it names, before
/// renderer startup.
///
/// The game's preflight, and what the crate does with no features: the library
/// parses, [`ashlar::MaterialLibrary::check_for`] holds it against the building
/// that uses it, and every map a `Surface::Files` names is opened, checked
/// for the container its extension claims and refused if one key is used as
/// both sRGB colour and linear data. A missing or miscoloured map is then a
/// startup error naming the key rather than a black surface.
///
/// What it does *not* do is look at a material graph, because featureless there
/// is no graph engine here to look with — and because the surface a game ships
/// does not need one. A `Surface::Files` carries its `baked_from` as
/// provenance and loads the files beside it; a library naming graphs still
/// loads, and a `Graph` or `Shader` definition in it is refused where it would
/// be *created*, by [`drawables`], naming the feature that would bake it.
///
/// `read_library_with_graphs`, under `runtime-bake`, is this plus the graph half: it
/// lowers every `Graph`
/// surface, partitions every `Shader` one, checks every strand layer against
/// the graph that declares it and compares the repeats. That is the tool's
/// preflight, and it is where a broken graph becomes a startup error the way a
/// missing PNG already is.
pub fn read_library(
    path: &Path,
    asset_root: &Path,
    building: &ashlar::Building,
) -> Result<MaterialLibrary> {
    let library = parse_library(path, building)?;
    check_library(&library, asset_root, building)?;
    Ok(library)
}

/// [`read_library`] for a library already in memory: the one a game assembled in Rust,
/// such as `ashlar_material::stdlib::materials()` merged with its own.
///
/// # Errors
///
/// The first definition the building refuses, or the first file or strand set
/// that is missing or wrong, by key.
pub fn check_library(
    library: &MaterialLibrary,
    asset_root: &Path,
    building: &ashlar::Building,
) -> Result<()> {
    library.check_for(building)?;
    preflight_textures(library, asset_root)?;
    preflight_sets(library, asset_root)
}

/// [`read_library`], plus every graph the library names, validated and lowered.
///
/// `graphs` is the library a `Surface::Graph` names into, as [`read_graphs`]
/// reads it; a set of surfaces that are all files or constants may be
/// preflighted against an empty one.
///
/// Every *binding* is preflighted as well as every definition, because an
/// instance may override the graph parameters of the `Graph` or `Shader`
/// surface it names — one wall key at three seeds rather than three keys — and
/// a name the graph does not declare is a startup error naming the instance,
/// the slot and the parameter, exactly as a broken definition is.
///
/// What is checked here depends on which features are compiled, and always in
/// the safe direction: a `Shader` surface is lowered and partitioned only under
/// `shader`, and a strand layer is checked against its graph only under
/// `strand-scatter`, because those are the builds that hold the code which would
/// otherwise *use* them. A build that cannot check one cannot create it either.
#[cfg(feature = "runtime-bake")]
pub fn read_library_with_graphs(
    path: &Path,
    asset_root: &Path,
    building: &ashlar::Building,
    graphs: &MaterialGraphLibrary,
) -> Result<MaterialLibrary> {
    let library = parse_library(path, building)?;
    check_graphs(&path.display().to_string(), &library, building, graphs)?;
    preflight_textures(&library, asset_root)?;
    preflight_sets(&library, asset_root)?;
    Ok(library)
}

/// [`read_library_with_graphs`] for a library already in memory. `origin` names it in
/// errors, where a read names the file.
///
/// # Errors
///
/// As [`read_library_with_graphs`].
#[cfg(feature = "runtime-bake")]
pub fn check_library_with_graphs(
    origin: &str,
    library: &MaterialLibrary,
    asset_root: &Path,
    building: &ashlar::Building,
    graphs: &MaterialGraphLibrary,
) -> Result<()> {
    library.check_for(building)?;
    check_graphs(origin, library, building, graphs)?;
    preflight_textures(library, asset_root)?;
    preflight_sets(library, asset_root)
}

/// Parse a material library and hold it against the building that uses it.
///
/// The half both reads share and the half that needs neither files nor graphs,
/// so that the two entry points differ only in what they preflight afterwards.
fn parse_library(path: &Path, building: &ashlar::Building) -> Result<MaterialLibrary> {
    let library: MaterialLibrary = ron::from_str(
        &std::fs::read_to_string(path)
            .with_context(|| format!("reading material library {}", path.display()))?,
    )?;
    library.check_for(building)?;
    Ok(library)
}

/// Lower every graph the library and its bindings name.
///
/// The graph half of [`read_library_with_graphs`], kept whole so that the featureless
/// [`read_library`] is visibly the same function with this step left out rather than a
/// second implementation of the same walk.
#[cfg(feature = "runtime-bake")]
fn check_graphs(
    origin: &str,
    library: &MaterialLibrary,
    building: &ashlar::Building,
    graphs: &MaterialGraphLibrary,
) -> Result<()> {
    // One lowering per *distinct* override rather than per instance that wears
    // one: a block of a hundred bays binds three, and lowering a plaster graph
    // a hundred times in front of a window is a startup this preflight would
    // have paid for nothing. A binding remembers the first slot that asked for
    // it, so the error still names a place in the recipe.
    let mut overrides = BTreeMap::new();
    for (instance, slot, binding) in building.bindings() {
        if binding.params.is_empty() {
            continue;
        }
        overrides
            .entry(binding)
            .or_insert_with(|| format!("instances[{instance}].materials[{slot}]"));
    }
    for (binding, path_of) in overrides {
        let definition = library
            .materials
            .get(&binding.material)
            .with_context(|| format!("material {}", binding.material))?;
        preflight_override(&path_of, definition, binding, graphs)
            .with_context(|| format!("preflighting {origin}"))?;
    }
    for (material, definition) in &library.materials {
        // Before the surface, and for every surface: a definition may grow
        // strands out of the graph it names whichever of the three ways it
        // names one, and a layer the graph does not declare should be a
        // startup error naming the material rather than a lawn that is not
        // there.
        #[cfg(feature = "strand-scatter")]
        strands::preflight_strands(definition, graphs)
            .with_context(|| format!("material {material}"))
            .with_context(|| format!("preflighting {origin}"))?;
        // Beside it, and advisory where that one refuses: a graph may now say
        // how many metres of wall it was drawn for, and this is the one place
        // that number can be compared with the metres the definition lays it
        // over. Once per definition per read, which is once per library load.
        preflight_tiling(material, definition, graphs);
        match &definition.surface {
            // Files and constants name no graph. A `Files` surface's own
            // `baked_from` is provenance and not a dependency — the files it
            // names are on disk and are what will be loaded — so it is
            // deliberately not lowered, and a library may ship textures long
            // after its graphs moved on.
            ashlar::Surface::Plain | ashlar::Surface::Files { .. } => {}
            ashlar::Surface::Graph(bake) => {
                preflight_graph(bake, graphs)
                    .with_context(|| format!("material {material}"))
                    .with_context(|| format!("preflighting {origin}"))?;
            }
            // A graph again, and compiled rather than baked: `preflight_shader`
            // lowers it, partitions it and prints both stages, so everything a
            // compiled material can be refused for except the WGSL itself is
            // refused here. `naga` is a dev-dependency of `ashlar-material` on
            // purpose — a tool linking this crate should not link a shader
            // front end — so the generated text is validated in that crate's
            // tests and compiled for real by the first frame that draws it.
            #[cfg(feature = "shader")]
            ashlar::Surface::Shader { graph, params } => {
                shader::preflight_shader(graph, params, graphs)
                    .with_context(|| format!("material {material}"))
                    .with_context(|| format!("preflighting {origin}"))?;
            }
            #[cfg(not(feature = "shader"))]
            ashlar::Surface::Shader { .. } => {}
        }
    }
    Ok(())
}

/// Open every map the library names, in the colour space its slot will load it
/// under.
///
/// The file half of both reads, and the whole of the featureless one. The
/// destructuring is exhaustive so that a map added to the surface is a build
/// failure here rather than a texture that never gets preflighted.
fn preflight_textures(library: &MaterialLibrary, asset_root: &Path) -> Result<()> {
    for (key, _) in texture_keys(library)? {
        open_texture(&asset_root.join(&key), &key)?;
    }
    Ok(())
}

/// Every texture key a library names, once each, with the colour space its
/// slot loads it in.
///
/// The half of the texture preflight that needs no files: each key is relative
/// to the asset root and nothing else, and no key is both a colour map and a
/// data map. [`preflight_textures`] opens each file from disk and the material
/// library loader opens each through Bevy's own asset reader, so both refuse
/// the same libraries.
pub(crate) fn texture_keys(library: &MaterialLibrary) -> Result<Vec<(String, bool)>> {
    let mut spaces: BTreeMap<String, bool> = BTreeMap::new();
    let mut keys = Vec::new();
    for definition in library.materials.values() {
        let files = match &definition.surface {
            ashlar::Surface::Files {
                base_color,
                normal,
                orm,
                height,
                emissive,
                baked_from: _,
            } => [
                (base_color, true),
                (normal, false),
                (orm, false),
                (height, false),
                // Emissive is linear HDR out of the bake, not an sRGB photograph.
                (emissive, false),
            ],
            // Constants name no files, and neither does a graph.
            ashlar::Surface::Plain | ashlar::Surface::Graph(_) | ashlar::Surface::Shader { .. } => {
                continue;
            }
        };
        for (key, srgb) in files {
            let Some(key) = key else {
                continue;
            };
            ensure!(
                relative(key),
                "texture key must be relative to the asset root: {key}"
            );
            if let Some(previous) = spaces.insert(key.clone(), srgb) {
                ensure!(
                    previous == srgb,
                    "texture {key} cannot be both sRGB color and linear data"
                );
                continue;
            }
            keys.push((key.clone(), srgb));
        }
    }
    Ok(keys)
}

/// Whether a key names a file under the asset root and nowhere else.
fn relative(key: &str) -> bool {
    !Path::new(key).is_absolute()
        && Path::new(key)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// Every baked strand set a library names, beside the material that names it.
///
/// The key half of [`preflight_sets`], shared with the material library loader
/// for [`texture_keys`]'s reason.
pub(crate) fn strand_set_keys(library: &MaterialLibrary) -> Result<Vec<(String, String)>> {
    let mut keys = Vec::new();
    for (material, definition) in &library.materials {
        let Some(key) = definition.strand_set_key() else {
            continue;
        };
        ensure!(
            relative(key),
            "material {material}: strand set key must be relative to the asset root: {key}"
        );
        keys.push((material.clone(), key.to_owned()));
    }
    Ok(keys)
}

/// Open every baked strand set the library names, and hold it to the layers
/// the definition that names it grows.
///
/// The strand half of [`read_library`], and — like the texture half — part of the
/// *featureless* preflight, because a baked set is a file a game ships and a
/// missing one should be a startup error naming the key rather than a lawn that
/// is not there.
///
/// What it checks depends on which features are compiled, and always in the
/// safe direction. Featureless it reads the file's identifier and version by
/// hand, which is [`open_strand_set`]'s shallow read. Under `strands` —
/// the build that holds the reader and so the build that will *use* the file —
/// it opens the directory as well and says whether every layer the definition
/// names is in there. That second check is the stale case: a set written before
/// a layer was added to the graph, or one written for another material.
fn preflight_sets(library: &MaterialLibrary, asset_root: &Path) -> Result<()> {
    for (material, key) in strand_set_keys(library)? {
        let bytes = std::fs::read(asset_root.join(&key))
            .with_context(|| format!("opening strand set {key}"))
            .with_context(|| format!("material {material}"))?;
        open_strand_set(&bytes, &key, &library.materials[&material])
            .with_context(|| format!("material {material}"))?;
    }
    Ok(())
}

/// The deep read of a baked strand set, and the one a build that can draw one
/// gets.
///
/// `ashlar-strands` is a dependency under `strands`, and it costs no second
/// parser: the crate that *reads* the container is already here, so the
/// directory is walked, every offset is held against the file's length, and the
/// layers the definition names are held against the layers the file holds.
#[cfg(feature = "strands")]
pub(crate) fn open_strand_set(
    bytes: &[u8],
    key: &str,
    definition: &ashlar::MaterialDefinition,
) -> Result<()> {
    let info = ashlar_strands::file::inspect(bytes)
        .with_context(|| format!("reading strand set {key}"))?;
    let Some(settings) = &definition.strands else {
        return Ok(());
    };
    for layer in &settings.layers {
        ensure!(
            info.holds(layer),
            "strand set {key} holds no layer {layer:?}; it holds {}. The set is stale: re-run \
             the content step that wrote it.",
            if info.layers.is_empty() {
                "none at all".to_owned()
            } else {
                info.layers
                    .iter()
                    .map(|entry| entry.layer.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        );
    }
    Ok(())
}

/// The shallow read, and the one a game that draws no strands gets.
///
/// Featureless there is no strand reader in the tree, and pulling one in for a
/// startup check would undo the point of the split — exactly the bargain
/// [`ktx2_claims`]'s two halves make one file type along. So this reads the
/// fixed-size front of the header by hand: the identifier, which is what tells
/// a baked set from something renamed, and the version, which is the field a
/// build refuses on. Both are in the first sixteen bytes and neither needs the
/// directory.
///
/// What it gives up against the deep read is the directory, so a file whose
/// layers do not match the definition passes here — and a build with no strand
/// reader in it was never going to grow those layers anyway.
#[cfg(not(feature = "strands"))]
pub(crate) fn open_strand_set(
    bytes: &[u8],
    key: &str,
    _definition: &ashlar::MaterialDefinition,
) -> Result<()> {
    /// The twelve bytes a baked strand set begins with, spelled out here rather
    /// than taken from `ashlar-strands` for [`ZSTD_SCHEME`]'s reason: this is
    /// the one fact the featureless build needs, and the whole point is that
    /// the crate holding the constant is not a dependency then.
    const IDENTIFIER: [u8; 12] = [
        0xAB, b'A', b'S', b'H', b'S', b'T', b'R', b'D', 0x0D, 0x0A, 0x1A, 0x0A,
    ];
    /// The format version this workspace writes. See above.
    const VERSION: u32 = 1;

    ensure!(
        bytes.len() >= 16,
        "strand set {key} is {} bytes, which is shorter than its own header",
        bytes.len()
    );
    ensure!(
        bytes[..IDENTIFIER.len()] == IDENTIFIER,
        "strand set {key} does not begin with the ashlar strand identifier, so it is not a \
         baked strand set"
    );
    let version = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    ensure!(
        version == VERSION,
        "strand set {key} is version {version}, and this build reads version {VERSION}"
    );
    Ok(())
}

/// The Zstandard supercompression scheme, as KTX2 numbers it.
///
/// Spelled out here rather than taken from `ashlar-material`, because this is
/// the one KTX2 fact the featureless build needs and the whole reason it needs
/// it is that the crate holding the constant is not a dependency then.
const ZSTD_SCHEME: u32 = 2;

/// Fail before opening the GPU window: a texture that is missing, corrupt or
/// not the container its extension claims is a startup error.
///
/// A `.ktx2` map is checked rather than decoded: the `image` crate does not
/// read KTX2, and what the preflight owes a caller is the error a missing file
/// or a file that is not the container it claims would otherwise have become —
/// a black wall, or a message from inside Bevy's loader.
///
/// What that check cannot do is decode a texel, so a KTX2 file whose levels are
/// nonsense inside passes here and fails in Bevy's loader. The preflight's job
/// is the one a missing file or a bad header fails, and it is the same job for
/// both containers.
///
/// Supercompression is the one thing it reads and acts on: Bevy is built here
/// with a Zstandard decoder and nothing else, so scheme 2 — what
/// `ashlar_material::ktx2::write_with` writes and what the study ships under —
/// passes, and any other non-zero scheme is refused by number rather than
/// failing inside the loader with a message about a container.
fn open_texture(path: &Path, key: &str) -> Result<()> {
    let bytes = std::fs::read(path).with_context(|| format!("opening texture {key}"))?;
    check_texture(&bytes, key)
}

/// [`open_texture`] over bytes already read, which is what the material library
/// loader has: it reads through Bevy's asset reader rather than the file system.
pub(crate) fn check_texture(bytes: &[u8], key: &str) -> Result<()> {
    if Path::new(key)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ktx2"))
    {
        let supercompression =
            ktx2_claims(bytes).with_context(|| format!("decoding texture {key}"))?;
        ensure!(
            matches!(supercompression, 0 | ZSTD_SCHEME),
            "texture {key}: supercompression scheme {supercompression}, which this adapter cannot \
             read — Bevy is built here with a Zstandard decoder (scheme {ZSTD_SCHEME}) and no \
             other, so write the file uncompressed or as zstd",
        );
        return Ok(());
    }
    image::load_from_memory(bytes).with_context(|| format!("decoding texture {key}"))?;
    Ok(())
}

/// What a KTX2 file claims about itself, as far as this build can read it, with
/// its supercompression scheme as the answer.
///
/// The deep read, and the one a tool gets: [`ashlar_material::ktx2::inspect`]
/// walks the header *and* the level index and says whether the file's claims
/// fit inside the file, so a truncated map is refused here rather than at load.
/// It costs no second parser because the crate that *writes* the container is
/// already a dependency under this feature.
#[cfg(feature = "runtime-bake")]
fn ktx2_claims(bytes: &[u8]) -> Result<u32> {
    Ok(ashlar_material::ktx2::inspect(bytes)?.supercompression)
}

/// The shallow read, and the one a game gets.
///
/// Featureless there is no KTX2 reader in the tree, and pulling one in for a
/// startup check would undo the point of the split — `ashlar-material` for the
/// level index, or the `ktx2` crate for a parser this workspace already ships
/// twice. So this reads the fixed-size header by hand: the identifier, which is
/// what tells a KTX2 from a PNG somebody renamed, and the supercompression
/// scheme, which is the field [`open_texture`] acts on. Both are at constant
/// offsets in the first forty-eight bytes and neither needs the level index.
///
/// What the shallow read gives up against the deep one is exactly the level
/// index: a file truncated *after* its header passes here and fails in Bevy's
/// loader, with the loader's message rather than this one's. That is the honest
/// cost of a game not linking a bake engine, and it is bounded — the file is
/// still known to exist, to be a KTX2 and to be compressed a way Bevy can
/// inflate.
#[cfg(not(feature = "runtime-bake"))]
fn ktx2_claims(bytes: &[u8]) -> Result<u32> {
    /// The twelve bytes every KTX2 file starts with, from the specification.
    const IDENTIFIER: [u8; 12] = [
        0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A,
    ];
    // `supercompressionScheme` is the ninth `u32` after the identifier, so the
    // header is read this far and no further.
    const SCHEME_AT: usize = IDENTIFIER.len() + 8 * 4;

    // The identifier first, and on however many bytes there are: a file that is
    // a PNG is more usefully reported as not a KTX2 file than as a short one.
    let head = IDENTIFIER.len().min(bytes.len());
    ensure!(
        bytes[..head] == IDENTIFIER[..head],
        "not a KTX2 file: the identifier is wrong"
    );
    let scheme = bytes
        .get(SCHEME_AT..SCHEME_AT + 4)
        .with_context(|| format!("truncated: {} bytes, and a header is 80", bytes.len()))?;
    Ok(u32::from_le_bytes(
        scheme.try_into().expect("a four-byte window is four bytes"),
    ))
}

/// How many anisotropic taps a surface's maps are filtered with.
///
/// Eight. A material is a repeating map on a wall, a floor or a patch of
/// ground, and every one of those is usually seen at an angle; a sampler with
/// one tap answers the coarsest mip either axis of the footprint asks for, so a
/// grazing view of a lawn reads as a flat sheet of the average colour. Eight is
/// the setting hardware has implemented for twenty years and the point past
/// which the difference stops being visible.
pub const ANISOTROPY: u16 = 8;

/// Create a PBR material using color-space-correct repeating textures.
///
/// The surfaces that need nothing but an [`AssetServer`]: `Plain` is the
/// definition's constants, and `Files` loads the files it names with the
/// colour space and the sampler each slot wants. A `Graph` or `Shader` surface
/// has no textures *here* — it names a graph, and turning one into images is
/// `runtime_bake::create_graph_material`, which needs the graph library, a
/// `runtime_bake::BakeCache` and `Assets<Image>` beside it. Passing one to this
/// function is not an error and gives the definition's constants, which is a
/// flat but honest picture; a library that can hold one should be created
/// through `create_graph_material`, which handles every variant.
///
/// # The normal map's green channel
///
/// A baked normal map is written in image coordinates, green decreasing down a
/// rising row, and Bevy reads OpenGL's green-up. So a `Files` normal map is
/// read in the *bake's* convention — flipped here — exactly when its
/// definition records `baked_from`, and as an OpenGL map when it does not.
/// `ashlar_material::export`, the content step, records `baked_from` on every
/// definition it writes, so a library it exported is right by construction; a
/// library written by hand over exported maps must record it too, and one over
/// maps from anywhere else — a scan, a photograph, another tool's OpenGL
/// export — leaves it `None`.
pub fn create_material(
    definition: &ashlar::MaterialDefinition,
    server: &AssetServer,
) -> StandardMaterial {
    let texture = |key: &Option<String>, srgb: bool| {
        key.as_ref().map(|key| {
            server
                .load_builder()
                .with_settings(move |settings: &mut ImageLoaderSettings| {
                    settings.is_srgb = srgb;
                    settings.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                        address_mode_u: ImageAddressMode::Repeat,
                        address_mode_v: ImageAddressMode::Repeat,
                        min_filter: ImageFilterMode::Linear,
                        mag_filter: ImageFilterMode::Linear,
                        mipmap_filter: ImageFilterMode::Linear,
                        // A wall is nearly always seen at an angle, and a
                        // trilinear sampler picks its mip off the *longer* axis
                        // of the footprint: at a grazing angle that is several
                        // levels coarser than the short axis wants, so the
                        // surface loses its detail in exactly the view that
                        // shows most of it. Eight taps is the usual setting and
                        // costs bandwidth on the pixels that are already
                        // cheapest.
                        anisotropy_clamp: ANISOTROPY,
                        ..default()
                    });
                })
                .load(key.clone())
        })
    };
    let maps = match &definition.surface {
        // `height` and `baked_from` are read by nobody here. Bevy 0.19 does
        // have a relief slot, `StandardMaterial::depth_map`, but it wants
        // black-is-top depth where a surface carries white-is-top height, and
        // filling it also means choosing a `parallax_depth_scale` that a map
        // saying only 0..=1 does not carry. That inversion and that scale are a
        // decision this step does not own, so height is preflighted and left on
        // disk; `baked_from` is provenance for the content step.
        ashlar::Surface::Files {
            base_color,
            normal,
            orm,
            height: _,
            emissive,
            baked_from: _,
        } => {
            let orm = texture(orm, false);
            Maps {
                base_color: texture(base_color, true),
                normal: texture(normal, false),
                metallic_roughness: orm.clone(),
                occlusion: orm,
                // Linear, matching the preflight: the bake writes linear HDR
                // emissive. Bevy multiplies it by `emissive`, which defaults to
                // zero, so a definition naming this map sets that constant too.
                emissive: texture(emissive, false),
            }
        }
        // A graph names no files. `Graph` goes through
        // `runtime_bake::create_graph_material`, which has what it takes to rasterise one;
        // `Shader` goes through `shader::create_shader_material`, which compiles one and
        // answers a different material type entirely.
        ashlar::Surface::Plain | ashlar::Surface::Graph(_) | ashlar::Surface::Shader { .. } => {
            Maps::default()
        }
    };
    standard(definition, maps)
}

/// The definition's constants, its UV transform and whatever maps a surface
/// produced, as one material.
///
/// Shared by [`create_material`] and [`runtime_bake::create_graph_material`] so that a runtime bake
/// and a set of files differ in where the handles came from and in nothing
/// else: the same tiling, the same constants, the same empty `depth_map`.
pub(crate) fn standard(definition: &ashlar::MaterialDefinition, maps: Maps) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::srgb_from_array(definition.base_color),
        perceptual_roughness: definition.roughness,
        metallic: definition.metallic,
        emissive: LinearRgba::rgb(
            definition.emissive[0],
            definition.emissive[1],
            definition.emissive[2],
        ),
        base_color_texture: maps.base_color,
        normal_map_texture: maps.normal,
        // Ashlar derives normals in image/UV coordinates: both derivatives are
        // negated, so G decreases down a rising image row. Bevy's Mikk basis
        // expects OpenGL +Y. Keep historical baked bytes and convert here.
        // Unprovenanced external textures retain Bevy's OpenGL convention.
        flip_normal_map_y: matches!(
            &definition.surface,
            ashlar::Surface::Graph(_)
                | ashlar::Surface::Files {
                    baked_from: Some(_),
                    ..
                }
        ),
        metallic_roughness_texture: maps.metallic_roughness,
        occlusion_texture: maps.occlusion,
        emissive_texture: maps.emissive,
        uv_transform: bevy::math::Affine2::from_scale_angle_translation(
            Vec2::from_array(definition.tile_metres).recip(),
            0.0,
            Vec2::from_array(definition.uv_offset),
        ),
        ..default()
    }
}

/// The image handles one surface contributes, so the match arms above stay one
/// expression each rather than a mutable `StandardMaterial` built in pieces.
#[derive(Default)]
pub(crate) struct Maps {
    pub(crate) base_color: Option<Handle<Image>>,
    pub(crate) normal: Option<Handle<Image>>,
    pub(crate) metallic_roughness: Option<Handle<Image>>,
    pub(crate) occlusion: Option<Handle<Image>>,
    pub(crate) emissive: Option<Handle<Image>>,
}

/// The mesh attribute a cut flag rides in: Bevy's **second UV set**.
///
/// Not a new attribute, and that is the whole finding of this step. A custom
/// attribute needs a vertex shader that forwards it, and a
/// [`MaterialExtension`](bevy::pbr::MaterialExtension) draws through Bevy's own
/// vertex stage into Bevy's own `VertexOutput` — a struct with no free
/// location. Emitting a vertex shader alongside the fragment would mean
/// reimplementing `mesh.wgsl` and `prepass.wgsl`, skinning, morph targets and
/// motion vectors included, twice, to carry one float. So the flag takes a slot
/// that already exists.
///
/// Of the two `VertexOutput` carries that this stack does not use, the second
/// UV set is the one that costs nothing: Bevy's mesh and prepass vertex stages
/// both forward `uv_b` verbatim under `VERTEX_UVS_B`, and the pipeline defines
/// that shader-def from the mesh's own layout, so nothing has to be told. The
/// other candidate, vertex colour, is read by `pbr_fragment` as
/// `pbr_input.material.base_color = in.color`, which would *replace* the
/// definition's base colour with whatever the mesh said — an assignment, not a
/// multiply — and that is a picture changed to carry one bit.
///
/// What repurposing costs is the second UV set itself, for as long as the flag
/// lives here: a lightmap reads `uv_b`, and so does any texture slot a
/// `StandardMaterial` points at channel B. Neither is used by anything in this
/// workspace, and the design's open question about a second UV set now has this
/// answer written into it.
///
/// The flag is `x`; `y` is zero and reserved. One on a face the kernel cut,
/// zero elsewhere, and never anything between — see [`mesh`].
pub const ATTRIBUTE_ASHLAR_CUT: bevy::mesh::MeshVertexAttribute = Mesh::ATTRIBUTE_UV_1;

/// Upload local-space triangle data, preserving metre UVs and generating PBR tangents.
///
/// A mesh with any cut face also carries [`ATTRIBUTE_ASHLAR_CUT`], so a
/// compiled material can read `ashlar_material::nodes::CutFlag` and treat the
/// faces a cutter made differently from the ones it did not. A mesh with none
/// carries no such attribute and costs no vertex bytes: the generated shader
/// reads zero where the attribute is absent, which is the same answer it would
/// have read off a buffer of zeros.
///
/// The flag is a fact about a *triangle* and an attribute is a fact about a
/// vertex, so the two meet at [`ashlar::TriangleMesh::weld`], which treats the
/// edge of a cut as a seam: a welded mesh has no vertex on both sides, every
/// triangle's three corners agree, and the interpolated value is exactly one or
/// exactly zero everywhere. A mesh that reached here still sharing one — an
/// unwelded soup, or one welded by something else — takes the **maximum** over
/// the faces at that vertex, so the value is defined and a cut face never
/// reads as uncut; what it loses is the crispness of the boundary, which ramps
/// across the neighbouring face instead of stopping at it.
// The renderer is the f64-to-f32 boundary.
#[allow(clippy::cast_possible_truncation)]
pub fn mesh(source: &ashlar::TriangleMesh) -> Result<Mesh> {
    let mut mesh = Mesh::new(
        bevy::mesh::PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::default(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        source
            .positions
            .iter()
            .map(|p| p.as_vec3().to_array())
            .collect::<Vec<_>>(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_NORMAL,
        source
            .normals
            .iter()
            .map(|p| p.as_vec3().to_array())
            .collect::<Vec<_>>(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_UV_0,
        source
            .uvs
            .iter()
            .map(|p| [p[0] as f32, p[1] as f32])
            .collect::<Vec<_>>(),
    )
    .with_inserted_indices(bevy::mesh::Indices::U32(source.indices.clone()));
    if let Some(cut) = cut_attribute(source) {
        mesh.insert_attribute(ATTRIBUTE_ASHLAR_CUT, cut);
    }
    mesh.generate_tangents()?;
    Ok(mesh)
}

/// One cut flag per vertex, or `None` where the surface has no cut face at all.
///
/// The maximum over the triangles at a vertex; [`mesh`] says why that case
/// should not arise and what it costs when it does.
fn cut_attribute(source: &ashlar::TriangleMesh) -> Option<Vec<[f32; 2]>> {
    if !source.cut_faces().any(|cut| cut) {
        return None;
    }
    let mut flags = vec![[0.0_f32, 0.0]; source.positions.len()];
    for (face, corners) in source.corners().enumerate() {
        if !source.is_cut(face) {
            continue;
        }
        for corner in corners {
            if let Some(flag) = flags.get_mut(corner) {
                flag[0] = 1.0;
            }
        }
    }
    Some(flags)
}

/// Foundation and ramp rendering uses the exact convex collision corners.
pub fn ground_mesh(solids: &[ashlar::GroundSolid]) -> Result<Mesh> {
    let mut source = ashlar::TriangleMesh::default();
    for solid in solids {
        for triangle in solid.triangles() {
            let normal = (triangle[1] - triangle[0])
                .cross(triangle[2] - triangle[0])
                .normalize();
            let uvs = ashlar::planar_uvs(triangle).context("invalid ground UV frame")?;
            source.push_triangle(triangle, [normal; 3], uvs, ashlar::FaceSource::BODY);
        }
    }
    source.weld(ashlar::WELD_TOLERANCE);
    mesh(&source)
}

#[cfg(test)]
mod tests {
    use super::{ScaleVerdict, Tiling, scale_verdict, tiling};

    /// The rule the tiling warning is made of, case by case, because every one
    /// of these numbers is a shipped definition's and a shipped graph's.
    #[test]
    fn a_declared_repeat_is_agreed_scaled_or_stretched() {
        // A graph that says nothing is agreed with by definition: nothing in
        // the workspace declared a repeat before the field existed.
        assert_eq!(tiling(None, [2.4, 2.4]), Tiling::Agreed);
        // The study's own walls, where the definition is written from the same
        // number the graph is.
        assert_eq!(tiling(Some([1.72, 1.72]), [1.72, 1.72]), Tiling::Agreed);
        // And the study's light strip, which is deliberately not square on
        // either side of the seam.
        assert_eq!(tiling(Some([2.25, 0.5]), [2.25, 0.5]), Tiling::Agreed);

        // The sheet and the benchmark divide the tiling by three and by nine to
        // fake distance without moving the camera. Both axes, so both are a
        // scale; whether that scale is quiet is `scale_verdict`'s rule, tested
        // below, and depends on the definition declaring it.
        let third = 2.0_f32 / 3.0;
        assert_eq!(
            tiling(Some([2.0, 2.0]), [third, third]),
            Tiling::Scaled(third / 2.0)
        );
        let ninth = 1.72_f32 / 9.0;
        let Tiling::Scaled(factor) = tiling(Some([1.72, 1.72]), [ninth, ninth]) else {
            panic!("a ninth of a square repeat is a scale");
        };
        assert!((factor - 1.0 / 9.0).abs() < 1e-6, "{factor}");

        // A repeat that changed shape is the case nothing legitimate does.
        let Tiling::Stretched([u, v]) = tiling(Some([1.72, 1.72]), [1.72, 0.86]) else {
            panic!("half as tall a repeat is a stretch");
        };
        assert!((u - 1.0).abs() < 1e-6 && (v - 0.5).abs() < 1e-6, "{u} {v}");

        // A millimetre in a metre is one number typed twice, not a
        // disagreement; a centimetre is the author meaning it.
        assert_eq!(tiling(Some([2.4, 2.4]), [2.4001, 2.3999]), Tiling::Agreed);
        assert!(matches!(
            tiling(Some([2.4, 2.4]), [2.4, 2.38]),
            Tiling::Stretched(_)
        ));

        // Neither side can hold one of these once it is validated, and a pure
        // function still answers rather than dividing somebody's wall by zero.
        assert!(matches!(
            tiling(Some([0.0, 2.0]), [2.0, 2.0]),
            Tiling::Stretched(_)
        ));
        assert!(matches!(
            tiling(Some([f32::NAN, 2.0]), [2.0, 2.0]),
            Tiling::Stretched(_)
        ));
    }

    /// The rule a uniform scale is judged by, case by case over the shipped
    /// factors: quiet where a definition declares the scale it tiles at,
    /// warned where nothing declares it or a strand layer cannot follow it
    /// regardless.
    #[test]
    fn a_uniform_scale_is_declared_undeclared_or_strands() {
        // The sheet divides by three and by nine; the benchmark reuses the
        // same two factors over its own presets. Both declare `tile_scale` as
        // the reciprocal, and a definition with no strand layer is quiet.
        assert_eq!(
            scale_verdict(1.0 / 3.0, Some(1.0 / 3.0), false),
            ScaleVerdict::Declared
        );
        assert_eq!(
            scale_verdict(1.0 / 9.0, Some(1.0 / 9.0), false),
            ScaleVerdict::Declared
        );

        // The same declared scale under a strand layer still warns: a blade's
        // length is absolute metres, so a texture-only declaration does not
        // excuse it.
        assert_eq!(
            scale_verdict(1.0 / 3.0, Some(1.0 / 3.0), true),
            ScaleVerdict::Strands
        );

        // The honest typo this field exists to catch: 2.4 typed where the
        // graph declares 1.72 is a factor of about 1.395, and nothing on the
        // definition says that is on purpose.
        let typo = 2.4_f32 / 1.72;
        assert_eq!(scale_verdict(typo, None, false), ScaleVerdict::Undeclared);
        // Declaring some other scale does not excuse a different factor
        // either — the two have to agree, not merely both exist.
        assert_eq!(
            scale_verdict(typo, Some(1.0 / 3.0), false),
            ScaleVerdict::Undeclared
        );

        // A millimetre of drift between the factor and the declared scale is
        // the same number meant twice, at the tolerance `tiling` itself
        // compares repeats with.
        assert_eq!(
            scale_verdict(1.0 / 3.0, Some(1.0 / 3.0 + 1e-6), false),
            ScaleVerdict::Declared
        );
    }
}
