//! The kept-solid fast path for a hit: one kernel solid per merge group, so a
//! blast subtracts from what is already unioned instead of re-unioning it.
//!
//! Re-unioning a storey is tens of milliseconds; subtracting one blast from a
//! solid that was kept is a few. Keeping the solid means keeping a
//! [`manifold_csg::Manifold`], which the kernel-free `ashlar` crate must never
//! name, so the kept state lives here, beside the port and not in it.
//!
//! [`GroupSolids::build`] evaluates a building once, then replays a damage log
//! record by record through the same [`GroupSolids::apply`] a game calls,
//! keeping every group's unioned solid. It is also what
//! [`mesh_building_with_damage`](crate::mesh_building_with_damage) answers, so
//! a replay and a played hit are one code path. Support crosses groups, which
//! is why the rule lives here and not in the port's one-group `mesh_group`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use ashlar::{
    Building, Damage, DamageLog, Element, FaceSource, GroupBatch, GroupMembers, Instance,
    MergedGroup, MeshError, PlacedCut, PlacedGeometry,
};
use glam::DVec3;
use manifold_csg::Manifold;

use crate::ManifoldMesher;

/// How close a piece has to be to the ground, in metres, to count as reaching
/// it.
const GROUND_TOLERANCE: f64 = 1e-3;
/// How far the kernel searches for a gap between two pieces, in metres.
const TOUCH_SEARCH: f64 = 1e-2;
/// The largest gap two pieces can have and still count as touching, in metres.
const TOUCH_TOLERANCE: f64 = 1e-3;

/// One kept kernel solid per merge group, so that a hit subtracts from what is
/// already unioned.
///
/// It is [`Send`], so a game can run [`apply`](Self::apply) on a task.
pub struct GroupSolids {
    building: Building,
    mesher: ManifoldMesher,
    log: DamageLog,
    groups: Vec<KeptGroup>,
    /// The lowest base of any group's undamaged union. A piece reaches the
    /// building when it reaches this.
    ground: Option<f64>,
    /// The pieces that were already unsupported in the undamaged building, as
    /// `(volume, bounds)`. A collapsing hit leaves such a piece alone unless it
    /// changed it: a rail or a sign stood off its wall was hanging on nothing
    /// before the hit, and a blast elsewhere has no business with it.
    exempt: Vec<(f64, [[f64; 3]; 2])>,
    /// The next piece identity to hand out. A piece keeps its identity while
    /// its signature is unchanged, so a contact answer can outlive a hit that
    /// did not touch either of the two pieces.
    next_piece_id: u64,
    /// Whether each pair of pieces touches, keyed by identity with the smaller
    /// id first, so the support walk does not ask the kernel twice about two
    /// pieces that have not changed since it last asked.
    contacts: BTreeMap<(u64, u64), bool>,
    /// How many support `min_gap` calls have been made since the build, which
    /// is what [`contact_tests`](Self::contact_tests) reports.
    contact_tests: u64,
}

/// One merge group's kept state: its identity, its members as owned ids, and
/// the unioned solid with the kernel-ID table that names a face source.
struct KeptGroup {
    id: String,
    storey: Option<i32>,
    /// The placed elements unioned, in operand order, as (instance, element).
    members: Vec<(String, String)>,
    solid: Manifold,
    sources: BTreeMap<u32, FaceSource>,
    /// The connected pieces of `solid`. A collapse compares these across
    /// groups, and they are refreshed only when this group's solid changes.
    pieces: Vec<KeptPiece>,
}

/// One connected piece of a kept solid, with the signature the exemption list
/// is compared against.
#[derive(Clone)]
struct KeptPiece {
    /// Stable identity, so contact answers survive a hit that leaves this piece
    /// alone. Global to the [`GroupSolids`], not per group.
    id: u64,
    solid: Manifold,
    volume: f64,
    bounds: [[f64; 3]; 2],
}

/// A piece of one group, borrowed, while support is propagated across the
/// building. `slot` and `index` say where the piece lives so a fall can be
/// reported against its group.
struct PieceView<'a> {
    slot: usize,
    index: usize,
    /// The piece's identity, which keys the contact cache.
    id: u64,
    solid: &'a Manifold,
    volume: f64,
    bounds: [[f64; 3]; 2],
}

