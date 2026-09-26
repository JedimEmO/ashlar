//! The baked strand set: every layer a definition grows, in one file.
//!
//! ADR 0004 said there would be no such thing — "generating a set is
//! milliseconds, so there is no strand file format and no serialized
//! `StrandSet`" — and the milliseconds part of that is still true and measured.
//! What changed is the premise underneath it. A set is scattered from a
//! material graph, so a game that scattered its own lawn would link the graph
//! engine to do it, and the decision of 2026-09-20 is that a game links baked
//! files and nothing else. This file is what a lawn's baked form looks like.
//!
//! [`write()`] takes the sets a definition grows and answers the bytes.
//! [`read()`] takes the bytes and answers the sets, refusing anything it cannot
//! account for rather than panicking on it. [`inspect`] reads the header and
//! the directory alone, which is what a preflight wants: what the file claims
//! to hold, checked against its own length, without decoding a strand.
//!
//! ```
//! use ashlar_strands::{Strand, StrandSet, StrandShape, file};
//!
//! let set = StrandSet::new(
//!     "blades".to_owned(),
//!     vec![Strand { root: [0.25, 0.75], rank: 0.5, length: 0.1, width: 0.003, ..Default::default() }],
//!     [64, 64],
//!     StrandShape::default(),
//! );
//! let bytes = file::write(std::slice::from_ref(&set))?;
//!
//! // The shallow read, which is what a startup preflight makes.
//! let info = file::inspect(&bytes)?;
//! assert_eq!(info.version, file::VERSION);
//! assert_eq!(info.layers[0].layer, "blades");
//! assert_eq!(info.layers[0].strands, 1);
//!
//! // And the whole thing, which is bit for bit what went in.
//! assert_eq!(file::read(&bytes)?, vec![set]);
//! # Ok::<(), file::StrandFileError>(())
//! ```
//!
//! # What the format commits to
//!
//! - **One file per definition, every layer it grows.** A lawn is `blades`,
//!   `fibres` and `stragglers` together and is meaningless as one of the three;
//!   the layers of one definition are grown together, shipped together and
//!   shared with nothing else, exactly as the four KTX2 maps of one bake are.
//!   So a definition names one path rather than three, a preflight opens one
//!   file, and a reader that wants one layer still finds it by name in the
//!   directory without decoding the others.
//! - **Little-endian, everywhere.** The same choice
//!   [`ktx2`](../../ashlar_material/ktx2/index.html) makes, and for its reason:
//!   every target this ships on is little-endian, and a byte order mark that
//!   nothing ever reads the other way round is a branch per field for nothing.
//! - **Field-major, not strand-major.** A layer's payload is twenty-three
//!   arrays — every root, then every rank, then every phase — rather than a
//!   record per strand. Two things come of that. A reader that wants one field,
//!   a debug dump of the roots or a spatial index, reads one contiguous run;
//!   and the values of one field are *alike*, so whatever compresses the file
//!   afterwards has the repetition in front of it rather than interleaved with
//!   twenty-two other distributions. [`FLOATS_PER_STRAND`] is the number of
//!   arrays, and a test holds it against `size_of::<Strand>()`.
//! - **Lossless.** Every field is the `f32` the scatter computed, stored by its
//!   bits. `read(write(set))` is `set`, and that is the whole reason a test can
//!   assert that a baked lawn and a scattered one mesh to the same triangles.
//!   It is also what the file costs: a strand is ninety-two bytes and a repeat
//!   of `library:grass` is a quarter of a million of them.
//! - **Stored, and the header says so.** [`Compression::Stored`] is the only
//!   scheme [`write()`] produces, and a file claiming another number is refused
//!   *by that number* rather than read as garbage. The field is here because
//!   the alternative — adding one later — is a second version of the format;
//!   the measurement behind leaving it at zero is in the crate's README.
//!
//! # What the reader refuses
//!
//! Everything it cannot account for, and it returns rather than panics.
//! The identifier, the version, every offset and length against the file's own
//! size, the layer count, each name's bytes as UTF-8, and then the strands
//! themselves: every float finite, every rank inside `0..=1`, and every length,
//! width and sink not negative.
//!
//! Those three range checks and no more, and the line is drawn where the
//! consequence is. A `NaN` in a position is not a bad blade, it is a mesh with
//! no coordinates and so a whole chunk that fails its frustum test and
//! disappears; a rank outside `0..=1` breaks the one invariant
//! [`StrandSet::prefix`] is a prefix *because of*; a negative length turns a
//! curve inside out. Everything else a strand carries — a taper past one, a
//! colour over white — is already clamped by the geometry that reads it, and a
//! reader that refused those would be refusing sets the scatter is entitled to
//! produce.

use crate::{FLOATS_PER_STRAND, MAX_SEGMENTS, Strand, StrandProfile, StrandSet, StrandShape};

