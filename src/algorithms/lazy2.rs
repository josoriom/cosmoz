use crate::block::{
    repeat_offsets::RepeatOffsets, sequence_codes::MAX_MATCH_LENGTH,
    sequence_record::SequenceRecord,
};
use crate::levels::{MatchFinder, level_table::LevelParameters};
use crate::simd::row_tag_match::{match_row_tags16, match_row_tags32};

const TAG_BITS: u32 = 8;
const HASH_READ_SIZE: usize = 8;
const HASH_CACHE_SIZE: usize = 8;
const INPUT_TAIL_RESERVE: usize = HASH_READ_SIZE + HASH_CACHE_SIZE;
const SEARCH_STRENGTH: usize = 8;
const LAZY_SKIPPING_STEP: usize = 8;
const UPDATE_SKIP_THRESHOLD: usize = 384;
const UPDATE_MATCH_START_POSITIONS: usize = 96;
const UPDATE_MATCH_END_POSITIONS: usize = 32;
const SHORTEST_MATCH: usize = 4;
const MAX_ROW_ENTRIES: usize = 32;

const HASH_PRIME_FOUR: u64 = 0x9E37_79B1_85EB_CA87;
const HASH_PRIME_FIVE: u64 = 0x00CF_1BBC_DCBB;
const HASH_PRIME_SIX: u64 = 0xCF1B_BCDC_BF9B;
const HASH_PRIME_SEVEN: u64 = 0x00CF_1BBC_DCBF_A563;

pub struct Lazy2Finder<'tables> {
    positions: &'tables mut [u32],
    tags: &'tables mut [u8],
    row_log: u32,
    row_mask: usize,
    hash_shift: u32,
    key_shift: u32,
    hash_prime: u64,
    window_log: u8,
    search_attempts: usize,
    next_to_insert: usize,
    hash_cache: [u32; HASH_CACHE_SIZE],
    lazy_skipping: bool,
}

#[derive(Clone, Copy)]
struct Found {
    length: usize,
    offset_base: usize,
}

fn row_log_for(parameters: LevelParameters) -> u32 {
    if parameters.window_log >= 22 && parameters.hash_log >= 22 {
        5
    } else {
        4
    }
}

fn tag_bytes_from_words(words: &mut [u32]) -> &mut [u8] {
    let byte_length = words.len() * 4;
    unsafe { core::slice::from_raw_parts_mut(words.as_mut_ptr() as *mut u8, byte_length) }
}

#[inline(always)]
fn prefetch_read(_ptr: *const u8) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::x86_64::_mm_prefetch(_ptr as *const i8, core::arch::x86_64::_MM_HINT_T0);
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!(
            "prfm pldl1keep, [{0}]",
            in(reg) _ptr,
            options(nostack, preserves_flags, readonly),
        );
    }
}

fn highest_bit(value: usize) -> i64 {
    (u32::BITS - 1 - (value as u32 | 1).leading_zeros()) as i64
}

fn offset_base_for_distance(distance: usize) -> usize {
    distance + 3
}

const REPEAT_OFFSET_BASE: usize = 1;

impl<'tables> Lazy2Finder<'tables> {
    pub fn new(
        hash_table: &'tables mut [u32],
        chain_table: &'tables mut [u32],
        parameters: LevelParameters,
    ) -> Self {
        let tags = tag_bytes_from_words(chain_table);
        let row_log = row_log_for(parameters);
        let capacity = hash_table.len().min(tags.len());
        let capacity_log = if capacity == 0 {
            0
        } else {
            usize::BITS - 1 - capacity.leading_zeros()
        };
        let total_log = capacity_log.min(parameters.hash_log as u32);
        let usable = if total_log >= row_log {
            1usize << total_log
        } else {
            0
        };
        let row_hash_log = total_log.saturating_sub(row_log);
        let (prime, used_bits) = match parameters.min_match {
            0..=4 => (HASH_PRIME_FOUR, 32u32),
            5 => (HASH_PRIME_FIVE, 40u32),
            6 => (HASH_PRIME_SIX, 48u32),
            _ => (HASH_PRIME_SEVEN, 56u32),
        };
        let row_entries = 1usize << row_log;
        Lazy2Finder {
            positions: &mut hash_table[..usable],
            tags: &mut tags[..usable],
            row_log,
            row_mask: row_entries - 1,
            hash_shift: 64 - (row_hash_log + TAG_BITS),
            key_shift: 64 - used_bits,
            hash_prime: prime,
            window_log: parameters.window_log,
            search_attempts: (1usize << parameters.search_log.min(31)).min(row_entries),
            next_to_insert: 0,
            hash_cache: [0; HASH_CACHE_SIZE],
            lazy_skipping: false,
        }
    }