/// What one hit did to one group.
#[derive(Clone, Debug)]
pub struct GroupHit {
    /// The group, re-meshed, with its triangle collider.
    pub group: MergedGroup,
    /// What was removed: one entry per connected piece.
    pub debris: Vec<Debris>,
}

/// What a piece of debris is.
///
/// Non-exhaustive: a new reason a piece leaves the building must not break a
/// consumer that matches on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DebrisKind {
    /// The intersection of the group with the blast: what the hit overlapped.
    Blasted,
    /// A piece a collapsing hit dropped, because it reached nothing.
    Fallen,
}

/// One connected piece of what a hit removed, with the materials still on it.
#[derive(Clone, Debug)]
pub struct Debris {
    /// Whether the blast cut this piece out or the collapse rule dropped it.
    pub kind: DebrisKind,
    /// Building-space faces by binding and side. Faces that were the group's
    /// surface keep the binding they wore; faces the blast made wear the damage
    /// slot.
    pub batches: Vec<GroupBatch>,
    /// Volume in cubic metres.
    pub volume: f64,
    /// Centre of the piece's bounds, building space: where to spawn it.
    pub centre: DVec3,
    /// The piece's axis-aligned bounds in building space. A consumer deciding
    /// what a fallen piece takes with it measures against these.
    pub bounds: [DVec3; 2],
}

// A game runs a hit off the frame thread, so `GroupSolids` has to cross one.
// `Manifold` is `Send` by its own adapter's guarantee; this locks the rest.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<GroupSolids>();
};

/// What one changed group's hit computes: its slot, new solid, ID table,
/// refreshed piece cache and reported hit, held aside until every group has
/// succeeded.
type ComputedHit = (
    usize,
    Manifold,
    BTreeMap<u32, FaceSource>,
    Vec<KeptPiece>,
    GroupHit,
);

impl GroupSolids {
    /// Mesh a building and replay `log` onto it, keeping every group's solid.
    ///
    /// The [`MeshedBuilding`](ashlar::MeshedBuilding) is exactly what
    /// [`mesh_building_with_damage`](crate::mesh_building_with_damage) answers
    /// for the same arguments: the log is replayed record by record through
    /// [`apply`](Self::apply), so a replay and a played hit are one code path
    /// and two implementations cannot drift.
    ///
    /// The pieces that are loose by design are found here, once, from the
    /// undamaged unions, before the log is replayed: a later collapse must not
    /// drop a sign the first record already removed, nor mistake a piece a
    /// blast changed for the one that was there before.
    ///
    /// # Errors
    ///
    /// The log is checked against the building first, and every kernel
    /// evaluation failure comes back with the geometry path that caused it.
    pub fn build(
        building: &Building,
        log: &DamageLog,
        mesher: ManifoldMesher,
    ) -> Result<(Self, ashlar::MeshedBuilding), MeshError> {
        log.check(building).map_err(|e| MeshError {
            path: e.path,
            reason: e.reason,
        })?;
        let weld_tolerance = ashlar::WELD_TOLERANCE;
        let merging = !building.merge_groups().is_empty();
        let (parts, part_colliders, part_portals) =
            crate::mesh_parts(building, &mesher, merging, weld_tolerance)?;
        let members = building.merge_groups();
        let undamaged = DamageLog::default();
        let mut groups = Vec::new();
        let mut kept = Vec::new();
        let mut next_piece_id = 0u64;
        for group in &members {
            let path = format!("groups[{}]", group.id);
            let solids: Vec<PlacedGeometry<'_>> = group
                .members
                .iter()
                .map(|(instance, element)| PlacedGeometry {
                    geometry: &element.geometry,
                    pose: instance.pose,
                })
                .collect();
            let (solid, sources) = mesher.union_operands(&solids).map_err(|e| MeshError {
                path: format!("{path}.{}", e.path),
                reason: e.reason,
            })?;
            let mesh = crate::extract_group(&solid, &sources, &mesher)?;
            let merged = crate::assemble_group(
                building,
                &undamaged,
                group,
                &mesh,
                false,
                weld_tolerance,
                &path,
            )?;
            groups.push(merged);
            kept.push(KeptGroup {
                id: group.id.to_owned(),
                storey: group.storey,
                members: group
                    .members
                    .iter()
                    .map(|(instance, element)| (instance.id.clone(), element.id.clone()))
                    .collect(),
                pieces: pieces_of(&solid, &[], &mut next_piece_id),
                solid,
                sources,
            });
        }
        // The ground is a fact about the undamaged unions, as each group's base
        // is: a later collapse must not move the floor line.
        let ground = kept
            .iter()
            .filter_map(|group| group.solid.bounding_box().map(|bounds| bounds.min()[1]))
            .reduce(f64::min);
        // The initial walk that finds the loose pieces counts too: the hook is
        // "since build", not "since the first hit".
        let mut contact_tests = 0u64;
        // Kept, not thrown away: a group no hit has touched keeps its pieces and
        // their ids, so what this walk learnt about them is still true when the
        // first hit asks.
        let mut contacts = BTreeMap::new();
        let exempt = unsupported_signatures(&kept, ground, &mut contacts, &mut contact_tests);

        let mut solids = Self {
            building: building.clone(),
            mesher,
            log: DamageLog::default(),
            groups: kept,
            ground,
            exempt,
            next_piece_id,
            contacts,
            contact_tests,
        };
        let mut answer = ashlar::MeshedBuilding {
            building: building.clone(),
            parts,
            part_colliders,
            part_portals,
            groups,
            damage: DamageLog::default(),
        };
        for record in &log.0 {
            for hit in solids.apply(record)? {
                answer.replace_group(hit.group);
            }
            answer.record(record.clone());
        }
        Ok((solids, answer))
    }

