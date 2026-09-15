use super::{MIN_MATCH, MatchFinder};
use crate::block::repeat_offsets::RepeatOffsets;
use crate::block::sequence_codes::MAX_MATCH_LENGTH;
use crate::block::sequence_record::SequenceRecord;
use crate::simd::count_matching_bytes::count_matching_bytes;
use crate::simd::hash_positions::hash_four_positions;

pub const HASH_LOG: usize = 16;
pub const HASH_TABLE_SIZE: usize = 1 << HASH_LOG;

const HASH_MULTIPLIER: u32 = 0x9E37_79B1;
const SKIP_SHIFT: usize = 6;

pub struct HashTableFinder {
    pub positions: [u32; HASH_TABLE_SIZE],
    pub window_log: u8,
}

impl HashTableFinder {
    pub const fn new(window_log: u8) -> Self {
        HashTableFinder {
            positions: [u32::MAX; HASH_TABLE_SIZE],
            window_log,
        }
    }
}

impl MatchFinder for HashTableFinder {
    fn reset(&mut self) {
        self.positions = [u32::MAX; HASH_TABLE_SIZE];
    }

    fn find_sequences(
        &mut self,
        input: &[u8],
        block_start: usize,
        sequences: &mut [SequenceRecord],
        repeat_offsets: &mut RepeatOffsets,
    ) -> (usize, usize) {
        debug_assert!(block_start <= input.len());

        let block_length = input.len() - block_start;

        if input.len() > u32::MAX as usize || sequences.is_empty() || block_length < MIN_MATCH {
            return (0, block_length);
        }

        let scan_end = input.len() - MIN_MATCH;
        let window_size = 1usize << self.window_log;

        let mut position = block_start;
        let mut literal_start = block_start;
        let mut sequence_count = 0usize;

        while position <= scan_end && sequence_count < sequences.len() {
            let hash = hash_single_position(input, position);
            let hash_candidate = self.positions[hash];

            let repeat_offset = repeat_offsets.first as usize;
            let repeat_is_valid = repeat_offset != 0
                && position >= repeat_offset
                && read_four_bytes(input, position)
                    == read_four_bytes(input, position - repeat_offset);

            let hash_is_valid = hash_candidate != u32::MAX
                && (hash_candidate as usize) < position
                && position - hash_candidate as usize <= window_size
                && read_four_bytes(input, position)
                    == read_four_bytes(input, hash_candidate as usize);

            self.positions[hash] = position as u32;

            let source_position = if repeat_is_valid {
                position - repeat_offset
            } else if hash_is_valid {
                hash_candidate as usize
            } else {
                position += skip_step(position, literal_start);
                continue;
            };

            let match_length = count_matching_bytes(&input[source_position..], &input[position..]);
            debug_assert!(match_length >= MIN_MATCH);
            debug_assert!(match_length <= MAX_MATCH_LENGTH as usize);

            let literal_length = (position - literal_start) as u32;
            let offset = (position - source_position) as u32;
            let offset_value = repeat_offsets.get_offset_value(offset, literal_length);

            sequences[sequence_count] = SequenceRecord {
                literal_length,
                match_length: match_length as u32,
                offset_value,
            };
            sequence_count += 1;

            let match_end = position + match_length;
            self.insert_positions_after_match(input, match_end, scan_end);

            position = match_end;
            literal_start = match_end;
        }

        let tail_literal_count = input.len() - literal_start;
        (sequence_count, tail_literal_count)
    }
}

impl HashTableFinder {
    fn insert_positions_after_match(&mut self, input: &[u8], match_end: usize, scan_end: usize) {
        let insert_base = match_end.saturating_sub(4);

        if insert_base + 7 < input.len() {
            let hashes = hash_four_positions(input, insert_base, HASH_LOG as u32);
            let mut lane = 0usize;
            while lane < 4 {
                self.positions[hashes[lane] as usize] = (insert_base + lane) as u32;
                lane += 1;
            }
            return;
        }

        let insert_position = match_end.saturating_sub(2);
        if insert_position >= match_end.saturating_sub(4) && insert_position <= scan_end {
            let hash = hash_single_position(input, insert_position);
            self.positions[hash] = insert_position as u32;
        }
    }
}

fn skip_step(position: usize, literal_start: usize) -> usize {
    1 + ((position - literal_start) >> SKIP_SHIFT)
}

fn hash_single_position(input: &[u8], position: usize) -> usize {
    let value = read_four_bytes(input, position);
    let hashed = value.wrapping_mul(HASH_MULTIPLIER);
    (hashed >> (32 - HASH_LOG as u32)) as usize
}