/// The twelve bytes every baked strand set starts with.
///
/// Shaped the way KTX2's own identifier is, and for its three reasons: the high
/// byte catches a transfer that stripped the eighth bit, the readable middle
/// says what the file is to anybody who opens it in a pager, and the trailing
/// `\r\n\x1A\n` catches a transfer that translated line endings or truncated at
/// an end-of-file character.
pub const IDENTIFIER: [u8; 12] = [
    0xAB, b'A', b'S', b'H', b'S', b'T', b'R', b'D', 0x0D, 0x0A, 0x1A, 0x0A,
];

/// The format version this crate writes and the only one it reads.
///
/// A file of another version is refused by number rather than guessed at. The
/// version moves when the *meaning* of the bytes does — a field added to
/// [`Strand`], a layout changed — and a test on `size_of::<Strand>()` is what
/// says the first of those happened.
pub const VERSION: u32 = 1;

/// The fixed header, before the layer directory.
///
/// Fifty-six rather than the fifty-two the fields come to: the identifier, four
/// `u32` and three `u64` leave the header four bytes short of an eight-byte
/// multiple, and a header that ends on one is a directory that starts on one.
/// The four bytes are written as zero and are not read.
pub const HEADER_BYTES: usize = 56;

/// One directory entry: everything about a layer except its strands.
const ENTRY_BYTES: usize = 48;

/// The most layers one file may hold.
///
/// A definition names the layers it grows and the densest thing in this
/// workspace names three. The bound is here so that a header claiming four
/// billion of them is refused before a directory that size is allocated, which
/// is the one thing a length field at the front of a file can be used for.
pub const MAX_LAYERS: usize = 64;

/// The longest a layer's name may be, in bytes.
///
/// A layer name is a key in a material graph, which validation already holds to
/// an identifier. This is the same guard [`MAX_LAYERS`] is: a length read out
/// of a file decides an allocation, so it is bounded before it is believed.
pub const MAX_NAME_BYTES: usize = 256;

/// The coarsest lattice a layer may claim to have been scattered on, per axis.
///
/// `ashlar-material`'s own `MAX_PERIOD`, which is what a strand layer's `count`
/// is validated against when the graph declaring it is read. Written out rather
/// than imported for this crate's usual reason — the graph engine is not in a
/// game's tree — and the two are one number.
pub const MAX_LATTICE: u32 = 4096;

/// How a layer's payload is stored.
///
/// Non-exhaustive because the point of the field is that a scheme can be added
/// to it: a match on this must carry a wildcard, so that the day one is added
/// is not the day every caller stops compiling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Compression {
    /// The floats as they are. Scheme 0, and the only one [`write()`] produces.
    #[default]
    Stored,
}

impl Compression {
    /// The scheme number a header records for this.
    const fn scheme(self) -> u32 {
        match self {
            Self::Stored => 0,
        }
    }

    /// The scheme a number names, or nothing for one this crate does not read.
    const fn of(scheme: u32) -> Option<Self> {
        match scheme {
            0 => Some(Self::Stored),
            _ => None,
        }
    }
}

/// Why a baked strand set could not be written, or is not one.
///
/// Non-exhaustive as [`Ktx2Error`](../../ashlar_material/ktx2/enum.Ktx2Error.html)
/// is, and for its reason: a reason to refuse added later must not break a
/// caller's match.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum StrandFileError {
    /// The bytes do not begin with [`IDENTIFIER`].
    #[error("not a baked strand set: it does not begin with the ashlar strand identifier")]
    Identifier,
    /// A version this crate does not read.
    #[error("strand set version {found}, and this build reads version {VERSION}")]
    Version {
        /// What the file claims.
        found: u32,
    },
    /// A supercompression scheme this crate does not read.
    #[error("strand set compression scheme {0}, and this build reads scheme 0 (stored) only")]
    Compression(u32),
    /// The file ends before something its own header or directory named.
    #[error("truncated strand set: {needed} bytes are named and the file is {found}")]
    Truncated {
        /// The end of the furthest thing the header or the directory named.
        needed: usize,
        /// The length of the file.
        found: usize,
    },
    /// More layers than [`MAX_LAYERS`], or none at all.
    #[error("strand set holds {found} layers, and a file holds 1..={MAX_LAYERS}")]
    Layers {
        /// How many the header claims.
        found: usize,
    },
    /// A layer name that is empty, too long, or not UTF-8.
    #[error("layer {index}: {reason}")]
    Name {
        /// Which layer, in directory order.
        index: usize,
        /// What is wrong with it.
        reason: String,
    },
    /// A shape constant outside what a layer may declare.
    #[error("layer {layer:?}: {reason}")]
    Shape {
        /// The layer that carries it.
        layer: String,
        /// What is wrong with it.
        reason: String,
    },
    /// A strand carrying a value no scatter produces.
    ///
    /// The three that matter, and the module docs say why these three: a
    /// non-finite number takes a whole chunk out of the frustum test, a rank
    /// outside `0..=1` breaks the prefix a level of detail cuts on, and a
    /// negative size turns a curve inside out.
    #[error("layer {layer:?}, strand {strand}: {reason}")]
    Strand {
        /// The layer it is in.
        layer: String,
        /// Which strand of that layer, in rank order.
        strand: usize,
        /// What is wrong with it.
        reason: String,
    },
    /// The directory's own numbers do not add up.
    #[error("strand set directory: {0}")]
    Directory(String),
}

