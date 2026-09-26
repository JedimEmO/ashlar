# Walkthrough: the facade bay, then the corporate kit

Sources: `examples/showcase/src/lib.rs` and `examples/showcase/src/corporate.rs`.
Dimensions are quoted from the code; the reasons are those the code states or
those its geometry forces.

## The helpers

`block(id, size, at, bevel, slot)` builds an `Element` whose geometry is
`chamfered_cuboid(size, bevel)` when `bevel > 0.0` and `cuboid(size)`
otherwise, placed at `Pose::at(at)`. Every bevel in the kit is therefore an
explicit number per element, from 0.004 on sign strips to 0.1 on the plant
housing. `solid(...)` is `block(...)` with `Collision::Bounds`. The
distinction is the collision policy: structure, glazing and door leaves are
solid to walk into; trim, slats, lights and signage are not and carry no
proxy.

## The shared bay body: `panel(entrance)`

Both `study:facade` and `study:entrance` come from one function, so the two
parts differ only where an argument says so.

1. **Body.** `chamfered_cuboid([3.94, 4.74, 0.85], 0.035)` placed at
   `[0.03, 0.03, 0.0]`. The bay pitch is 4.0 and the body is 3.94 wide, so
   neighbouring bodies leave a 0.06 gap between chamfered edges rather than a
   coplanar seam. The exterior face is at Z=0 and the wall is 0.85 deep toward
   +Z.
2. **Opening.** One cutter `cuboid([width, height, 1.3])` at `[x, y, -0.2]`.
   For the facade that is 2.8 by 1.65 at `[0.6, 1.8]`; for the entrance 2.7 by
   3.45 at `[0.65, -0.1]`. The cutter runs from Z=-0.2 to Z=1.1 and the body
   from 0 to 0.85, so it passes both faces and makes an opening, not a recess.
   The entrance cutter also starts below Y=0, so the doorway is open at the
   floor.
3. **Fixing holes.** Four cutters `cylinder(0.045, 0.11, 12)` at X in
   {0.38, 3.62}, Y in {0.85, 3.85}, Z=-0.03, rotated `from_rotation_x(π/2)`.
   The rotation turns the cylinder's Y axis onto +Z, so each bore runs from
   Z=-0.03 to Z=0.08: it starts outside the face and stops 0.08 into the
   panel, a blind hole. Twelve segments is enough for a 9 cm hole.
4. **Sockets.** `left` at the origin rotated `from_rotation_y(-π/2)`, so its
   +Z points along -X; `right` at `[4.0, 0.0, 0.0]` rotated `+π/2`, +Z along
   +X. The comment says why: the next bay attaches to this one instead of
   being placed by multiplying its index. Attaching `left` to a neighbour's
   `right` lands one bay along with the same rotation, which the test
   `attached_bays_land_exactly_where_the_index_arithmetic_put_them` checks
   against the old arithmetic.
5. **Panel element, two ways.** The entrance keeps
   `Element::new("panel", body, "formed").cut_material("reveal")`: the mesher
   splits the element into an outer batch on `formed` and a cut batch on
   `reveal`, and the palette binds `library:formed-concrete` to both, so the
   split is there for a game that wants a second material on the reveal.
   The facade uses `Element::new("panel", body, "panel")` with no cut slot and
   binds `showcase:concrete-cut-aware`, a shader surface that reads the cut flag
   `ashlar-bevy` uploads per vertex. The comment explains the choice: a
   second slot is right when the reveal is a different material, and the
   material-side flag is right when it is the same concrete sawn through. The
   facade shows a hand's width of lit reveal on all four sides of its window,
   so it is the bay that demonstrates the flag; the entrance's reveal is hidden
   behind door leaves and canopy, so the slot lives there. Both carry
   `Collision::Bounds`, and because a proxy is conservative the window opening
   is not subtracted from the box, which is fine for a window and would be
   wrong for a doorway if the doorway were meant to be passable through this
   element.
6. **Coping.** `solid("top-course", [3.98, 0.25, 1.25], [0.01, 4.53, -0.25], 0.025, "wet")`.
   It overhangs the face by 0.25 and is 1.25 deep against a 0.85 body. Its
   slot is `wet`, bound to `showcase:concrete-wet`, because rain lands on faces
   that point up; the comment records that this is a slot until a world-normal
   input lets the material decide by itself.
