/*
 * comparison.h -- C ABI for the columnar engine's comparison kernels.
 *
 * The ABI is deliberately tiny: two functions and a probe. Everything the
 * engine needs from the C++ side is "compare a column against a threshold and
 * give me a bit mask", and every function here is a leaf that allocates
 * nothing and touches no global state. That is what makes the FFI seam cheap
 * enough to leave in the hot path -- see ADR-0003.
 *
 * Three implementations of the same predicate exist behind these entry
 * points: a genuinely scalar reference, a compiler-vectorised reference, and
 * a SIMD path. The scalar reference is branchless and computes exactly the
 * same predicate, so the ratio between it and the SIMD path measures
 * vectorisation rather than branch prediction. This matters: a speedup
 * measured against a naive branching baseline is not a claim about SIMD.
 * The compiler-vectorised reference is the *stronger* baseline -- it is what
 * you get for free by writing the loop in plain C++ -- and the hand-written
 * SIMD path is only worth its complexity if it beats that one too.
 *
 * NULL semantics: a row whose validity bit is clear is NULL and never
 * matches -- for every operator, including "!=".
 *
 * NaN semantics: a comparison involving a NaN value or a NaN threshold never
 * matches, for every operator, including "!=". This is "unknown propagates".
 * It is stricter than SQL's treatment of NaN and it is documented in the
 * engine's plan output, because a filter that silently matched NaN rows
 * would be a bug that only shows up in an aggregate.
 */

#ifndef COL_COMPARISON_H
#define COL_COMPARISON_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Comparison operators.
 *
 * These integer codes MUST match columnar_core::ComparisonOp::abi_code().
 * A mismatch would silently change what the filter does, so the Rust side
 * asserts the mapping against these constants in a test.
 */
#define COL_CMP_LT 0
#define COL_CMP_LE 1
#define COL_CMP_GT 2
#define COL_CMP_GE 3
#define COL_CMP_EQ 4
#define COL_CMP_NE 5

/* Which SIMD path this build and machine can actually run. */
#define COL_SIMD_NONE 0
#define COL_SIMD_NEON 1
#define COL_SIMD_AVX2 2

/*
 * Compare `n` int64 values against `threshold`.
 *
 *   values    -- dense array of `n` values; NULL values are still present
 *                here, the validity mask is what says whether a value counts.
 *   n         -- number of values.
 *   op        -- one of the COL_CMP_* codes.
 *   threshold -- the value to compare against.
 *   validity  -- optional bit-packed mask, LSB-first, one bit per row.
 *                Bit set means "this row holds a value". NULL means every
 *                row is valid.
 *   out       -- bit-packed match mask, LSB-first, one bit per row.
 *                MUST have room for (n + 63) / 64 words. Bits beyond row n
 *                are not written.
 *
 * Returns the number of set bits in `out`, or a negative value on error
 * (unknown operator, or `out` too small).
 */
int64_t col_compare_i64(const int64_t *values, uint64_t n, int32_t op, int64_t threshold,
                        const uint64_t *validity, uint64_t *out);

/*
 * Compare `n` float64 values against `threshold`.
 *
 * Same contract as col_compare_i64, with the NaN rule stated above.
 */
int64_t col_compare_f64(const double *values, uint64_t n, int32_t op, double threshold,
                        const uint64_t *validity, uint64_t *out);

/*
 * The two scalar baselines.
 *
 * Both compute the same predicate, in the same branchless shape, as the SIMD
 * paths -- the only difference is whether the compiler was allowed to
 * vectorise the loop. They are separate entry points because they are
 * separate translation units, compiled with different flags:
 *
 *   - `col_compare_*_auto`  lives in `comparison.cpp` and is compiled with
 *     ordinary flags, so the compiler vectorises it. This is the baseline
 *     the hand-written kernel has to beat to be worth anything: it is what
 *     you get for free by writing the loop in plain C++.
 *   - `col_compare_*_scalar` lives in `scalar.cpp` and is compiled with
 *     `-fno-vectorize`. This is the same loop with vectorisation taken away,
 *     and it is the baseline that answers "how much is vectorisation worth?".
 *
 * Publishing only the second would produce a speedup number that credits
 * SIMD for work the compiler was already doing. See ADR-0006.
 *
 * Neither ever dispatches to a SIMD path.
 */
int64_t col_compare_i64_auto(const int64_t *values, uint64_t n, int32_t op, int64_t threshold,
                             const uint64_t *validity, uint64_t *out);

int64_t col_compare_f64_auto(const double *values, uint64_t n, int32_t op, double threshold,
                             const uint64_t *validity, uint64_t *out);

/*
 * The genuine scalar reference: one comparison per row, compiled with
 * vectorisation disabled. Never dispatches to a SIMD path.
 */
int64_t col_compare_i64_scalar(const int64_t *values, uint64_t n, int32_t op, int64_t threshold,
                                const uint64_t *validity, uint64_t *out);

int64_t col_compare_f64_scalar(const double *values, uint64_t n, int32_t op, double threshold,
                                const uint64_t *validity, uint64_t *out);

/*
 * Which SIMD path is live on this machine right now.
 *
 * This is a runtime probe, not a compile-time constant: on x86-64 the
 * binary is built for the baseline ISA and the AVX2 path is reached through
 * a target-attributed function only after a CPU feature check. A build that
 * hard-codes -mavx2 would fault on any machine without AVX2 before it had a
 * chance to fall back.
 */
int32_t col_simd_kind(void);

#ifdef __cplusplus
}
#endif

#endif /* COL_COMPARISON_H */
