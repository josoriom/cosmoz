use crate::levels::{MIN_MATCH, MatchFinder};
use crate::{
    block::{
        repeat_offsets::RepeatOffsets, sequence_codes::MAX_MATCH_LENGTH,
        sequence_record::SequenceRecord,
    },
    simd::count_matching_bytes::count_matching_bytes_unchecked,
};

pub(crate) const HASH_LOG: usize = 16;
pub(crate) const HASH_TABLE_SIZE: usize = 1 << HASH_LOG;

const HASH_MULTIPLIER: u64 = 227_718_039_650_203;
const HASHED_BYTE_SHIFT: u32 = 16;
const HASH_READ_SIZE: usize = 8;
const SKIP_SHIFT: usize = 6;
const MIN_STEP: usize = 2;

pub(crate) struct FastFinder {
    pub positions: [u32; HASH_TABLE_SIZE],
    pub window_log: u8,
    base: u32,
    last_input_length: usize,
}

impl FastFinder {
    pub(crate) const fn new(window_log: u8) -> Self {
        FastFinder {
            positions: [0; HASH_TABLE_SIZE],
            window_log,
            base: 0,
            last_input_length: 0,
        }
    }
}

impl MatchFinder for FastFinder {
    fn reset(&mut self, input_length: usize) {
        let next_base = self.base as usize + self.last_input_length + 1;
        if next_base + input_length >= u32::MAX as usize {
            self.positions.fill(0);
            self.base = 0;
        } else {
            self.base = next_base as u32;
        }
        self.last_input_length = input_length;
    }

    fn window_log(&self) -> u8 {
        self.window_log
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

        if input.len() < HASH_READ_SIZE + 1 {
            return (0, block_length);
        }

        let scan_limit = input.len() - (HASH_READ_SIZE + 1);

        if block_start > scan_limit {
            return (0, block_length);
        }

        unsafe {
            self.find_sequences_unchecked(input, block_start, scan_limit, sequences, repeat_offsets)
        }
    }
}

impl FastFinder {
    unsafe fn find_sequences_unchecked(
        &mut self,
        input: &[u8],
        block_start: usize,
        scan_limit: usize,
        sequences: &mut [SequenceRecord],
        repeat_offsets: &mut RepeatOffsets,
    ) -> (usize, usize) {
        debug_assert!(block_start <= input.len());
        debug_assert!(input.len() > HASH_READ_SIZE);
        debug_assert!(scan_limit == input.len() - (HASH_READ_SIZE + 1));
        debug_assert!(block_start <= scan_limit);

        let window_size = 1usize << self.window_log;

        let mut position = block_start;
        let mut literal_start = block_start;
        let mut sequence_count = 0usize;

        while position <= scan_limit && sequence_count < sequences.len() {
            let next_position = position + 1;

            let value0 = unsafe { read_eight_bytes_unchecked(input, position) };
            let value1 = unsafe { read_eight_bytes_unchecked(input, next_position) };

            let hash0 = hash_table_index(value0, HASH_LOG as u32);
            let hash1 = hash_table_index(value1, HASH_LOG as u32);

            let candidate0 = self.positions[hash0].wrapping_sub(self.base);
            let candidate1 = self.positions[hash1].wrapping_sub(self.base);

            self.positions[hash0] = position as u32 + 1 + self.base;
            self.positions[hash1] = next_position as u32 + 1 + self.base;

            let repeat_offset = repeat_offsets.first as usize;

            let found = unsafe {
                candidate_match_unchecked(
                    input,
                    position,
                    value0,
                    candidate0,
                    repeat_offset,
                    window_size,
                )
                .or_else(|| {
                    candidate_match_unchecked(
                        input,
                        next_position,
                        value1,
                        candidate1,
                        repeat_offset,
                        window_size,
                    )
                })
            };

            let (matched_position, matched_source) = match found {
                Some(pair) => pair,
                None => {
                    let step = skip_step(position, literal_start).max(MIN_STEP);
                    position += step;
                    continue;
                }
            };

            let mut match_start = matched_position;
            let mut source_start = matched_source;

            while match_start > literal_start
                && source_start > 0
                && unsafe { read_byte_unchecked(input, match_start - 1) }
                    == unsafe { read_byte_unchecked(input, source_start - 1) }
            {
                match_start -= 1;
                source_start -= 1;
            }

            let remaining = input.len() - match_start;
            let match_length = unsafe {
                count_matching_bytes_unchecked(
                    input.as_ptr().add(source_start),
                    input.as_ptr().add(match_start),
                    remaining,
                )
            };
            debug_assert!(match_length >= MIN_MATCH);
            debug_assert!(match_length <= MAX_MATCH_LENGTH as usize);

            let literal_length = (match_start - literal_start) as u32;
            let offset = (match_start - source_start) as u32;
            let offset_value = repeat_offsets.get_offset_value(offset, literal_length);

            sequences[sequence_count] = SequenceRecord {
                literal_length,
                match_length: match_length as u32,
                offset_value,
            };
            sequence_count += 1;

            let match_end = match_start + match_length;
            self.insert_positions_after_match(input, match_end, scan_limit);

            position = match_end;
            literal_start = match_end;
        }

        let tail_literal_count = input.len() - literal_start;
        (sequence_count, tail_literal_count)
    }