    /// Apply one hit. Returns a [`GroupHit`] for every group the solid actually
    /// changed, in group order; a miss is an empty vector.
    ///
    /// A group that the blast never touched is still reported when a collapsing
    /// hit removes a piece of it, because support crosses groups: cutting the
    /// storey under another drops that one too.
    ///
    /// The record joins the log either way, because the log is the order things
    /// happened in and a replay must see the same list. A hit is all or
    /// nothing: every changed group is computed into a local value first, and
    /// only once all of them have succeeded does the record join the log and
    /// each new solid, ID table and piece cache reach `self`. A failure
    /// part-way through therefore leaves `self` exactly as it was, log included,
    /// so a group that cannot be meshed cannot poison the ones after it.
    ///
    /// # Errors
    ///
    /// A record that does not check against the building is refused before it
    /// joins the log. A kernel failure while evaluating the blast or subtracting
    /// it comes back with the path that caused it.
    pub fn apply(&mut self, damage: &Damage) -> Result<Vec<GroupHit>, MeshError> {
        let index = u32::try_from(self.log.len()).map_err(|_| MeshError {
            path: "damage".into(),
            reason: "damage log is too long".into(),
        })?;
        let path = format!("damage[{index}]");
        damage.check(&self.building, &path).map_err(|e| MeshError {
            path: e.path,
            reason: e.reason,
        })?;
        // Batching of a face the blast exposes resolves its slot through the
        // log, so the computation reads a log that already has the record in
        // it. The record must not join `self.log` until every group has
        // succeeded, so the computation gets one trial log rather than a clone
        // per group.
        let mut trial_log = self.log.clone();
        trial_log.push(damage.clone());

        let placed = PlacedCut {
            geometry: &damage.solid,
            pose: damage.pose,
            collapse: damage.collapse,
        };
        let (cut, cut_sources) = self.mesher.evaluate_cut(&placed, index, &path)?;
        let damage_bounds = damage.bounds();
        let weld_tolerance = ashlar::WELD_TOLERANCE;

        // The trial values the all-or-nothing commit will take if every group
        // succeeds: fresh identities for the pieces the blast changes, and the
        // contact answers learned while looking at support.
        let mut next_piece_id = self.next_piece_id;
        let mut contacts = self.contacts.clone();
        let mut contact_tests = self.contact_tests;

        let mut work =
            self.subtract(&cut, &cut_sources, damage_bounds, &path, &mut next_piece_id)?;
        // Only a collapsing hit that changed something looks at support, and it
        // looks across every group: a storey is held up by the one under it,
        // which may not be the group the blast touched.
        let mut fallen: Vec<Vec<usize>> = vec![Vec::new(); self.groups.len()];
        if damage.collapse && work.iter().any(Option::is_some) {
            fallen = self.fallen_pieces(&work, &mut contacts, &mut contact_tests);
        }

        // Every changed group's new solid, ID table, piece cache and hit, held
        // aside until the last one has been computed.
        let mut computed: Vec<ComputedHit> = Vec::new();
        for (slot, group) in self.groups.iter().enumerate() {
            let touched = work[slot].is_some();
            if !touched && fallen[slot].is_empty() {
                continue;
            }
            computed.push(self.changed_group(
                slot,
                group,
                work[slot].take(),
                &fallen[slot],
                &trial_log,
                weld_tolerance,
                &path,
            )?);
        }

        self.log.push(damage.clone());
        self.next_piece_id = next_piece_id;
        self.contact_tests = contact_tests;
        let mut hits = Vec::with_capacity(computed.len());
        for (slot, solid, sources, pieces, hit) in computed {
            let group = &mut self.groups[slot];
            group.solid = solid;
            group.sources = sources;
            group.pieces = pieces;
            hits.push(hit);
        }
        // Drop the answers that name a piece the hit replaced, so the map does
        // not grow with the log. This is the last step of the commit: nothing
        // fallible follows, and a failed hit never reaches here.
        let mut alive = BTreeSet::new();
        for group in &self.groups {
            for piece in &group.pieces {
                alive.insert(piece.id);
            }
        }
        contacts.retain(|&(a, b), _| alive.contains(&a) && alive.contains(&b));
        self.contacts = contacts;
        Ok(hits)
    }

