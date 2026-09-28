/*
 * examples/gen/wrapper.c - macro-driven wrapper demo for `ccaller gen`.
 *
 * Every function is declared with CCALLER_FUNC instead of a hand-written
 * Call_* signature. The variadic tail is scanner metadata: `ident` is a
 * value parameter, `ident:write|read|read_write` marks a param_page slot
 * index. `ccaller gen wrapper.c` turns this file into libs.toml, so the
 * wrapper source stays the single source of truth.
 */

#include "ccaller_gen.h"

int64_t CCaller_abi_version(void) { return CCALLER_ABI_VERSION; }

/* Value parameters only; returns a + b. */
CCALLER_FUNC(add, a, b)
{
    (void)param_page;
    if (param_len != 2) return -1;
    return params[0] + params[1];
}

/* Value parameter only; echoes it back (handy for assertion tests). */
CCALLER_FUNC(ret, v)
{
    (void)param_page;
    if (param_len != 1) return -1;
    return params[0];
}

/* idx is a written slot index: param_page[idx] = v. */
CCALLER_FUNC(store, idx:write, v)
{
    if (param_len != 2) return -1;
    if (params[0] < 0 || params[0] >= CCALLER_PARAM_PAGE_SLOTS) return -2;
    param_page[(uint64_t)params[0]] = (uint64_t)params[1];
    return CCALLER_OK;
}

/* idx is a read slot index: returns param_page[idx]. */
CCALLER_FUNC(load, idx:read)
{
    if (param_len != 1) return -1;
    if (params[0] < 0 || params[0] >= CCALLER_PARAM_PAGE_SLOTS) return -2;
    return (int64_t)param_page[(uint64_t)params[0]];
}

/* idx is a read-write slot index: returns the old value, then bumps it. */
CCALLER_FUNC(bump, idx:read_write)
{
    if (param_len != 1) return -1;
    if (params[0] < 0 || params[0] >= CCALLER_PARAM_PAGE_SLOTS) return -2;
    return (int64_t)(param_page[(uint64_t)params[0]]++);
}

/* Returns the framework skip code; the Cmd is counted skipped, not failed. */
CCALLER_FUNC(skip)
{
    (void)param_page;
    return CCALLER_ERR_SKIP;
}

/* Crashes when flag != 0 (death-test demo); returns 0 otherwise. */
CCALLER_FUNC(boom, flag)
{
    (void)param_page;
    if (param_len != 1) return -1;
    if (params[0] != 0) {
        volatile int *p = (volatile int *)0;
        *p = 42; /* SIGSEGV */
    }
    return CCALLER_OK;
}
