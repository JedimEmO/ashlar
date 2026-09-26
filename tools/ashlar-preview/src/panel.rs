//! The parameter panel: the selected graph's `Param`s as sliders, over the scene.
//!
//! Material work is a few hundred small adjustments made while looking at the
//! result, and a text editor makes each of them a save and a wait. The panel is
//! the short way round: every exposed parameter of one graph, with a slider or a
//! colour's three, editing the library in memory and re-baking when the pointer
//! lets go. `S` writes the edited library back to RON, so a session of turning
//! knobs ends in a file rather than in a screenshot.
//!
//! Bevy's own UI, and none of it painted from scratch: `bevy_ui_widgets` ships
//! headless [`Slider`] and [`Checkbox`] widgets in 0.19, so what is written here
//! is their appearance and what they mean. The alternative was `bevy_egui`,
//! which was measured first as the step asked: at 0.42, the version built
//! against Bevy 0.19, it brings a second `itertools` and a second
//! `guillotiere`, neither under an existing `skip-tree` entry, and
//! `cargo deny`'s duplicate rule is `deny`. Bevy UI costs no dependency at all.
//!
//! Re-baking on release rather than on every step of a drag is what
//! [`ValueChange::is_final`] is for: a 1024 bake is a fifth of a second, which
//! is a fine pause after a drag and an unusable one during it.
use ashlar_material::{Param, ParamValue};
use bevy::{
    ecs::system::SystemParam,
    prelude::*,
    ui_widgets::{
        Activate, Button as WidgetButton, Checkbox, Slider, SliderDragState, SliderRange,
        SliderStep, SliderThumb, SliderValue, ValueChange,
    },
    window::PrimaryWindow,
};

use crate::{
    Options,
    reload::{GraphSource, Rebake, Redress, Reloaded},
};

/// How wide the panel is, and therefore how much of the window the camera
/// controls have to let go of while it is open.
const WIDTH: f32 = 330.0;
/// Thumb size, and the amount the track is inset by so the thumb can be placed
/// as a plain percentage without overhanging the right end.
const THUMB: f32 = 12.0;
/// Row height for a slider track and a checkbox, in logical pixels.
const ROW: f32 = 14.0;

/// A dark, nearly opaque column: the scene behind it stays readable at a
/// glance, and the text over it does not fight the wall it is describing.
const BACKDROP: Color = Color::srgba(0.05, 0.06, 0.08, 0.93);
const INK: Color = Color::srgb(0.86, 0.88, 0.91);
const MUTED: Color = Color::srgb(0.55, 0.59, 0.64);
const TRACK: Color = Color::srgb(0.16, 0.18, 0.22);
const HANDLE: Color = Color::srgb(0.55, 0.72, 0.92);
const CHROME: Color = Color::srgb(0.20, 0.23, 0.28);

/// The panel's own state: whether it is open, and which graph it is showing.
///
/// The selection is a graph key rather than a material key because parameters
/// belong to graphs. What a click on a wall does is resolve that wall's material
/// to the graph it was baked from, which is [`picked`].
#[derive(Resource, Default)]
pub(crate) struct Panel {
    /// Whether `M` has opened it.
    pub(crate) open: bool,
    /// The graph whose parameters are shown.
    pub(crate) selected: Option<String>,
    /// The spawned UI, while it is open.
    root: Option<Entity>,
    /// Rebuild the rows before the next frame: opened, closed, selection moved,
    /// or the library reloaded under it.
    dirty: bool,
}

impl Panel {
    /// Show a different graph, if it is a different graph.
    pub(crate) fn select(&mut self, key: &str) {
        if self.selected.as_deref() != Some(key) {
            self.selected = Some(key.to_owned());
            self.dirty = true;
        }
    }

    /// Rebuild the rows before the next frame.
    pub(crate) fn invalidate(&mut self) {
        self.dirty = true;
    }
}

/// Which parameter of the shown graph a widget edits.
///
/// `channel` is the component of a colour, and `None` for everything else. One
/// component on the slider is all the write-back needs: the graph key comes
/// from the panel, so a rebuilt panel cannot leave a slider pointing at the
/// graph it was built for.
#[derive(Component, Clone, Debug)]
pub(crate) struct Edits {
    param: String,
    channel: Option<usize>,
}

/// A text node showing one parameter's current value.
#[derive(Component, Clone, Debug)]
pub(crate) struct ReadOut(String);

/// A colour swatch showing one parameter's current value.
#[derive(Component, Clone, Debug)]
pub(crate) struct Swatch(String);

/// What one of the panel's buttons does.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Show the previous graph in the library.
    Previous,
    /// Show the next graph in the library.
    Next,
    /// Write the library back to RON.
    Save,
}

