# Assets

Nothing in this directory is authored, and nothing baked is committed. Every
material is a Rust graph in `ashlar_material::stdlib` (plus the showcase's four
graphs in `examples/showcase/src/materials.rs`), and every scene bakes its
materials from those graphs when they are registered.

The content step writes the same library as files, for a build that wants to
load maps instead of baking them and for looking at maps flat:

```sh
just materials            # every material at its own resolution
cargo run --release -p ashlar-showcase --example export-materials -- assets 2048 brick png
```

That fills the ignored `materials/` directory here with KTX2 maps, strand sets,
a file-backed `library.materials.ron` and the `graphs.ron` they were baked from.
`just content` does the same and also bakes every showcase scene into the
ignored `buildings/` directory as `.ashlar` files, which the `ashlar-bevy`
`baked` example loads.
