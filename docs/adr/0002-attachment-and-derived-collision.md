# ADR 0002: Sockets attach, and collision is derived from geometry

Status: accepted, 2026-09-13.

## Context

Two things in the first contract were metadata only. A `Socket` recorded a
frame and nothing consumed it, so a wall of modules was placed by multiplying a
bay index by a bay width and rotating the result, once per kit, in the content.
Collision was worse: each showcase kit carried a hand-typed table of boxes
beside its geometry, matched to it by eye, and every dimension that moved had
to be retyped in two places or the proxies quietly drifted off the building.

Both are the same problem. The recipe already knows where the parts are and
what shape they have, and the content was restating it.

## Decision

### Attachment

An `Instance` may name its own socket, another instance, and that instance's
socket. `build()` derives the pose so the two socket frames meet, in dependency
order, and reports unknown sockets, unknown targets, self-attachment and cycles
with the path of the instance that asked. The derived pose is written into the
instance, so nothing downstream has to know an attachment happened.

**Sockets face outward along +Z.** Attaching therefore turns the attached part
half a turn about the socket's Y axis, so the two outward directions oppose: the
convention of a plug and a socket, not of two frames laid on top of each other.
`Facing::Aligned` asks for the other one, for a socket that marks a direction to
continue in rather than a face to meet.

The convention pays for itself where it is used. A bay with a `left` socket at
its origin facing -X and a `right` socket at its far edge facing +X, attached
left to right, lands the next bay exactly one bay along with the same rotation,
which is what the study kit's walls used to compute by hand.

### Derived collision

An `Element` declares `Collision::Bounds` or `Collision::Hull`, and the mesher
derives the proxy from the same evaluated solid it meshed, once per part
definition and a transform per placement. The default is `None`, so trim,
slats, lights and signage cost nothing and say so.

**A proxy is convex and conservative.** An opening subtracted inside an element
is not subtracted from its proxy. An opening that has to be walked through is
therefore the gap between elements, not a hole inside one. That is a real
constraint on how a passable kit is authored, and it is the price of deriving
collision from a boolean result without a decomposition step.

## Consequences

Content states dimensions once. A collider set cannot drift from the geometry,
because there is nothing left to drift from. The showcase's old tables survive
only as the fixture of the test that shows the derived set occupies what they
occupied.

The proxies are conservative, which suits static architecture and does not suit
anything that has to be walked into and out of through its own openings.
Convex decomposition, per-element materials for collision, and a proxy that
follows a cut are all still open. An attachment names an instance by ID, so
composing a building out of another building's instances, as the corporate
block does, has to rewrite attachment targets along with the IDs it renames.