/// Whether the pointer belongs to the panel rather than to the camera.
///
/// Two cases, and the second is the one that bites: the pointer is over the
/// panel, or a slider is being dragged and the pointer has left the panel on
/// the way. Without the second, letting a slider run to the end of its track
/// spins the building.
#[derive(SystemParam)]
pub(crate) struct PointerGrab<'w, 's> {
    panel: Option<Res<'w, Panel>>,
    options: Option<Res<'w, Options>>,
    windows: Query<'w, 's, &'static Window, With<PrimaryWindow>>,
    dragging: Query<'w, 's, &'static SliderDragState>,
}

impl PointerGrab<'_, '_> {
    /// Whether the camera should answer the pointer at all this frame: not
    /// while a capture is being set up, and not while the panel has it.
    pub(crate) fn interactive(&self) -> bool {
        self.options
            .as_ref()
            .is_none_or(|options| options.screenshot.is_none())
            && !self.active()
    }

    /// Whether the panel has the pointer this frame.
    fn active(&self) -> bool {
        if !self.panel.as_ref().is_some_and(|panel| panel.open) {
            return false;
        }
        if self.dragging.iter().any(|drag| drag.dragging) {
            return true;
        }
        self.windows
            .iter()
            .filter_map(Window::cursor_position)
            .any(|cursor| cursor.x <= WIDTH)
    }
}

/// `M` opens and closes the panel.
pub(crate) fn toggle(
    keys: Res<ButtonInput<KeyCode>>,
    mut panel: ResMut<Panel>,
    options: Option<Res<Options>>,
) {
    if options.is_some_and(|o| o.screenshot.is_some()) {
        return;
    }
    if keys.just_pressed(KeyCode::KeyM) {
        panel.open = !panel.open;
        panel.invalidate();
    }
}

/// Click a wall to show the graph it was baked from.
///
/// The picking backend answers with the entity under the pointer, and a preview
/// object carries the material key it was spawned with, so the rest is the
/// material library: a `Graph` surface names its graph, and a `Files` surface
/// records the one it came from under `baked_from`. A surface that is neither —
/// the study's glass, its light strip — leaves the selection alone rather than
/// emptying the panel, because a click that lands on the sky should not clear
/// what is being edited.
///
/// The guard is the drag: the same left button orbits the camera, and picking
/// reports a click whenever a press and a release share an entity however far
/// the pointer travelled between them. A click that moved more than a few
/// pixels was an orbit.
pub(crate) fn picked(
    click: On<Pointer<Click>>,
    keys: Query<&crate::viewer::SurfaceKey>,
    definitions: Res<crate::Definitions>,
    mut panel: ResMut<Panel>,
    origin: Res<PressOrigin>,
) {
    // Read rather than taken: one press can raise a click on more than one
    // entity, and the second of them is no less a click than the first.
    let travelled = origin
        .0
        .is_none_or(|from| from.distance(click.pointer_location.position) > 4.0);
    if travelled || !panel.open {
        return;
    }
    let Ok(key) = keys.get(click.entity) else {
        return;
    };
    let Some(library) = definitions.0.as_ref() else {
        return;
    };
    let Some(definition) = library.materials.get(&key.0) else {
        return;
    };
    if let Some(graph) = graph_of(&definition.surface) {
        panel.select(graph);
    }
}

/// The graph a surface came from, if it names one.
///
/// A baked surface names its graph; a textured one records the graph its files
/// were written from, which is the same answer to "what made this wall" and the
/// reason `--bake` can flip between the two; a compiled one *is* its graph.
/// Everything else — constants, and a hand-painted set with no provenance —
/// answers nothing, and a click on one leaves the panel showing what it was
/// showing.
///
/// What a slider then costs depends on which of the three it was, and that is
/// the whole of what the partition bought. A live parameter of a compiled
/// surface is a uniform: [`redress`](crate::reload::redress) writes the new
/// block and the wall changes in the same frame, on every step of the drag. A
/// baked parameter of anything, live or not, is a bake or a recompile, and
/// [`rebake`](crate::reload::rebake) does it once when the pointer lets go.
pub(crate) fn graph_of(surface: &ashlar::Surface) -> Option<&str> {
    match surface {
        ashlar::Surface::Graph(bake)
        | ashlar::Surface::Files {
            baked_from: Some(bake),
            ..
        } => Some(&bake.graph),
        ashlar::Surface::Shader { graph, .. } => Some(graph),
        _ => None,
    }
}

/// Where the pointer was when the button went down, so that [`picked`] can tell
/// a click from an orbit.
#[derive(Resource, Default)]
pub(crate) struct PressOrigin(Option<Vec2>);

/// Remember where a press started.
pub(crate) fn pressed(press: On<Pointer<Press>>, mut origin: ResMut<PressOrigin>) {
    origin.0 = Some(press.pointer_location.position);
}

