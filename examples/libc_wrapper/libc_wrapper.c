/*
 * libc_wrapper.c - a libc wrapper for the cCaller test framework.
 *
 * Exposes a slice of the C standard library through the cCaller unified
 * ABI. Every function is a `Call_<name>` instance and the library also
 * exports `CCaller_abi_version` for the load-time handshake (FR-A-04).
 *
 * Return-code contract (see ccaller.h): 0 success, [-127, -1] wrapper
 * custom failure, -255 skip. Every param_page index is bounds-checked
 * against [0, CCALLER_PARAM_PAGE_SLOTS) before it is touched.
 *
 * Build (from the repository root):
 *   gcc --shared -fPIC -I crates/ffi/include \
 *       examples/libc_wrapper/libc_wrapper.c \
 *       -o examples/libc_wrapper/libc_wrapper.so
 */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "ccaller.h"

/*
 * Wrapper-custom failure codes. All values stay inside [-127, -1]; the
 * framework reports them verbatim as a failed Cmd without interpreting
 * them (requirement spec 7.1).
 */
enum {
    E_PARAM_LEN = -1, /* param_len mismatch */
    E_SLOT_OOB  = -2, /* param_page index out of range */
    E_NULL_PTR  = -3, /* a slot held a NULL pointer */
    E_NOMEM     = -4, /* allocation failed */
    E_STRING    = -5, /* string too long to fit */
};

/* ---- helpers ---- */

/* The load-time ABI handshake (FR-A-04). */
int64_t CCaller_abi_version(void) {
    return CCALLER_ABI_VERSION;
}

/* Read one param_page slot; bounds-checked against [0, 512). */
static int slot_load(const uint64_t *page, int64_t idx, uint64_t *out) {
    if (idx < 0 || idx >= CCALLER_PARAM_PAGE_SLOTS) {
        return E_SLOT_OOB;
    }
    *out = page[idx];
    return 0;
}

/* Write one param_page slot; bounds-checked against [0, 512). */
static int slot_store(uint64_t *page, int64_t idx, uint64_t val) {
    if (idx < 0 || idx >= CCALLER_PARAM_PAGE_SLOTS) {
        return E_SLOT_OOB;
    }
    page[idx] = val;
    return 0;
}

/* Resolve a pointer held in a slot, bounds-checked and NULL-checked. */
static int slot_ptr(const uint64_t *page, const int64_t *params, int64_t pidx, void **out) {
    uint64_t raw;
    int rc = slot_load(page, params[pidx], &raw);
    if (rc != 0) {
        return rc;
    }
    if (raw == 0) {
        return E_NULL_PTR;
    }
    *out = (void *)(uintptr_t)raw;
    return 0;
}

/*
 * Unaligned, strict-aliasing-safe load/store helpers. memcpy-based so the
 * reads/writes work on any byte offset without UB.
 */
static uint8_t ld8(const void *p) {
    uint8_t v;
    memcpy(&v, p, 1);
    return v;
}
static uint16_t ld16(const void *p) {
    uint16_t v;
    memcpy(&v, p, 2);
    return v;
}
static uint32_t ld32(const void *p) {
    uint32_t v;
    memcpy(&v, p, 4);
    return v;
}
static uint64_t ld64(const void *p) {
    uint64_t v;
    memcpy(&v, p, 8);
    return v;
}
static void st8(void *p, uint8_t v) { memcpy(p, &v, 1); }
static void st16(void *p, uint16_t v) { memcpy(p, &v, 2); }
static void st32(void *p, uint32_t v) { memcpy(p, &v, 4); }
static void st64(void *p, uint64_t v) { memcpy(p, &v, 8); }

/* ---- memory management ---- */

int64_t Call_malloc(uint64_t *page, const int64_t *params, int64_t param_len) {
    if (param_len != 2) {
        return E_PARAM_LEN;
    }
    if (params[0] < 0) {
        return E_PARAM_LEN;
    }
    void *ptr = malloc((size_t)params[0]);
    if (!ptr) {
        return E_NOMEM;
    }
    int rc = slot_store(page, params[1], (uint64_t)(uintptr_t)ptr);
    if (rc != 0) {
        free(ptr);
        return rc;
    }
    return CCALLER_OK;
}

int64_t Call_free(uint64_t *page, const int64_t *params, int64_t param_len) {
    if (param_len != 1) {
        return E_PARAM_LEN;
    }
    void *ptr = NULL;
    int rc = slot_ptr(page, params, 0, &ptr);
    if (rc != 0) {
        return rc;
    }
    free(ptr);
    return CCALLER_OK;
}

/* ---- memory operations ---- */

