use super::{
    MIN_MATCH, MatchFinder,
    hash_chain_finder::{Candidate, SearchMethod},
    level_table::LevelParameters,
};
use crate::block::{
    repeat_offsets::RepeatOffsets, sequence_codes::MAX_MATCH_LENGTH,
    sequence_record::SequenceRecord,
};

pub const HASH_READ_SIZE: usize = 8;
const SEARCH_STRENGTH: usize = 8;
const TAG_BITS: u32 = 8;
const ROW_UPDATE_MAX_PENDING: usize = 96;
const ROW_UPDATE_INSERT_ON_SKIP: usize = 32;

const HASH_PRIME_FOUR: u64 = 0x9E37_79B1_85EB_CA87;
const HASH_PRIME_FIVE: u64 = 0x00CF_1BBC_DCBB;
const HASH_PRIME_SIX: u64 = 0xCF1B_BCDC_BF9B;
const HASH_PRIME_SEVEN: u64 = 0x00CF_1BBC_DCBF_A563;

struct Best {
    length: usize,
    distance: usize,
    is_repeat: bool,
    start: usize,
    gain_offset: u32,
}

pub struct RowHashFinder<'tables> {
    positions: &'tables mut [u32],
    heads: &'tables mut [u32],
    tags: &'tables mut [u8],
    row_entries: usize,
    num_rows: usize,
    parameters: LevelParameters,
    search_method: SearchMethod,
    base_position: u32,
    next_to_insert: u32,
    cached_position: u32,
    cached_row: usize,
    cached_tag: u8,
}

const NO_CACHED_POSITION: u32 = u32::MAX;

fn row_log_for(parameters: LevelParameters) -> u8 {
    if parameters.window_log >= 22 && parameters.hash_log >= 22 {
        5
    } else {
        4
    }
}

