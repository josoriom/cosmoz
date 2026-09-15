const PRIME_1: u64 = 0x9E3779B185EBCA87;
const PRIME_2: u64 = 0xC2B2AE3D27D4EB4F;
const PRIME_3: u64 = 0x165667B19E3779F9;
const PRIME_4: u64 = 0x85EBCA77C2B2AE63;
const PRIME_5: u64 = 0x27D4EB2F165667C5;

const STRIPE_LENGTH: usize = 32;

pub struct XxHash64 {
    seed: u64,
    lane_1: u64,
    lane_2: u64,
    lane_3: u64,
    lane_4: u64,
    pending: [u8; 32],
    pending_count: usize,
    total_length: u64,
}

impl XxHash64 {
    pub fn new(seed: u64) -> Self {
        XxHash64 {
            seed,
            lane_1: seed.wrapping_add(PRIME_1).wrapping_add(PRIME_2),
            lane_2: seed.wrapping_add(PRIME_2),
            lane_3: seed,
            lane_4: seed.wrapping_sub(PRIME_1),
            pending: [0; STRIPE_LENGTH],
            pending_count: 0,
            total_length: 0,
        }
    }

    pub fn update(&mut self, input: &[u8]) {
        self.total_length = self.total_length.wrapping_add(input.len() as u64);
        let mut remaining_input = input;

        if self.pending_count > 0 {
            let space_left = STRIPE_LENGTH - self.pending_count;
            let bytes_to_copy = space_left.min(remaining_input.len());
            self.pending[self.pending_count..self.pending_count + bytes_to_copy]
                .copy_from_slice(&remaining_input[..bytes_to_copy]);
            self.pending_count += bytes_to_copy;
            remaining_input = &remaining_input[bytes_to_copy..];

            if self.pending_count < STRIPE_LENGTH {
                return;
            }

            let completed_stripe = self.pending;
            self.process_stripe(&completed_stripe);
            self.pending_count = 0;
        }

        while remaining_input.len() >= STRIPE_LENGTH {
            self.process_stripe(&remaining_input[..STRIPE_LENGTH]);
            remaining_input = &remaining_input[STRIPE_LENGTH..];
        }

        if !remaining_input.is_empty() {
            self.pending[..remaining_input.len()].copy_from_slice(remaining_input);
            self.pending_count = remaining_input.len();
        }
    }

    fn process_stripe(&mut self, stripe: &[u8]) {
        let word_1 = read_u64_little_endian(&stripe[0..8]);
        let word_2 = read_u64_little_endian(&stripe[8..16]);
        let word_3 = read_u64_little_endian(&stripe[16..24]);
        let word_4 = read_u64_little_endian(&stripe[24..32]);
        self.lane_1 = mix_round(self.lane_1, word_1);
        self.lane_2 = mix_round(self.lane_2, word_2);
        self.lane_3 = mix_round(self.lane_3, word_3);
        self.lane_4 = mix_round(self.lane_4, word_4);
    }

    pub fn finish(&self) -> u64 {
        let mut accumulator = if self.total_length >= STRIPE_LENGTH as u64 {
            let converged = self
                .lane_1
                .rotate_left(1)
                .wrapping_add(self.lane_2.rotate_left(7))
                .wrapping_add(self.lane_3.rotate_left(12))
                .wrapping_add(self.lane_4.rotate_left(18));
            let converged = merge_lane(converged, self.lane_1);
            let converged = merge_lane(converged, self.lane_2);
            let converged = merge_lane(converged, self.lane_3);
            merge_lane(converged, self.lane_4)
        } else {
            self.seed.wrapping_add(PRIME_5)
        };

        accumulator = accumulator.wrapping_add(self.total_length);

        let mut remaining = &self.pending[..self.pending_count];

        while remaining.len() >= 8 {
            let word = read_u64_little_endian(&remaining[..8]);
            accumulator ^= mix_round(0, word);
            accumulator = accumulator.rotate_left(27).wrapping_mul(PRIME_1);
            accumulator = accumulator.wrapping_add(PRIME_4);
            remaining = &remaining[8..];
        }

        if remaining.len() >= 4 {
            let word = read_u32_little_endian(&remaining[..4]) as u64;
            accumulator ^= word.wrapping_mul(PRIME_1);
            accumulator = accumulator.rotate_left(23).wrapping_mul(PRIME_2);
            accumulator = accumulator.wrapping_add(PRIME_3);
            remaining = &remaining[4..];
        }

        while !remaining.is_empty() {
            let byte = remaining[0] as u64;
            accumulator ^= byte.wrapping_mul(PRIME_5);
            accumulator = accumulator.rotate_left(11).wrapping_mul(PRIME_1);
            remaining = &remaining[1..];
        }

        final_mix(accumulator)
    }
}

