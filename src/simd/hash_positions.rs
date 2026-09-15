const HASH_MULTIPLIER: u32 = 0x9E37_79B1;

pub fn hash_four_positions(input: &[u8], position: usize, hash_log: u32) -> [u32; 4] {
    debug_assert!(position + 7 < input.len());

    #[cfg(target_arch = "aarch64")]
    {
        hash_four_positions_neon(input, position, hash_log)
    }
    #[cfg(all(target_arch = "x86_64", target_feature = "sse4.1"))]
    {
        hash_four_positions_sse2(input, position, hash_log)
    }
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    {
        hash_four_positions_simd128(input, position, hash_log)
    }
    #[cfg(not(any(
        target_arch = "aarch64",
        all(target_arch = "x86_64", target_feature = "sse4.1"),
        all(target_arch = "wasm32", target_feature = "simd128")
    )))]
    {
        hash_four_positions_scalar(input, position, hash_log)
    }
}

fn read_unaligned_u32_le(input: &[u8], position: usize) -> u32 {
    u32::from_le_bytes([
        input[position],
        input[position + 1],
        input[position + 2],
        input[position + 3],
    ])
}

fn hash_four_positions_scalar(input: &[u8], position: usize, hash_log: u32) -> [u32; 4] {
    let shift = 32 - hash_log;
    [
        read_unaligned_u32_le(input, position).wrapping_mul(HASH_MULTIPLIER) >> shift,
        read_unaligned_u32_le(input, position + 1).wrapping_mul(HASH_MULTIPLIER) >> shift,
        read_unaligned_u32_le(input, position + 2).wrapping_mul(HASH_MULTIPLIER) >> shift,
        read_unaligned_u32_le(input, position + 3).wrapping_mul(HASH_MULTIPLIER) >> shift,
    ]
}

#[cfg(target_arch = "aarch64")]
fn hash_four_positions_neon(input: &[u8], position: usize, hash_log: u32) -> [u32; 4] {
    use core::arch::aarch64::{
        vdupq_n_s32, vdupq_n_u32, vld1q_u32, vmulq_u32, vshlq_u32, vst1q_u32,
    };

    let shift = 32 - hash_log;
    let lanes: [u32; 4] = [
        read_unaligned_u32_le(input, position),
        read_unaligned_u32_le(input, position + 1),
        read_unaligned_u32_le(input, position + 2),
        read_unaligned_u32_le(input, position + 3),
    ];

    let mut result = [0u32; 4];
    unsafe {
        let values = vld1q_u32(lanes.as_ptr());
        let multiplier = vdupq_n_u32(HASH_MULTIPLIER);
        let multiplied = vmulq_u32(values, multiplier);
        let shift_amount = vdupq_n_s32(-(shift as i32));
        let shifted = vshlq_u32(multiplied, shift_amount);
        vst1q_u32(result.as_mut_ptr(), shifted);
    }
    result
}

#[cfg(all(target_arch = "x86_64", target_feature = "sse4.1"))]
fn hash_four_positions_sse2(input: &[u8], position: usize, hash_log: u32) -> [u32; 4] {
    use core::arch::x86_64::{
        __m128i, _mm_cvtsi32_si128, _mm_mullo_epi32, _mm_set_epi32, _mm_set1_epi32, _mm_srl_epi32,
        _mm_storeu_si128,
    };

    let shift = 32 - hash_log;
    let lanes: [u32; 4] = [
        read_unaligned_u32_le(input, position),
        read_unaligned_u32_le(input, position + 1),
        read_unaligned_u32_le(input, position + 2),
        read_unaligned_u32_le(input, position + 3),
    ];

    let mut result = [0u32; 4];
    unsafe {
        let values = _mm_set_epi32(
            lanes[3] as i32,
            lanes[2] as i32,
            lanes[1] as i32,
            lanes[0] as i32,
        );
        let multiplier = _mm_set1_epi32(HASH_MULTIPLIER as i32);
        let multiplied = _mm_mullo_epi32(values, multiplier);
        let shift_amount = _mm_cvtsi32_si128(shift as i32);
        let shifted = _mm_srl_epi32(multiplied, shift_amount);
        _mm_storeu_si128(result.as_mut_ptr() as *mut __m128i, shifted);
    }
    result
}

#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
fn hash_four_positions_simd128(input: &[u8], position: usize, hash_log: u32) -> [u32; 4] {
    use core::arch::wasm32::{u32x4, u32x4_mul, u32x4_shr, v128};

    let shift = 32 - hash_log;
    let lanes: [u32; 4] = [
        read_unaligned_u32_le(input, position),
        read_unaligned_u32_le(input, position + 1),
        read_unaligned_u32_le(input, position + 2),
        read_unaligned_u32_le(input, position + 3),
    ];

    let mut result = [0u32; 4];
    unsafe {
        let values: v128 = u32x4(lanes[0], lanes[1], lanes[2], lanes[3]);
        let multiplier: v128 = u32x4(
            HASH_MULTIPLIER,
            HASH_MULTIPLIER,
            HASH_MULTIPLIER,
            HASH_MULTIPLIER,
        );
        let multiplied = u32x4_mul(values, multiplier);
        let shifted = u32x4_shr(multiplied, shift);
        core::ptr::write_unaligned(result.as_mut_ptr() as *mut v128, shifted);
    }
    result
}

pub fn run_self_tests() -> Option<u32> {
    let mut input = [0u8; 64];
    let mut state: u32 = 0x1234_5678;

    let mut fill_index = 0usize;
    while fill_index < input.len() {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        input[fill_index] = (state & 0xFF) as u8;
        fill_index += 1;
    }

    let mut test_index = 0u32;

    let mut hash_log = 10u32;
    while hash_log <= 22 {
        let mut position = 0usize;
        while position + 7 < input.len() {
            let expected = hash_four_positions_scalar(&input, position, hash_log);
            let actual = hash_four_positions(&input, position, hash_log);
            if actual != expected {
                return Some(test_index);
            }
            test_index += 1;
            position += 3;
        }
        hash_log += 4;
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

    fn assert_all_bodies_agree(input: &[u8], position: usize, hash_log: u32) {
        let expected = hash_four_positions_scalar(input, position, hash_log);

        assert_eq!(hash_four_positions(input, position, hash_log), expected);

        #[cfg(target_arch = "aarch64")]
        assert_eq!(
            hash_four_positions_neon(input, position, hash_log),
            expected
        );

        #[cfg(all(target_arch = "x86_64", target_feature = "sse4.1"))]
        assert_eq!(
            hash_four_positions_sse2(input, position, hash_log),
            expected
        );

        #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
        assert_eq!(
            hash_four_positions_simd128(input, position, hash_log),
            expected
        );
    }

    #[test]
    fn matches_calling_the_single_hash_four_times() {
        for &length in &[16usize, 1000, 65536] {
            for offset in 0..=15usize {
                let total = offset + length + 16;
                let input = generate_pseudo_random_bytes(
                    total,
                    0xA5A5_5A5A ^ length as u32 ^ offset as u32,
                );

                for hash_log in [10u32, 16, 20, 22] {
                    let position = offset;
                    if position + 7 >= input.len() {
                        continue;
                    }
                    assert_all_bodies_agree(&input, position, hash_log);
                }
            }
        }
    }

    #[test]
    fn matches_at_every_position_in_a_window() {
        let input = generate_pseudo_random_bytes(4096, 0xDEAD_BEEF);
        for position in 0..(input.len() - 7) {
            assert_all_bodies_agree(&input, position, 16);
        }
    }

    #[test]
    fn self_tests_pass() {
        assert_eq!(run_self_tests(), None);
    }
}
