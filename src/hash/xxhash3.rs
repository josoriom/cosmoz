const PRIME_32_1: u64 = 0x9E3779B1;
const PRIME_32_2: u64 = 0x85EBCA77;
const PRIME_32_3: u64 = 0xC2B2AE3D;
const PRIME_64_1: u64 = 0x9E3779B185EBCA87;
const PRIME_64_2: u64 = 0xC2B2AE3D27D4EB4F;
const PRIME_64_3: u64 = 0x165667B19E3779F9;
const PRIME_64_4: u64 = 0x85EBCA77C2B2AE63;
const PRIME_64_5: u64 = 0x27D4EB2F165667C5;
const PRIME_MIX_1: u64 = 0x165667919E3779F9;
const PRIME_MIX_2: u64 = 0x9FB21C651E98DF25;

pub(crate) const SECRET_LENGTH: usize = 192;

const DEFAULT_SECRET: [u8; SECRET_LENGTH] = [
    0xb8, 0xfe, 0x6c, 0x39, 0x23, 0xa4, 0x4b, 0xbe, 0x7c, 0x01, 0x81, 0x2c, 0xf7, 0x21, 0xad, 0x1c,
    0xde, 0xd4, 0x6d, 0xe9, 0x83, 0x90, 0x97, 0xdb, 0x72, 0x40, 0xa4, 0xa4, 0xb7, 0xb3, 0x67, 0x1f,
    0xcb, 0x79, 0xe6, 0x4e, 0xcc, 0xc0, 0xe5, 0x78, 0x82, 0x5a, 0xd0, 0x7d, 0xcc, 0xff, 0x72, 0x21,
    0xb8, 0x08, 0x46, 0x74, 0xf7, 0x43, 0x24, 0x8e, 0xe0, 0x35, 0x90, 0xe6, 0x81, 0x3a, 0x26, 0x4c,
    0x3c, 0x28, 0x52, 0xbb, 0x91, 0xc3, 0x00, 0xcb, 0x88, 0xd0, 0x65, 0x8b, 0x1b, 0x53, 0x2e, 0xa3,
    0x71, 0x64, 0x48, 0x97, 0xa2, 0x0d, 0xf9, 0x4e, 0x38, 0x19, 0xef, 0x46, 0xa9, 0xde, 0xac, 0xd8,
    0xa8, 0xfa, 0x76, 0x3f, 0xe3, 0x9c, 0x34, 0x3f, 0xf9, 0xdc, 0xbb, 0xc7, 0xc7, 0x0b, 0x4f, 0x1d,
    0x8a, 0x51, 0xe0, 0x4b, 0xcd, 0xb4, 0x59, 0x31, 0xc8, 0x9f, 0x7e, 0xc9, 0xd9, 0x78, 0x73, 0x64,
    0xea, 0xc5, 0xac, 0x83, 0x34, 0xd3, 0xeb, 0xc3, 0xc5, 0x81, 0xa0, 0xff, 0xfa, 0x13, 0x63, 0xeb,
    0x17, 0x0d, 0xdd, 0x51, 0xb7, 0xf0, 0xda, 0x49, 0xd3, 0x16, 0x55, 0x26, 0x29, 0xd4, 0x68, 0x9e,
    0x2b, 0x16, 0xbe, 0x58, 0x7d, 0x47, 0xa1, 0xfc, 0x8f, 0xf8, 0xb8, 0xd1, 0x7a, 0xd0, 0x31, 0xce,
    0x45, 0xcb, 0x3a, 0x8f, 0x95, 0x16, 0x04, 0x28, 0xaf, 0xd7, 0xfb, 0xca, 0xbb, 0x4b, 0x40, 0x7e,
];

