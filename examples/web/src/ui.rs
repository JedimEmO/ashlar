//! The side panel: one small window over the view, which is all the interface
//! the demo has.
use ashlar::ParamValue;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use crate::{
    LodView, Mode, Quality, View,
    camera::{NARROW, Rig},
    catalog,
    gallery::{BakeState, Gallery},
    scenes::{Load, Status, TINTS},
};

/// Registers the panel.
pub(crate) fn plugin(app: &mut App) {
    app.add_systems(EguiPrimaryContextPass, panel);
}

/// Where the rest of the site is, relative to the demo's own page.
const MANUAL: &str = "../book/";
const API: &str = "../api/ashlar/index.html";
const REPOSITORY: &str = "https://github.com/JedimEmO/ashlar";

/// The panel's width, in points, where the window has room for it.
const PANEL_WIDTH: f32 = 300.0;

/// The gap between the panel and the window's edges, in points.
const MARGIN: f32 = 12.0;

/// The panel's accent: the warm grey-gold of dressed sandstone.
const ACCENT: egui::Color32 = egui::Color32::from_rgb(214, 178, 112);

/// Dark, a little translucent, rounded, with the accent on what is selected.
fn style(ctx: &egui::Context, quality: Quality) {
    let mut visuals = egui::Visuals::dark();
    visuals.window_fill = egui::Color32::from_rgba_unmultiplied(18, 20, 24, 232);
    visuals.window_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(52, 56, 64));
    visuals.window_corner_radius = egui::CornerRadius::same(10);
    visuals.selection.bg_fill = egui::Color32::from_rgb(120, 96, 52);
    visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    visuals.hyperlink_color = ACCENT;
    visuals.slider_trailing_fill = true;
    ctx.set_visuals(visuals);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.slider_width = 150.0;
        // A fingertip is about forty points across; a mouse needs a third of
        // that.
        if quality == Quality::Handheld {
            style.spacing.interact_size.y = 32.0;
            style.spacing.button_padding = egui::vec2(10.0, 6.0);
            style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        }
    });
}

/// A section heading.
fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(text).small().strong().color(ACCENT));
}

#[expect(
    clippy::too_many_arguments,
    reason = "a Bevy system's parameters are its inputs"
)]
fn panel(
    mut contexts: EguiContexts,
    mut styled: Local<bool>,
    mut view: ResMut<View>,
    mut mode: ResMut<Mode>,
    status: Res<Status>,
    gallery: Option<ResMut<Gallery>>,
    mut rigs: Query<&mut Rig>,
    quality: Res<Quality>,
    mut folded: Local<bool>,
    mut was_narrow: Local<Option<bool>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    if !*styled {
        style(ctx, *quality);
        *styled = true;
    }
    let screen = ctx.content_rect();
    // On a phone the panel lies over the view rather than beside it, so it
    // folds to its first row whenever the window becomes narrow, and is as
    // wide as the screen allows. Decided on a change rather than once: the
    // first frames are drawn before a browser has said how big the page is.
    let narrow = screen.width() < NARROW;
    if *was_narrow != Some(narrow) {
        *was_narrow = Some(narrow);
        *folded = narrow;
    }
    let folded = &mut *folded;
    let width = PANEL_WIDTH.min(screen.width() - 2.0 * MARGIN - 16.0);
    egui::Window::new("ashlar")
        .title_bar(false)
        .resizable(false)
        .anchor(egui::Align2::LEFT_TOP, [MARGIN, MARGIN])
        .default_width(width)
        .max_height(screen.height() - 2.0 * MARGIN)
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("ashlar")
                        .size(26.0)
                        .strong()
                        .color(ACCENT),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Folds the panel to this one row, for a small screen.
                    if ui
                        .button(if *folded { "+" } else { "\u{2212}" })
                        .on_hover_text(if *folded {
                            "Show the panel"
                        } else {
                            "Fold the panel"
                        })
                        .clicked()
                    {
                        *folded = !*folded;
                    }
                    ui.hyperlink_to("GitHub", REPOSITORY);
                    ui.hyperlink_to("API", API);
                    ui.hyperlink_to("Manual", MANUAL);
                });
            });
            if *folded {
                return;
            }
            ui.label(
                egui::RichText::new(
                    "Procedural architecture for Rust games: buildings and materials as Rust, \
                     baked into files a Bevy game loads.",
                )
                .small()
                .weak(),
            );
            ui.separator();
            // Mode switch: set through a copy so change detection only fires
            // when it really changes.
            let mut wanted = *mode;
            ui.horizontal(|ui| {
                ui.selectable_value(&mut wanted, Mode::Scenes, "Scenes");
                ui.selectable_value(&mut wanted, Mode::Materials, "Materials");
            });
            if wanted != *mode {
                *mode = wanted;
            }
            egui::ScrollArea::vertical()
                .auto_shrink([false, true])
                .show(ui, |ui| match *mode {
                    Mode::Scenes => scenes(ui, &mut view, &status, &mut rigs),
                    Mode::Materials => {
                        if let Some(mut gallery) = gallery {
                            materials(ui, &mut gallery);
                        }
                    }
                });
            ui.separator();
            ui.collapsing("Controls", |ui| controls(ui, view.walk));
        });
    Ok(())
}

