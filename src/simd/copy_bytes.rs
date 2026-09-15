const TILE_LENGTH: usize = 48;

pub fn copy_bytes(source: &[u8], destination: &mut [u8]) {
    assert_eq!(source.len(), destination.len());
    #[cfg(target_arch = "aarch64")]
    unsafe {
        copy_bytes_neon(source.as_ptr(), destination.as_mut_ptr(), source.len());
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        copy_bytes_sse2(source.as_ptr(), destination.as_mut_ptr(), source.len());
    }
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    unsafe {
        copy_bytes_simd128(source.as_ptr(), destination.as_mut_ptr(), source.len());
    }
    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "x86_64",
        all(target_arch = "wasm32", target_feature = "simd128")
    )))]
    copy_bytes_scalar(source, destination);
}

pub fn fill_pattern(pattern: &[u8], destination: &mut [u8]) {
    assert!(!pattern.is_empty());
    assert!(pattern.len() <= 32);
    let tile = build_tile(pattern);
    let pattern_length = pattern.len();
    #[cfg(target_arch = "aarch64")]
    return fill_pattern_neon(&tile, pattern_length, destination);
    #[cfg(target_arch = "x86_64")]
    return fill_pattern_sse2(&tile, pattern_length, destination);
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    return fill_pattern_simd128(&tile, pattern_length, destination);
    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "x86_64",
        all(target_arch = "wasm32", target_feature = "simd128")
    )))]
    fill_pattern_scalar(&tile, pattern_length, destination, 0);
}

fn build_tile(pattern: &[u8]) -> [u8; TILE_LENGTH] {
    let mut tile = [0u8; TILE_LENGTH];
    let pattern_length = pattern.len();
    let mut index = 0usize;
    while index < TILE_LENGTH {
        tile[index] = pattern[index % pattern_length];
        index += 1;
    }
    tile
}

fn copy_bytes_scalar(source: &[u8], destination: &mut [u8]) {
    let length = source.len();
    let mut position = 0usize;
    while position + 8 <= length {
        let word = u64::from_le_bytes(source[position..position + 8].try_into().unwrap());
        destination[position..position + 8].copy_from_slice(&word.to_le_bytes());
        position += 8;
    }
    while position < length {
        destination[position] = source[position];
        position += 1;
    }
}