    #[inline(always)]
    unsafe fn read_u32_unchecked(input: &[u8], position: usize) -> u32 {
        debug_assert!(position + 4 <= input.len());
        unsafe { core::ptr::read_unaligned(input.as_ptr().add(position) as *const u32) }
    }

    #[inline(always)]
    unsafe fn hash_at_unchecked(&self, input: &[u8], position: usize) -> u32 {
        debug_assert!(position + HASH_READ_SIZE <= input.len());
        let value =
            unsafe { core::ptr::read_unaligned(input.as_ptr().add(position) as *const u64) };
        ((value << self.key_shift).wrapping_mul(self.hash_prime) >> self.hash_shift) as u32
    }

    #[inline(always)]
    fn row_base_of(&self, hash: u32) -> usize {
        ((hash >> TAG_BITS) as usize) << self.row_log
    }

    #[inline(always)]
    unsafe fn prefetch_row_unchecked(&self, hash: u32) {
        let row_base = self.row_base_of(hash);
        debug_assert!(row_base + self.row_mask < self.tags.len());
        unsafe {
            prefetch_read(self.positions.as_ptr().add(row_base) as *const u8);
            prefetch_read(self.tags.as_ptr().add(row_base));
        }
    }

    #[inline(always)]
    unsafe fn next_cached_hash_unchecked(&mut self, input: &[u8], position: usize) -> u32 {
        debug_assert!(position + INPUT_TAIL_RESERVE <= input.len());
        let new_hash = unsafe { self.hash_at_unchecked(input, position + HASH_CACHE_SIZE) };
        unsafe { self.prefetch_row_unchecked(new_hash) };
        let slot = position & (HASH_CACHE_SIZE - 1);
        let hash = self.hash_cache[slot];
        debug_assert_eq!(hash, unsafe { self.hash_at_unchecked(input, position) });
        self.hash_cache[slot] = new_hash;
        hash
    }

    unsafe fn fill_hash_cache_unchecked(
        &mut self,
        input: &[u8],
        start: usize,
        last_position: usize,
    ) {
        debug_assert!(last_position + INPUT_TAIL_RESERVE <= input.len());
        let end = (start + HASH_CACHE_SIZE).min(last_position + 1);
        let mut position = start;
        while position < end {
            let hash = unsafe { self.hash_at_unchecked(input, position) };
            unsafe { self.prefetch_row_unchecked(hash) };
            self.hash_cache[position & (HASH_CACHE_SIZE - 1)] = hash;
            position += 1;
        }
    }

    #[inline(always)]
    unsafe fn insert_unchecked(&mut self, hash: u32, position: usize) {
        let row_base = self.row_base_of(hash);
        debug_assert!(row_base + self.row_mask < self.tags.len());
        debug_assert!(row_base + self.row_mask < self.positions.len());
        unsafe {
            let head = self.tags.get_unchecked_mut(row_base);
            let mut next = (head.wrapping_sub(1) as usize) & self.row_mask;
            if next == 0 {
                next = self.row_mask;
            }
            *head = next as u8;
            *self.tags.get_unchecked_mut(row_base + next) = hash as u8;
            *self.positions.get_unchecked_mut(row_base + next) = position as u32;
        }
    }

