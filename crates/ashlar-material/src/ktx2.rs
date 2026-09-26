//! KTX2: the container a baked map goes on disk in, written by hand.
//!
//! A PNG cannot carry a mip chain, and Bevy 0.19 does not build one for an
//! image it loaded, so a PNG bake throws away the renormalised normals and the
//! widened roughness that [`mips`](crate::mips) exists for. KTX2 carries every
//! level, says which colour space the bytes are in, and is simple enough to
//! write without a dependency: an eighty-byte header, a level index, a data
//! format descriptor, a key/value block, and the levels.
//!
//! [`write()`] takes one [`Encoded`] map and the resolution of its level 0 and
//! answers the file. [`write_with`] is the same with a
//! [`Supercompression`] chosen. [`inspect`] reads a file's header and level
//! index back, which is what a preflight wants: it says what the file claims to
//! be and whether the claim is consistent with its own length, without decoding
//! a texel.
//!
//! # What the writer commits to
//!
//! - **Supercompression is the caller's choice, and it is per level.**
//!   [`write()`] stores the encoder's bytes unchanged; [`write_with`] and
//!   `Supercompression::Zstd` compress each level on its own under scheme 2,
//!   recording the stored length and the uncompressed one the spec asks for, so
//!   a loader that wants level 6 decompresses level 6. Zstd needs a decoder on
//!   the loading side and this crate never links one — the file is the
//!   container, and whoever reads it brings the decoder, as `ashlar-bevy` does
//!   through Bevy's own.
//! - **One plain 2D image**: `pixelDepth` 0, `layerCount` 0, `faceCount` 1. No
//!   array, cube map or volume comes out of a bake.
//! - **Levels largest first in the index, smallest first in the file.** That is
//!   the spec's order and not an implementation detail: a loader streaming the
//!   file gets the small levels it can show first.
//! - **A real data format descriptor.** It is mandatory, and it is where a
//!   loader reads the transfer function from, so base colour says sRGB and
//!   every other map says linear. The alpha of an sRGB map is marked linear,
//!   as alpha always is.
//! - **`KTXwriter`**, so a file found later says what wrote it.
//!
//! ```
//! use ashlar_material::{
//!     bake::{Encoded, PlaneFormat},
//!     ktx2::{self, Ktx2Error},
//! };
//!
//! // Two levels of a tiny linear map: 2x2, then the 1x1 under it.
//! let encoded = Encoded {
//!     format: PlaneFormat::Rgba8Unorm,
//!     mips: vec![vec![0x20; 2 * 2 * 4], vec![0x20; 4]],
//! };
//! let file = ktx2::write(&encoded, 2)?;
//! let info = ktx2::inspect(&file)?;
//! assert_eq!((info.width, info.height, info.levels), (2, 2, 2));
//! assert_eq!(info.plane_format(), Some(PlaneFormat::Rgba8Unorm));
//! # Ok::<(), Ktx2Error>(())
//! ```
//!
//! The same map again, with its levels compressed. A file written this way says
//! so in its header, and the levels it carries are no longer the bytes that went
//! in — which is the whole of the difference, and why a reader has to know.
//!
//! ```
//! # #[cfg(feature = "zstd")] {
//! use ashlar_material::{
//!     bake::{Encoded, PlaneFormat},
//!     ktx2::{self, Supercompression},
//! };
//!
//! // One flat 64-squared level, which is the kind of thing a compressor eats.
//! let encoded = Encoded {
//!     format: PlaneFormat::Rgba8Unorm,
//!     mips: vec![vec![0x20; 64 * 64 * 4]],
//! };
//! let plain = ktx2::write(&encoded, 64).expect("a KTX2 file");
//! let small = ktx2::write_with(&encoded, 64, Supercompression::Zstd).expect("a KTX2 file");
//! assert!(small.len() < plain.len() / 10);
//! assert_eq!(ktx2::inspect(&small).expect("a KTX2 file").supercompression, 2);
//! # }
//! ```

use crate::bake::{Encoded, PlaneFormat};
use crate::mips::levels;