    /// Subtract the blast from every group it can reach, answering each
    /// touched group's new solid, ID table, removed piece and piece cache.
    ///
    /// The kept solid's own bounds are exact where the pre-mesh estimate was
    /// conservative, so a group the blast cannot touch is skipped without
    /// paying for the boolean.
    fn subtract(
        &self,
        cut: &Manifold,
        cut_sources: &BTreeMap<u32, FaceSource>,
        damage: Option<[DVec3; 2]>,
        path: &str,
        next_piece_id: &mut u64,
    ) -> Result<Vec<Option<Work>>, MeshError> {
        let mut work: Vec<Option<Work>> = Vec::new();
        work.resize_with(self.groups.len(), || None);
        for (slot, group) in self.groups.iter().enumerate() {
            if !overlaps_kept(&group.solid, damage) {
                continue;
            }
            let removed = group.solid.intersection(cut);
            if removed.is_empty() || removed.num_tri() == 0 {
                continue;
            }
            let solid = group.solid.difference(cut);
            self.mesher.check_solid(&solid, path)?;
            let mut sources = group.sources.clone();
            for (id, source) in cut_sources {
                sources.insert(*id, *source);
            }
            let pieces = pieces_of(&solid, &group.pieces, next_piece_id);
            work[slot] = Some(Work {
                solid,
                sources,
                removed,
                pieces,
            });
        }
        Ok(work)
    }

    /// The indices of the pieces that fall, as one list per group.
    ///
    /// A piece is supported when it reaches the ground or touches a supported
    /// piece, of this group or another, and support is propagated to a fixed
    /// point. A piece that was loose in the undamaged building and that no hit
    /// has changed is exempt: a rail or a sign stood off its wall was hanging
    /// on nothing before the hit. The signature test is the documented limit,
    /// because geometry alone cannot see that its wall has gone.
    fn fallen_pieces(
        &self,
        work: &[Option<Work>],
        contacts: &mut BTreeMap<(u64, u64), bool>,
        tests: &mut u64,
    ) -> Vec<Vec<usize>> {
        let mut views: Vec<PieceView<'_>> = Vec::new();
        for (slot, group) in self.groups.iter().enumerate() {
            let pieces = work[slot]
                .as_ref()
                .map_or(group.pieces.as_slice(), |worked| worked.pieces.as_slice());
            for (index, piece) in pieces.iter().enumerate() {
                views.push(PieceView {
                    slot,
                    index,
                    id: piece.id,
                    solid: &piece.solid,
                    volume: piece.volume,
                    bounds: piece.bounds,
                });
            }
        }
        let supported = supported_mask(&views, self.ground, contacts, tests);
        let mut fallen = vec![Vec::new(); self.groups.len()];
        for (position, view) in views.iter().enumerate() {
            if supported[position]
                || self
                    .exempt
                    .iter()
                    .any(|known| crate::same_piece(*known, (view.volume, view.bounds)))
            {
                continue;
            }
            fallen[view.slot].push(view.index);
        }
        fallen
    }

