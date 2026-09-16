use super::{MatchFinder, level_table::LevelParameters};
use crate::{
    block::{
        repeat_offsets::RepeatOffsets, sequence_codes::MAX_MATCH_LENGTH,
        sequence_record::SequenceRecord,
    },
    simd::count_matching_bytes::count_matching_bytes,
};

const HASH_READ_SIZE: usize = 8;
const SKIP_SHIFT: usize = 8;
const NO_ENTRY: u32 = u32::MAX;

const PRIME_FOUR_BYTES: u64 = 0x0000_0000_9E37_79B1;
const PRIME_FIVE_BYTES: u64 = 0x0000_00CF_1BBC_DCBB;
const PRIME_SIX_BYTES: u64 = 0x0000_CF1B_BCDC_BF9B;
const PRIME_SEVEN_BYTES: u64 = 0x00CF_1BBC_DCBF_A563;
const PRIME_EIGHT_BYTES: u64 = 0xCF1B_BCDC_B7A5_6463;

pub struct DoubleFastFinder<'tables> {
    pub short_hash_table: &'tables mut [u32],
    pub long_hash_table: &'tables mut [u32],
    pub parameters: LevelParameters,
    pub base_position: u32,
}

impl<'tables> DoubleFastFinder<'tables> {
    pub fn new(
        short_hash_table: &'tables mut [u32],
        long_hash_table: &'tables mut [u32],
        parameters: LevelParameters,
    ) -> Self {
        debug_assert!(short_hash_table.len().is_power_of_two());
        debug_assert!(long_hash_table.len().is_power_of_two());
        DoubleFastFinder {
            short_hash_table,
            long_hash_table,
            parameters,
            base_position: 0,
        }
    }
}

impl MatchFinder for DoubleFastFinder<'_> {
    fn reset(&mut self) {
        self.short_hash_table.fill(NO_ENTRY);
        self.long_hash_table.fill(NO_ENTRY);
        self.base_position = 0;
    }

    fn window_log(&self) -> u8 {
        self.parameters.window_log
    }

    fn find_sequences(
        &mut self,
        input: &[u8],
        block_start: usize,
        sequences: &mut [SequenceRecord],
        repeat_offsets: &mut RepeatOffsets,
    ) -> (usize, usize) {
        debug_assert!(block_start <= input.len());

        let min_match = self.parameters.min_match as usize;
        let block_length = input.len() - block_start;

        if input.len() > u32::MAX as usize || sequences.is_empty() || block_length < min_match {
            return (0, block_length);
        }

        if input.len() < HASH_READ_SIZE + 1 {
            return (0, block_length);
        }

        let scan_limit = input.len() - (HASH_READ_SIZE + 1);

        if block_start > scan_limit {
            return (0, block_length);
        }

        self.find_sequences_within_bounds(input, block_start, scan_limit, sequences, repeat_offsets)
    }
}