/// What one layer of a file claims about itself.
///
/// Everything [`read()`] would answer except the strands, which is exactly what a
/// preflight can afford to look at.
#[derive(Clone, Debug, PartialEq)]
pub struct StrandLayerInfo {
    /// The layer's name, as the graph declared it.
    pub layer: String,
    /// How many strands it holds.
    pub strands: u32,
    /// The lattice they were scattered on, per axis.
    pub count: [u32; 2],
    /// The shape constants its layer declared.
    pub shape: StrandShape,
}

/// What a baked strand set's header and directory say about it.
///
/// The file's own claims, checked against the file's length and nothing else.
#[derive(Clone, Debug, PartialEq)]
pub struct StrandFileInfo {
    /// The format version it declares, which [`inspect`] has already held to
    /// [`VERSION`].
    pub version: u32,
    /// How its payload is stored.
    pub compression: Compression,
    /// One entry per layer, in the order the file holds them.
    pub layers: Vec<StrandLayerInfo>,
}

impl StrandFileInfo {
    /// How many strands the whole file holds.
    pub fn strands(&self) -> u64 {
        self.layers
            .iter()
            .map(|layer| u64::from(layer.strands))
            .sum()
    }

    /// Whether the file holds a layer of this name.
    pub fn holds(&self, layer: &str) -> bool {
        self.layers.iter().any(|entry| entry.layer == layer)
    }
}

/// Write every layer of one definition as one file.
///
/// The sets are written in the order they are handed over, which is the order a
/// definition names its layers in: a file is a picture of one definition's
/// strand settings, and keeping the order means a reader hands them back in it.
///
/// # Errors
///
/// If a set carries something [`read()`] would refuse — a non-finite field, a
/// rank outside `0..=1`, a negative size, a shape constant outside its range, a
/// name that is empty or too long — or if there are no sets at all or more than
/// [`MAX_LAYERS`]. The writer refuses exactly what the reader refuses, which is
/// what makes the round trip total in both directions rather than only one.
pub fn write(sets: &[StrandSet]) -> Result<Vec<u8>, StrandFileError> {
    if sets.is_empty() || sets.len() > MAX_LAYERS {
        return Err(StrandFileError::Layers { found: sets.len() });
    }
    for set in sets {
        check_set(set)?;
    }

    // The layout, front to back: header, directory, names, payload. Every
    // offset is computed here and written once, so a reader checking them
    // against the file's length is checking arithmetic this function did
    // rather than a claim it took on trust.
    let directory_at = HEADER_BYTES;
    let names_at = directory_at + sets.len() * ENTRY_BYTES;
    let mut names = Vec::new();
    let mut name_spans = Vec::with_capacity(sets.len());
    for set in sets {
        let at = names_at + names.len();
        names.extend_from_slice(set.layer().as_bytes());
        name_spans.push((at, set.layer().len()));
    }
    // The payload begins on a sixteen-byte boundary, which is the alignment a
    // reader would want if it ever mapped the file instead of copying it. The
    // padding is at most fifteen bytes over a file that is megabytes.
    let payload_at = (names_at + names.len()).next_multiple_of(16);

    let mut payload_spans = Vec::with_capacity(sets.len());
    let mut cursor = payload_at;
    for set in sets {
        let bytes = set.len() * FLOATS_PER_STRAND * size_of::<f32>();
        payload_spans.push(cursor);
        cursor += bytes;
    }
    let payload_bytes = cursor - payload_at;

    let mut file = vec![0_u8; cursor];
    file[..IDENTIFIER.len()].copy_from_slice(&IDENTIFIER);
    let mut header = Writer::new(&mut file[IDENTIFIER.len()..HEADER_BYTES]);
    header.u32(VERSION);
    header.u32(Compression::Stored.scheme());
    header.u32(count(sets.len()));
    header.u32(count(sets.iter().map(StrandSet::len).sum::<usize>()));
    header.u64(directory_at as u64);
    header.u64(payload_at as u64);
    header.u64(payload_bytes as u64);

    let mut directory = Writer::new(&mut file[directory_at..names_at]);
    for (index, set) in sets.iter().enumerate() {
        let shape = set.shape();
        directory.u32(count(name_spans[index].0));
        directory.u32(count(name_spans[index].1));
        directory.u32(count(set.len()));
        directory.u32(set.count()[0]);
        directory.u32(set.count()[1]);
        directory.u32(profile_code(shape.profile));
        directory.u32(shape.segments);
        directory.f32(shape.taper);
        directory.f32(shape.root_occlusion);
        directory.f32(shape.midpoint);
        directory.u64(payload_spans[index] as u64);
    }
    file[names_at..names_at + names.len()].copy_from_slice(&names);

    for (index, set) in sets.iter().enumerate() {
        let at = payload_spans[index];
        let bytes = set.len() * FLOATS_PER_STRAND * size_of::<f32>();
        let mut lanes = Writer::new(&mut file[at..at + bytes]);
        // Field-major: one whole array per field, in the order `fields` names
        // them, which is the order the reader walks. See the module docs.
        for field in 0..FLOATS_PER_STRAND {
            for strand in set.strands() {
                lanes.f32(fields(strand)[field]);
            }
        }
    }
    Ok(file)
}

