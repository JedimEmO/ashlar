//! Click to blast: shift-clicking a merged scene subtracts a ball there.
//!
//! The hit is computed the way the design says a hit should be: it runs on
//! [`AsyncComputeTaskPool`], one at a time and in the order the clicks happened
//! in, because the damage log is a list and a list has an order. The frame that
//! receives the answer swaps the group's meshes, drops its debris and records
//! the hit, so the rest of the preview keeps telling the truth about the scene.
//!
//! Debris is decoration here. Fracture and physics are out of scope: a piece is
//! thrown along a straight line, falls under gravity and is removed after a few
//! seconds, and nothing collides with anything.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use ashlar::{
    Building, Damage, DamageLog, GroupBatch, MeshError, MeshedBuilding, Piece, PieceOrigin, Pose,
    TriangleMesh,
};
use ashlar_bevy as materials;
use ashlar_manifold::{DebrisKind, GroupHit, GroupSolids, ManifoldMesher};
// The preview does not depend on `tracing` directly; Bevy's `log` facade is the
// same crate, re-exported, and every other log line in the workspace goes
// through it.
use bevy::log as tracing;
use bevy::math::DVec3;
use bevy::{
    ecs::error::Result as BevyResult,
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future},
};

use crate::Options;
use crate::panel::PointerGrab;
use crate::viewer::{
    Baking, Center, Compiling, Palette, PieceElement, PieceGroup, PieceMaterial, Preview,
    PreviewObject, SurfaceKey, palette_entry, palette_material, spawn_piece,
};

/// How hard gravity pulls debris down, in metres per second squared.
const GRAVITY: f32 = 9.81;
/// How fast a piece of debris is thrown away from the blast, in metres per
/// second, before the upward kick is added.
const DEBRIS_SPEED: f32 = 2.0;
/// How much upward kick every piece gets, in metres per second.
const DEBRIS_LIFT: f32 = 1.5;
/// How long a piece of debris lives, in seconds.
const DEBRIS_LIFETIME: f32 = 4.0;
/// How far below where it started a piece is removed, in metres.
const DEBRIS_FALL: f32 = 30.0;
/// The smallest blast radius the bracket keys allow, in metres.
const MIN_BLAST_RADIUS: f64 = 0.3;
/// The largest blast radius the bracket keys allow, in metres.
const MAX_BLAST_RADIUS: f64 = 12.0;
/// How much one bracket key press scales the radius.
const BLAST_RADIUS_STEP: f64 = 1.25;
/// How much shift-control multiplies the radius of one click.
const BLAST_RADIUS_BOOST: f64 = 3.0;

/// The queue and the running hit for click-to-blast.
///
/// Built once, at startup, for a merged scene whose palette binds the blast
/// slot. The resource's absence is what "blasting is inert here" means: every
/// system and observer below takes it as an [`Option`] and does nothing without
/// one.
#[derive(Resource)]
pub(crate) struct BlastState {
    /// One kept kernel solid per merge group, unioned once at startup and
    /// shared with the task that applies a hit.
    ///
    /// An `Arc<Mutex<..>>` rather than owned: the hit runs on the compute pool
    /// and `GroupSolids::apply` needs `&mut`. One task at a time keeps the lock
    /// uncontended and the log in order.
    pub(crate) solids: Arc<Mutex<GroupSolids>>,
    /// Hits waiting their turn, in the order they were clicked.
    pub(crate) queue: VecDeque<QueuedBlast>,
    /// The hit in flight, if any, with the blast that queued it.
    ///
    /// The [`QueuedBlast`] waits beside the task because a finished hit has to
    /// remove the standalone pieces the ball engulfed, and neither the log
    /// record nor the kernel answer carries the radius that decision needs.
    pub(crate) running: Option<(Task<Result<Vec<GroupHit>, MeshError>>, QueuedBlast)>,
    /// The radius the next click blasts at, in metres. `[` and `]` change it,
    /// clamped to [`MIN_BLAST_RADIUS`] and [`MAX_BLAST_RADIUS`].
    pub(crate) radius: f64,
}