    /// One changed group as a hit: its final solid, ID table and piece cache,
    /// its re-meshed surface with a triangle collider, and the debris the blast
    /// cut out and the collapse dropped.
    #[expect(
        clippy::too_many_arguments,
        reason = "each argument is one fact the changed group is computed from"
    )]
    fn changed_group(
        &self,
        slot: usize,
        group: &KeptGroup,
        worked: Option<Work>,
        fallen: &[usize],
        trial_log: &DamageLog,
        weld_tolerance: f64,
        path: &str,
    ) -> Result<ComputedHit, MeshError> {
        let mut removed_pieces: Vec<(DebrisKind, Manifold)> = Vec::new();
        let solid;
        let sources;
        let pieces;
        if let Some(worked) = worked {
            for piece in worked.removed.decompose() {
                if !piece.is_empty() {
                    removed_pieces.push((DebrisKind::Blasted, piece));
                }
            }
            sources = worked.sources;
            if fallen.is_empty() {
                solid = worked.solid;
                pieces = worked.pieces;
            } else {
                let (recomposed, kept, fell) = keep_and_fall(worked.pieces, fallen)?;
                self.mesher.check_solid(&recomposed, path)?;
                solid = recomposed;
                pieces = kept;
                removed_pieces.extend(fell.into_iter().map(|piece| (DebrisKind::Fallen, piece)));
            }
        } else {
            let (recomposed, kept, fell) = keep_and_fall(group.pieces.clone(), fallen)?;
            self.mesher.check_solid(&recomposed, path)?;
            solid = recomposed;
            sources = group.sources.clone();
            pieces = kept;
            removed_pieces.extend(fell.into_iter().map(|piece| (DebrisKind::Fallen, piece)));
        }
        let mesh = crate::extract_group(&solid, &sources, &self.mesher)?;
        let group_ref = group_members(&self.building, group);
        // A group that changed gets its triangle collider, because its convex
        // proxies can no longer describe it.
        let assembled = crate::assemble_group(
            &self.building,
            trial_log,
            &group_ref,
            &mesh,
            true,
            weld_tolerance,
            path,
        )?;
        let mut debris = Vec::new();
        for (kind, piece) in removed_pieces {
            if let Some(piece) = debris_piece(
                &self.building,
                trial_log,
                &group_ref,
                &sources,
                &self.mesher,
                weld_tolerance,
                path,
                piece,
                kind,
            )? {
                debris.push(piece);
            }
        }
        Ok((
            slot,
            solid,
            sources,
            pieces,
            GroupHit {
                group: assembled,
                debris,
            },
        ))
    }

    /// The log so far.
    pub fn log(&self) -> &DamageLog {
        &self.log
    }

    /// How many support `min_gap` calls have been made since the build.
    ///
    /// A test and profiling hook, and nothing a game should use: the count is
    /// an implementation detail of the contact cache and moves with it.
    #[doc(hidden)]
    pub fn contact_tests(&self) -> u64 {
        self.contact_tests
    }
}

/// A touched group's new state before anything is committed.
struct Work {
    solid: Manifold,
    sources: BTreeMap<u32, FaceSource>,
    removed: Manifold,
    pieces: Vec<KeptPiece>,
}

/// Whether a kept solid's exact bounds overlap a hit's conservative bounds.
/// `None` on the hit's side means the solid cannot be located and is treated as
/// touching everything, exactly as the port treats it.
fn overlaps_kept(solid: &Manifold, damage: Option<[DVec3; 2]>) -> bool {
    match (solid.bounding_box(), damage) {
        (Some(bounds), Some(damage)) => {
            let min = bounds.min();
            let max = bounds.max();
            (0..3).all(|axis| min[axis] <= damage[1][axis] && damage[0][axis] <= max[axis])
        }
        _ => true,
    }
}

