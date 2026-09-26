//! The KTX2 writer, read back by somebody else's parser.
//!
//! The container is written here by hand, which is only worth doing if what
//! comes out is a KTX2 file by the specification's reading and not by this
//! crate's. So every test below parses the bytes with the `ktx2` crate — the
//! same version and the same parser Bevy's own loader uses — and asserts on
//! what *it* saw: the format, the dimensions, the level count, each level's
//! bytes, the data format descriptor, and the writer key. `Reader::new`
//! validates the identifier, every section's bounds, the level index and the
//! descriptor blocks as it parses, so a file that comes back at all is a file
//! whose layout holds together.
//!
//! The descriptor gets a stronger check than that: the `ktx2` crate can
//! generate the basic block for a `VkFormat` itself, so the one written here is
//! compared against that, sample for sample. That is what pins the transfer
//! function a loader picks the colour space from.
#![allow(
    clippy::unwrap_used,
    reason = "fixtures built in the test; a failure is a test failure"
)]
#![allow(
    clippy::cast_possible_truncation,
    reason = "level indices and texel counts bounded by the sizes these tests write"
)]
use std::{collections::BTreeMap, num::NonZeroUsize};

use ashlar_material::{
    MaterialGraph, MaterialGraphLibrary, PbrOutput,
    bake::{BakeRequest, Encoded, PlaneFormat, TextureSet, bake},
    ktx2::{HEADER_BYTES, IDENTIFIER, Ktx2Error, inspect, plane_format, vk_format, write},
    mips::levels,
    nodes::{Colorize, Levels, Noise},
};

/// The four formats a bake writes, each with the `ktx2` crate's name for it and
/// the `typeSize` the container should declare.
const FORMATS: [(PlaneFormat, ktx2::Format, u32); 4] = [
    (PlaneFormat::Rgba8Srgb, ktx2::Format::R8G8B8A8_SRGB, 1),
    (PlaneFormat::Rgba8Unorm, ktx2::Format::R8G8B8A8_UNORM, 1),
    (PlaneFormat::R16Unorm, ktx2::Format::R16_UNORM, 2),
    (
        PlaneFormat::Rgba16Float,
        ktx2::Format::R16G16B16A16_SFLOAT,
        2,
    ),
];

/// What the `KTXwriter` value starts with, whatever version the crate is at.
const WRITER: &str = "ashlar-material ";

/// One level of a map, filled with a pattern that depends on the level, so a
/// test that confused two levels fails rather than passing on equal bytes.
fn level(format: PlaneFormat, size: u32, tag: u8) -> Vec<u8> {
    let texels = size as usize * size as usize * format.bytes_per_texel();
    (0..texels)
        .map(|index| (index as u8).wrapping_mul(31).wrapping_add(tag))
        .collect()
}

/// A whole chain of made-up levels in one format, level 0 at `resolution`.
fn chain(format: PlaneFormat, resolution: u32) -> Encoded {
    Encoded {
        format,
        mips: (0..levels(resolution))
            .map(|n| level(format, (resolution >> n).max(1), n as u8 + 1))
            .collect(),
    }
}

/// The level index as the file stores it: one `(offset, length)` per level,
/// level 0 first, read out of the raw bytes rather than through the parser,
/// because where a level sits is exactly what is being asserted.
fn level_index(file: &[u8], levels: usize) -> Vec<(usize, usize)> {
    (0..levels)
        .map(|n| {
            let at = HEADER_BYTES + n * 24;
            let word = |k: usize| {
                u64::from_le_bytes(file[at + k * 8..at + k * 8 + 8].try_into().unwrap()) as usize
            };
            assert_eq!(word(1), word(2), "uncompressed length, with no compression");
            (word(0), word(1))
        })
        .collect()
}