impl DoubleFastFinder<'_> {
    fn find_sequences_within_bounds(
        &mut self,
        input: &[u8],
        block_start: usize,
        scan_limit: usize,
        sequences: &mut [SequenceRecord],
        repeat_offsets: &mut RepeatOffsets,
    ) -> (usize, usize) {
        let window_size = 1usize << self.parameters.window_log;
        let min_match = self.parameters.min_match as usize;
        let short_hash_bits = self.short_hash_table.len().trailing_zeros() as u8;
        let long_hash_bits = self.long_hash_table.len().trailing_zeros() as u8;

        let mut position = block_start;
        let mut literal_start = block_start;
        let mut sequence_count = 0usize;

        while position <= scan_limit && sequence_count < sequences.len() {
            let value_at_position = match read_eight_bytes(input, position) {
                Some(value) => value,
                None => break,
            };

            let long_index = hash_long(value_at_position, long_hash_bits);
            let short_index = hash_short(value_at_position, min_match, short_hash_bits);

            let long_candidate = self.long_hash_table[long_index];
            let short_candidate = self.short_hash_table[short_index];

            self.long_hash_table[long_index] = position as u32;
            self.short_hash_table[short_index] = position as u32;

            let next_position = position + 1;
            let repeat_first = repeat_offsets.first as usize;

            let repeat_match = if repeat_first != 0 && next_position >= repeat_first {
                let repeat_source = next_position - repeat_first;
                match (
                    read_four_bytes(input, next_position),
                    read_four_bytes(input, repeat_source),
                ) {
                    (Some(current), Some(source)) if current == source => Some(extend_match(
                        input,
                        literal_start,
                        next_position,
                        repeat_source,
                    )),
                    _ => None,
                }
            } else {
                None
            };

            let matched = if repeat_match.is_some() {
                repeat_match
            } else if self.is_candidate_valid(long_candidate, position, window_size)
                && read_eight_bytes(input, long_candidate as usize) == Some(value_at_position)
            {
                Some(extend_match(
                    input,
                    literal_start,
                    position,
                    long_candidate as usize,
                ))
            } else if self.is_candidate_valid(short_candidate, position, window_size)
                && matches_bytes(input, position, short_candidate as usize, min_match)
            {
                let short_result =
                    extend_match(input, literal_start, position, short_candidate as usize);

                let lookahead_result = if next_position <= scan_limit {
                    read_eight_bytes(input, next_position).and_then(|lookahead_value| {
                        let lookahead_long_index = hash_long(lookahead_value, long_hash_bits);
                        let lookahead_candidate = self.long_hash_table[lookahead_long_index];
                        if self.is_candidate_valid(lookahead_candidate, next_position, window_size)
                            && read_eight_bytes(input, lookahead_candidate as usize)
                                == Some(lookahead_value)
                        {
                            Some(extend_match(
                                input,
                                literal_start,
                                next_position,
                                lookahead_candidate as usize,
                            ))
                        } else {
                            None
                        }
                    })
                } else {
                    None
                };

                match lookahead_result {
                    Some(candidate) if candidate.2 > short_result.2 => Some(candidate),
                    _ => Some(short_result),
                }
            } else {
                None
            };

            let (match_start, source_start, match_length) = match matched {
                Some(triple) => triple,
                None => {
                    let step = 1 + ((position - literal_start) >> SKIP_SHIFT);
                    position += step;
                    continue;
                }
            };

            debug_assert!(match_length <= MAX_MATCH_LENGTH as usize);

            emit_sequence(
                sequences,
                &mut sequence_count,
                repeat_offsets,
                literal_start,
                match_start,
                source_start,
                match_length,
            );

            let match_end = match_start + match_length;
            self.insert_positions_after_match(input, match_end);
            position = match_end;
            literal_start = match_end;

            self.consume_immediate_repeats(
                input,
                &mut position,
                &mut literal_start,
                sequences,
                &mut sequence_count,
                repeat_offsets,
            );
        }

        let tail_literal_count = input.len() - literal_start;
        (sequence_count, tail_literal_count)
    }

    fn consume_immediate_repeats(
        &mut self,
        input: &[u8],
        position: &mut usize,
        literal_start: &mut usize,
        sequences: &mut [SequenceRecord],
        sequence_count: &mut usize,
        repeat_offsets: &mut RepeatOffsets,
    ) {
        loop {
            if *sequence_count >= sequences.len() {
                return;
            }

            let repeat_second = repeat_offsets.second as usize;
            if repeat_second == 0 || *position < repeat_second {
                return;
            }

            let current = read_four_bytes(input, *position);
            let source = read_four_bytes(input, *position - repeat_second);
            if current.is_none() || current != source {
                return;
            }

            let (match_start, source_start, match_length) =
                extend_match(input, *literal_start, *position, *position - repeat_second);

            debug_assert!(match_length <= MAX_MATCH_LENGTH as usize);

            emit_sequence(
                sequences,
                sequence_count,
                repeat_offsets,
                *literal_start,
                match_start,
                source_start,
                match_length,
            );

            let match_end = match_start + match_length;
            self.insert_positions_after_match(input, match_end);
            *position = match_end;
            *literal_start = match_end;
        }
    }

    fn insert_positions_after_match(&mut self, input: &[u8], match_end: usize) {
        let min_match = self.parameters.min_match as usize;
        let short_hash_bits = self.short_hash_table.len().trailing_zeros() as u8;
        let long_hash_bits = self.long_hash_table.len().trailing_zeros() as u8;

        for offset in [2usize, 1usize] {
            if match_end < offset {
                continue;
            }
            let position = match_end - offset;
            if let Some(value) = read_eight_bytes(input, position) {
                let long_index = hash_long(value, long_hash_bits);
                self.long_hash_table[long_index] = position as u32;
                let short_index = hash_short(value, min_match, short_hash_bits);
                self.short_hash_table[short_index] = position as u32;
            }
        }
    }

    fn is_candidate_valid(&self, candidate: u32, position: usize, window_size: usize) -> bool {
        candidate != NO_ENTRY
            && (candidate as usize) < position
            && (candidate as usize) >= self.base_position as usize
            && position - candidate as usize <= window_size
    }
}