/// The scene half of the panel.
fn scenes(ui: &mut egui::Ui, view: &mut ResMut<View>, status: &Status, rigs: &mut Query<&mut Rig>) {
    let current = catalog::SCENES[view.scene];
    heading(ui, "SCENE");
    let mut scene = view.scene;
    egui::ComboBox::from_id_salt("scene")
        .width(ui.available_width())
        .selected_text(current.title)
        .show_ui(ui, |ui| {
            for (index, option) in catalog::SCENES.iter().enumerate() {
                ui.selectable_value(&mut scene, index, option.title);
            }
        });
    if scene != view.scene {
        let next = catalog::SCENES[scene];
        view.scene = scene;
        view.night = next.night;
        view.max_storey = None;
        view.hide_exterior = false;
        view.lod = LodView::Bands;
    }
    ui.label(egui::RichText::new(current.blurb).weak());
    match &status.load {
        Load::Idle | Load::Loading => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading the baked building…");
            });
        }
        Load::Failed(reason) => {
            ui.colored_label(egui::Color32::from_rgb(230, 110, 100), reason);
        }
        Load::Ready => {}
    }

    heading(ui, "LIGHT AND CAMERA");
    ui.horizontal(|ui| {
        let mut night = view.night;
        ui.selectable_value(&mut night, false, "Day");
        ui.selectable_value(&mut night, true, "Night");
        if night != view.night {
            view.night = night;
        }
        ui.separator();
        let mut walk = view.walk;
        ui.selectable_value(&mut walk, false, "Orbit");
        ui.selectable_value(&mut walk, true, "Walk");
        if walk != view.walk {
            view.walk = walk;
        }
    });
    if ui
        .button("Street level")
        .on_hover_text("Stand at head height in front of the building, walking")
        .clicked()
    {
        for mut rig in rigs.iter_mut() {
            rig.street_level();
        }
        view.walk = true;
    }

    let Some(info) = &status.info else {
        return;
    };
    if let Some((low, high)) = info.storeys
        && high > low
    {
        heading(ui, "STOREYS");
        let mut cut = view.max_storey.is_some();
        let mut storey = view.max_storey.unwrap_or(high);
        ui.horizontal(|ui| {
            ui.checkbox(&mut cut, "Hide above");
            ui.add_enabled(cut, egui::Slider::new(&mut storey, low..=high));
        });
        let wanted = cut.then_some(storey);
        if wanted != view.max_storey {
            view.max_storey = wanted;
        }
    }
    let mut hide = view.hide_exterior;
    ui.checkbox(&mut hide, "Hide the exterior, to see inside")
        .on_hover_text("Hides every piece seen from outside: walls' outer faces, roofs, facades");
    if hide != view.hide_exterior {
        view.hide_exterior = hide;
    }

    levels(ui, view, info);
}

/// The level-of-detail views, and a line per level: its triangles, where it
/// stops, and its tint as the tinted view's key; and by night, the lamps.
fn levels(ui: &mut egui::Ui, view: &mut ResMut<View>, info: &crate::scenes::Info) {
    heading(ui, "LEVELS OF DETAIL");
    let mut lod = view.lod;
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut lod, LodView::Bands, "By distance");
        ui.selectable_value(&mut lod, LodView::Tint, "Tinted");
        for level in 0..info.levels.len() {
            ui.selectable_value(&mut lod, LodView::Force(level), format!("L{level}"));
        }
    });
    if lod != view.lod {
        view.lod = lod;
    }
    egui::Grid::new("levels").striped(true).show(ui, |ui| {
        for (level, (triangles, until)) in info.levels.iter().enumerate() {
            // The level's tint beside its name, as the tinted view's key.
            let [r, g, b, _] = TINTS[level.min(TINTS.len() - 1)].to_srgba().to_u8_array();
            ui.horizontal(|ui| {
                ui.colored_label(egui::Color32::from_rgb(r, g, b), "\u{25A0}");
                ui.label(format!("L{level}"));
            });
            ui.label(format!("{} triangles", thousands(*triangles)));
            ui.label(until.map_or_else(
                || "to any distance".to_owned(),
                |d| format!("until {d:.0} m"),
            ));
            ui.end_row();
        }
    });
    if view.night && info.lamps > 0 {
        ui.label(
            egui::RichText::new(format!(
                "{} lamps; the nearest are lit as you move.",
                info.lamps
            ))
            .small()
            .weak(),
        );
    }
}

/// `46272` as `46 272`.
fn thousands(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push('\u{2009}');
        }
        out.push(digit);
    }
    out
}