/// Spawn or respawn the panel when something about it changed.
///
/// Rebuilt whole rather than patched: a graph has a dozen parameters at most,
/// the rows differ in kind and not just in value, and the values themselves are
/// carried every frame by [`readouts`] anyway.
pub(crate) fn rebuild(
    mut commands: Commands,
    mut panel: ResMut<Panel>,
    graphs: Res<crate::Graphs>,
    definitions: Res<crate::Definitions>,
    source: Res<GraphSource>,
) {
    if !panel.dirty {
        return;
    }
    panel.dirty = false;
    if let Some(root) = panel.root.take() {
        commands.entity(root).despawn();
    }
    if !panel.open {
        return;
    }
    if panel
        .selected
        .as_ref()
        .is_none_or(|key| !graphs.0.graphs.contains_key(key))
    {
        panel.selected = graphs.0.graphs.keys().next().cloned();
    }
    let selected = panel.selected.clone();
    let root = commands
        .spawn((
            Name::new("parameter panel"),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                bottom: Val::Px(0.0),
                width: Val::Px(WIDTH),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(12.0)),
                row_gap: Val::Px(6.0),
                ..default()
            },
            BackgroundColor(BACKDROP),
        ))
        .id();
    commands.entity(root).with_children(|panel| {
        panel.spawn(heading("Graph parameters"));
        let Some(key) = selected.as_deref() else {
            panel.spawn(line(
                "This scene brought no material graphs. Pass --graphs to name a library.",
                MUTED,
            ));
            return;
        };
        panel
            .spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(6.0),
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|row| {
                row.spawn(button("<", Action::Previous));
                row.spawn((
                    line(key, INK),
                    Node {
                        flex_grow: 1.0,
                        ..default()
                    },
                ));
                row.spawn(button(">", Action::Next));
            });
        panel.spawn(line(&reach(&definitions, key), MUTED));
        let Some(graph) = graphs.0.get(key) else {
            return;
        };
        if graph.params.is_empty() {
            panel.spawn(line(
                "This graph exposes no parameters. Add a Param to the graph to get a slider.",
                MUTED,
            ));
        }
        for param in &graph.params {
            rows(panel, param);
        }
        panel
            .spawn(Node {
                margin: UiRect::top(Val::Px(10.0)),
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(6.0),
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|row| {
                row.spawn(button("Save", Action::Save));
                row.spawn(line(&destination(&source), MUTED));
            });
    });
    panel.root = Some(root);
}

/// What the selected graph reaches in this scene, said plainly.
///
/// A `Files` surface loads the files a content step wrote, so editing the
/// graph it records changes nothing on screen until the surface is baked
/// instead. That is a sentence rather than a silence, because a slider that
/// visibly does nothing reads as a broken slider.
fn reach(definitions: &crate::Definitions, key: &str) -> String {
    let Some(library) = definitions.0.as_ref() else {
        return "No material library is loaded; nothing renders from this graph.".into();
    };
    let (mut baked, mut filed) = (0, 0);
    for definition in library.materials.values() {
        match &definition.surface {
            ashlar::Surface::Graph(bake) if bake.graph == key => baked += 1,
            ashlar::Surface::Files {
                baked_from: Some(bake),
                ..
            } if bake.graph == key => filed += 1,
            _ => {}
        }
    }
    match (baked, filed) {
        (0, 0) => "No surface in this scene uses this graph.".into(),
        (0, filed) => format!(
            "{} load files baked from this graph; pass --bake to render the graph itself.",
            surfaces(filed)
        ),
        (baked, 0) => format!("{} bake from this graph.", surfaces(baked)),
        (baked, filed) => format!(
            "{} bake from this graph, {} load files.",
            surfaces(baked),
            surfaces(filed)
        ),
    }
}

/// "1 surface" or "4 surfaces", because a panel that says "1 surface(s)" reads
/// like a panel nobody finished.
fn surfaces(count: usize) -> String {
    if count == 1 {
        "1 surface".to_owned()
    } else {
        format!("{count} surfaces")
    }
}

/// Where a save would go, for the line beside the button.
///
/// The file's name rather than its path: the panel is a third of a narrow
/// window wide, and an asset root reached through a crate directory wraps over
/// four lines and says nothing the file name does not. The whole path is
/// printed to the terminal when a save actually happens.
fn destination(source: &GraphSource) -> String {
    source.destination().map_or_else(
        || "nowhere to save: this scene brought no graph library".to_owned(),
        |path| {
            format!(
                "S or Save writes {}",
                path.file_name().unwrap_or(path.as_os_str()).display()
            )
        },
    )
}