/// Read a baked strand set's header and directory, checking both against the
/// file's own length.
///
/// What a startup preflight makes: it says what the file claims to hold and
/// whether those claims fit inside the bytes, at the cost of the header and a
/// few dozen bytes per layer rather than the cost of a quarter of a million
/// strands. A file that passes here can still fail [`read()`], and only on the
/// strands themselves.
///
/// # Errors
///
/// If the bytes are not a baked strand set, are a version or a compression
/// scheme this build does not read, or name anything outside themselves.
pub fn inspect(bytes: &[u8]) -> Result<StrandFileInfo, StrandFileError> {
    Ok(opened(bytes)?.0)
}

/// Read every layer of a baked strand set.
///
/// The answer is in the file's own order, which is the order the definition
/// that wrote it names its layers in.
///
/// # Errors
///
/// Everything [`inspect`] refuses, and then the strands: a non-finite field, a
/// rank outside `0..=1`, or a negative length, width or sink.
pub fn read(bytes: &[u8]) -> Result<Vec<StrandSet>, StrandFileError> {
    let (info, spans) = opened(bytes)?;
    let mut sets = Vec::with_capacity(info.layers.len());
    for (entry, at) in info.layers.iter().zip(spans) {
        let strands = entry.strands as usize;
        let mut built = vec![Strand::default(); strands];
        let mut lanes =
            Reader::new(&bytes[at..at + strands * FLOATS_PER_STRAND * size_of::<f32>()]);
        for field in 0..FLOATS_PER_STRAND {
            for strand in &mut built {
                *fields_mut(strand)[field] = lanes.f32();
            }
        }
        for (index, strand) in built.iter().enumerate() {
            check_strand(strand, &entry.layer, index)?;
        }
        // `new` sorts, and over a file written by `write` that is a no-op: the
        // set that went in was in rank order and a stable sort over a sorted
        // slice moves nothing. It is here because a *hand-edited* file must
        // still answer a set a level of detail can cut, which is what
        // `StrandSet::new` is the one place to guarantee.
        sets.push(StrandSet::new(
            entry.layer.clone(),
            built,
            entry.count,
            entry.shape,
        ));
    }
    Ok(sets)
}

/// The header and directory, with each layer's payload offset beside it.
///
/// What [`inspect`] and [`read()`] share: the second half of the answer is the
/// thing only `read` needs, so the checks are made once and in one order.
fn opened(bytes: &[u8]) -> Result<(StrandFileInfo, Vec<usize>), StrandFileError> {
    // The identifier on however many bytes there are, so that a file which is
    // something else entirely is reported as not a strand set rather than as a
    // short one.
    let head = IDENTIFIER.len().min(bytes.len());
    if bytes[..head] != IDENTIFIER[..head] {
        return Err(StrandFileError::Identifier);
    }
    fits(HEADER_BYTES, bytes.len())?;
    let mut header = Reader::new(&bytes[IDENTIFIER.len()..HEADER_BYTES]);
    let version = header.u32();
    if version != VERSION {
        return Err(StrandFileError::Version { found: version });
    }
    let scheme = header.u32();
    let Some(compression) = Compression::of(scheme) else {
        return Err(StrandFileError::Compression(scheme));
    };
    let layers = header.u32() as usize;
    let declared_strands = u64::from(header.u32());
    let directory_at = offset(header.u64());
    let payload_at = offset(header.u64());
    let payload_bytes = offset(header.u64());
    if layers == 0 || layers > MAX_LAYERS {
        return Err(StrandFileError::Layers { found: layers });
    }
    let directory_end = directory_at.saturating_add(layers * ENTRY_BYTES);
    fits(directory_end, bytes.len())?;
    let payload_end = payload_at.saturating_add(payload_bytes);
    fits(payload_end, bytes.len())?;

    let mut entries = Vec::with_capacity(layers);
    let mut spans = Vec::with_capacity(layers);
    let mut total = 0_u64;
    let mut directory = Reader::new(&bytes[directory_at..directory_end]);
    for index in 0..layers {
        let name_at = directory.u32() as usize;
        let name_bytes = directory.u32() as usize;
        let strands = directory.u32();
        let count = [directory.u32(), directory.u32()];
        let profile = directory.u32();
        let segments = directory.u32();
        let taper = directory.f32();
        let root_occlusion = directory.f32();
        let midpoint = directory.f32();
        let at = offset(directory.u64());

        let name_end = name_at.saturating_add(name_bytes);
        fits(name_end, bytes.len())?;
        if name_bytes == 0 || name_bytes > MAX_NAME_BYTES {
            return Err(StrandFileError::Name {
                index,
                reason: format!("a layer name is 1..={MAX_NAME_BYTES} bytes, not {name_bytes}"),
            });
        }
        let layer = std::str::from_utf8(&bytes[name_at..name_end])
            .map_err(|error| StrandFileError::Name {
                index,
                reason: format!("a layer name is UTF-8: {error}"),
            })?
            .to_owned();

        let Some(profile) = profile_of(profile) else {
            return Err(StrandFileError::Shape {
                layer,
                reason: format!("profile {profile}, and a strand is a blade (0) or a fibre (1)"),
            });
        };
        let shape = StrandShape {
            profile,
            segments,
            taper,
            root_occlusion,
            midpoint,
        };
        check_shape(&layer, count, shape)?;

        // Where this layer's floats are, held against the payload the header
        // declared rather than only against the file: a directory entry
        // pointing at the *name block* would otherwise read names as strands.
        let span = (strands as usize).saturating_mul(FLOATS_PER_STRAND * size_of::<f32>());
        let end = at.saturating_add(span);
        if at < payload_at || end > payload_end {
            return Err(StrandFileError::Directory(format!(
                "layer {layer:?} claims bytes {at}..{end}, which is not inside the \
                 {payload_at}..{payload_end} the header declared for strands"
            )));
        }
        total += u64::from(strands);
        spans.push(at);
        entries.push(StrandLayerInfo {
            layer,
            strands,
            count,
            shape,
        });
    }
    if total != declared_strands {
        return Err(StrandFileError::Directory(format!(
            "the header declares {declared_strands} strands and the layers come to {total}"
        )));
    }
    Ok((
        StrandFileInfo {
            version,
            compression,
            layers: entries,
        },
        spans,
    ))
}

