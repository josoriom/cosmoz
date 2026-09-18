const PRIME_32_1: u32 = 0x9E3779B1;

pub(crate) fn accumulate_stripe(accumulators: &mut [u64; 8], stripe: &[u8; 64], secret: &[u8; 64]) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        accumulate_stripe_neon(accumulators, stripe, secret)
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        accumulate_stripe_sse2(accumulators, stripe, secret)
    }
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    unsafe {
        accumulate_stripe_simd128(accumulators, stripe, secret)
    }
    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "x86_64",
        all(target_arch = "wasm32", target_feature = "simd128")
    )))]
    accumulate_stripe_scalar(accumulators, stripe, secret);
}

pub(crate) fn accumulate_stripes(
    accumulators: &mut [u64; 8],
    input: &[u8],
    secret: &[u8],
    stripe_count: usize,
) {
    let mut stripe_index = 0usize;
    while stripe_index < stripe_count {
        let stripe_start = stripe_index * 64;
        let secret_start = stripe_index * 8;
        let stripe: &[u8; 64] = input[stripe_start..stripe_start + 64].try_into().unwrap();
        let secret_window: &[u8; 64] = secret[secret_start..secret_start + 64].try_into().unwrap();
        accumulate_stripe(accumulators, stripe, secret_window);
        stripe_index += 1;
    }
}

pub(crate) fn scramble_accumulators(accumulators: &mut [u64; 8], secret: &[u8; 64]) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        scramble_accumulators_neon(accumulators, secret)
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        scramble_accumulators_sse2(accumulators, secret)
    }
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    unsafe {
        scramble_accumulators_simd128(accumulators, secret)
    }
    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "x86_64",
        all(target_arch = "wasm32", target_feature = "simd128")
    )))]
    scramble_accumulators_scalar(accumulators, secret);
}