fn read_four_bytes(input: &[u8], position: usize) -> u32 {
    u32::from_le_bytes(input[position..position + 4].try_into().unwrap())
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

    fn resolve_and_verify_sequences(
        input: &[u8],
        block_start: usize,
        sequences: &[SequenceRecord],
        decoder_history: &mut RepeatOffsets,
    ) -> (bool, usize, Vec<u32>) {
        let mut cursor = block_start;
        let mut source_before_block_start = false;
        let mut resolved_offsets = Vec::with_capacity(sequences.len());

        for sequence in sequences {
            cursor += sequence.literal_length as usize;
            let offset = decoder_history.get_offset(sequence.offset_value, sequence.literal_length);
            resolved_offsets.push(offset);
            let source_position = cursor - offset as usize;

            if source_position < block_start {
                source_before_block_start = true;
            }

            for index in 0..sequence.match_length as usize {
                assert_eq!(
                    input[source_position + index],
                    input[cursor + index],
                    "mismatched byte at match offset {index}"
                );
            }

            cursor += sequence.match_length as usize;
        }

        (
            source_before_block_start,
            cursor - block_start,
            resolved_offsets,
        )
    }

    #[test]
    fn finds_repeating_pattern_matches() {
        let pattern = b"abc";
        let mut input = Vec::new();
        while input.len() < 60 {
            input.extend_from_slice(pattern);
        }

        let mut finder = HashTableFinder::new(20);
        let mut repeat_offsets = RepeatOffsets::new();
        let mut sequences = [SequenceRecord::default(); 8];

        let (sequence_count, tail_literal_count) =
            finder.find_sequences(&input, 0, &mut sequences, &mut repeat_offsets);

        assert_eq!(sequence_count, 1);
        assert_eq!(tail_literal_count, 0);
        assert_eq!(sequences[0].literal_length, 3);
        assert_eq!(sequences[0].match_length, 57);
        assert_eq!(sequences[0].offset_value, 6);
    }

    #[test]
    fn sequences_cover_block_and_every_match_is_valid() {
        let paragraph = generate_pseudo_random_bytes(4000, 0x9E37_79B1);
        let mut input = Vec::with_capacity(100_000);
        while input.len() < 100_000 {
            input.extend_from_slice(&paragraph);
        }
        input.truncate(100_000);

        let mut finder = HashTableFinder::new(20);
        let mut repeat_offsets = RepeatOffsets::new();
        let mut decoder_history = RepeatOffsets::new();
        let mut sequences = vec![SequenceRecord::default(); 4096];

        let first_block_input = &input[..50_000];
        let (first_sequence_count, first_tail_literal_count) =
            finder.find_sequences(first_block_input, 0, &mut sequences, &mut repeat_offsets);

        let (_, first_covered, _) = resolve_and_verify_sequences(
            first_block_input,
            0,
            &sequences[..first_sequence_count],
            &mut decoder_history,
        );
        assert_eq!(first_covered + first_tail_literal_count, 50_000);

        let mut second_sequences = vec![SequenceRecord::default(); 4096];
        let (second_sequence_count, second_tail_literal_count) =
            finder.find_sequences(&input, 50_000, &mut second_sequences, &mut repeat_offsets);

        let (reaches_into_first_block, second_covered, _) = resolve_and_verify_sequences(
            &input,
            50_000,
            &second_sequences[..second_sequence_count],
            &mut decoder_history,
        );
        assert_eq!(second_covered + second_tail_literal_count, 50_000);
        assert!(reaches_into_first_block);
    }

    #[test]
    fn respects_window_and_sequence_capacity() {
        let mut input = Vec::new();
        input.extend_from_slice(b"FARF");
        input.extend(generate_pseudo_random_bytes(40, 0x1111_1111));
        input.extend_from_slice(b"FARF");
        input.extend(generate_pseudo_random_bytes(4, 0x2222_2222));
        input.extend_from_slice(b"REPEATED_TOKEN_ONE_");
        input.extend(generate_pseudo_random_bytes(4, 0x3333_3333));
        input.extend_from_slice(b"REPEATED_TOKEN_ONE_");
        input.extend(generate_pseudo_random_bytes(4, 0x4444_4444));
        input.extend_from_slice(b"REPEATED_TOKEN_TWO_");
        input.extend(generate_pseudo_random_bytes(4, 0x5555_5555));
        input.extend_from_slice(b"REPEATED_TOKEN_TWO_");
        input.extend(generate_pseudo_random_bytes(20, 0x6666_6666));

        let mut finder = HashTableFinder::new(5);
        let mut repeat_offsets = RepeatOffsets::new();
        let mut decoder_history = RepeatOffsets::new();
        let mut sequences = [SequenceRecord::default(); 2];

        let (sequence_count, tail_literal_count) =
            finder.find_sequences(&input, 0, &mut sequences, &mut repeat_offsets);

        assert_eq!(sequence_count, 2);

        let (_, covered, resolved_offsets) = resolve_and_verify_sequences(
            &input,
            0,
            &sequences[..sequence_count],
            &mut decoder_history,
        );
        assert_eq!(covered + tail_literal_count, input.len());

        for resolved_offset in resolved_offsets {
            assert_ne!(resolved_offset, 44);
        }
    }

    #[test]
    fn random_input_is_mostly_literals_and_still_valid() {
        let input = generate_pseudo_random_bytes(64 * 1024, 0xC0FF_EE11);

        let mut finder = HashTableFinder::new(20);
        let mut repeat_offsets = RepeatOffsets::new();
        let mut decoder_history = RepeatOffsets::new();
        let mut sequences = vec![SequenceRecord::default(); 4096];

        let (sequence_count, tail_literal_count) =
            finder.find_sequences(&input, 0, &mut sequences, &mut repeat_offsets);

        let (_, covered, _) = resolve_and_verify_sequences(
            &input,
            0,
            &sequences[..sequence_count],
            &mut decoder_history,
        );
        assert_eq!(covered + tail_literal_count, input.len());
        assert!(sequence_count < 50);
    }

    #[test]
    fn skip_ahead_still_covers_incompressible_input_exactly() {
        let input = generate_pseudo_random_bytes(200_000, 0xFACE_FEED);

        let mut finder = HashTableFinder::new(20);
        let mut repeat_offsets = RepeatOffsets::new();
        let mut decoder_history = RepeatOffsets::new();
        let mut sequences = vec![SequenceRecord::default(); 8192];

        let (sequence_count, tail_literal_count) =
            finder.find_sequences(&input, 0, &mut sequences, &mut repeat_offsets);

        let (_, covered, _) = resolve_and_verify_sequences(
            &input,
            0,
            &sequences[..sequence_count],
            &mut decoder_history,
        );
        assert_eq!(covered + tail_literal_count, input.len());
    }
}
