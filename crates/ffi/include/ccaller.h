/*
 * ccaller.h - wrapper-side ABI contract of the cCaller test framework.
 *
 * A wrapper is a dynamic library, written against the SDK under test,
 * that exposes the SDK through the unified call signature documented
 * below. This header is the single, self-contained place where the
 * contract constants live (requirement spec 7.1; features F-A-04 and
 * F-A-05). The Rust side mirrors every constant defined here, and a
 * unit test in the ccaller-ffi crate fails the build when the two
 * drift apart (style guide 8.4).
 *
 * Return-code namespace (normative, requirement spec 7.1):
 *
 *   value          owner       meaning
 *   -------------  ---------   ---------------------------------------
 *   0              framework   success (CCALLER_OK)
 *   [-127, -1]     wrapper     custom failure; the framework treats the
 *                              call as failed and passes the number
 *                              through verbatim, without interpreting it
 *   [-255, -128]   framework   reserved; wrappers must never return a
 *                              value from this range; the only constant
 *                              defined here is CCALLER_ERR_SKIP (-255)
 *   > 0            reserved    undefined in this version; treated as a
 *                              failure and passed through
 */

#ifndef CCALLER_H
#define CCALLER_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * ABI version for the load-time handshake (FR-A-04). Every wrapper
 * must export CCaller_abi_version() returning exactly this value; the
 * framework refuses to load a library that reports a different
 * version and prints both numbers. Bumped on every breaking ABI
 * change.
 *
 * Precondition: none. param_page: neither read nor written.
 * Return value: the version constant; never an error code.
 */
#define CCALLER_ABI_VERSION 1

/*
 * Success (FR-A-05). Whatever the wrapper wrote to param_page stays
 * visible to later Cmds on the same thread (FR-A-06, decision Q-06).
 *
 * Precondition: none. Return value: 0.
 */
#define CCALLER_OK 0

/*
 * Skip the current Cmd (FR-T-07, decision Q-01). Semantics: the
 * framework continues with the next Cmd of the test, counts the skip
 * in the statistics, shows it in the report, and does not treat it as
 * a failure for the exit code.
 *
 * Precondition: none. param_page: neither read nor written.
 * Return value: -255.
 */
#define CCALLER_ERR_SKIP (-255)

/*
 * Inclusive bounds of the wrapper-custom failure range [-127, -1]
 * (requirement spec 7.1). Codes in this range belong to the wrapper;
 * the framework reports the Cmd as failed and passes the number
 * through verbatim. Wrappers use these bounds to guard what they
 * return.
 */
#define CCALLER_ERR_WRAPPER_CUSTOM_MIN (-127)
#define CCALLER_ERR_WRAPPER_CUSTOM_MAX (-1)

/*
 * Inclusive bounds of the framework-reserved range [-255, -128]
 * (requirement spec 7.1). Values in this range belong to the
 * framework; wrappers must never return them. The only constant
 * defined in this version is CCALLER_ERR_SKIP at the lower bound;
 * further framework-internal codes are appended here with future
 * versions.
 */
#define CCALLER_ERR_FRAMEWORK_RESERVED_MIN (-255)
#define CCALLER_ERR_FRAMEWORK_RESERVED_MAX (-128)

/*
 * Number of uint64_t slots in one thread's param_page (FR-A-06).
 * Valid indices are [0, CCALLER_PARAM_PAGE_SLOTS). Wrappers must
 * bounds-check every index before touching the page (style guide
 * 8.1.5); the fixed size keeps the static def-use analysis
 * (FR-C-10) and the runtime page in exact agreement.
 *
 * Precondition: none. param_page: neither read nor written by this
 * constant itself. Return value: n/a (compile-time constant).
 */
#define CCALLER_PARAM_PAGE_SLOTS 512

/*
 * Unified call signature - TEMPLATE ONLY, do not paste as a
 * declaration. Each library exports its own concrete instances named
 * Call_<name>, resolved by symbol name at load time (FR-A-02):
 *
 *   int64_t Call_<name>(uint64_t *param_page,
 *                       const int64_t *params,
 *                       int64_t param_len);
 *
 * param_page  Page of CCALLER_PARAM_PAGE_SLOTS uint64_t slots owned
 *             by the framework, one per worker thread. A value
 *             written during one Cmd stays visible to later Cmds on
 *             the same thread; pages are never shared across threads
 *             (FR-A-06, decision Q-06). Bounds-check every index
 *             before access (style guide 8.1.5).
 * params      Read-only arguments of this call, in the order declared
 *             by the library description `paras` list.
 * param_len   Number of elements in params; must match the declared
 *             parameter count.
 *
 * String arguments arrive as pointers whose lifetime covers only the
 * current call (decision Q-07): a wrapper that needs the bytes later
 * must copy them before returning.
 *
 * Return value: CCALLER_OK on success, a code from the wrapper-custom
 * range [-127, -1] on domain failure, or CCALLER_ERR_SKIP to skip
 * this Cmd.
 */

/*
 * Load-time version handshake (FR-A-04). Every wrapper must export
 * exactly this symbol with exactly this signature.
 *
 * Precondition: none. param_page: neither read nor written.
 * Return value: the wrapper's ABI version, which must equal
 * CCALLER_ABI_VERSION.
 */
int64_t CCaller_abi_version(void);

#ifdef __cplusplus
}
#endif

#endif /* CCALLER_H */