/// Whether a file that named `needed` bytes is long enough to hold them.
fn fits(needed: usize, found: usize) -> Result<(), StrandFileError> {
    if needed > found {
        return Err(StrandFileError::Truncated { needed, found });
    }
    Ok(())
}

/// One set held to everything [`read()`] would hold it to.
///
/// The writer's half of the symmetry: it refuses what the reader refuses, so a
/// file this crate wrote always reads back and a set this crate read always
/// writes back.
fn check_set(set: &StrandSet) -> Result<(), StrandFileError> {
    let layer = set.layer();
    if layer.is_empty() || layer.len() > MAX_NAME_BYTES {
        return Err(StrandFileError::Name {
            index: 0,
            reason: format!(
                "a layer name is 1..={MAX_NAME_BYTES} bytes, and {layer:?} is {}",
                layer.len()
            ),
        });
    }
    check_shape(layer, set.count(), set.shape())?;
    for (index, strand) in set.strands().iter().enumerate() {
        check_strand(strand, layer, index)?;
    }
    Ok(())
}

/// One layer's shape constants and lattice, held to the ranges a graph is held
/// to when it declares them.
fn check_shape(layer: &str, count: [u32; 2], shape: StrandShape) -> Result<(), StrandFileError> {
    let refuse = |reason: String| {
        Err(StrandFileError::Shape {
            layer: layer.to_owned(),
            reason,
        })
    };
    if shape.segments == 0 || shape.segments > MAX_SEGMENTS {
        return refuse(format!(
            "{} segments, and a strand is built from 1..={MAX_SEGMENTS}",
            shape.segments
        ));
    }
    for (what, value) in [
        ("taper", shape.taper),
        ("root_occlusion", shape.root_occlusion),
        ("midpoint", shape.midpoint),
    ] {
        if !value.is_finite() {
            return refuse(format!("{what} is {value}, which is not a number"));
        }
    }
    for (axis, value) in count.iter().enumerate() {
        if *value > MAX_LATTICE {
            return refuse(format!(
                "a lattice of {value} cells on axis {axis}, and a repeat carries at most \
                 {MAX_LATTICE}"
            ));
        }
    }
    Ok(())
}

/// One strand, held to the three things whose failure is not a bad blade but a
/// broken chunk. The module docs say where the line is and why.
fn check_strand(strand: &Strand, layer: &str, index: usize) -> Result<(), StrandFileError> {
    let refuse = |reason: String| {
        Err(StrandFileError::Strand {
            layer: layer.to_owned(),
            strand: index,
            reason,
        })
    };
    for (field, value) in fields(strand).into_iter().enumerate() {
        if !value.is_finite() {
            return refuse(format!(
                "field {field} is {value}; a strand with a non-finite number in it is not a bad \
                 blade, it is a chunk with no coordinates, which fails its frustum test and takes \
                 every blade around it out of the frame"
            ));
        }
    }
    if !(0.0..=1.0).contains(&strand.rank) {
        return refuse(format!(
            "a rank of {}, and a rank is in 0..=1 because a level of detail is a prefix of the \
             set sorted by it",
            strand.rank
        ));
    }
    for (what, value) in [
        ("length", strand.length),
        ("width", strand.width),
        ("height_offset", strand.height_offset),
    ] {
        if value < 0.0 {
            return refuse(format!(
                "a {what} of {value}, which turns the curve inside out"
            ));
        }
    }
    Ok(())
}