#[test]
fn every_format_round_trips_through_the_ktx2_crate() {
    for (format, expected, type_size) in FORMATS {
        let encoded = chain(format, 32);
        let file = write(&encoded, 32).unwrap();
        let reader = ktx2::Reader::new(file.as_slice()).unwrap();
        let header = reader.header();
        assert_eq!(header.format, Some(expected), "{format:?}");
        assert_eq!(header.type_size, type_size, "{format:?}");
        assert_eq!((header.pixel_width, header.pixel_height), (32, 32));
        // A plain 2D image: no volume, no array, one face, and the face count
        // is one rather than the zero the other two use.
        assert_eq!(
            (header.pixel_depth, header.layer_count, header.face_count),
            (0, 0, 1),
            "{format:?}"
        );
        assert_eq!(header.level_count as usize, levels(32), "{format:?}");
        assert_eq!(header.supercompression_scheme, None, "{format:?}");

        // Levels come back largest first, each byte for byte what went in.
        let read: Vec<&[u8]> = reader.levels().map(|level| level.data).collect();
        assert_eq!(read.len(), encoded.mips.len(), "{format:?}");
        for (n, (written, back)) in encoded.mips.iter().zip(&read).enumerate() {
            assert_eq!(written.as_slice(), *back, "{format:?} level {n}");
        }
        let writer = reader.writer().unwrap();
        assert!(
            writer.trim_end_matches('\0').starts_with(WRITER),
            "{writer:?}"
        );
    }
}

#[test]
fn the_descriptor_is_the_one_the_format_generates() {
    for (format, expected, type_size) in FORMATS {
        let file = write(&chain(format, 8), 8).unwrap();
        let reader = ktx2::Reader::new(file.as_slice()).unwrap();
        let written = reader.basic_dfd().expect("a basic descriptor block");
        let (generated, generated_type_size) =
            ktx2::dfd::Basic::from_format(expected).expect("the crate knows this format");
        // Sample for sample: the bit offsets and lengths, the channel ids, the
        // qualifiers — including the alpha of an sRGB map being marked linear —
        // and each channel's range.
        assert_eq!(*written, generated, "{format:?}");
        assert_eq!(generated_type_size, type_size, "{format:?}");

        // And the part of it a loader actually acts on.
        let srgb = format == PlaneFormat::Rgba8Srgb;
        assert_eq!(
            reader.transfer_function(),
            Some(if srgb {
                ktx2::TransferFunction::SRGB
            } else {
                ktx2::TransferFunction::Linear
            }),
            "{format:?} is the wrong colour space"
        );
        assert_eq!(reader.color_model(), Some(ktx2::ColorModel::RGBSDA));
        assert_eq!(reader.is_alpha_premultiplied(), Some(false));
    }
}

#[test]
fn the_levels_are_stored_smallest_first_and_aligned() {
    for (format, ..) in FORMATS {
        let encoded = chain(format, 64);
        let file = write(&encoded, 64).unwrap();
        let index = level_index(&file, encoded.mips.len());
        // The index runs largest first and the data runs the other way, which
        // is the one thing about this container that is easy to get backwards.
        for pair in index.windows(2) {
            assert!(
                pair[0].0 > pair[1].0,
                "{format:?}: level data is ordered smallest first in the file"
            );
        }
        // Every level's offset is a multiple of the least common multiple of
        // the texel size and four, which for the half-float emissive is eight
        // and for everything else is four.
        let alignment = format.bytes_per_texel().max(4);
        for (n, (at, length)) in index.iter().enumerate() {
            assert_eq!(at % alignment, 0, "{format:?} level {n} at {at}");
            assert_eq!(*length, encoded.mips[n].len(), "{format:?} level {n}");
        }
        // Nothing overlaps, and the file ends with the largest level.
        let (last_at, last_len) = index[0];
        assert_eq!(last_at + last_len, file.len(), "{format:?}");
    }
}