fn tag_bytes_from_words(words: &mut [u32], length: usize) -> &mut [u8] {
    let byte_capacity = words.len() * 4;
    let length = length.min(byte_capacity);
    unsafe { core::slice::from_raw_parts_mut(words.as_mut_ptr() as *mut u8, length) }
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

impl<'tables> RowHashFinder<'tables> {
    pub fn new(
        hash_table: &'tables mut [u32],
        chain_table: &'tables mut [u32],
        parameters: LevelParameters,
        search_method: SearchMethod,
    ) -> Self {
        let row_log = row_log_for(parameters);
        let row_entries = 1usize << row_log;
        let num_rows = (hash_table.len() >> row_log).max(1);
        let tag_length = num_rows * row_entries;
        let tag_words = tag_length.div_ceil(4);
        let heads_needed = num_rows.min(chain_table.len());
        let (heads_words, tag_words_slice) = chain_table.split_at_mut(heads_needed);
        let tag_words_slice = if tag_words_slice.len() > tag_words {
            &mut tag_words_slice[..tag_words]
        } else {
            tag_words_slice
        };
        let tags = tag_bytes_from_words(tag_words_slice, tag_length);
        RowHashFinder {
            positions: hash_table,
            heads: heads_words,
            tags,
            row_entries,
            num_rows,
            parameters,
            search_method,
            base_position: 0,
            next_to_insert: 0,
            cached_position: NO_CACHED_POSITION,
            cached_row: 0,
            cached_tag: 0,
        }
    }

    fn num_rows(&self) -> usize {
        self.num_rows
            .min(self.positions.len() / self.row_entries.max(1))
            .min(self.heads.len().max(1))
            .min((self.tags.len() / self.row_entries.max(1)).max(1))
            .max(1)
    }

    fn get_row_next(&self, row: usize) -> usize {
        if row >= self.heads.len() {
            return 0;
        }
        self.heads[row] as usize % self.row_entries
    }

    fn set_row_next(&mut self, row: usize, value: usize) {
        if row >= self.heads.len() {
            return;
        }
        self.heads[row] = value as u32;
    }

    fn set_tag(&mut self, index: usize, tag: u8) {
        if let Some(slot) = self.tags.get_mut(index) {
            *slot = tag;
        }
    }

    fn row_tag_mask(&self, row: usize, tag: u8) -> u32 {
        let row_entries = self.row_entries;
        let base = row * row_entries;
        if base + row_entries > self.tags.len() {
            return 0;
        }
        if row_entries == 32 {
            let tags: &[u8; 32] = self.tags[base..base + 32].try_into().unwrap();
            crate::simd::row_tag_match::match_row_tags32(tags, tag)
        } else {
            let tags: &[u8; 16] = self.tags[base..base + 16].try_into().unwrap();
            crate::simd::row_tag_match::match_row_tags16(tags, tag) as u32
        }
    }

    fn row_and_tag(&self, input: &[u8], position: usize) -> Option<(usize, u8)> {
        if self.positions.is_empty() {
            return None;
        }
        if self.cached_position as usize == position {
            return Some((self.cached_row, self.cached_tag));
        }
        Self::compute_row_and_tag(input, position, self.parameters, self.num_rows())
    }

    fn compute_row_and_tag(
        input: &[u8],
        position: usize,
        parameters: LevelParameters,
        num_rows: usize,
    ) -> Option<(usize, u8)> {
        let value = if position + HASH_READ_SIZE <= input.len() {
            unsafe { read_u64_le_unchecked(input, position) }
        } else {
            read_u64_le(input, position)?
        };
        let wide_log = (parameters.hash_log as u32 + TAG_BITS).min(56) as u8;
        let wide_hash = hash_bytes(value, parameters.min_match, wide_log);
        let row_mask = num_rows.saturating_sub(1);
        let row = (wide_hash >> TAG_BITS) & row_mask;
        let tag = (wide_hash & 0xFF) as u8;
        Some((row, tag))
    }

    fn prime_next_row(&mut self, input: &[u8], position: usize) {
        let next_position = position.wrapping_add(1);
        if self.positions.is_empty() || next_position >= input.len() {
            return;
        }
        if self.cached_position as usize == next_position {
            return;
        }
        let Some((row, tag)) =
            Self::compute_row_and_tag(input, next_position, self.parameters, self.num_rows())
        else {
            return;
        };
        self.cached_position = next_position as u32;
        self.cached_row = row;
        self.cached_tag = tag;
        if let Some(next_row_tags) = self.tags.get(row * self.row_entries) {
            prefetch_read(next_row_tags as *const u8);
        }
        let row_base = row * self.row_entries;
        if let Some(slot_value) = self.positions.get(row_base) {
            prefetch_read(slot_value as *const u32 as *const u8);
        }
    }

    fn insert_position(&mut self, input: &[u8], position: usize) {
        let Some((row, tag)) = self.row_and_tag(input, position) else {
            return;
        };
        self.insert_position_with_tag(input, position, row, tag);
    }

    fn insert_position_with_tag(&mut self, input: &[u8], position: usize, row: usize, tag: u8) {
        let slot = self.get_row_next(row);
        let index = row * self.row_entries + slot;
        if index < self.positions.len() {
            self.positions[index] = position as u32;
            self.set_tag(index, tag);
        }
        self.set_row_next(row, (slot + 1) % self.row_entries);
        if let Some(next_byte) = input.get(position.wrapping_add(HASH_READ_SIZE)) {
            prefetch_read(next_byte as *const u8);
        }
        if let Some(next_row_tags) = self.tags.get(row * self.row_entries) {
            prefetch_read(next_row_tags as *const u8);
        }
    }

    fn insert_positions_up_to(&mut self, input: &[u8], position: usize) {
        let mut insert_at = (self.next_to_insert as usize).max(self.base_position as usize);
        if position.saturating_sub(insert_at) > ROW_UPDATE_MAX_PENDING {
            insert_at = position - ROW_UPDATE_INSERT_ON_SKIP;
        }
        while insert_at < position {
            self.insert_position(input, insert_at);
            insert_at += 1;
        }
        if insert_at > self.next_to_insert as usize {
            self.next_to_insert = insert_at as u32;
        }
    }

    fn window_lowest(&self, position: usize) -> usize {
        let window_size = 1usize << self.parameters.window_log;
        position
            .saturating_sub(window_size)
            .max(self.base_position as usize)
    }

    fn search_max(&mut self, input: &[u8], position: usize, limit: usize) -> Option<Candidate> {
        self.insert_positions_up_to(input, position);

        let lowest_valid = self.window_lowest(position);
        let Some((row, tag)) = self.row_and_tag(input, position) else {
            self.next_to_insert = (position as u32).saturating_add(1);
            return None;
        };

        let row_entries = self.row_entries;

        self.insert_position_with_tag(input, position, row, tag);
        self.next_to_insert = (position as u32).saturating_add(1);
        self.prime_next_row(input, position);

        let search_max_attempts = (1usize << self.parameters.search_log.min(31)).min(row_entries);
        let tag_mask = self.row_tag_mask(row, tag);

        let positions_len = self.positions.len();
        let row_base = row * row_entries;
        let input_len = input.len();
        debug_assert!(position + limit <= input_len);
        let input_ptr = input.as_ptr();
        let row_in_bounds = row_base + row_entries <= positions_len;

        debug_assert!(position + limit <= input_len);
        debug_assert!(!row_in_bounds || row_base + row_entries <= positions_len);
        let (best_length, best_distance) = unsafe {
            self.candidate_loop_unchecked(
                input_ptr,
                input_len,
                position,
                limit,
                lowest_valid,
                row_base,
                search_max_attempts,
                tag_mask,
                row_in_bounds,
            )
        };

        if best_length < MIN_MATCH {
            return None;
        }
        Some(Candidate {
            length: best_length,
            distance: best_distance,
            offset_value: (best_distance as u32).saturating_add(3),
        })
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn candidate_loop_unchecked(
        &self,
        input_ptr: *const u8,
        input_len: usize,
        position: usize,
        limit: usize,
        lowest_valid: usize,
        row_base: usize,
        search_max_attempts: usize,
        tag_mask: u32,
        row_in_bounds: bool,
    ) -> (usize, usize) {
        debug_assert!(position + limit <= input_len);
        debug_assert!(!row_in_bounds || row_base + self.row_entries <= self.positions.len());
        let mut best_length = 0usize;
        let mut best_distance = 0usize;
        let positions_len = self.positions.len();
        let max_safe_check_offset = input_len.saturating_sub(position + 4);

        let mut remaining_mask = tag_mask;
        let mut attempts_left = search_max_attempts;
        while remaining_mask != 0 && attempts_left > 0 {
            let slot = remaining_mask.trailing_zeros() as usize;
            remaining_mask &= remaining_mask - 1;
            attempts_left -= 1;

            let index = row_base + slot;
            if !row_in_bounds && index >= positions_len {
                continue;
            }
            let candidate_position = if row_in_bounds {
                unsafe { *self.positions.get_unchecked(index) as usize }
            } else {
                self.positions[index] as usize
            };
            if candidate_position == u32::MAX as usize
                || candidate_position >= position
                || candidate_position < lowest_valid
            {
                continue;
            }

            let check_offset = best_length.saturating_sub(3);
            let plausible = if check_offset <= max_safe_check_offset {
                unsafe {
                    read_u32_le_unchecked(input_ptr, input_len, candidate_position + check_offset)
                        == read_u32_le_unchecked(input_ptr, input_len, position + check_offset)
                }
            } else {
                true
            };
            if !plausible {
                continue;
            }

            let length = unsafe {
                crate::simd::count_matching_bytes::count_matching_bytes_unchecked(
                    input_ptr.add(position),
                    input_ptr.add(candidate_position),
                    limit,
                )
            };
            if length > best_length {
                best_length = length;
                best_distance = position - candidate_position;
                if length >= limit {
                    break;
                }
            }
        }

        (best_length, best_distance)
    }

    fn repeat_candidate(
        &self,
        input: &[u8],
        position: usize,
        distance: usize,
        limit: usize,
    ) -> Option<usize> {
        if distance == 0 || distance > position {
            return None;
        }
        let source = position - distance;
        if !matches_four_bytes(input, position, source) {
            return None;
        }
        let length = matching_length(input, position, source, limit);
        if length >= MIN_MATCH {
            Some(length)
        } else {
            None
        }
    }
}

fn limit_at(input_length: usize, position: usize) -> usize {
    (input_length - position).min(MAX_MATCH_LENGTH as usize)
}

fn gain(multiplier: i64, length: usize, offset_value: u32, bias: i64) -> i64 {
    multiplier * length as i64 - highest_bit(offset_value) as i64 + bias
}

impl MatchFinder for RowHashFinder<'_> {
    fn reset(&mut self) {
        self.positions.fill(u32::MAX);
        self.heads.fill(0);
        self.tags.fill(0);
        self.base_position = 0;
        self.next_to_insert = 0;
        self.cached_position = NO_CACHED_POSITION;
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

        let block_length = input.len() - block_start;
        let min_match = self.parameters.min_match as usize;

        if input.len() > u32::MAX as usize || sequences.is_empty() || block_length < min_match {
            return (0, block_length);
        }
        if input.len() < HASH_READ_SIZE {
            return (0, block_length);
        }

        let scan_limit = input.len() - HASH_READ_SIZE;
        if block_start > scan_limit {
            return (0, block_length);
        }

        let depth: u8 = match self.search_method {
            SearchMethod::Greedy => 0,
            SearchMethod::Lazy => 1,
            SearchMethod::Lazy2 => 2,
        };

        let mut position = block_start;
        let mut literal_start = block_start;
        let mut sequence_count = 0usize;

        while position <= scan_limit && sequence_count < sequences.len() {
            let repeat_distance = repeat_offsets.first as usize;

            let mut match_length = 0usize;
            let mut match_distance = 0usize;
            let mut is_repeat = false;
            let mut start = position + 1;
            let mut gain_offset = 1u32;
            let mut skip_search_at_position = false;

            if let Some(length) = self.repeat_candidate(
                input,
                position,
                repeat_distance,
                limit_at(input.len(), position),
            ) {
                match_length = length;
                match_distance = repeat_distance;
                is_repeat = true;
                start = position;
                gain_offset = 1;
                if depth == 0 {
                    skip_search_at_position = true;
                }
            } else if position < scan_limit
                && let Some(length) = self.repeat_candidate(
                    input,
                    position + 1,
                    repeat_distance,
                    limit_at(input.len(), position + 1),
                )
            {
                match_length = length;
                match_distance = repeat_distance;
                is_repeat = true;
                start = position + 1;
                gain_offset = 1;
                if depth == 0 {
                    skip_search_at_position = true;
                }
            }

            if !skip_search_at_position
                && let Some(candidate) =
                    self.search_max(input, position, limit_at(input.len(), position))
                && candidate.length > match_length
            {
                match_length = candidate.length;
                match_distance = candidate.distance;
                is_repeat = false;
                start = position;
                gain_offset = candidate.offset_value;
            }

            if match_length < MIN_MATCH {
                let step = ((position - literal_start) >> SEARCH_STRENGTH) + 1;
                position += step;
                continue;
            }

            if depth >= 1 {
                let mut probe = position;

                'lazy: loop {
                    if probe >= scan_limit {
                        break;
                    }
                    probe += 1;

                    let best_step = self.evaluate_lazy_step(
                        input,
                        probe,
                        repeat_distance,
                        match_length,
                        gain_offset,
                        1,
                    );
                    if let Some(step_best) = best_step {
                        match_length = step_best.length;
                        match_distance = step_best.distance;
                        is_repeat = step_best.is_repeat;
                        start = step_best.start;
                        gain_offset = step_best.gain_offset;
                        if !step_best.is_repeat {
                            continue 'lazy;
                        }
                    }

                    if depth == 2 && probe < scan_limit {
                        probe += 1;

                        let best_step2 = self.evaluate_lazy_step(
                            input,
                            probe,
                            repeat_distance,
                            match_length,
                            gain_offset,
                            2,
                        );
                        if let Some(step_best) = best_step2 {
                            match_length = step_best.length;
                            match_distance = step_best.distance;
                            is_repeat = step_best.is_repeat;
                            start = step_best.start;
                            gain_offset = step_best.gain_offset;
                            if !step_best.is_repeat {
                                continue 'lazy;
                            }
                        }
                    }

                    break;
                }
            }

            let mut match_start = start;
            let mut source_start = start - match_distance;

            if !is_repeat {
                let lowest = self.base_position as usize;
                while match_start > literal_start
                    && source_start > lowest
                    && input[match_start - 1] == input[source_start - 1]
                {
                    match_start -= 1;
                    source_start -= 1;
                    match_length += 1;
                }
            }

            let literal_length = (match_start - literal_start) as u32;
            let offset_value =
                repeat_offsets.get_offset_value(match_distance as u32, literal_length);

            sequences[sequence_count] = SequenceRecord {
                literal_length,
                match_length: match_length as u32,
                offset_value,
            };
            sequence_count += 1;

            position = match_start + match_length;
            literal_start = position;

            while position <= scan_limit && sequence_count < sequences.len() {
                let repeat_distance = repeat_offsets.second as usize;
                if repeat_distance == 0 || repeat_distance > position {
                    break;
                }
                let source = position - repeat_distance;
                if !matches_four_bytes(input, position, source) {
                    break;
                }

                let repeat_limit = limit_at(input.len(), position);
                let repeat_length = matching_length(input, position, source, repeat_limit);
                if repeat_length < MIN_MATCH {
                    break;
                }

                let offset_value = repeat_offsets.get_offset_value(repeat_distance as u32, 0);
                sequences[sequence_count] = SequenceRecord {
                    literal_length: 0,
                    match_length: repeat_length as u32,
                    offset_value,
                };
                sequence_count += 1;

                position += repeat_length;
                literal_start = position;
            }
        }

        let tail_literal_count = input.len() - literal_start;
        (sequence_count, tail_literal_count)
    }
}