/// One strand as the floats the payload holds, in the order it holds them.
///
/// The order is the declaration order of [`Strand`], which is what makes the
/// pairing with [`fields_mut`] readable: the two are the same list written
/// twice, and a field added to one without the other is a compile error at the
/// array's length rather than a lawn read back shifted by one.
fn fields(strand: &Strand) -> [f32; FLOATS_PER_STRAND] {
    [
        strand.root[0],
        strand.root[1],
        strand.rank,
        strand.phase,
        strand.length,
        strand.width,
        strand.direction[0],
        strand.direction[1],
        strand.lean,
        strand.bend,
        strand.root_color[0],
        strand.root_color[1],
        strand.root_color[2],
        strand.tip_color[0],
        strand.tip_color[1],
        strand.tip_color[2],
        strand.roughness,
        strand.facing,
        strand.height_offset,
        strand.clump_id,
        strand.clump_distance,
        strand.clump_pull[0],
        strand.clump_pull[1],
    ]
}

/// The same list, as the places a read writes into.
fn fields_mut(strand: &mut Strand) -> [&mut f32; FLOATS_PER_STRAND] {
    let [root_u, root_v] = &mut strand.root;
    let [direction_u, direction_v] = &mut strand.direction;
    let [root_r, root_g, root_b] = &mut strand.root_color;
    let [tip_r, tip_g, tip_b] = &mut strand.tip_color;
    let [pull_u, pull_v] = &mut strand.clump_pull;
    [
        root_u,
        root_v,
        &mut strand.rank,
        &mut strand.phase,
        &mut strand.length,
        &mut strand.width,
        direction_u,
        direction_v,
        &mut strand.lean,
        &mut strand.bend,
        root_r,
        root_g,
        root_b,
        tip_r,
        tip_g,
        tip_b,
        &mut strand.roughness,
        &mut strand.facing,
        &mut strand.height_offset,
        &mut strand.clump_id,
        &mut strand.clump_distance,
        pull_u,
        pull_v,
    ]
}

/// The number a profile is stored as.
const fn profile_code(profile: StrandProfile) -> u32 {
    match profile {
        StrandProfile::Blade => 0,
        StrandProfile::Fibre => 1,
    }
}

/// The profile a number names, or nothing for a number this crate does not
/// write.
const fn profile_of(code: u32) -> Option<StrandProfile> {
    match code {
        0 => Some(StrandProfile::Blade),
        1 => Some(StrandProfile::Fibre),
        _ => None,
    }
}

/// A count this module computed, as the `u32` a header field holds.
///
/// Saturating is unreachable and deliberate all the same: every caller passes a
/// layer count bounded by [`MAX_LAYERS`], a name length bounded by
/// [`MAX_NAME_BYTES`], or a strand count bounded by the lattice a scatter runs
/// over.
fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// A `u64` offset or length a file declared, as the `usize` a slice is indexed
/// by.
///
/// Saturating rather than failing, exactly as the KTX2 reader does it: on a
/// 64-bit target the conversion is the identity, and on a 32-bit one a file
/// claiming an offset past four gigabytes is a file whose bounds check is about
/// to fail anyway, which is where it should be reported rather than here.
fn offset(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// Little-endian fields into a slice that is already the right size.
///
/// The KTX2 writer's own helper, because the two files are laid out the same
/// way: every length is computed before a byte is written, so a writer that ran
/// past its slice would be this module having miscounted, and the index panics
/// on that rather than truncating a header into something a reader would take
/// for a different file.
struct Writer<'a> {
    bytes: &'a mut [u8],
    at: usize,
}

impl<'a> Writer<'a> {
    fn new(bytes: &'a mut [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn u32(&mut self, value: u32) {
        self.bytes[self.at..self.at + 4].copy_from_slice(&value.to_le_bytes());
        self.at += 4;
    }