const STRIPE_LENGTH: usize = 64;
const SECRET_CONSUME_RATE: usize = 8;
const STRIPES_PER_BLOCK: usize = (SECRET_LENGTH - STRIPE_LENGTH) / SECRET_CONSUME_RATE;
const SECRET_LIMIT: usize = SECRET_LENGTH - STRIPE_LENGTH;
const LAST_ACCUMULATION_SECRET_START: usize = 7;
const MERGE_ACCUMULATORS_SECRET_START: usize = 11;

const MIDSIZE_MAX: usize = 240;
const SECRET_SIZE_MIN: usize = 136;
const MIDSIZE_START_OFFSET: usize = 3;
const MIDSIZE_LAST_OFFSET: usize = 17;
const MIDSIZE_LAST_MIX_SECRET_OFFSET: usize = SECRET_SIZE_MIN - MIDSIZE_LAST_OFFSET;

const INTERNAL_BUFFER_LENGTH: usize = 256;
const INTERNAL_BUFFER_STRIPES: usize = INTERNAL_BUFFER_LENGTH / STRIPE_LENGTH;

const INITIAL_ACCUMULATORS: [u64; 8] = [
    PRIME_32_3, PRIME_64_1, PRIME_64_2, PRIME_64_3, PRIME_64_4, PRIME_32_2, PRIME_64_5, PRIME_32_1,
];

pub(crate) struct XxHash3 {
    accumulators: [u64; 8],
    buffer: [u8; INTERNAL_BUFFER_LENGTH],
    buffered_length: usize,
    stripes_processed_in_block: usize,
    total_length: u64,
}

impl XxHash3 {
    pub(crate) fn new() -> Self {
        XxHash3 {
            accumulators: INITIAL_ACCUMULATORS,
            buffer: [0; INTERNAL_BUFFER_LENGTH],
            buffered_length: 0,
            stripes_processed_in_block: 0,
            total_length: 0,
        }
    }

    pub(crate) fn update(&mut self, input: &[u8]) {
        if input.is_empty() {
            return;
        }
        self.total_length = self.total_length.wrapping_add(input.len() as u64);

        if input.len() <= INTERNAL_BUFFER_LENGTH - self.buffered_length {
            let start = self.buffered_length;
            let end = start + input.len();
            self.buffer[start..end].copy_from_slice(input);
            self.buffered_length = end;
            return;
        }

        let mut remaining_input = input;

        if self.buffered_length > 0 {
            let load_length = INTERNAL_BUFFER_LENGTH - self.buffered_length;
            let buffer_end = self.buffered_length + load_length;
            self.buffer[self.buffered_length..buffer_end]
                .copy_from_slice(&remaining_input[..load_length]);
            remaining_input = &remaining_input[load_length..];
            let buffer_snapshot = self.buffer;
            consume_stripes(
                &mut self.accumulators,
                &mut self.stripes_processed_in_block,
                &buffer_snapshot,
                INTERNAL_BUFFER_STRIPES,
            );
            self.buffered_length = 0;
        }

        if remaining_input.len() > INTERNAL_BUFFER_LENGTH {
            let stripe_count = (remaining_input.len() - 1) / STRIPE_LENGTH;
            consume_stripes(
                &mut self.accumulators,
                &mut self.stripes_processed_in_block,
                remaining_input,
                stripe_count,
            );
            let consumed_length = stripe_count * STRIPE_LENGTH;
            let last_stripe_start = consumed_length - STRIPE_LENGTH;
            self.buffer[INTERNAL_BUFFER_LENGTH - STRIPE_LENGTH..INTERNAL_BUFFER_LENGTH]
                .copy_from_slice(&remaining_input[last_stripe_start..consumed_length]);
            remaining_input = &remaining_input[consumed_length..];
        }

        self.buffer[..remaining_input.len()].copy_from_slice(remaining_input);
        self.buffered_length = remaining_input.len();
    }

