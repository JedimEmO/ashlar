#!/usr/bin/env bash
# Build the project site into target/site. `just site` runs this, and so does
# the GitHub Pages workflow, so the two cannot drift apart.
#
#   target/site/index.html   the landing page (examples/web/site/index.html)
#   target/site/demo/        the web demo: trunk's build and the content step's files
#   target/site/book/        the manual, from mdBook
#   target/site/api/         rustdoc for the published crates
#
# Nothing here writes into the repository. The content step stages its files
# under target/web/assets, trunk builds into target/web/dist, and both are
# copied into place. CARGO_BUILD_JOBS bounds every cargo invocation; it
# defaults to four.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$root"

jobs="${CARGO_BUILD_JOBS:-4}"
stage="$root/target/web"
site="$root/target/site"

step() { printf '\n== %s\n' "$*"; }

step "content: the demo's scenes and materials, into $stage/assets"
cargo build -j "$jobs" --release -p ashlar-web --features content --example content
rm -rf "$stage/assets"
"$root/target/release/examples/content" "$stage/assets"

step "app: the demo compiled to WebAssembly by trunk, into $stage/dist"
# The cargo profile is `web-release`, which index.html names and
# examples/web/.cargo/config.toml defines. `CARGO_BUILD_JOBS` reaches cargo
# through the environment, and `BINARYEN_CORES` bounds wasm-opt, which would
# otherwise take every core for several minutes.
# Trunk reads NO_COLOR as a boolean and refuses the common `NO_COLOR=1`.
if [ -n "${NO_COLOR:-}" ]; then
    export NO_COLOR=true
fi
(cd examples/web && CARGO_BUILD_JOBS="$jobs" BINARYEN_CORES="$jobs" trunk build --release)

rm -rf "$site"
mkdir -p "$site"

step "manual"
# The manual's mdBook lives in docs/. A tree without one yet still builds a
# site, with a page in the manual's place saying so, so the demo can be
# deployed before the book exists.
book=""
for candidate in docs docs/book; do
    if [ -f "$candidate/book.toml" ]; then
        book="$candidate"
        break
    fi
done
if [ -n "$book" ]; then
    mdbook build "$book" --dest-dir "$site/book"
else
    echo "no book.toml under docs/ or docs/book/; the manual is a placeholder" >&2
    mkdir -p "$site/book"
    cat > "$site/book/index.html" <<'HTML'
<!doctype html><meta charset="utf-8"><title>ashlar manual</title>
<p>The manual is not built yet. Read the guides in the
<a href="https://github.com/JedimEmO/ashlar/tree/main/docs/guide">repository</a>.</p>
HTML
fi

step "API docs"
# The published crates, each with the features docs.rs documents it with.
# `--no-deps` keeps Bevy's own documentation out of a site about ashlar.
cargo doc -j "$jobs" --no-deps \
    -p ashlar -p ashlar-surface -p ashlar-manifold -p ashlar-material \
    -p ashlar-content -p ashlar-strands -p ashlar-bevy \
    --features ashlar-bevy/gpu-bake,ashlar-bevy/shader,ashlar-bevy/strand-scatter,ashlar-material/zstd
mkdir -p "$site/api"
cp -r "$root/target/doc/." "$site/api/"

step "demo and landing page"
mkdir -p "$site/demo"
cp -r "$stage/dist/." "$site/demo/"
cp -r "$stage/assets" "$site/demo/assets"
cp "$root/examples/web/site/index.html" "$site/index.html"
# GitHub Pages runs Jekyll over an upload unless told not to, and Jekyll
# drops every path starting with an underscore — rustdoc has some.
touch "$site/.nojekyll"

step "done"
du -sh "$site"/* | sed 's/^/  /'
echo "  serve it with: just site-serve"