    #[inline(always)]
    unsafe fn update_up_to_unchecked(&mut self, input: &[u8], target: usize) {
        let mut position = self.next_to_insert;
        if target > position + UPDATE_SKIP_THRESHOLD {
            let bound = position + UPDATE_MATCH_START_POSITIONS;
            while position < bound {
                let hash = unsafe { self.next_cached_hash_unchecked(input, position) };
                unsafe { self.insert_unchecked(hash, position) };
                position += 1;
            }
            position = target - UPDATE_MATCH_END_POSITIONS;
            unsafe { self.fill_hash_cache_unchecked(input, position, target) };
        }
        while position < target {
            let hash = unsafe { self.next_cached_hash_unchecked(input, position) };
            unsafe { self.insert_unchecked(hash, position) };
            position += 1;
        }
        if target > self.next_to_insert {
            self.next_to_insert = target;
        }
    }

    #[inline(always)]
    fn rotated_match_mask(&self, row_base: usize, tag: u8, head: usize) -> u64 {
        if self.row_mask == 15 {
            let tags: &[u8; 16] = self.tags[row_base..row_base + 16].try_into().unwrap();
            match_row_tags16(tags, tag).rotate_right(head as u32) as u64
        } else {
            let tags: &[u8; 32] = self.tags[row_base..row_base + 32].try_into().unwrap();
            match_row_tags32(tags, tag).rotate_right(head as u32) as u64
        }
    }

    #[inline(always)]
    unsafe fn search_unchecked(&mut self, input: &[u8], position: usize) -> Found {
        debug_assert!(position + INPUT_TAIL_RESERVE <= input.len());
        let hash = if self.lazy_skipping {
            self.next_to_insert = position;
            unsafe { self.hash_at_unchecked(input, position) }
        } else {
            unsafe {
                self.update_up_to_unchecked(input, position);
                self.next_cached_hash_unchecked(input, position)
            }
        };
        let row_base = self.row_base_of(hash);
        let head = self.tags[row_base] as usize & self.row_mask;
        let mut matches = self.rotated_match_mask(row_base, hash as u8, head);
        let lowest_valid = position.saturating_sub(1usize << self.window_log);

        let mut candidates = [0u32; MAX_ROW_ENTRIES];
        let mut candidate_count = 0usize;
        let mut attempts_left = self.search_attempts;
        while matches != 0 && attempts_left > 0 {
            let slot = (matches.trailing_zeros() as usize + head) & self.row_mask;
            matches &= matches - 1;
            if slot == 0 {
                continue;
            }
            let candidate = unsafe { *self.positions.get_unchecked(row_base + slot) } as usize;
            if candidate < lowest_valid {
                break;
            }
            if candidate >= position {
                continue;
            }
            prefetch_read(unsafe { input.as_ptr().add(candidate) });
            candidates[candidate_count] = candidate as u32;
            candidate_count += 1;
            attempts_left -= 1;
        }

        unsafe { self.insert_unchecked(hash, position) };
        self.next_to_insert = position + 1;

        let limit = (input.len() - position).min(MAX_MATCH_LENGTH as usize);
        let mut best = Found {
            length: SHORTEST_MATCH - 1,
            offset_base: 0,
        };
        for &candidate in &candidates[..candidate_count] {
            let candidate = candidate as usize;
            let probe = best.length - 3;
            let plausible = unsafe {
                Self::read_u32_unchecked(input, candidate + probe)
                    == Self::read_u32_unchecked(input, position + probe)
            };
            if !plausible {
                continue;
            }
            let length = unsafe { count_unchecked(input, position, candidate, limit) };
            if length > best.length {
                best = Found {
                    length,
                    offset_base: offset_base_for_distance(position - candidate),
                };
                if length == limit {
                    break;
                }
            }
        }
        best
    }

    #[inline(always)]
    unsafe fn repeat_length_unchecked(input: &[u8], position: usize, distance: usize) -> usize {
        debug_assert!(position + 4 <= input.len());
        if distance == 0 || distance > position {
            return 0;
        }
        let source = position - distance;
        if unsafe {
            Self::read_u32_unchecked(input, position) != Self::read_u32_unchecked(input, source)
        } {
            return 0;
        }
        let limit = (input.len() - position).min(MAX_MATCH_LENGTH as usize);
        unsafe { count_unchecked(input, position, source, limit) }
    }

