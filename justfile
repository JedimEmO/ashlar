# ashlar developer commands. `just --list` shows this.

[doc('The same as `just check`.')]
default: check

# Type-check every member, including tests, examples and benches.
check:
    cargo check --workspace --all-targets

# Run the whole test suite.
test:
    cargo test --workspace

# Clippy with the workspace lint set, warnings fatal.
lint:
    cargo clippy --workspace --all-targets -- -D warnings

[doc('Format the workspace.')]
fmt:
    cargo fmt --all

[doc('Check formatting without changing anything.')]
fmt-check:
    cargo fmt --all -- --check

# Rustdoc for the workspace, with broken intra-doc links fatal.
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

# Licences, advisories, banned crates and sources. Needs `cargo install
# cargo-deny`. Part of `ci` so an advisory shows up on the branch that
# introduced it rather than months later.
[doc('Licences, advisories, banned crates and sources (needs cargo-deny).')]
deny:
    cargo deny check

# The configurations a game compiles, one at a time: the workspace build unifies
# every feature, so without this nothing checks that the game path builds and
# documents without the tool features. `ashlar-bevy` with no features is the
# game path; `strands` is the one game feature; `ashlar-material` without
# `zstd` is a crate that bakes in memory; `integration-game` is the template.
[doc('Check and document each configuration a game compiles, one at a time.')]
configs:
    cargo check -p ashlar-bevy --all-targets
    cargo check -p ashlar-bevy --features strands --all-targets
    cargo check -p ashlar-material --all-targets
    cargo check -p ashlar-content --all-targets
    cargo check -p integration-game --all-targets
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps -p ashlar-bevy
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps -p ashlar-bevy --features strands
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps -p ashlar-material
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps -p ashlar-content

# What CI runs.
ci: fmt-check lint test configs doc deny

# GPU-only checks, outside `ci`: compare emitted WGSL against the CPU
# interpreter for every primitive IR op, the shipped graphs with live
# parameters, and the bit-exact lattice hash. Bake comparisons also check
# every map and mip, and verify byte-exact CPU fallback for graphs exceeding
# the binding budget. Run after changes to either interpreter or emitter.
# `--nocapture` prints the worst difference per case.
#
# The non-ignored tests in this target run in CI: operation coverage, binding
# fallbacks and other invariants that need no adapter.
#
# Each GPU case opens an adapter. Run serially to avoid driver crashes during
# concurrent adapter initialization and to bound device memory use.
# The named features are required by the test target.
[doc('GPU: hold the WGSL backend to the CPU interpreter, texel for texel.')]
conformance *args:
    cargo test -p ashlar-bevy --features gpu-bake,shader --test conformance -- --ignored --nocapture --test-threads 1 {{args}}

# GPU-only render checks: draw fixed studios offscreen and assert on pixels.
# This catches missing vertex attributes, texture-loading failures and shader
# assembly problems that compute conformance alone cannot see.
#
# Three configurations: the default baked-files game path, compiled materials
# with `shader`, and baked lawns with only the `strands` game feature. Testing
# them separately keeps tool features from masking a broken game build.
# Cases run serially for the adapter-initialization reason above. The framing
# test needs no GPU and runs in normal CI.
[doc('GPU: render fixed studios offscreen and assert on the pixels.')]
draw *args:
    cargo test -p ashlar-bevy --test draw -- --ignored --nocapture --test-threads 1 {{args}}
    cargo test -p ashlar-bevy --features shader --test draw -- --ignored --nocapture --test-threads 1 {{args}}
    cargo test -p ashlar-bevy --features strands --test draw -- --ignored --nocapture --test-threads 1 {{args}}

# The content step: bake every showcase scene to assets/buildings/<scene>.ashlar
# at every level of detail, and export the material library beside them under
# assets/materials/ (both ignored by git). `just content 512 metropolis` bakes
# at a lower resolution and one scene. The ashlar-bevy `baked` example loads it.
[doc('Bake every showcase scene and the material library into assets/.')]
content *args:
    cargo run --release -p ashlar-showcase --example content -- assets {{args}}

# The material half of `just content`: the showcase's library alone, through
# the same `ashlar-content` step, under assets/materials/ (ignored by git).
# Nothing needs it to run — every scene bakes its materials from the graphs —
# it is for review: `just materials 2048 brick png` narrows it to one material
# and writes level-0 PNGs beside the KTX2.
[doc("The material half of `just content`, for review: `just materials 2048 brick png`.")]
materials resolution="" name="" png="":
    cargo run --release -p ashlar-showcase --example export-materials -- assets {{resolution}} {{name}} {{png}}

# The showcase preview in a window. `just preview --scene corporate`.
# One codegen job: the Manifold C++ build is what dominates a cold compile.
[doc('The showcase preview in a window, e.g. `just preview --scene corporate`.')]
preview *args:
    CARGO_BUILD_JOBS=1 cargo run -p ashlar-preview -- {{args}}

# The whole showcase gallery, headless, into target/references/, then open its
# index.html. Build and run are separate so the capture gets four-core affinity
# and reduced priority: one GPU process compiling shaders serially is what it
# wants, not the whole machine.
[doc('Render the headless reference gallery into target/references/.')]
references *args:
    CARGO_BUILD_JOBS=1 cargo build -p ashlar-preview
    taskset -c 0-3 nice -n 10 ./target/debug/ashlar-preview \
        --references target/references {{args}}

# The project site into target/site: the web demo (examples/web) with its
# content step's files, the manual from mdBook and the API docs. Needs `trunk`,
# `mdbook` and the wasm32-unknown-unknown target. Everything it writes is under
# target/; the GitHub Pages workflow runs this same recipe.
[doc('Build the project site into target/site: web demo, manual and API docs.')]
site:
    examples/web/site/build.sh

# Serve target/site locally, as GitHub Pages would: the demo is at /demo/.
[doc('Serve target/site locally; the demo is at /demo/.')]
site-serve port="8080":
    python3 -m http.server --directory target/site {{port}}

# The web demo natively, in a window, over the files `just site` staged.
# `just web --scene city-alley --night`.
[doc('The web demo natively, over the files `just site` staged.')]
web *args:
    cargo run -j 4 -p ashlar-web -- {{args}}