fn emit_sequence(
    sequences: &mut [SequenceRecord],
    sequence_count: &mut usize,
    repeat_offsets: &mut RepeatOffsets,
    literal_start: usize,
    match_start: usize,
    source_start: usize,
    match_length: usize,
) {
    let literal_length = (match_start - literal_start) as u32;
    let offset = (match_start - source_start) as u32;
    let offset_value = repeat_offsets.get_offset_value(offset, literal_length);

    sequences[*sequence_count] = SequenceRecord {
        literal_length,
        match_length: match_length as u32,
        offset_value,
    };
    *sequence_count += 1;
}

fn extend_match(
    input: &[u8],
    literal_start: usize,
    position: usize,
    source: usize,
) -> (usize, usize, usize) {
    let mut match_start = position;
    let mut source_start = source;

    while match_start > literal_start
        && source_start > 0
        && input[match_start - 1] == input[source_start - 1]
    {
        match_start -= 1;
        source_start -= 1;
    }

    let match_length = count_matching_bytes(&input[source_start..], &input[match_start..]);
    (match_start, source_start, match_length)
}

fn matches_bytes(input: &[u8], first: usize, second: usize, length: usize) -> bool {
    match (
        input.get(first..first + length),
        input.get(second..second + length),
    ) {
        (Some(first_bytes), Some(second_bytes)) => first_bytes == second_bytes,
        _ => false,
    }
}

fn read_eight_bytes(input: &[u8], position: usize) -> Option<u64> {
    input.get(position..position + 8).map(|bytes| {
        u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ])
    })
}

