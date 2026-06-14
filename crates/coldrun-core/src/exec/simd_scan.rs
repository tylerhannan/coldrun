//! SIMD-friendly column scans for fused GROUP BY kernels.

const UNROLL: usize = 32;
const BLOCK: usize = 8192;

/// Wide unrolled i32 scan — LLVM autovectorizes the inner loop.
#[inline]
pub fn for_each_i32_wide(slice: &[i32], mut f: impl FnMut(i32)) {
    let mut i = 0;
    let len = slice.len();
    while i + UNROLL <= len {
        f(slice[i]);
        f(slice[i + 1]);
        f(slice[i + 2]);
        f(slice[i + 3]);
        f(slice[i + 4]);
        f(slice[i + 5]);
        f(slice[i + 6]);
        f(slice[i + 7]);
        f(slice[i + 8]);
        f(slice[i + 9]);
        f(slice[i + 10]);
        f(slice[i + 11]);
        f(slice[i + 12]);
        f(slice[i + 13]);
        f(slice[i + 14]);
        f(slice[i + 15]);
        f(slice[i + 16]);
        f(slice[i + 17]);
        f(slice[i + 18]);
        f(slice[i + 19]);
        f(slice[i + 20]);
        f(slice[i + 21]);
        f(slice[i + 22]);
        f(slice[i + 23]);
        f(slice[i + 24]);
        f(slice[i + 25]);
        f(slice[i + 26]);
        f(slice[i + 27]);
        f(slice[i + 28]);
        f(slice[i + 29]);
        f(slice[i + 30]);
        f(slice[i + 31]);
        i += UNROLL;
    }
    while i < len {
        f(slice[i]);
        i += 1;
    }
}

#[inline(always)]
fn q41_row_ok(
    i: usize,
    referer_hash: i64,
    counter: i32,
    min_date: i32,
    max_date: i32,
    is_refresh: i16,
    referer: &[i64],
    counters: &[i32],
    dates: &[i32],
    refresh: &[i16],
    traffic: &[i16],
) -> bool {
    referer[i] == referer_hash
        && counters[i] == counter
        && {
            let d = dates[i];
            d >= min_date && d <= max_date
        }
        && refresh[i] == is_refresh
        && (traffic[i] == -1 || traffic[i] == 6)
}

/// Q41 zone scan: referer-first with SIMD prefilter, then remaining predicates.
pub fn for_each_q41_zone_match(
    start: usize,
    end: usize,
    referer_hash: i64,
    counter: i32,
    min_date: i32,
    max_date: i32,
    is_refresh: i16,
    referer: &[i64],
    counters: &[i32],
    dates: &[i32],
    refresh: &[i16],
    traffic: &[i16],
    mut on_match: impl FnMut(usize),
) {
    let mut i = start;
    while i < end {
        let block_end = end.min(i + BLOCK);
        i = scan_q41_block(
            i,
            block_end,
            referer_hash,
            counter,
            min_date,
            max_date,
            is_refresh,
            referer,
            counters,
            dates,
            refresh,
            traffic,
            &mut on_match,
        );
    }
}

#[inline]
fn scan_q41_block(
    start: usize,
    end: usize,
    referer_hash: i64,
    counter: i32,
    min_date: i32,
    max_date: i32,
    is_refresh: i16,
    referer: &[i64],
    counters: &[i32],
    dates: &[i32],
    refresh: &[i16],
    traffic: &[i16],
    on_match: &mut impl FnMut(usize),
) -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            return unsafe {
                scan_q41_block_avx2(
                    start,
                    end,
                    referer_hash,
                    counter,
                    min_date,
                    max_date,
                    is_refresh,
                    referer,
                    counters,
                    dates,
                    refresh,
                    traffic,
                    on_match,
                )
            };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        return unsafe {
            scan_q41_block_neon(
                start,
                end,
                referer_hash,
                counter,
                min_date,
                max_date,
                is_refresh,
                referer,
                counters,
                dates,
                refresh,
                traffic,
                on_match,
            )
        };
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        scan_q41_block_scalar(
            start,
            end,
            referer_hash,
            counter,
            min_date,
            max_date,
            is_refresh,
            referer,
            counters,
            dates,
            refresh,
            traffic,
            on_match,
        )
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn scan_q41_block_avx2(
    start: usize,
    end: usize,
    referer_hash: i64,
    counter: i32,
    min_date: i32,
    max_date: i32,
    is_refresh: i16,
    referer: &[i64],
    counters: &[i32],
    dates: &[i32],
    refresh: &[i16],
    traffic: &[i16],
    on_match: &mut impl FnMut(usize),
) -> usize {
    use std::arch::x86_64::*;

    let mut i = start;
    let target = _mm256_set1_epi64x(referer_hash);
    while i + 4 <= end {
        let v = _mm256_loadu_si256(referer.as_ptr().add(i) as *const __m256i);
        let eq = _mm256_cmpeq_epi64(v, target);
        let mask = _mm256_movemask_epi8(eq);
        if mask != 0 {
            for j in 0..4 {
                let idx = i + j;
                if q41_row_ok(
                    idx,
                    referer_hash,
                    counter,
                    min_date,
                    max_date,
                    is_refresh,
                    referer,
                    counters,
                    dates,
                    refresh,
                    traffic,
                ) {
                    on_match(idx);
                }
            }
        }
        i += 4;
    }
    scan_q41_block_scalar(
        i,
        end,
        referer_hash,
        counter,
        min_date,
        max_date,
        is_refresh,
        referer,
        counters,
        dates,
        refresh,
        traffic,
        on_match,
    )
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn scan_q41_block_neon(
    start: usize,
    end: usize,
    referer_hash: i64,
    counter: i32,
    min_date: i32,
    max_date: i32,
    is_refresh: i16,
    referer: &[i64],
    counters: &[i32],
    dates: &[i32],
    refresh: &[i16],
    traffic: &[i16],
    on_match: &mut impl FnMut(usize),
) -> usize {
    use std::arch::aarch64::*;

    let mut i = start;
    let target = vdupq_n_s64(referer_hash);
    while i + 2 <= end {
        let v = vld1q_s64(referer.as_ptr().add(i));
        let eq = vceqq_s64(v, target);
        if vgetq_lane_u64(eq, 0) != 0 || vgetq_lane_u64(eq, 1) != 0 {
            for j in 0..2 {
                let idx = i + j;
                if q41_row_ok(
                    idx,
                    referer_hash,
                    counter,
                    min_date,
                    max_date,
                    is_refresh,
                    referer,
                    counters,
                    dates,
                    refresh,
                    traffic,
                ) {
                    on_match(idx);
                }
            }
        }
        i += 2;
    }
    scan_q41_block_scalar(
        i,
        end,
        referer_hash,
        counter,
        min_date,
        max_date,
        is_refresh,
        referer,
        counters,
        dates,
        refresh,
        traffic,
        on_match,
    )
}

#[inline]
fn scan_q41_block_scalar(
    start: usize,
    end: usize,
    referer_hash: i64,
    counter: i32,
    min_date: i32,
    max_date: i32,
    is_refresh: i16,
    referer: &[i64],
    counters: &[i32],
    dates: &[i32],
    refresh: &[i16],
    traffic: &[i16],
    on_match: &mut impl FnMut(usize),
) -> usize {
    let mut i = start;
    while i < end {
        if q41_row_ok(
            i,
            referer_hash,
            counter,
            min_date,
            max_date,
            is_refresh,
            referer,
            counters,
            dates,
            refresh,
            traffic,
        ) {
            on_match(i);
        }
        i += 1;
    }
    end
}
