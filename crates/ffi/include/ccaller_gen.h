/*
 * ccaller_gen.h - macro-driven wrapper declarations for `ccaller gen`.
 *
 * A wrapper author declares every exported function with
 * CCALLER_FUNC instead of writing the fixed signature by hand:
 *
 *   #include "ccaller_gen.h"
 *
 *   CCALLER_FUNC(malloc, len, mem_idx:write)
 *   {
 *       ... the body, written against param_page / params / param_len ...
 *   }
 *
 *   CCALLER_FUNC(add, a, b)
 *   {
 *       ...
 *   }
 *
 *   CCALLER_FUNC(skip)
 *   {
 *       return CCALLER_ERR_SKIP;
 *   }
 *
 * Syntax of the macro arguments (what `ccaller gen` scans):
 *
 *   CCALLER_FUNC( name [, param]... )
 *
 *   name    A C identifier; the exported symbol is Call_<name>.
 *   param   A parameter of the unified signature, in call order.
 *           A plain identifier is a value parameter. An identifier with
 *           a `:role` suffix marks it as a param_page slot index:
 *
 *             ident          value parameter (passed through params[])
 *             ident:read     slot index the wrapper only reads
 *             ident:write    slot index the wrapper only writes
 *             ident:read_write
 *                            slot index the wrapper reads and writes
 *
 *   The variadic tail is metadata for the scanner only; the expansion
 *   ignores it, so the arguments never affect compilation.
 *
 * `ccaller gen <source.c>` scans these call sites (skipping comments,
 * #define lines, and conditional blocks of other platforms) and writes
 * a library description:
 *
 *   version = 1
 *
 *   [[libs]]
 *   path = "<source-stem>.so"        (".dll" on Windows)
 *
 *   funcs = [
 *     { name = "Call_malloc", paras = ["len", "mem_idx"],
 *       slot_roles = { mem_idx = "write" } },
 *     { name = "Call_add", paras = ["a", "b"] },
 *   ]
 *
 * The library path is the source file's stem with the platform
 * extension, matching the `init` scaffold's naming convention; the
 * description expects the built library next to the generated file.
 *
 * Precondition: the macro only declares; the body that follows must
 * obey the ccaller.h return-code namespace.
 */

#ifndef CCALLER_GEN_H
#define CCALLER_GEN_H

#include "ccaller.h"

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Declares one Call_<name> instance of the unified call signature
 * (requirement spec 7.1). The variadic arguments carry the parameter
 * names and slot roles for `ccaller gen`; they are consumed here and
 * do not appear in the expansion.
 */
#define CCALLER_FUNC(name, ...) \
    int64_t Call_##name(uint64_t *param_page, const int64_t *params, int64_t param_len)

#ifdef __cplusplus
}
#endif

#endif /* CCALLER_GEN_H */
