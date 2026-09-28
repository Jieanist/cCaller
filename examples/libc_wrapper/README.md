# libc_wrapper example

An end-to-end example wrapper for **cCaller**: a C dynamic library that
exposes a slice of the C standard library (`malloc`/`free`/`memcpy`/`memset`/
`memcmp`, 8/16/32/64-bit reads and writes, `strncpy`/`strcmp`/`strlen`/
`atoi`, plus slot and death-test helpers) through the unified ABI.

## Layout

| file            | purpose                                                     |
| --------------- | ----------------------------------------------------------- |
| `libc_wrapper.c`| the C wrapper (exports `CCaller_abi_version` + `Call_<name>`)|
| `libs.toml`     | library description (`[[libs]] path/funcs/slot_roles`)        |
| `cases.toml`    | test cases (`[[tests]] cmds`, env layers, shared inputs, …)   |
| `build.sh`      | compiles `libc_wrapper.c` into `libc_wrapper.so` (gcc only)   |

## Build the wrapper

From the repository root:

```sh
./examples/libc_wrapper/build.sh
# or, equivalently:
gcc --shared -fPIC -I crates/ffi/include \
    examples/libc_wrapper/libc_wrapper.c \
    -o examples/libc_wrapper/libc_wrapper.so
```

The build only needs `gcc`; it does not require `rustc`.

## Check the configuration (no commands executed)

```sh
ccaller --test examples/libc_wrapper/cases.toml \
        --lib examples/libc_wrapper/libs.toml \
        check
# expect: ok: N tests, M subcases, K commands  (0 findings)
```

## Run the cases

```sh
ccaller --test examples/libc_wrapper/cases.toml \
        --lib examples/libc_wrapper/libs.toml
# exit code 0 == all pass
```

## ABI notes

- The handshake symbol `CCaller_abi_version()` must return `1`
  (`CCALLER_ABI_VERSION`).
- Every `Call_<name>` has the signature
  `int64_t Call_<name>(uint64_t *param_page, const int64_t *params, int64_t param_len)`.
- Return values: `0` success, `[-127,-1]` wrapper custom failure, `-255` skip.
- String arguments arrive as NUL-terminated pointers valid for the duration
  of a single call; the wrapper must copy any bytes it keeps.
- `param_page` has 512 `uint64_t` slots (`CCALLER_PARAM_PAGE_SLOTS`); every
  index is bounds-checked before access.
