# Introduction

ashlar is a toolkit for procedural buildings and materials in Rust games.
We describe a building as ordinary Rust data, and its materials as graphs of tiled fields,
and a content step turns both into files a Bevy game loads.

An *ashlar* is a squared, dressed block of stone.
That is roughly what these crates hand you: blocks you assemble into whatever your game needs.

## The problem

A game with a city in it needs a lot of buildings,
and every one of them has to be modelled, textured, given collision, cut into levels of detail and exported.
Doing that by hand does not scale, and doing it with a node editor in a separate tool
means the buildings live somewhere our code cannot reach.

ashlar takes the other road.
A building is a `Building` value: parts made of solids, placed as instances, with material slots and sockets.
Because it is data, a loop places a hundred storeys and a seed lays out a city,
and the same recipe is validated, meshed, baked and shipped by code we can read and test.

## Two halves

Everything in ashlar belongs to one of two halves, and it helps to keep them apart from the start.

The *content step* is a small binary in your game's repository that runs when content changes.
It links the geometry kernel and the material graph engine, meshes every building at every level of detail,
bakes every material to KTX2 texture sets, and writes files.

The *game* links none of that.
It adds `AshlarPlugin`, spawns an `AshlarBuilding` with a building file and a material library,
and gets meshes, levels of detail, collision proxies and rooms back.
This may seem like a lot of ceremony for a game that could bake at startup, but the split is the point:
a shipped game has no C++ toolchain in its build and spends no frame time baking.

## Where to go from here

- [Getting started](guide/getting-started.md) takes you from a clone to a building of your own on screen.
- [Building recipes](guide/recipes.md) and [Materials](guide/materials.md) are the two authoring guides.
- [Shipping to a game](guide/integration.md) is the production path: the content step and the plugin.
- [A tour of the kits](guide/kits.md) walks the showcase's buildings, which are examples to copy, not a style you are stuck with.
- The [live demo](https://jedimemo.github.io/ashlar/demo/) runs the showcase in the browser,
  with a material gallery whose parameters you can move and watch re-bake.

The [API documentation](https://jedimemo.github.io/ashlar/api/ashlar/) has every type,
and [the decisions](adr/README.md) say why things are the way they are.
