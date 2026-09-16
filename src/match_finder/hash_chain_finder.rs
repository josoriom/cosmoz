use super::{MIN_MATCH, MatchFinder, level_table::LevelParameters};
use crate::{
    block::{
        repeat_offsets::RepeatOffsets, sequence_codes::MAX_MATCH_LENGTH,
        sequence_record::SequenceRecord,
    },
    simd::count_matching_bytes::{count_matching_bytes, count_matching_bytes_unchecked},
};

pub const HASH_READ_SIZE: usize = 8;
const SEARCH_STRENGTH: usize = 8;

const HASH_PRIME_FOUR: u64 = 0x9E37_79B1_85EB_CA87;
const HASH_PRIME_FIVE: u64 = 0x00CF_1BBC_DCBB;
const HASH_PRIME_SIX: u64 = 0xCF1B_BCDC_BF9B;
const HASH_PRIME_SEVEN: u64 = 0x00CF_1BBC_DCBF_A563;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SearchMethod {
    Greedy,
    Lazy,
    Lazy2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub length: usize,
    pub distance: usize,
    pub offset_value: u32,
}

pub struct HashChainFinder<'tables> {
    pub hash_table: &'tables mut [u32],
    pub chain_table: &'tables mut [u32],
    pub parameters: LevelParameters,
    pub search_method: SearchMethod,
    pub base_position: u32,
    pub next_to_insert: u32,
}

struct Best {
    length: usize,
    distance: usize,
    is_repeat: bool,
    start: usize,
    gain_offset: u32,
}

impl<'tables> HashChainFinder<'tables> {
    pub fn new(
        hash_table: &'tables mut [u32],
        chain_table: &'tables mut [u32],
        parameters: LevelParameters,
        search_method: SearchMethod,
    ) -> Self {
        HashChainFinder {
            hash_table,
            chain_table,
            parameters,
            search_method,
            base_position: 0,
            next_to_insert: 0,
        }
    }

    fn hash_index(&self, input: &[u8], position: usize) -> Option<usize> {
        let value = read_u64_le(input, position)?;
        let hash_table_length = self.hash_table.len();
        if hash_table_length == 0 {
            return None;
        }
        let hash = hash_bytes(value, self.parameters.min_match, self.parameters.hash_log);
        Some(hash % hash_table_length)
    }

    fn insert_position(&mut self, input: &[u8], position: usize) {
        let Some(hash) = self.hash_index(input, position) else {
            return;
        };
        let chain_table_length = self.chain_table.len();
        if chain_table_length == 0 {
            return;
        }
        let chain_index = position % chain_table_length;
        let previous = self.hash_table[hash];
        self.chain_table[chain_index] = previous;
        self.hash_table[hash] = position as u32;
    }

    fn insert_position_proven(&mut self, input_ptr: *const u8, input_len: usize, position: usize) {
        debug_assert!(position + HASH_READ_SIZE <= input_len);
        let hash_table_length = self.hash_table.len();
        if hash_table_length == 0 {
            return;
        }
        debug_assert!(hash_table_length.is_power_of_two());
        let hash = unsafe {
            hash_index_unchecked(
                input_ptr,
                input_len,
                position,
                self.parameters.min_match,
                self.parameters.hash_log,
                hash_table_length,
            )
        };
        let chain_table_length = self.chain_table.len();
        if chain_table_length == 0 {
            return;
        }
        debug_assert!(chain_table_length.is_power_of_two());
        let chain_index = position & (chain_table_length - 1);
        unsafe {
            let previous = *self.hash_table.get_unchecked(hash);
            *self.chain_table.get_unchecked_mut(chain_index) = previous;
            *self.hash_table.get_unchecked_mut(hash) = position as u32;
        }
    }

