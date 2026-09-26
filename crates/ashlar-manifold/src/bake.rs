//! The content step for geometry: a building meshed at every level of detail.
//!
//! [`bake`] is what a game's content step runs once and writes with
//! [`BakedBuilding::write`]; the game then reads the file with no kernel
//! linked. Level zero is the building as a [`mesh_building`] call makes it,
//! with its collision proxies and portals; every later level is the building
//! simplified by that level's [`LodPolicy`] and meshed the same way. See
//! ADR 0006.
use std::collections::BTreeMap;

use ashlar::{
    BakedBuilding, BakedLevel, Building, Collision, ElementMesh, GeometryMesher, LodPolicy,
    MeshError, Part, Shape,
};

use crate::{batches, error, mesh_building, mesh_element, mesh_groups};

/// Mesh `building` once per level of `ladder`.
///
/// Level `n` is `building` simplified by `ladder[n]` and draws out to its
/// `until`. An empty ladder bakes the building as authored, to any distance.
/// A part whose simplification is the same as the level before's is not meshed
/// again: its meshes are that level's. Merge groups are meshed again only when
/// one of their members changed.
///
/// # Errors
///
/// The first element or group the mesher refuses, by path; or a
/// simplification that no longer validates, which would be a bug.
pub fn bake(
    building: &Building,
    ladder: &[LodPolicy],
    mesher: &dyn GeometryMesher,
) -> Result<BakedBuilding, MeshError> {
    let full = [LodPolicy::default()];
    let ladder = if ladder.is_empty() { &full[..] } else { ladder };
    let weld = ashlar::WELD_TOLERANCE;
    let authored = mesh_building(building, mesher)?;
    let mut levels: Vec<BakedLevel> = Vec::with_capacity(ladder.len());
    // The previous level's simplified parts, stripped of what does not change
    // a surface, which is how a later level knows a part is unchanged.
    let mut previous: BTreeMap<String, Part> = BTreeMap::new();
    for (index, policy) in ladder.iter().enumerate() {
        if index == 0 && policy.is_identity() {
            previous = used_parts(building);
            levels.push(BakedLevel {
                until: policy.until,
                parts: authored.parts.clone(),
                groups: authored.groups.clone(),
            });
            continue;
        }
        let Some(simplified) = building
            .simplified(policy)
            .map_err(|e| error(&format!("levels[{index}].{}", e.path), e.reason))?
        else {
            // Nothing is left at this distance: an empty level draws nothing,
            // which is what a small prop should do past it.
            levels.push(BakedLevel {
                until: policy.until,
                parts: BTreeMap::new(),
                groups: Vec::new(),
            });
            previous = BTreeMap::new();
            continue;
        };
        let current = used_parts(&simplified);
        let merging = !simplified.merge_groups().is_empty();
        let mut parts = BTreeMap::new();
        let mut changed = current.len() != previous.len();
        for (id, part) in &current {
            let reused = levels
                .last()
                .filter(|_| previous.get(id) == Some(part))
                .and_then(|level| level.parts.get(id));
            if let Some(meshes) = reused {
                parts.insert(id.clone(), meshes.clone());
                continue;
            }
            changed = true;
            let mut elements: Vec<ElementMesh> = Vec::new();
            for element in &part.elements {
                if merging && !element.standalone {
                    continue;
                }
                let mesh = mesh_element(id, element, mesher, weld)
                    .map_err(|e| error(&format!("levels[{index}].{}", e.path), e.reason))?;
                elements.extend(batches(element, mesh, weld));
            }
            parts.insert(id.clone(), elements);
        }
        let groups = match levels.last() {
            Some(level) if merging && !changed => level.groups.clone(),
            _ if merging => mesh_groups(&simplified, mesher, weld)
                .map_err(|e| error(&format!("levels[{index}].{}", e.path), e.reason))?,
            _ => Vec::new(),
        };
        levels.push(BakedLevel {
            until: policy.until,
            parts,
            groups,
        });
        previous = current;
    }
    Ok(BakedBuilding::new(
        building.clone(),
        levels,
        authored.part_colliders,
        authored.part_portals,
    ))
}

/// Every part an instance places, with collision proxies and portal markers
/// taken off: neither changes a surface, and a later level needs neither.
fn used_parts(building: &Building) -> BTreeMap<String, Part> {
    let recipe = building.recipe();
    recipe
        .parts
        .iter()
        .filter(|part| recipe.instances.iter().any(|i| i.part == part.id))
        .map(|part| {
            let mut part = part.clone();
            for element in &mut part.elements {
                element.collision = Collision::None;
                unmark(&mut element.geometry.shape);
            }
            (part.id.clone(), part)
        })
        .collect()
}

fn unmark(shape: &mut Shape) {
    match shape {
        Shape::Difference { solid, cutters } => {
            unmark(&mut solid.shape);
            for cutter in cutters {
                cutter.portal = None;
                unmark(&mut cutter.shape);
            }
        }
        Shape::Union { solids } => {
            for solid in solids {
                unmark(&mut solid.shape);
            }
        }
        Shape::Array { solid, .. } | Shape::Mirror { solid, .. } => unmark(&mut solid.shape),
        _ => {}
    }
}