/// The twelve bytes every KTX2 file starts with: `«KTX 20»\r\n\x1A\n`.
pub const IDENTIFIER: [u8; 12] = [
    0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A,
];

/// The header and the index that follows it, which the spec fixes at eighty
/// bytes together. The level index begins here.
pub const HEADER_BYTES: usize = 80;

/// One level index entry: byte offset, byte length, uncompressed byte length,
/// all `u64`.
const LEVEL_ENTRY_BYTES: usize = 24;

/// `VK_FORMAT_R8G8B8A8_UNORM`.
const VK_R8G8B8A8_UNORM: u32 = 37;
/// `VK_FORMAT_R8G8B8A8_SRGB`.
const VK_R8G8B8A8_SRGB: u32 = 43;
/// `VK_FORMAT_R16_UNORM`.
const VK_R16_UNORM: u32 = 70;
/// `VK_FORMAT_R16G16B16A16_SFLOAT`.
const VK_R16G16B16A16_SFLOAT: u32 = 97;

/// Why a KTX2 file could not be written, or is not one.
///
/// Non-exhaustive for the same reason [`BakeError`](crate::bake::BakeError) is:
/// a reason to refuse added later must not break a caller's match.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Ktx2Error {
    /// A resolution of zero. An image is at least one texel.
    #[error("resolution: a KTX2 image is at least one texel, not {0}")]
    Resolution(u32),
    /// No levels at all, or more levels than the resolution can halve into.
    #[error("levels: {resolution} squared holds 1..={most} levels, not {levels}")]
    Levels {
        /// How many levels the map carried.
        levels: usize,
        /// How many a full chain over this resolution has.
        most: usize,
        /// The resolution of level 0.
        resolution: u32,
    },
    /// A level whose length is not what its format and its own size require.
    #[error("level {level} is {found} bytes, not the {expected} its format and size require")]
    Level {
        /// Which level, level 0 being the largest.
        level: usize,
        /// What the format and the level's dimensions come to.
        expected: usize,
        /// What the map actually carried.
        found: usize,
    },
    /// The bytes do not begin with the KTX2 identifier.
    #[error("not a KTX2 file: it does not begin with the KTX2 identifier")]
    Identifier,
    /// The file ends before something its own header named.
    #[error("truncated KTX2 file: {needed} bytes are named and the file is {found}")]
    Truncated {
        /// The end of the furthest thing the header or the level index named.
        needed: usize,
        /// The length of the file.
        found: usize,
    },
    /// The compressor refused a level.
    ///
    /// Only a supercompressed write can raise this, and in practice only an
    /// allocation failure inside the encoder does: the input is a slice this
    /// crate holds and the output is a `Vec`. The reason is the encoder's own
    /// message, kept as a string so this type stays comparable.
    #[error("compressing level {level}: {reason}")]
    Compress {
        /// Which level, level 0 being the largest.
        level: usize,
        /// What the encoder said.
        reason: String,
    },
    /// Not one plain 2D image: an array, a cube map or a volume.
    #[error(
        "not a plain 2D KTX2 texture: {faces} faces, {layers} layers, depth {depth}, \
         {width}x{height}"
    )]
    Shape {
        /// Texels across.
        width: u32,
        /// Texels down. Zero means a 1D texture.
        height: u32,
        /// Texels through. Non-zero means a volume.
        depth: u32,
        /// Array layers. Non-zero means an array.
        layers: u32,
        /// Cube map faces. Six means a cube map.
        faces: u32,
    },
}

/// Zstandard, which is supercompression scheme 2 in the KTX2 registry.
///
/// Named here rather than left as a literal because [`inspect`] answers it and
/// a reader comparing against it should not have to look the number up.
pub const ZSTD_SCHEME: u32 = 2;