    unsafe fn find_sequences_unchecked(
        &mut self,
        input: &[u8],
        block_start: usize,
        sequences: &mut [SequenceRecord],
        repeat_offsets: &mut RepeatOffsets,
    ) -> (usize, usize) {
        debug_assert!(block_start + INPUT_TAIL_RESERVE < input.len());
        let input_limit = input.len() - INPUT_TAIL_RESERVE;

        self.lazy_skipping = false;
        if self.next_to_insert < block_start.saturating_sub(UPDATE_SKIP_THRESHOLD) {
            self.next_to_insert = block_start.saturating_sub(UPDATE_MATCH_END_POSITIONS);
        }
        unsafe { self.fill_hash_cache_unchecked(input, self.next_to_insert, input_limit) };

        let mut position = block_start;
        let mut anchor = block_start;
        let mut sequence_count = 0usize;

        while position < input_limit && sequence_count < sequences.len() {
            let repeat_distance = repeat_offsets.first as usize;
            let mut match_length = 0usize;
            let mut offset_base = REPEAT_OFFSET_BASE;
            let mut start = position + 1;

            let repeat_length =
                unsafe { Self::repeat_length_unchecked(input, position + 1, repeat_distance) };
            if repeat_length >= SHORTEST_MATCH {
                match_length = repeat_length;
            }

            let found = unsafe { self.search_unchecked(input, position) };
            if found.length > match_length {
                match_length = found.length;
                offset_base = found.offset_base;
                start = position;
            }

            if match_length < SHORTEST_MATCH {
                let step = ((position - anchor) >> SEARCH_STRENGTH) + 1;
                position += step;
                self.lazy_skipping = step > LAZY_SKIPPING_STEP;
                continue;
            }

            while position < input_limit {
                position += 1;
                let repeat_length =
                    unsafe { Self::repeat_length_unchecked(input, position, repeat_distance) };
                if repeat_length >= SHORTEST_MATCH {
                    let repeat_gain = repeat_length as i64 * 3;
                    let current_gain = match_length as i64 * 3 - highest_bit(offset_base) + 1;
                    if repeat_gain > current_gain {
                        match_length = repeat_length;
                        offset_base = REPEAT_OFFSET_BASE;
                        start = position;
                    }
                }
                let found = unsafe { self.search_unchecked(input, position) };
                if found.length >= SHORTEST_MATCH {
                    let found_gain = found.length as i64 * 4 - highest_bit(found.offset_base);
                    let current_gain = match_length as i64 * 4 - highest_bit(offset_base) + 4;
                    if found_gain > current_gain {
                        match_length = found.length;
                        offset_base = found.offset_base;
                        start = position;
                        continue;
                    }
                }

                if position < input_limit {
                    position += 1;
                    let repeat_length =
                        unsafe { Self::repeat_length_unchecked(input, position, repeat_distance) };
                    if repeat_length >= SHORTEST_MATCH {
                        let repeat_gain = repeat_length as i64 * 4;
                        let current_gain = match_length as i64 * 4 - highest_bit(offset_base) + 1;
                        if repeat_gain > current_gain {
                            match_length = repeat_length;
                            offset_base = REPEAT_OFFSET_BASE;
                            start = position;
                        }
                    }
                    let found = unsafe { self.search_unchecked(input, position) };
                    if found.length >= SHORTEST_MATCH {
                        let found_gain = found.length as i64 * 4 - highest_bit(found.offset_base);
                        let current_gain = match_length as i64 * 4 - highest_bit(offset_base) + 7;
                        if found_gain > current_gain {
                            match_length = found.length;
                            offset_base = found.offset_base;
                            start = position;
                            continue;
                        }
                    }
                }
                break;
            }

            let distance = if offset_base == REPEAT_OFFSET_BASE {
                repeat_distance
            } else {
                let distance = offset_base - 3;
                while start > anchor
                    && start - distance > 0
                    && input[start - 1] == input[start - 1 - distance]
                {
                    start -= 1;
                    match_length += 1;
                }
                distance
            };

            let literal_length = (start - anchor) as u32;
            sequences[sequence_count] = SequenceRecord {
                literal_length,
                match_length: match_length as u32,
                offset_value: repeat_offsets.get_offset_value(distance as u32, literal_length),
            };
            sequence_count += 1;
            position = start + match_length;
            anchor = position;

            if self.lazy_skipping {
                unsafe { self.fill_hash_cache_unchecked(input, self.next_to_insert, input_limit) };
                self.lazy_skipping = false;
            }

            while position <= input_limit && sequence_count < sequences.len() {
                let second_distance = repeat_offsets.second as usize;
                let repeat_length =
                    unsafe { Self::repeat_length_unchecked(input, position, second_distance) };
                if repeat_length < SHORTEST_MATCH {
                    break;
                }
                sequences[sequence_count] = SequenceRecord {
                    literal_length: 0,
                    match_length: repeat_length as u32,
                    offset_value: repeat_offsets.get_offset_value(second_distance as u32, 0),
                };
                sequence_count += 1;
                position += repeat_length;
                anchor = position;
            }
        }

        (sequence_count, input.len() - anchor)
    }
}