/// One queued blast: the hit to subtract and the ball that decided it.
///
/// A [`Damage`] is the record that reaches the log and the wire, and it holds
/// the solid but not the radius it was clicked with. The preview's own
/// standalone rule measures against that radius, so the two travel together
/// here; the centre is carried beside it because a shift-control boost changes
/// the radius and not the solid alone.
pub(crate) struct QueuedBlast {
    /// The hit to subtract.
    damage: Damage,
    /// The ball's centre in building space.
    centre: DVec3,
    /// The ball's radius in metres, the boosted one when shift-control was held.
    radius: f64,
}

/// A piece of debris thrown by a blast, waiting for [`fall`] to move it.
#[derive(Component)]
pub(crate) struct DebrisPiece {
    /// Renderer-space velocity, in metres per second. Building space and
    /// renderer space differ by a constant translation, so a velocity is the
    /// same in both and needs no conversion.
    velocity: Vec3,
    /// How long the piece has been falling, in seconds.
    age: f32,
    /// Where the piece started, in renderer space: the height the "fell thirty
    /// metres" removal is measured against.
    start_y: f32,
}

/// Where the pointer was when a blast-click's button went down.
///
/// The same guard [`panel`](crate::panel) puts on its own click, kept here
/// because the panel's resource is private to its module: the left button that
/// carries a shift-click is also the button that orbits, and a press and a
/// release over one entity is not a click when the pointer travelled.
#[derive(Resource, Default)]
pub(crate) struct BlastPress(Option<Vec2>);

/// Build the kept solids for a merged scene, or `None` when blasting is off.
///
/// Blasting is off for an unmerged scene, and for a merged one whose palette
/// does not bind [`Options::blast_slot`]; the second is said once, here, and
/// then the resource simply does not exist.
pub(crate) fn prepare(
    building: &Building,
    options: &Options,
) -> anyhow::Result<Option<BlastState>> {
    if !building.recipe().merged {
        return Ok(None);
    }
    if !building
        .recipe()
        .materials
        .contains_key(&options.blast_slot)
    {
        tracing::warn!(
            "blast slot {:?} is not bound in the building's palette; shift-click blasting is off",
            options.blast_slot
        );
        return Ok(None);
    }
    let (solids, _meshed) =
        GroupSolids::build(building, &DamageLog::default(), ManifoldMesher::default())?;
    let mut queue = VecDeque::new();
    for centre in &options.blast {
        queue.push_back(blast_damage(centre.0, options.blast_radius, options));
    }
    Ok(Some(BlastState {
        solids: Arc::new(Mutex::new(solids)),
        queue,
        running: None,
        radius: options.blast_radius,
    }))
}

/// One queued hit: a ball at `centre`, collapsing unless the command line asked
/// otherwise. Every blast the preview makes goes through here.
fn blast_damage(centre: [f64; 3], radius: f64, options: &Options) -> QueuedBlast {
    let damage = Damage::blast(centre, radius, options.blast_slot.clone());
    let damage = if options.no_collapse {
        damage
    } else {
        damage.collapsing()
    };
    QueuedBlast {
        damage,
        centre: DVec3::from_array(centre),
        radius,
    }
}

/// Create the material for the blast slot, which the scene may never draw.
///
/// The slot's binding is in the building palette whether or not any piece on
/// the scene ever wore it, and a blast's exposed faces do; a group batch that
/// wears it has to find a material in the palette, or it is skipped. This runs
/// after [`setup_materials`](crate::viewer::setup_materials) and reuses its
/// creation path, so a baked or compiled slot costs exactly what the same
/// binding costs as a scene surface.
#[expect(
    clippy::too_many_arguments,
    reason = "each argument is one resource the material creation needs"
)]
pub(crate) fn prepare_material(
    state: Option<Res<BlastState>>,
    options: Res<Options>,
    preview: Res<Preview>,
    definitions: Res<crate::Definitions>,
    server: Res<AssetServer>,
    mut baking: Baking,
    mut compiling: Compiling,
    mut palette: ResMut<Palette>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) -> BevyResult {
    if state.is_none() {
        return Ok(());
    }
    let Some(binding) = preview
        .0
        .building
        .recipe()
        .materials
        .get(&options.blast_slot)
    else {
        return Ok(());
    };
    palette_entry(
        binding,
        &definitions,
        &server,
        &mut baking,
        &mut compiling,
        &mut palette,
        &mut materials,
    )
}