#[test]
fn a_map_with_one_level_is_a_file_with_one_level() {
    // What a bake without mips writes, and a legal KTX2 file: `levelCount` is
    // one, and a loader that wants a chain makes or point samples its own.
    let encoded = Encoded {
        format: PlaneFormat::Rgba8Srgb,
        mips: vec![level(PlaneFormat::Rgba8Srgb, 16, 7)],
    };
    let file = write(&encoded, 16).unwrap();
    let reader = ktx2::Reader::new(file.as_slice()).unwrap();
    assert_eq!(reader.header().level_count, 1);
    assert_eq!(reader.levels().count(), 1);
    assert_eq!(reader.levels().next().unwrap().data, encoded.mips[0]);
    assert_eq!(inspect(&file).unwrap().levels, 1);
}

#[test]
fn a_bake_with_mips_writes_every_level_of_every_map() {
    let set = study(256);
    let mut maps = vec![
        ("base_color", &set.base_color),
        ("normal", &set.normal),
        ("orm", &set.orm),
    ];
    maps.push(("height", set.height.as_ref().expect("a height map")));
    maps.push(("emissive", set.emissive.as_ref().expect("an emissive map")));
    for (name, encoded) in maps {
        let file = write(encoded, set.resolution).unwrap();
        let reader = ktx2::Reader::new(file.as_slice()).unwrap();
        assert_eq!(
            reader.header().format,
            Some(format_of(encoded.format)),
            "{name}"
        );
        assert_eq!(reader.header().pixel_width, 256, "{name}");
        assert_eq!(reader.header().level_count as usize, levels(256), "{name}");
        let read: Vec<&[u8]> = reader.levels().map(|level| level.data).collect();
        assert_eq!(read.len(), encoded.mips.len(), "{name}");
        for (n, (baked, back)) in encoded.mips.iter().zip(&read).enumerate() {
            // The level a renderer will sample, texel for texel: a chain that
            // arrived shuffled or short would pass every structural check above
            // and still be the wrong picture at distance.
            assert_eq!(baked.as_slice(), *back, "{name} level {n}");
            let size = (256_usize >> n).max(1);
            assert_eq!(
                back.len(),
                size * size * encoded.format.bytes_per_texel(),
                "{name} level {n}"
            );
        }
        // And what the crate's own reader makes of the same bytes.
        let info = inspect(&file).unwrap();
        assert_eq!(info.plane_format(), Some(encoded.format), "{name}");
        assert_eq!((info.width, info.height), (256, 256), "{name}");
        assert_eq!(info.levels, levels(256), "{name}");
        assert_eq!(info.supercompression, 0, "{name}");
    }
}

#[test]
fn inspect_refuses_what_is_not_a_file_this_crate_would_write() {
    let file = write(&chain(PlaneFormat::Rgba8Unorm, 16), 16).unwrap();
    assert!(inspect(&file).is_ok());

    // Not KTX2 at all.
    assert_eq!(
        inspect(b"PNG, actually").unwrap_err(),
        Ktx2Error::Identifier
    );
    let mut wrong = file.clone();
    wrong[3] = b'!';
    assert_eq!(inspect(&wrong).unwrap_err(), Ktx2Error::Identifier);

    // Long enough to be a header, and not long enough to be the file it claims.
    let short = &file[..HEADER_BYTES + 8];
    assert!(matches!(
        inspect(short).unwrap_err(),
        Ktx2Error::Truncated { .. }
    ));
    assert!(matches!(
        inspect(&IDENTIFIER).unwrap_err(),
        Ktx2Error::Truncated {
            needed: HEADER_BYTES,
            found: 12
        }
    ));

    // A header that claims a bigger picture than its levels hold. The bytes are
    // all there and every offset is inside the file; what is wrong is that the
    // level index no longer describes a 32-wide image, and a loader handed this
    // reads a row of one level as a row of another.
    let mut lying = file.clone();
    lying[20] = 32;
    assert!(matches!(
        inspect(&lying).unwrap_err(),
        Ktx2Error::Level { level: 0, .. }
    ));

    // A cube map is a KTX2 file and is not a material map. `faceCount` is the
    // eighth word of the header, after the twelve-byte identifier.
    let mut cube = file.clone();
    cube[36] = 6;
    assert!(matches!(
        inspect(&cube).unwrap_err(),
        Ktx2Error::Shape { faces: 6, .. }
    ));
}

