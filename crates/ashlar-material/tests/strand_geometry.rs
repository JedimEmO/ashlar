//! Where a strand ends up, and what it is made of once it is there.
//!
//! Two claims, and a case for each corner of them. Placement plants every
//! strand of a set **exactly once** per repeat a surface covers — not twice on a
//! shared edge, not never on it, and not at all in a repeat the mesh does not
//! reach. Geometry turns one strand into a fixed, countable number of triangles
//! whose tip is where the closed form says it is.
//!
//! The quad cases are the ones worth reading first: a lattice with the jitter
//! turned off puts roots exactly on the diagonal a quad is split along, so the
//! shared-edge rule is exercised on purpose rather than by luck.
#![allow(
    clippy::unwrap_used,
    reason = "fixtures built in the test; a failure is a test failure"
)]
use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use ashlar_material::{
    MaterialGraph, MaterialGraphLibrary, ParamValue, PbrOutput, StrandLayer, StrandProfile,
    glam::Vec3,
    nodes::Noise,
    strands::{
        PlacedStrand, Strand, StrandRequest, StrandSet, StrandShape, SurfaceTriangles, mesh, place,
        scatter, tip_of,
    },
};

/// Two threads, named rather than defaulted: the machine running this is busy
/// and its cores are not the point of any case here.
fn threads() -> Option<NonZeroUsize> {
    NonZeroUsize::new(2)
}

/// Six decimals, which is the last place an `f32` expression this short holds.
fn close(left: f32, right: f32) -> bool {
    (left - right).abs() < 1e-5
}

fn close3(left: [f32; 3], right: [f32; 3]) -> bool {
    left.iter().zip(right).all(|(l, r)| close(*l, r))
}

/// A lattice of upright strands with the jitter off, so every root is at the
/// centre of its own cell and a test can say where it is.
///
/// Unjittered on purpose: a root at `(i + 0.5) / count` lands exactly on the
/// diagonal `u = v` whenever `i` is the same in both axes, which is the shared
/// edge a quad is split along and the only interesting case placement has.
fn lattice(count: u32) -> StrandSet {
    let graph = MaterialGraph::builder("test:lattice")
        .node("field", Noise::value().period(4))
        .output(PbrOutput::new().roughness("field"))
        .strands(
            "blades",
            StrandLayer::new()
                .count(count)
                .jitter(0.0)
                .metres(0.1, 0.01)
                .segments(3),
        )
        .into_graph();
    let params: BTreeMap<String, ParamValue> = BTreeMap::new();
    scatter(&StrandRequest {
        graph: &graph,
        library: &MaterialGraphLibrary::default(),
        params: &params,
        layer: "blades",
        field_resolution: 256,
        threads: threads(),
    })
    .unwrap()
}

/// A flat quad in the `xz` plane, `metres` across, mapped to `repeats` repeats
/// of the material on each axis and split along its `(0, 0)`–`(1, 1)` diagonal.
struct Quad {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl Quad {
    fn new(metres: f32, repeats: f32) -> Self {
        Self {
            positions: vec![
                [0.0, 0.0, 0.0],
                [metres, 0.0, 0.0],
                [metres, 0.0, metres],
                [0.0, 0.0, metres],
            ],
            normals: vec![[0.0, 1.0, 0.0]; 4],
            uvs: vec![
                [0.0, 0.0],
                [repeats, 0.0],
                [repeats, repeats],
                [0.0, repeats],
            ],
            // The diagonal is 0–2, which in UV is the line `u = v`.
            indices: vec![0, 1, 2, 0, 2, 3],
        }
    }

    fn surface(&self) -> SurfaceTriangles<'_> {
        SurfaceTriangles {
            positions: &self.positions,
            normals: &self.normals,
            uvs: &self.uvs,
            indices: &self.indices,
        }
    }
}

#[test]
fn a_quad_of_one_repeat_plants_every_strand_of_the_set_exactly_once() {
    let set = lattice(4);
    let quad = Quad::new(2.0, 1.0);
    let placed = place(&set, &quad.surface(), 1.0);

    assert_eq!(placed.len(), set.len(), "one repeat is one set of strands");
    // The roots that came back are the roots that went in, each once. Sorted
    // rather than compared in order, because placement walks triangles and the
    // set is in rank order.
    let mut planted: Vec<[u32; 2]> = placed
        .iter()
        .map(|strand| strand.strand.root.map(f32::to_bits))
        .collect();
    planted.sort_unstable();
    let mut expected: Vec<[u32; 2]> = set
        .strands()
        .iter()
        .map(|strand| strand.root.map(f32::to_bits))
        .collect();
    expected.sort_unstable();
    assert_eq!(planted, expected);

    // And the case was actually exercised: with the jitter off, a quarter of a
    // four-by-four lattice sits exactly on the diagonal the quad is split
    // along, so a rule that double-plants a shared edge would have answered
    // twenty strands here rather than sixteen.
    let on_the_diagonal = set
        .strands()
        .iter()
        .filter(|strand| strand.root[0].to_bits() == strand.root[1].to_bits())
        .count();
    assert_eq!(on_the_diagonal, 4, "the diagonal runs through four roots");
}