/// The building-space centre of a blast at a renderer-space hit.
///
/// The renderer puts a model-space point at `model - center.origin`, so the way
/// back is one addition. Pure, so the sign is testable without a window.
pub(crate) fn blast_centre(hit: Vec3, center: &Center) -> [f64; 3] {
    (hit.as_dvec3() + center.origin).to_array()
}

/// Remember where a shift-click's button went down.
pub(crate) fn pressed(press: On<Pointer<Press>>, mut origin: ResMut<BlastPress>) {
    origin.0 = Some(press.pointer_location.position);
}

/// Queue a blast where a shift-click landed on the scene.
///
/// Only while a shift is held, so an ordinary click still points the parameter
/// panel, and only on a `PreviewObject`, so a click that lands on the sky and
/// not the model says nothing.
pub(crate) fn clicked(
    click: On<Pointer<Click>>,
    keys: Res<ButtonInput<KeyCode>>,
    options: Res<Options>,
    center: Res<Center>,
    state: Option<ResMut<BlastState>>,
    origin: Res<BlastPress>,
    objects: Query<(), With<PreviewObject>>,
) {
    let Some(mut state) = state else {
        return;
    };
    if !keys.pressed(KeyCode::ShiftLeft) && !keys.pressed(KeyCode::ShiftRight) {
        return;
    }
    let travelled = origin
        .0
        .is_some_and(|from| from.distance(click.pointer_location.position) > 4.0);
    if travelled || objects.get(click.entity).is_err() {
        return;
    }
    let Some(position) = click.hit.position else {
        return;
    };
    let centre = blast_centre(position, &center);
    // Shift-control reaches further: three times the radius the brackets set.
    let boosted = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let radius = if boosted {
        state.radius * BLAST_RADIUS_BOOST
    } else {
        state.radius
    };
    state
        .queue
        .push_back(blast_damage(centre, radius, &options));
}

/// `[` and `]` shrink and grow the radius the next click blasts at.
///
/// The bracket keys are handled where the preview's other keys are, under the
/// same interactive guard, so a capture or a panel drag never changes a blast.
pub(crate) fn resize(
    keys: Res<ButtonInput<KeyCode>>,
    state: Option<ResMut<BlastState>>,
    grab: PointerGrab,
) {
    let Some(mut state) = state else {
        return;
    };
    if !grab.interactive() {
        return;
    }
    if keys.just_pressed(KeyCode::BracketLeft) {
        state.radius = (state.radius / BLAST_RADIUS_STEP).clamp(MIN_BLAST_RADIUS, MAX_BLAST_RADIUS);
        tracing::info!("blast radius {:.2} m", state.radius);
    }
    if keys.just_pressed(KeyCode::BracketRight) {
        state.radius = (state.radius * BLAST_RADIUS_STEP).clamp(MIN_BLAST_RADIUS, MAX_BLAST_RADIUS);
        tracing::info!("blast radius {:.2} m", state.radius);
    }
}