/// The widgets one parameter needs: a slider, three of them, or a checkbox.
fn rows(panel: &mut ChildSpawnerCommands<'_>, param: &Param) {
    panel
        .spawn((
            Node {
                margin: UiRect::top(Val::Px(8.0)),
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                ..default()
            },
            Name::new(param.name.clone()),
        ))
        .with_children(|row| {
            row.spawn(line(&label(param), INK));
            row.spawn((
                line(&describe(&param.value), MUTED),
                ReadOut(param.name.clone()),
            ));
        });
    match &param.value {
        ParamValue::Float(value) => {
            let (low, high) = bounds(*value, param.range);
            panel.spawn(slider(&param.name, None, *value, low, high, false));
        }
        ParamValue::Int(value) => {
            #[expect(
                clippy::cast_precision_loss,
                reason = "an authored count on its way to a slider, not a bit pattern"
            )]
            let value = *value as f32;
            let (low, high) = bounds(value, param.range);
            panel.spawn(slider(&param.name, None, value, low, high, true));
        }
        ParamValue::Color(rgb) => {
            let high = rgb.iter().fold(1.0_f32, |high, c| high.max(*c));
            for (channel, value) in rgb.iter().enumerate() {
                panel.spawn(slider(&param.name, Some(channel), *value, 0.0, high, false));
            }
            panel.spawn((
                Node {
                    height: Val::Px(10.0),
                    margin: UiRect::top(Val::Px(2.0)),
                    ..default()
                },
                BackgroundColor(Color::linear_rgb(rgb[0], rgb[1], rgb[2])),
                Swatch(param.name.clone()),
            ));
        }
        ParamValue::Bool(value) => {
            let mut entity = panel.spawn((
                Node {
                    width: Val::Px(28.0),
                    height: Val::Px(ROW),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BorderColor::all(CHROME),
                BackgroundColor(if *value { HANDLE } else { TRACK }),
                Checkbox,
                Edits {
                    param: param.name.clone(),
                    channel: None,
                },
            ));
            if *value {
                entity.insert(bevy::ui::Checked);
            }
        }
    }
}

/// The name a row carries, saying where its bounds came from.
///
/// A parameter that declared a range gets it; one that did not gets bounds
/// derived from its own default, and says so, because a slider whose ends were
/// invented is a slider whose ends mean nothing.
fn label(param: &Param) -> String {
    match (&param.value, param.range) {
        (ParamValue::Bool(_) | ParamValue::Color(_), _) | (_, Some(_)) => param.name.clone(),
        (_, None) => format!("{} (derived range)", param.name),
    }
}

/// Slider bounds for a numeric parameter: the declared range, or one built
/// around the default so that the default is reachable and so is zero.
pub(crate) fn bounds(value: f32, declared: Option<(f32, f32)>) -> (f32, f32) {
    if let Some((low, high)) = declared
        && high > low
    {
        return (low, high);
    }
    if (0.0..=1.0).contains(&value) {
        (0.0, 1.0)
    } else if value > 0.0 {
        (0.0, value * 2.0)
    } else {
        (value * 2.0, 0.0)
    }
}

/// One parameter value as the panel prints it.
pub(crate) fn describe(value: &ParamValue) -> String {
    match value {
        ParamValue::Float(value) => format!("{value:.3}"),
        ParamValue::Int(value) => format!("{value}"),
        ParamValue::Bool(value) => (if *value { "on" } else { "off" }).to_owned(),
        ParamValue::Color(rgb) => format!("{:.3} {:.3} {:.3}", rgb[0], rgb[1], rgb[2]),
    }
}

/// A track with a thumb, as [`Slider`] wants it: the inner track is inset by the
/// thumb's width so the thumb's position is a plain percentage of it, which is
/// the arrangement the widget's own drag arithmetic assumes.
fn slider(
    param: &str,
    channel: Option<usize>,
    value: f32,
    low: f32,
    high: f32,
    integer: bool,
) -> impl Bundle {
    (
        Node {
            height: Val::Px(ROW),
            justify_content: JustifyContent::Center,
            ..default()
        },
        BackgroundColor(TRACK),
        Slider::default(),
        SliderValue(value),
        SliderRange::new(low, high),
        SliderStep(if integer { 1.0 } else { (high - low) / 100.0 }),
        Edits {
            param: param.to_owned(),
            channel,
        },
        children![(
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(THUMB),
                top: Val::Px(0.0),
                bottom: Val::Px(0.0),
                ..default()
            },
            children![(
                SliderThumb,
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Px(THUMB),
                    height: Val::Px(ROW),
                    left: Val::Percent(0.0),
                    ..default()
                },
                BackgroundColor(HANDLE),
            )],
        )],
    )
}