    pub(crate) fn finish(&self) -> u64 {
        if self.total_length > MIDSIZE_MAX as u64 {
            return hash_long_input(
                self.accumulators,
                &self.buffer,
                self.buffered_length,
                self.stripes_processed_in_block,
                self.total_length,
            );
        }

        let input = &self.buffer[..self.buffered_length];
        if input.len() <= 16 {
            hash_length_0_to_16(input)
        } else if input.len() <= 128 {
            hash_length_17_to_128(input)
        } else {
            hash_length_129_to_240(input)
        }
    }
}

impl Default for XxHash3 {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "compression", feature = "wasm-exports"))]
pub(crate) fn hash_bytes(input: &[u8]) -> u64 {
    let mut hasher = XxHash3::new();
    hasher.update(input);
    hasher.finish()
}

fn read_u32_little_endian(bytes: &[u8]) -> u32 {
    match bytes.first_chunk::<4>() {
        Some(chunk) => u32::from_le_bytes(*chunk),
        None => 0,
    }
}

fn read_u64_little_endian(bytes: &[u8]) -> u64 {
    match bytes.first_chunk::<8>() {
        Some(chunk) => u64::from_le_bytes(*chunk),
        None => 0,
    }
}

fn read_u32_at(bytes: &[u8], offset: usize) -> u32 {
    match bytes.get(offset..) {
        Some(rest) => read_u32_little_endian(rest),
        None => 0,
    }
}

fn read_u64_at(bytes: &[u8], offset: usize) -> u64 {
    match bytes.get(offset..) {
        Some(rest) => read_u64_little_endian(rest),
        None => 0,
    }
}

fn slice_from(bytes: &[u8], start: usize) -> &[u8] {
    match bytes.get(start..) {
        Some(rest) => rest,
        None => &[],
    }
}

fn avalanche(value: u64) -> u64 {
    let mixed = value ^ (value >> 37);
    let mixed = mixed.wrapping_mul(PRIME_MIX_1);
    mixed ^ (mixed >> 32)
}

fn avalanche_xxhash64(value: u64) -> u64 {
    let mixed = value ^ (value >> 33);
    let mixed = mixed.wrapping_mul(PRIME_64_2);
    let mixed = mixed ^ (mixed >> 29);
    let mixed = mixed.wrapping_mul(PRIME_64_3);
    mixed ^ (mixed >> 32)
}

fn rrmxmx_avalanche(value: u64, length: u64) -> u64 {
    let mixed = value ^ (value.rotate_left(49) ^ value.rotate_left(24));
    let mixed = mixed.wrapping_mul(PRIME_MIX_2);
    let mixed = mixed ^ ((mixed >> 35).wrapping_add(length));
    let mixed = mixed.wrapping_mul(PRIME_MIX_2);
    mixed ^ (mixed >> 28)
}

fn multiply_fold_64(left: u64, right: u64) -> u64 {
    let product = (left as u128) * (right as u128);
    (product as u64) ^ ((product >> 64) as u64)
}

fn mix_16_bytes(data: &[u8], secret_offset: usize) -> u64 {
    let data_low = read_u64_little_endian(data);
    let data_high = read_u64_at(data, 8);
    let secret_low = read_u64_at(&DEFAULT_SECRET, secret_offset);
    let secret_high = read_u64_at(&DEFAULT_SECRET, secret_offset + 8);
    multiply_fold_64(data_low ^ secret_low, data_high ^ secret_high)
}

fn hash_empty_input() -> u64 {
    avalanche_xxhash64(read_u64_at(&DEFAULT_SECRET, 56) ^ read_u64_at(&DEFAULT_SECRET, 64))
}

fn hash_length_1_to_3(input: &[u8]) -> u64 {
    let length = input.len();
    let first_byte = input.first().copied().unwrap_or(0) as u32;
    let middle_byte = input.get(length >> 1).copied().unwrap_or(0) as u32;
    let last_byte = input.get(length.saturating_sub(1)).copied().unwrap_or(0) as u32;
    let combined = (first_byte << 16) | (middle_byte << 24) | last_byte | ((length as u32) << 8);
    let bitflip =
        (read_u32_little_endian(&DEFAULT_SECRET) ^ read_u32_at(&DEFAULT_SECRET, 4)) as u64;
    avalanche_xxhash64((combined as u64) ^ bitflip)
}