/// Run one queued hit at a time, and swap the scene when it lands.
///
/// One task at a time, in queue order: the log is the order things happened in,
/// and two tasks racing for the lock would not keep it.
#[expect(
    clippy::too_many_arguments,
    reason = "each query is one component set the landed hit replaces or removes"
)]
pub(crate) fn drive_blasts(
    mut commands: Commands,
    state: Option<ResMut<BlastState>>,
    mut preview: ResMut<Preview>,
    mut meshes: ResMut<Assets<Mesh>>,
    center: Res<Center>,
    palette: Res<Palette>,
    groups: Query<(Entity, &PieceGroup)>,
    elements: Query<(Entity, &PieceElement, &Transform)>,
) {
    let Some(mut state) = state else {
        return;
    };
    if state.running.is_none() {
        let Some(queued) = state.queue.pop_front() else {
            return;
        };
        let solids = Arc::clone(&state.solids);
        let damage = queued.damage.clone();
        let task = AsyncComputeTaskPool::get().spawn(async move {
            let Ok(mut solids) = solids.lock() else {
                return Err(MeshError {
                    path: "damage".into(),
                    reason: "the blast solids lock was poisoned".into(),
                });
            };
            solids.apply(&damage)
        });
        state.running = Some((task, queued));
    }
    let Some((task, _)) = state.running.as_mut() else {
        return;
    };
    let Some(result) = block_on(future::poll_once(task)) else {
        return;
    };
    let Some((_, queued)) = state.running.take() else {
        return;
    };
    let hits = match result {
        Ok(hits) => hits,
        Err(error) => {
            tracing::warn!("a blast failed: {error}");
            return;
        }
    };
    // The record joined the kept log whether or not the hit changed a group,
    // because the log is the order things happened in, so the record to give
    // the preview is the log's last one.
    let record = state
        .solids
        .lock()
        .ok()
        .and_then(|solids| solids.log().0.last().cloned());
    let blast = queued.centre;
    // The bounds of every piece the collapse dropped, so a standalone piece the
    // blast never reached still falls with the storey it stood on.
    let mut fallen_bounds: Vec<[DVec3; 2]> = Vec::new();
    for hit in hits {
        for (entity, group) in &groups {
            if group.0 == hit.group.id {
                commands.entity(entity).despawn();
            }
        }
        for (batch, batch_mesh) in hit.group.batches.iter().enumerate() {
            if batch_mesh.mesh.positions.is_empty() {
                continue;
            }
            let Ok(mesh) = materials::mesh(&batch_mesh.mesh) else {
                continue;
            };
            let piece = Piece {
                origin: PieceOrigin::Group {
                    group: &hit.group.id,
                    batch,
                },
                storey: hit.group.storey,
                side: batch_mesh.side,
                pose: Pose::default(),
                mesh: &batch_mesh.mesh,
                binding: &batch_mesh.binding,
            };
            spawn_piece(&mut commands, &center, &palette, &piece, meshes.add(mesh));
        }
        for debris in &hit.debris {
            // A blasted piece keeps the outward kick; a piece the collapse
            // dropped has nothing pushing it and falls straight down.
            let velocity = if debris.kind == DebrisKind::Fallen {
                fallen_bounds.push(debris.bounds);
                Vec3::ZERO
            } else {
                let direction = (debris.centre - blast).normalize_or_zero().as_vec3();
                direction * DEBRIS_SPEED + Vec3::Y * DEBRIS_LIFT
            };
            for batch in &debris.batches {
                spawn_debris(
                    &mut commands,
                    &center,
                    &palette,
                    &mut meshes,
                    batch,
                    velocity,
                );
            }
        }
        preview.0.replace_group(hit.group);
    }
    // A piece engulfed by the ball is despawned, not dropped, so this runs
    // first and the despawn stands.
    attach_fallen(&mut commands, &preview.0, &fallen_bounds, &elements);
    despawn_engulfed(&mut commands, &preview.0, &queued, &elements);
    if let Some(record) = record {
        preview.0.record(record);
    }
}

/// Whether a blast's ball reaches an axis-aligned box.
///
/// The library only reports what a hit touches: [`standalone_touched`] is a box
/// against box test and `Damage` carries the solid rather than the ball, because
/// whether glass shatters is the game's decision and not the mesher's. The
/// preview is the game here, and this is the smallest decision a game could
/// make — the pane is simply gone; a real game would shatter it and throw the
/// shards. The test is a sphere against the box rather than the box's
/// axis-aligned bounds, so a pane at a corner of the ball's bounding box is
/// left where it is.
///
/// [`standalone_touched`]: MeshedBuilding::standalone_touched
fn sphere_reaches_box(centre: DVec3, radius: f64, min: DVec3, max: DVec3) -> bool {
    centre.distance(centre.clamp(min, max)) <= radius
}