#[test]
fn write_refuses_a_map_that_does_not_describe_its_own_resolution() {
    let format = PlaneFormat::Rgba8Unorm;
    // A level 0 of the wrong size, which is the mistake a writer that passed
    // the wrong resolution would make.
    let encoded = Encoded {
        format,
        mips: vec![level(format, 16, 1)],
    };
    assert_eq!(
        write(&encoded, 32).unwrap_err(),
        Ktx2Error::Level {
            level: 0,
            expected: 32 * 32 * 4,
            found: 16 * 16 * 4,
        }
    );

    // A chain with a level that did not halve.
    let encoded = Encoded {
        format,
        mips: vec![level(format, 16, 1), level(format, 4, 2)],
    };
    assert!(matches!(
        write(&encoded, 16).unwrap_err(),
        Ktx2Error::Level { level: 1, .. }
    ));

    // No levels, and more levels than a resolution has.
    assert!(matches!(
        write(
            &Encoded {
                format,
                mips: vec![]
            },
            16
        )
        .unwrap_err(),
        Ktx2Error::Levels { levels: 0, .. }
    ));
    let mut too_many = chain(format, 16);
    too_many.mips.push(vec![0; 4]);
    assert!(matches!(
        write(&too_many, 16).unwrap_err(),
        Ktx2Error::Levels { most: 5, .. }
    ));
    assert_eq!(
        write(&chain(format, 16), 0).unwrap_err(),
        Ktx2Error::Resolution(0)
    );
}

#[test]
fn the_format_table_is_the_same_table_read_both_ways() {
    for (format, expected, _) in FORMATS {
        assert_eq!(vk_format(format), expected.value(), "{format:?}");
        assert_eq!(plane_format(vk_format(format)), Some(format), "{format:?}");
    }
    // A format this crate does not write is not an error, it is simply not one
    // of ours: a BC7 file is a KTX2 file.
    assert_eq!(plane_format(0), None);
    assert_eq!(plane_format(ktx2::Format::BC7_SRGB_BLOCK.value()), None);
}

/// The `ktx2` crate's name for one of the four formats.
fn format_of(format: PlaneFormat) -> ktx2::Format {
    FORMATS
        .iter()
        .find_map(|(ours, theirs, _)| (*ours == format).then_some(*theirs))
        .expect("the four formats a bake writes")
}

/// A bake of a small graph that binds every map, so the two optional formats
/// are exercised by a real chain rather than by a fixture.
fn study(resolution: u32) -> TextureSet {
    let graph = MaterialGraph::builder("test:maps")
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
                .normal_strength(0.05),
        )
        .into_graph();
    let (library, params) = (MaterialGraphLibrary::default(), BTreeMap::new());
    bake(&BakeRequest {
        graph: &graph,
        library: &library,
        params: &params,
        resolution,
        mips: true,
        // A named count rather than the machine's: a test binary runs its cases
        // in parallel already.
        threads: NonZeroUsize::new(3),
    })
    .unwrap()
}

/// The same container with its levels compressed, which is what the study ships
/// and what the `zstd` feature exists for.
///
/// The writer is checked against a *decoder* rather than against itself: every
/// level is parsed out by the `ktx2` crate and inflated by `zstd`, and what
/// comes back has to be the bytes that went in. Nothing else about the file may
/// move — a loader reads the format, the dimensions and the transfer function
/// from the same places either way — so the tests that pin those run over a
/// supercompressed file too.
#[cfg(feature = "zstd")]
mod supercompressed {
    use super::{FORMATS, WRITER, chain, study};
    use ashlar_material::{
        bake::PlaneFormat,
        ktx2::{HEADER_BYTES, Ktx2Error, Supercompression, inspect, write, write_with},
        mips::levels,
    };

