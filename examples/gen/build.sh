#!/bin/sh
# Build the wrapper library for this example. Produces wrapper.so, which is
# the path `ccaller gen wrapper.c` writes into libs.toml.
set -e
cc -shared -fPIC -I ../../crates/ffi/include -o wrapper.so wrapper.c
