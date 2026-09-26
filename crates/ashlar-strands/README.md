# ashlar-strands

Strand geometry — grass, fur, moss, carpet pile — as a baked set a game reads
off disk, plants on its own triangles and turns into a mesh. No graph engine, no
Bevy, no IO.

A game rarely names this crate. The content step writes a material's
`set.strands` beside its maps (`ashlar-content`), and `ashlar-bevy`'s `strands`
feature reads it and grows the lawn. This crate is what both of those share.

A **strand** is one blade: a root in UV, a length and a width in metres, a
direction, a lean, a bend, two colours. A `StrandSet` is every strand of one
layer over one repeat of a material, sorted by rank. This crate holds the set,
the file it goes on disk in, and the three things that read one.

## Layering

```
ashlar-material        the scatter: a material graph read once per strand.
  <- ashlar-strands    <- you are here. glam, serde, thiserror.
                          The set, the file, place, mesh, cards.
  <- ashlar-bevy       *under `strands`*: chunks, levels of detail, wind,
                       and the impostor cards past the last of them.
```

The arrow points the way it does because of what is *not* here. A set is
scattered from a material graph — that needs a lowering, an interpreter and
rasterised planes, and it is `ashlar_material::strands::scatter`. Everything
after the scatter needs none of it: placement is a barycentric test, the mesh
builder is a quadratic Bézier, and a card atlas is a rasteriser over the same
set. So a game that ships baked lawns links this crate and not the graph engine,
and `ashlar-material` re-exports every name here from `ashlar_material::strands`
so nothing a caller already wrote has to move.

A scatter takes milliseconds, so why ship a file at all? Because the cost that
matters is not the scatter's time but the **dependency**: a game that scattered
its own lawn would compile the whole graph engine to do it. The 2026-09-20
amendment to
[ADR 0004](https://github.com/JedimEmO/ashlar/blob/main/docs/adr/0004-strand-layers.md)
has the reasoning.

## The three readers of a set

| Call | What it answers | Where it is used |
| --- | --- | --- |
| `place` | The roots of a set on a mesh's triangles, with a tangent frame each. | Every chunk of every level of detail. |
| `mesh` | The ribbons or tubes those roots grow into. | The same. |
| `cards` | The set drawn *side on* into an atlas. | The impostor level past the last band of real strands. |

A level of detail is a `StrandSet::prefix` — a cut on the rank — so the
survivors of a tighter level are a subset of a looser one's and nothing
reshuffles at a switch. That is the invariant `StrandSet::new` establishes by
sorting, and it holds for a set from a scatter, a set read off disk and a set
built by hand alike.

Note what `cards` does **not** need: a graph. The atlas an impostor wears is a
pure function of the set, so a game that holds the file draws its own — there is
no second file to ship and no second thing to keep in step.

## The file

`file::write` takes every layer one material definition grows and answers the
bytes; `file::read` takes them back, refusing anything it cannot account for
rather than panicking on it; `file::inspect` reads the header and the directory
alone, which is what a startup preflight can afford.

```
identifier  12 bytes, shaped the way KTX2's is
version     u32, and a file of another one is refused by number
directory   one fixed entry per layer: name, count, lattice, shape, offset
payload     per layer, field-major: every root, then every rank, then …
```

Little-endian, lossless and stored. `read(write(set)) == set`, bit for bit,
which is what lets a test assert that a lawn read off disk and a lawn scattered
from its graph mesh to the same triangles.

**Field-major, not strand-major**, for two reasons: a reader that wants one
field reads one contiguous run, and the values of one field are alike, so
whatever compresses the file afterwards has that repetition in front of it
rather than interleaved with twenty-two other distributions.

**Stored, and measured.** A strand is 23 `f32` — 92 bytes — so a set is big:
`library:grass` is about a quarter of a million strands over its three layers.
The header carries a compression scheme and the only value written is zero.
Compressing the payload would pull a decoder into every game that draws
grass, and the measured saving did not pay for it.

## What it costs to build

`glam`, `serde` and `thiserror`, and nothing else. `serde` is here for one type:
`StrandProfile` is a field of a strand layer and so part of a material graph's
RON, and the type belongs beside the geometry that reads it. Every game linking
`ashlar` already pulls the same crate.

## Licence

MIT OR Apache-2.0, as the rest of the workspace.