fn hash_length_4_to_8(input: &[u8]) -> u64 {
    let length = input.len();
    let input_first = read_u32_little_endian(input) as u64;
    let input_last = read_u32_at(input, length - 4) as u64;
    let bitflip = read_u64_at(&DEFAULT_SECRET, 8) ^ read_u64_at(&DEFAULT_SECRET, 16);
    let combined = input_last | (input_first << 32);
    rrmxmx_avalanche(combined ^ bitflip, length as u64)
}

fn hash_length_9_to_16(input: &[u8]) -> u64 {
    let length = input.len();
    let bitflip_low = read_u64_at(&DEFAULT_SECRET, 24) ^ read_u64_at(&DEFAULT_SECRET, 32);
    let bitflip_high = read_u64_at(&DEFAULT_SECRET, 40) ^ read_u64_at(&DEFAULT_SECRET, 48);
    let input_low = read_u64_little_endian(input) ^ bitflip_low;
    let input_high = read_u64_at(input, length - 8) ^ bitflip_high;
    let folded = multiply_fold_64(input_low, input_high);
    let value = (length as u64)
        .wrapping_add(input_low.swap_bytes())
        .wrapping_add(input_high)
        .wrapping_add(folded);
    avalanche(value)
}

fn hash_length_0_to_16(input: &[u8]) -> u64 {
    let length = input.len();
    if length > 8 {
        hash_length_9_to_16(input)
    } else if length >= 4 {
        hash_length_4_to_8(input)
    } else if length >= 1 {
        hash_length_1_to_3(input)
    } else {
        hash_empty_input()
    }
}

fn hash_length_17_to_128(input: &[u8]) -> u64 {
    let length = input.len();
    let mut accumulator = (length as u64).wrapping_mul(PRIME_64_1);
    if length > 32 {
        if length > 64 {
            if length > 96 {
                accumulator = accumulator.wrapping_add(mix_16_bytes(slice_from(input, 48), 96));
                accumulator =
                    accumulator.wrapping_add(mix_16_bytes(slice_from(input, length - 64), 112));
            }
            accumulator = accumulator.wrapping_add(mix_16_bytes(slice_from(input, 32), 64));
            accumulator =
                accumulator.wrapping_add(mix_16_bytes(slice_from(input, length - 48), 80));
        }
        accumulator = accumulator.wrapping_add(mix_16_bytes(slice_from(input, 16), 32));
        accumulator = accumulator.wrapping_add(mix_16_bytes(slice_from(input, length - 32), 48));
    }
    accumulator = accumulator.wrapping_add(mix_16_bytes(input, 0));
    accumulator = accumulator.wrapping_add(mix_16_bytes(slice_from(input, length - 16), 16));
    avalanche(accumulator)
}

fn hash_length_129_to_240(input: &[u8]) -> u64 {
    let length = input.len();
    let round_count = length / 16;
    let mut accumulator = (length as u64).wrapping_mul(PRIME_64_1);
    for round in 0..8 {
        accumulator =
            accumulator.wrapping_add(mix_16_bytes(slice_from(input, round * 16), round * 16));
    }
    let mut accumulator_end = mix_16_bytes(
        slice_from(input, length - 16),
        MIDSIZE_LAST_MIX_SECRET_OFFSET,
    );
    accumulator = avalanche(accumulator);
    for round in 8..round_count {
        accumulator_end = accumulator_end.wrapping_add(mix_16_bytes(
            slice_from(input, round * 16),
            (round - 8) * 16 + MIDSIZE_START_OFFSET,
        ));
    }
    avalanche(accumulator.wrapping_add(accumulator_end))
}

fn secret_window(secret_offset: usize) -> &'static [u8; 64] {
    DEFAULT_SECRET[secret_offset..secret_offset + STRIPE_LENGTH]
        .try_into()
        .unwrap()
}

