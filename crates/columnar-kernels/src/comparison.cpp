/*
 * comparison.cpp -- scalar reference and SIMD comparison kernels.
 *
 * Structure:
 *
 *   1. A branchless scalar reference, shared with `scalar.cpp` through
 *      `scalar_impl.h`. Compiled here with ordinary flags, the compiler
 *      vectorises it; this copy is exposed as `col_compare_*_auto` and is
 *      the baseline the hand-written kernel has to beat to be worth
 *      anything. The genuinely unvectorised copy lives in `scalar.cpp`.
 *
 *   2. An AVX2 path, reached only through functions carrying
 *      __attribute__((target("avx2"))) and only after a runtime CPU probe.
 *      The translation unit is NOT compiled with -mavx2, so the scalar path
 *      stays runnable on a machine that has no AVX2 at all.
 *
 *   3. A NEON path for AArch64, where NEON is architecturally mandatory and
 *      therefore needs no probe.
 *
 * Both SIMD paths compute, per 64-row block, a 64-bit mask of matching rows.
 * The validity mask is ANDed in once per block rather than per lane: that
 * keeps the SIMD inner loop free of a second memory stream and turns the
 * null handling into a single scalar operation per 64 rows.
 */

#include "comparison.h"
#include "scalar_impl.h"

#include <atomic>
#include <cstddef>
#include <cstdlib>
#include <cstring>

#if defined(__aarch64__) && defined(__ARM_NEON)
#include <arm_neon.h>
#define COL_HAS_NEON 1
#endif

#if defined(__x86_64__)
#include <immintrin.h>
#if defined(__GNUC__) || defined(__clang__)
#define COL_HAS_AVX2_DISPATCH 1
#endif
#endif