    fn insert_positions_after_match(&mut self, input: &[u8], match_end: usize, scan_limit: usize) {
        let insert_base = match_end.saturating_sub(2);

        let mut offset = 0usize;
        while offset < 2 {
            let position = insert_base + offset;
            if position > scan_limit {
                break;
            }
            let hash = unsafe { hash_position_unchecked(input, position) };
            self.positions[hash] = position as u32 + 1 + self.base;
            offset += 1;
        }
    }
}

unsafe fn candidate_match_unchecked(
    input: &[u8],
    position: usize,
    value: u64,
    candidate: u32,
    repeat_offset: usize,
    window_size: usize,
) -> Option<(usize, usize)> {
    debug_assert!(position + 4 <= input.len());

    let low_four_bytes = value as u32;

    if repeat_offset != 0 && position >= repeat_offset {
        let source = position - repeat_offset;
        if low_four_bytes == unsafe { read_four_bytes_unchecked(input, source) } {
            return Some((position, source));
        }
    }

    if candidate != 0 {
        let candidate_position = (candidate - 1) as usize;
        if candidate_position < position
            && position - candidate_position <= window_size
            && low_four_bytes == unsafe { read_four_bytes_unchecked(input, candidate_position) }
        {
            return Some((position, candidate_position));
        }
    }

    None
}

fn skip_step(position: usize, literal_start: usize) -> usize {
    1 + ((position - literal_start) >> SKIP_SHIFT)
}

fn hash_table_index(value: u64, hash_log: u32) -> usize {
    ((value << HASHED_BYTE_SHIFT).wrapping_mul(HASH_MULTIPLIER) >> (64 - hash_log)) as usize
}

unsafe fn hash_position_unchecked(input: &[u8], position: usize) -> usize {
    let value = unsafe { read_eight_bytes_unchecked(input, position) };
    hash_table_index(value, HASH_LOG as u32)
}

unsafe fn read_eight_bytes_unchecked(input: &[u8], position: usize) -> u64 {
    debug_assert!(position + 8 <= input.len());
    unsafe {
        u64::from_le(core::ptr::read_unaligned(
            input.as_ptr().add(position) as *const u64
        ))
    }
}

unsafe fn read_four_bytes_unchecked(input: &[u8], position: usize) -> u32 {
    debug_assert!(position + 4 <= input.len());
    unsafe {
        u32::from_le(core::ptr::read_unaligned(
            input.as_ptr().add(position) as *const u32
        ))
    }
}

