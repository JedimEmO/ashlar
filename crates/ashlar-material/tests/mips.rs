//! The mip chain: what each level is, and the two planes that are not a plain
//! average of the level above.
//!
//! A mip is the one part of a bake nobody looks at directly — it is what the
//! hardware picks when a wall is far away — so every claim here is arithmetic
//! against a filter written out a second time in this file, rather than against
//! a picture. Three things are checked: that a level is the box filter of the
//! one above it, everywhere and with no edge case at the seam; that the normals
//! are unit vectors after the renormalisation, in the `f32` plane and in the
//! bytes; and that the roughness only ever widens, which is the whole reason
//! the chain carries the mean normal's length around.
#![allow(
    clippy::unwrap_used,
    reason = "fixtures built in the test; a failure is a test failure"
)]
#![allow(
    clippy::float_cmp,
    reason = "a flat normal is exactly [0, 0, 1] and an encoded code is an \
              integer held in an f32; where a filter's arithmetic is compared \
              against the same filter written twice, the comparison carries an \
              explicit margin instead"
)]
#![allow(
    clippy::cast_precision_loss,
    reason = "a texel index is bounded by the resolution these tests bake at"
)]
use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use ashlar_material::{
    Channel, MaterialGraph, MaterialGraphLibrary, MathOp, PbrOutput,
    bake::{BakeRequest, Dither, Planes, TextureSet, bake_with_report, f32_to_f16_bits, rasterise},
    mips::{TOKSVIG_K, levels},
    nodes::{Colorize, Decompose, Levels, Math, Noise, Uv},
};

/// The resolution every bake here asks for: the smallest a bake allows, which
/// is nine levels and still an honest chain.
const N: u32 = 256;

/// Three threads rather than the default. A test binary runs its cases in
/// parallel already, and naming a count keeps the division of the rows the same
/// on every machine.
const THREADS: usize = 3;

/// How far two numbers computed by the same arithmetic in a different order may
/// sit apart. The filters here are means of four `f32`s; nothing in that can
/// drift further than a few ulps of a number below one.
const EPSILON: f32 = 1e-6;

fn request<'a>(
    graph: &'a MaterialGraph,
    library: &'a MaterialGraphLibrary,
    params: &'a BTreeMap<String, ashlar_material::ParamValue>,
    resolution: u32,
    mips: bool,
) -> BakeRequest<'a> {
    BakeRequest {
        graph,
        library,
        params,
        resolution,
        mips,
        threads: NonZeroUsize::new(THREADS),
    }
}

fn planes(graph: &MaterialGraph, resolution: u32) -> Planes {
    let (library, params) = (MaterialGraphLibrary::default(), BTreeMap::new());
    rasterise(&request(graph, &library, &params, resolution, false))
        .unwrap()
        .0
}

fn baked(graph: &MaterialGraph, resolution: u32) -> (TextureSet, usize) {
    let (library, params) = (MaterialGraphLibrary::default(), BTreeMap::new());
    let (set, report) =
        bake_with_report(&request(graph, &library, &params, resolution, true)).unwrap();
    (set, report.levels)
}

/// A surface with relief in it: a noise for the height, a second for the
/// roughness, a colour that varies, and a `normal_strength` large enough that
/// the derived normals really point in different directions.
///
/// That last part is what the chain is about. A gentle relief mips to normals
/// that were nearly parallel to start with and a Toksvig term of nothing; this
/// graph loses coherence fast enough that the roughness visibly widens.
fn relief() -> MaterialGraph {
    MaterialGraph::builder("test:relief")
        .node("bumps", Noise::value().period(16))
        .node("grain", Noise::value().period(64))
        .node(
            "albedo",
            Colorize::new("grain").gradient([(0.0, [0.1, 0.2, 0.3]), (1.0, [0.8, 0.7, 0.6])]),
        )
        .output(
            PbrOutput::new()
                .base_color("albedo")
                .roughness(Levels::new("grain").out_range(0.3, 0.8))
                .metallic("grain")
                .occlusion(Levels::new("bumps").out_range(0.6, 1.0))
                .height("bumps")
                .emissive(Levels::new("grain").out_range(0.0, 4.0))
                .normal_strength(0.08),
        )
        .into_graph()
}