namespace {

/*
 * Rows processed by the AVX2 path.
 *
 * `col_simd_kind()` says which path the dispatch *would* take; this counter
 * says the path's instructions actually ran. The difference matters when the
 * path is reachable only through the `COLUMNAR_FORCE_SIMD` override -- which
 * is exactly the case on an ARM64 Mac under Rosetta 2, where the CPU probe
 * reports no AVX2 but the instructions execute correctly.
 */
std::atomic<uint64_t> g_avx2_rows{0};

/* ------------------------------------------------------------------ */
/* Scalar reference                                                    */
/* ------------------------------------------------------------------ */

/*
 * The scalar reference itself lives in `scalar_impl.h`, so that the same
 * source can be compiled twice with different flags. This translation unit
 * gets the auto-vectorised copy; `scalar.cpp` gets the unvectorised one.
 */

using col::cmp_f64;
using col::cmp_i64;

/* ------------------------------------------------------------------ */
/* AVX2                                                                */
/* ------------------------------------------------------------------ */

#if defined(COL_HAS_AVX2_DISPATCH)

/*
 * Four int64 lanes per iteration. _mm256_cmpgt_epi64 and
 * _mm256_cmpeq_epi64 are both AVX2; there is no AVX2 signed 64-bit
 * "less than", so LT is expressed as GT with the operands swapped.
 */
template <int OP>
__attribute__((target("avx2"))) inline uint32_t avx2_lanes_i64(const int64_t *p, int64_t th) {
    const __m256i a = _mm256_loadu_si256(reinterpret_cast<const __m256i *>(p));
    const __m256i b = _mm256_set1_epi64x(th);
    __m256i cmp;
    if constexpr (OP == COL_CMP_LT) {
        cmp = _mm256_cmpgt_epi64(b, a);
    } else if constexpr (OP == COL_CMP_LE) {
        cmp = _mm256_or_si256(_mm256_cmpgt_epi64(b, a), _mm256_cmpeq_epi64(a, b));
    } else if constexpr (OP == COL_CMP_GT) {
        cmp = _mm256_cmpgt_epi64(a, b);
    } else if constexpr (OP == COL_CMP_GE) {
        cmp = _mm256_or_si256(_mm256_cmpgt_epi64(a, b), _mm256_cmpeq_epi64(a, b));
    } else if constexpr (OP == COL_CMP_EQ) {
        cmp = _mm256_cmpeq_epi64(a, b);
    } else {
        cmp = _mm256_xor_si256(_mm256_cmpeq_epi64(a, b), _mm256_set1_epi64x(-1));
    }
    /* Each lane is all-ones or all-zeros; the sign bit of each 64-bit lane
     * is therefore the lane's predicate result. */
    return static_cast<uint32_t>(_mm256_movemask_pd(_mm256_castsi256_pd(cmp)));
}

/*
 * Four float64 lanes per iteration. The _OQ (ordered, quiet) predicates
 * return false for any comparison involving a NaN, which is exactly the
 * "unknown propagates" rule the scalar path implements.
 */
template <int OP>
__attribute__((target("avx2"))) inline uint32_t avx2_lanes_f64(const double *p, double th) {
    const __m256d a = _mm256_loadu_pd(p);
    const __m256d b = _mm256_set1_pd(th);
    __m256d cmp;
    if constexpr (OP == COL_CMP_LT) {
        cmp = _mm256_cmp_pd(a, b, _CMP_LT_OQ);
    } else if constexpr (OP == COL_CMP_LE) {
        cmp = _mm256_cmp_pd(a, b, _CMP_LE_OQ);
    } else if constexpr (OP == COL_CMP_GT) {
        cmp = _mm256_cmp_pd(a, b, _CMP_GT_OQ);
    } else if constexpr (OP == COL_CMP_GE) {
        cmp = _mm256_cmp_pd(a, b, _CMP_GE_OQ);
    } else if constexpr (OP == COL_CMP_EQ) {
        cmp = _mm256_cmp_pd(a, b, _CMP_EQ_OQ);
    } else {
        cmp = _mm256_cmp_pd(a, b, _CMP_NEQ_OQ);
    }
    return static_cast<uint32_t>(_mm256_movemask_pd(cmp));
}

template <int OP>
__attribute__((target("avx2"))) int64_t avx2_i64(const int64_t *values, uint64_t n, int64_t th,
                                               const uint64_t *validity, uint64_t *out) {
    const uint64_t blocks = (n + 63) / 64;
    int64_t total = 0;
    for (uint64_t b = 0; b < blocks; ++b) {
        const uint64_t base = b * 64;
        const uint64_t upto = (base + 64 <= n) ? 64 : (n - base);
        uint64_t mask = 0;
        if (upto == 64) {
            for (uint64_t k = 0; k < 16; ++k) {
                const uint32_t bits = avx2_lanes_i64<OP>(values + base + k * 4, th);
                mask |= static_cast<uint64_t>(bits) << (k * 4);
            }
        } else {
            /* Tail block -- see the note in neon_i64. */
            for (uint64_t i = 0; i < upto; ++i) {
                const bool m = cmp_i64<OP>(values[base + i], th);
                mask |= static_cast<uint64_t>(m) << i;
            }
        }
        if (validity != nullptr) {
            mask &= validity[b];
        }
        out[b] = mask;
        total += col::popcount64(mask);
    }
    return total;
}

template <int OP>
__attribute__((target("avx2"))) int64_t avx2_f64(const double *values, uint64_t n, double th,
                                               const uint64_t *validity, uint64_t *out) {
    const uint64_t blocks = (n + 63) / 64;
    int64_t total = 0;
    for (uint64_t b = 0; b < blocks; ++b) {
        const uint64_t base = b * 64;
        const uint64_t upto = (base + 64 <= n) ? 64 : (n - base);
        uint64_t mask = 0;
        if (upto == 64) {
            for (uint64_t k = 0; k < 16; ++k) {
                const uint32_t bits = avx2_lanes_f64<OP>(values + base + k * 4, th);
                mask |= static_cast<uint64_t>(bits) << (k * 4);
            }
        } else {
            /* Tail block -- see the note in neon_i64. */
            for (uint64_t i = 0; i < upto; ++i) {
                const bool m = cmp_f64<OP>(values[base + i], th);
                mask |= static_cast<uint64_t>(m) << i;
            }
        }
        if (validity != nullptr) {
            mask &= validity[b];
        }
        out[b] = mask;
        total += col::popcount64(mask);
    }
    return total;
}

#endif /* COL_HAS_AVX2_DISPATCH */

/* ------------------------------------------------------------------ */
/* NEON (AArch64)                                                      */
/* ------------------------------------------------------------------ */

#if defined(COL_HAS_NEON)

/*
 * Two int64 lanes per iteration. AArch64 makes NEON architecturally
 * mandatory, so there is no probe and no fallback within this path.
 *
 * The result is a 128-bit vector of two lanes, each all-ones (the predicate
 * held) or all-zeros (it did not). It is deliberately *not* reduced to two
 * bits here: the caller accumulates the mask in vector registers, and only
 * extracts once per 64-row block.
 *
 * Why that matters: the first version of this kernel reduced each pair of
 * lanes to two scalar bits with `vgetq_lane_u64` and shifted them into a
 * scalar mask. That put two vector-to-GPR moves (`umov`) on the critical
 * path for every two rows, and the kernel ended up 19% slower than the same
 * source compiled with the vectoriser left on. The accumulator below removes
 * the extraction from the inner loop entirely.
 */
template <int OP>
inline uint64x2_t neon_cmp_i64(const int64_t *p, int64_t th) {
    const int64x2_t a = vld1q_s64(p);
    const int64x2_t b = vdupq_n_s64(th);
    uint64x2_t cmp;
    if constexpr (OP == COL_CMP_LT) {
        cmp = vreinterpretq_u64_s64(vcltq_s64(a, b));
    } else if constexpr (OP == COL_CMP_LE) {
        cmp = vorrq_u64(vreinterpretq_u64_s64(vcltq_s64(a, b)),
                        vreinterpretq_u64_s64(vceqq_s64(a, b)));
    } else if constexpr (OP == COL_CMP_GT) {
        cmp = vreinterpretq_u64_s64(vcgtq_s64(a, b));
    } else if constexpr (OP == COL_CMP_GE) {
        cmp = vorrq_u64(vreinterpretq_u64_s64(vcgtq_s64(a, b)),
                        vreinterpretq_u64_s64(vceqq_s64(a, b)));
    } else if constexpr (OP == COL_CMP_EQ) {
        cmp = vreinterpretq_u64_s64(vceqq_s64(a, b));
    } else {
        cmp = veorq_u64(vreinterpretq_u64_s64(vceqq_s64(a, b)), vdupq_n_u64(~0ULL));
    }
    return cmp;
}

template <int OP>
inline uint64x2_t neon_cmp_f64(const double *p, double th) {
    const float64x2_t a = vld1q_f64(p);
    const float64x2_t b = vdupq_n_f64(th);
    uint64x2_t cmp;
    if constexpr (OP == COL_CMP_LT) {
        cmp = vreinterpretq_u64_f64(vcltq_f64(a, b));
    } else if constexpr (OP == COL_CMP_LE) {
        cmp = vorrq_u64(vreinterpretq_u64_f64(vcltq_f64(a, b)),
                        vreinterpretq_u64_f64(vceqq_f64(a, b)));
    } else if constexpr (OP == COL_CMP_GT) {
        cmp = vreinterpretq_u64_f64(vcgtq_f64(a, b));
    } else if constexpr (OP == COL_CMP_GE) {
        cmp = vorrq_u64(vreinterpretq_u64_f64(vcgtq_f64(a, b)),
                        vreinterpretq_u64_f64(vceqq_f64(a, b)));
    } else if constexpr (OP == COL_CMP_EQ) {
        cmp = vreinterpretq_u64_f64(vceqq_f64(a, b));
    } else {
        /* Ordered not-equal: false when either operand is NaN. */
        cmp = vorrq_u64(vreinterpretq_u64_f64(vcgtq_f64(a, b)),
                        vreinterpretq_u64_f64(vcltq_f64(a, b)));
    }
    return cmp;
}

/*
 * The mask accumulator.
 *
 * `acc` is a 128-bit register holding two 64-bit halves of the same 64-row
 * block's match mask:
 *
 *   - lane 0 collects the even-numbered rows, at their own bit positions
 *     (row 2k lands in bit 2k);
 *   - lane 1 collects the odd-numbered rows, at their own bit positions
 *     (row 2k+1 lands in bit 2k+1).
 *
 * Both halves therefore use the *same* bit numbering as the final mask, and
 * the two halves are OR-ed together exactly once, at the end of the block:
 *
 *     mask = lane0 | lane1
 *
 * That is the whole trick. The shift amounts for lane 0 and lane 1 differ by
 * one, and they are carried in a vector that is incremented by two each
 * iteration, so no per-pair scalar shift is needed either.
 */
struct NeonMaskAccumulator {
    uint64x2_t lanes;
    uint64x2_t shifts;