/// The compression level `Supercompression::Zstd` writes at.
///
/// Nineteen, the highest of zstd's ordinary levels, because these files are
/// written once by a content step and read by everyone who clones the
/// repository. Measured over the study's twenty maps, 93.3 MiB of levels: level
/// 9 gives 26.9 MiB in 1.2 s, level 12 gives 26.0 MiB in 2.2 s, and level 19
/// gives 23.1 MiB in 21.6 s. Twenty seconds on a step run by hand buys twelve
/// per cent, and the twelve per cent is what is committed.
///
/// Fixed rather than a parameter, so that a file is reproducible from the graph
/// that wrote it: the shipped assets are checked byte for byte against a fresh
/// bake, and a knob here is one more way for two runs to disagree.
#[cfg(feature = "zstd")]
pub const ZSTD_LEVEL: i32 = 19;

/// What a file's levels are stored under.
///
/// Non-exhaustive because the registry has more schemes than this crate writes,
/// and because `Supercompression::Zstd` exists only under the `zstd` feature:
/// a match on this must carry a wildcard either way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Supercompression {
    /// The encoder's bytes, unchanged. Scheme 0.
    #[default]
    None,
    /// Zstandard, scheme 2: each level compressed on its own at
    /// `ZSTD_LEVEL`, with the uncompressed length recorded beside the stored
    /// one.
    ///
    /// Per level rather than over the file because that is what the container
    /// says: a loader that wants one level decompresses one level. It costs a
    /// little ratio — the small levels of a chain are a few hundred bytes each
    /// and a zstd frame header is a dozen of them — and a level of a chain is
    /// the unit a streaming loader asks for.
    #[cfg(feature = "zstd")]
    Zstd,
}

impl Supercompression {
    /// The scheme number a header records for this.
    const fn scheme(self) -> u32 {
        match self {
            Self::None => 0,
            #[cfg(feature = "zstd")]
            Self::Zstd => ZSTD_SCHEME,
        }
    }

    /// The alignment a level's offset must be a multiple of under this scheme.
    ///
    /// One for anything supercompressed, which is what the specification says:
    /// the stored bytes are not texels, so there is nothing for a GPU upload to
    /// be aligned for, and padding between levels would be bytes committed for
    /// nothing.
    const fn alignment(self, format: PlaneFormat) -> usize {
        match self {
            Self::None => alignment(format),
            #[cfg(feature = "zstd")]
            Self::Zstd => 1,
        }
    }

    /// One level's bytes as they are stored, or what the encoder said instead.
    ///
    /// The level number is the caller's to attach: this knows how a level is
    /// compressed and not which level it is.
    #[cfg_attr(
        not(feature = "zstd"),
        allow(
            clippy::unnecessary_wraps,
            reason = "with no encoder compiled in there is nothing here that can fail; the \
                      signature is the one the `zstd` feature needs and the caller is \
                      written once"
        )
    )]
    fn store(self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        match self {
            Self::None => Ok(bytes.to_vec()),
            #[cfg(feature = "zstd")]
            Self::Zstd => zstd::encode_all(bytes, ZSTD_LEVEL).map_err(|error| error.to_string()),
        }
    }
}

/// The `VkFormat` value one encoded plane is written under.
///
/// These four are what a bake produces and what Bevy's KTX2 loader accepts
/// without transcoding.
pub const fn vk_format(format: PlaneFormat) -> u32 {
    match format {
        PlaneFormat::Rgba8Srgb => VK_R8G8B8A8_SRGB,
        PlaneFormat::Rgba8Unorm => VK_R8G8B8A8_UNORM,
        PlaneFormat::R16Unorm => VK_R16_UNORM,
        PlaneFormat::Rgba16Float => VK_R16G16B16A16_SFLOAT,
    }
}

/// The plane format a `VkFormat` value names, where this crate writes one.
///
/// `None` is not "invalid": it is a format this crate does not write, such as a
/// block-compressed one or a Basis file whose `vkFormat` is undefined. A reader
/// that only wants to know whether a file is loadable should ask the loader.
pub const fn plane_format(vk_format: u32) -> Option<PlaneFormat> {
    match vk_format {
        VK_R8G8B8A8_SRGB => Some(PlaneFormat::Rgba8Srgb),
        VK_R8G8B8A8_UNORM => Some(PlaneFormat::Rgba8Unorm),
        VK_R16_UNORM => Some(PlaneFormat::R16Unorm),
        VK_R16G16B16A16_SFLOAT => Some(PlaneFormat::Rgba16Float),
        _ => None,
    }
}

