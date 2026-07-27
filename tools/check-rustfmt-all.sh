#!/usr/bin/env bash
set -euo pipefail

# `cargo fmt` only follows Rust modules that rustfmt can discover through the
# normal module tree. This repository also composes large modules from
# `include!("...")` fragments and contains macro-body sources that cargo fmt
# can silently skip, so check every Rust source file directly as well.
mapfile -d '' rust_sources < <(find crates -type f -name '*.rs' -print0 | sort -z)

if ((${#rust_sources[@]} == 0)); then
    echo "no Rust source files found under crates/" >&2
    exit 1
fi

rustfmt --edition 2024 --check "${rust_sources[@]}"
