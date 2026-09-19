use crate::{
    block::{repeat_offsets::RepeatOffsets, sequence_record::SequenceRecord},
    levels::{
        MatchFinder,
        level_table::{self, LevelParameters},
    },
    simd::count_matching_bytes::count_matching_bytes_by_words_unchecked,
};

pub(crate) const MAX_HASH_LOG: usize = 15;
pub(crate) const HASH_TABLE_SIZE: usize = 1 << MAX_HASH_LOG;

const READ_SIZE: usize = 8;
const SHORTEST_MATCH: usize = 4;
const FIRST_STEP: usize = 2;
const STEP_GROWTH_DISTANCE: usize = 128;

const PRIME_FOUR_BYTES: u32 = 2_654_435_761;
const PRIME_FIVE_BYTES: u64 = 889_523_592_379;
const PRIME_SIX_BYTES: u64 = 227_718_039_650_203;
const PRIME_SEVEN_BYTES: u64 = 58_295_818_150_454_627;

pub(crate) struct FastFinder {
    pub positions: [u32; HASH_TABLE_SIZE],
    window_log: u8,
    hash_log: u32,
    hashed_bytes: usize,
    first_index: u32,
    last_input_length: usize,
}

struct FoundMatch {
    start: usize,
    source: usize,
    known_length: usize,
    is_repeat: bool,
}

impl FastFinder {
    pub(crate) const fn new(parameters: LevelParameters) -> Self {
        FastFinder {
            positions: [0; HASH_TABLE_SIZE],
            window_log: parameters.window_log,
            hash_log: get_hash_log(parameters),
            hashed_bytes: get_hashed_bytes(parameters),
            first_index: 1,
            last_input_length: 0,
        }
    }

    fn use_parameters(&mut self, parameters: LevelParameters) {
        self.window_log = parameters.window_log;
        self.hash_log = get_hash_log(parameters);
        self.hashed_bytes = get_hashed_bytes(parameters);
    }
}