/// The building-space bounds of a piece's mesh after its pose, or `None` when
/// the mesh has no points.
fn posed_bounds(pose: Pose, mesh: &TriangleMesh) -> Option<[DVec3; 2]> {
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    for position in &mesh.positions {
        let point = pose.transform_point(*position);
        min = min.min(point);
        max = max.max(point);
    }
    (min.is_finite() && max.is_finite()).then_some([min, max])
}

/// Remove the standalone pieces a blast engulfs, the preview's own decision.
///
/// [`MeshedBuilding::standalone_touched`] is the candidate list, and its box
/// test is too generous for a round ball, so a candidate survives only when the
/// nearest point of its posed box is within the ball. Every entity wearing a
/// kept `(instance, element)` goes: a pane is one thing however many batches it
/// is drawn in. These pieces join no debris and nothing else changes.
fn despawn_engulfed(
    commands: &mut Commands,
    preview: &MeshedBuilding,
    queued: &QueuedBlast,
    elements: &Query<(Entity, &PieceElement, &Transform)>,
) {
    let candidates: Vec<(&str, &str)> = preview.standalone_touched(&queued.damage);
    if candidates.is_empty() {
        return;
    }
    let mut kept: Vec<(&str, &str)> = Vec::new();
    for piece in preview.pieces() {
        let PieceOrigin::Element {
            instance, element, ..
        } = piece.origin
        else {
            continue;
        };
        if !candidates
            .iter()
            .any(|(candidate, other)| *candidate == instance && *other == element)
        {
            continue;
        }
        let Some(bounds) = posed_bounds(piece.pose, piece.mesh) else {
            continue;
        };
        if sphere_reaches_box(queued.centre, queued.radius, bounds[0], bounds[1])
            && !kept
                .iter()
                .any(|(candidate, other)| *candidate == instance && *other == element)
        {
            kept.push((instance, element));
        }
    }
    for (entity, piece_element, _) in elements {
        if kept.iter().any(|(instance, element)| {
            *instance == piece_element.instance.as_str()
                && *element == piece_element.element.as_str()
        }) {
            commands.entity(entity).despawn();
        }
    }
}

/// Drop the standalone pieces a fallen storey takes with it.
///
/// A blast subtracts a group, but glass is standalone, so a pane on a storey the
/// collision rule dropped is not cut and would hang in the air. The library
/// only reports what happened to the groups, and which piece a game wants to
/// take with a storey is its decision: the preview gives every standalone piece
/// whose posed centre lies inside a fallen piece a zero-velocity [`DebrisPiece`],
/// so it falls under the same gravity and despawns with the rest.
///
/// Whether a pane is taken is a box test with no kernel in it, so it is the
/// same shape of decision [`despawn_engulfed`] makes, but the bounds come from
/// the fallen piece rather than the ball.
fn attach_fallen(
    commands: &mut Commands,
    preview: &MeshedBuilding,
    fallen: &[[DVec3; 2]],
    elements: &Query<(Entity, &PieceElement, &Transform)>,
) {
    if fallen.is_empty() {
        return;
    }
    let mut matched: Vec<(&str, &str)> = Vec::new();
    for piece in preview.pieces() {
        let PieceOrigin::Element {
            instance, element, ..
        } = piece.origin
        else {
            continue;
        };
        let Some(bounds) = posed_bounds(piece.pose, piece.mesh) else {
            continue;
        };
        let centre = (bounds[0] + bounds[1]) * 0.5;
        let inside = fallen.iter().any(|piece| {
            (0..3).all(|axis| piece[0][axis] <= centre[axis] && centre[axis] <= piece[1][axis])
        });
        if inside
            && !matched
                .iter()
                .any(|(candidate, other)| *candidate == instance && *other == element)
        {
            matched.push((instance, element));
        }
    }
    for (entity, piece_element, transform) in elements {
        if matched.iter().any(|(instance, element)| {
            *instance == piece_element.instance.as_str()
                && *element == piece_element.element.as_str()
        }) {
            commands.entity(entity).insert(DebrisPiece {
                velocity: Vec3::ZERO,
                age: 0.0,
                start_y: transform.translation.y,
            });
        }
    }
}

