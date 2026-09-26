//! The camera: an orbit around a focus, and a walk at street level.
//!
//! Orbit is the default because a building is looked *at*: drag to turn, right
//! drag or two fingers to pan, scroll or pinch to zoom. Walk is for the city,
//! which is looked at from inside: the same drag turns the head and the keys
//! move the feet. Both read the pointer only when the panel does not want it.
use bevy::{
    input::{
        mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit},
        touch::Touches,
    },
    prelude::*,
    window::PrimaryWindow,
};
use bevy_egui::input::EguiWantsInput;

use crate::{Quality, View};

/// Registers the camera and its controls.
pub(crate) fn plugin(app: &mut App) {
    app.add_systems(Startup, spawn)
        .add_systems(Update, (switch, (orbit, walk), place).chain());
}

/// The camera's state in both modes. The transform is written from this every
/// frame and never read back.
#[derive(Component, Clone, Debug)]
pub(crate) struct Rig {
    /// What the orbit turns about.
    pub focus: Vec3,
    /// Heading, in radians about +Y.
    pub yaw: f32,
    /// Elevation above the horizon, in radians.
    pub pitch: f32,
    /// How far the orbit stands off the focus, in metres.
    pub distance: f32,
    /// The walking eye, in metres.
    pub eye: Vec3,
    /// How far the scene reaches, which scales pan, zoom and walking speed.
    pub extent: f32,
    /// The height of the ground the eye walks on.
    pub ground: f32,
    /// The scene's bounds, which street level stands just outside of.
    pub bounds: [Vec3; 2],
}

impl Default for Rig {
    fn default() -> Self {
        Self {
            focus: Vec3::ZERO,
            yaw: 0.8,
            pitch: 0.35,
            distance: 30.0,
            eye: Vec3::new(0.0, 1.7, 0.0),
            extent: 50.0,
            ground: 0.0,
            bounds: [Vec3::splat(-25.0), Vec3::splat(25.0)],
        }
    }
}

impl Rig {
    /// Where the camera looks from, in the current mode.
    fn direction(&self) -> Vec3 {
        Vec3::new(
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.cos(),
        )
    }

    /// Frame a scene: stand off `bounds` by `distance` of its diagonal, at
    /// `pitch` above the horizon, from the south-east.
    pub(crate) fn frame(&mut self, low: Vec3, high: Vec3, distance: f32, pitch: f32) {
        let diagonal = (high - low).length().max(1.0);
        self.focus = Vec3::new(
            f32::midpoint(low.x, high.x),
            low.y + (high.y - low.y) * 0.25,
            f32::midpoint(low.z, high.z),
        );
        self.yaw = 0.7;
        self.pitch = pitch;
        self.distance = diagonal * distance;
        self.extent = diagonal;
        self.ground = low.y;
        self.bounds = [low, high];
        self.eye = self.street_eye();
    }

    /// Head height on the ground just outside the scene's bounds, on the side
    /// the orbit looks from: the pavement at a city's edge, the garden in
    /// front of a house. The middle of the bounds is usually inside a
    /// building.
    fn street_eye(&self) -> Vec3 {
        let [low, high] = self.bounds;
        let out = self.direction().with_y(0.0).normalize_or(Vec3::Z);
        // Where a ray from the focus along `out` leaves the bounds, per axis.
        let exit = |at: f32, way: f32, low: f32, high: f32| {
            if way > 1e-4 {
                (high - at) / way
            } else if way < -1e-4 {
                (low - at) / way
            } else {
                f32::MAX
            }
        };
        let reach = exit(self.focus.x, out.x, low.x, high.x)
            .min(exit(self.focus.z, out.z, low.z, high.z))
            .min(self.extent);
        Vec3::new(self.focus.x, self.ground + 1.7, self.focus.z) + out * (reach + 2.0)
    }

    /// Stand at street level looking in, level with the horizon.
    pub(crate) fn street_level(&mut self) {
        self.eye = self.street_eye();
        self.pitch = 0.04;
    }

    /// The transform in orbit mode: looking at the focus from `stand` times
    /// the distance, and stepped aside by `shift` of it, so the focus sits
    /// right of centre in the part of the window the panel does not cover.
    fn orbit_transform(&self, stand: f32, shift: f32) -> Transform {
        let distance = self.distance * stand;
        let mut transform = Transform::from_translation(self.focus + self.direction() * distance)
            .looking_at(self.focus, Vec3::Y);
        let left = -transform.right();
        transform.translation += left * distance * shift;
        transform
    }

