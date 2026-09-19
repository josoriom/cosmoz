pub(crate) fn count_matching_bytes(first: &[u8], second: &[u8]) -> usize {
    #[cfg(target_arch = "aarch64")]
    {
        count_matching_bytes_neon(first, second)
    }
    #[cfg(target_arch = "x86_64")]
    {
        count_matching_bytes_sse2(first, second)
    }
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    {
        count_matching_bytes_simd128(first, second)
    }
    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "x86_64",
        all(target_arch = "wasm32", target_feature = "simd128")
    )))]
    {
        count_matching_bytes_scalar(first, second)
    }
}

#[allow(clippy::missing_safety_doc)]
pub unsafe fn count_matching_bytes_unchecked(
    first: *const u8,
    second: *const u8,
    limit: usize,
) -> usize {
    debug_assert!(!first.is_null());
    debug_assert!(!second.is_null());
    unsafe {
        let first_slice = core::slice::from_raw_parts(first, limit);
        let second_slice = core::slice::from_raw_parts(second, limit);
        count_matching_bytes(first_slice, second_slice)
    }
}

#[allow(clippy::missing_safety_doc)]
#[inline(always)]
pub unsafe fn count_matching_bytes_by_words_unchecked(
    first: *const u8,
    second: *const u8,
    limit: usize,
) -> usize {
    unsafe {
        let first_slice = core::slice::from_raw_parts(first, limit);
        let second_slice = core::slice::from_raw_parts(second, limit);
        count_matching_bytes_scalar(first_slice, second_slice)
    }
}

#[inline(always)]
fn count_matching_bytes_scalar(first: &[u8], second: &[u8]) -> usize {
    let limit = first.len().min(second.len());
    let mut matched = 0usize;

    while matched + 8 <= limit {
        let first_word = u64::from_le_bytes(first[matched..matched + 8].try_into().unwrap());
        let second_word = u64::from_le_bytes(second[matched..matched + 8].try_into().unwrap());
        let difference = first_word ^ second_word;
        if difference != 0 {
            return matched + (difference.trailing_zeros() as usize) / 8;
        }
        matched += 8;
    }

    while matched < limit && first[matched] == second[matched] {
        matched += 1;
    }

    matched
}

#[cfg(target_arch = "aarch64")]
fn count_matching_bytes_neon(first: &[u8], second: &[u8]) -> usize {
    use core::arch::aarch64::{
        vceqq_u8, vget_lane_u64, vld1q_u8, vreinterpret_u64_u8, vreinterpretq_u16_u8, vshrn_n_u16,
    };

    let limit = first.len().min(second.len());
    let mut matched = 0usize;

    while matched + 16 <= limit {
        let mask = unsafe {
            let first_chunk = vld1q_u8(first.as_ptr().add(matched));
            let second_chunk = vld1q_u8(second.as_ptr().add(matched));
            let equal = vceqq_u8(first_chunk, second_chunk);
            let narrowed = vshrn_n_u16::<4>(vreinterpretq_u16_u8(equal));
            vget_lane_u64::<0>(vreinterpret_u64_u8(narrowed))
        };
        if mask != u64::MAX {
            return matched + ((!mask).trailing_zeros() as usize) / 4;
        }
        matched += 16;
    }

    matched + count_matching_bytes_scalar(&first[matched..limit], &second[matched..limit])
}

#[cfg(target_arch = "x86_64")]
fn count_matching_bytes_sse2(first: &[u8], second: &[u8]) -> usize {
    use core::arch::x86_64::{__m128i, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8};

    let limit = first.len().min(second.len());
    let mut matched = 0usize;

    while matched + 16 <= limit {
        let mask = unsafe {
            let first_chunk = _mm_loadu_si128(first.as_ptr().add(matched) as *const __m128i);
            let second_chunk = _mm_loadu_si128(second.as_ptr().add(matched) as *const __m128i);
            let equal = _mm_cmpeq_epi8(first_chunk, second_chunk);
            _mm_movemask_epi8(equal) as u32
        };
        if mask != 0xFFFF {
            return matched + mask.trailing_ones() as usize;
        }
        matched += 16;
    }

    matched + count_matching_bytes_scalar(&first[matched..limit], &second[matched..limit])
}

#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
fn count_matching_bytes_simd128(first: &[u8], second: &[u8]) -> usize {
    use core::arch::wasm32::{u8x16_bitmask, u8x16_eq, v128, v128_load};

    let limit = first.len().min(second.len());
    let mut matched = 0usize;

    while matched + 16 <= limit {
        let mask = unsafe {
            let first_chunk = v128_load(first.as_ptr().add(matched) as *const v128);
            let second_chunk = v128_load(second.as_ptr().add(matched) as *const v128);
            let equal = u8x16_eq(first_chunk, second_chunk);
            u8x16_bitmask(equal) as u32
        };
        if mask != 0xFFFF {
            return matched + mask.trailing_ones() as usize;
        }
        matched += 16;
    }

    matched + count_matching_bytes_scalar(&first[matched..limit], &second[matched..limit])
}