/// Spawn one piece of a blast's debris.
fn spawn_debris(
    commands: &mut Commands,
    center: &Center,
    palette: &Palette,
    meshes: &mut Assets<Mesh>,
    batch: &GroupBatch,
    velocity: Vec3,
) {
    if batch.mesh.positions.is_empty() {
        return;
    }
    let Some(material) = palette_material(palette, &batch.binding) else {
        return;
    };
    let Ok(mesh) = materials::mesh(&batch.mesh) else {
        return;
    };
    // A group batch is meshed in building space, so its pose is the identity
    // and the whole of its renderer-space placement is the origin shift.
    let translation = -center.origin.as_vec3();
    let mut entity = commands.spawn((
        PreviewObject,
        SurfaceKey(batch.binding.material.clone()),
        Name::new(format!("debris/{}", batch.binding.material)),
        Mesh3d(meshes.add(mesh)),
        Transform::from_translation(translation),
        DebrisPiece {
            velocity,
            age: 0.0,
            start_y: translation.y,
        },
        Visibility::Inherited,
    ));
    match material {
        PieceMaterial::Standard(handle) => {
            entity.insert(MeshMaterial3d(handle));
        }
        PieceMaterial::Shader(handle) => {
            entity.insert(MeshMaterial3d(handle));
        }
    }
}

/// Integrate debris under gravity, and remove the pieces that have outlived
/// their welcome.
pub(crate) fn fall(
    mut commands: Commands,
    time: Res<Time>,
    mut pieces: Query<(Entity, &mut DebrisPiece, &mut Transform)>,
) {
    let dt = time.delta_secs();
    for (entity, mut piece, mut transform) in &mut pieces {
        piece.velocity.y -= GRAVITY * dt;
        transform.translation += piece.velocity * dt;
        piece.age += dt;
        if piece.age >= DEBRIS_LIFETIME || transform.translation.y < piece.start_y - DEBRIS_FALL {
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DVec3;

    #[test]
    fn a_hit_is_read_back_out_of_renderer_space() {
        let center = Center {
            model: DVec3::new(2.0, 1.0, -3.0),
            origin: DVec3::new(2.0, 0.0, -3.0),
        };
        let expected = [2.5, 1.25, -3.0];
        for (got, expected) in blast_centre(Vec3::new(0.5, 1.25, 0.0), &center)
            .into_iter()
            .zip(expected)
        {
            assert!((got - expected).abs() < 1e-9, "{got} against {expected}");
        }
    }

    #[test]
    fn a_blasts_reach_is_a_sphere_and_not_the_box_it_fills() {
        let centre = DVec3::ZERO;
        // A metre cube one metre along +x: its nearest corner is exactly 1 m
        // away, so a 1.0 ball touches it and a 0.99 one does not.
        let min = DVec3::new(1.0, 0.0, 0.0);
        let max = DVec3::new(2.0, 1.0, 1.0);
        assert!(sphere_reaches_box(centre, 1.0, min, max));
        assert!(!sphere_reaches_box(centre, 0.99, min, max));
        // A box in the corner at (1,1,1): its nearest point is sqrt(3) away, so
        // a 1.7 ball misses it even though its axis-aligned bounds overlap the
        // ball's.
        assert!(!sphere_reaches_box(
            centre,
            1.7,
            DVec3::splat(1.0),
            DVec3::splat(2.0)
        ));
    }
}
