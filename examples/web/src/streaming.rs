//! Streaming by distance: pieces whose level of detail cannot be drawn from
//! where the camera stands are taken out of the world's queries altogether.
//!
//! A level's `VisibilityRange` already decides what draws, but Bevy decides it
//! per piece, per view, every frame, and in a browser it does so on one thread
//! with no GPU culling. The metropolis spawns over a hundred thousand pieces
//! across its three levels, and most of them are out of range from any one
//! place: level zero ends at 60 m, so from above the city all seventy thousand
//! of its pieces are range-checked and culled only to draw nothing.
//!
//! So the pieces are grouped by band and by cell of the ground plan, and a
//! group whose band cannot reach the camera from anywhere in its cell is
//! [`Disabled`]: no system sees it until the camera comes back into range. The
//! decision is conservative — a group is enabled whenever any of its pieces
//! could be in range — so the per-piece range still says exactly what draws,
//! and nothing on screen changes. What changes is the work: the pieces in play
//! from above the metropolis are its coarse levels, a fifth of the whole.
use std::collections::{HashMap, HashSet};

use ashlar_bevy::prelude::AshlarPiece;
use bevy::{ecs::entity_disabling::Disabled, prelude::*};

use crate::{
    LodView, Mode, View,
    camera::MainCamera,
    scenes::{KeptRange, Status},
};

/// Registers the streaming.
pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<Cells>()
        .add_systems(Update, (collect, stream).chain());
}

/// The side of a cell of the ground plan, in metres. Small enough that a cell
/// on the far side of a band's boundary is not held in play by its near
/// corner, large enough that the metropolis is a few hundred groups.
const CELL: f32 = 40.0;

/// How far an interior reaches, in metres, while the buildings are closed: a
/// room is seen through a window or a door from nearer than this, and from
/// further its lit windows are what shows. Level zero's band runs to 60 m, and
/// in the metropolis two pieces in five of it are furniture behind glass.
const INTERIOR_REACH: f32 = 40.0;

/// How far past its band a group is brought into play, in metres, so that it
/// is enabled and settled a moment before its first piece comes into range.
const EARLY: f32 = 8.0;

/// How far past its band a group has to be before it leaves play again, in
/// metres: more than [`EARLY`], so a camera resting on a boundary does not
/// switch a group every frame.
const LATE: f32 = 16.0;

/// The pieces of one band in one cell.
struct Group {
    /// The bounds of the pieces' origins, which is what a range is measured
    /// to.
    low: Vec3,
    high: Vec3,
    /// The band: drawn at all from `near` to `far` metres.
    near: f32,
    far: f32,
    /// The finest level any of its pieces starts at, for the forced-level
    /// view.
    level: usize,
    /// Whether its pieces are seen from inside.
    interior: bool,
    members: Vec<Entity>,
    /// Whether it is in play.
    enabled: bool,
}

/// Every group of the scene on show.
#[derive(Resource, Default)]
struct Cells {
    /// The scene the groups belong to, as [`Status::scene`] names it.
    scene: Option<usize>,
    groups: Vec<Group>,
    /// Groups by cell, band bits and side.
    index: HashMap<(i32, i32, u32, u32, bool), usize>,
    /// Pieces already grouped.
    seen: HashSet<Entity>,
}

/// Group the pieces that have received their band since last frame. A piece
/// with no band draws at every distance and is never streamed.
fn collect(
    status: Res<Status>,
    mut cells: ResMut<Cells>,
    pieces: Query<(Entity, &Transform, &KeptRange, &AshlarPiece), Added<KeptRange>>,
) {
    if cells.scene != status.scene {
        // The old scene's pieces went with its root.
        *cells = Cells {
            scene: status.scene,
            ..default()
        };
    }
    for (entity, transform, range, piece) in &pieces {
        if !cells.seen.insert(entity) {
            continue;
        }
        let at = transform.translation;
        let (near, far) = (range.0.start_margin.start, range.0.end_margin.end);
        let interior = piece.side == ashlar::Side::Interior;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a cell index of a scene a few kilometres across"
        )]
        let key = (
            (at.x / CELL).floor() as i32,
            (at.z / CELL).floor() as i32,
            near.to_bits(),
            far.to_bits(),
            interior,
        );
        let cells = &mut *cells;
        let index = *cells.index.entry(key).or_insert_with(|| {
            cells.groups.push(Group {
                low: at,
                high: at,
                near,
                far,
                level: piece.level,
                interior,
                members: Vec::new(),
                enabled: true,
            });
            cells.groups.len() - 1
        });
        let group = &mut cells.groups[index];
        group.low = group.low.min(at);
        group.high = group.high.max(at);
        group.level = group.level.min(piece.level);
        group.members.push(entity);
    }
}

/// Take groups out of play and bring them back as the camera moves.
///
/// A group comes back with its `Transform` and `Visibility` inserted again:
/// while disabled no propagation reached it, so what it inherited is stale —
/// a storey cut or a mode switch made while it was away — and inserting marks
/// both changed, which has the next propagation recompute them.
fn stream(
    mut commands: Commands,
    mode: Res<Mode>,
    view: Res<View>,
    mut cells: ResMut<Cells>,
    camera: Query<&GlobalTransform, With<MainCamera>>,
    pieces: Query<(&Transform, &Visibility), Allow<Disabled>>,
) {
    // While the material stage is up the scene is hidden at its root, and a
    // piece brought back now would inherit nothing to hide it.
    if *mode != Mode::Scenes {
        return;
    }
    let Ok(eye) = camera.single().map(GlobalTransform::translation) else {
        return;
    };
    // A storey cut or a hidden exterior opens the buildings to the sky, and
    // then their rooms are seen from as far as their band reaches.
    let opened = view.hide_exterior || view.max_storey.is_some();
    for group in &mut cells.groups {
        let wanted = match view.lod {
            // One level at every distance: everything that might draw it.
            LodView::Force(level) => group.level <= level,
            LodView::Bands | LodView::Tint => {
                let slack = if group.enabled { LATE } else { EARLY };
                let nearest = eye.clamp(group.low, group.high).distance(eye);
                let farthest = (eye - group.low)
                    .abs()
                    .max((eye - group.high).abs())
                    .length();
                let far = if group.interior && !opened {
                    group.far.min(INTERIOR_REACH)
                } else {
                    group.far
                };
                farthest + slack >= group.near && nearest - slack < far
            }
        };
        if wanted == group.enabled {
            continue;
        }
        group.enabled = wanted;
        for &member in &group.members {
            if wanted {
                if let Ok((transform, visibility)) = pieces.get(member) {
                    commands
                        .entity(member)
                        .try_remove::<Disabled>()
                        .try_insert((*transform, *visibility));
                }
            } else {
                commands.entity(member).try_insert(Disabled);
            }
        }
    }
}