    inline NeonMaskAccumulator()
        : lanes(vdupq_n_u64(0)),
          shifts(vsetq_lane_u64(1, vdupq_n_u64(0), 1)) {}

    /* Folds the comparison of the two rows at `p` into the accumulator. */
    template <int OP>
    inline void push_i64(const int64_t *p, int64_t th) {
        const uint64x2_t bit = vshrq_n_u64(neon_cmp_i64<OP>(p, th), 63);
        lanes = vorrq_u64(lanes, vshlq_u64(bit, shifts));
        shifts = vaddq_u64(shifts, vdupq_n_u64(2));
    }

    template <int OP>
    inline void push_f64(const double *p, double th) {
        const uint64x2_t bit = vshrq_n_u64(neon_cmp_f64<OP>(p, th), 63);
        lanes = vorrq_u64(lanes, vshlq_u64(bit, shifts));
        shifts = vaddq_u64(shifts, vdupq_n_u64(2));
    }

    /* Collapses the two halves into the 64-bit mask for this block. */
    inline uint64_t finish() const {
        return vgetq_lane_u64(lanes, 0) | vgetq_lane_u64(lanes, 1);
    }
};

template <int OP>
int64_t neon_i64(const int64_t *values, uint64_t n, int64_t th, const uint64_t *validity,
                 uint64_t *out) {
    const uint64_t blocks = (n + 63) / 64;
    int64_t total = 0;
    for (uint64_t b = 0; b < blocks; ++b) {
        const uint64_t base = b * 64;
        const uint64_t upto = (base + 64 <= n) ? 64 : (n - base);
        uint64_t mask = 0;
        if (upto == 64) {
            NeonMaskAccumulator acc;
            for (uint64_t k = 0; k < 32; ++k) {
                acc.push_i64<OP>(values + base + k * 2, th);
            }
            mask = acc.finish();
        } else {
            /* Tail block. The SIMD loop always covers 64 rows, so running it
             * on a partial block would read past the end of the array. The
             * partial block is computed with the scalar reference instead. */
            for (uint64_t i = 0; i < upto; ++i) {
                const bool m = cmp_i64<OP>(values[base + i], th);
                mask |= static_cast<uint64_t>(m) << i;
            }
        }
        if (validity != nullptr) {
            mask &= validity[b];
        }
        out[b] = mask;
        total += col::popcount64(mask);
    }
    return total;
}

template <int OP>
int64_t neon_f64(const double *values, uint64_t n, double th, const uint64_t *validity,
                 uint64_t *out) {
    const uint64_t blocks = (n + 63) / 64;
    int64_t total = 0;
    for (uint64_t b = 0; b < blocks; ++b) {
        const uint64_t base = b * 64;
        const uint64_t upto = (base + 64 <= n) ? 64 : (n - base);
        uint64_t mask = 0;
        if (upto == 64) {
            NeonMaskAccumulator acc;
            for (uint64_t k = 0; k < 32; ++k) {
                acc.push_f64<OP>(values + base + k * 2, th);
            }
            mask = acc.finish();
        } else {
            /* Tail block -- see the note in neon_i64. */
            for (uint64_t i = 0; i < upto; ++i) {
                const bool m = cmp_f64<OP>(values[base + i], th);
                mask |= static_cast<uint64_t>(m) << i;
            }
        }
        if (validity != nullptr) {
            mask &= validity[b];
        }
        out[b] = mask;
        total += col::popcount64(mask);
    }
    return total;
}

#endif /* COL_HAS_NEON */

/* ------------------------------------------------------------------ */
/* Dispatch                                                            */
/* ------------------------------------------------------------------ */

/*
 * The `COLUMNAR_FORCE_SIMD` override.
 *
 * The probe below reports the *host* CPU's feature bits. Rosetta 2 on Apple
 * Silicon executes AVX2 instructions correctly but reports `avx2=0`, because
 * the host is an ARM64 chip with no AVX2 units -- so the probe would hide a
 * path that in fact runs, and the AVX2 path could never be correctness tested
 * on an ARM64 Mac.
 *
 * The override makes the path reachable for correctness testing. It is not a
 * performance switch: under Rosetta the instructions are emulated, so any
 * timing taken with the override measures the emulator and must never be
 * published. Forcing `avx2` on a real x86-64 machine without AVX2 raises
 * SIGILL, which is what "force" means.
 *
 * The value is read once and cached, because this sits on the dispatch path.
 */
inline const char *forced_simd_value() {
    static const char *value = []() { return std::getenv("COLUMNAR_FORCE_SIMD"); }();
    return value;
}

inline bool force_is(const char *wanted) {
    const char *value = forced_simd_value();
    return value != nullptr && std::strcmp(value, wanted) == 0;
}

/*
 * The AVX2 feature probe.
 *
 * This used to be `__builtin_cpu_supports("avx2")`, which on Darwin expands
 * to a call into `libcpu_indicator` -- a library that is not part of the
 * toolchain's default link line on every machine, and that made the x86-64
 * build fail to link. The probe is a handful of instructions, so it is
 * written out here instead of depended upon:
 *
 *   1. CPUID leaf 1: ECX bit 27 (OSXSAVE) and bit 28 (AVX).
 *   2. XGETBV: XCR0 must have the XMM (bit 1) and YMM (bit 2) states
 *      enabled, or the OS will not save the registers across a context
 *      switch and an AVX instruction would fault.
 *   3. CPUID leaf 7, subleaf 0: EBX bit 5 (AVX2).
 *
 * The checks are the same ones the compiler's builtin performs; the
 * difference is that this one has no link-time dependency.
 */
#if defined(COL_HAS_AVX2_DISPATCH)
inline void cpuid_count(uint32_t leaf, uint32_t subleaf, uint32_t regs[4]) {
    __asm__ volatile("cpuid"
                     : "=a"(regs[0]), "=b"(regs[1]), "=c"(regs[2]), "=d"(regs[3])
                     : "a"(leaf), "c"(subleaf));
}

inline uint64_t xgetbv0() {
    uint32_t lo = 0;
    uint32_t hi = 0;
    __asm__ volatile("xgetbv" : "=a"(lo), "=d"(hi) : "c"(0));
    return (static_cast<uint64_t>(hi) << 32) | lo;
}
#endif /* COL_HAS_AVX2_DISPATCH */

inline bool avx2_available() {
#if defined(COL_HAS_AVX2_DISPATCH)
    if (force_is("none")) {
        return false;
    }
    if (force_is("avx2")) {
        return true;
    }

    uint32_t regs[4];
    cpuid_count(1, 0, regs);
    const bool osxsave = (regs[2] & (1u << 27)) != 0;
    const bool avx = (regs[2] & (1u << 28)) != 0;
    if (!osxsave || !avx) {
        return false;
    }

    /* XCR0 bits 1 (XMM) and 2 (YMM) must both be enabled by the OS. */
    if ((xgetbv0() & 0x6u) != 0x6u) {
        return false;
    }

    cpuid_count(7, 0, regs);
    return (regs[1] & (1u << 5)) != 0;
#else
    return false;
#endif
}

#define COL_DISPATCH_AVX2_I64(OPVAR, ...)                                  \
    switch (OPVAR) {                                                       \
    case COL_CMP_LT:                                                       \
        return avx2_i64<COL_CMP_LT>(__VA_ARGS__);                          \
    case COL_CMP_LE:                                                       \
        return avx2_i64<COL_CMP_LE>(__VA_ARGS__);                          \
    case COL_CMP_GT:                                                       \
        return avx2_i64<COL_CMP_GT>(__VA_ARGS__);                          \
    case COL_CMP_GE:                                                       \
        return avx2_i64<COL_CMP_GE>(__VA_ARGS__);                          \
    case COL_CMP_EQ:                                                       \
        return avx2_i64<COL_CMP_EQ>(__VA_ARGS__);                          \
    case COL_CMP_NE:                                                       \
        return avx2_i64<COL_CMP_NE>(__VA_ARGS__);                          \
    default:                                                               \
        return -1;                                                         \
    }

#define COL_DISPATCH_AVX2_F64(OPVAR, ...)                                  \
    switch (OPVAR) {                                                       \
    case COL_CMP_LT:                                                       \
        return avx2_f64<COL_CMP_LT>(__VA_ARGS__);                          \
    case COL_CMP_LE:                                                       \
        return avx2_f64<COL_CMP_LE>(__VA_ARGS__);                          \
    case COL_CMP_GT:                                                       \
        return avx2_f64<COL_CMP_GT>(__VA_ARGS__);                          \
    case COL_CMP_GE:                                                       \
        return avx2_f64<COL_CMP_GE>(__VA_ARGS__);                          \
    case COL_CMP_EQ:                                                       \
        return avx2_f64<COL_CMP_EQ>(__VA_ARGS__);                          \
    case COL_CMP_NE:                                                       \
        return avx2_f64<COL_CMP_NE>(__VA_ARGS__);                          \
    default:                                                               \
        return -1;                                                         \
    }

#define COL_DISPATCH_NEON_I64(OPVAR, ...)                                  \
    switch (OPVAR) {                                                       \
    case COL_CMP_LT:                                                       \
        return neon_i64<COL_CMP_LT>(__VA_ARGS__);                          \
    case COL_CMP_LE:                                                       \
        return neon_i64<COL_CMP_LE>(__VA_ARGS__);                          \
    case COL_CMP_GT:                                                       \
        return neon_i64<COL_CMP_GT>(__VA_ARGS__);                          \
    case COL_CMP_GE:                                                       \
        return neon_i64<COL_CMP_GE>(__VA_ARGS__);                          \
    case COL_CMP_EQ:                                                       \
        return neon_i64<COL_CMP_EQ>(__VA_ARGS__);                          \
    case COL_CMP_NE:                                                       \
        return neon_i64<COL_CMP_NE>(__VA_ARGS__);                          \
    default:                                                               \
        return -1;                                                         \
    }

#define COL_DISPATCH_NEON_F64(OPVAR, ...)                                  \
    switch (OPVAR) {                                                       \
    case COL_CMP_LT:                                                       \
        return neon_f64<COL_CMP_LT>(__VA_ARGS__);                          \
    case COL_CMP_LE:                                                       \
        return neon_f64<COL_CMP_LE>(__VA_ARGS__);                          \
    case COL_CMP_GT:                                                       \
        return neon_f64<COL_CMP_GT>(__VA_ARGS__);                          \
    case COL_CMP_GE:                                                       \
        return neon_f64<COL_CMP_GE>(__VA_ARGS__);                          \
    case COL_CMP_EQ:                                                       \
        return neon_f64<COL_CMP_EQ>(__VA_ARGS__);                          \
    case COL_CMP_NE:                                                       \
        return neon_f64<COL_CMP_NE>(__VA_ARGS__);                          \
    default:                                                               \
        return -1;                                                         \
    }

} /* namespace */

