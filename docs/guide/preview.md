# The preview

`ashlar-preview` is the authoring tool.
It draws any showcase scene, a material sheet or a recipe of your own,
bakes materials from their graphs as it goes, and lets you move a graph's parameters and watch the surface change.
It is a tool and links everything: the kernel, the graph engine and Bevy with every feature.
A game never needs it.

```sh
just preview --scene corporate-block
```

`just preview` builds it and passes the arguments through.
The first build compiles the Manifold kernel, so it takes a while; after that it starts in seconds.
Run it with `--help` for the full list; this chapter covers what you reach for.

## Moving around

Drag with the left button to orbit, with the right to pan, and scroll to zoom.
`Home` frames the model again, and `Escape` quits.

A few keys change what is drawn.
`F` toggles the wireframe and `P` toggles clay, which drops every material for a neutral grey,
so silhouettes and gaps read before surfaces distract.
`PageDown` and `PageUp` move a storey cut, hiding every storey above it,
and `I` hides the exterior, which is how we look into a building's rooms.

The camera orbits the middle of the model by default.
That may seem a strange default for a city, where the middle is half a tower up,
but it frames anything without configuration.
`--focus X,Y,Z` orbits a point in building space instead, and with it `--zoom` can go right down to street level:

```sh
just preview --scene metropolis --night --focus 30,2,10 --zoom 0.03 --pitch 0.1 --yaw 0.15
```

## Lighting

The default rig is daylight: a key at fourteen thousand lux and enough fill to read inside reveals.
`--night` swaps it for a dim, cold moon, a blue-black sky and bloom,
and stands a small light at every strongly emitting piece, so street lamps pool on the road and signs wash their wall.
`--key` and `--ambient` set the two numbers by hand.

By day the preview turns emission down to a quarter, so a lit window reads as a lit room rather than as neon in full sun.
That is a choice of the preview's rig, not of the materials; a game lights its own scene.

## Materials

Press `M` for the material panel, then click a surface to select the graph behind it.
Every parameter of that graph gets a slider.
A parameter a compiled `Surface::Shader` reads live moves under the pointer,
and anything else re-bakes when you let go.

`S` writes the graph library back to RON, over the file it came from or to `--graphs-out`.
The preview also watches that file, so editing the RON in an editor reloads it within half a second.
Keep in mind that the Rust builder is the source: copy the numbers you settle on back into it,
because the next content step bakes from Rust, not from the RON.

`sheet-<name>` and `detail-<name>` scenes exist for every surface of the default library.
A sheet is twelve specimens, two shapes at three distances, baked and live side by side;
a detail is a two-metre patch, close enough to see a joint or a blade of grass.

## Your own recipes

`--recipe wall.ron` draws a recipe of your own instead of a catalog scene,
and `--write-example out.ron` writes the current scene as editable RON to start from.
Without `--materials` a recipe draws every slot in its own diagnostic colour,
which is what we want while the shape is still moving.

## Damage

In a merged scene, such as `corporate-merged`, shift-clicking a surface blasts a hole in it.
`[` and `]` change the radius, and holding control reaches three times as far.
Whatever the blast leaves hanging falls, unless `--no-collapse` says otherwise.
[ADR 0005](../adr/0005-merged-geometry-and-damage.md) is the design behind it.

## Screenshots and galleries

`--screenshot out.png` renders one frame and exits, with `--yaw`, `--pitch` and `--zoom` for a reproducible view.
It is how every capture in this repository's history was made.

`just references` renders a windowless gallery of every scene, three views each, into `target/references`.
`--reference-scenes` narrows it, and `--reference-distance` and `--reference-view` set up close captures of a surface.

Screenshots and galleries work headless on a software renderer such as llvmpipe, just slowly.
A driver that crashes compiling shaders can be worked around with `--serial-pipelines`.