#[test]
fn the_count_scales_with_the_repeats_the_surface_covers() {
    let set = lattice(4);
    for repeats in [1_u32, 2, 3] {
        let quad = Quad::new(2.0, f32::from(u16::try_from(repeats).unwrap()));
        let placed = place(&set, &quad.surface(), 1.0);
        let repeats = (repeats * repeats) as usize;
        assert_eq!(
            placed.len(),
            set.len() * repeats,
            "a surface of {repeats} repeats carries {repeats} sets"
        );
    }
    // Every repeat the footprint covers is named, and named once: a strand and
    // the repeat it stands in are together unique.
    let quad = Quad::new(2.0, 2.0);
    let placed = place(&set, &quad.surface(), 1.0);
    let mut stamped: Vec<([u32; 2], [i32; 2])> = placed
        .iter()
        .map(|strand| (strand.strand.root.map(f32::to_bits), strand.repeat))
        .collect();
    stamped.sort_unstable();
    let planted = stamped.len();
    stamped.dedup();
    assert_eq!(stamped.len(), planted, "no strand was planted twice");
}

#[test]
fn a_level_of_detail_plants_the_prefix_and_nothing_else() {
    let set = lattice(8);
    let quad = Quad::new(2.0, 1.0);
    for keep in [0.0_f32, 0.25, 0.5, 1.0] {
        let placed = place(&set, &quad.surface(), keep);
        assert_eq!(placed.len(), set.prefix(keep).len(), "keep {keep}");
        assert!(
            placed.iter().all(|strand| strand.strand.rank < keep),
            "keep {keep} planted a strand the cut dropped"
        );
    }
}

#[test]
fn a_root_takes_the_position_normal_and_frame_of_the_surface_under_it() {
    let set = lattice(4);
    let quad = Quad::new(2.0, 1.0);
    let placed = place(&set, &quad.surface(), 1.0);
    for strand in &placed {
        // The quad is two metres of `xz` under one repeat, so a root at `u` is
        // at `x = 2u`, and the length in metres is untouched by that mapping.
        assert!(close(strand.position[0], strand.strand.root[0] * 2.0));
        assert!(close(strand.position[2], strand.strand.root[1] * 2.0));
        assert!(close(strand.position[1], 0.0));
        assert!(close3(strand.normal, [0.0, 1.0, 0.0]));
        assert!(close3(strand.tangent, [1.0, 0.0, 0.0]), "+u is +x");
        assert!(close3(strand.bitangent, [0.0, 0.0, 1.0]), "+v is +z");
        assert_eq!(strand.repeat, [0, 0]);
    }
}

#[test]
fn a_degenerate_uv_triangle_plants_nothing_and_takes_nothing_with_it() {
    let set = lattice(4);
    // The second triangle's UV collapses to a line, so it has no gradient to
    // build a frame from. The first is the quad's own half and is unaffected.
    let positions = vec![
        [0.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [2.0, 0.0, 2.0],
        [0.0, 0.0, 2.0],
    ];
    let surface = SurfaceTriangles {
        positions: &positions,
        normals: &[[0.0, 1.0, 0.0]; 4],
        uvs: &[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.5, 0.5]],
        indices: &[0, 1, 2, 0, 2, 3],
    };
    let placed = place(&set, &surface, 1.0);
    let lower = place(
        &set,
        &SurfaceTriangles {
            indices: &[0, 1, 2],
            ..surface
        },
        1.0,
    );
    assert_eq!(placed.len(), lower.len());
    assert!(!placed.is_empty(), "the sound half still planted");
    assert!(placed.len() < set.len(), "and only half of the repeat");
}