unsafe fn read_byte_unchecked(input: &[u8], position: usize) -> u8 {
    debug_assert!(position < input.len());
    unsafe { *input.as_ptr().add(position) }
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

        let mut finder = FastFinder::new(20);
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

        let mut finder = FastFinder::new(20);
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

        let mut finder = FastFinder::new(5);
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

        let mut finder = FastFinder::new(20);
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

        let mut finder = FastFinder::new(20);
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

    struct ReferenceFinder {
        positions: std::collections::HashMap<usize, usize>,
        window_log: u8,
    }

    impl ReferenceFinder {
        fn new(window_log: u8) -> Self {
            ReferenceFinder {
                positions: std::collections::HashMap::new(),
                window_log,
            }
        }

        fn hash_at(input: &[u8], position: usize) -> usize {
            let value = u64::from_le_bytes(input[position..position + 8].try_into().unwrap());
            hash_table_index(value, HASH_LOG as u32)
        }

        fn find_sequences(
            &mut self,
            input: &[u8],
            block_start: usize,
            sequences: &mut Vec<SequenceRecord>,
            repeat_offsets: &mut RepeatOffsets,
        ) -> usize {
            let window_size = 1usize << self.window_log;

            if input.len() < HASH_READ_SIZE + 1 {
                return input.len() - block_start;
            }

            let scan_limit = input.len() - (HASH_READ_SIZE + 1);
            let mut position = block_start;
            let mut literal_start = block_start;

            while position <= scan_limit {
                let hash = Self::hash_at(input, position);
                let candidate = self.positions.get(&hash).copied();
                self.positions.insert(hash, position);

                let repeat_offset = repeat_offsets.first as usize;
                let repeat_source = position.checked_sub(repeat_offset);

                let source = if repeat_offset != 0
                    && repeat_source.is_some()
                    && input[position..position + 4]
                        == input[repeat_source.unwrap()..repeat_source.unwrap() + 4]
                {
                    repeat_source
                } else {
                    candidate.filter(|&candidate| {
                        candidate < position
                            && position - candidate <= window_size
                            && input[position..position + 4] == input[candidate..candidate + 4]
                    })
                };

                let source = match source {
                    Some(source) => source,
                    None => {
                        position += 1;
                        continue;
                    }
                };

                let mut match_start = position;
                let mut source_start = source;
                while match_start > literal_start
                    && source_start > 0
                    && input[match_start - 1] == input[source_start - 1]
                {
                    match_start -= 1;
                    source_start -= 1;
                }

                let mut match_length = 4 + (position - match_start);
                while match_start + match_length < input.len()
                    && input[source_start + match_length] == input[match_start + match_length]
                {
                    match_length += 1;
                }

                let literal_length = (match_start - literal_start) as u32;
                let offset = (match_start - source_start) as u32;
                let offset_value = repeat_offsets.get_offset_value(offset, literal_length);

                sequences.push(SequenceRecord {
                    literal_length,
                    match_length: match_length as u32,
                    offset_value,
                });

                position = match_start + match_length;
                literal_start = position;
            }

            input.len() - literal_start
        }
    }

    #[test]
    fn unchecked_finder_matches_a_reference_finder_on_fuzz_shapes() {
        let mut random_state = 0x1357_9BDFu32;

        for case in 0..500u32 {
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

            let mut finder = FastFinder::new(20);
            let mut repeat_offsets = RepeatOffsets::new();
            let mut decoder_history = RepeatOffsets::new();
            let mut sequences = vec![SequenceRecord::default(); input.len() / 2 + 4];

            let (sequence_count, tail_literal_count) =
                finder.find_sequences(&input, 0, &mut sequences, &mut repeat_offsets);
            let (_, covered, _) = resolve_and_verify_sequences(
                &input,
                0,
                &sequences[..sequence_count],
                &mut decoder_history,
            );
            assert_eq!(covered + tail_literal_count, input.len());

            let unchecked_total: usize = sequences[..sequence_count]
                .iter()
                .map(|sequence| sequence.literal_length as usize + sequence.match_length as usize)
                .sum::<usize>()
                + tail_literal_count;

            let mut reference_finder = ReferenceFinder::new(20);
            let mut reference_repeat_offsets = RepeatOffsets::new();
            let mut reference_decoder_history = RepeatOffsets::new();
            let mut reference_sequences = Vec::new();
            let reference_tail = reference_finder.find_sequences(
                &input,
                0,
                &mut reference_sequences,
                &mut reference_repeat_offsets,
            );
            let (_, reference_covered, _) = resolve_and_verify_sequences(
                &input,
                0,
                &reference_sequences,
                &mut reference_decoder_history,
            );
            assert_eq!(reference_covered + reference_tail, input.len());

            let reference_total: usize = reference_sequences
                .iter()
                .map(|sequence| sequence.literal_length as usize + sequence.match_length as usize)
                .sum::<usize>()
                + reference_tail;

            assert_eq!(unchecked_total, input.len());
            assert_eq!(reference_total, input.len());
        }
    }
}