7. **Pilasters.** Two `solid` blocks `[0.23, 4.4, 1.0]` at X 0.08 and 3.69,
   Z=-0.25, slot `concrete`. They stand proud of the face by 0.25 and stop at
   Y=4.5, under the coping.

## The facade's own elements: `facade()`

- **Window frame.** `chamfered_cuboid([3.04, 1.91, 0.16], 0.025)` at
  `[0.48, 1.67, -0.055]` minus `cuboid([2.75, 1.61, 0.4])` at
  `[0.625, 1.82, -0.15]`. The cutter passes through the 0.16 frame, leaving
  a rectangular ring 0.145 wide that sits 0.055 proud of the face around the
  2.8 by 1.65 opening. Slot `metal`, no collision: it is trim.
- **Recessed glass.** `solid("recessed-glass", [2.78, 1.64, 0.045], [0.61, 1.805, 0.69], 0.008, "glass")`.
  The pane is a separate element rather than a filled cutter, so it gets its
  own slot and its own `Bounds` proxy, and it sits at Z=0.69, deep inside the
  0.85 wall, so the reveal is visible. It is 0.01 narrower than the opening
  on each side.
- **Mullion.** `block("mullion", [0.065, 1.65, 0.67], [1.97, 1.8, 0.04], 0.01, "metal")`,
  spanning X 1.97 to 2.035 in the middle of the opening, no proxy.
- **Sill.** `block("sill", [3.06, 0.13, 0.46], [0.47, 1.62, -0.28], 0.025, "metal")`,
  projecting 0.28 outward just under the frame.
- **Plinth.** `solid("plinth", [3.95, 0.44, 0.99], [0.025, 0.025, -0.14], 0.035, "concrete")`,
  the one trim element that is walkable into, hence `solid`.
- **Vent.** `block("vent-recess", [1.68, 0.55, 0.035], [1.16, 0.76, -0.045], 0.01, "dark")`
  is a thin dark plate on the face, not a cut: the recess is read from the
  material, and six `vent-fin-{i}` blocks `[1.72, 0.045, 0.11]` stepped 0.1
  apart in Y at Z=-0.11 sit in front of it. They are six separate elements;
  an `arrayed` node inside one element would fuse them into one mesh on one
  slot, which is the cheaper choice for a new row of identical fins.

`Scene::Facade` builds `study:facade-specimen` with one instance `specimen`,
which is what `just preview --scene facade` shows.

## The entrance's own elements: `entrance()`

The lintel `[3.1, 0.35, 1.7]` at Z=-0.9 overhangs the doorway; a light
housing and a `light` strip hang under it. The threshold is a `solid` on
`wet` at Y=-0.02, the study's second compiled surface, with the comment that
a threshold is the one slab reliably wet. Two `jamb-{side}` solids
`[0.21, 3.35, 0.97]` line the 2.7 opening, and two door leaves
`[1.085, 3.15, 0.14]` at Z=0.69 are `solid` while their insets and stripes
are `block`. `sign(part)` adds six raised strips at Z=-0.085 for a "07" mark.
The panel's `Bounds` proxy fills the doorway and the leaves are `solid`, so
the entrance is closed in collision terms; the corporate study's in-world
test walked the ramp to each kit's closed entrance.

## Assembling the kit: `kit`, `walls` and `building`

`kit(id)` registers `facade()` and `entrance()`, binds the slots `concrete`,
`metal`, `glass`, `dark`, `paint`, `rust` and `rubble` to library materials
(`library:formed-concrete`, `library:steel` and so on) and `light` to
`showcase:light`, then `panel` to `showcase:concrete-cut-aware`, `reveal` and
`formed` to `library:formed-concrete` and `wet` to `showcase:concrete-wet`, and
adds the `study:parapet` and `study:plant` parts.

`walls(builder, bays, depth_bays, floors)` walks four sides per floor. Each
side has an origin and a yaw: front at the origin with 0, back at
`[width, 0, depth]` with π, left at `[0, 0, depth]` with π/2, right at
`[width, 0, 0]` with -π/2. Bay 0 of a side is placed with
`Pose::at(position).rotated(rotation)`; every later bay is
`attach("left", "<side>-<floor>-<bay-1>", "right")`. The front, ground-floor,
bay 1 is `study:entrance`; every other bay is `study:facade`. Instance
`front-0-0` overrides `panel` with `study:repair`. On the top floor a
`study:parapet` instance is placed per bay at 4.96 above the bay.