fn read_four_bytes(input: &[u8], position: usize) -> Option<u32> {
    input
        .get(position..position + 4)
        .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn hash_long(value: u64, hash_bits: u8) -> usize {
    debug_assert!(hash_bits > 0 && hash_bits < 64);
    (value.wrapping_mul(PRIME_EIGHT_BYTES) >> (64 - hash_bits as u32)) as usize
}

fn hash_short(value: u64, min_match: usize, hash_bits: u8) -> usize {
    debug_assert!((4..=7).contains(&min_match));
    debug_assert!(hash_bits > 0 && hash_bits < 64);

    let (prime, used_bits) = match min_match {
        4 => (PRIME_FOUR_BYTES, 32u32),
        5 => (PRIME_FIVE_BYTES, 40u32),
        6 => (PRIME_SIX_BYTES, 48u32),
        _ => (PRIME_SEVEN_BYTES, 56u32),
    };

    let shifted = value.wrapping_shl(64 - used_bits);
    (shifted.wrapping_mul(prime) >> (64 - hash_bits as u32)) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::match_finder::level_table::{Strategy, get_level_parameters};

    fn level_three_parameters() -> LevelParameters {
        get_level_parameters(3).expect("level 3 parameters must exist")
    }

    fn new_finder(parameters: LevelParameters) -> (Vec<u32>, Vec<u32>) {
        let short_hash_table = vec![NO_ENTRY; 1usize << parameters.hash_log];
        let long_hash_table = vec![NO_ENTRY; 1usize << parameters.chain_log];
        (short_hash_table, long_hash_table)
    }

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
    ) -> usize {
        let mut cursor = block_start;

        for sequence in sequences {
            cursor += sequence.literal_length as usize;
            let offset = decoder_history.get_offset(sequence.offset_value, sequence.literal_length);
            assert!(
                offset as usize <= cursor,
                "offset reaches before the chunk start"
            );
            let source_position = cursor - offset as usize;

            for index in 0..sequence.match_length as usize {
                assert_eq!(
                    input[source_position + index],
                    input[cursor + index],
                    "mismatched byte at match offset {index}"
                );
            }

            cursor += sequence.match_length as usize;
        }

        cursor - block_start
    }

    #[test]
    fn finds_long_match_over_short() {
        let parameters = level_three_parameters();
        assert_eq!(parameters.strategy, Strategy::DoubleFast);
        let (mut short_hash_table, mut long_hash_table) = new_finder(parameters);

        let mut input = Vec::new();
        input.extend(generate_pseudo_random_bytes(64, 0x1234_5678));

        let long_repeat: Vec<u8> = (0u8..24).collect();
        input.extend_from_slice(&long_repeat);
        input.extend(generate_pseudo_random_bytes(64, 0x9999_1111));
        input.extend_from_slice(&long_repeat);

        let short_repeat = b"ABCDE";
        input.extend(generate_pseudo_random_bytes(4, 0x2222_3333));
        input.extend_from_slice(short_repeat);
        input.extend(generate_pseudo_random_bytes(4, 0x4444_5555));
        input.extend_from_slice(short_repeat);
        input.extend_from_slice(&long_repeat);
        input.extend(generate_pseudo_random_bytes(64, 0x7777_8888));
        input.extend_from_slice(&long_repeat);
        input.extend(generate_pseudo_random_bytes(16, 0xAAAA_BBBB));

        let mut finder =
            DoubleFastFinder::new(&mut short_hash_table, &mut long_hash_table, parameters);
        let mut repeat_offsets = RepeatOffsets::new();
        let mut decoder_history = RepeatOffsets::new();
        let mut sequences = vec![SequenceRecord::default(); 64];

        let (sequence_count, tail_literal_count) =
            finder.find_sequences(&input, 0, &mut sequences, &mut repeat_offsets);

        let covered = resolve_and_verify_sequences(
            &input,
            0,
            &sequences[..sequence_count],
            &mut decoder_history,
        );
        assert_eq!(covered + tail_literal_count, input.len());

        let has_long_repeat_match = sequences[..sequence_count]
            .iter()
            .any(|sequence| sequence.match_length as usize >= long_repeat.len());
        assert!(
            has_long_repeat_match,
            "expected at least one match covering the 24-byte repeat"
        );
    }

    #[test]
    fn sequences_cover_block_and_every_match_is_valid_fuzz() {
        let parameters = level_three_parameters();
        let mut random_state = 0x1357_9BDFu32;

        for case in 0..300u32 {
            let shape = case % 4;
            let length = 200 + (case as usize % 3000);
            let input = match shape {
                0 => generate_pseudo_random_bytes(length, random_state ^ case),
                1 => {
                    let mut bytes = Vec::with_capacity(length);
                    let words = [
                        b"the quick brown fox jumps".as_slice(),
                        b"over the lazy dog again and again ".as_slice(),
                        b"pack my box with five dozen liquor jugs ".as_slice(),
                    ];
                    let mut index = 0usize;
                    while bytes.len() < length {
                        bytes.extend_from_slice(words[index % words.len()]);
                        index += 1;
                    }
                    bytes.truncate(length);
                    bytes
                }
                2 => {
                    let pattern_length = 1 + (case as usize % 11);
                    let pattern = generate_pseudo_random_bytes(pattern_length, random_state ^ case);
                    let mut bytes = Vec::with_capacity(length);
                    while bytes.len() < length {
                        bytes.extend_from_slice(&pattern);
                    }
                    bytes.truncate(length);
                    bytes
                }
                _ => {
                    let mut bytes = Vec::with_capacity(length);
                    let mut run_state = random_state ^ case;
                    while bytes.len() < length {
                        let run_length =
                            1 + (next_pseudo_random_number(&mut run_state) % 40) as usize;
                        let byte = (next_pseudo_random_number(&mut run_state) & 0xFF) as u8;
                        for _ in 0..run_length {
                            if bytes.len() >= length {
                                break;
                            }
                            bytes.push(byte);
                        }
                    }
                    bytes
                }
            };

            random_state = next_pseudo_random_number(&mut random_state);

            let (mut short_hash_table, mut long_hash_table) = new_finder(parameters);
            let mut finder =
                DoubleFastFinder::new(&mut short_hash_table, &mut long_hash_table, parameters);
            let mut repeat_offsets = RepeatOffsets::new();
            let mut decoder_history = RepeatOffsets::new();
            let mut sequences = vec![SequenceRecord::default(); input.len() / 2 + 4];

            let (sequence_count, tail_literal_count) =
                finder.find_sequences(&input, 0, &mut sequences, &mut repeat_offsets);

            let covered = resolve_and_verify_sequences(
                &input,
                0,
                &sequences[..sequence_count],
                &mut decoder_history,
            );
            assert_eq!(
                covered + tail_literal_count,
                input.len(),
                "case {case} shape {shape} did not cover the whole input"
            );
        }
    }

    #[test]
    fn respects_window_size() {
        let mut parameters = level_three_parameters();
        parameters.window_log = 5;
        let (mut short_hash_table, mut long_hash_table) = new_finder(parameters);

        let mut input = Vec::new();
        let pattern: Vec<u8> = (0u8..20).collect();
        input.extend_from_slice(&pattern);
        input.extend(generate_pseudo_random_bytes(100, 0x1111_2222));
        input.extend_from_slice(&pattern);
        input.extend(generate_pseudo_random_bytes(16, 0x3333_4444));

        let mut finder =
            DoubleFastFinder::new(&mut short_hash_table, &mut long_hash_table, parameters);
        let mut repeat_offsets = RepeatOffsets::new();
        let mut decoder_history = RepeatOffsets::new();
        let mut sequences = vec![SequenceRecord::default(); 32];

        let (sequence_count, tail_literal_count) =
            finder.find_sequences(&input, 0, &mut sequences, &mut repeat_offsets);

        let covered = resolve_and_verify_sequences(
            &input,
            0,
            &sequences[..sequence_count],
            &mut decoder_history,
        );
        assert_eq!(covered + tail_literal_count, input.len());
    }

    #[test]
    fn reset_clears_tables_and_base_position() {
        let parameters = level_three_parameters();
        let (mut short_hash_table, mut long_hash_table) = new_finder(parameters);
        short_hash_table[0] = 7;
        long_hash_table[0] = 9;

        let mut finder =
            DoubleFastFinder::new(&mut short_hash_table, &mut long_hash_table, parameters);
        finder.base_position = 42;
        finder.reset();

        assert_eq!(finder.short_hash_table[0], NO_ENTRY);
        assert_eq!(finder.long_hash_table[0], NO_ENTRY);
        assert_eq!(finder.base_position, 0);
    }
}