fn button(text: &str, action: Action) -> impl Bundle {
    (
        Node {
            padding: UiRect::axes(Val::Px(8.0), Val::Px(2.0)),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BorderColor::all(CHROME),
        BackgroundColor(TRACK),
        WidgetButton,
        action,
        children![line(text, INK)],
    )
}

fn heading(text: &str) -> impl Bundle {
    (
        Text::new(text.to_owned()),
        TextFont::from_font_size(14.0),
        TextColor(INK),
    )
}

fn line(text: &str, color: Color) -> impl Bundle {
    (
        Text::new(text.to_owned()),
        TextFont::from_font_size(11.0),
        TextColor(color),
    )
}

/// Put every thumb where its slider's value says.
///
/// The widget deliberately does not move the thumb itself: where it sits is the
/// look of the slider, and the look belongs to whoever drew it.
pub(crate) fn thumbs(
    sliders: Query<(Entity, &SliderValue, &SliderRange), With<Slider>>,
    children: Query<&Children>,
    mut thumbs: Query<&mut Node, With<SliderThumb>>,
) {
    for (slider, value, range) in &sliders {
        for child in children.iter_descendants(slider) {
            if let Ok(mut node) = thumbs.get_mut(child) {
                node.left = Val::Percent(range.thumb_position(value.0) * 100.0);
            }
        }
    }
}

/// Keep the printed values and the colour swatch on the library.
///
/// Read from the library rather than from the sliders, so what the panel shows
/// is what a bake would use — including after a file reload moved a value the
/// panel was not the one to change.
pub(crate) fn readouts(
    graphs: Res<crate::Graphs>,
    panel: Res<Panel>,
    mut labels: Query<(&ReadOut, &mut Text)>,
    mut swatches: Query<(&Swatch, &mut BackgroundColor)>,
) {
    let Some(graph) = panel.selected.as_deref().and_then(|key| graphs.0.get(key)) else {
        return;
    };
    for (readout, mut text) in &mut labels {
        if let Some(param) = graph.param(&readout.0) {
            let shown = describe(&param.value);
            if text.0 != shown {
                text.0 = shown;
            }
        }
    }
    for (swatch, mut color) in &mut swatches {
        if let Some(Param {
            value: ParamValue::Color(rgb),
            ..
        }) = graph.param(&swatch.0)
        {
            color.0 = Color::linear_rgb(rgb[0], rgb[1], rgb[2]);
        }
    }
}

/// Write one slider's value into the library, drive the compiled surfaces, and
/// ask for a bake on release.
///
/// The two halves of the drag, and which is which is the whole of what the
/// partition decided. Every step asks for a [`Redress`]: a live parameter is a
/// uniform, so a compiled wall follows the pointer for the cost of writing
/// sixteen `vec4`s. Only the release asks for a [`Rebake`], because a baked
/// surface is a fifth of a second of rasterising and a compiled one whose
/// parameter was folded is that plus a pipeline.
pub(crate) fn dragged(
    change: On<ValueChange<f32>>,
    sliders: Query<&Edits>,
    panel: Res<Panel>,
    mut graphs: ResMut<crate::Graphs>,
    mut rebake: ResMut<Rebake>,
    mut redress: ResMut<Redress>,
    mut commands: Commands,
) {
    let Ok(edits) = sliders.get(change.source) else {
        return;
    };
    let Some(key) = panel.selected.clone() else {
        return;
    };
    let value = edit(&mut graphs, &key, edits, change.value);
    commands.entity(change.source).insert(SliderValue(value));
    redress.request();
    if change.is_final {
        rebake.request();
    }
}

/// Write one checkbox's state into the library, and bake.
///
/// No release to wait for: a switch is one click, and it is either the graph it
/// was or the other one.
pub(crate) fn toggled(
    change: On<ValueChange<bool>>,
    boxes: Query<&Edits>,
    panel: Res<Panel>,
    mut graphs: ResMut<crate::Graphs>,
    mut rebake: ResMut<Rebake>,
    mut redress: ResMut<Redress>,
    mut commands: Commands,
) {
    let Ok(edits) = boxes.get(change.source) else {
        return;
    };
    let Some(key) = panel.selected.clone() else {
        return;
    };
    edit(&mut graphs, &key, edits, f32::from(u8::from(change.value)));
    let mut entity = commands.entity(change.source);
    if change.value {
        entity.insert(bevy::ui::Checked);
    } else {
        entity.remove::<bevy::ui::Checked>();
    }
    entity.insert(BackgroundColor(if change.value { HANDLE } else { TRACK }));
    redress.request();
    rebake.request();
}

/// Set one parameter of one graph, answering what the graph now holds.
///
/// The answer is not always the argument: an `Int` rounds, and a slider that is
/// told what it stored keeps its thumb where the value really is rather than
/// where the pointer was.
pub(crate) fn edit(graphs: &mut crate::Graphs, key: &str, edits: &Edits, value: f32) -> f32 {
    let Some(param) = graphs
        .0
        .graphs
        .get_mut(key)
        .and_then(|graph| graph.params.iter_mut().find(|p| p.name == edits.param))
    else {
        return value;
    };
    match (&mut param.value, edits.channel) {
        (ParamValue::Float(held), _) => {
            *held = value;
            value
        }
        (ParamValue::Int(held), _) => {
            let rounded = value.round();
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a slider bounded by the parameter's own range; the cast saturates"
            )]
            {
                *held = rounded as i32;
            }
            rounded
        }
        (ParamValue::Bool(held), _) => {
            *held = value != 0.0;
            value
        }
        // A colour edited without a channel, or past the third: neither is
        // something the panel builds, and neither is a value to write.
        (ParamValue::Color(rgb), channel) => {
            if let Some(channel) = channel.filter(|channel| *channel < rgb.len()) {
                rgb[channel] = value;
            }
            value
        }
    }
}