impl RowHashFinder<'_> {
    #[allow(clippy::too_many_arguments)]
    fn evaluate_lazy_step(
        &mut self,
        input: &[u8],
        probe: usize,
        repeat_distance: usize,
        current_length: usize,
        current_gain_offset: u32,
        step: u8,
    ) -> Option<Best> {
        let limit = limit_at(input.len(), probe);

        let mut length = current_length;
        let mut gain_offset = current_gain_offset;
        let mut distance = 0usize;
        let mut is_repeat = false;
        let mut start = probe;
        let mut updated = false;

        let repeat_multiplier = if step == 1 { 3 } else { 4 };
        if let Some(repeat_length) = self.repeat_candidate(input, probe, repeat_distance, limit) {
            let gain2 = repeat_multiplier * repeat_length as i64;
            let gain1 = gain(repeat_multiplier, length, gain_offset, 1);
            if gain2 > gain1 {
                length = repeat_length;
                gain_offset = 1;
                distance = repeat_distance;
                is_repeat = true;
                start = probe;
                updated = true;
            }
        }

        let search_bias = if step == 1 { 4 } else { 7 };
        if let Some(candidate) = self.search_max(input, probe, limit) {
            let gain2 = gain(4, candidate.length, candidate.offset_value, 0);
            let gain1 = gain(4, length, gain_offset, search_bias);
            if gain2 > gain1 {
                length = candidate.length;
                gain_offset = candidate.offset_value;
                distance = candidate.distance;
                is_repeat = false;
                start = probe;
                updated = true;
            }
        }

        if updated {
            Some(Best {
                length,
                distance,
                is_repeat,
                start,
                gain_offset,
            })
        } else {
            None
        }
    }
}

