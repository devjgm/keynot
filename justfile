# Recurring project tasks. Run `just` to list them.

default:
    @just --list

# Run the full test suite
test:
    cargo test

# Clippy (warnings denied) and formatting, as CI checks them
lint:
    cargo clippy --all-targets -- -D warnings
    cargo +nightly fmt --check

# Format the tree
fmt:
    cargo +nightly fmt

# Check the minimum supported Rust version still builds
msrv:
    cargo +1.90 check --locked

# Validate every .keynot file in the repo with `keynot check`
check-examples:
    find . -name '*.keynot' -not -path './target/*' -print0 | xargs -0 -n1 cargo run --quiet -- check

# Everything CI checks
ci: test lint msrv check-examples

# Regenerate the README screenshots from the tour (the README.md gallery updates by hand)
screenshots:
    cargo build
    cargo xtask screenshots 2 3 4

# Check all release prerequisites: local CI, fresh screenshots, a bumped version, green GitHub CI, a packageable crate
prerelease: ci screenshots
    #!/usr/bin/env bash
    set -euo pipefail
    # The screenshots recipe just regenerated the gallery; any diff
    # means the committed images were stale for this code.
    if [ -n "$(jj diff --name-only assets/screenshots 2>/dev/null || git status --porcelain assets/screenshots)" ]; then
        echo "screenshots were stale; regenerated ones are in the working copy -- review and commit them" >&2
        exit 1
    fi
    version=$(cargo pkgid | sed 's/.*[#@]//')
    # Catch a forgotten version bump before anything irreversible: the
    # version must not already be tagged nor already on crates.io.
    if git rev-parse -q --verify "refs/tags/v$version" >/dev/null \
        || [ -n "$(git ls-remote --tags origin "v$version")" ]; then
        echo "v$version is already tagged; bump the version in Cargo.toml first" >&2
        exit 1
    fi
    if curl -fsSL "https://index.crates.io/ke/yn/keynot" | grep -q "\"vers\":\"$version\""; then
        echo "keynot $version is already on crates.io; bump the version in Cargo.toml first" >&2
        exit 1
    fi
    # The release notes come from CHANGELOG.md, so the entry must exist
    # (rename the Unreleased section when cutting a release).
    if ! grep -q "^## \[$version\]" CHANGELOG.md; then
        echo "CHANGELOG.md has no entry for $version; add one first" >&2
        exit 1
    fi
    # Local checks (the `ci` dependency) only cover this machine; the
    # GitHub run for this exact commit also covers Windows and macOS.
    sha=$(git rev-parse HEAD)
    if [ "$(gh run list --commit "$sha" --json conclusion --jq 'length')" -eq 0 ]; then
        echo "no GitHub CI runs found for $sha; push it and let CI finish first" >&2
        exit 1
    fi
    running=$(gh run list --commit "$sha" --json status \
        --jq '[.[] | select(.status != "completed")] | length')
    if [ "$running" -ne 0 ]; then
        echo "GitHub CI is still running for $sha; try again when it finishes:" >&2
        gh run list --commit "$sha" >&2
        exit 1
    fi
    failed=$(gh run list --commit "$sha" --json conclusion \
        --jq '[.[] | select(.conclusion != "success")] | length')
    if [ "$failed" -ne 0 ]; then
        echo "GitHub CI failed for $sha:" >&2
        gh run list --commit "$sha" >&2
        exit 1
    fi
    # Package and compile exactly what crates.io would receive; catches
    # over-eager excludes and files that only exist in the repo.
    # (--allow-dirty so these checks are runnable mid-work; `release`
    # is what insists on a clean tree.)
    cargo publish --dry-run --allow-dirty
    echo "ready to release keynot $version"

# Publish to crates.io, tag vX.Y.Z, and create the GitHub release
[confirm("Publish to crates.io, push a tag, and create a GitHub release?")]
release: prerelease
    #!/usr/bin/env bash
    set -euo pipefail
    version=$(cargo pkgid | sed 's/.*[#@]//')
    if [ -n "$(jj diff --name-only 2>/dev/null || git status --porcelain)" ]; then
        echo "working copy is not clean; commit (and push) first" >&2
        exit 1
    fi
    echo "Releasing keynot $version"
    cargo publish
    git tag "v$version"
    git push origin "v$version"
    notes=$(awk "/^## \[$version\]/{found=1; next} /^## /{found=0} found" CHANGELOG.md)
    gh release create "v$version" --title "keynot $version" --notes "$notes"
