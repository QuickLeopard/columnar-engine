/*
 * scalar.cpp -- the scalar reference, compiled WITHOUT vectorisation.
 *
 * This translation unit exists to be a *honest baseline*.
 *
 * The scalar reference in `scalar_impl.h` is the same loop that the SIMD
 * kernels replace. When it is compiled normally, clang vectorises it: on
 * AArch64 the object contains `fcmgt.2d`, `ushl.2d` and `dup.2d`, which
 * means the "scalar" reference was not scalar at all. A speedup measured
 * against that object is not a vectorisation speedup -- it is a comparison
 * between two vectorised implementations, which is a legitimate thing to
 * measure but is not the thing the ratio claims to be.
 *
 * So the same source is compiled twice:
 *
 *   - `comparison.cpp`  -- normal flags, auto-vectorised. Exposed as
 *     `col_compare_*_auto`.
 *   - `scalar.cpp`      -- `-fno-vectorize`, genuinely scalar. Exposed as
 *     `col_compare_*_scalar`.
 *
 * The flags are applied in `build.rs`, not here: a header or a source file
 * cannot carry per-file compiler flags, and the two copies must differ.
 *
 * The result is that the published kernel A/B answers two questions at once:
 *
 *   1. `scalar -> cpp-simd`: how much is vectorisation worth?
 *   2. `cpp-auto -> cpp-simd`: does the hand-written kernel beat what the
 *      compiler does with the same loop?
 *
 * Question 2 has no predetermined answer. On this machine it came out the
 * other way -- see ADR-0006.
 */

#include "comparison.h"
#include "scalar_impl.h"

extern "C" {

/*
 * The genuine scalar reference: one comparison per row, no vectorisation.
 * Never dispatches to a SIMD path.
 */
int64_t col_compare_i64_scalar(const int64_t *values, uint64_t n, int32_t op, int64_t threshold,
                                const uint64_t *validity, uint64_t *out) {
    if (values == nullptr || out == nullptr) {
        return -1;
    }
    COL_DISPATCH_SCALAR_I64(op, values, n, threshold, validity, out);
}

int64_t col_compare_f64_scalar(const double *values, uint64_t n, int32_t op, double threshold,
                                const uint64_t *validity, uint64_t *out) {
    if (values == nullptr || out == nullptr) {
        return -1;
    }
    COL_DISPATCH_SCALAR_F64(op, values, n, threshold, validity, out);
}

} /* extern "C" */