#[allow(dead_code)]
fn read_u64_le(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

#[allow(dead_code)]
fn accumulate_stripe_scalar(accumulators: &mut [u64; 8], stripe: &[u8; 64], secret: &[u8; 64]) {
    let mut lane = 0usize;
    while lane < 8 {
        let data_value = read_u64_le(stripe, lane * 8);
        let data_key = data_value ^ read_u64_le(secret, lane * 8);
        let opposite_lane = lane ^ 1;
        accumulators[opposite_lane] = accumulators[opposite_lane].wrapping_add(data_value);
        let low_half = data_key & 0xFFFF_FFFF;
        let high_half = data_key >> 32;
        accumulators[lane] = accumulators[lane].wrapping_add(low_half.wrapping_mul(high_half));
        lane += 1;
    }
}

#[allow(dead_code)]
fn scramble_accumulators_scalar(accumulators: &mut [u64; 8], secret: &[u8; 64]) {
    let mut lane = 0usize;
    while lane < 8 {
        let key = read_u64_le(secret, lane * 8);
        let mut value = accumulators[lane];
        value ^= value >> 47;
        value ^= key;
        value = value.wrapping_mul(PRIME_32_1 as u64);
        accumulators[lane] = value;
        lane += 1;
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn accumulate_stripe_neon(
    accumulators: &mut [u64; 8],
    stripe: &[u8; 64],
    secret: &[u8; 64],
) {
    use core::arch::aarch64::{
        vaddq_u64, veorq_u64, vextq_u64, vld1q_u8, vld1q_u64, vmovn_u64, vmull_u32,
        vreinterpretq_u64_u8, vshrn_n_u64, vst1q_u64,
    };
    let accumulator_pointer = accumulators.as_mut_ptr();
    let mut block = 0usize;
    while block < 4 {
        let offset = block * 16;
        unsafe {
            let accumulator_vector = vld1q_u64(accumulator_pointer.add(block * 2));
            let data_vector = vreinterpretq_u64_u8(vld1q_u8(stripe.as_ptr().add(offset)));
            let key_vector = vreinterpretq_u64_u8(vld1q_u8(secret.as_ptr().add(offset)));
            let data_key = veorq_u64(data_vector, key_vector);
            let data_key_low = vmovn_u64(data_key);
            let data_key_high = vshrn_n_u64(data_key, 32);
            let product = vmull_u32(data_key_low, data_key_high);
            let data_swapped = vextq_u64(data_vector, data_vector, 1);
            let sum = vaddq_u64(accumulator_vector, data_swapped);
            let result = vaddq_u64(sum, product);
            vst1q_u64(accumulator_pointer.add(block * 2), result);
        }
        block += 1;
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn scramble_accumulators_neon(accumulators: &mut [u64; 8], secret: &[u8; 64]) {
    use core::arch::aarch64::{
        vaddq_u64, vdup_n_u32, veorq_u64, vld1q_u8, vld1q_u64, vmovn_u64, vmull_u32,
        vreinterpretq_u64_u8, vshlq_n_u64, vshrn_n_u64, vshrq_n_u64, vst1q_u64,
    };
    let accumulator_pointer = accumulators.as_mut_ptr();
    let prime = unsafe { vdup_n_u32(PRIME_32_1) };
    let mut block = 0usize;
    while block < 4 {
        let offset = block * 16;
        unsafe {
            let accumulator_vector = vld1q_u64(accumulator_pointer.add(block * 2));
            let shifted = vshrq_n_u64(accumulator_vector, 47);
            let data_shifted = veorq_u64(accumulator_vector, shifted);
            let key_vector = vreinterpretq_u64_u8(vld1q_u8(secret.as_ptr().add(offset)));
            let data_key = veorq_u64(data_shifted, key_vector);
            let data_key_low = vmovn_u64(data_key);
            let data_key_high = vshrn_n_u64(data_key, 32);
            let product_low = vmull_u32(data_key_low, prime);
            let product_high = vmull_u32(data_key_high, prime);
            let result = vaddq_u64(product_low, vshlq_n_u64(product_high, 32));
            vst1q_u64(accumulator_pointer.add(block * 2), result);
        }
        block += 1;
    }
}

#[cfg(target_arch = "x86_64")]
const fn mm_shuffle(third: i32, second: i32, first: i32, zeroth: i32) -> i32 {
    (third << 6) | (second << 4) | (first << 2) | zeroth
}

#[cfg(target_arch = "x86_64")]
unsafe fn accumulate_stripe_sse2(
    accumulators: &mut [u64; 8],
    stripe: &[u8; 64],
    secret: &[u8; 64],
) {
    use core::arch::x86_64::{
        __m128i, _mm_add_epi64, _mm_loadu_si128, _mm_mul_epu32, _mm_shuffle_epi32,
        _mm_storeu_si128, _mm_xor_si128,
    };
    let accumulator_pointer = accumulators.as_mut_ptr() as *mut __m128i;
    let mut block = 0usize;
    while block < 4 {
        let offset = block * 16;
        unsafe {
            let accumulator_vector = _mm_loadu_si128(accumulator_pointer.add(block));
            let data_vector = _mm_loadu_si128(stripe.as_ptr().add(offset) as *const __m128i);
            let key_vector = _mm_loadu_si128(secret.as_ptr().add(offset) as *const __m128i);
            let data_key = _mm_xor_si128(data_vector, key_vector);
            let data_key_low = _mm_shuffle_epi32(data_key, mm_shuffle(0, 3, 0, 1));
            let product = _mm_mul_epu32(data_key, data_key_low);
            let data_swapped = _mm_shuffle_epi32(data_vector, mm_shuffle(1, 0, 3, 2));
            let sum = _mm_add_epi64(accumulator_vector, data_swapped);
            let result = _mm_add_epi64(sum, product);
            _mm_storeu_si128(accumulator_pointer.add(block), result);
        }
        block += 1;
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn scramble_accumulators_sse2(accumulators: &mut [u64; 8], secret: &[u8; 64]) {
    use core::arch::x86_64::{
        __m128i, _mm_add_epi64, _mm_loadu_si128, _mm_mul_epu32, _mm_set1_epi32, _mm_shuffle_epi32,
        _mm_slli_epi64, _mm_srli_epi64, _mm_storeu_si128, _mm_xor_si128,
    };
    let accumulator_pointer = accumulators.as_mut_ptr() as *mut __m128i;
    let prime_vector = unsafe { _mm_set1_epi32(PRIME_32_1 as i32) };
    let mut block = 0usize;
    while block < 4 {
        let offset = block * 16;
        unsafe {
            let accumulator_vector = _mm_loadu_si128(accumulator_pointer.add(block));
            let shifted = _mm_srli_epi64(accumulator_vector, 47);
            let data_shifted = _mm_xor_si128(accumulator_vector, shifted);
            let key_vector = _mm_loadu_si128(secret.as_ptr().add(offset) as *const __m128i);
            let data_key = _mm_xor_si128(data_shifted, key_vector);
            let data_key_high = _mm_shuffle_epi32(data_key, mm_shuffle(0, 3, 0, 1));
            let product_low = _mm_mul_epu32(data_key, prime_vector);
            let product_high = _mm_slli_epi64(_mm_mul_epu32(data_key_high, prime_vector), 32);
            let result = _mm_add_epi64(product_low, product_high);
            _mm_storeu_si128(accumulator_pointer.add(block), result);
        }
        block += 1;
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
unsafe fn accumulate_stripe_simd128(
    accumulators: &mut [u64; 8],
    stripe: &[u8; 64],
    secret: &[u8; 64],
) {
    use core::arch::wasm32::{
        i64x2_extmul_low_u32x4, u32x4_shuffle, u64x2_add, v128, v128_load, v128_store, v128_xor,
    };
    let accumulator_pointer = accumulators.as_mut_ptr() as *mut v128;
    let mut block = 0usize;
    while block < 4 {
        let offset = block * 16;
        unsafe {
            let accumulator_vector = v128_load(accumulator_pointer.add(block) as *const v128);
            let data_vector = v128_load(stripe.as_ptr().add(offset) as *const v128);
            let key_vector = v128_load(secret.as_ptr().add(offset) as *const v128);
            let data_key = v128_xor(data_vector, key_vector);
            let low_halves = u32x4_shuffle::<0, 2, 0, 2>(data_key, data_key);
            let high_halves = u32x4_shuffle::<1, 3, 1, 3>(data_key, data_key);
            let product = i64x2_extmul_low_u32x4(low_halves, high_halves);
            let data_swapped = u32x4_shuffle::<2, 3, 0, 1>(data_vector, data_vector);
            let sum = u64x2_add(accumulator_vector, data_swapped);
            let result = u64x2_add(sum, product);
            v128_store(accumulator_pointer.add(block), result);
        }
        block += 1;
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
unsafe fn scramble_accumulators_simd128(accumulators: &mut [u64; 8], secret: &[u8; 64]) {
    use core::arch::wasm32::{
        i64x2_extmul_low_u32x4, i64x2_shl, u32x4_shuffle, u32x4_splat, u64x2_add, u64x2_shr, v128,
        v128_load, v128_store, v128_xor,
    };
    let accumulator_pointer = accumulators.as_mut_ptr() as *mut v128;
    let prime_vector = u32x4_splat(PRIME_32_1);
    let mut block = 0usize;
    while block < 4 {
        let offset = block * 16;
        unsafe {
            let accumulator_vector = v128_load(accumulator_pointer.add(block) as *const v128);
            let shifted = u64x2_shr(accumulator_vector, 47);
            let data_shifted = v128_xor(accumulator_vector, shifted);
            let key_vector = v128_load(secret.as_ptr().add(offset) as *const v128);
            let data_key = v128_xor(data_shifted, key_vector);
            let low_halves = u32x4_shuffle::<0, 2, 0, 2>(data_key, data_key);
            let high_halves = u32x4_shuffle::<1, 3, 1, 3>(data_key, data_key);
            let product_low = i64x2_extmul_low_u32x4(low_halves, prime_vector);
            let product_high = i64x2_extmul_low_u32x4(high_halves, prime_vector);
            let result = u64x2_add(product_low, i64x2_shl(product_high, 32));
            v128_store(accumulator_pointer.add(block), result);
        }
        block += 1;
    }
}

#[cfg(any(test, feature = "wasm-exports"))]
pub(crate) fn run_self_tests() -> Option<u32> {
    let mut state = 0x9E37_79B9u32;
    let mut test_index = 0u32;
    let mut case = 0usize;
    while case < 64 {
        let mut accumulators_expected = [0u64; 8];
        let mut lane = 0usize;
        while lane < 8 {
            accumulators_expected[lane] = xorshift_u64(&mut state);
            lane += 1;
        }
        let mut accumulators_actual = accumulators_expected;

        let mut stripe = [0u8; 64];
        let mut secret = [0u8; 64];
        let mut byte_index = 0usize;
        while byte_index < 64 {
            stripe[byte_index] = (xorshift_u64(&mut state) & 0xff) as u8;
            secret[byte_index] = (xorshift_u64(&mut state) & 0xff) as u8;
            byte_index += 1;
        }

        accumulate_stripe_scalar(&mut accumulators_expected, &stripe, &secret);
        accumulate_stripe(&mut accumulators_actual, &stripe, &secret);
        if accumulators_expected != accumulators_actual {
            return Some(test_index);
        }
        test_index += 1;

        scramble_accumulators_scalar(&mut accumulators_expected, &secret);
        scramble_accumulators(&mut accumulators_actual, &secret);
        if accumulators_expected != accumulators_actual {
            return Some(test_index);
        }
        test_index += 1;

        case += 1;
    }
    None
}

#[cfg(any(test, feature = "wasm-exports"))]
fn xorshift_u64(state: &mut u32) -> u64 {
    let high = xorshift_next(state);
    let low = xorshift_next(state);
    ((high as u64) << 32) | (low as u64)
}

#[cfg(any(test, feature = "wasm-exports"))]
fn xorshift_next(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn random_stripe(seed: u32) -> ([u8; 64], u32) {
        let mut state = seed;
        let mut stripe = [0u8; 64];
        let mut index = 0usize;
        while index < 64 {
            stripe[index] = (xorshift_next(&mut state) & 0xff) as u8;
            index += 1;
        }
        (stripe, state)
    }

    fn random_accumulators(seed: u32) -> ([u64; 8], u32) {
        let mut state = seed;
        let mut accumulators = [0u64; 8];
        let mut index = 0usize;
        while index < 8 {
            accumulators[index] = xorshift_u64(&mut state);
            index += 1;
        }
        (accumulators, state)
    }

    #[test]
    fn accumulate_stripe_matches_scalar_on_random_cases() {
        let mut state = 0x1234_5678u32;
        for _ in 0..1000 {
            let (accumulators_seed, next_state) = random_accumulators(state);
            state = next_state;
            let (stripe, next_state) = random_stripe(state);
            state = next_state;
            let (secret, next_state) = random_stripe(state);
            state = next_state;

            let mut expected = accumulators_seed;
            let mut actual = accumulators_seed;
            accumulate_stripe_scalar(&mut expected, &stripe, &secret);
            accumulate_stripe(&mut actual, &stripe, &secret);
            assert_eq!(expected, actual);
        }
    }

    #[test]
    fn scramble_accumulators_matches_scalar_on_random_cases() {
        let mut state = 0x8765_4321u32;
        for _ in 0..1000 {
            let (accumulators_seed, next_state) = random_accumulators(state);
            state = next_state;
            let (secret, next_state) = random_stripe(state);
            state = next_state;

            let mut expected = accumulators_seed;
            let mut actual = accumulators_seed;
            scramble_accumulators_scalar(&mut expected, &secret);
            scramble_accumulators(&mut actual, &secret);
            assert_eq!(expected, actual);
        }
    }

    #[test]
    fn accumulate_stripes_matches_calling_accumulate_stripe_per_stripe() {
        let mut state = 0x1111_2222u32;
        let (accumulators_seed, next_state) = random_accumulators(state);
        state = next_state;

        let stripe_count = 4usize;
        let mut input = std::vec![0u8; stripe_count * 64];
        for slot in input.iter_mut() {
            *slot = (xorshift_next(&mut state) & 0xff) as u8;
        }
        let mut secret = std::vec![0u8; stripe_count * 8 + 64];
        for slot in secret.iter_mut() {
            *slot = (xorshift_next(&mut state) & 0xff) as u8;
        }

        let mut expected = accumulators_seed;
        let mut stripe_index = 0usize;
        while stripe_index < stripe_count {
            let stripe: &[u8; 64] = input[stripe_index * 64..stripe_index * 64 + 64]
                .try_into()
                .unwrap();
            let secret_window: &[u8; 64] = secret[stripe_index * 8..stripe_index * 8 + 64]
                .try_into()
                .unwrap();
            accumulate_stripe(&mut expected, stripe, secret_window);
            stripe_index += 1;
        }

        let mut actual = accumulators_seed;
        accumulate_stripes(&mut actual, &input, &secret, stripe_count);

        assert_eq!(expected, actual);
    }

    #[test]
    fn run_self_tests_reports_success() {
        assert_eq!(run_self_tests(), None);
    }
}