    /// The transform in walk mode: the orbit's heading turned about, so the
    /// view does not jump when the mode changes.
    fn walk_transform(&self) -> Transform {
        Transform::from_translation(self.eye).looking_to(-self.direction(), Vec3::Y)
    }
}

/// How far the orbit steps aside for the panel, as a share of its distance.
const PANEL_SHIFT: f32 = 0.14;

/// The narrowest window, in logical pixels, the panel sits beside the view in
/// rather than over it. A phone held upright is about four hundred.
pub(crate) const NARROW: f32 = 700.0;

/// The camera entity.
#[derive(Component)]
pub(crate) struct MainCamera;

fn spawn(mut commands: Commands, quality: Res<Quality>) {
    commands.spawn((
        MainCamera,
        Camera3d::default(),
        // Bloom and the night rig want HDR; by day it costs nothing visible.
        bevy::camera::Hdr,
        // At two or three device pixels to the CSS pixel a phone's edges are
        // fine already, and four samples of every one of them are what its
        // GPU can least afford.
        if *quality == Quality::Handheld {
            Msaa::Off
        } else {
            Msaa::Sample4
        },
        Rig::default(),
        Transform::default(),
    ));
}

/// Keep the walking eye where the orbit left it when walking starts, so the
/// switch is a change of controls and not a jump.
fn switch(view: Res<View>, mut rigs: Query<&mut Rig>, mut last: Local<bool>) {
    if view.walk == *last {
        return;
    }
    *last = view.walk;
    if view.walk {
        for mut rig in &mut rigs {
            let eye = rig.focus + rig.direction() * rig.distance;
            rig.eye = eye;
        }
    }
}

/// Pointer movement this frame, in logical pixels, and the zoom asked for,
/// from a mouse or a touch screen. `None` when the panel has the pointer.
struct Gesture {
    turn: Vec2,
    pan: Vec2,
    zoom: f32,
}

fn gesture(
    buttons: &ButtonInput<MouseButton>,
    motion: AccumulatedMouseMotion,
    scroll: &AccumulatedMouseScroll,
    touches: &Touches,
    egui: &EguiWantsInput,
) -> Option<Gesture> {
    if egui.wants_any_pointer_input() {
        return None;
    }
    let mut gesture = Gesture {
        turn: Vec2::ZERO,
        pan: Vec2::ZERO,
        zoom: 0.0,
    };
    if buttons.pressed(MouseButton::Left) {
        gesture.turn += motion.delta;
    }
    if buttons.pressed(MouseButton::Right) || buttons.pressed(MouseButton::Middle) {
        gesture.pan += motion.delta;
    }
    gesture.zoom += match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / 60.0,
    };
    let fingers: Vec<_> = touches.iter().collect();
    match fingers.as_slice() {
        [one] => gesture.turn += one.delta(),
        [one, two] => {
            gesture.pan += (one.delta() + two.delta()) * 0.5;
            let now = one.position().distance(two.position());
            let before = one.previous_position().distance(two.previous_position());
            gesture.zoom += (now - before) / 40.0;
        }
        _ => {}
    }
    Some(gesture)
}

/// How many radians one logical pixel of drag turns.
const TURN: f32 = 0.005;