#[inline(always)]
unsafe fn count_unchecked(input: &[u8], first: usize, second: usize, limit: usize) -> usize {
    debug_assert!(first + limit <= input.len());
    debug_assert!(second < first);
    unsafe {
        crate::simd::count_matching_bytes::count_matching_bytes_unchecked(
            input.as_ptr().add(first),
            input.as_ptr().add(second),
            limit,
        )
    }
}

impl MatchFinder for Lazy2Finder<'_> {
    fn reset(&mut self) {
        self.positions.fill(u32::MAX);
        self.tags.fill(0);
        self.next_to_insert = 0;
        self.lazy_skipping = false;
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
        let block_length = input.len() - block_start;
        if input.len() > u32::MAX as usize
            || sequences.is_empty()
            || self.positions.is_empty()
            || block_start + INPUT_TAIL_RESERVE >= input.len()
        {
            return (0, block_length);
        }
        unsafe { self.find_sequences_unchecked(input, block_start, sequences, repeat_offsets) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::levels::level_table::get_level_parameters;

    fn new_tables(parameters: LevelParameters) -> (Vec<u32>, Vec<u32>) {
        let hash_length = 1usize << parameters.hash_log;
        let chain_length = 1usize << parameters.chain_log;
        (vec![u32::MAX; hash_length], vec![0u32; chain_length])
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
            let source_position = cursor - offset as usize;
            assert!(
                source_position >= block_start,
                "offset reached before chunk start"
            );

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
    fn lazy2_finder_covers_the_whole_input_on_fuzz_shapes() {
        let mut random_state = 0x9999_1111u32;

        for case in 0..200u32 {
            let shape = case % 3;
            let length = 200 + (case as usize % 4000);
            let input = match shape {
                0 => generate_pseudo_random_bytes(length, random_state ^ case),
                1 => {
                    let pattern_length = 1 + (case as usize % 13);
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
                    let words = [
                        b"the quick brown fox jumps".as_slice(),
                        b"over the lazy dog again and again ".as_slice(),
                    ];
                    let mut index = 0usize;
                    while bytes.len() < length {
                        bytes.extend_from_slice(words[index % words.len()]);
                        index += 1;
                    }
                    bytes.truncate(length);
                    bytes
                }
            };
            random_state = next_pseudo_random_number(&mut random_state);

            for level in [9, 12] {
                let parameters = get_level_parameters(level).unwrap();
                let (mut hash_table, mut chain_table) = new_tables(parameters);
                let mut finder = Lazy2Finder::new(&mut hash_table, &mut chain_table, parameters);

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
                    "case {case} level {level}"
                );
            }
        }
    }
}