int64_t Call_memcpy(uint64_t *page, const int64_t *params, int64_t param_len) {
    if (param_len != 3) {
        return E_PARAM_LEN;
    }
    void *dst = NULL;
    void *src = NULL;
    int rc = slot_ptr(page, params, 0, &dst);
    if (rc == 0) {
        rc = slot_ptr(page, params, 1, &src);
    }
    if (rc != 0) {
        return rc;
    }
    if (params[2] < 0) {
        return E_PARAM_LEN;
    }
    memcpy(dst, src, (size_t)params[2]);
    return CCALLER_OK;
}

int64_t Call_memset(uint64_t *page, const int64_t *params, int64_t param_len) {
    if (param_len != 3) {
        return E_PARAM_LEN;
    }
    void *dst = NULL;
    int rc = slot_ptr(page, params, 0, &dst);
    if (rc != 0) {
        return rc;
    }
    if (params[2] < 0) {
        return E_PARAM_LEN;
    }
    memset(dst, (int)params[1], (size_t)params[2]);
    return CCALLER_OK;
}

int64_t Call_memcmp(uint64_t *page, const int64_t *params, int64_t param_len) {
    if (param_len != 5) {
        return E_PARAM_LEN;
    }
    void *dst = NULL;
    void *src = NULL;
    int rc = slot_ptr(page, params, 0, &dst);
    if (rc == 0) {
        rc = slot_ptr(page, params, 2, &src);
    }
    if (rc != 0) {
        return rc;
    }
    if (params[1] < 0 || params[3] < 0 || params[4] < 0) {
        return E_PARAM_LEN;
    }
    int c = memcmp((const char *)dst + params[1], (const char *)src + params[3], (size_t)params[4]);
    return (c < 0) ? -1 : (c > 0) ? 1 : 0;
}

/* ---- data access (read/write 8/16/32/64) ---- */

int64_t Call_read8(uint64_t *page, const int64_t *params, int64_t param_len) {
    if (param_len != 2) {
        return E_PARAM_LEN;
    }
    void *p = NULL;
    int rc = slot_ptr(page, params, 0, &p);
    if (rc != 0) {
        return rc;
    }
    if (params[1] < 0) {
        return E_PARAM_LEN;
    }
    return (int64_t)ld8((const uint8_t *)p + params[1]);
}

int64_t Call_read16(uint64_t *page, const int64_t *params, int64_t param_len) {
    if (param_len != 2) {
        return E_PARAM_LEN;
    }
    void *p = NULL;
    int rc = slot_ptr(page, params, 0, &p);
    if (rc != 0) {
        return rc;
    }
    if (params[1] < 0) {
        return E_PARAM_LEN;
    }
    return (int64_t)ld16((const uint8_t *)p + params[1]);
}

int64_t Call_read32(uint64_t *page, const int64_t *params, int64_t param_len) {
    if (param_len != 2) {
        return E_PARAM_LEN;
    }
    void *p = NULL;
    int rc = slot_ptr(page, params, 0, &p);
    if (rc != 0) {
        return rc;
    }
    if (params[1] < 0) {
        return E_PARAM_LEN;
    }
    return (int64_t)ld32((const uint8_t *)p + params[1]);
}

int64_t Call_read64(uint64_t *page, const int64_t *params, int64_t param_len) {
    if (param_len != 2) {
        return E_PARAM_LEN;
    }
    void *p = NULL;
    int rc = slot_ptr(page, params, 0, &p);
    if (rc != 0) {
        return rc;
    }
    if (params[1] < 0) {
        return E_PARAM_LEN;
    }
    return (int64_t)ld64((const uint8_t *)p + params[1]);
}

int64_t Call_write8(uint64_t *page, const int64_t *params, int64_t param_len) {
    if (param_len != 3) {
        return E_PARAM_LEN;
    }
    void *p = NULL;
    int rc = slot_ptr(page, params, 0, &p);
    if (rc != 0) {
        return rc;
    }
    if (params[1] < 0) {
        return E_PARAM_LEN;
    }
    st8((uint8_t *)p + params[1], (uint8_t)params[2]);
    return CCALLER_OK;
}

int64_t Call_write16(uint64_t *page, const int64_t *params, int64_t param_len) {
    if (param_len != 3) {
        return E_PARAM_LEN;
    }
    void *p = NULL;
    int rc = slot_ptr(page, params, 0, &p);
    if (rc != 0) {
        return rc;
    }
    if (params[1] < 0) {
        return E_PARAM_LEN;
    }
    st16((uint8_t *)p + params[1], (uint16_t)params[2]);
    return CCALLER_OK;
}

int64_t Call_write32(uint64_t *page, const int64_t *params, int64_t param_len) {
    if (param_len != 3) {
        return E_PARAM_LEN;
    }
    void *p = NULL;
    int rc = slot_ptr(page, params, 0, &p);
    if (rc != 0) {
        return rc;
    }
    if (params[1] < 0) {
        return E_PARAM_LEN;
    }
    st32((uint8_t *)p + params[1], (uint32_t)params[2]);
    return CCALLER_OK;
}

