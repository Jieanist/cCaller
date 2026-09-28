#!/usr/bin/env bash
# Build the libc_wrapper shared library used by libs.toml / cases.toml.
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/../.." && pwd)"

gcc --shared -fPIC -Wall -Wextra \
    -I "$ROOT/crates/ffi/include" \
    "$DIR/libc_wrapper.c" \
    -o "$DIR/libc_wrapper.so"

echo "built $DIR/libc_wrapper.so"