impl MatchFinder for FastFinder {
    fn reset(&mut self, input_length: usize) {
        self.use_parameters(level_table::get_level_one_parameters_for_input_length(
            input_length,
        ));
        let next_first_index = self.first_index as usize + self.last_input_length + 1;
        if next_first_index + input_length >= u32::MAX as usize {
            self.positions.fill(0);
            self.first_index = 1;
        } else {
            self.first_index = next_first_index as u32;
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
        if input.len() > u32::MAX as usize || block_length <= READ_SIZE {
            return (0, block_length);
        }

        unsafe {
            match self.hashed_bytes {
                5 => self.search::<5>(input, block_start, sequences, repeat_offsets),
                6 => self.search::<6>(input, block_start, sequences, repeat_offsets),
                7 => self.search::<7>(input, block_start, sequences, repeat_offsets),
                _ => self.search::<4>(input, block_start, sequences, repeat_offsets),
            }
        }
    }
}

impl FastFinder {
    #[inline(always)]
    unsafe fn search<const HASHED_BYTES: usize>(
        &mut self,
        input: &[u8],
        block_start: usize,
        sequences: &mut [SequenceRecord],
        repeat_offsets: &mut RepeatOffsets,
    ) -> (usize, usize) {
        let window_size = 1usize << self.window_log;
        let input_end = input.len();
        let search_end = input_end - READ_SIZE;
        let window_start = input_end.saturating_sub(window_size);

        let mut table = PositionTable::<HASHED_BYTES> {
            positions: &mut self.positions,
            hash_log: self.hash_log,
            first_index: self.first_index,
        };

        let mut position = block_start + (block_start == window_start) as usize;
        let largest_repeat = position.min(window_size);
        let mut repeats = Repeats {
            first: allow_repeat(repeat_offsets.first, largest_repeat),
            second: allow_repeat(repeat_offsets.second, largest_repeat),
        };
        let mut list = SequenceList {
            sequences,
            count: 0,
            literal_start: block_start,
            repeat_offsets,
        };

        'search: while !list.is_full() {
            let mut step = FIRST_STEP;
            let mut next_step_position = position + STEP_GROWTH_DISTANCE;
            let mut next_position = position + 1;
            let mut far_position = position + step;
            let mut far_next_position = far_position + 1;
            if far_next_position >= search_end {
                break;
            }

            let mut hash = unsafe { table.get_hash(input, position) };
            let mut next_hash = unsafe { table.get_hash(input, next_position) };
            let mut candidate = unsafe { table.get_candidate(hash) };
            let mut last_saved_position;

            let found = loop {
                last_saved_position = position;
                unsafe { table.save_position(hash, position) };

                if repeats.first > 0
                    && unsafe { read_four_bytes(input, far_position) }
                        == unsafe { read_four_bytes(input, far_position - repeats.first) }
                {
                    unsafe { table.save_position(next_hash, next_position) };
                    break unsafe { extend_repeat_backward(input, far_position, repeats.first) };
                }

                if unsafe { is_match(input, position, candidate, window_start) } {
                    unsafe { table.save_position(next_hash, next_position) };
                    break unsafe {
                        extend_backward(
                            input,
                            position,
                            candidate,
                            list.literal_start,
                            window_start,
                        )
                    };
                }

                candidate = unsafe { table.get_candidate(next_hash) };
                hash = next_hash;
                next_hash = unsafe { table.get_hash(input, far_position) };
                position = next_position;
                next_position = far_position;
                far_position = far_next_position;

                last_saved_position = position;
                unsafe { table.save_position(hash, position) };

                if unsafe { is_match(input, position, candidate, window_start) } {
                    if step <= SHORTEST_MATCH {
                        unsafe { table.save_position(next_hash, next_position) };
                    }
                    break unsafe {
                        extend_backward(
                            input,
                            position,
                            candidate,
                            list.literal_start,
                            window_start,
                        )
                    };
                }

                candidate = unsafe { table.get_candidate(next_hash) };
                hash = next_hash;
                next_hash = unsafe { table.get_hash(input, far_position) };
                position = next_position;
                next_position = far_position;
                far_position = position + step;
                far_next_position = next_position + step;

                if far_position >= next_step_position {
                    step += 1;
                    next_step_position += STEP_GROWTH_DISTANCE;
                }
                if far_next_position >= search_end {
                    break 'search;
                }
            };

            if !found.is_repeat {
                repeats.use_new_offset(found.start - found.source);
            }
            let match_length = found.known_length
                + unsafe {
                    count_matching_bytes(
                        input,
                        found.source + found.known_length,
                        found.start + found.known_length,
                    )
                };
            list.add(found.start, found.source, match_length);
            position = found.start + match_length;

            if position <= search_end {
                unsafe {
                    table.save_position_at(input, last_saved_position + 2);
                    table.save_position_at(input, position - 2);
                    position = add_second_repeats(
                        input,
                        &mut table,
                        &mut list,
                        &mut repeats,
                        position,
                        search_end,
                    );
                }
            }
        }

        (list.count, input_end - list.literal_start)
    }
}

struct Repeats {
    first: usize,
    second: usize,
}

impl Repeats {
    #[inline(always)]
    fn use_new_offset(&mut self, offset: usize) {
        self.second = self.first;
        self.first = offset;
    }

    #[inline(always)]
    fn swap(&mut self) {
        core::mem::swap(&mut self.first, &mut self.second);
    }
}

struct SequenceList<'list> {
    sequences: &'list mut [SequenceRecord],
    count: usize,
    literal_start: usize,
    repeat_offsets: &'list mut RepeatOffsets,
}

impl SequenceList<'_> {
    #[inline(always)]
    fn is_full(&self) -> bool {
        self.count == self.sequences.len()
    }

    #[inline(always)]
    fn add(&mut self, start: usize, source: usize, match_length: usize) {
        let literal_length = (start - self.literal_start) as u32;
        let offset = (start - source) as u32;
        self.sequences[self.count] = SequenceRecord {
            literal_length,
            match_length: match_length as u32,
            offset_value: self.repeat_offsets.get_offset_value(offset, literal_length),
        };
        self.count += 1;
        self.literal_start = start + match_length;
    }
}