/// The panel's buttons: move through the library, or write it back.
pub(crate) fn activated(
    activate: On<Activate>,
    buttons: Query<&Action>,
    graphs: Res<crate::Graphs>,
    mut source: ResMut<GraphSource>,
    mut panel: ResMut<Panel>,
) {
    let Ok(action) = buttons.get(activate.entity) else {
        return;
    };
    match action {
        Action::Previous | Action::Next => {
            if let Some(key) = step(&graphs, panel.selected.as_deref(), *action == Action::Next) {
                panel.select(&key);
            }
        }
        Action::Save => match crate::reload::save(&graphs, &mut source) {
            Ok(path) => println!("Wrote {}", path.display()),
            Err(error) => eprintln!("Save failed: {error:#}"),
        },
    }
}

/// `S` saves, as the button does.
pub(crate) fn shortcuts(
    keys: Res<ButtonInput<KeyCode>>,
    graphs: Res<crate::Graphs>,
    panel: Res<Panel>,
    mut source: ResMut<GraphSource>,
    options: Option<Res<Options>>,
) {
    if options.is_some_and(|o| o.screenshot.is_some()) || !panel.open {
        return;
    }
    if keys.just_pressed(KeyCode::KeyS) {
        match crate::reload::save(&graphs, &mut source) {
            Ok(path) => println!("Wrote {}", path.display()),
            Err(error) => eprintln!("Save failed: {error:#}"),
        }
    }
}

/// The key before or after `current` in the library, wrapping.
pub(crate) fn step(graphs: &crate::Graphs, current: Option<&str>, forward: bool) -> Option<String> {
    let keys = graphs.0.graphs.keys().collect::<Vec<_>>();
    if keys.is_empty() {
        return None;
    }
    let at = current
        .and_then(|key| keys.iter().position(|held| held.as_str() == key))
        .unwrap_or(0);
    let next = if forward {
        (at + 1) % keys.len()
    } else {
        (at + keys.len() - 1) % keys.len()
    };
    Some(keys[next].clone())
}

/// Rebuild the panel when the file replaced the library it was showing.
///
/// The flag rather than `Res::is_changed`: the panel's own sliders write to the
/// same resource on every step of a drag, and a rebuild there would despawn the
/// slider under the pointer.
pub(crate) fn follow_reload(mut reloaded: ResMut<Reloaded>, mut panel: ResMut<Panel>) {
    if std::mem::take(&mut reloaded.0) && panel.open {
        panel.invalidate();
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "exact fixtures: a slider stores what it was given, or rounds it to an integer"
)]
mod tests {
    use std::collections::BTreeMap;

    use ashlar::{Bake, MaterialDefinition, MaterialLibrary, Surface};
    use ashlar_material::{MaterialGraph, MaterialGraphLibrary, PbrOutput, nodes::Noise};
    use tempfile::TempDir;

    use super::*;
    use crate::reload::tests::{KEY, harness, library, texture, write};

    /// A library of one graph carrying one parameter of each kind, for the
    /// write-back rules that do not need an app around them.
    fn assorted() -> crate::Graphs {
        let mut graphs = MaterialGraphLibrary::default();
        graphs.insert(
            MaterialGraph::builder("kinds")
                .param(Param::float("wear", 0.25).range(0.0, 1.0))
                .param(Param::int("rows", 4).range(1.0, 16.0))
                .param(Param::color("tint", [0.4, 0.3, 0.2]))
                .param(Param::bool("wet", false))
                .node("grain", Noise::value().period(16))
                .output(PbrOutput::new().base_color("grain").roughness("grain"))
                .into_graph(),
        );
        crate::Graphs(graphs)
    }

    fn held(graphs: &crate::Graphs, key: &str, name: &str) -> ParamValue {
        graphs
            .0
            .get(key)
            .expect("graph")
            .param(name)
            .expect("param")
            .value
    }

    fn edits(param: &str, channel: Option<usize>) -> Edits {
        Edits {
            param: param.to_owned(),
            channel,
        }
    }