    fn f32(&mut self, value: f32) {
        self.u32(value.to_bits());
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

    fn f32(&mut self) -> f32 {
        f32::from_bits(self.u32())
    }

    fn u64(&mut self) -> u64 {
        let mut word = [0_u8; 8];
        word.copy_from_slice(&self.bytes[self.at..self.at + 8]);
        self.at += 8;
        u64::from_le_bytes(word)
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        reason = "fixtures built in the test, where a failure is the test failing"
    )]
    #![allow(
        clippy::float_cmp,
        reason = "the whole claim of this format is that a float goes in and the \
                  same bits come out; a tolerance here would pass a writer that \
                  had quietly started rounding"
    )]
    use super::*;

    /// A set of `count` strands whose every field is a different number, so
    /// that a field written into the wrong lane is a difference rather than a
    /// coincidence.
    fn set(layer: &str, strands: u32, profile: StrandProfile) -> StrandSet {
        let built = (0..strands)
            .map(|index| {
                let t = f32::from(u16::try_from(index).unwrap_or(0)) / f32::from(u16::MAX);
                Strand {
                    root: [t, 1.0 - t],
                    rank: t,
                    phase: (t * 7.0).fract(),
                    length: 0.05 + t * 0.1,
                    width: 0.001 + t * 0.003,
                    direction: [t.cos(), t.sin()],
                    lean: t * 0.5,
                    bend: t * 0.25,
                    root_color: [t * 0.1, t * 0.2, t * 0.05],
                    tip_color: [t * 0.4, t * 0.5, t * 0.1],
                    roughness: 0.9 - t * 0.1,
                    facing: (t - 0.5) * 1.5,
                    height_offset: t * 0.01,
                    clump_id: (t * 3.0).fract(),
                    clump_distance: (t * 5.0).fract(),
                    clump_pull: [t * 0.01, -t * 0.01],
                }
            })
            .collect();
        StrandSet::new(
            layer.to_owned(),
            built,
            [64, 32],
            StrandShape {
                profile,
                segments: 4,
                taper: 0.8,
                root_occlusion: 0.3,
                midpoint: 0.25,
            },
        )
    }

    #[test]
    fn a_set_written_and_read_back_is_the_set_that_went_in() {
        // The claim the whole file exists for, and the one the parity test in
        // `ashlar-material` leans on: bit for bit, field for field, layer for
        // layer, in order.
        let sets = vec![
            set("blades", 500, StrandProfile::Blade),
            set("fibres", 1_200, StrandProfile::Fibre),
            set("stragglers", 7, StrandProfile::Blade),
        ];
        let bytes = write(&sets).unwrap();
        assert_eq!(read(&bytes).unwrap(), sets);
        // And writing what was read is the same file, which is the other half
        // of the round trip: a reader that dropped a field would still pass the
        // line above if the writer dropped it too.
        assert_eq!(write(&read(&bytes).unwrap()).unwrap(), bytes);
    }

    #[test]
    fn the_shallow_read_says_what_the_deep_one_would_without_a_strand_in_it() {
        let sets = vec![
            set("blades", 64, StrandProfile::Blade),
            set("shoots", 31, StrandProfile::Fibre),
        ];
        let bytes = write(&sets).unwrap();
        let info = inspect(&bytes).unwrap();
        assert_eq!(info.version, VERSION);
        assert_eq!(info.compression, Compression::Stored);
        assert_eq!(info.strands(), 95);
        assert!(info.holds("shoots") && !info.holds("blade"));
        for (entry, set) in info.layers.iter().zip(&sets) {
            assert_eq!(entry.layer, set.layer());
            assert_eq!(entry.strands as usize, set.len());
            assert_eq!(entry.count, set.count());
            assert_eq!(entry.shape, set.shape());
        }
    }

    #[test]
    fn an_empty_layer_is_a_layer_and_writes_a_file_with_no_strands_in_it() {
        // A density field that kept nothing is a real answer — a patch of bare
        // earth — and a file that refused it would make the content step fail
        // on the one material that meant it.
        let empty = StrandSet::new(
            "blades".to_owned(),
            Vec::new(),
            [16, 16],
            StrandShape::default(),
        );
        let bytes = write(std::slice::from_ref(&empty)).unwrap();
        assert_eq!(read(&bytes).unwrap(), vec![empty]);
        assert_eq!(inspect(&bytes).unwrap().strands(), 0);
    }

    #[test]
    fn a_file_that_is_not_one_is_refused_by_what_is_wrong_with_it() {
        assert_eq!(
            read(b"not a strand set at all"),
            Err(StrandFileError::Identifier)
        );
        // The identifier alone, and nothing after it. An empty file answers the
        // same way rather than `Identifier`, which is the KTX2 reader's own
        // behaviour: nothing disagrees with the first zero bytes of anything,
        // so what is wrong with an empty file is its length.
        for short in [&IDENTIFIER[..], &[][..]] {
            assert!(
                matches!(read(short), Err(StrandFileError::Truncated { .. })),
                "{} bytes were read as a strand set",
                short.len()
            );
        }
    }

    #[test]
    fn a_version_this_build_does_not_read_is_refused_by_number() {
        // The point of a version: a file from a future format says so and is
        // named, rather than being read as this one and drawing wrong.
        let mut bytes = write(&[set("blades", 4, StrandProfile::Blade)]).unwrap();
        bytes[12..16].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert_eq!(
            read(&bytes),
            Err(StrandFileError::Version { found: VERSION + 1 })
        );
        // And a compression scheme nothing here can inflate.
        let mut bytes = write(&[set("blades", 4, StrandProfile::Blade)]).unwrap();
        bytes[16..20].copy_from_slice(&2_u32.to_le_bytes());
        assert_eq!(read(&bytes), Err(StrandFileError::Compression(2)));
    }

    #[test]
    fn a_file_truncated_after_its_header_is_refused_rather_than_read_short() {
        let bytes = write(&[set("blades", 400, StrandProfile::Blade)]).unwrap();
        for cut in [HEADER_BYTES, HEADER_BYTES + 8, bytes.len() - 1] {
            assert!(
                matches!(read(&bytes[..cut]), Err(StrandFileError::Truncated { .. })),
                "a file cut at {cut} was read anyway"
            );
        }
    }

    #[test]
    fn a_directory_pointing_outside_the_payload_is_refused() {
        // The failure a bounds check on the file alone would miss: an offset
        // that is inside the bytes but inside the *names* rather than the
        // strands, which would read a layer name as a field of floats.
        let bytes = write(&[set("blades", 4, StrandProfile::Blade)]).unwrap();
        let mut broken = bytes.clone();
        // The payload offset is the last `u64` of the single directory entry.
        let at = HEADER_BYTES + ENTRY_BYTES - 8;
        broken[at..at + 8].copy_from_slice(&0_u64.to_le_bytes());
        assert!(matches!(read(&broken), Err(StrandFileError::Directory(_))));
        // And a strand count the payload cannot hold.
        let mut broken = bytes;
        broken[HEADER_BYTES + 8..HEADER_BYTES + 12].copy_from_slice(&9_999_u32.to_le_bytes());
        assert!(matches!(
            read(&broken),
            Err(StrandFileError::Directory(_) | StrandFileError::Truncated { .. })
        ));
    }

    #[test]
    fn a_strand_a_scatter_could_not_have_produced_is_refused_at_its_own_index() {
        // The three the module docs draw the line at, each through the writer,
        // which refuses exactly what the reader does.
        let broken = |mutate: fn(&mut Strand)| {
            let mut strands = vec![Strand::default(); 3];
            mutate(&mut strands[1]);
            write(&[StrandSet::new(
                "blades".to_owned(),
                strands,
                [8, 8],
                StrandShape::default(),
            )])
        };
        for mutate in [
            (|strand: &mut Strand| strand.length = f32::NAN) as fn(&mut Strand),
            |strand: &mut Strand| strand.root[1] = f32::INFINITY,
            |strand: &mut Strand| strand.rank = 1.5,
            |strand: &mut Strand| strand.width = -0.001,
        ] {
            assert!(
                matches!(broken(mutate), Err(StrandFileError::Strand { .. })),
                "a strand no scatter produces was written anyway"
            );
        }
    }

    #[test]
    fn a_shape_outside_what_a_layer_may_declare_is_refused() {
        let with = |shape: StrandShape, count: [u32; 2]| {
            write(&[StrandSet::new(
                "blades".to_owned(),
                vec![Strand::default()],
                count,
                shape,
            )])
        };
        let shape = |segments: u32| StrandShape {
            segments,
            ..Default::default()
        };
        assert!(matches!(
            with(shape(0), [8, 8]),
            Err(StrandFileError::Shape { .. })
        ));
        assert!(matches!(
            with(shape(MAX_SEGMENTS + 1), [8, 8]),
            Err(StrandFileError::Shape { .. })
        ));
        assert!(matches!(
            with(shape(3), [MAX_LATTICE + 1, 8]),
            Err(StrandFileError::Shape { .. })
        ));
        assert!(with(shape(MAX_SEGMENTS), [MAX_LATTICE, MAX_LATTICE]).is_ok());
    }

    #[test]
    fn a_file_holds_at_least_one_layer_and_at_most_the_bound() {
        assert_eq!(write(&[]), Err(StrandFileError::Layers { found: 0 }));
        let many: Vec<StrandSet> = (0..=MAX_LAYERS)
            .map(|index| {
                StrandSet::new(
                    format!("layer{index}"),
                    Vec::new(),
                    [8, 8],
                    StrandShape::default(),
                )
            })
            .collect();
        assert_eq!(
            write(&many),
            Err(StrandFileError::Layers {
                found: MAX_LAYERS + 1
            })
        );
        assert!(write(&many[..MAX_LAYERS]).is_ok());
    }

    #[test]
    fn the_payload_is_field_major_so_one_field_is_one_run_of_bytes() {
        // The layout claim, checked rather than asserted in prose: the first
        // array of the payload is every strand's root `u` and nothing else.
        let sets = vec![set("blades", 6, StrandProfile::Blade)];
        let bytes = write(&sets).unwrap();
        // The payload offset: the identifier, four `u32` and one `u64` in.
        let at = offset(u64::from_le_bytes(
            bytes[36..44].try_into().expect("the payload offset"),
        ));
        for (index, strand) in sets[0].strands().iter().enumerate() {
            let word = at + index * 4;
            let found = f32::from_le_bytes(bytes[word..word + 4].try_into().expect("a float"));
            assert_eq!(found, strand.root[0], "strand {index}'s root u");
        }
    }
}