`building(scene)` picks `(bays, depth_bays, floors)` of `(3, 2, 1)` for
`Outpost` and `(4, 3, 3)` for `Office`, then adds a foundation slab at
Y=-0.32 and a roof slab at `floors * 4.8`, each
`[width + 0.65, 0.3, depth + 0.65]` at `[-0.325, 0, -0.325]`, a canopy at
`[3.7, 3.8, -2.2]` over the entrance, and one or two plant units on the roof.
The test `both_layouts_reuse_identical_facade_and_entrance_definitions`
asserts that the two scenes share byte-identical part definitions.

## The corporate kit: `corporate.rs`

`detail(id, size, at, slot)` derives the bevel as `min(size) * 0.18` capped
at 0.045, so the kit has no bevel arguments; `solid` again adds `Bounds`.
Storeys are 3.8 and the podium 6.45.

**Parts, thirteen registered; the tower uses 12 because nothing in it places
`metro:paving`.** `glazed_bay(podium)` makes `metro:podium`
(6.0 tall, opening 4.5 from Y=0.55) or `metro:curtain` (3.8 tall, opening
2.95 from Y=0.4): a `chamfered_cuboid([4.0, height, 0.8], 0.045)` minus a
`[3.16, opening, 1.3]` cutter at Z=-0.2, as element `frame` on `stone` with
`cut_material("reveal")` and `Bounds`; then `glass` as a `solid` at Z=0.64,
a mullion, a sill, two full-height fins on `spine` at Z=-0.3, and on the
curtain bay only an `occupancy-strip` on `light`. `lobby()` is an 8 by 6
portal with a 5.6 by 4.8 doorway, a glass wall, door frames, a threshold on
`reveal`, a canopy, and an `entrance` socket at `[4.0, 0.13, -0.6]` facing
-Z; nothing in the kit attaches to that socket. `service_riser()`, `identity()` and `metro:plant` are
detail stacks. Seven one-element parts come from a table: base, podium cap,
tower cap, core base and core crown with `Bounds`; `metro:rail` and
`metro:paving` with `Collision::None`, because a rail is walked past and a
slab walked over.

**Rings.** `ring(builder, level, origin, bays, podium)` places
`{level}-{side}-{bay}` instances around a rectangle exactly as `walls` does,
but every bay is placed by hand with `Pose::at(at).rotated(rotation)`; the
corporate kit uses no attachments. The podium ring is `[4, 3]` bays and skips
front bays 1 and 2, where the 8 m lobby stands at X=4. Upper rings are
`[3, 2]` bays inset to `[2, y, 2]`. A curtain bay where `(bay + level) % 3 == 0`
overrides `glass` with `showcase:occupied-glass`.

**One building.** `building(storeys)` refuses anything outside `1 ..= MAX_STOREYS` (forty) with
a `ValidationError` at path `corporate.storeys`. It lays the podium ring,
base, lobby at `[4, 0, 0]`, podium cap at Y=6.0, core base at `[13.35, 0, 5]`
and the identity sign, then one upper ring plus a `core-{floor}` riser per
storey at `PODIUM + floor * STOREY`, then tower cap, core crown, roof plant
and six rails at `roof + 0.5`. `Corporate` is `building(4)`;
`CorporateAnnex` is `building(1)`.

**Three buildings from one kit.** `block()` starts a fresh `kit("metro:city-block")`
and, for the tower at `[0, 0, 16]`, the annex at `[-4, 0, -4]` yawed -π/2
and the wing at `[32, 0, 12]` yawed +π/2, calls `building(floors)`, asks
the built result which instances resolve a `stone` binding, then takes the
recipe apart with `into_recipe()`: every instance id is prefixed `name/`,
every pose is composed with the building's placement, and every instance
that carries `stone` gets `binding("stone", Binding::new("showcase:corporate-stone").param("variation", Float(v)))`
with 0.37 for the annex and 0.71 for the wing. The tower keeps the library's
value so the `corporate` scene and the block show the same wall. The clad
check happens before the rewrite because an override naming a slot the part
does not declare is a validation error and most of the kit is trim. Paving
slabs are placed on an 8 m grid at indices -1 to 3 in X and -1 to 1 in Z.
The tests assert
that all three share identical parts and that exactly three distinct `stone`
bindings exist across the block.