fn accumulate_one_stripe(accumulators: &mut [u64; 8], stripe: &[u8], secret_offset: usize) {
    let mut padded_stripe = [0u8; STRIPE_LENGTH];
    let copy_length = stripe.len().min(STRIPE_LENGTH);
    padded_stripe[..copy_length].copy_from_slice(&stripe[..copy_length]);
    crate::simd::xxhash3_stripes::accumulate_stripe(
        accumulators,
        &padded_stripe,
        secret_window(secret_offset),
    );
}

fn accumulate_stripes(
    accumulators: &mut [u64; 8],
    input: &[u8],
    secret_offset: usize,
    stripe_count: usize,
) {
    let full_stripe_count = stripe_count.min(input.len() / STRIPE_LENGTH);
    if full_stripe_count > 0 {
        crate::simd::xxhash3_stripes::accumulate_stripes(
            accumulators,
            &input[..full_stripe_count * STRIPE_LENGTH],
            &DEFAULT_SECRET[secret_offset..],
            full_stripe_count,
        );
    }
    for stripe_index in full_stripe_count..stripe_count {
        let stripe = slice_from(input, stripe_index * STRIPE_LENGTH);
        accumulate_one_stripe(
            accumulators,
            stripe,
            secret_offset + stripe_index * SECRET_CONSUME_RATE,
        );
    }
}

fn scramble_accumulators(accumulators: &mut [u64; 8], secret_offset: usize) {
    crate::simd::xxhash3_stripes::scramble_accumulators(accumulators, secret_window(secret_offset));
}

fn consume_stripes(
    accumulators: &mut [u64; 8],
    stripes_processed_in_block: &mut usize,
    input: &[u8],
    mut stripe_count: usize,
) {
    let mut input_offset = 0usize;
    let mut secret_offset = *stripes_processed_in_block * SECRET_CONSUME_RATE;

    if stripe_count >= STRIPES_PER_BLOCK - *stripes_processed_in_block {
        let mut stripes_this_round = STRIPES_PER_BLOCK - *stripes_processed_in_block;
        loop {
            accumulate_stripes(
                accumulators,
                slice_from(input, input_offset),
                secret_offset,
                stripes_this_round,
            );
            scramble_accumulators(accumulators, SECRET_LIMIT);
            input_offset += stripes_this_round * STRIPE_LENGTH;
            stripe_count -= stripes_this_round;
            stripes_this_round = STRIPES_PER_BLOCK;
            secret_offset = 0;
            if stripe_count < STRIPES_PER_BLOCK {
                break;
            }
        }
        *stripes_processed_in_block = 0;
    }

    if stripe_count > 0 {
        accumulate_stripes(
            accumulators,
            slice_from(input, input_offset),
            secret_offset,
            stripe_count,
        );
        *stripes_processed_in_block += stripe_count;
    }
}

fn merge_accumulators(accumulators: &[u64; 8], secret_offset: usize, initial_value: u64) -> u64 {
    let mut result = initial_value;
    for pair in 0..4 {
        let mixed = multiply_fold_64(
            accumulators[pair * 2] ^ read_u64_at(&DEFAULT_SECRET, secret_offset + pair * 16),
            accumulators[pair * 2 + 1]
                ^ read_u64_at(&DEFAULT_SECRET, secret_offset + pair * 16 + 8),
        );
        result = result.wrapping_add(mixed);
    }
    avalanche(result)
}

