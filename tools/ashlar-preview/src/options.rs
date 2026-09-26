//! The command line, shared by the interactive viewer and the gallery.
use std::path::PathBuf;

use bevy::prelude::Resource;
use clap::{CommandFactory, FromArgMatches, Parser, builder::PossibleValuesParser};

use crate::Catalog;

/// Preview and reference-gallery switches.
///
/// `--scene` and `--reference-scenes` are plain strings here and get their
/// permitted values from the [`Catalog`] at parse time, so a downstream binary
/// with its own content still gets validation and a useful `--help`.
#[derive(Parser, Resource)]
#[command(about = "Preview ashlar building recipes", version)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent command-line switches"
)]
pub struct Options {
    /// Render a windowless reference gallery, then exit. Defaults to every scene.
    #[arg(long, conflicts_with_all = ["recipe", "screenshot", "write_example", "materials", "graphs", "graphs_out", "bake", "wireframe", "clay", "max_storey", "hide_exterior", "frames", "scene", "yaw", "pitch", "zoom"])]
    pub references: Option<PathBuf>,
    /// Limit the reference gallery to these comma-separated scenes.
    #[arg(long, value_delimiter = ',', requires = "references")]
    pub reference_scenes: Vec<String>,
    /// Stand the gallery camera this many metres from what it frames, instead
    /// of the model's own diagonal.
    ///
    /// The gallery frames a whole model, and at that distance anything at the
    /// scale of a material is under a pixel: an eighteen-millimetre pile on a
    /// two-metre patch is four metres away, which is nothing. This is what a
    /// capture of the *surface* rather than of the model asks for, and it is
    /// deliberately a plain distance rather than a zoom, so a set of captures
    /// at 0.4, 2 and 8 metres is a set anyone can reproduce.
    #[arg(long, requires = "references")]
    pub reference_distance: Option<f32>,
    /// Render one view at this yaw and pitch, in radians, instead of the three
    /// the gallery normally takes.
    ///
    /// A grazing angle is what shows a silhouette, and none of the three fixed
    /// views is one. Written as `yaw,pitch`; the view is named `custom` in the
    /// manifest and in the file name.
    #[arg(long, value_delimiter = ',', requires = "references")]
    pub reference_view: Vec<f32>,
    /// Light the gallery with this much ambient instead of the building rig's.
    ///
    /// The gallery frames whole buildings, and a building wants enough fill to
    /// read inside its own reveals. A *surface* does not: at the scale of a
    /// material that same fill flattens every recess the relief has, so a
    /// close capture of a pile reads as a pale floor whatever the pile does.
    /// The studio the material swatches are rendered under uses 150, and a
    /// capture meant to be compared with one of those should say so.
    #[arg(long, requires = "references")]
    pub reference_ambient: Option<f32>,
    /// Light the gallery's key at this many lux instead of the building rig's
    /// 14 000. The fill follows in the same ratio.
    ///
    /// The other half of the same argument `--reference-ambient` makes. A
    /// building is lit to read across a facade; a material at arm's length is
    /// not, and under a key meant for a facade an 18 mm pile is a flat pale
    /// floor with the shadows blown off it. The material studio the swatches
    /// are rendered under uses 6500, and a close capture meant to be compared
    /// with one of those should say so.
    #[arg(long, requires = "references")]
    pub reference_key: Option<f32>,
    /// Light the scene by night: a dim, cold moon for a key, almost no fill, a
    /// blue-black sky and bloom on the camera, so what the picture is made of
    /// is what emits — neon, lit windows, street lamps. Works for the viewer,
    /// its `--screenshot` and the gallery alike.
    #[arg(long)]
    pub night: bool,
    /// Light the viewer's key at this many lux instead of the rig's, 14 000
    /// by day and the moon's by night. The fill follows in the same ratio.
    ///
    /// The viewer's twin of `--reference-key`, for tuning emissive surfaces
    /// against a light level without rebuilding.
    #[arg(long, conflicts_with = "references")]
    pub key: Option<f32>,
    /// Light the viewer with this much ambient instead of the rig's, 350 by
    /// day and a fraction of that by night.
    #[arg(long, conflicts_with = "references")]
    pub ambient: Option<f32>,
    /// Compile shader pipelines serially when investigating driver compiler crashes.
    #[arg(long)]
    pub serial_pipelines: bool,
    /// Catalog scene to display or export; the catalog's default otherwise.
    #[arg(long)]
    pub scene: Option<String>,
    /// Material library RON; custom recipes otherwise use diagnostic colors.
    #[arg(long)]
    pub materials: Option<PathBuf>,
    /// Material graph library RON; the catalog scene's own otherwise.
    #[arg(long)]
    pub graphs: Option<PathBuf>,
    /// Where the parameter panel's Save writes; over the loaded library otherwise.
    #[arg(long)]
    pub graphs_out: Option<PathBuf>,
    /// Bake every surface that records a graph instead of loading the files it
    /// names, so that editing the graph library changes the picture.
    #[arg(long)]
    pub bake: bool,
    /// Root directory for texture resource keys. A catalog may replace this
    /// default with its own game's assets folder.
    #[arg(long, default_value = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"))]
    pub asset_root: PathBuf,
    /// Start in neutral clay mode, with the same geometry.
    #[arg(long)]
    pub clay: bool,
    /// Initial view yaw in radians.
    #[arg(long, default_value = "-0.5", allow_hyphen_values = true)]
    pub yaw: f32,
    /// Initial view elevation in radians.
    #[arg(long, default_value = "0.25", allow_hyphen_values = true)]
    pub pitch: f32,
    /// Orbit about `<X,Y,Z>` in building-space metres instead of the middle of
    /// the model, so a view can stand in a street rather than always look at
    /// half a tower's height. The distance still comes from `--zoom`.
    #[arg(long, value_parser = parse_blast, allow_hyphen_values = true, conflicts_with = "references")]
    pub focus: Option<BlastCentre>,
    /// Camera distance multiplier; smaller values give a closer view.
    #[arg(long, default_value = "1.0")]
    pub zoom: f32,
    /// Read an experimental RON building recipe; otherwise use the catalog scene.
    #[arg(long)]
    pub recipe: Option<PathBuf>,
    /// Write the catalog scene as editable RON and exit without opening a window.
    #[arg(long)]
    pub write_example: Option<PathBuf>,
    /// Save a preview PNG after shader warmup, then exit.
    #[arg(long)]
    pub screenshot: Option<PathBuf>,
    /// Exit after this many frames (default 180 when taking a screenshot).
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub frames: Option<u32>,
    /// Start with triangle edges visible.
    #[arg(long)]
    pub wireframe: bool,
    /// Start with every storey above this one hidden. `PageDown` and `PageUp`
    /// move the cut one storey at a time, and past the highest storey they
    /// clear it.
    #[arg(long)]
    pub max_storey: Option<i32>,
    /// Start with exterior faces hidden, so a building's interior reads. `I`
    /// toggles it while the preview runs.
    #[arg(long)]
    pub hide_exterior: bool,
    /// The building palette slot a blast's exposed faces wear.
    ///
    /// Shift-clicking a merged scene's surface subtracts a ball of
    /// `--blast-radius` there, and the faces the ball exposes are bound through
    /// this slot exactly as a cutter's reveal is. A slot the palette does not
    /// bind turns blasting off with a warning rather than failing.
    #[arg(long, default_value = "stone")]
    pub blast_slot: String,
    /// The radius in metres of a shift-click blast.
    #[arg(long, default_value = "1.2")]
    pub blast_radius: f64,
    /// Do not drop what a blast leaves unattached; a blast removes only what it
    /// overlaps.
    ///
    /// Blasting is collapsing by default: after a hit, every connected piece
    /// of the group that does not reach the group's base falls with it, because
    /// a wall stub with a hole on every side has nothing holding it. The flag
    /// lives on the recorded hit, not in the preview, so this only changes what
    /// the preview queues.
    #[arg(long)]
    pub no_collapse: bool,
    /// Queue a blast at `<X,Y,Z>` when a merged scene starts, in building-space
    /// metres.
    ///
    /// Repeatable, so one capture can show several holes, and they are applied
    /// in the order given. Each centre uses `--blast-radius` and `--blast-slot`
    /// exactly as a shift-click does. A scene that is not merged has no blast
    /// state, so the option is inert there. It exists because a headless
    /// screenshot cannot shift-click, and a capture of damage has to come from
    /// somewhere.
    #[arg(long, value_parser = parse_blast, allow_hyphen_values = true)]
    pub blast: Vec<BlastCentre>,
}

