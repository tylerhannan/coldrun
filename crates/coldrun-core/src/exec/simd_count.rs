//! SIMD-friendly nonzero / nonempty counts (LLVM autovectorizes the inner loops).

#[inline]
pub fn count_i16_ne_zero(slice: &[i16]) -> u64 {
    count_ne_zero_impl(slice, |x| *x != 0)
}

#[inline]
pub fn count_i32_ne_zero(slice: &[i32]) -> u64 {
    count_ne_zero_impl(slice, |x| *x != 0)
}

#[inline]
pub fn count_i64_ne_zero(slice: &[i64]) -> u64 {
    count_ne_zero_impl(slice, |x| *x != 0)
}

#[inline]
#[allow(dead_code)]
pub fn count_utf8_nonempty(slice: &[String]) -> u64 {
    count_ne_zero_impl(slice, |s| !s.is_empty())
}

#[inline]
fn count_ne_zero_impl<T>(slice: &[T], pred: impl Fn(&T) -> bool) -> u64
{
    const CHUNK: usize = 32;
    let mut sum = 0u64;
    let mut i = 0;
    let len = slice.len();
    while i + CHUNK <= len {
        let mut acc = 0u32;
        let mut j = 0;
        while j < CHUNK {
            acc += u32::from(pred(&slice[i + j]));
            j += 1;
        }
        sum += u64::from(acc);
        i += CHUNK;
    }
    while i < len {
        sum += u64::from(pred(&slice[i]));
        i += 1;
    }
    sum
}