#[cfg(any(test, feature = "wasm-exports"))]
pub(crate) fn run_self_tests() -> Option<u32> {
    let mut first = [0u8; 96];
    let mut second = [0u8; 96];
    let mut state: u32 = 0x2545_F491;

    let mut fill_index = 0usize;
    while fill_index < first.len() {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        first[fill_index] = (state & 0xFF) as u8;
        second[fill_index] = (state & 0xFF) as u8;
        fill_index += 1;
    }

    let mut test_index = 0u32;

    let mut offset = 0usize;
    while offset <= 15 {
        let length = first.len() - offset - 16;

        let expected = count_matching_bytes_scalar(
            &first[offset..offset + length],
            &second[offset..offset + length],
        );
        let actual = count_matching_bytes(
            &first[offset..offset + length],
            &second[offset..offset + length],
        );
        if actual != expected {
            return Some(test_index);
        }
        test_index += 1;

        let mut diff_position = 0usize;
        while diff_position < length && diff_position <= 40 {
            let mut second_copy = second;
            second_copy[offset + diff_position] ^= 0xFF;

            let expected = count_matching_bytes_scalar(
                &first[offset..offset + length],
                &second_copy[offset..offset + length],
            );
            let actual = count_matching_bytes(
                &first[offset..offset + length],
                &second_copy[offset..offset + length],
            );
            if actual != expected || expected != diff_position {
                return Some(test_index);
            }
            test_index += 1;
            diff_position += 7;
        }

        offset += 1;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next_pseudo_random_number(state: &mut u32) -> u32 {
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        *state
    }

    fn generate_pseudo_random_bytes(length: usize, seed: u32) -> Vec<u8> {
        let mut state = seed;
        let mut bytes = Vec::with_capacity(length);
        while bytes.len() < length {
            let value = next_pseudo_random_number(&mut state);
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.truncate(length);
        bytes
    }

    fn assert_all_bodies_agree(first: &[u8], second: &[u8]) {
        let expected = count_matching_bytes_scalar(first, second);

        assert_eq!(count_matching_bytes(first, second), expected);

        #[cfg(target_arch = "aarch64")]
        assert_eq!(count_matching_bytes_neon(first, second), expected);

        #[cfg(target_arch = "x86_64")]
        assert_eq!(count_matching_bytes_sse2(first, second), expected);

        #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
        assert_eq!(count_matching_bytes_simd128(first, second), expected);
    }

    fn lengths() -> Vec<usize> {
        let mut values: Vec<usize> = (0..=100).collect();
        values.push(1000);
        values.push(65536);
        values
    }

    #[test]
    fn agrees_with_scalar_when_no_difference() {
        let padding = 16 + 65536;
        for &length in &lengths() {
            for offset in 0..=15usize {
                let total = offset + length + padding;
                let first = generate_pseudo_random_bytes(
                    total,
                    0x1234_5678 ^ length as u32 ^ offset as u32,
                );
                let second = first.clone();
                assert_all_bodies_agree(
                    &first[offset..offset + length],
                    &second[offset..offset + length],
                );
            }
        }
    }

    #[test]
    fn agrees_with_scalar_with_first_difference_at_every_early_position() {
        for length in 1..=100usize {
            for offset in 0..=15usize {
                for diff_position in 0..length.min(71) {
                    let total = offset + length + 16;
                    let first = generate_pseudo_random_bytes(
                        total,
                        0x9E37_79B1
                            ^ (length as u32)
                            ^ (offset as u32).wrapping_shl(8)
                            ^ (diff_position as u32).wrapping_shl(16),
                    );
                    let mut second = first.clone();
                    second[offset + diff_position] ^= 0xFF;

                    assert_all_bodies_agree(
                        &first[offset..offset + length],
                        &second[offset..offset + length],
                    );
                }
            }
        }
    }

    #[test]
    fn agrees_with_scalar_for_large_lengths_with_a_difference() {
        for &length in &[1000usize, 65536usize] {
            for offset in 0..=15usize {
                for &diff_position in &[0usize, 1, 15, 63, 64, 65, length - 1] {
                    let total = offset + length + 16;
                    let first = generate_pseudo_random_bytes(
                        total,
                        0xC0FF_EE00 ^ (length as u32) ^ (offset as u32),
                    );
                    let mut second = first.clone();
                    second[offset + diff_position] ^= 0xFF;

                    assert_all_bodies_agree(
                        &first[offset..offset + length],
                        &second[offset..offset + length],
                    );
                }
            }
        }
    }

    #[test]
    fn no_match_at_byte_zero() {
        let first = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let second = [9u8, 2, 3, 4, 5, 6, 7, 8];
        assert_all_bodies_agree(&first, &second);
        assert_eq!(count_matching_bytes(&first, &second), 0);
    }

    #[test]
    fn match_to_end_of_shorter_slice() {
        let first = generate_pseudo_random_bytes(200, 0xABCD_1234);
        let mut second = first.clone();
        second.truncate(120);
        assert_all_bodies_agree(&first[..120], &second);
        assert_eq!(count_matching_bytes(&first[..120], &second), 120);
    }

    #[test]
    fn slices_of_different_lengths() {
        let first = generate_pseudo_random_bytes(50, 0x1111_2222);
        let second = generate_pseudo_random_bytes(90, 0x1111_2222);
        assert_all_bodies_agree(&first, &second);
    }

    #[test]
    fn self_tests_pass() {
        assert_eq!(run_self_tests(), None);
    }

    #[test]
    fn unchecked_entry_point_agrees_with_the_safe_function() {
        for &length in &lengths() {
            let first = generate_pseudo_random_bytes(length + 32, 0x5EED_0001 ^ length as u32);
            let mut second = first.clone();
            if length > 0 {
                second[length / 2] ^= 0xFF;
            }
            let expected = count_matching_bytes(&first[..length], &second[..length]);
            let actual =
                unsafe { count_matching_bytes_unchecked(first.as_ptr(), second.as_ptr(), length) };
            assert_eq!(actual, expected);
        }
    }
}