    /// The file, and the levels it carries once they are inflated.
    fn round_trip(
        encoded: &ashlar_material::bake::Encoded,
        resolution: u32,
    ) -> (Vec<u8>, Vec<Vec<u8>>) {
        let file = write_with(encoded, resolution, Supercompression::Zstd).unwrap();
        let reader = ktx2::Reader::new(file.as_slice()).unwrap();
        let read = reader
            .levels()
            .map(|level| {
                assert!(
                    level.uncompressed_byte_length > 0,
                    "a zstd level records what it comes to"
                );
                let inflated = zstd::decode_all(level.data).expect("a zstd frame");
                assert_eq!(inflated.len() as u64, level.uncompressed_byte_length);
                inflated
            })
            .collect();
        (file, read)
    }

    #[test]
    fn every_format_round_trips_through_a_decoder_with_its_header_unmoved() {
        for (format, expected, type_size) in FORMATS {
            let encoded = chain(format, 32);
            let (file, read) = round_trip(&encoded, 32);
            let reader = ktx2::Reader::new(file.as_slice()).unwrap();
            let header = reader.header();
            assert_eq!(
                header.supercompression_scheme,
                Some(ktx2::SupercompressionScheme::Zstandard),
                "{format:?}"
            );
            // Everything a loader reads to find out what the file is, which a
            // compressed file says exactly as a plain one does.
            assert_eq!(header.format, Some(expected), "{format:?}");
            assert_eq!(header.type_size, type_size, "{format:?}");
            assert_eq!(header.pixel_width, 32, "{format:?}");
            assert_eq!(header.pixel_height, 32, "{format:?}");
            assert_eq!(header.level_count as usize, levels(32), "{format:?}");
            let (generated, _) =
                ktx2::dfd::Basic::from_format(expected).expect("the crate knows this format");
            assert_eq!(
                *reader.basic_dfd().expect("a basic descriptor block"),
                generated,
                "{format:?}"
            );
            assert!(reader.writer().unwrap().starts_with(WRITER), "{format:?}");
            // And the levels themselves, in order, texel for texel.
            assert_eq!(read, encoded.mips, "{format:?}");
            // The crate's own reader, which has no decoder in it, agrees about
            // what the file claims.
            let info = inspect(&file).unwrap();
            assert_eq!(info.supercompression, 2, "{format:?}");
            assert_eq!(info.plane_format(), Some(format), "{format:?}");
            assert_eq!(info.levels, levels(32), "{format:?}");
        }
    }

    #[test]
    fn a_compressed_chain_carries_the_same_pictures_in_a_fraction_of_the_bytes() {
        let set = study(256);
        let mut plain_total = 0;
        let mut small_total = 0;
        let mut height = (0, 0);
        for (name, encoded) in [
            ("base_color", &set.base_color),
            ("normal", &set.normal),
            ("orm", &set.orm),
            ("height", set.height.as_ref().unwrap()),
            ("emissive", set.emissive.as_ref().unwrap()),
        ] {
            let plain = write(encoded, set.resolution).unwrap();
            let (small, read) = round_trip(encoded, set.resolution);
            assert_eq!(read, encoded.mips, "{name}");
            println!(
                "{name}: {} compressed against {} plain ({}%)",
                small.len(),
                plain.len(),
                100 * small.len() / plain.len()
            );
            plain_total += plain.len();
            small_total += small.len();
            if name == "height" {
                height = (small.len(), plain.len());
            }
        }
        // A bake is noise and zstd is not an image codec, so what is asserted
        // is "worth the scheme" rather than a ratio: this graph's maps come
        // back at a third to three fifths of their bytes.
        assert!(
            small_total * 10 < plain_total * 7,
            "{small_total} compressed against {plain_total} plain"
        );
        // Except the height, which does not compress at all, and this is where
        // that is written down: it is a 16-bit plane of a noise field, so its
        // low byte is very nearly random and there is nothing for a matcher to
        // find. At 256 squared the frame headers cost as much as the matches
        // save. It still ships compressed — at 1024 the shipped heights come
        // back at 87% and the container has no way to store one level of a file
        // raw — and a future encoder that byte-split the plane first would
        // break this assertion, which is the moment to read this comment.
        assert!(
            height.0 * 20 > height.1 * 19,
            "the 16-bit height compressed to {} from {}, which is better than \
             this test expects a plane of noise to do",
            height.0,
            height.1
        );
    }