fn highest_bit(value: u32) -> u32 {
    if value == 0 {
        0
    } else {
        31 - value.leading_zeros()
    }
}

fn hash_bytes(value: u64, min_match: u8, hash_log: u8) -> usize {
    let hash_log = hash_log.clamp(1, 63);
    let (prime, used_bits) = match min_match {
        0..=4 => (HASH_PRIME_FOUR, 32u32),
        5 => (HASH_PRIME_FIVE, 40u32),
        6 => (HASH_PRIME_SIX, 48u32),
        _ => (HASH_PRIME_SEVEN, 56u32),
    };
    let shift = 64 - used_bits;
    let truncated = (value << shift) >> shift;
    let shifted = truncated << shift;
    (shifted.wrapping_mul(prime) >> (64 - hash_log as u32)) as usize
}

#[inline(always)]
unsafe fn read_u32_le_unchecked(input_ptr: *const u8, input_len: usize, position: usize) -> u32 {
    debug_assert!(position + 4 <= input_len);
    let ptr = unsafe { input_ptr.add(position) };
    u32::from_le_bytes(unsafe { core::ptr::read_unaligned(ptr as *const [u8; 4]) })
}

#[inline(always)]
unsafe fn read_u64_le_unchecked(input: &[u8], position: usize) -> u64 {
    debug_assert!(position + HASH_READ_SIZE <= input.len());
    let ptr = unsafe { input.as_ptr().add(position) };
    u64::from_le_bytes(unsafe { core::ptr::read_unaligned(ptr as *const [u8; 8]) })
}