fn hash_long_input(
    mut accumulators: [u64; 8],
    buffer: &[u8; INTERNAL_BUFFER_LENGTH],
    buffered_length: usize,
    mut stripes_processed_in_block: usize,
    total_length: u64,
) -> u64 {
    if buffered_length >= STRIPE_LENGTH {
        let stripe_count = (buffered_length - 1) / STRIPE_LENGTH;
        consume_stripes(
            &mut accumulators,
            &mut stripes_processed_in_block,
            buffer,
            stripe_count,
        );
        let last_stripe_start = buffered_length - STRIPE_LENGTH;
        let last_stripe: &[u8; STRIPE_LENGTH] = buffer
            [last_stripe_start..last_stripe_start + STRIPE_LENGTH]
            .try_into()
            .unwrap();
        crate::simd::xxhash3_stripes::accumulate_stripe(
            &mut accumulators,
            last_stripe,
            secret_window(SECRET_LIMIT - LAST_ACCUMULATION_SECRET_START),
        );
    } else {
        let catch_up_length = STRIPE_LENGTH - buffered_length;
        let mut last_stripe = [0u8; STRIPE_LENGTH];
        last_stripe[..catch_up_length].copy_from_slice(
            &buffer[INTERNAL_BUFFER_LENGTH - catch_up_length..INTERNAL_BUFFER_LENGTH],
        );
        last_stripe[catch_up_length..].copy_from_slice(&buffer[..buffered_length]);
        crate::simd::xxhash3_stripes::accumulate_stripe(
            &mut accumulators,
            &last_stripe,
            secret_window(SECRET_LIMIT - LAST_ACCUMULATION_SECRET_START),
        );
    }
    merge_accumulators(
        &accumulators,
        MERGE_ACCUMULATORS_SECRET_START,
        total_length.wrapping_mul(PRIME_64_1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern_bytes(length: usize) -> Vec<u8> {
        (0..length)
            .map(|index| ((index * 7 + 13) % 256) as u8)
            .collect()
    }

    #[test]
    fn matches_xxhsum_for_every_length_path() {
        let cases: [(usize, u64); 17] = [
            (0, 0x2d06800538d394c2),
            (1, 0x8a21d78b1538b1c0),
            (3, 0xb7e23e9c1ad24e4b),
            (4, 0x3fc0c554cc1bfd24),
            (8, 0x8e9d87b2621686c2),
            (9, 0xd78b2e5e6eee1a29),
            (16, 0x2255ac040382fb28),
            (17, 0x80297144ea363493),
            (128, 0xa03b5825ff901dc3),
            (129, 0x41ec3e4722a25af7),
            (240, 0x38b97a24f68efc13),
            (241, 0xa59556a86c6a6ea6),
            (256, 0xa778510b7ac1383e),
            (1024, 0xf9c1054610f5a6a3),
            (1025, 0x67372fe80e1bef4f),
            (1088, 0xf436e4305b526910),
            (100_000, 0x806b2ff4d823e1cc),
        ];

        for (length, expected) in cases {
            let input = pattern_bytes(length);
            assert_eq!(hash_bytes(&input), expected, "length {length}");
        }
    }

    #[test]
    fn streaming_matches_one_shot_for_chunk_sizes_crossing_internal_boundaries() {
        let input = pattern_bytes(100_000);
        let expected = 0x806b2ff4d823e1cc;
        assert_eq!(hash_bytes(&input), expected);

        for chunk_size in [1usize, 63, 64, 65, 255, 256, 257, 1023] {
            let mut hasher = XxHash3::new();
            for chunk in input.chunks(chunk_size) {
                hasher.update(chunk);
            }
            assert_eq!(hasher.finish(), expected, "chunk size {chunk_size}");
        }

        let short_input = pattern_bytes(240);
        let short_expected = 0x38b97a24f68efc13;
        let mut short_hasher = XxHash3::new();
        for chunk in short_input.chunks(17) {
            short_hasher.update(chunk);
        }
        assert_eq!(short_hasher.finish(), short_expected);

        let boundary_input = pattern_bytes(256);
        let boundary_expected = 0xa778510b7ac1383e;
        let mut boundary_hasher = XxHash3::new();
        for chunk in boundary_input.chunks(17) {
            boundary_hasher.update(chunk);
        }
        assert_eq!(boundary_hasher.finish(), boundary_expected);
    }
}