    #[test]
    fn a_compressed_file_packs_its_levels_and_records_what_each_comes_to() {
        let encoded = chain(PlaneFormat::R16Unorm, 64);
        let file = write_with(&encoded, 64, Supercompression::Zstd).unwrap();
        // Offset, stored length, uncompressed length, out of the raw index.
        let entries: Vec<(usize, usize, usize)> = (0..encoded.mips.len())
            .map(|n| {
                let at = HEADER_BYTES + n * 24;
                let word = |k: usize| {
                    u64::from_le_bytes(file[at + k * 8..at + k * 8 + 8].try_into().unwrap())
                        as usize
                };
                (word(0), word(1), word(2))
            })
            .collect();
        for (n, (offset, stored, uncompressed)) in entries.iter().copied().enumerate() {
            let size = (64_usize >> n).max(1);
            assert_eq!(uncompressed, size * size * 2, "level {n}");
            assert!(stored > 0 && offset + stored <= file.len(), "level {n}");
        }
        // The level a renderer spends its bytes on shrinks; the bottom of the
        // chain does not, and is allowed not to. A 4x4 level is thirty-two
        // bytes and a zstd frame's header is a dozen of them, so the smallest
        // levels come out *larger* than they went in. Nothing in the container
        // lets a level opt out of the scheme, and a few hundred bytes at the
        // bottom of a chain against a megabyte at the top is not a reason to
        // want one.
        assert!(entries[0].1 < entries[0].2, "level 0 is smaller compressed");
        assert!(
            entries.last().unwrap().1 > entries.last().unwrap().2,
            "the 1x1 level is a frame header around two bytes"
        );
        // Smallest first and packed end to end: a supercompressed level is not
        // texels, so the specification asks for no alignment and there is
        // nothing between one level and the next.
        for pair in entries.windows(2) {
            let (later, earlier) = (pair[0], pair[1]);
            assert_eq!(
                earlier.0 + earlier.1,
                later.0,
                "a level begins where the smaller one ended"
            );
        }
        assert_eq!(
            entries[0].0 + entries[0].1,
            file.len(),
            "level 0 is last and the file ends with it"
        );
    }

    #[test]
    fn two_writes_of_one_map_are_the_same_file() {
        // What the shipped assets rest on: the golden test compares the files
        // on disk with a fresh bake byte for byte, and a compressor that
        // answered differently twice would make that test a coin toss.
        let encoded = chain(PlaneFormat::Rgba8Srgb, 64);
        let once = write_with(&encoded, 64, Supercompression::Zstd).unwrap();
        let twice = write_with(&encoded, 64, Supercompression::Zstd).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn inspect_refuses_a_level_that_does_not_come_to_the_size_its_own_index_claims() {
        let encoded = chain(PlaneFormat::Rgba8Unorm, 16);
        let mut file = write_with(&encoded, 16, Supercompression::Zstd).unwrap();
        assert!(inspect(&file).is_ok());
        // Level 0's uncompressed length, which is the one number a reader with
        // no decoder can check a compressed level against.
        let at = HEADER_BYTES + 16;
        file[at..at + 8].copy_from_slice(&7_u64.to_le_bytes());
        assert_eq!(
            inspect(&file),
            Err(Ktx2Error::Level {
                level: 0,
                expected: 16 * 16 * 4,
                found: 7,
            })
        );
    }
}
