# mediation — task runner (https://just.systems)
#
# `just` with no arguments lists every recipe.

set shell := ["bash", "-uc"]

# MSRV — keep in sync with `rust-version` in Cargo.toml and rust-toolchain.toml.
msrv := "1.94"

# Zola version CI pins — keep in sync with `tool: zola@…` in the workflows.
#
# It is pinned because template validation has tightened between *patch*
# releases: 0.23.4 rejects a child template that 0.23.3 accepts. An older local
# build therefore passes a site CI will reject, which is the one failure mode a
# local check exists to prevent. Homebrew can lag; the release binaries at
# https://github.com/getzola/zola/releases are the reliable way to match.
zola_version := "0.23.4"

# 📋 List all recipes
default:
    @just --list

# ✅ Everything CI runs, in CI order
ci: fmt-check lint purity no-floats test doc site-check package
    @echo "✅ all checks passed"

# 🧊 Enforce the "zero I/O, no clock" guarantee over non-comment source lines
purity:
    #!/usr/bin/env bash
    set -uo pipefail
    hits="$(grep -rn --include='*.rs' -E \
        'now_utc|SystemTime::now|Instant::now|std::(fs|env|process)|std::net::(Tcp|Udp|SocketAddr|ToSocketAddrs)|\bunsafe\b' \
        src/ | grep -vE ':[[:space:]]*(///|//!|//)' || true)"
    if [ -n "$hits" ]; then
        echo "❌ ambient state or unsafe reached the source:" >&2
        echo "$hits" >&2
        echo "" >&2
        echo "This crate promises equal inputs give equal outputs. Take the" >&2
        echo "evaluation instant as a parameter instead." >&2
        exit 1
    fi
    echo "🧊 pure: no clock, no I/O, no unsafe"

# 🔢 Enforce that no float type reaches a metered quantity
no-floats:
    #!/usr/bin/env bash
    set -uo pipefail
    hits="$(grep -rn --include='*.rs' -E '\bf(32|64)\b' src/ \
        | grep -vE ':[[:space:]]*(///|//!|//)' || true)"
    if [ -n "$hits" ]; then
        echo "❌ a float reached the source:" >&2
        echo "$hits" >&2
        echo "" >&2
        echo "Volumes are integer octets and durations integer seconds." >&2
        echo "Floats belong only in ratios and scores — add an allowance here" >&2
        echo "deliberately if one is genuinely needed." >&2
        exit 1
    fi
    echo "🔢 exact: no floats in quantities"

# 🎨 Format the workspace
fmt:
    cargo fmt --all

# 🎨 Fail if anything is unformatted
fmt-check:
    cargo fmt --all -- --check

# 📎 Clippy with warnings denied (default + all features)
lint:
    cargo clippy --all-targets -- -D warnings
    cargo clippy --all-targets --all-features -- -D warnings

# 🔎 Fast type-check, all features
check:
    cargo check --all-targets --all-features

# 🧪 Full test suite (all features, incl. doctests)
test:
    cargo test --all-features

# 🧪 Run tests matching a filter, e.g. `just test-one luhn`
test-one filter:
    cargo test --all-features {{ filter }} -- --nocapture

# 🎛️ Build every feature combination
features:
    cargo build --no-default-features
    cargo build --no-default-features --features serde
    cargo build --all-features

# 🦀 Compile on the pinned MSRV
msrv:
    RUSTUP_TOOLCHAIN={{ msrv }} cargo check --all-features --all-targets

# 📚 Build the API docs with warnings denied
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features

# 📚 Build and open the docs
doc-open:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --open

# 🌐 Build the documentation site (needs `zola`: https://www.getzola.org)
site:
    cd site && zola build

# 🌐 Serve the site with live reload at http://127.0.0.1:1111
site-serve:
    cd site && zola serve --open

# 🔗 Check every internal link on the site, and every page for orphans
#
# The site's Rust examples are covered by `test`, because `src/lib.rs` includes
# every page as a doctest. Skipped with a note when `zola` is not installed, so
# a contributor working on the crate is not blocked by a tool they do not need;
# CI installs it and runs this for real.
site-check:
    #!/usr/bin/env bash
    set -uo pipefail
    if ! command -v zola >/dev/null 2>&1; then
        echo "⏭️  zola not installed — skipping the site link check"
        echo "   install it from https://www.getzola.org to run \`just site\`"
        exit 0
    fi
    have="$(zola --version | awk '{print $2}')"
    want="{{ zola_version }}"
    # A *lower* local version is the dangerous direction: it accepts templates
    # CI rejects, so the check passes here and fails there. Warn rather than
    # fail, because the link checking it still does is worth having.
    if [ "$have" != "$want" ] \
       && [ "$(printf '%s\n%s\n' "$want" "$have" | sort -V | head -1)" = "$have" ]; then
        echo "⚠️  zola $have is older than the $want that CI pins" >&2
        echo "   template validation has tightened between patch releases —" >&2
        echo "   this check can pass locally and still fail in CI." >&2
    fi
    cd site && zola check

# 📦 Dry-run the crates.io package (catches bad metadata before tagging)
# `--allow-dirty` is local-only convenience; CI runs this on a clean checkout.
package:
    cargo publish --dry-run --all-features --allow-dirty

# 🛡️ Audit dependencies for advisories (needs `cargo install cargo-audit`)
audit:
    cargo audit

# ⬆️ Show outdated dependencies (needs `cargo install cargo-outdated`)
outdated:
    cargo outdated --root-deps-only

# 🏷️ Tag the current Cargo.toml version — pushing the tag triggers release.yml
tag:
    #!/usr/bin/env bash
    set -euo pipefail
    version="$(cargo metadata --no-deps --format-version 1 \
        | grep -o '"version":"[^"]*"' | head -1 | cut -d'"' -f4)"
    if [ -n "$(git status --porcelain)" ]; then
        echo "❌ working tree is dirty — commit first" >&2
        exit 1
    fi
    just ci
    git tag -a "v${version}" -m "v${version}"
    echo "🏷️  tagged v${version} — push with: git push origin v${version}"

# 🧹 Remove build artifacts
clean:
    cargo clean