/// The connected pieces of a solid, as kept pieces with their signatures and
/// identities.
///
/// A piece whose `(volume, bounds)` signature matches one of `old` is the same
/// piece the kernel has already seen, so it keeps that piece's id and its
/// contact answers stay valid. Any other piece gets a fresh id from `next_id`.
/// `old` is empty when a group is first built, and is the group's previous
/// pieces whenever a hit recomputes it.
///
/// An empty solid has no pieces, and a piece with no bounds of its own is
/// dropped rather than kept with a fabricated box.
fn pieces_of(solid: &Manifold, old: &[KeptPiece], next_id: &mut u64) -> Vec<KeptPiece> {
    let mut pieces = Vec::new();
    for piece in solid.decompose() {
        if piece.is_empty() {
            continue;
        }
        let Some(bounds) = piece.bounding_box() else {
            continue;
        };
        let volume = piece.volume();
        let bounds = [bounds.min(), bounds.max()];
        let id = old
            .iter()
            .find(|known| crate::same_piece((known.volume, known.bounds), (volume, bounds)))
            .map_or_else(
                || {
                    let id = *next_id;
                    *next_id += 1;
                    id
                },
                |known| known.id,
            );
        pieces.push(KeptPiece {
            id,
            volume,
            bounds,
            solid: piece,
        });
    }
    pieces
}

/// The `(volume, bounds)` of every piece that reaches nothing in a set of
/// undamaged groups, which is the exemption list a collapse consults.
fn unsupported_signatures(
    groups: &[KeptGroup],
    ground: Option<f64>,
    contacts: &mut BTreeMap<(u64, u64), bool>,
    tests: &mut u64,
) -> Vec<(f64, [[f64; 3]; 2])> {
    let mut views: Vec<PieceView<'_>> = Vec::new();
    for (slot, group) in groups.iter().enumerate() {
        for (index, piece) in group.pieces.iter().enumerate() {
            views.push(PieceView {
                slot,
                index,
                id: piece.id,
                solid: &piece.solid,
                volume: piece.volume,
                bounds: piece.bounds,
            });
        }
    }
    let supported = supported_mask(&views, ground, contacts, tests);
    views
        .iter()
        .zip(&supported)
        .filter(|(_, supported)| !**supported)
        .map(|(view, _)| (view.volume, view.bounds))
        .collect()
}