/// What a KTX2 file's header and level index say about it.
///
/// Everything here is the file's own claim, checked against the file's length
/// and nothing else. That is what a preflight can afford: opening a texture to
/// find out whether it is a texture, without decoding a million texels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ktx2Info {
    /// The `VkFormat` the header names. Zero is `VK_FORMAT_UNDEFINED`, which a
    /// Basis Universal file uses and a bake never writes.
    pub vk_format: u32,
    /// Texels across, at level 0.
    pub width: u32,
    /// Texels down, at level 0.
    pub height: u32,
    /// How many levels the file carries, level 0 included.
    pub levels: usize,
    /// The supercompression scheme, zero for none and [`ZSTD_SCHEME`] for the
    /// one this crate writes. A non-zero one means the level bytes are
    /// compressed and a decoder is needed to read them; the level index's
    /// uncompressed lengths say what they come to.
    pub supercompression: u32,
}

impl Ktx2Info {
    /// The plane format this file's `vkFormat` names, where this crate writes
    /// one.
    pub const fn plane_format(&self) -> Option<PlaneFormat> {
        plane_format(self.vk_format)
    }
}

/// Write one encoded map as a KTX2 file, with every level it carries and no
/// supercompression.
///
/// `resolution` is level 0's size in both axes; level `n` is
/// `(resolution >> n).max(1)` squared, which is how the container itself
/// derives a level's dimensions. Every level is checked against that before a
/// byte is written, because a level index that lies is a file a loader reads as
/// garbage rather than rejects.
///
/// A map with one level is a legal KTX2 file and says so: `levelCount` is 1,
/// and a loader that wants a chain generates or point samples one. That is what
/// a bake without [`mips`](crate::bake::BakeRequest::mips) can offer.
pub fn write(encoded: &Encoded, resolution: u32) -> Result<Vec<u8>, Ktx2Error> {
    write_with(encoded, resolution, Supercompression::None)
}