#[inline(always)]
unsafe fn add_second_repeats<const HASHED_BYTES: usize>(
    input: &[u8],
    table: &mut PositionTable<'_, HASHED_BYTES>,
    list: &mut SequenceList<'_>,
    repeats: &mut Repeats,
    mut position: usize,
    search_end: usize,
) -> usize {
    while repeats.second > 0
        && position <= search_end
        && !list.is_full()
        && unsafe { read_four_bytes(input, position) }
            == unsafe { read_four_bytes(input, position - repeats.second) }
    {
        let source = position - repeats.second;
        let match_length = SHORTEST_MATCH
            + unsafe {
                count_matching_bytes(input, source + SHORTEST_MATCH, position + SHORTEST_MATCH)
            };
        repeats.swap();
        unsafe { table.save_position_at(input, position) };
        list.add(position, source, match_length);
        position += match_length;
    }
    position
}

struct PositionTable<'table, const HASHED_BYTES: usize> {
    positions: &'table mut [u32; HASH_TABLE_SIZE],
    hash_log: u32,
    first_index: u32,
}

impl<const HASHED_BYTES: usize> PositionTable<'_, HASHED_BYTES> {
    #[inline(always)]
    unsafe fn get_hash(&self, input: &[u8], position: usize) -> usize {
        if HASHED_BYTES == 4 {
            let value = unsafe { read_four_bytes(input, position) };
            return (value.wrapping_mul(PRIME_FOUR_BYTES) >> (32 - self.hash_log)) as usize;
        }
        let value = unsafe { read_eight_bytes(input, position) };
        let (shift, prime) = match HASHED_BYTES {
            5 => (24, PRIME_FIVE_BYTES),
            6 => (16, PRIME_SIX_BYTES),
            _ => (8, PRIME_SEVEN_BYTES),
        };
        ((value << shift).wrapping_mul(prime) >> (64 - self.hash_log)) as usize
    }

    #[inline(always)]
    unsafe fn get_candidate(&self, hash: usize) -> usize {
        debug_assert!(hash < HASH_TABLE_SIZE);
        let stored = unsafe { *self.positions.get_unchecked(hash) };
        stored.wrapping_sub(self.first_index) as usize
    }

    #[inline(always)]
    unsafe fn save_position_at(&mut self, input: &[u8], position: usize) {
        let hash = unsafe { self.get_hash(input, position) };
        unsafe { self.save_position(hash, position) };
    }

    #[inline(always)]
    unsafe fn save_position(&mut self, hash: usize, position: usize) {
        debug_assert!(hash < HASH_TABLE_SIZE);
        let stored = position as u32 + self.first_index;
        unsafe { *self.positions.get_unchecked_mut(hash) = stored };
    }
}

const fn get_hash_log(parameters: LevelParameters) -> u32 {
    if parameters.hash_log as usize > MAX_HASH_LOG {
        MAX_HASH_LOG as u32
    } else {
        parameters.hash_log as u32
    }
}

const fn get_hashed_bytes(parameters: LevelParameters) -> usize {
    match parameters.min_match {
        0..=4 => 4,
        5 => 5,
        6 => 6,
        _ => 7,
    }
}

fn allow_repeat(offset: u32, largest_repeat: usize) -> usize {
    if offset as usize <= largest_repeat {
        offset as usize
    } else {
        0
    }
}

#[inline(always)]
unsafe fn is_match(input: &[u8], position: usize, candidate: usize, window_start: usize) -> bool {
    let is_inside_window = candidate.wrapping_sub(window_start) < position - window_start;
    is_inside_window
        && unsafe { read_four_bytes(input, candidate) == read_four_bytes(input, position) }
}

#[inline(always)]
unsafe fn extend_repeat_backward(input: &[u8], position: usize, repeat: usize) -> FoundMatch {
    let step_back =
        unsafe { read_byte(input, position - 1) == read_byte(input, position - 1 - repeat) }
            as usize;
    let start = position - step_back;
    FoundMatch {
        start,
        source: start - repeat,
        known_length: SHORTEST_MATCH + step_back,
        is_repeat: true,
    }
}