/// A flat surface: no height, so every level-0 normal is exactly `[0, 0, 1]`
/// and the chain has no variance to widen anything with.
fn flat() -> MaterialGraph {
    MaterialGraph::builder("test:flat")
        .node("grain", Noise::value().period(32))
        .output(
            PbrOutput::new()
                .base_color("grain")
                .roughness(Levels::new("grain").out_range(0.2, 0.9)),
        )
        .into_graph()
}

/// A height of `0.5 + 0.5 sin(2 pi v)`: smooth, one repeat, and the same in
/// every column, so a seam in `u` is a seam the filter made.
fn sines() -> MaterialGraph {
    MaterialGraph::builder("test:sines")
        .node("u", Decompose::new(Uv::new(), Channel::R))
        .node("su", Math::unary(MathOp::Sin, "u"))
        .node(
            "height",
            Math::new(MathOp::Add, Math::new(MathOp::Mul, "su", 0.5), 0.5),
        )
        .output(
            PbrOutput::new()
                .base_color("height")
                .height("height")
                .normal_strength(0.05),
        )
        .into_graph()
}

/// The 2x2 box filter, written again and without a wrap: this is the reference
/// the chain is checked against, and the fact that it needs no edge case at an
/// even size is half of what makes a mip of a tiling plane tile.
fn boxed<T: Copy + Default + Lane>(plane: &[T], resolution: u32) -> Vec<T> {
    let width = resolution as usize;
    let half = width / 2;
    let mut out = Vec::with_capacity(half * half);
    for y in 0..half {
        for x in 0..half {
            let mut sum = T::default();
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                sum = sum.add(plane[(2 * y + dy) * width + 2 * x + dx]);
            }
            out.push(sum.scale(0.25));
        }
    }
    out
}

/// What `boxed` can average. The crate has its own copy of this and that is the
/// point: two implementations that agree are a filter, one is a claim.
trait Lane {
    fn add(self, other: Self) -> Self;
    fn scale(self, by: f32) -> Self;
}

impl Lane for f32 {
    fn add(self, other: Self) -> Self {
        self + other
    }
    fn scale(self, by: f32) -> Self {
        self * by
    }
}

impl Lane for [f32; 3] {
    fn add(self, other: Self) -> Self {
        [self[0] + other[0], self[1] + other[1], self[2] + other[2]]
    }
    fn scale(self, by: f32) -> Self {
        [self[0] * by, self[1] * by, self[2] * by]
    }
}

fn close(left: f32, right: f32) -> bool {
    (left - right).abs() <= EPSILON
}