    #[test]
    fn a_slider_writes_the_kind_of_value_its_parameter_holds() {
        let mut graphs = assorted();
        assert_eq!(edit(&mut graphs, "kinds", &edits("wear", None), 0.75), 0.75);
        assert_eq!(held(&graphs, "kinds", "wear"), ParamValue::Float(0.75));

        // An integer rounds, and the slider is told what was stored rather than
        // what the pointer said, so its thumb sits on the value that is real.
        assert_eq!(edit(&mut graphs, "kinds", &edits("rows", None), 6.4), 6.0);
        assert_eq!(held(&graphs, "kinds", "rows"), ParamValue::Int(6));

        assert_eq!(
            edit(&mut graphs, "kinds", &edits("tint", Some(1)), 0.9),
            0.9
        );
        assert_eq!(
            held(&graphs, "kinds", "tint"),
            ParamValue::Color([0.4, 0.9, 0.2])
        );

        assert_eq!(edit(&mut graphs, "kinds", &edits("wet", None), 1.0), 1.0);
        assert_eq!(held(&graphs, "kinds", "wet"), ParamValue::Bool(true));

        // Nothing a panel builds, and nothing that writes: an unknown parameter,
        // an unknown graph, a colour with no channel and a channel past the third.
        let before = assorted();
        let mut graphs = assorted();
        edit(&mut graphs, "kinds", &edits("absent", None), 1.0);
        edit(&mut graphs, "nosuch", &edits("wear", None), 1.0);
        edit(&mut graphs, "kinds", &edits("tint", None), 1.0);
        edit(&mut graphs, "kinds", &edits("tint", Some(7)), 1.0);
        assert_eq!(graphs.0, before.0);
    }

    #[test]
    fn a_parameter_that_declared_no_range_gets_one_that_reaches_its_own_default() {
        // Declared wins, whatever it is.
        assert_eq!(bounds(0.5, Some((-2.0, 3.0))), (-2.0, 3.0));
        // A default inside the unit interval is almost always a fraction of
        // something, so that is the range it gets.
        assert_eq!(bounds(0.25, None), (0.0, 1.0));
        assert_eq!(bounds(0.0, None), (0.0, 1.0));
        assert_eq!(bounds(1.0, None), (0.0, 1.0));
        // Outside it, the default sits in the middle of what it can reach, and
        // zero is still on the track.
        assert_eq!(bounds(8.0, None), (0.0, 16.0));
        assert_eq!(bounds(-3.0, None), (-6.0, 0.0));
        // A range that is not a range is no more use than none at all.
        assert_eq!(bounds(0.25, Some((1.0, 1.0))), (0.0, 1.0));
        // And the panel says which of the two a row is showing.
        assert!(label(&Param::float("wear", 0.25)).contains("derived"));
        assert!(!label(&Param::float("wear", 0.25).range(0.0, 1.0)).contains("derived"));
        assert!(!label(&Param::color("tint", [0.1; 3])).contains("derived"));
    }

    #[test]
    fn the_buttons_walk_the_library_in_its_own_order_and_wrap() {
        let mut graphs = MaterialGraphLibrary::default();
        for id in ["a", "b", "c"] {
            graphs.insert(
                MaterialGraph::builder(id)
                    .node("grain", Noise::value().period(16))
                    .output(PbrOutput::new().base_color("grain").roughness("grain"))
                    .into_graph(),
            );
        }
        let graphs = crate::Graphs(graphs);
        assert_eq!(step(&graphs, Some("a"), true).as_deref(), Some("b"));
        assert_eq!(step(&graphs, Some("c"), true).as_deref(), Some("a"));
        assert_eq!(step(&graphs, Some("a"), false).as_deref(), Some("c"));
        // A selection the library no longer holds lands on the first key rather
        // than on nothing, which is what a reload that renamed a graph leaves.
        assert_eq!(step(&graphs, Some("gone"), true).as_deref(), Some("b"));
        assert_eq!(step(&graphs, None, true).as_deref(), Some("b"));
        assert_eq!(step(&crate::Graphs::default(), None, true), None);
    }

    #[test]
    fn a_click_resolves_a_surface_to_the_graph_behind_it() {
        let bake = Bake {
            graph: "study:concrete".to_owned(),
            params: BTreeMap::new(),
            resolution: 256,
        };
        assert_eq!(
            graph_of(&Surface::Graph(bake.clone())),
            Some("study:concrete")
        );
        // A file surface answers the graph it was written from, which is what
        // makes clicking a shipped wall useful at all.
        assert_eq!(
            graph_of(&Surface::Files {
                base_color: Some("base.ktx2".to_owned()),
                normal: None,
                orm: None,
                height: None,
                emissive: None,
                baked_from: Some(bake),
            }),
            Some("study:concrete")
        );
        assert_eq!(graph_of(&Surface::Plain), None);
        assert_eq!(
            graph_of(&Surface::Files {
                base_color: Some("base.ktx2".to_owned()),
                normal: None,
                orm: None,
                height: None,
                emissive: None,
                baked_from: None,
            }),
            None
        );
    }

