/*
 * scalar_impl.h -- the branchless scalar reference, as a header.
 *
 * The header exists so that the *same* scalar implementation can be compiled
 * twice, with different compiler flags, into two different object files:
 *
 *   - `comparison.cpp` includes it normally, and the compiler auto-vectorises
 *     the loop. That is what a normal build of this reference produces, and
 *     it is a surprisingly strong baseline.
 *   - `scalar.cpp` includes it and is compiled with `-fno-vectorize`. That is
 *     the reference with the vectorisation taken away.
 *
 * Publishing both is the difference between "SIMD is Nx faster" -- which
 * would be measured against a baseline the compiler had already vectorised,
 * and would therefore be a claim about nothing -- and a measurement that
 * says two things at once:
 *
 *   1. how much vectorisation is worth, and
 *   2. whether the hand-written kernel beats what the compiler does with
 *      the same loop.
 *
 * The second question is the interesting one, and it does not have a
 * predetermined answer.
 *
 * The loop itself is branchless: the predicate is evaluated unconditionally
 * and folded into the mask with a bit-or, so the loop carries no
 * data-dependent branch. The SIMD paths do exactly the same thing, wider.
 *
 * This header deliberately contains no SIMD code and no compiler flags. The
 * flags live in `build.rs`, where they are applied to one translation unit
 * and not the other -- a flag cannot be attached to a header.
 */

#ifndef COL_SCALAR_IMPL_H
#define COL_SCALAR_IMPL_H

/* For the COL_CMP_* operator codes. */
#include "comparison.h"

#include <stdint.h>

namespace col {

inline int64_t popcount64(uint64_t v) {
#if defined(__GNUC__) || defined(__clang__)
    return static_cast<int64_t>(__builtin_popcountll(v));
#else
    int64_t count = 0;
    while (v) {
        v &= v - 1;
        ++count;
    }
    return count;
#endif
}

template <int OP>
inline bool cmp_i64(int64_t a, int64_t th) {
    if constexpr (OP == COL_CMP_LT) {
        return a < th;
    } else if constexpr (OP == COL_CMP_LE) {
        return a <= th;
    } else if constexpr (OP == COL_CMP_GT) {
        return a > th;
    } else if constexpr (OP == COL_CMP_GE) {
        return a >= th;
    } else if constexpr (OP == COL_CMP_EQ) {
        return a == th;
    } else {
        return a != th;
    }
}

template <int OP>
inline bool cmp_f64(double a, double th) {
    if constexpr (OP == COL_CMP_LT) {
        return a < th;
    } else if constexpr (OP == COL_CMP_LE) {
        return a <= th;
    } else if constexpr (OP == COL_CMP_GT) {
        return a > th;
    } else if constexpr (OP == COL_CMP_GE) {
        return a >= th;
    } else if constexpr (OP == COL_CMP_EQ) {
        return a == th;
    } else {
        /* Ordered not-equal: false when either operand is NaN. */
        return a < th || a > th;
    }
}

template <int OP>
int64_t scalar_i64(const int64_t *values, uint64_t n, int64_t th, const uint64_t *validity,
                   uint64_t *out) {
    const uint64_t blocks = (n + 63) / 64;
    int64_t total = 0;
    for (uint64_t b = 0; b < blocks; ++b) {
        const uint64_t base = b * 64;
        const uint64_t upto = (base + 64 <= n) ? 64 : (n - base);
        uint64_t mask = 0;
        for (uint64_t i = 0; i < upto; ++i) {
            /* The comparison is evaluated unconditionally and folded into the
             * mask with a bit-or, so the loop carries no data-dependent
             * branch. The SIMD paths do exactly the same thing 4 (or 2) rows
             * at a time; the only difference between them is width. */
            const bool m = cmp_i64<OP>(values[base + i], th);
            mask |= static_cast<uint64_t>(m) << i;
        }
        if (validity != nullptr) {
            mask &= validity[b];
        }
        out[b] = mask;
        total += popcount64(mask);
    }
    return total;
}

template <int OP>
int64_t scalar_f64(const double *values, uint64_t n, double th, const uint64_t *validity,
                   uint64_t *out) {
    const uint64_t blocks = (n + 63) / 64;
    int64_t total = 0;
    for (uint64_t b = 0; b < blocks; ++b) {
        const uint64_t base = b * 64;
        const uint64_t upto = (base + 64 <= n) ? 64 : (n - base);
        uint64_t mask = 0;
        for (uint64_t i = 0; i < upto; ++i) {
            const bool m = cmp_f64<OP>(values[base + i], th);
            mask |= static_cast<uint64_t>(m) << i;
        }
        if (validity != nullptr) {
            mask &= validity[b];
        }
        out[b] = mask;
        total += popcount64(mask);
    }
    return total;
}

} /* namespace col */

/*
 * The operator dispatch macros. Each arm instantiates the templated kernel
 * with a compile-time operator, so the inner loop has no switch in it.
 *
 * They live here rather than in `comparison.cpp` because both translation
 * units need them: `comparison.cpp` dispatches to its own (auto-vectorised)
 * copy, and `scalar.cpp` dispatches to the copy compiled without
 * vectorisation.
 */
#define COL_DISPATCH_SCALAR_I64(OPVAR, ...)                                   \
    switch (OPVAR) {                                                       \
    case COL_CMP_LT:                                                       \
        return col::scalar_i64<COL_CMP_LT>(__VA_ARGS__);                   \
    case COL_CMP_LE:                                                       \
        return col::scalar_i64<COL_CMP_LE>(__VA_ARGS__);                   \
    case COL_CMP_GT:                                                       \
        return col::scalar_i64<COL_CMP_GT>(__VA_ARGS__);                   \
    case COL_CMP_GE:                                                       \
        return col::scalar_i64<COL_CMP_GE>(__VA_ARGS__);                   \
    case COL_CMP_EQ:                                                       \
        return col::scalar_i64<COL_CMP_EQ>(__VA_ARGS__);                   \
    case COL_CMP_NE:                                                       \
        return col::scalar_i64<COL_CMP_NE>(__VA_ARGS__);                   \
    default:                                                               \
        return -1;                                                         \
    }

#define COL_DISPATCH_SCALAR_F64(OPVAR, ...)                                   \
    switch (OPVAR) {                                                       \
    case COL_CMP_LT:                                                       \
        return col::scalar_f64<COL_CMP_LT>(__VA_ARGS__);                   \
    case COL_CMP_LE:                                                       \
        return col::scalar_f64<COL_CMP_LE>(__VA_ARGS__);                   \
    case COL_CMP_GT:                                                       \
        return col::scalar_f64<COL_CMP_GT>(__VA_ARGS__);                   \
    case COL_CMP_GE:                                                       \
        return col::scalar_f64<COL_CMP_GE>(__VA_ARGS__);                   \
    case COL_CMP_EQ:                                                       \
        return col::scalar_f64<COL_CMP_EQ>(__VA_ARGS__);                   \
    case COL_CMP_NE:                                                       \
        return col::scalar_f64<COL_CMP_NE>(__VA_ARGS__);                   \
    default:                                                               \
        return -1;                                                         \
    }

#endif /* COL_SCALAR_IMPL_H */
