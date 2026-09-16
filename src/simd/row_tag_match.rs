pub fn match_row_tags16(tags: &[u8; 16], target: u8) -> u16 {
    #[cfg(target_arch = "aarch64")]
    {
        match_row_tags16_neon(tags, target)
    }
    #[cfg(target_arch = "x86_64")]
    {
        match_row_tags16_sse2(tags, target)
    }
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    {
        match_row_tags16_simd128(tags, target)
    }
    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "x86_64",
        all(target_arch = "wasm32", target_feature = "simd128")
    )))]
    {
        match_row_tags16_scalar(tags, target)
    }
}

pub fn match_row_tags32(tags: &[u8; 32], target: u8) -> u32 {
    let mut low = [0u8; 16];
    let mut high = [0u8; 16];
    low.copy_from_slice(&tags[..16]);
    high.copy_from_slice(&tags[16..]);
    let low_mask = match_row_tags16(&low, target) as u32;
    let high_mask = match_row_tags16(&high, target) as u32;
    low_mask | (high_mask << 16)
}

fn match_row_tags16_scalar(tags: &[u8; 16], target: u8) -> u16 {
    let mut mask = 0u16;
    let mut index = 0usize;
    while index < 16 {
        if tags[index] == target {
            mask |= 1 << index;
        }
        index += 1;
    }
    mask
}

#[cfg(target_arch = "aarch64")]
fn match_row_tags16_neon(tags: &[u8; 16], target: u8) -> u16 {
    use core::arch::aarch64::{
        vandq_u8, vceqq_u8, vdupq_n_u8, vget_high_u8, vget_lane_u16, vget_low_u8, vld1q_u8,
        vpadd_u8, vreinterpret_u16_u8,
    };

    unsafe {
        let values = vld1q_u8(tags.as_ptr());
        let targets = vdupq_n_u8(target);
        let equal = vceqq_u8(values, targets);

        let bit_positions: [u8; 16] = [1, 2, 4, 8, 16, 32, 64, 128, 1, 2, 4, 8, 16, 32, 64, 128];
        let bit_mask = vld1q_u8(bit_positions.as_ptr());
        let masked = vandq_u8(equal, bit_mask);

        let low = vget_low_u8(masked);
        let high = vget_high_u8(masked);
        let sum0 = vpadd_u8(low, high);
        let sum1 = vpadd_u8(sum0, sum0);
        let sum2 = vpadd_u8(sum1, sum1);
        vget_lane_u16::<0>(vreinterpret_u16_u8(sum2))
    }
}

#[cfg(target_arch = "x86_64")]
fn match_row_tags16_sse2(tags: &[u8; 16], target: u8) -> u16 {
    use core::arch::x86_64::{
        __m128i, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_set1_epi8,
    };

    unsafe {
        let values = _mm_loadu_si128(tags.as_ptr() as *const __m128i);
        let targets = _mm_set1_epi8(target as i8);
        let equal = _mm_cmpeq_epi8(values, targets);
        _mm_movemask_epi8(equal) as u16
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
fn match_row_tags16_simd128(tags: &[u8; 16], target: u8) -> u16 {
    use core::arch::wasm32::{u8x16_bitmask, u8x16_eq, u8x16_splat, v128, v128_load};

    unsafe {
        let values = v128_load(tags.as_ptr() as *const v128);
        let targets = u8x16_splat(target);
        let equal = u8x16_eq(values, targets);
        u8x16_bitmask(equal)
    }
}

pub fn run_self_tests() -> Option<u32> {
    let mut tags = [0u8; 32];
    let mut state: u32 = 0x1357_9BDF;
    let mut fill_index = 0usize;
    while fill_index < tags.len() {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        tags[fill_index] = (state & 0xFF) as u8;
        fill_index += 1;
    }

    let mut test_index = 0u32;
    let mut target_value = 0u32;
    while target_value <= 255 {
        let target = target_value as u8;
        let mut small = [0u8; 16];
        small.copy_from_slice(&tags[..16]);
        let expected16 = match_row_tags16_scalar(&small, target);
        if match_row_tags16(&small, target) != expected16 {
            return Some(test_index);
        }
        test_index += 1;

        let mut expected32 = 0u32;
        let mut index = 0usize;
        while index < 32 {
            if tags[index] == target {
                expected32 |= 1 << index;
            }
            index += 1;
        }
        if match_row_tags32(&tags, target) != expected32 {
            return Some(test_index);
        }
        test_index += 1;

        target_value += 1;
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

    fn generate_pseudo_random_tags(seed: u32) -> [u8; 32] {
        let mut state = seed;
        let mut tags = [0u8; 32];
        let mut index = 0usize;
        while index < tags.len() {
            tags[index] = (next_pseudo_random_number(&mut state) & 0xFF) as u8;
            index += 1;
        }
        tags
    }

    fn assert_all_bodies_agree16(tags: &[u8; 16], target: u8) {
        let expected = match_row_tags16_scalar(tags, target);
        assert_eq!(match_row_tags16(tags, target), expected);

        #[cfg(target_arch = "aarch64")]
        assert_eq!(match_row_tags16_neon(tags, target), expected);

        #[cfg(target_arch = "x86_64")]
        assert_eq!(match_row_tags16_sse2(tags, target), expected);

        #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
        assert_eq!(match_row_tags16_simd128(tags, target), expected);
    }

    #[test]
    fn agrees_with_scalar_across_random_rows() {
        for seed in [0x1111_2222u32, 0x3333_4444, 0xAAAA_5555, 0xCAFE_BABE] {
            let tags = generate_pseudo_random_tags(seed);
            let mut small = [0u8; 16];
            small.copy_from_slice(&tags[..16]);
            for target in 0..=255u8 {
                assert_all_bodies_agree16(&small, target);
            }
        }
    }

    #[test]
    fn matches_every_lane_position() {
        for lane in 0..16usize {
            let mut tags = [1u8; 16];
            tags[lane] = 9;
            assert_all_bodies_agree16(&tags, 9);
            assert_eq!(match_row_tags16(&tags, 9), 1u16 << lane);
        }
    }

    #[test]
    fn no_matches_gives_zero_mask() {
        let tags = [1u8; 16];
        assert_all_bodies_agree16(&tags, 2);
        assert_eq!(match_row_tags16(&tags, 2), 0);
    }

    #[test]
    fn all_matches_gives_full_mask() {
        let tags = [7u8; 16];
        assert_all_bodies_agree16(&tags, 7);
        assert_eq!(match_row_tags16(&tags, 7), 0xFFFF);
    }

    #[test]
    fn combines_two_halves_for_32_lanes() {
        for lane in 0..32usize {
            let mut tags = [1u8; 32];
            tags[lane] = 42;
            assert_eq!(match_row_tags32(&tags, 42), 1u32 << lane);
        }
    }

    #[test]
    fn self_tests_pass() {
        assert_eq!(run_self_tests(), None);
    }
}