#[expect(
    clippy::too_many_arguments,
    reason = "a Bevy system's parameters are its inputs"
)]
fn orbit(
    view: Res<View>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    touches: Res<Touches>,
    egui: Res<EguiWantsInput>,
    time: Res<Time>,
    mut rigs: Query<&mut Rig>,
) {
    if view.walk {
        return;
    }
    let gesture = gesture(&buttons, *motion, &scroll, &touches, &egui);
    for mut rig in &mut rigs {
        if let Some(gesture) = &gesture {
            rig.yaw -= gesture.turn.x * TURN;
            rig.pitch = (rig.pitch + gesture.turn.y * TURN).clamp(-0.2, 1.5);
            rig.distance =
                (rig.distance * 0.88_f32.powf(gesture.zoom)).clamp(0.5, rig.extent * 4.0);
            // Pan in the plane of the screen, scaled so the point under the
            // pointer stays under it at the focus's depth.
            let scale = rig.distance * 0.0015;
            let right = Vec3::new(rig.yaw.cos(), 0.0, -rig.yaw.sin());
            let up = Vec3::Y;
            let pan = (-right * gesture.pan.x + up * gesture.pan.y) * scale;
            rig.focus += pan;
        }
        if !egui.wants_keyboard_input() {
            let step = rig.distance * 0.8 * time.delta_secs();
            let forward = -Vec3::new(rig.yaw.sin(), 0.0, rig.yaw.cos());
            let right = Vec3::new(rig.yaw.cos(), 0.0, -rig.yaw.sin());
            let mut shift = Vec3::ZERO;
            for (key, direction) in [
                (KeyCode::KeyW, forward),
                (KeyCode::KeyS, -forward),
                (KeyCode::KeyD, right),
                (KeyCode::KeyA, -right),
            ] {
                if keys.pressed(key) {
                    shift += direction;
                }
            }
            rig.focus += shift.normalize_or_zero() * step;
        }
    }
}

/// Metres a second at a walk, and when running with shift.
const WALK: f32 = 6.0;
const RUN: f32 = 30.0;

#[expect(
    clippy::too_many_arguments,
    reason = "a Bevy system's parameters are its inputs"
)]
fn walk(
    view: Res<View>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    touches: Res<Touches>,
    egui: Res<EguiWantsInput>,
    time: Res<Time>,
    mut rigs: Query<&mut Rig>,
) {
    if !view.walk {
        return;
    }
    let gesture = gesture(&buttons, *motion, &scroll, &touches, &egui);
    for mut rig in &mut rigs {
        let forward = -rig.direction();
        let flat = forward.with_y(0.0).normalize_or_zero();
        let right = flat.cross(Vec3::Y);
        let mut step = Vec3::ZERO;
        if let Some(gesture) = &gesture {
            // Dragging the world: the view turns the way the pointer pulls.
            rig.yaw -= gesture.turn.x * TURN;
            rig.pitch = (rig.pitch + gesture.turn.y * TURN).clamp(-1.4, 1.4);
            // A scroll or a pinch is a step forward, a pan a step aside.
            step += forward * gesture.zoom * 1.5;
            step += (-right * gesture.pan.x + Vec3::Y * gesture.pan.y) * 0.02;
        }
        if !egui.wants_keyboard_input() {
            let speed = if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
                RUN
            } else {
                WALK
            } * time.delta_secs();
            for (key, direction) in [
                (KeyCode::KeyW, flat),
                (KeyCode::ArrowUp, flat),
                (KeyCode::KeyS, -flat),
                (KeyCode::ArrowDown, -flat),
                (KeyCode::KeyD, right),
                (KeyCode::KeyA, -right),
                (KeyCode::KeyE, Vec3::Y),
                (KeyCode::Space, Vec3::Y),
                (KeyCode::KeyQ, -Vec3::Y),
            ] {
                if keys.pressed(key) {
                    step += direction * speed;
                }
            }
            for (key, turn) in [(KeyCode::ArrowLeft, 1.0), (KeyCode::ArrowRight, -1.0)] {
                if keys.pressed(key) {
                    rig.yaw += turn * 1.5 * time.delta_secs();
                }
            }
        }
        rig.eye += step;
        // Never below the street.
        rig.eye.y = rig.eye.y.max(rig.ground + 0.3);
    }
}

/// Write the transform from the rig. Every frame, because it is one entity
/// and the mode can change without the rig doing so.
///
/// On a narrow window the panel lies over the view, folded, and the focus
/// stays in the middle. On a window taller than it is wide the orbit stands
/// further off: a scene is framed by the field of view, which is vertical, and
/// held upright a phone sees half as much across as up.
fn place(
    view: Res<View>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut rigs: Query<(&Rig, &mut Transform)>,
) {
    let (width, height) = windows
        .single()
        .map_or((1.0, 1.0), |window| (window.width(), window.height()));
    let shift = if width < NARROW { 0.0 } else { PANEL_SHIFT };
    let stand = (height / width.max(1.0)).max(1.0).sqrt();
    for (rig, mut transform) in &mut rigs {
        *transform = if view.walk {
            rig.walk_transform()
        } else {
            rig.orbit_transform(stand, shift)
        };
    }
}