/// A cylinder standing on `y`, its UV wrapped once around and once up.
///
/// The seam is a duplicated column of vertices at `u = 0` and `u = 1`, which is
/// how every real UV-mapped cylinder is built: a single column would have to be
/// both, and the triangles either side of it would span the whole repeat
/// backwards.
fn cylinder(sides: u32, radius: f32, height: f32) -> Quad {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    let columns = sides + 1;
    for column in 0..columns {
        let u =
            f32::from(u16::try_from(column).unwrap()) / f32::from(u16::try_from(sides).unwrap());
        let angle = u * std::f32::consts::TAU;
        let (sine, cosine) = angle.sin_cos();
        for (row, v) in [(0_u32, 0.0_f32), (1, 1.0)] {
            positions.push([radius * cosine, v * height, radius * sine]);
            normals.push([cosine, 0.0, sine]);
            uvs.push([u, v]);
            let _ = row;
        }
    }
    for column in 0..sides {
        let near = column * 2;
        let far = near + 2;
        indices.extend_from_slice(&[near, far, near + 1, near + 1, far, far + 1]);
    }
    Quad {
        positions,
        normals,
        uvs,
        indices,
    }
}

#[test]
fn a_uv_wrapped_cylinder_plants_every_strand_once_and_stands_them_off_its_surface() {
    let set = lattice(8);
    let (radius, height) = (0.5_f32, 1.5_f32);
    let tube = cylinder(24, radius, height);
    let placed = place(&set, &tube.surface(), 1.0);

    assert_eq!(placed.len(), set.len(), "one wrap is one set of strands");
    // A root sits on the mesh rather than on the circle the mesh approximates,
    // so it is between the chord and the vertices: a twenty-four sided tube
    // pulls a mid-chord root in by half a per cent, and a placement that put a
    // root on the ideal cylinder would be reporting a surface that is not there.
    let chord = radius * (std::f32::consts::PI / 24.0).cos();
    for strand in &placed {
        let position = Vec3::from_array(strand.position);
        let reach = position.x.hypot(position.z);
        assert!(
            (chord - 1e-5..=radius + 1e-5).contains(&reach),
            "a root left the surface: {position:?}"
        );
        // The frame follows the mapping: `+u` runs around the tube and `+v`
        // runs up it, so a strand leaning along `+v` leans towards the sky
        // wherever on the cylinder it stands.
        let normal = Vec3::from_array(strand.normal);
        let tangent = Vec3::from_array(strand.tangent);
        let bitangent = Vec3::from_array(strand.bitangent);
        assert!(close(normal.length(), 1.0));
        assert!(close(normal.dot(tangent), 0.0), "the frame is orthogonal");
        assert!(close3(bitangent.to_array(), [0.0, 1.0, 0.0]), "+v is up");
        assert!(
            normal.dot(Vec3::new(position.x, 0.0, position.z)) > 0.0,
            "the normal points away from the axis"
        );
        assert!(close(position.y, strand.strand.root[1] * height));
    }
}

/// One strand standing on the ground at the origin, with whatever fields a case
/// wants to set.
fn planted(direction: [f32; 2], lean: f32, bend: f32, length: f32) -> PlacedStrand {
    PlacedStrand {
        strand: Strand {
            root: [0.5, 0.5],
            rank: 0.0,
            phase: 0.25,
            length,
            width: 0.01,
            direction,
            lean,
            bend,
            root_color: [0.0, 0.2, 0.0],
            tip_color: [0.4, 0.6, 0.1],
            roughness: 0.9,
            ..Default::default()
        },
        position: [0.0, 0.0, 0.0],
        normal: [0.0, 1.0, 0.0],
        tangent: [1.0, 0.0, 0.0],
        bitangent: [0.0, 0.0, 1.0],
        repeat: [0, 0],
    }
}

#[test]
fn the_tip_is_where_the_closed_form_puts_it() {
    let quarter = std::f32::consts::FRAC_PI_2;
    // Upright: no direction is no plane, so neither the lean nor the bend has
    // anywhere to tilt into and the strand is exactly its own length of normal.
    assert!(close3(
        tip_of(&planted([0.0, 0.0], 0.7, 0.4, 0.2)),
        [0.0, 0.2, 0.0]
    ));
    // A quarter turn of lean with no bend lies the strand flat along its own
    // direction, which here is `+x`.
    assert!(close3(
        tip_of(&planted([1.0, 0.0], 1.0, 0.0, 0.2)),
        [0.2, 0.0, 0.0]
    ));
    // And the general case: half a length along each of the two Bézier
    // tangents, the first at `lean` and the second at `lean + bend` quarter
    // turns off the normal.
    let (lean, bend, length) = (0.3_f32, 0.5_f32, 0.24_f32);
    let strand = planted([0.0, 1.0], lean, bend, length);
    let angle = |turns: f32| {
        let (sine, cosine) = (turns * quarter).sin_cos();
        Vec3::new(0.0, cosine, sine)
    };
    let expected = (angle(lean) + angle(lean + bend)) * (length * 0.5);
    assert!(
        close3(tip_of(&strand), expected.to_array()),
        "{:?} is not {expected:?}",
        tip_of(&strand)
    );
}