int64_t Call_write64(uint64_t *page, const int64_t *params, int64_t param_len) {
    if (param_len != 3) {
        return E_PARAM_LEN;
    }
    void *p = NULL;
    int rc = slot_ptr(page, params, 0, &p);
    if (rc != 0) {
        return rc;
    }
    if (params[1] < 0) {
        return E_PARAM_LEN;
    }
    st64((uint8_t *)p + params[1], (uint64_t)params[2]);
    return CCALLER_OK;
}

/* ---- strings & arithmetic ---- */

int64_t Call_strlen(uint64_t *page, const int64_t *params, int64_t param_len) {
    (void)page;
    if (param_len != 1) {
        return E_PARAM_LEN;
    }
    const char *s = (const char *)(intptr_t)params[0];
    if (!s) {
        return E_NULL_PTR;
    }
    return (int64_t)strlen(s);
}

int64_t Call_atoi(uint64_t *page, const int64_t *params, int64_t param_len) {
    (void)page;
    if (param_len != 1) {
        return E_PARAM_LEN;
    }
    const char *s = (const char *)(intptr_t)params[0];
    if (!s) {
        return E_NULL_PTR;
    }
    return (int64_t)atoi(s);
}

int64_t Call_strcmp(uint64_t *page, const int64_t *params, int64_t param_len) {
    (void)page;
    if (param_len != 2) {
        return E_PARAM_LEN;
    }
    const char *a = (const char *)(intptr_t)params[0];
    const char *b = (const char *)(intptr_t)params[1];
    if (!a || !b) {
        return E_NULL_PTR;
    }
    int c = strcmp(a, b);
    return (c < 0) ? -1 : (c > 0) ? 1 : 0;
}

/*
 * Bounded string copy with a guaranteed NUL terminator: the source must
 * fit into `len` bytes including its terminator, otherwise the Cmd fails.
 */
int64_t Call_strncpy(uint64_t *page, const int64_t *params, int64_t param_len) {
    if (param_len != 3) {
        return E_PARAM_LEN;
    }
    void *dst = NULL;
    int rc = slot_ptr(page, params, 0, &dst);
    if (rc != 0) {
        return rc;
    }
    const char *src = (const char *)(intptr_t)params[1];
    if (!src) {
        return E_NULL_PTR;
    }
    if (params[2] < 0) {
        return E_PARAM_LEN;
    }
    size_t src_len = strlen(src);
    if (src_len >= (size_t)params[2]) {
        return E_STRING;
    }
    memset(dst, 0, (size_t)params[2]);
    memcpy(dst, src, src_len);
    return CCALLER_OK;
}

int64_t Call_add(uint64_t *page, const int64_t *params, int64_t param_len) {
    (void)page;
    if (param_len != 2) {
        return E_PARAM_LEN;
    }
    return params[0] + params[1];
}

/* ---- slot helpers (exercise read / write / read_write) ---- */

/* Store a raw u64 into a param_page slot. */
int64_t Call_store(uint64_t *page, const int64_t *params, int64_t param_len) {
    if (param_len != 2) {
        return E_PARAM_LEN;
    }
    int rc = slot_store(page, params[0], (uint64_t)params[1]);
    return rc == 0 ? CCALLER_OK : rc;
}

/* Atomically add `delta` to a slot, returning its previous value. */
int64_t Call_fetch_add(uint64_t *page, const int64_t *params, int64_t param_len) {
    if (param_len != 2) {
        return E_PARAM_LEN;
    }
    uint64_t old;
    int rc = slot_load(page, params[0], &old);
    if (rc != 0) {
        return rc;
    }
    rc = slot_store(page, params[0], old + (uint64_t)params[1]);
    if (rc != 0) {
        return rc;
    }
    return (int64_t)old;
}

/* ---- lifecycle / death-test helpers ---- */

/* Always skips the current Cmd (Q-01): returns CCALLER_ERR_SKIP. */
int64_t Call_skip(uint64_t *page, const int64_t *params, int64_t param_len) {
    (void)page;
    (void)params;
    (void)param_len;
    return CCALLER_ERR_SKIP;
}

/*
 * Aborts the process. Intended for `should_panic = true` death tests: the
 * M2 executor skips such tests wholesale; when subprocess isolation lands
 * (M3) the parent observes the abort as the expected death.
 */
int64_t Call_abort(uint64_t *page, const int64_t *params, int64_t param_len) {
    (void)page;
    (void)params;
    (void)param_len;
    abort();
    /* not reached */
    return CCALLER_OK;
}