#[inline(always)]
unsafe fn extend_backward(
    input: &[u8],
    position: usize,
    candidate: usize,
    literal_start: usize,
    window_start: usize,
) -> FoundMatch {
    let mut start = position;
    let mut source = candidate;
    while start > literal_start
        && source > window_start
        && unsafe { read_byte(input, start - 1) == read_byte(input, source - 1) }
    {
        start -= 1;
        source -= 1;
    }
    FoundMatch {
        start,
        source,
        known_length: SHORTEST_MATCH + position - start,
        is_repeat: false,
    }
}

#[inline(always)]
unsafe fn count_matching_bytes(input: &[u8], source: usize, position: usize) -> usize {
    unsafe {
        count_matching_bytes_by_words_unchecked(
            input.as_ptr().add(source),
            input.as_ptr().add(position),
            input.len() - position,
        )
    }
}

#[inline(always)]
unsafe fn read_eight_bytes(input: &[u8], position: usize) -> u64 {
    debug_assert!(position + 8 <= input.len());
    unsafe {
        u64::from_le(core::ptr::read_unaligned(
            input.as_ptr().add(position) as *const u64
        ))
    }
}

#[inline(always)]
unsafe fn read_four_bytes(input: &[u8], position: usize) -> u32 {
    debug_assert!(position + 4 <= input.len());
    unsafe {
        u32::from_le(core::ptr::read_unaligned(
            input.as_ptr().add(position) as *const u32
        ))
    }
}