fn read_u64_le(input: &[u8], position: usize) -> Option<u64> {
    let bytes = input.get(position..position + 8)?;
    let mut array = [0u8; 8];
    array.copy_from_slice(bytes);
    Some(u64::from_le_bytes(array))
}

fn matches_four_bytes(input: &[u8], first: usize, second: usize) -> bool {
    let input_len = input.len();
    if first + 4 > input_len || second + 4 > input_len {
        return false;
    }
    let input_ptr = input.as_ptr();
    unsafe {
        read_u32_le_unchecked(input_ptr, input_len, first)
            == read_u32_le_unchecked(input_ptr, input_len, second)
    }
}

#[inline(always)]
unsafe fn matching_length_unchecked(
    input_ptr: *const u8,
    input_len: usize,
    first: usize,
    second: usize,
    limit: usize,
) -> usize {
    debug_assert!(first + limit <= input_len);
    debug_assert!(second + limit <= input_len);
    unsafe {
        crate::simd::count_matching_bytes::count_matching_bytes_unchecked(
            input_ptr.add(first),
            input_ptr.add(second),
            limit,
        )
    }
}

fn matching_length(input: &[u8], first: usize, second: usize, limit: usize) -> usize {
    let input_len = input.len();
    let first_end = first.saturating_add(limit).min(input_len);
    let second_end = second.saturating_add(limit).min(input_len);
    if first_end <= first || second_end <= second {
        return 0;
    }
    let effective_limit = limit.min(first_end - first).min(second_end - second);
    unsafe { matching_length_unchecked(input.as_ptr(), input_len, first, second, effective_limit) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::match_finder::level_table::get_level_parameters;

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
    fn row_hash_finder_covers_the_whole_input_on_fuzz_shapes() {
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

            for (level, method) in [
                (5, SearchMethod::Greedy),
                (6, SearchMethod::Lazy),
                (9, SearchMethod::Lazy2),
                (12, SearchMethod::Lazy2),
            ] {
                let parameters = get_level_parameters(level).unwrap();
                let (mut hash_table, mut chain_table) = new_tables(parameters);
                let mut finder =
                    RowHashFinder::new(&mut hash_table, &mut chain_table, parameters, method);

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