/// The same, with the levels stored under `supercompression`.
///
/// What changes between the two is three things and no more: the header records
/// the scheme, each level index entry carries the stored length beside the
/// uncompressed one, and the levels are packed rather than aligned — a
/// supercompressed level is not texels, so there is nothing for a GPU upload to
/// be aligned for and the specification asks for no padding. Everything a
/// loader reads to find out *what* the file is — the format, the dimensions,
/// the descriptor's transfer function, the `KTXwriter` key — is written
/// identically, and every level is checked against the size its dimensions
/// imply before it is compressed.
///
/// The levels are compressed one at a time, on this thread. At the study's
/// 1024 squared that is around a second per map at `ZSTD_LEVEL`, which is
/// several times what baking the map cost; it is a content step, and the
/// numbers are on `ZSTD_LEVEL`.
// Offsets and lengths inside a file this function has already laid out and
// bounded: the descriptor and the key/value block are under a kilobyte, the
// level count is under a hundred, and a level offset is an index into a `Vec`
// that exists. Each is written in the width the container declares for it.
#[allow(
    clippy::cast_possible_truncation,
    reason = "container field widths; every value is bounded by the file just laid out"
)]
pub fn write_with(
    encoded: &Encoded,
    resolution: u32,
    supercompression: Supercompression,
) -> Result<Vec<u8>, Ktx2Error> {
    if resolution == 0 {
        return Err(Ktx2Error::Resolution(resolution));
    }
    let texel = encoded.format.bytes_per_texel();
    let most = levels(resolution);
    let count = encoded.mips.len();
    if count == 0 || count > most {
        return Err(Ktx2Error::Levels {
            levels: count,
            most,
            resolution,
        });
    }
    for (level, bytes) in encoded.mips.iter().enumerate() {
        let size = level_size(resolution, level);
        let expected = texels(size, size) * texel;
        if bytes.len() != expected {
            return Err(Ktx2Error::Level {
                level,
                expected,
                found: bytes.len(),
            });
        }
    }

    // What goes in the file, which is the encoder's bytes or those bytes
    // compressed. Done before the layout because a compressed level's length is
    // not a function of anything the caller said: it has to be measured.
    let stored = encoded
        .mips
        .iter()
        .enumerate()
        .map(|(level, bytes)| {
            supercompression
                .store(bytes)
                .map_err(|reason| Ktx2Error::Compress { level, reason })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let descriptor = descriptor(encoded.format);
    let metadata = key_value();
    let index_bytes = count * LEVEL_ENTRY_BYTES;
    let descriptor_at = HEADER_BYTES + index_bytes;
    let metadata_at = descriptor_at + descriptor.len();

    // The levels are stored smallest first, each at an offset the spec calls
    // the required level alignment. Walking the chain backwards from the end of
    // the key/value block is what assigns those offsets, and the padding
    // between two levels is whatever that alignment left.
    let alignment = supercompression.alignment(encoded.format);
    let mut cursor = metadata_at + metadata.len();
    let mut offsets = vec![0_usize; count];
    for level in (0..count).rev() {
        cursor = cursor.next_multiple_of(alignment);
        offsets[level] = cursor;
        cursor += stored[level].len();
    }

    let mut file = vec![0_u8; cursor];
    file[..IDENTIFIER.len()].copy_from_slice(&IDENTIFIER);
    let mut header = Writer::new(&mut file[IDENTIFIER.len()..HEADER_BYTES]);
    header.u32(vk_format(encoded.format));
    header.u32(type_size(encoded.format));
    header.u32(resolution);
    header.u32(resolution);
    // Depth, layers: zero for a plain 2D image. Faces: one, and not zero.
    header.u32(0);
    header.u32(0);
    header.u32(1);
    header.u32(count as u32);
    header.u32(supercompression.scheme());
    // Where the descriptor and the key/value block are, and how long each is.
    header.u32(descriptor_at as u32);
    header.u32(descriptor.len() as u32);
    header.u32(metadata_at as u32);
    header.u32(metadata.len() as u32);
    // Supercompression global data: a BasisLZ file's codebooks, and nothing at
    // all here — zstd needs none, each level being a frame of its own — so its
    // offset and its length are both zero.
    header.u64(0);
    header.u64(0);

    let mut index = Writer::new(&mut file[HEADER_BYTES..descriptor_at]);
    for (level, bytes) in stored.iter().enumerate() {
        index.u64(offsets[level] as u64);
        // The stored length, then the length after decompression. With no
        // supercompression the two are one number, and the spec asks for both
        // all the same.
        index.u64(bytes.len() as u64);
        index.u64(encoded.mips[level].len() as u64);
    }

    file[descriptor_at..metadata_at].copy_from_slice(&descriptor);
    file[metadata_at..metadata_at + metadata.len()].copy_from_slice(&metadata);
    for (level, bytes) in stored.iter().enumerate() {
        file[offsets[level]..offsets[level] + bytes.len()].copy_from_slice(bytes);
    }
    Ok(file)
}

/// Read a KTX2 file's header and level index, checking both against the file's
/// own length.
///
/// What this answers is what the file claims: its format, its size, how many
/// levels it holds, and what its levels are compressed with. What it checks is
/// that those claims fit inside the bytes — the identifier, the header's own
/// sections, and every level's offset and length — and, for a format this crate
/// writes, that each level comes to exactly the size its dimensions require:
/// the stored length where nothing is compressed, and the recorded uncompressed
/// length where something is.
///
/// It is not a decoder and does not pretend to be one. A block-compressed or
/// Basis file passes here with [`Ktx2Info::plane_format`] `None`, because
/// whether such a file loads is the loader's question and not the container's.
pub fn inspect(bytes: &[u8]) -> Result<Ktx2Info, Ktx2Error> {
    // The identifier first, and on however many bytes there are: a file that is
    // a PNG is more usefully reported as not a KTX2 file than as a short one.
    let head = IDENTIFIER.len().min(bytes.len());
    if bytes[..head] != IDENTIFIER[..head] {
        return Err(Ktx2Error::Identifier);
    }
    if bytes.len() < HEADER_BYTES {
        return Err(Ktx2Error::Truncated {
            needed: HEADER_BYTES,
            found: bytes.len(),
        });
    }
    let mut reader = Reader::new(&bytes[IDENTIFIER.len()..HEADER_BYTES]);
    let vk_format = reader.u32();
    let _type_size = reader.u32();
    let width = reader.u32();
    let height = reader.u32();
    let depth = reader.u32();
    let layers = reader.u32();
    let faces = reader.u32();
    let declared = reader.u32();
    let supercompression = reader.u32();
    if width == 0 || height == 0 || depth != 0 || layers != 0 || faces != 1 {
        return Err(Ktx2Error::Shape {
            width,
            height,
            depth,
            layers,
            faces,
        });
    }
    let descriptor_at = reader.u32() as usize;
    let descriptor_bytes = reader.u32() as usize;
    let metadata_at = reader.u32() as usize;
    let metadata_bytes = reader.u32() as usize;
    let global_at = offset(reader.u64());
    let global_bytes = offset(reader.u64());
    for (at, length) in [
        (descriptor_at, descriptor_bytes),
        (metadata_at, metadata_bytes),
        (global_at, global_bytes),
    ] {
        let needed = at.saturating_add(length);
        if needed > bytes.len() {
            return Err(Ktx2Error::Truncated {
                needed,
                found: bytes.len(),
            });
        }
    }

    // A `levelCount` of zero means "generate the chain from level 0", and the
    // file still stores that one level, which is why the index is read at
    // `max(1)` either way.
    let count = declared.max(1) as usize;
    let index_end = HEADER_BYTES + count * LEVEL_ENTRY_BYTES;
    if index_end > bytes.len() {
        return Err(Ktx2Error::Truncated {
            needed: index_end,
            found: bytes.len(),
        });
    }
    let plain = supercompression == 0;
    let format = plane_format(vk_format);
    let mut index = Reader::new(&bytes[HEADER_BYTES..index_end]);
    for level in 0..count {
        let at = offset(index.u64());
        let length = offset(index.u64());
        let uncompressed = offset(index.u64());
        let needed = at.saturating_add(length);
        if needed > bytes.len() {
            return Err(Ktx2Error::Truncated {
                needed,
                found: bytes.len(),
            });
        }
        if let Some(format) = format {
            // The size a level of these dimensions comes to, which is the
            // stored length of a plain file and the *uncompressed* length of a
            // supercompressed one — the one number that means the same thing
            // under either scheme, and the one worth checking without a
            // decoder. A BasisLZ file records zero there and never reaches
            // here, its `vkFormat` being undefined.
            let expected = texels(level_size(width, level), level_size(height, level))
                * format.bytes_per_texel();
            let found = if plain { length } else { uncompressed };
            if found != expected {
                return Err(Ktx2Error::Level {
                    level,
                    expected,
                    found,
                });
            }
        }
    }
    Ok(Ktx2Info {
        vk_format,
        width,
        height,
        levels: count,
        supercompression,
    })
}

/// A `u64` offset or length a file declared, as the `usize` a slice is indexed
/// by.
///
/// Saturating rather than failing here: on a 64-bit target the conversion is
/// the identity, and on a 32-bit one a file that claims an offset past four
/// gigabytes is a file whose bounds check is about to fail anyway, which is
/// where it should be reported rather than here.
fn offset(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// How many texels an image of these dimensions holds.
fn texels(width: u32, height: u32) -> usize {
    width as usize * height as usize
}

/// The size of one axis at `level`, which is how the container derives it:
/// halve, and never below one.
fn level_size(resolution: u32, level: usize) -> u32 {
    let level = u32::try_from(level).unwrap_or(u32::MAX);
    (resolution >> level.min(31)).max(1)
}

/// The `typeSize` a format declares: the size of one component, which is what a
/// big-endian loader would byte-swap by.
const fn type_size(format: PlaneFormat) -> u32 {
    match format {
        PlaneFormat::Rgba8Srgb | PlaneFormat::Rgba8Unorm => 1,
        PlaneFormat::R16Unorm | PlaneFormat::Rgba16Float => 2,
    }
}

/// The alignment every level's offset must be a multiple of, which the spec
/// defines as the least common multiple of the texel size and four.
///
/// Four for the 8-bit maps and for the 16-bit height, eight for the half-float
/// emissive. It is what makes the smallest levels of a chain pad rather than
/// pack: a 1x1 height level is two bytes and the next level up starts four
/// bytes on.
const fn alignment(format: PlaneFormat) -> usize {
    // Every texel size here is a power of two, so the least common multiple
    // with four is whichever of the two is larger.
    let texel = format.bytes_per_texel();
    if texel > 4 { texel } else { 4 }
}

/// The data format descriptor for one format, with its four-byte total length
/// in front of it.
///
/// This is the mandatory block, and the only part of the container that carries
/// meaning rather than layout: the transfer function here is what tells a
/// loader that the base colour is sRGB and the normal map is not. The rest is
/// the Khronos Data Format Specification's basic block, filled in for an
/// uncompressed single-plane format: one sample per channel, in ascending bit
/// order, each with the range its bits cover.
#[allow(
    clippy::cast_possible_truncation,
    reason = "descriptor field widths; the block is under a hundred bytes and the samples are \
              this module's own table"
)]
fn descriptor(format: PlaneFormat) -> Vec<u8> {
    /// The fixed part of a basic descriptor block, before its samples.
    const BLOCK_PREFIX: usize = 24;
    /// One sample descriptor.
    const SAMPLE: usize = 16;
    /// Channel ids in the RGBSDA colour model. Alpha is fifteen, not three.
    const R: u8 = 0;
    /// Green.
    const G: u8 = 1;
    /// Blue.
    const B: u8 = 2;
    /// Alpha.
    const A: u8 = 15;
    /// The sample is linear whatever the block's transfer function says. Alpha
    /// always is.
    const LINEAR: u8 = 1 << 0;
    /// The sample is signed, which for a float means it carries a sign bit.
    const SIGNED: u8 = 1 << 2;
    /// The sample is an IEEE 754 float.
    const FLOAT: u8 = 1 << 3;
    /// `KHR_DF_TRANSFER_LINEAR`.
    const TRANSFER_LINEAR: u8 = 1;
    /// `KHR_DF_TRANSFER_SRGB`.
    const TRANSFER_SRGB: u8 = 2;

    // Bit length, channel, qualifiers, and the values that map to zero and to
    // one. A float's bounds are the bit patterns of -1.0 and 1.0, which is how
    // the specification writes the range of a signed floating-point channel.
    let unorm8 = |channel: u8, qualifiers: u8| (8_u16, channel, qualifiers, 0_u32, 255_u32);
    let (transfer, samples) = match format {
        PlaneFormat::Rgba8Srgb => (
            TRANSFER_SRGB,
            vec![unorm8(R, 0), unorm8(G, 0), unorm8(B, 0), unorm8(A, LINEAR)],
        ),
        PlaneFormat::Rgba8Unorm => (
            TRANSFER_LINEAR,
            vec![unorm8(R, 0), unorm8(G, 0), unorm8(B, 0), unorm8(A, 0)],
        ),
        PlaneFormat::R16Unorm => (TRANSFER_LINEAR, vec![(16_u16, R, 0_u8, 0_u32, 65_535_u32)]),
        PlaneFormat::Rgba16Float => {
            let half = |channel: u8| {
                (
                    16_u16,
                    channel,
                    SIGNED | FLOAT,
                    (-1.0_f32).to_bits(),
                    1.0_f32.to_bits(),
                )
            };
            (TRANSFER_LINEAR, vec![half(R), half(G), half(B), half(A)])
        }
    };

    let block = BLOCK_PREFIX + samples.len() * SAMPLE;
    let mut bytes = vec![0_u8; 4 + block];
    let mut out = Writer::new(&mut bytes);
    out.u32(total(4 + block));
    // Vendor id zero is Khronos, descriptor type zero is the basic block, and
    // they share a word: seventeen bits and fifteen.
    out.u32(0);
    // Version two of the basic block, and the block's own size.
    out.u16(2);
    out.u16(block as u16);
    // Colour model RGBSDA, primaries BT.709, the transfer function, and the
    // flags, of which the only one is premultiplied alpha and ours is straight.
    out.u8(1);
    out.u8(1);
    out.u8(transfer);
    out.u8(0);
    // Texel block dimensions, stored one less than the actual size: a 1x1x1x1
    // block, which is what an uncompressed format has.
    out.u8(0);
    out.u8(0);
    out.u8(0);
    out.u8(0);
    // One plane, of the texel's own size; the other seven are zero.
    out.u8(format.bytes_per_texel() as u8);
    for _ in 1..8 {
        out.u8(0);
    }
    let mut offset = 0_u16;
    for (bits, channel, qualifiers, lower, upper) in samples {
        out.u16(offset);
        // Stored one less than the length, so a sample is never zero bits.
        out.u8((bits - 1) as u8);
        out.u8(channel | (qualifiers << 4));
        // The sample's position inside the texel block, which for an
        // uncompressed format is the origin in all four axes.
        for _ in 0..4 {
            out.u8(0);
        }
        out.u32(lower);
        out.u32(upper);
        offset += bits;
    }
    bytes
}

/// The key/value block: `KTXwriter`, and nothing else.
///
/// Each entry is its own length, then a NUL-terminated key, then the value,
/// then padding to the next four bytes. `KTXwriter`'s value is a
/// NUL-terminated string, so the NUL is part of it.
fn key_value() -> Vec<u8> {
    let key = b"KTXwriter\0";
    let value = format!("ashlar-material {}\0", env!("CARGO_PKG_VERSION"));
    let length = key.len() + value.len();
    let mut bytes = Vec::with_capacity(4 + length.next_multiple_of(4));
    bytes.extend_from_slice(&total(length).to_le_bytes());
    bytes.extend_from_slice(key);
    bytes.extend_from_slice(value.as_bytes());
    bytes.resize(4 + length.next_multiple_of(4), 0);
    bytes
}

/// A byte count this module computed, as the `u32` a header field holds.
///
/// Saturating is unreachable and deliberate all the same: every caller passes
/// the length of a descriptor or a key/value block, both of which this module
/// writes itself and both of which are under a kilobyte.
fn total(bytes: usize) -> u32 {
    u32::try_from(bytes).unwrap_or(u32::MAX)
}

/// Little-endian fields into a slice that is already the right size.
///
/// Every length here is computed before a byte is written, so a writer that
/// ran past its slice would be this module having miscounted; the index panics
/// on that rather than truncating a header into something a loader would read
/// as a different file.
struct Writer<'a> {
    bytes: &'a mut [u8],
    at: usize,
}

impl<'a> Writer<'a> {
    fn new(bytes: &'a mut [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn u8(&mut self, value: u8) {
        self.bytes[self.at] = value;
        self.at += 1;
    }

    fn u16(&mut self, value: u16) {
        self.bytes[self.at..self.at + 2].copy_from_slice(&value.to_le_bytes());
        self.at += 2;
    }

    fn u32(&mut self, value: u32) {
        self.bytes[self.at..self.at + 4].copy_from_slice(&value.to_le_bytes());
        self.at += 4;
    }

    fn u64(&mut self, value: u64) {
        self.bytes[self.at..self.at + 8].copy_from_slice(&value.to_le_bytes());
        self.at += 8;
    }
}

/// The same fields back out, over a slice whose length was checked first.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn u32(&mut self) -> u32 {
        let mut word = [0_u8; 4];
        word.copy_from_slice(&self.bytes[self.at..self.at + 4]);
        self.at += 4;
        u32::from_le_bytes(word)
    }

    fn u64(&mut self) -> u64 {
        let mut word = [0_u8; 8];
        word.copy_from_slice(&self.bytes[self.at..self.at + 8]);
        self.at += 8;
        u64::from_le_bytes(word)
    }
}