/// The material half of the panel.
fn materials(ui: &mut egui::Ui, gallery: &mut Gallery) {
    heading(ui, "MATERIAL");
    let mut selected = gallery.selected.clone();
    egui::ComboBox::from_id_salt("material")
        .width(ui.available_width())
        .height(360.0)
        .selected_text(title(&selected))
        .show_ui(ui, |ui| {
            for key in &gallery.keys {
                ui.selectable_value(&mut selected, key.clone(), title(key));
            }
        });
    if selected != gallery.selected {
        gallery.selected = selected;
    }
    let description = catalog::describe(&gallery.selected);
    if !description.is_empty() {
        ui.label(egui::RichText::new(description).weak());
    }
    ui.label(
        egui::RichText::new(format!("graph {}", gallery.selected))
            .small()
            .monospace()
            .weak(),
    );

    heading(ui, "PARAMETERS");
    if gallery.rows.is_empty() {
        ui.label(egui::RichText::new("This graph has no parameters.").weak());
    }
    let mut moved = false;
    egui::Grid::new("params")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            for row in &mut gallery.rows {
                ui.label(&row.param.name);
                moved |= match &mut row.value {
                    ParamValue::Float(value) => {
                        let (low, high) = bounds(*value, row.param.range);
                        ui.add(egui::Slider::new(value, low..=high)).changed()
                    }
                    ParamValue::Int(value) => {
                        #[expect(
                            clippy::cast_possible_truncation,
                            clippy::cast_precision_loss,
                            reason = "a slider bounded by the parameter's own range"
                        )]
                        let (low, high) = {
                            let (low, high) = bounds(*value as f32, row.param.range);
                            (low.floor() as i32, high.ceil() as i32)
                        };
                        ui.add(egui::Slider::new(value, low..=high)).changed()
                    }
                    ParamValue::Bool(value) => ui.checkbox(value, "").changed(),
                    ParamValue::Color(rgb) => {
                        // The graph's colours are linear; the picker shows
                        // sRGB, so convert both ways.
                        let linear = LinearRgba::rgb(rgb[0], rgb[1], rgb[2]);
                        let srgb = Srgba::from(linear);
                        let mut shown = [srgb.red, srgb.green, srgb.blue];
                        let changed = ui.color_edit_button_rgb(&mut shown).changed();
                        if changed {
                            let back = LinearRgba::from(Srgba::rgb(shown[0], shown[1], shown[2]));
                            *rgb = [back.red, back.green, back.blue];
                        }
                        changed
                    }
                };
                ui.end_row();
            }
        });
    if moved {
        gallery.touch();
    }
    ui.horizontal(|ui| {
        if ui.button("Reset").clicked() {
            gallery.reset();
        }
        match &gallery.state {
            BakeState::Files if gallery.changed.is_none() => {
                ui.label(egui::RichText::new("The maps the content step wrote.").weak());
            }
            BakeState::Baking => {
                ui.spinner();
                ui.label("Baking…");
            }
            _ if gallery.changed.is_some() => {
                ui.label(egui::RichText::new("Waiting for the slider to rest…").weak());
            }
            BakeState::Baked { resolution, millis } => {
                ui.label(
                    egui::RichText::new(format!(
                        "Re-baked at {resolution}² on one thread in {millis:.0} ms."
                    ))
                    .weak(),
                );
            }
            BakeState::Failed(reason) => {
                ui.colored_label(egui::Color32::from_rgb(230, 110, 100), reason);
            }
            BakeState::Files => {}
        }
    });
}

/// `library:clay-roof-tiles` as `Clay roof tiles`.
fn title(key: &str) -> String {
    let name = key.rsplit(':').next().unwrap_or(key).replace('-', " ");
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// Slider bounds for a numeric parameter: the declared range, or one built
/// around the default so that the default is reachable and so is zero. The
/// preview's rule.
fn bounds(value: f32, declared: Option<(f32, f32)>) -> (f32, f32) {
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

/// What the pointer and the keys do.
fn controls(ui: &mut egui::Ui, walk: bool) {
    let lines: &[(&str, &str)] = if walk {
        &[
            ("drag", "look around"),
            ("W A S D", "walk; shift runs"),
            ("E / Q", "up / down"),
            ("scroll, pinch", "step forward"),
        ]
    } else {
        &[
            ("drag, one finger", "turn"),
            ("right drag, two fingers", "pan"),
            ("scroll, pinch", "zoom"),
            ("W A S D", "move the focus"),
        ]
    };
    egui::Grid::new("controls").show(ui, |ui| {
        for (input, action) in lines {
            ui.label(egui::RichText::new(*input).monospace().small());
            ui.label(egui::RichText::new(*action).small());
            ui.end_row();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_keys_read_as_prose() {
        assert_eq!(thousands(46_272), "46\u{2009}272");
        assert_eq!(thousands(658), "658");
        assert_eq!(thousands(1_234_567), "1\u{2009}234\u{2009}567");
        assert_eq!(title("library:clay-roof-tiles"), "Clay roof tiles");
    }

    #[test]
    fn a_slider_reaches_its_default_and_zero() {
        assert_eq!(bounds(0.3, None), (0.0, 1.0));
        assert_eq!(bounds(7.0, None), (0.0, 14.0));
        assert_eq!(bounds(-2.0, None), (-4.0, 0.0));
        assert_eq!(bounds(3.0, Some((1.0, 5.0))), (1.0, 5.0));
    }
}