The rewrite copies `pose` and never touches `attach`. That is safe only
because no corporate instance attaches; a kit that chains bays by socket
would have to rename each attachment's `target` alongside the ids.

## The dark city: `city/`

The city kit is the corporate kit's facade language taken tall and dark, in
four files under `examples/showcase/src/city/`.

**`storey.rs`: one storey, one part.** A `Plan` is a chamfered rectangle and a
lift core, the `Style` it is dressed in and its tier. `rect_storey(id, plan,
height, lobby)` builds the whole storey as one part: four wall runs and four
chamfer corners (each its own element and its own convex proxy), liners over
their inner faces, a slab, a floor finish, the core with its doors, and an
office's desks and screens. What the facade does is a `Facade` read from the
style and tier: wall depth, glass depth, sill and head, how many bays share an
opening (one for punched windows, two for pairs, zero for a ribbon the length
of the run), lights per bay, a splay on the jambs, tapered fins at the lines
between openings, blades down the bays' middles, a slab edge at the floor
line, and precast spandrels over and under the openings. Each opening is cut
through wall and liner alike, with a pane per bay deep in the reveal, a steel
frame and mullions standing on the glass, a sill a centimetre under it and a
strip light in its head. Per-bay features are authored once in the front run's
frame and turned onto each run by `Plan::side`, which works because every run's
bays are symmetric about its middle. The panes are split over three slots,
`pane-a` to `pane-c`, so an instance override can light a third of a storey at a
time. Each corner has a channel cut down its face with a light line in it, slot
`edge`. A lobby replaces the front run with piers, shop glass over a plinth, a
fascia with a neon line, a deep portal under a canopy, and a cornice round the
top. `rect_ceiling` and `rect_roof` finish a storey; a roof is also a terrace,
with a cornice, a coped parapet lit under its coping (slot `crown`) and
planters. A tower's top wears a `Crown` part per plan — a louvred penthouse,
stepped and lit, or lit blades round a plant room — which says how high its
deck is for the topper. Lights are `far` so they survive every level of
detail. Rings round a footprint (`Plan::ring`) are one outline less another,
because a union of runs and corners leaves slivers where they meet. The plans
are `TOWER`, `BANDED` and `CAGE` (one per style, each on the same 18 x 16 foot),
`SPIRE` (30 x 26, 22 x 20, 14 x 14) and `TENEMENT` (38 x 12).

**`metropolis.rs`: stacking and dressing.** `tiers(kind, style, storeys)`
splits a building's storeys over its style's plans, `levels` turns that into a
stack of lobby, floors and a roof per tier, and `tower` places the stack with
`attach_aligned` from each level's `bottom` socket to the one below's `top`, one
room and one ceiling per storey in that storey's merge group, then the crown on
the last roof and the topper on the crown's deck. `plan_lots` draws each quad
tower's style and crown from the seed, as a `Spec`. What varies per storey is
material, not geometry: `tower` puts `WINDOWS` overrides on the pane slots by
hash, turns off `DARK_STOREYS` of the storeys and their strips, and binds the
building's one neon colour to `crown` (and to `edge` on landmarks and a third of
towers). A thousand storeys of one part cost a handful of materials.

**Alleys.** Quad-lot towers stand flush with the lot's edges, which leaves a
six-metre alley east to west and a ten-metre one north to south. `alley_walls`
names each tower's two walls that face them, and `Wall::at` turns a wall-mounted
piece (authored X along the wall, -Z out of it) onto a wall at a distance along
it. Every style stands on the same foot with the same bays, so the dressing
fits any of them with a little help from the style: pipes go beside the first
fin, on a cage's first pier or at a banded ribbon's end, a fire escape down one
bay, air-conditioners under other bays' windows (none on a cage's floor-length
glass), blade signs and caged lamps on the fins or the wall, dumpsters at the
foot; everything but the gutters goes in its storey's group, so the storey
cut hides it with the storey.

**`alley.rs` and `street.rs`** hold the props. Everything that glows and must
read from afar is `far`; everything thin that need not is left to the level of
detail to drop.