#[test]
fn the_curve_arrives_at_its_own_tip() {
    // The sampler and the closed form are the same curve: at full taper the
    // last ring has no width, so both of its vertices are the tip itself.
    let strand = planted([0.6, 0.8], 0.4, 0.6, 0.18);
    let shape = StrandShape {
        profile: StrandProfile::Blade,
        segments: 4,
        taper: 1.0,
        root_occlusion: 0.0,
        ..Default::default()
    };
    let built = mesh(&[strand], shape);
    let tip = tip_of(&strand);
    for vertex in &built.positions[built.positions.len() - 2..] {
        assert!(close3(*vertex, tip), "{vertex:?} is not the tip {tip:?}");
    }
    // And it leaves from the root, which is the other end of the closed form.
    // The root ring is the full width, so it is the *midpoint* of the two that
    // is the root: a blade tapers towards its tip and not away from its base.
    let root = (Vec3::from_array(built.positions[0]) + Vec3::from_array(built.positions[1])) * 0.5;
    assert!(close3(root.to_array(), strand.position));
}

#[test]
fn a_strand_is_the_same_count_of_triangles_whatever_it_looks_like() {
    for profile in [StrandProfile::Blade, StrandProfile::Fibre] {
        for segments in [1_u32, 2, 3, 8] {
            let shape = StrandShape {
                profile,
                segments,
                taper: 0.8,
                root_occlusion: 0.4,
                ..Default::default()
            };
            let ring = match profile {
                StrandProfile::Blade => 2,
                StrandProfile::Fibre => 3,
            };
            let faces = match profile {
                StrandProfile::Blade => 1,
                StrandProfile::Fibre => 3,
            };
            assert_eq!(
                shape.vertices_per_strand(),
                (segments as usize + 1) * ring,
                "{profile:?} at {segments}"
            );
            assert_eq!(
                shape.triangles_per_strand(),
                segments as usize * faces * 2,
                "{profile:?} at {segments}"
            );

            let strands = [
                planted([1.0, 0.0], 0.2, 0.3, 0.2),
                planted([0.0, 0.0], 0.0, 0.0, 0.1),
            ];
            let built = mesh(&strands, shape);
            assert_eq!(
                built.vertices(),
                strands.len() * shape.vertices_per_strand()
            );
            assert_eq!(
                built.triangles(),
                strands.len() * shape.triangles_per_strand()
            );
            // Every attribute is one per vertex, and every index names one.
            assert_eq!(built.normals.len(), built.vertices());
            assert_eq!(built.uvs.len(), built.vertices());
            assert_eq!(built.colors.len(), built.vertices());
            assert_eq!(built.wind.len(), built.vertices());
            assert!(
                built
                    .indices
                    .iter()
                    .all(|index| (*index as usize) < built.vertices()),
                "{profile:?} at {segments} indexed past its own vertices"
            );
            assert!(
                built
                    .normals
                    .iter()
                    .all(|normal| close(Vec3::from_array(*normal).length(), 1.0)),
                "{profile:?} at {segments} wrote a normal that is not a direction"
            );
        }
    }
}

#[test]
fn the_vertex_colour_carries_the_gradient_with_the_root_occlusion_in_it() {
    let strand = planted([1.0, 0.0], 0.2, 0.2, 0.2);
    let shape = StrandShape {
        profile: StrandProfile::Blade,
        segments: 2,
        taper: 1.0,
        root_occlusion: 0.75,
        ..Default::default()
    };
    let built = mesh(&[strand], shape);
    // The root is the root colour, a quarter lit, and the tip is the tip colour
    // at full. What sits between them is the gradient, which is the whole point
    // of the occlusion: the dark is at the bottom of the lawn.
    let root = built.colors[0];
    let tip = built.colors[built.colors.len() - 1];
    for channel in 0..3 {
        assert!(close(
            root[channel],
            strand.strand.root_color[channel] * 0.25
        ));
        assert!(close(tip[channel], strand.strand.tip_color[channel]));
    }
    assert!(close(root[3], 1.0), "the colour is opaque");
    // `v` runs from zero at the root to one at the tip on both the UV and the
    // wind attribute, and the phase is the strand's own on every vertex.
    assert!(close(built.uvs[0][1], 0.0));
    assert!(close(built.uvs[built.uvs.len() - 1][1], 1.0));
    assert!(built.wind.iter().all(|wind| close(wind[0], 0.25)));
    assert!(close(built.wind[0][1], 0.0));
    assert!(close(built.wind[built.wind.len() - 1][1], 1.0));
    // Stiffness is what a lean has left over, so a strand at `lean = 0.2` is
    // four-fifths stiff.
    assert!(built.wind.iter().all(|wind| close(wind[2], 0.8)));
}