#[inline(always)]
unsafe fn read_byte(input: &[u8], position: usize) -> u8 {
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

    fn get_finder_for_input(input_length: usize) -> FastFinder {
        let mut finder = FastFinder::new(level_table::level_one_parameters());
        finder.reset(input_length);
        finder
    }

    fn get_finder_with_window_log(window_log: u8) -> FastFinder {
        let parameters = LevelParameters {
            window_log,
            ..level_table::level_one_parameters()
        };
        FastFinder::new(parameters)
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

    fn get_fuzz_input(case: u32, seed: u32) -> Vec<u8> {
        let length = 200 + (case as usize % 3000);
        match case % 4 {
            0 => generate_pseudo_random_bytes(length, seed),
            1 => {
                let words = [
                    b"the quick brown fox jumps".as_slice(),
                    b"over the lazy dog again and again ".as_slice(),
                    b"pack my box with five dozen liquor jugs ".as_slice(),
                ];
                let mut bytes = Vec::with_capacity(length);
                let mut index = 0usize;
                while bytes.len() < length {
                    bytes.extend_from_slice(words[index % words.len()]);
                    index += 1;
                }
                bytes.truncate(length);
                bytes
            }
            2 => {
                let pattern = generate_pseudo_random_bytes(1 + (case as usize % 11), seed);
                let mut bytes = Vec::with_capacity(length);
                while bytes.len() < length {
                    bytes.extend_from_slice(&pattern);
                }
                bytes.truncate(length);
                bytes
            }
            _ => {
                let mut bytes = Vec::with_capacity(length);
                let mut run_state = seed;
                while bytes.len() < length {
                    let run_length = 1 + (next_pseudo_random_number(&mut run_state) % 40) as usize;
                    let byte = (next_pseudo_random_number(&mut run_state) & 0xFF) as u8;
                    for _ in 0..run_length.min(length - bytes.len()) {
                        bytes.push(byte);
                    }
                }
                bytes
            }
        }
    }

    #[test]
    fn finds_repeating_pattern_matches() {
        let mut input = Vec::new();
        while input.len() < 60 {
            input.extend_from_slice(b"abc");
        }

        let mut finder = get_finder_for_input(input.len());
        let mut repeat_offsets = RepeatOffsets::new();
        let mut decoder_history = RepeatOffsets::new();
        let mut sequences = [SequenceRecord::default(); 8];

        let (sequence_count, tail_literal_count) =
            finder.find_sequences(&input, 0, &mut sequences, &mut repeat_offsets);

        assert_eq!(sequence_count, 1);
        let (_, covered, resolved_offsets) = resolve_and_verify_sequences(
            &input,
            0,
            &sequences[..sequence_count],
            &mut decoder_history,
        );
        assert_eq!(covered + tail_literal_count, input.len());
        assert_eq!(resolved_offsets, vec![3]);
        assert!(sequences[0].match_length >= 50);
    }

    #[test]
    fn sequences_cover_block_and_every_match_is_valid() {
        let paragraph = generate_pseudo_random_bytes(4000, 0x9E37_79B1);
        let mut input = Vec::with_capacity(100_000);
        while input.len() < 100_000 {
            input.extend_from_slice(&paragraph);
        }
        input.truncate(100_000);

        let mut finder = get_finder_for_input(input.len());
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

        let (second_sequence_count, second_tail_literal_count) =
            finder.find_sequences(&input, 50_000, &mut sequences, &mut repeat_offsets);
        let (reaches_into_first_block, second_covered, _) = resolve_and_verify_sequences(
            &input,
            50_000,
            &sequences[..second_sequence_count],
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

        let mut finder = get_finder_with_window_log(5);
        let mut repeat_offsets = RepeatOffsets::new();
        let mut decoder_history = RepeatOffsets::new();
        let mut sequences = [SequenceRecord::default(); 2];

        let (sequence_count, tail_literal_count) =
            finder.find_sequences(&input, 0, &mut sequences, &mut repeat_offsets);

        assert!(sequence_count <= 2);
        let (_, covered, resolved_offsets) = resolve_and_verify_sequences(
            &input,
            0,
            &sequences[..sequence_count],
            &mut decoder_history,
        );
        assert_eq!(covered + tail_literal_count, input.len());
        for resolved_offset in resolved_offsets {
            assert!(resolved_offset <= 32);
        }
    }

    #[test]
    fn random_input_is_mostly_literals_and_still_valid() {
        let input = generate_pseudo_random_bytes(64 * 1024, 0xC0FF_EE11);

        let mut finder = get_finder_for_input(input.len());
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
    fn every_hashed_byte_count_covers_fuzz_shapes_exactly() {
        let mut seed = 0x1357_9BDFu32;
        for case in 0..500u32 {
            let input = get_fuzz_input(case, seed ^ case);
            seed = next_pseudo_random_number(&mut seed);

            for min_match in 4..=7u8 {
                let parameters = LevelParameters {
                    min_match,
                    ..level_table::level_one_parameters()
                };
                let mut finder = FastFinder::new(parameters);
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
                assert_eq!(
                    (
                        repeat_offsets.first,
                        repeat_offsets.second,
                        repeat_offsets.third
                    ),
                    (
                        decoder_history.first,
                        decoder_history.second,
                        decoder_history.third
                    )
                );
            }
        }
    }

    #[test]
    fn stale_positions_from_an_earlier_input_are_never_used() {
        let first_input = generate_pseudo_random_bytes(10_000, 0xABCD_0001);
        let second_input = generate_pseudo_random_bytes(10_000, 0xABCD_0002);

        let mut finder = get_finder_for_input(first_input.len());
        let mut repeat_offsets = RepeatOffsets::new();
        let mut sequences = vec![SequenceRecord::default(); 4096];
        finder.find_sequences(&first_input, 0, &mut sequences, &mut repeat_offsets);

        finder.reset(second_input.len());
        let mut repeat_offsets = RepeatOffsets::new();
        let mut decoder_history = RepeatOffsets::new();
        let (sequence_count, tail_literal_count) =
            finder.find_sequences(&second_input, 0, &mut sequences, &mut repeat_offsets);
        let (_, covered, _) = resolve_and_verify_sequences(
            &second_input,
            0,
            &sequences[..sequence_count],
            &mut decoder_history,
        );
        assert_eq!(covered + tail_literal_count, second_input.len());
    }
}