extern "C" {

int64_t col_compare_i64(const int64_t *values, uint64_t n, int32_t op, int64_t threshold,
                        const uint64_t *validity, uint64_t *out) {
    if (values == nullptr || out == nullptr) {
        return -1;
    }
#if defined(COL_HAS_NEON)
    COL_DISPATCH_NEON_I64(op, values, n, threshold, validity, out);
#elif defined(COL_HAS_AVX2_DISPATCH)
    if (avx2_available()) {
        g_avx2_rows.fetch_add(n, std::memory_order_relaxed);
        COL_DISPATCH_AVX2_I64(op, values, n, threshold, validity, out);
    }
#endif
    COL_DISPATCH_SCALAR_I64(op, values, n, threshold, validity, out);
}

int64_t col_compare_f64(const double *values, uint64_t n, int32_t op, double threshold,
                        const uint64_t *validity, uint64_t *out) {
    if (values == nullptr || out == nullptr) {
        return -1;
    }
#if defined(COL_HAS_NEON)
    COL_DISPATCH_NEON_F64(op, values, n, threshold, validity, out);
#elif defined(COL_HAS_AVX2_DISPATCH)
    if (avx2_available()) {
        g_avx2_rows.fetch_add(n, std::memory_order_relaxed);
        COL_DISPATCH_AVX2_F64(op, values, n, threshold, validity, out);
    }
#endif
    COL_DISPATCH_SCALAR_F64(op, values, n, threshold, validity, out);
}

int32_t col_simd_kind(void) {
#if defined(COL_HAS_NEON)
    return COL_SIMD_NEON;
#elif defined(COL_HAS_AVX2_DISPATCH)
    if (avx2_available()) {
        return COL_SIMD_AVX2;
    }
#endif
    return COL_SIMD_NONE;
}

uint64_t col_avx2_rows(void) {
    return g_avx2_rows.load(std::memory_order_relaxed);
}

int64_t col_compare_i64_auto(const int64_t *values, uint64_t n, int32_t op, int64_t threshold,
                                const uint64_t *validity, uint64_t *out) {
    if (values == nullptr || out == nullptr) {
        return -1;
    }
    COL_DISPATCH_SCALAR_I64(op, values, n, threshold, validity, out);
}

int64_t col_compare_f64_auto(const double *values, uint64_t n, int32_t op, double threshold,
                                const uint64_t *validity, uint64_t *out) {
    if (values == nullptr || out == nullptr) {
        return -1;
    }
    COL_DISPATCH_SCALAR_F64(op, values, n, threshold, validity, out);
}

} /* extern "C" */