/// Which pieces are supported: those on the ground, and those that touch a
/// supported piece, of this group or any other. A breadth-first walk from the
/// ground pieces, so the whole chain a column carries reaches everything it
/// holds up.
///
/// A pair whose answer is already in `contacts` is not asked again; a fresh
/// answer is stored there. `tests` counts the `min_gap` calls that were
/// actually needed.
fn supported_mask(
    views: &[PieceView<'_>],
    ground: Option<f64>,
    contacts: &mut BTreeMap<(u64, u64), bool>,
    tests: &mut u64,
) -> Vec<bool> {
    let Some(ground) = ground else {
        return vec![false; views.len()];
    };
    let mut supported = vec![false; views.len()];
    let mut queue = VecDeque::new();
    for (index, view) in views.iter().enumerate() {
        if (view.bounds[0][1] - ground).abs() <= GROUND_TOLERANCE {
            supported[index] = true;
            queue.push_back(index);
        }
    }
    while let Some(a) = queue.pop_front() {
        for b in 0..views.len() {
            if supported[b] || !grown_bounds_overlap(views[a].bounds, views[b].bounds) {
                continue;
            }
            // The bounds test only saves the kernel call; the kernel call
            // decides whether the two pieces actually touch, and the answer
            // stays good while both pieces keep their identity.
            let key = ordered_ids(views[a].id, views[b].id);
            let touching = if let Some(&known) = contacts.get(&key) {
                known
            } else {
                *tests += 1;
                let touching =
                    views[a].solid.min_gap(views[b].solid, TOUCH_SEARCH) <= TOUCH_TOLERANCE;
                contacts.insert(key, touching);
                touching
            };
            if touching {
                supported[b] = true;
                queue.push_back(b);
            }
        }
    }
    supported
}

/// The key of a contact: the two ids with the smaller one first, so the two
/// pieces of a pair always name the same entry.
fn ordered_ids(a: u64, b: u64) -> (u64, u64) {
    if a <= b { (a, b) } else { (b, a) }
}

/// Whether two bounds, each grown by the ground tolerance, overlap. This is a
/// cheap filter in front of [`Manifold::min_gap`], never the answer.
fn grown_bounds_overlap(a: [[f64; 3]; 2], b: [[f64; 3]; 2]) -> bool {
    (0..3).all(|axis| {
        a[0][axis] - GROUND_TOLERANCE <= b[1][axis] + GROUND_TOLERANCE
            && b[0][axis] - GROUND_TOLERANCE <= a[1][axis] + GROUND_TOLERANCE
    })
}

/// Split a group's pieces into the ones that stay and the ones that fall.
///
/// The kept solid is recomposed from the survivors without a boolean, so their
/// original IDs, and the face sources that name them, survive, and then
/// compacted because a decomposed piece carries the whole run table.
fn keep_and_fall(
    pieces: Vec<KeptPiece>,
    fallen: &[usize],
) -> Result<(Manifold, Vec<KeptPiece>, Vec<Manifold>), MeshError> {
    let mut kept = Vec::with_capacity(pieces.len());
    let mut fell = Vec::new();
    for (index, piece) in pieces.into_iter().enumerate() {
        if fallen.contains(&index) {
            fell.push(piece.solid);
        } else {
            kept.push(piece);
        }
    }
    let solid = compose_pieces(&kept)?;
    Ok((solid, kept, fell))
}

/// Recompose kept pieces without a boolean and compact the run table, so the
/// pieces the solid is made of are the pieces that were kept.
fn compose_pieces(pieces: &[KeptPiece]) -> Result<Manifold, MeshError> {
    let solid = match pieces {
        [] => Manifold::empty(),
        [piece] => piece.solid.clone(),
        _ => Manifold::compose(
            &pieces
                .iter()
                .map(|piece| piece.solid.clone())
                .collect::<Vec<_>>(),
        ),
    };
    if solid.is_empty() {
        Ok(solid)
    } else {
        crate::compact_runs(&solid)
    }
}

/// Turn one connected piece into debris, resolving its faces through the same
/// sources as the group. An empty piece is no debris.
#[expect(
    clippy::too_many_arguments,
    reason = "the piece needs the group's whole batching context"
)]
fn debris_piece(
    building: &Building,
    log: &DamageLog,
    group: &GroupMembers<'_>,
    sources: &BTreeMap<u32, FaceSource>,
    mesher: &ManifoldMesher,
    weld_tolerance: f64,
    path: &str,
    piece: Manifold,
    kind: DebrisKind,
) -> Result<Option<Debris>, MeshError> {
    if piece.num_tri() == 0 {
        return Ok(None);
    }
    let Some(bounds) = piece.bounding_box() else {
        return Ok(None);
    };
    let min = DVec3::from_array(bounds.min());
    let max = DVec3::from_array(bounds.max());
    let piece_mesh = crate::extract_group(&piece, sources, mesher)?;
    let batches = crate::group_batches(building, log, group, &piece_mesh, weld_tolerance, path)?;
    Ok(Some(Debris {
        kind,
        batches,
        volume: piece.volume(),
        centre: (min + max) * 0.5,
        bounds: [min, max],
    }))
}

/// The placed elements of a kept group as the borrows the batching code wants,
/// looked up in the owned building by the ids stored with the group.
fn group_members<'a>(building: &'a Building, group: &'a KeptGroup) -> GroupMembers<'a> {
    let members: Vec<(&Instance, &Element)> = group
        .members
        .iter()
        .map(|(instance_id, element_id)| {
            let instance = building.instance(instance_id).expect("kept instance");
            let element = building
                .part(&instance.part)
                .and_then(|part| {
                    part.elements
                        .iter()
                        .find(|element| &element.id == element_id)
                })
                .expect("kept element");
            (instance, element)
        })
        .collect();
    GroupMembers {
        id: &group.id,
        storey: group.storey,
        members,
    }
}