pub fn hash_bytes(input: &[u8], seed: u64) -> u64 {
    let mut hasher = XxHash64::new(seed);
    hasher.update(input);
    hasher.finish()
}

fn mix_round(lane: u64, input_word: u64) -> u64 {
    let updated_lane = lane.wrapping_add(input_word.wrapping_mul(PRIME_2));
    updated_lane.rotate_left(31).wrapping_mul(PRIME_1)
}

fn merge_lane(hash: u64, lane: u64) -> u64 {
    let mixed = hash ^ mix_round(0, lane);
    mixed.wrapping_mul(PRIME_1).wrapping_add(PRIME_4)
}

fn read_u64_little_endian(bytes: &[u8]) -> u64 {
    let mut buffer = [0u8; 8];
    let length = bytes.len().min(8);
    buffer[..length].copy_from_slice(&bytes[..length]);
    u64::from_le_bytes(buffer)
}

fn read_u32_little_endian(bytes: &[u8]) -> u32 {
    let mut buffer = [0u8; 4];
    let length = bytes.len().min(4);
    buffer[..length].copy_from_slice(&bytes[..length]);
    u32::from_le_bytes(buffer)
}

fn final_mix(hash: u64) -> u64 {
    let mixed = hash ^ (hash >> 33);
    let mixed = mixed.wrapping_mul(PRIME_2);
    let mixed = mixed ^ (mixed >> 29);
    let mixed = mixed.wrapping_mul(PRIME_3);
    mixed ^ (mixed >> 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern_bytes(length: usize) -> Vec<u8> {
        (0..length).map(|index| (index % 256) as u8).collect()
    }

    #[test]
    fn hash_of_empty_input_matches_known_vector() {
        assert_eq!(hash_bytes(&[], 0), 0xEF46DB3751D8E999);
    }

    #[test]
    fn hash_of_single_byte_matches_known_vector() {
        assert_eq!(hash_bytes(b"a", 0), 0xD24EC4F1A98C6E5B);
    }

    #[test]
    fn less_than_four_bytes_matches_zstd_checksum_oracle() {
        let input = b"abc";
        let low_32_bits = (hash_bytes(input, 0) & 0xFFFF_FFFF) as u32;
        assert_eq!(low_32_bits, 0xAD77_0999);
    }

    #[test]
    fn four_to_eight_byte_tail_matches_zstd_checksum_oracle() {
        let input = b"abcdef";
        let low_32_bits = (hash_bytes(input, 0) & 0xFFFF_FFFF) as u32;
        assert_eq!(low_32_bits, 0xC423_144D);
    }

    #[test]
    fn exactly_one_stripe_matches_zstd_checksum_oracle() {
        let input = pattern_bytes(32);
        let low_32_bits = (hash_bytes(&input, 0) & 0xFFFF_FFFF) as u32;
        assert_eq!(low_32_bits, 0x16FF_32B4);
    }

    #[test]
    fn one_stripe_plus_one_byte_matches_zstd_checksum_oracle() {
        let input = pattern_bytes(33);
        let low_32_bits = (hash_bytes(&input, 0) & 0xFFFF_FFFF) as u32;
        assert_eq!(low_32_bits, 0xCAFB_8EAD);
    }

    #[test]
    fn streaming_updates_in_any_chunk_size_match_single_call_and_oracle() {
        let input = pattern_bytes(1000);
        let expected_low_32_bits: u32 = 0x0EBA_4078;

        let whole_call_hash = hash_bytes(&input, 0);
        assert_eq!((whole_call_hash & 0xFFFF_FFFF) as u32, expected_low_32_bits);

        for chunk_size in [1usize, 7, 33] {
            let mut hasher = XxHash64::new(0);
            for chunk in input.chunks(chunk_size) {
                hasher.update(chunk);
            }
            let streamed_hash = hasher.finish();
            assert_eq!(streamed_hash, whole_call_hash);
        }
    }
}