fn fill_pattern_scalar(
    tile: &[u8; TILE_LENGTH],
    pattern_length: usize,
    destination: &mut [u8],
    start_phase: usize,
) {
    let mut position = 0usize;
    while position < destination.len() {
        let start = (start_phase + position) % pattern_length;
        let remaining = destination.len() - position;
        if remaining >= 8 {
            let word = u64::from_le_bytes(tile[start..start + 8].try_into().unwrap());
            destination[position..position + 8].copy_from_slice(&word.to_le_bytes());
            position += 8;
        } else {
            destination[position] = tile[start];
            position += 1;
        }
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn copy_bytes_neon(source: *const u8, destination: *mut u8, length: usize) {
    use core::arch::aarch64::{vld1q_u8, vst1q_u8};
    let mut position = 0usize;
    while position + 16 <= length {
        unsafe {
            let chunk = vld1q_u8(source.add(position));
            vst1q_u8(destination.add(position), chunk);
        }
        position += 16;
    }
    while position < length {
        unsafe {
            *destination.add(position) = *source.add(position);
        }
        position += 1;
    }
}

#[cfg(target_arch = "aarch64")]
fn fill_pattern_neon(tile: &[u8; TILE_LENGTH], pattern_length: usize, destination: &mut [u8]) {
    use core::arch::aarch64::{vld1q_u8, vst1q_u8};
    let mut position = 0usize;
    while position + 16 <= destination.len() {
        let start = position % pattern_length;
        unsafe {
            let chunk = vld1q_u8(tile.as_ptr().add(start));
            vst1q_u8(destination.as_mut_ptr().add(position), chunk);
        }
        position += 16;
    }
    if position < destination.len() {
        fill_pattern_scalar(tile, pattern_length, &mut destination[position..], position);
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn copy_bytes_sse2(source: *const u8, destination: *mut u8, length: usize) {
    use core::arch::x86_64::{__m128i, _mm_loadu_si128, _mm_storeu_si128};
    let mut position = 0usize;
    while position + 16 <= length {
        unsafe {
            let chunk = _mm_loadu_si128(source.add(position) as *const __m128i);
            _mm_storeu_si128(destination.add(position) as *mut __m128i, chunk);
        }
        position += 16;
    }
    while position < length {
        unsafe {
            *destination.add(position) = *source.add(position);
        }
        position += 1;
    }
}

#[cfg(target_arch = "x86_64")]
fn fill_pattern_sse2(tile: &[u8; TILE_LENGTH], pattern_length: usize, destination: &mut [u8]) {
    use core::arch::x86_64::{__m128i, _mm_loadu_si128, _mm_storeu_si128};
    let mut position = 0usize;
    while position + 16 <= destination.len() {
        let start = position % pattern_length;
        unsafe {
            let chunk = _mm_loadu_si128(tile.as_ptr().add(start) as *const __m128i);
            _mm_storeu_si128(
                destination.as_mut_ptr().add(position) as *mut __m128i,
                chunk,
            );
        }
        position += 16;
    }
    if position < destination.len() {
        fill_pattern_scalar(tile, pattern_length, &mut destination[position..], position);
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
unsafe fn copy_bytes_simd128(source: *const u8, destination: *mut u8, length: usize) {
    use core::arch::wasm32::{v128, v128_load, v128_store};
    let mut position = 0usize;
    while position + 16 <= length {
        unsafe {
            let chunk = v128_load(source.add(position) as *const v128);
            v128_store(destination.add(position) as *mut v128, chunk);
        }
        position += 16;
    }
    while position < length {
        unsafe {
            *destination.add(position) = *source.add(position);
        }
        position += 1;
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
fn fill_pattern_simd128(tile: &[u8; TILE_LENGTH], pattern_length: usize, destination: &mut [u8]) {
    use core::arch::wasm32::{v128, v128_load, v128_store};
    let mut position = 0usize;
    while position + 16 <= destination.len() {
        let start = position % pattern_length;
        unsafe {
            let chunk = v128_load(tile.as_ptr().add(start) as *const v128);
            v128_store(destination.as_mut_ptr().add(position) as *mut v128, chunk);
        }
        position += 16;
    }
    if position < destination.len() {
        fill_pattern_scalar(tile, pattern_length, &mut destination[position..], position);
    }
}

#[allow(clippy::missing_safety_doc)]
pub unsafe fn copy_bytes_overshoot_unchecked(
    source: *const u8,
    destination: *mut u8,
    length: usize,
) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        copy_bytes_overshoot_neon(source, destination, length);
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        copy_bytes_overshoot_sse2(source, destination, length);
    }
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    unsafe {
        copy_bytes_overshoot_simd128(source, destination, length);
    }
    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "x86_64",
        all(target_arch = "wasm32", target_feature = "simd128")
    )))]
    unsafe {
        copy_bytes_overshoot_scalar_unchecked(source, destination, length);
    }
}

#[cfg(not(any(
    target_arch = "aarch64",
    target_arch = "x86_64",
    all(target_arch = "wasm32", target_feature = "simd128")
)))]
unsafe fn copy_bytes_overshoot_scalar_unchecked(
    source: *const u8,
    destination: *mut u8,
    length: usize,
) {
    let mut position = 0usize;
    while position < length {
        unsafe {
            let word_pointer = source.add(position) as *const u64;
            let word = word_pointer.read_unaligned();
            (destination.add(position) as *mut u64).write_unaligned(word);
        }
        position += 8;
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn copy_bytes_overshoot_neon(source: *const u8, destination: *mut u8, length: usize) {
    use core::arch::aarch64::{vld1q_u8, vst1q_u8};
    let mut position = 0usize;
    while position < length {
        unsafe {
            let chunk = vld1q_u8(source.add(position));
            vst1q_u8(destination.add(position), chunk);
        }
        position += 16;
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn copy_bytes_overshoot_sse2(source: *const u8, destination: *mut u8, length: usize) {
    use core::arch::x86_64::{__m128i, _mm_loadu_si128, _mm_storeu_si128};
    let mut position = 0usize;
    while position < length {
        unsafe {
            let chunk = _mm_loadu_si128(source.add(position) as *const __m128i);
            _mm_storeu_si128(destination.add(position) as *mut __m128i, chunk);
        }
        position += 16;
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
unsafe fn copy_bytes_overshoot_simd128(source: *const u8, destination: *mut u8, length: usize) {
    use core::arch::wasm32::{v128, v128_load, v128_store};
    let mut position = 0usize;
    while position < length {
        unsafe {
            let chunk = v128_load(source.add(position) as *const v128);
            v128_store(destination.add(position) as *mut v128, chunk);
        }
        position += 16;
    }
}

pub fn run_self_tests() -> Option<u32> {
    let lengths = [0usize, 1, 15, 16, 17, 31, 32, 33, 63, 64, 65];
    let mut state = 0x1234_5678u32;
    let mut source = [0u8; 96];
    let mut index = 0usize;
    while index < source.len() {
        source[index] = (xorshift_next(&mut state) & 0xff) as u8;
        index += 1;
    }

    let mut length_index = 0usize;
    while length_index < lengths.len() {
        let length = lengths[length_index];
        let mut expected = [0u8; 96];
        let mut actual = [0u8; 96];
        copy_bytes_scalar(&source[..length], &mut expected[..length]);
        copy_bytes(&source[..length], &mut actual[..length]);
        if expected[..length] != actual[..length] {
            return Some(1);
        }
        length_index += 1;
    }

    let pattern = [1u8, 2, 3, 4, 5, 6, 7, 8];
    let mut destination = [0u8; 64];
    fill_pattern(&pattern, &mut destination);
    let mut check_index = 0usize;
    while check_index < destination.len() {
        if destination[check_index] != pattern[check_index % pattern.len()] {
            return Some(2);
        }
        check_index += 1;
    }

    let mut overshoot_source = [0u8; 96];
    let mut overshoot_index = 0usize;
    while overshoot_index < overshoot_source.len() {
        overshoot_source[overshoot_index] = (xorshift_next(&mut state) & 0xff) as u8;
        overshoot_index += 1;
    }
    let overshoot_length = 40usize;
    let mut overshoot_expected = [0u8; 40];
    copy_bytes_scalar(
        &overshoot_source[..overshoot_length],
        &mut overshoot_expected,
    );
    let mut overshoot_actual = [0u8; 96];
    unsafe {
        copy_bytes_overshoot_unchecked(
            overshoot_source.as_ptr(),
            overshoot_actual.as_mut_ptr(),
            overshoot_length,
        );
    }
    if overshoot_expected[..] != overshoot_actual[..overshoot_length] {
        return Some(3);
    }

    None
}

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

    const LENGTHS: [usize; 13] = [0, 1, 15, 16, 17, 31, 32, 33, 63, 64, 65, 1000, 65536];

    fn random_vec(count: usize, seed: u32) -> std::vec::Vec<u8> {
        let mut state = seed;
        let mut buffer = std::vec![0u8; count];
        for slot in buffer.iter_mut() {
            *slot = (xorshift_next(&mut state) & 0xff) as u8;
        }
        buffer
    }

    #[test]
    fn copy_bytes_matches_scalar_across_lengths_and_alignments() {
        for &length in LENGTHS.iter() {
            for source_offset in 0..16usize {
                for destination_offset in 0..16usize {
                    let source_buffer = random_vec(length + source_offset, 0x1234_5678);
                    let mut expected = std::vec![0u8; length + destination_offset];
                    let mut actual = std::vec![0u8; length + destination_offset];

                    copy_bytes_scalar(
                        &source_buffer[source_offset..source_offset + length],
                        &mut expected[destination_offset..destination_offset + length],
                    );
                    copy_bytes(
                        &source_buffer[source_offset..source_offset + length],
                        &mut actual[destination_offset..destination_offset + length],
                    );

                    assert_eq!(
                        expected, actual,
                        "length {} source_offset {} destination_offset {}",
                        length, source_offset, destination_offset
                    );
                }
            }
        }
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn copy_bytes_neon_matches_scalar_across_lengths_and_alignments() {
        for &length in LENGTHS.iter() {
            for source_offset in 0..16usize {
                for destination_offset in 0..16usize {
                    let source_buffer = random_vec(length + source_offset, 0xabcd_ef01);
                    let mut expected = std::vec![0u8; length + destination_offset];
                    let mut actual = std::vec![0u8; length + destination_offset];

                    copy_bytes_scalar(
                        &source_buffer[source_offset..source_offset + length],
                        &mut expected[destination_offset..destination_offset + length],
                    );
                    unsafe {
                        copy_bytes_neon(
                            source_buffer[source_offset..].as_ptr(),
                            actual[destination_offset..].as_mut_ptr(),
                            length,
                        );
                    }

                    assert_eq!(
                        expected[destination_offset..destination_offset + length],
                        actual[destination_offset..destination_offset + length]
                    );
                }
            }
        }
    }

    #[test]
    fn fill_pattern_repeats_over_destination() {
        for pattern_length in 1..=32usize {
            let pattern = random_vec(pattern_length, 0x0f0f_1111 + pattern_length as u32);
            for destination_length in 0..=100usize {
                let mut destination = std::vec![0xffu8; destination_length];
                fill_pattern(&pattern, &mut destination);
                for index in 0..destination_length {
                    assert_eq!(destination[index], pattern[index % pattern_length]);
                }
            }
        }
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn fill_pattern_neon_matches_scalar() {
        for pattern_length in 1..=32usize {
            let pattern = random_vec(pattern_length, 0x2222_0000 + pattern_length as u32);
            let tile = build_tile(&pattern);
            for destination_length in 0..=100usize {
                let mut expected = std::vec![0u8; destination_length];
                let mut actual = std::vec![0u8; destination_length];
                fill_pattern_scalar(&tile, pattern_length, &mut expected, 0);
                fill_pattern_neon(&tile, pattern_length, &mut actual);
                assert_eq!(expected, actual);
            }
        }
    }

    #[test]
    fn copy_bytes_overshoot_unchecked_matches_scalar_within_length() {
        for &length in LENGTHS.iter() {
            if length == 0 || length > 65536 {
                continue;
            }
            let overshoot_room = 32usize;
            let source_buffer = random_vec(length + overshoot_room, 0x2468_1357);
            let mut expected = std::vec![0u8; length];
            copy_bytes_scalar(&source_buffer[..length], &mut expected);

            let mut actual = std::vec![0u8; length + overshoot_room];
            unsafe {
                copy_bytes_overshoot_unchecked(source_buffer.as_ptr(), actual.as_mut_ptr(), length);
            }

            assert_eq!(expected[..], actual[..length]);
        }
    }

    #[test]
    fn copy_bytes_overshoot_unchecked_repeats_a_pattern_through_overlap() {
        let overshoot_room = 32usize;
        for offset in 1..=32usize {
            let mut local = std::vec![0u8; offset + 300 + overshoot_room];
            for slot in local[..offset].iter_mut() {
                *slot = b'a';
            }
            let mut pattern = [0u8; 16];
            fill_pattern(&local[..offset], &mut pattern);

            let mut written = 0usize;
            let base = local.as_mut_ptr();
            while written < 300 {
                unsafe {
                    copy_bytes_overshoot_unchecked(
                        pattern.as_ptr(),
                        base.add(offset + written),
                        16,
                    );
                }
                written += 16;
            }
            for index in 0..300 {
                assert_eq!(local[offset + index], b'a');
            }
        }
    }

    #[test]
    fn run_self_tests_reports_success() {
        assert_eq!(run_self_tests(), None);
    }
}