/// A blast centre from the command line, in building space.
///
/// Three metres, comma separated, in the same coordinates the recipe is
/// authored in. A negative coordinate is ordinary, which is why the option
/// taking one allows hyphen values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlastCentre(
    /// The centre, in building-space metres.
    pub [f64; 3],
);

/// Parse one `X,Y,Z` blast centre, or name the value that was not three finite
/// floats.
fn parse_blast(value: &str) -> Result<BlastCentre, String> {
    let invalid = || format!("{value:?} is not three finite floats X,Y,Z");
    let parts = value.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(invalid());
    }
    let mut centre = [0.0; 3];
    for (coordinate, part) in centre.iter_mut().zip(parts) {
        let number = part.parse::<f64>().map_err(|_| invalid())?;
        if !number.is_finite() {
            return Err(invalid());
        }
        *coordinate = number;
    }
    Ok(BlastCentre(centre))
}

impl Options {
    /// Parse the process arguments, validating scene names against `catalog`.
    ///
    /// Exits the process on a usage error, as `clap`'s own `parse` does.
    pub fn parse_for(catalog: &Catalog) -> Self {
        match Self::try_parse_for(catalog, std::env::args_os()) {
            Ok(options) => options,
            Err(error) => error.exit(),
        }
    }

    /// Parse `arguments`, validating scene names against `catalog`.
    pub fn try_parse_for<I, T>(catalog: &Catalog, arguments: I) -> clap::error::Result<Self>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let names = catalog.names().map(str::to_owned).collect::<Vec<_>>();
        let values = move |argument: clap::Arg| {
            argument.value_parser(PossibleValuesParser::new(names.clone()))
        };
        let mut command = Self::command()
            .mut_arg("scene", values.clone())
            .mut_arg("reference_scenes", values);
        if let Some(root) = catalog.default_asset_root() {
            let root = root.to_string_lossy().into_owned();
            command = command.mut_arg("asset_root", move |argument: clap::Arg| {
                argument.default_value(root.clone())
            });
        }
        Self::from_arg_matches(&command.try_get_matches_from(arguments)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blast_centre_parses_three_finite_floats() {
        assert_eq!(parse_blast("1,2,3"), Ok(BlastCentre([1.0, 2.0, 3.0])));
        assert_eq!(
            parse_blast("-1.5, 2 ,0.25"),
            Ok(BlastCentre([-1.5, 2.0, 0.25])),
            "spaces around a coordinate are tolerated"
        );
    }

    #[test]
    fn a_blast_centre_names_the_value_it_rejects() {
        for value in ["1,2", "1,2,x", "1,2,nan"] {
            let error = parse_blast(value).expect_err("must not parse");
            assert!(error.contains(value), "{error} does not name {value:?}");
        }
    }
}