    #[test]
    fn the_panel_says_whether_the_graph_it_is_showing_reaches_anything() {
        let bake = |graph: &str| Bake {
            graph: graph.to_owned(),
            params: BTreeMap::new(),
            resolution: 256,
        };
        let mut materials = BTreeMap::new();
        materials.insert(
            "baked".to_owned(),
            MaterialDefinition {
                surface: Surface::Graph(bake("study:concrete")),
                ..default()
            },
        );
        materials.insert(
            "filed".to_owned(),
            MaterialDefinition {
                surface: Surface::Files {
                    base_color: Some("base.ktx2".to_owned()),
                    normal: None,
                    orm: None,
                    height: None,
                    emissive: None,
                    baked_from: Some(bake("study:metal")),
                },
                ..default()
            },
        );
        let definitions = crate::Definitions(Some(MaterialLibrary { materials }));
        assert!(reach(&definitions, "study:concrete").contains("bake from this graph"));
        // The honest sentence: editing this graph changes nothing on screen
        // until the surface using it is baked instead of loaded.
        let filed = reach(&definitions, "study:metal");
        assert!(filed.contains("load files"), "{filed}");
        assert!(filed.contains("--bake"), "{filed}");
        assert!(reach(&definitions, "study:brick").contains("No surface"));
        assert!(reach(&crate::Definitions(None), "study:concrete").contains("No material"));
    }

    #[test]
    fn m_opens_and_closes_the_panel_and_a_capture_ignores_it() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Panel>()
            .add_systems(Update, toggle);
        // Released and cleared before each press: a key that is already held
        // is not pressed again, and `just_pressed` is what the system reads.
        let press = |app: &mut App, key| {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.reset_all();
            keys.clear();
            keys.press(key);
            app.update();
        };
        press(&mut app, KeyCode::KeyM);
        assert!(app.world().resource::<Panel>().open);
        press(&mut app, KeyCode::KeyM);
        assert!(!app.world().resource::<Panel>().open);
        // Another key is not this one.
        press(&mut app, KeyCode::KeyF);
        assert!(!app.world().resource::<Panel>().open);

        // A capture is a fixed shot, and a panel across a third of it would be
        // in every reference image the gallery and the study compare.
        app.insert_resource(
            Options::try_parse_for(
                &crate::tests::catalog(),
                ["preview", "--screenshot", "/tmp/unused.png"],
            )
            .expect("options"),
        );
        press(&mut app, KeyCode::KeyM);
        assert!(!app.world().resource::<Panel>().open);
    }

    /// The panel's own edit path, without a window: the widget event, the
    /// library, the flag and the bake.
    ///
    /// A slider is an entity carrying [`Edits`] and a value change is an event
    /// aimed at it, so the whole of what the panel does to a graph can be driven
    /// from a test that never lays out a node.
    #[test]
    fn a_slider_edits_the_library_as_it_moves_and_bakes_when_it_is_let_go() {
        let directory = TempDir::new().expect("temp dir");
        let path = directory.path().join("materials.graphs.ron");
        write(&path, &library(1));
        let mut app = harness(&path);
        app.init_resource::<Panel>().add_observer(dragged);
        app.world_mut().resource_mut::<Panel>().selected = Some("test:wall".to_owned());
        let slider = app
            .world_mut()
            .spawn((edits("wear", None), SliderValue(0.25)))
            .id();

        // Mid-drag: the library moves, and nothing bakes. A 1024 bake is a fifth
        // of a second, which is a fine pause after a drag and an impossible one
        // during it.
        app.world_mut().trigger(ValueChange {
            source: slider,
            value: 0.9_f32,
            is_final: false,
        });
        assert_eq!(
            held(app.world().resource::<crate::Graphs>(), "test:wall", "wear"),
            ParamValue::Float(0.9)
        );
        assert!(!app.world().resource::<crate::reload::Rebake>().0);
        // A redress, though, on every step: a compiled surface's live parameters
        // are a uniform, and writing sixteen bytes under the pointer is what a
        // slider over one is for.
        assert!(app.world().resource::<crate::reload::Redress>().0);
        app.update();
        assert!(texture(&app).is_none(), "no bake while the pointer is down");

        // Released: one bake, and the material the scene holds carries it.
        app.world_mut().trigger(ValueChange {
            source: slider,
            value: 0.6_f32,
            is_final: true,
        });
        assert!(app.world().resource::<crate::reload::Rebake>().0);
        app.update();
        let baked = texture(&app).expect("the release baked");
        let handle = app.world().resource::<crate::viewer::Palette>().handles
            [&ashlar::Binding::new(KEY)]
            .clone();
        assert_eq!(
            app.world()
                .resource::<Assets<StandardMaterial>>()
                .get(&handle)
                .expect("the scene's material")
                .base_color_texture,
            Some(baked)
        );
        // The slider was told what the graph stored, so its thumb and the
        // parameter agree.
        assert_eq!(
            app.world().get::<SliderValue>(slider),
            Some(&SliderValue(0.6))
        );
    }
}