#[test]
fn a_blade_is_as_wide_as_it_says_and_tapers_to_what_it_says() {
    let strand = planted([0.0, 1.0], 0.0, 0.0, 0.2);
    for taper in [0.0_f32, 0.5, 1.0] {
        let shape = StrandShape {
            profile: StrandProfile::Blade,
            segments: 2,
            taper,
            root_occlusion: 0.0,
            ..Default::default()
        };
        let built = mesh(&[strand], shape);
        let width = |ring: usize| {
            let left = Vec3::from_array(built.positions[ring * 2]);
            let right = Vec3::from_array(built.positions[ring * 2 + 1]);
            (right - left).length()
        };
        assert!(close(width(0), strand.strand.width), "taper {taper}");
        assert!(
            close(width(2), strand.strand.width * (1.0 - taper)),
            "taper {taper}"
        );
    }
}

#[test]
fn nothing_planted_builds_nothing() {
    let shape = StrandShape::default();
    let built = mesh(&[], shape);
    assert!(built.is_empty());
    assert_eq!(built.vertices(), 0);

    // And a surface with no triangles in it is not an error either: a chunk of
    // a mesh may genuinely hold no strands, and a caller should get an empty
    // list rather than a reason.
    let set = lattice(4);
    let empty = SurfaceTriangles {
        positions: &[],
        normals: &[],
        uvs: &[],
        indices: &[],
    };
    assert!(place(&set, &empty, 1.0).is_empty());
}

#[test]
fn two_faces_projected_onto_one_uv_square_each_grow_their_own_strands() {
    // A box projection maps every face of a cube onto the same UV, which is
    // what `Geometry::cuboid` and the benchmark's own specimens do. Two pieces
    // of surface over one root are two places for a blade to stand, so both get
    // one: a rule that handed the root to whichever triangle asked first would
    // leave five faces of a cube bald.
    let set = lattice(4);
    let positions = vec![
        [0.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [2.0, 0.0, 2.0],
        [0.0, 0.0, 2.0],
        // The same square, a metre up.
        [0.0, 1.0, 0.0],
        [2.0, 1.0, 0.0],
        [2.0, 1.0, 2.0],
        [0.0, 1.0, 2.0],
    ];
    let uvs = vec![
        [0.0, 0.0],
        [1.0, 0.0],
        [1.0, 1.0],
        [0.0, 1.0],
        [0.0, 0.0],
        [1.0, 0.0],
        [1.0, 1.0],
        [0.0, 1.0],
    ];
    let surface = SurfaceTriangles {
        positions: &positions,
        normals: &[[0.0, 1.0, 0.0]; 8],
        uvs: &uvs,
        indices: &[0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
    };
    let placed = place(&set, &surface, 1.0);
    assert_eq!(placed.len(), set.len() * 2, "both faces are planted");
    let upper = placed
        .iter()
        .filter(|strand| close(strand.position[1], 1.0))
        .count();
    assert_eq!(upper, set.len(), "and each face carries one whole set");
}

#[test]
fn a_shared_edge_wound_the_same_way_by_both_triangles_neither_doubles_nor_drops() {
    // The case a top-left fill rule gets wrong. The second triangle's UV winds
    // the shared diagonal the *same* direction as the first — a mirrored UV
    // seam, which real mappings have — so the rule that decides who owns the
    // edge cannot be a rule about winding. Sixteen strands, four of them
    // exactly on that diagonal, and sixteen is the only right answer.
    let set = lattice(4);
    let positions = vec![
        [0.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [2.0, 0.0, 2.0],
        [0.0, 0.0, 2.0],
    ];
    let uvs = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let surface = SurfaceTriangles {
        positions: &positions,
        normals: &[[0.0, 1.0, 0.0]; 4],
        uvs: &uvs,
        // `0, 3, 2` rather than `0, 2, 3`: the same triangle, wound the other
        // way, so the two halves of the quad disagree about the diagonal.
        indices: &[0, 1, 2, 0, 3, 2],
    };
    let placed = place(&set, &surface, 1.0);
    assert_eq!(placed.len(), set.len());
    let mut roots: Vec<[u32; 2]> = placed
        .iter()
        .map(|strand| strand.strand.root.map(f32::to_bits))
        .collect();
    roots.sort_unstable();
    let planted = roots.len();
    roots.dedup();
    assert_eq!(roots.len(), planted, "the diagonal was not planted twice");
}