fn length(vector: [f32; 3]) -> f32 {
    (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt()
}

/// One texel of an encoded normal map, back in `-1..=1`.
fn decode_normal(bytes: &[u8], index: usize) -> [f32; 3] {
    let texel = &bytes[index * 4..index * 4 + 4];
    [0, 1, 2].map(|axis| f32::from(texel[axis]) / 255.0 * 2.0 - 1.0)
}

#[test]
fn a_bake_that_asks_for_mips_writes_every_level_down_to_one_by_one() {
    let (set, reported) = baked(&relief(), N);
    assert_eq!(reported, 9, "256 halves to 1 in nine levels");
    assert_eq!(levels(N), 9);
    assert_eq!(levels(1024), 11);

    let height = set.height.as_ref().unwrap();
    let emissive = set.emissive.as_ref().unwrap();
    for map in [&set.base_color, &set.normal, &set.orm, height, emissive] {
        assert_eq!(map.mips.len(), 9, "every map carries the same chain");
        for (level, bytes) in map.mips.iter().enumerate() {
            // Level `n` is the resolution shifted down `n` times, and its
            // length is that squared in the format's own texel size. A level
            // that is nearly the right length is a file no loader will read.
            let resolution = (N >> level).max(1) as usize;
            assert_eq!(
                bytes.len(),
                resolution * resolution * map.format.bytes_per_texel(),
                "{:?} level {level}",
                map.format
            );
        }
    }
    // The last level is one texel: four bytes of colour, two of height, eight
    // of emissive.
    assert_eq!(set.base_color.mips[8].len(), 4);
    assert_eq!(height.mips[8].len(), 2);
    assert_eq!(emissive.mips[8].len(), 8);
    // And the resolution a set reports is still level 0's, which is what a
    // loader lays the chain out from.
    assert_eq!(set.resolution, N);

    // A bake that did not ask for a chain says so, and writes the one level it
    // wrote rather than a count of what it might have written.
    let (library, params) = (MaterialGraphLibrary::default(), BTreeMap::new());
    let (plain, report) =
        bake_with_report(&request(&relief(), &library, &params, N, false)).unwrap();
    assert_eq!(report.levels, 1);
    assert_eq!(plain.base_color.mips.len(), 1);
    assert_eq!(
        plain.base_color.mips[0], set.base_color.mips[0],
        "level 0 is the same bytes either way, which is what the study's golden hashes rest on"
    );
}

#[test]
fn every_level_is_the_box_filter_of_the_one_above_it() {
    let chain = planes(&relief(), N).mips();
    assert_eq!(chain.len(), 9);
    assert_eq!(
        chain[0],
        planes(&relief(), N),
        "level 0 is what it came from"
    );

    for (above, here) in chain.iter().zip(chain.iter().skip(1)) {
        let level = levels(N) - levels(here.resolution);
        assert_eq!(here.resolution, above.resolution / 2);
        assert_eq!(
            here.base_color.len(),
            (here.resolution * here.resolution) as usize
        );
        assert_eq!(here.normal_strength, above.normal_strength);

        // The four planes that mean their own average, against the filter
        // written out again above. The reference does not wrap, which is the
        // "no edge special case" half of the claim: at an even size the 2x2
        // groups partition the plane, so the wrapped filter and the plain one
        // are the same numbers, texel for texel — including the first column
        // and the last.
        for (name, got, want) in [
            (
                "metallic",
                &here.metallic,
                &boxed(&above.metallic, above.resolution),
            ),
            (
                "occlusion",
                &here.occlusion,
                &boxed(&above.occlusion, above.resolution),
            ),
            (
                "height",
                here.height.as_ref().unwrap(),
                &boxed(above.height.as_ref().unwrap(), above.resolution),
            ),
        ] {
            assert_eq!(got.len(), want.len(), "{name} at level {level}");
            for (index, (got, want)) in got.iter().zip(want).enumerate() {
                assert!(
                    close(*got, *want),
                    "{name} level {level} texel {index}: {got} is not the mean {want}"
                );
            }
        }
        for (name, got, want) in [
            (
                "base colour",
                &here.base_color,
                &boxed(&above.base_color, above.resolution),
            ),
            (
                "emissive",
                here.emissive.as_ref().unwrap(),
                &boxed(above.emissive.as_ref().unwrap(), above.resolution),
            ),
        ] {
            for (index, (got, want)) in got.iter().zip(want).enumerate() {
                for channel in 0..3 {
                    assert!(
                        close(got[channel], want[channel]),
                        "{name} level {level} texel {index}: {got:?} is not the mean {want:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn a_level_meets_itself_across_the_seam_as_closely_as_it_meets_itself_anywhere() {
    // The height is a sine of `u` with one repeat, so every column differs from
    // its neighbour by a known smooth amount and the pair that straddles the
    // seam is one of those neighbours. A filter that padded or clamped at the
    // edge would leave the first and last columns half a texel out of step,
    // which is a hairline down every wall of the building and nothing the
    // period inference would notice.
    let chain = planes(&sines(), N).mips();
    for (level, planes) in chain.iter().enumerate() {
        let width = planes.resolution as usize;
        if width < 8 {
            // Below this the "sine" is four texels and a neighbour difference
            // is the whole wave; there is no interior to compare against.
            continue;
        }
        let height = planes.height.as_ref().unwrap();
        let mut interior: f32 = 0.0;
        for y in 0..width {
            for x in 1..width {
                interior = interior.max((height[y * width + x] - height[y * width + x - 1]).abs());
            }
        }
        let mut seam: f32 = 0.0;
        for y in 0..width {
            seam = seam.max((height[y * width] - height[y * width + width - 1]).abs());
        }
        assert!(
            seam <= interior + EPSILON,
            "level {level}: the seam steps {seam} where the widest step inside the level is \
             {interior}"
        );
    }
}

#[test]
fn every_normal_is_a_unit_vector_at_every_level() {
    let graph = relief();
    let chain = planes(&graph, N).mips();
    for (level, planes) in chain.iter().enumerate() {
        for (index, normal) in planes.normal.iter().enumerate() {
            let length = length(*normal);
            assert!(
                (length - 1.0).abs() <= 1e-5,
                "level {level} texel {index}: {normal:?} is {length} long"
            );
        }
    }

    // And in the bytes, where the only thing between a unit vector and a code
    // is the quantiser. One 8-bit step is 1/255 of the `-1..=1` a byte covers,
    // so three of them can move a length by at most sqrt(3)/255.
    let (set, _) = baked(&graph, N);
    let tolerance = 3.0_f32.sqrt() / 255.0;
    for (level, bytes) in set.normal.mips.iter().enumerate() {
        for index in 0..bytes.len() / 4 {
            let normal = decode_normal(bytes, index);
            let length = length(normal);
            assert!(
                (length - 1.0).abs() <= tolerance,
                "level {level} texel {index}: {normal:?} decodes to {length}"
            );
            assert_eq!(bytes[index * 4 + 3], 255, "the alpha of a normal is opaque");
        }
    }
}

#[test]
fn a_level_normal_is_the_direction_of_the_mean_of_the_level_0_normals() {
    // Not of the mean of the level above's *renormalised* normals, which is a
    // different vector and, more to the point, a different length: renormalise
    // at every level and the spread is gone after the first one, taking the
    // roughness widening with it.
    let chain = planes(&relief(), N).mips();
    let mut mean = chain[0].normal.clone();
    let mut resolution = chain[0].resolution;
    for (level, planes) in chain.iter().enumerate().skip(1) {
        mean = boxed(&mean, resolution);
        resolution /= 2;
        for (index, (got, want)) in planes.normal.iter().zip(&mean).enumerate() {
            let length = length(*want);
            let want = [want[0] / length, want[1] / length, want[2] / length];
            for axis in 0..3 {
                assert!(
                    close(got[axis], want[axis]),
                    "level {level} texel {index}: {got:?} is not {want:?}"
                );
            }
        }
    }
}

#[test]
fn roughness_never_falls_below_the_average_it_was_widened_from() {
    // The invariant the widening actually has, and the one that matters: no
    // level is smoother than a plain box filter of the level-0 roughness would
    // have made it. Toksvig only adds.
    let chain = planes(&relief(), N).mips();
    let mut plain = chain[0].roughness.clone();
    let mut resolution = chain[0].resolution;
    let mut widened_somewhere = false;
    for (level, planes) in chain.iter().enumerate().skip(1) {
        plain = boxed(&plain, resolution);
        resolution /= 2;
        for (index, (got, want)) in planes.roughness.iter().zip(&plain).enumerate() {
            assert!(
                *got >= want - EPSILON,
                "level {level} texel {index}: {got} is below the plain average {want}"
            );
            widened_somewhere |= *got > want + 0.01;
        }
    }
    assert!(
        widened_somewhere,
        "a surface with this much relief should lose normal coherence and widen"
    );
}

#[test]
fn a_flat_surface_widens_nothing_and_stays_flat() {
    let chain = planes(&flat(), N).mips();
    let mut plain = chain[0].roughness.clone();
    let mut resolution = chain[0].resolution;
    for (level, planes) in chain.iter().enumerate().skip(1) {
        plain = boxed(&plain, resolution);
        resolution /= 2;
        for (index, normal) in planes.normal.iter().enumerate() {
            assert_eq!(
                *normal,
                [0.0, 0.0, 1.0],
                "level {level} texel {index}: a graph with no height has no slope to lose"
            );
        }
        for (index, (got, want)) in planes.roughness.iter().zip(&plain).enumerate() {
            assert!(
                close(*got, *want),
                "level {level} texel {index}: {got} is not the plain average {want}, and there \
                 was no variance to widen it with"
            );
        }
    }
}

#[test]
fn the_widening_is_the_formula_the_design_writes() {
    // Four texels whose normals point four ways, so the mean is short and the
    // term is large enough to read off by hand. Built rather than baked: what
    // is under test is the arithmetic, and a graph would only be a way of
    // arriving at these numbers.
    let tilt = 1.0_f32 / 2.0_f32.sqrt();
    let normal = vec![
        [tilt, 0.0, tilt],
        [-tilt, 0.0, tilt],
        [0.0, tilt, tilt],
        [0.0, -tilt, tilt],
    ];
    let mean = [0.0, 0.0, tilt];
    let level0 = Planes {
        resolution: 2,
        normal_strength: 0.1,
        base_color: vec![[0.5, 0.5, 0.5]; 4],
        roughness: vec![0.4; 4],
        metallic: vec![0.0; 4],
        occlusion: vec![1.0; 4],
        normal,
        height: None,
        emissive: None,
    };
    let chain = level0.mips();
    assert_eq!(chain.len(), 2, "a 2x2 plane has one level below it");
    let level1 = &chain[1];
    assert_eq!(level1.resolution, 1);

    // The mean of the four normals points straight up and is `tilt` long, so
    // the renormalised normal is flat and the term is `1 - tilt`.
    assert_eq!(level1.normal[0], [0.0, 0.0, 1.0]);
    let want = (0.4_f32 * 0.4 + (1.0 - length(mean)) * TOKSVIG_K)
        .sqrt()
        .min(1.0);
    assert!(
        close(level1.roughness[0], want),
        "{} is not sqrt(0.4^2 + (1 - {}) * {TOKSVIG_K}) = {want}",
        level1.roughness[0],
        length(mean)
    );
    // Which is a long way from the 0.4 it started at: four facets leaning a
    // forty-five degrees apart average to a vector a third short of unit, and a
    // third of the Toksvig term is most of a roughness.
    assert!(level1.roughness[0] > 0.8, "{}", level1.roughness[0]);
}

#[test]
fn a_level_is_never_smoother_than_the_texels_it_was_made_from() {
    // The per-texel reading of "roughness never decreases with level", on the
    // one input where it is exactly true: a constant roughness, so the plain
    // average is the constant and every difference between levels is the
    // Toksvig term, which only grows as the normals below a texel disagree with
    // each other more.
    let resolution = 16_u32;
    let texels = (resolution * resolution) as usize;
    let tilt = 0.6_f32;
    let flat = (1.0_f32 - tilt * tilt).sqrt();
    let normal: Vec<[f32; 3]> = (0..texels)
        .map(|index| {
            // Every texel leans the same amount in a direction of its own, from
            // a hash rather than a pattern: a pinwheel that repeats every two
            // texels averages to the same vector at every level below the
            // first, and then there is no variance left to lose and nothing to
            // test. The turn is what a level's mean gets shorter from.
            let hash = (index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
            let turn = (hash >> 40) as f32 / 16_777_216.0 * std::f32::consts::TAU;
            [tilt * turn.cos(), tilt * turn.sin(), flat]
        })
        .collect();
    let level0 = Planes {
        resolution,
        normal_strength: 0.1,
        base_color: vec![[0.5, 0.5, 0.5]; texels],
        roughness: vec![0.25; texels],
        metallic: vec![0.0; texels],
        occlusion: vec![1.0; texels],
        normal,
        height: None,
        emissive: None,
    };
    let chain = level0.mips();
    assert_eq!(chain.len(), 5);
    for (above, here) in chain.iter().zip(chain.iter().skip(1)) {
        let level = levels(resolution) - levels(here.resolution);
        let width = above.resolution as usize;
        for y in 0..here.resolution as usize {
            for x in 0..here.resolution as usize {
                let parents = [(0, 0), (1, 0), (0, 1), (1, 1)]
                    .map(|(dx, dy)| above.roughness[(2 * y + dy) * width + 2 * x + dx]);
                let smoothest = parents.iter().copied().fold(f32::INFINITY, f32::min);
                let got = here.roughness[y * here.resolution as usize + x];
                assert!(
                    got >= smoothest - EPSILON,
                    "level {level} texel ({x}, {y}): {got} is smoother than {smoothest}"
                );
            }
        }
    }
    // And over the chain as a whole it really did widen: the last level is one
    // texel averaging two hundred and fifty six normals pointing every way, and
    // a quarter-rough surface made of those is not a quarter-rough surface.
    let last = chain.last().unwrap().roughness[0];
    assert!(last > 0.25 + 0.2, "the 1x1 level is {last} rough");
}

#[test]
fn height_and_emissive_carry_the_plain_average_into_their_own_formats() {
    let graph = relief();
    let chain = planes(&graph, N).mips();
    let (set, _) = baked(&graph, N);

    // The 16-bit height of level 1 is the level-1 plane, rounded. Read the
    // first texel, which is the mean of the first 2x2 group of level 0.
    let height = chain[1].height.as_ref().unwrap();
    let bytes = &set.height.as_ref().unwrap().mips[1];
    let code = u16::from_le_bytes([bytes[0], bytes[1]]);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is clamped into 0..=65535 before the cast, so it is exactly a code"
    )]
    let want = (height[0].clamp(0.0, 1.0) * 65535.0 + 0.5).floor() as u16;
    assert_eq!(code, want, "level 1 height is not its plane quantised");
    let above = chain[0].height.as_ref().unwrap();
    let mean = (above[0] + above[1] + above[N as usize] + above[N as usize + 1]) / 4.0;
    assert!(close(height[0], mean), "{} is not {mean}", height[0]);

    // Emissive is a half-float and unclamped: the graph runs it to four, and
    // the level carries the mean of what was there.
    let emissive = chain[1].emissive.as_ref().unwrap();
    let bytes = &set.emissive.as_ref().unwrap().mips[1];
    assert_eq!(
        u16::from_le_bytes([bytes[0], bytes[1]]),
        f32_to_f16_bits(emissive[0][0]),
        "level 1 emissive is not its plane in halves"
    );
    assert!(
        chain[0]
            .emissive
            .as_ref()
            .unwrap()
            .iter()
            .any(|t| t[0] > 1.0),
        "the fixture's emissive should leave 0..=1, which is what the half is for"
    );
}

#[test]
fn the_chain_and_the_encode_of_it_are_the_same_levels() {
    // Two public ways in — `mips` for the numbers and `encode_mips` for the
    // bytes — and a bake takes the second. They walk the same filter, and this
    // is what says so.
    let level0 = planes(&relief(), N);
    let chain = level0.mips();
    let set = level0.encode_mips(Dither::Ordered);
    let mut expected = level0.encode(Dither::Ordered);
    for level in &chain[1..] {
        let encoded = level.encode(Dither::Ordered);
        expected.base_color.mips.extend(encoded.base_color.mips);
        expected.normal.mips.extend(encoded.normal.mips);
        expected.orm.mips.extend(encoded.orm.mips);
        expected
            .height
            .as_mut()
            .unwrap()
            .mips
            .extend(encoded.height.unwrap().mips);
        expected
            .emissive
            .as_mut()
            .unwrap()
            .mips
            .extend(encoded.emissive.unwrap().mips);
    }
    assert_eq!(set, expected);

    // And level 0 of a mipped set is byte for byte the set without mips, which
    // is what keeps the golden hashes of the study the same bake they were.
    assert_eq!(
        set.base_color.mips[0],
        level0.encode(Dither::Ordered).base_color.mips[0]
    );
}