    fn insert_positions_up_to(&mut self, input: &[u8], position: usize) {
        let mut insert_at = (self.next_to_insert as usize).max(self.base_position as usize);
        let proven = position + HASH_READ_SIZE <= input.len();
        let input_ptr = input.as_ptr();
        let input_len = input.len();
        while insert_at < position {
            if proven {
                self.insert_position_proven(input_ptr, input_len, insert_at);
            } else {
                self.insert_position(input, insert_at);
            }
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
        let hash_table_length = self.hash_table.len();
        if hash_table_length == 0 {
            return None;
        }
        let chain_table_length = self.chain_table.len();

        let proven = hash_table_length.is_power_of_two()
            && (chain_table_length == 0 || chain_table_length.is_power_of_two())
            && position + HASH_READ_SIZE <= input.len()
            && position + limit <= input.len();

        let head;
        let hash;

        if proven {
            hash = unsafe {
                hash_index_unchecked(
                    input.as_ptr(),
                    input.len(),
                    position,
                    self.parameters.min_match,
                    self.parameters.hash_log,
                    hash_table_length,
                )
            };
            head = unsafe { *self.hash_table.get_unchecked(hash) };
            if chain_table_length != 0 {
                let chain_index = position & (chain_table_length - 1);
                unsafe {
                    *self.chain_table.get_unchecked_mut(chain_index) = head;
                }
            }
            unsafe {
                *self.hash_table.get_unchecked_mut(hash) = position as u32;
            }
        } else {
            hash = self.hash_index(input, position)?;
            head = self.hash_table[hash];
            if chain_table_length != 0 {
                let chain_index = position % chain_table_length;
                self.chain_table[chain_index] = head;
            }
            self.hash_table[hash] = position as u32;
        }

        self.next_to_insert = (position as u32).saturating_add(1);

        if chain_table_length == 0 {
            return None;
        }

        let search_limit = 1usize << self.parameters.search_log;

        let best = if proven {
            unsafe {
                search_chain_unchecked(
                    input.as_ptr(),
                    input.len(),
                    position,
                    limit,
                    lowest_valid,
                    head,
                    search_limit,
                    self.chain_table,
                    chain_table_length - 1,
                )
            }
        } else {
            search_chain_checked(
                input,
                position,
                limit,
                lowest_valid,
                head,
                search_limit,
                self.chain_table,
                chain_table_length,
            )
        };

        best.filter(|candidate| candidate.length >= MIN_MATCH)
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

    pub fn find_best_match(
        &mut self,
        input: &[u8],
        position: usize,
        anchor: usize,
        repeat_offsets: &RepeatOffsets,
        limit: usize,
    ) -> Option<Candidate> {
        debug_assert!(anchor <= position);
        debug_assert!(position + limit <= input.len());

        let repeat_distance = repeat_offsets.first as usize;
        if let Some(length) = self.repeat_candidate(input, position, repeat_distance, limit) {
            self.insert_positions_up_to(input, position);
            self.insert_position(input, position);
            self.next_to_insert = (position as u32).saturating_add(1);
            if length >= self.parameters.min_match as usize {
                return Some(Candidate {
                    length,
                    distance: repeat_distance,
                    offset_value: 1,
                });
            }
        }

        let candidate = self.search_max(input, position, limit)?;
        if candidate.length >= self.parameters.min_match as usize {
            Some(candidate)
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

impl MatchFinder for HashChainFinder<'_> {
    fn reset(&mut self) {
        self.hash_table.fill(u32::MAX);
        self.chain_table.fill(u32::MAX);
        self.base_position = 0;
        self.next_to_insert = 0;
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
            let mut skip_chain_at_position = false;

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
                    skip_chain_at_position = true;
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
                    skip_chain_at_position = true;
                }
            }

            if !skip_chain_at_position
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

impl HashChainFinder<'_> {
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

        let chain_bias = if step == 1 { 4 } else { 7 };
        if let Some(candidate) = self.search_max(input, probe, limit) {
            let gain2 = gain(4, candidate.length, candidate.offset_value, 0);
            let gain1 = gain(4, length, gain_offset, chain_bias);
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

fn read_u64_le(input: &[u8], position: usize) -> Option<u64> {
    let bytes = input.get(position..position + 8)?;
    let mut array = [0u8; 8];
    array.copy_from_slice(bytes);
    Some(u64::from_le_bytes(array))
}

fn read_u32_le(input: &[u8], position: usize) -> Option<u32> {
    let bytes = input.get(position..position + 4)?;
    let mut array = [0u8; 4];
    array.copy_from_slice(bytes);
    Some(u32::from_le_bytes(array))
}

fn matches_four_bytes(input: &[u8], first: usize, second: usize) -> bool {
    match (read_u32_le(input, first), read_u32_le(input, second)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn hash_index_unchecked(
    input_ptr: *const u8,
    input_len: usize,
    position: usize,
    min_match: u8,
    hash_log: u8,
    hash_table_length: usize,
) -> usize {
    debug_assert!(!input_ptr.is_null());
    debug_assert!(position + HASH_READ_SIZE <= input_len);
    debug_assert!(hash_table_length.is_power_of_two());

    let raw = unsafe { core::ptr::read_unaligned(input_ptr.add(position) as *const u64) };
    let value = u64::from_le(raw);
    let hash = hash_bytes(value, min_match, hash_log);
    hash & (hash_table_length - 1)
}

#[allow(clippy::too_many_arguments)]
fn search_chain_checked(
    input: &[u8],
    position: usize,
    limit: usize,
    lowest_valid: usize,
    head: u32,
    search_limit: usize,
    chain_table: &mut [u32],
    chain_table_length: usize,
) -> Option<Candidate> {
    let mut steps = 0usize;
    let mut candidate = head;
    let mut best_length = 0usize;
    let mut best: Option<Candidate> = None;

    while steps < search_limit
        && candidate != u32::MAX
        && (candidate as usize) < position
        && (candidate as usize) >= lowest_valid
    {
        let candidate_position = candidate as usize;
        let check_offset = best_length.saturating_sub(3);

        let plausible = match (
            read_u32_le(input, candidate_position + check_offset),
            read_u32_le(input, position + check_offset),
        ) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        };

        if plausible {
            let length = matching_length(input, position, candidate_position, limit);
            if length > best_length {
                best_length = length;
                let distance = position - candidate_position;
                best = Some(Candidate {
                    length,
                    distance,
                    offset_value: (distance as u32).saturating_add(3),
                });
                if length >= limit {
                    break;
                }
            }
        }

        let next_candidate = chain_table[candidate_position % chain_table_length];
        if next_candidate >= candidate {
            break;
        }
        candidate = next_candidate;
        steps += 1;
    }

    best
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

#[allow(clippy::too_many_arguments)]
unsafe fn search_chain_unchecked(
    input_ptr: *const u8,
    input_len: usize,
    position: usize,
    limit: usize,
    lowest_valid: usize,
    head: u32,
    search_limit: usize,
    chain_table: &[u32],
    chain_mask: usize,
) -> Option<Candidate> {
    debug_assert!(!input_ptr.is_null());
    debug_assert!(chain_table.len() == chain_mask + 1);
    debug_assert!(chain_table.len().is_power_of_two());
    debug_assert!(position + limit <= input_len);

    let mut steps = 0usize;
    let mut candidate = head;
    let mut best_length = 0usize;
    let mut best: Option<Candidate> = None;

    while steps < search_limit
        && candidate != u32::MAX
        && (candidate as usize) < position
        && (candidate as usize) >= lowest_valid
    {
        let candidate_position = candidate as usize;
        let chain_index = candidate_position & chain_mask;
        let next_candidate = unsafe { *chain_table.get_unchecked(chain_index) };

        if next_candidate < candidate {
            let next_position = next_candidate as usize;
            let next_chain_index = next_position & chain_mask;
            unsafe {
                prefetch_read(chain_table.as_ptr().add(next_chain_index) as *const u8);
                prefetch_read(input_ptr.add(next_position));
            }
        }

        debug_assert!(best_length < limit);
        debug_assert!(position + best_length < input_len);
        debug_assert!(candidate_position + best_length < input_len);
        let plausible = unsafe {
            *input_ptr.add(candidate_position + best_length)
                == *input_ptr.add(position + best_length)
        };

        if plausible {
            let length = unsafe {
                count_matching_bytes_unchecked(
                    input_ptr.add(candidate_position),
                    input_ptr.add(position),
                    limit,
                )
            };
            if length > best_length {
                best_length = length;
                let distance = position - candidate_position;
                best = Some(Candidate {
                    length,
                    distance,
                    offset_value: (distance as u32).saturating_add(3),
                });
                if length >= limit {
                    break;
                }
            }
        }

        if next_candidate >= candidate {
            break;
        }
        candidate = next_candidate;
        steps += 1;
    }

    best
}

fn matching_length(input: &[u8], first: usize, second: usize, limit: usize) -> usize {
    let first_end = first.saturating_add(limit).min(input.len());
    let second_end = second.saturating_add(limit).min(input.len());
    if first_end <= first || second_end <= second {
        return 0;
    }
    let first_slice = &input[first..first_end];
    let second_slice = &input[second..second_end];
    count_matching_bytes(first_slice, second_slice)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::match_finder::level_table::{Strategy, get_level_parameters};

    fn level_five_parameters() -> LevelParameters {
        get_level_parameters(5).unwrap()
    }

    fn new_tables(parameters: LevelParameters) -> (Vec<u32>, Vec<u32>) {
        let hash_length = 1usize << parameters.hash_log;
        let chain_length = 1usize << parameters.chain_log;
        (vec![u32::MAX; hash_length], vec![u32::MAX; chain_length])
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

    #[test]
    fn chain_walk_finds_the_longest_of_three_candidates() {
        let parameters = level_five_parameters();
        let (mut hash_table, mut chain_table) = new_tables(parameters);
        let mut finder = HashChainFinder::new(
            &mut hash_table,
            &mut chain_table,
            parameters,
            SearchMethod::Greedy,
        );

        let needle = [7u8, 1, 9, 4, 2];
        let tail = generate_pseudo_random_bytes(40, 0xABCD_EF01);

        let mut input = Vec::new();
        let short_occurrence = input.len();
        input.extend_from_slice(&needle);
        input.extend_from_slice(&tail[..5]);
        input.extend_from_slice(b"AAAAAAAAAA");
        let long_occurrence = input.len();
        input.extend_from_slice(&needle);
        input.extend_from_slice(&tail[..35]);
        input.extend_from_slice(b"BBBB");
        let medium_occurrence = input.len();
        input.extend_from_slice(&needle);
        input.extend_from_slice(&tail[..20]);
        input.extend_from_slice(b"CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC");
        let search_occurrence = input.len();
        input.extend_from_slice(&needle);
        input.extend_from_slice(&tail);
        input.extend_from_slice(b"DDDDDDDDDDDDDDDDDD");

        let repeat_offsets = RepeatOffsets {
            first: 0,
            second: 0,
            third: 0,
        };

        let search_position = search_occurrence;
        let limit = input.len() - search_position;
        let best = finder
            .find_best_match(&input, search_position, 0, &repeat_offsets, limit)
            .expect("expected a match against an earlier occurrence");

        assert_eq!(best.distance, search_position - long_occurrence);
        assert!(best.length >= needle.len() + 35);
        assert_ne!(best.distance, search_position - short_occurrence);
        assert_ne!(best.distance, search_position - medium_occurrence);
    }

    #[test]
    fn search_depth_is_bounded() {
        let mut parameters = level_five_parameters();
        parameters.search_log = 0;
        let (mut hash_table, mut chain_table) = new_tables(parameters);
        let mut finder = HashChainFinder::new(
            &mut hash_table,
            &mut chain_table,
            parameters,
            SearchMethod::Greedy,
        );

        let needle = [3u8, 5, 8, 2, 1];
        let tail = generate_pseudo_random_bytes(40, 0x1357_2468);

        let mut input = Vec::new();
        let old_occurrence = input.len();
        input.extend_from_slice(&needle);
        input.extend_from_slice(&tail);

        let newest_occurrence = input.len();
        input.extend_from_slice(&needle);
        input.extend_from_slice(&tail[..3]);
        input.extend_from_slice(b"YY");

        let last_position = input.len();
        input.extend_from_slice(&needle);
        input.extend_from_slice(&tail);

        let repeat_offsets = RepeatOffsets {
            first: 0,
            second: 0,
            third: 0,
        };

        let limit = input.len() - last_position;

        let best = finder
            .find_best_match(&input, last_position, 0, &repeat_offsets, limit)
            .expect("expected the closest candidate to match");

        assert_eq!(best.distance, last_position - newest_occurrence);
        assert!(best.length < needle.len() + tail.len());

        let mut unbounded_parameters = level_five_parameters();
        unbounded_parameters.search_log = 8;
        let (mut unbounded_hash_table, mut unbounded_chain_table) =
            new_tables(unbounded_parameters);
        let mut unbounded_finder = HashChainFinder::new(
            &mut unbounded_hash_table,
            &mut unbounded_chain_table,
            unbounded_parameters,
            SearchMethod::Greedy,
        );
        let unbounded_best = unbounded_finder
            .find_best_match(&input, last_position, 0, &repeat_offsets, limit)
            .expect("expected a match with a deeper search");
        assert_eq!(unbounded_best.distance, last_position - old_occurrence);
        assert_eq!(unbounded_best.length, needle.len() + tail.len());
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
            assert!(offset as usize <= cursor - block_start || cursor >= offset as usize);
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
    fn unchecked_finder_matches_real_bytes_on_fuzz_shapes() {
        let mut random_state = 0x2468_ACE1u32;

        for case in 0..300u32 {
            let shape = case % 4;
            let length = 200 + (case as usize % 4000);
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

            for search_method in [
                SearchMethod::Greedy,
                SearchMethod::Lazy,
                SearchMethod::Lazy2,
            ] {
                let parameters = level_five_parameters();
                let (mut hash_table, mut chain_table) = new_tables(parameters);
                let mut finder = HashChainFinder::new(
                    &mut hash_table,
                    &mut chain_table,
                    parameters,
                    search_method,
                );

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
                    "case {case} method {search_method:?}"
                );
            }
        }
    }

    #[test]
    fn lazy_prefers_longer_match_one_byte_later() {
        let mut parameters = level_five_parameters();
        parameters.search_log = 6;
        let (mut hash_table, mut chain_table) = new_tables(parameters);
        let mut finder = HashChainFinder::new(
            &mut hash_table,
            &mut chain_table,
            parameters,
            SearchMethod::Lazy,
        );

        let run_bytes: Vec<u8> = (0u8..21).collect();

        let mut input = Vec::new();
        input.extend(generate_pseudo_random_bytes(40, 0x1111_2222));

        let short_source = input.len();
        input.extend_from_slice(&run_bytes[0..6]);
        input.push(250);
        input.extend(generate_pseudo_random_bytes(40, 0x3333_4444));

        let long_source = input.len();
        input.push(251);
        input.extend_from_slice(&run_bytes[1..21]);
        input.push(252);
        input.extend(generate_pseudo_random_bytes(40, 0x5555_6666));

        let scan_position = input.len();
        input.extend_from_slice(&run_bytes);
        input.push(253);
        input.extend(generate_pseudo_random_bytes(16, 0x7777_8888));

        let _ = short_source;
        let _ = long_source;

        let mut repeat_offsets = RepeatOffsets::new();
        repeat_offsets.first = 0;
        let mut decoder_history = RepeatOffsets::new();
        decoder_history.first = 0;
        let mut sequences = vec![SequenceRecord::default(); 16];

        let (sequence_count, tail_literal_count) =
            finder.find_sequences(&input, 0, &mut sequences, &mut repeat_offsets);

        let covered = resolve_and_verify_sequences(
            &input,
            0,
            &sequences[..sequence_count],
            &mut decoder_history,
        );
        assert_eq!(covered + tail_literal_count, input.len());

        let matching_scan_position_sequence = sequences[..sequence_count].iter().find(|sequence| {
            sequence.match_length as usize >= 18 && sequence.match_length as usize <= 20
        });

        assert!(
            matching_scan_position_sequence.is_some(),
            "expected the lazy search to prefer the 20-byte match one byte later, sequences: {:?}",
            &sequences[..sequence_count]
        );

        let scan_position_covered_by_a_literal_then_long_match = {
            let mut cursor = 0usize;
            let mut found = false;
            for sequence in &sequences[..sequence_count] {
                let literal_end = cursor + sequence.literal_length as usize;
                if literal_end == scan_position + 1
                    && sequence.match_length as usize >= 18
                    && sequence.match_length as usize <= 20
                {
                    found = true;
                }
                cursor = literal_end + sequence.match_length as usize;
            }
            found
        };
        assert!(
            scan_position_covered_by_a_literal_then_long_match,
            "expected one extra literal at the scan position before the long match, sequences: {:?}",
            &sequences[..sequence_count]
        );
    }

    #[test]
    fn level_five_round_trips_through_our_decoder_and_the_cli() {
        use crate::{
            decoder::{DecodeWorkspace, decompress},
            encoder::{CompressOptions, EncodeWorkspace, compress, get_max_compressed_size},
            frame::frame_header::FrameFormat,
        };

        let paragraph = "the quick brown fox jumps over the lazy dog while a sphinx of black \
        quartz judges its vow and pack my box with five dozen liquor jugs before the vexingly \
        quick daft zebras jump away. ";
        let mut text = Vec::new();
        let mut counter: u64 = 0;
        while text.len() < 512 * 1024 {
            text.extend_from_slice(paragraph.as_bytes());
            text.extend_from_slice(counter.to_string().as_bytes());
            counter += 1;
        }

        let options = CompressOptions {
            format: FrameFormat::Zstd,
            with_checksum: true,
            chunk_size: 0,
            level: 5,
        };
        let mut workspace = EncodeWorkspace::new_boxed_for_level(5).unwrap();

        let level_parameters = get_level_parameters(5).unwrap();
        assert_eq!(level_parameters.strategy, Strategy::Greedy);

        let mut output = vec![0u8; get_max_compressed_size(text.len(), &options)];
        let written = compress(&text, &mut output, &options, &mut workspace).unwrap();
        output.truncate(written);

        let mut decode_workspace = DecodeWorkspace::new_boxed();
        let mut decoded = vec![0u8; text.len()];
        let decoded_length = decompress(&output, &mut decoded, &mut decode_workspace).unwrap();
        assert_eq!(decoded_length, text.len());
        assert_eq!(decoded, text);

        let zstd_available = std::process::Command::new("which")
            .arg("zstd")
            .output()
            .map(|value| value.status.success())
            .unwrap_or(false);

        if zstd_available {
            use std::io::Write;
            let mut child = std::process::Command::new("zstd")
                .arg("-d")
                .arg("-c")
                .arg("-q")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(&output).unwrap();
            let cli_output = child.wait_with_output().unwrap();
            assert!(cli_output.status.success());
            assert_eq!(cli_output.stdout, text);
        }
    }

    #[test]
    fn level_nine_output_is_pinned() {
        use crate::{
            encoder::{CompressOptions, EncodeWorkspace, compress, get_max_compressed_size},
            frame::frame_header::FrameFormat,
            hash::xxhash64,
        };

        const GOLDEN_LEVEL_NINE_HASH: u64 = 0x50e1_5d5e_38fb_ba34;
        const GOLDEN_LEVEL_NINE_LENGTH: usize = 763;

        let paragraph = "the quick brown fox jumps over the lazy dog while a sphinx of black \
        quartz judges its vow and pack my box with five dozen liquor jugs before the vexingly \
        quick daft zebras jump away. ";
        let mut text = Vec::new();
        let mut counter: u64 = 0;
        while text.len() < 64 * 1024 {
            text.extend_from_slice(paragraph.as_bytes());
            text.extend_from_slice(counter.to_string().as_bytes());
            counter += 1;
        }

        let options = CompressOptions {
            format: FrameFormat::Zstd,
            with_checksum: true,
            chunk_size: 0,
            level: 9,
        };
        let mut workspace = EncodeWorkspace::new_boxed_for_level(9).unwrap();
        let mut output = vec![0u8; get_max_compressed_size(text.len(), &options)];
        let written = compress(&text, &mut output, &options, &mut workspace).unwrap();
        output.truncate(written);

        assert_eq!(output.len(), GOLDEN_LEVEL_NINE_LENGTH);
        assert_eq!(xxhash64::hash_bytes(&output, 0), GOLDEN_LEVEL_NINE_HASH);
    }

    #[test]
    fn levels_six_to_twelve_round_trip() {
        use crate::{
            decoder::{DecodeWorkspace, decompress},
            encoder::{CompressOptions, EncodeWorkspace, compress, get_max_compressed_size},
            frame::frame_header::FrameFormat,
        };

        let paragraph = "the quick brown fox jumps over the lazy dog while a sphinx of black \
        quartz judges its vow and pack my box with five dozen liquor jugs before the vexingly \
        quick daft zebras jump away. ";
        let mut text = Vec::new();
        let mut counter: u64 = 0;
        while text.len() < 256 * 1024 {
            text.extend_from_slice(paragraph.as_bytes());
            text.extend_from_slice(counter.to_string().as_bytes());
            counter += 1;
        }

        for level in 6..=12u8 {
            let options = CompressOptions {
                format: FrameFormat::Zstd,
                with_checksum: true,
                chunk_size: 0,
                level,
            };
            let mut workspace = EncodeWorkspace::new_boxed_for_level(level).unwrap();
            let mut output = vec![0u8; get_max_compressed_size(text.len(), &options)];
            let written = compress(&text, &mut output, &options, &mut workspace).unwrap();
            output.truncate(written);

            let mut decode_workspace = DecodeWorkspace::new_boxed();
            let mut decoded = vec![0u8; text.len()];
            let decoded_length = decompress(&output, &mut decoded, &mut decode_workspace).unwrap();
            assert_eq!(decoded_length, text.len(), "level {level}");
            assert_eq!(decoded, text, "level {level}");
        }
    }
}
