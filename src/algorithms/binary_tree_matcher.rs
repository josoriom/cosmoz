use crate::levels::level_table::LevelParameters;

pub(crate) const MAX_HASH3_LOG: u8 = 17;
pub(crate) const LONGEST_OPTIMAL_MATCH: u32 = 1 << 12;
pub(crate) const INPUT_TAIL_RESERVE: usize = 8;
pub(crate) const WINDOW_START_INDEX: u32 = 2;
const PRIME_THREE_BYTES: u32 = 506_832_829;
const PRIME_FOUR_BYTES: u32 = 2_654_435_761;
const MAX_HASH3_DISTANCE: u32 = 1 << 18;
const SHORTEST_MATCH: u32 = 3;
const REPEAT_OFFSET_COUNT: u32 = 3;
const LONG_MATCH_SKIP_THRESHOLD: u32 = 384;
const LONG_MATCH_SKIP_LIMIT: u32 = 192;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct MatchCandidate {
    pub offset_base: u32,
    pub length: u32,
}

pub(crate) struct BinaryTreeMatcher<'tables> {
    hash_table: &'tables mut [u32],
    tree_table: &'tables mut [u32],
    hash3_table: &'tables mut [u32],
    hash_log: u32,
    hash3_log: u32,
    tree_mask: u32,
    window_log: u32,
    search_count: u32,
    sufficient_length: u32,
    index_offset: u32,
    lowest_index: u32,
    next_to_update: u32,
    tables_are_zero: bool,
}

#[inline(always)]
unsafe fn read_u32_unchecked(input: &[u8], position: usize) -> u32 {
    debug_assert!(position + 4 <= input.len());
    unsafe { core::ptr::read_unaligned(input.as_ptr().add(position) as *const u32) }.to_le()
}

#[inline(always)]
unsafe fn read_three_bytes_unchecked(input: &[u8], position: usize) -> u32 {
    unsafe { read_u32_unchecked(input, position) << 8 }
}

#[inline(always)]
unsafe fn hash_three_bytes_unchecked(input: &[u8], position: usize, hash_log: u32) -> usize {
    let value = unsafe { read_u32_unchecked(input, position) };
    ((value << 8).wrapping_mul(PRIME_THREE_BYTES) >> (32 - hash_log)) as usize
}

#[inline(always)]
unsafe fn hash_four_bytes_unchecked(input: &[u8], position: usize, hash_log: u32) -> usize {
    let value = unsafe { read_u32_unchecked(input, position) };
    (value.wrapping_mul(PRIME_FOUR_BYTES) >> (32 - hash_log)) as usize
}

#[inline(always)]
unsafe fn count_unchecked(input: &[u8], first: usize, second: usize) -> usize {
    debug_assert!(second < first && first <= input.len());
    let end = input.len();
    let pointer = input.as_ptr();
    let mut matched = 0usize;
    while first + matched + 8 <= end {
        let difference = unsafe {
            core::ptr::read_unaligned(pointer.add(first + matched) as *const u64)
                ^ core::ptr::read_unaligned(pointer.add(second + matched) as *const u64)
        };
        if difference != 0 {
            return matched + (difference.to_le().trailing_zeros() / 8) as usize;
        }
        matched += 8;
    }
    while first + matched < end
        && unsafe { *pointer.add(first + matched) == *pointer.add(second + matched) }
    {
        matched += 1;
    }
    matched
}

impl<'tables> BinaryTreeMatcher<'tables> {
    pub(crate) fn new_over_zeroed_tables(
        hash_table: &'tables mut [u32],
        tree_table: &'tables mut [u32],
        hash3_table: &'tables mut [u32],
    ) -> Self {
        BinaryTreeMatcher {
            hash_table,
            tree_table,
            hash3_table,
            hash_log: 0,
            hash3_log: 0,
            tree_mask: 0,
            window_log: 0,
            search_count: 0,
            sufficient_length: 0,
            index_offset: WINDOW_START_INDEX,
            lowest_index: WINDOW_START_INDEX,
            next_to_update: WINDOW_START_INDEX,
            tables_are_zero: true,
        }
    }

    pub(crate) fn is_usable(&self) -> bool {
        self.hash_log > 0
    }

    pub(crate) fn reset(&mut self, parameters: LevelParameters) {
        let hash_log = parameters.hash_log as u32;
        let tree_log = parameters.chain_log as u32;
        let hash3_log = parameters.window_log.min(MAX_HASH3_LOG) as u32;
        let fits = self.hash_table.len() >= 1 << hash_log
            && self.tree_table.len() >= 1 << tree_log
            && self.hash3_table.len() >= 1 << hash3_log
            && (2..=30).contains(&tree_log)
            && (6..=30).contains(&hash_log);
        if !fits {
            self.hash_log = 0;
            return;
        }
        self.hash_log = hash_log;
        self.hash3_log = hash3_log;
        self.tree_mask = (1 << (tree_log - 1)) - 1;
        self.window_log = parameters.window_log as u32;
        self.search_count = 1 << parameters.search_log.min(30);
        self.sufficient_length = parameters.target_length.min(LONGEST_OPTIMAL_MATCH - 1);
        if !self.tables_are_zero {
            self.hash_table[..1 << hash_log].fill(0);
            self.hash3_table[..1 << hash3_log].fill(0);
        }
        self.tables_are_zero = false;
        self.index_offset = WINDOW_START_INDEX;
        self.lowest_index = WINDOW_START_INDEX;
        self.next_to_update = WINDOW_START_INDEX;
    }

    pub(crate) fn sufficient_length(&self) -> u32 {
        self.sufficient_length
    }

    pub(crate) fn next_to_update(&self) -> u32 {
        self.next_to_update
    }

    #[inline(always)]
    fn index_of(&self, position: usize) -> u32 {
        position as u32 + self.index_offset
    }

    #[inline(always)]
    fn position_of(&self, index: u32) -> usize {
        (index - self.index_offset) as usize
    }

    pub(crate) fn is_history_start(&self, position: usize) -> bool {
        self.index_of(position) == self.lowest_index
    }

    pub(crate) fn prepare_for_block(&mut self, block_start: usize) {
        let max_distance = 1u32 << self.window_log;
        let current = self.index_of(block_start);
        if current > max_distance {
            self.lowest_index = self.lowest_index.max(current - max_distance);
        }
        self.next_to_update = self.next_to_update.max(self.lowest_index);
        if current > self.next_to_update + LONG_MATCH_SKIP_THRESHOLD {
            let skipped = current - self.next_to_update - LONG_MATCH_SKIP_THRESHOLD;
            self.next_to_update = current - skipped.min(LONG_MATCH_SKIP_LIMIT);
        }
    }

    pub(crate) fn forget_history_before(&mut self, block_length: usize) {
        self.index_offset += block_length as u32;
        self.lowest_index += block_length as u32;
        self.next_to_update = self.lowest_index;
    }

    #[inline(always)]
    fn find_lowest_match_index(&self, current: u32) -> u32 {
        let max_distance = 1u32 << self.window_log;
        if current - self.lowest_index > max_distance {
            current - max_distance
        } else {
            self.lowest_index
        }
    }

    unsafe fn insert_position_unchecked(
        &mut self,
        input: &[u8],
        position: usize,
        target: u32,
    ) -> u32 {
        debug_assert!(position + INPUT_TAIL_RESERVE <= input.len());
        let input_end = input.len();
        let hash = unsafe { hash_four_bytes_unchecked(input, position, self.hash_log) };
        let current = self.index_of(position);
        let tree = self.tree_table.as_mut_ptr();
        let mut match_index = unsafe { *self.hash_table.get_unchecked(hash) };
        let tree_low = current.saturating_sub(self.tree_mask);
        let mut dummy = 0u32;
        let mut smaller_slot: *mut u32 =
            unsafe { tree.add(2 * (current & self.tree_mask) as usize) };
        let mut larger_slot: *mut u32 = unsafe { smaller_slot.add(1) };
        let window_low = self.find_lowest_match_index(target);
        let mut match_end_index = current + 8 + 1;
        let mut best_length = 8usize;
        let mut compares_left = self.search_count;
        let mut smaller_common_length = 0usize;
        let mut larger_common_length = 0usize;
        let tree_mask = self.tree_mask;
        let index_offset = self.index_offset;
        unsafe { *self.hash_table.get_unchecked_mut(hash) = current };

        while compares_left > 0 && match_index >= window_low {
            let next_node = unsafe { tree.add(2 * (match_index & tree_mask) as usize) };
            let match_position = (match_index - index_offset) as usize;
            let mut match_length = smaller_common_length.min(larger_common_length);
            match_length += unsafe {
                count_unchecked(
                    input,
                    position + match_length,
                    match_position + match_length,
                )
            };

            if match_length > best_length {
                best_length = match_length;
                if match_length as u32 > match_end_index - match_index {
                    match_end_index = match_index + match_length as u32;
                }
            }

            if position + match_length == input_end {
                break;
            }

            let match_byte = unsafe { *input.get_unchecked(match_position + match_length) };
            let current_byte = unsafe { *input.get_unchecked(position + match_length) };
            if match_byte < current_byte {
                unsafe { *smaller_slot = match_index };
                smaller_common_length = match_length;
                if match_index <= tree_low {
                    smaller_slot = &mut dummy;
                    break;
                }
                smaller_slot = unsafe { next_node.add(1) };
                match_index = unsafe { *next_node.add(1) };
            } else {
                unsafe { *larger_slot = match_index };
                larger_common_length = match_length;
                if match_index <= tree_low {
                    larger_slot = &mut dummy;
                    break;
                }
                larger_slot = next_node;
                match_index = unsafe { *next_node };
            }
            compares_left -= 1;
        }

        unsafe {
            *smaller_slot = 0;
            *larger_slot = 0;
        }
        let skipped = if best_length > LONG_MATCH_SKIP_THRESHOLD as usize {
            (best_length as u32 - LONG_MATCH_SKIP_THRESHOLD).min(LONG_MATCH_SKIP_LIMIT)
        } else {
            0
        };
        skipped.max(match_end_index - (current + 8))
    }

    unsafe fn update_tree_unchecked(&mut self, input: &[u8], position: usize) {
        let target = self.index_of(position);
        let mut index = self.next_to_update;
        while index < target {
            let insert_position = self.position_of(index);
            index += unsafe { self.insert_position_unchecked(input, insert_position, target) };
        }
        self.next_to_update = target;
    }

    unsafe fn insert_hash3_and_find_first_unchecked(
        &mut self,
        input: &[u8],
        next_to_update3: &mut u32,
        position: usize,
    ) -> u32 {
        let target = self.index_of(position);
        let mut index = *next_to_update3;
        while index < target {
            let hash = unsafe {
                hash_three_bytes_unchecked(input, self.position_of(index), self.hash3_log)
            };
            unsafe { *self.hash3_table.get_unchecked_mut(hash) = index };
            index += 1;
        }
        *next_to_update3 = target;
        let hash = unsafe { hash_three_bytes_unchecked(input, position, self.hash3_log) };
        unsafe { *self.hash3_table.get_unchecked(hash) }
    }

    #[allow(clippy::missing_safety_doc)]
    pub unsafe fn find_all_matches_unchecked(
        &mut self,
        input: &[u8],
        next_to_update3: &mut u32,
        position: usize,
        repeats: &[u32; 3],
        literal_length_is_zero: bool,
        candidates: &mut [MatchCandidate],
    ) -> usize {
        debug_assert!(position + INPUT_TAIL_RESERVE <= input.len());
        if self.index_of(position) < self.next_to_update {
            return 0;
        }
        unsafe {
            self.update_tree_unchecked(input, position);
            self.insert_and_find_all_matches_unchecked(
                input,
                next_to_update3,
                position,
                repeats,
                literal_length_is_zero,
                candidates,
            )
        }
    }

    unsafe fn insert_and_find_all_matches_unchecked(
        &mut self,
        input: &[u8],
        next_to_update3: &mut u32,
        position: usize,
        repeats: &[u32; 3],
        literal_length_is_zero: bool,
        candidates: &mut [MatchCandidate],
    ) -> usize {
        let input_end = input.len();
        let current = self.index_of(position);
        let hash = unsafe { hash_four_bytes_unchecked(input, position, self.hash_log) };
        let mut match_index = unsafe { *self.hash_table.get_unchecked(hash) };
        let tree = self.tree_table.as_mut_ptr();
        let tree_low = current.saturating_sub(self.tree_mask);
        let window_low = self.find_lowest_match_index(current);
        let mut smaller_slot: *mut u32 =
            unsafe { tree.add(2 * (current & self.tree_mask) as usize) };
        let mut larger_slot: *mut u32 = unsafe { smaller_slot.add(1) };
        let mut dummy = 0u32;
        let mut match_end_index = current + 8 + 1;
        let mut candidate_count = 0usize;
        let mut compares_left = self.search_count;
        let mut smaller_common_length = 0usize;
        let mut larger_common_length = 0usize;
        let mut best_length = (SHORTEST_MATCH - 1) as usize;
        let sufficient_length = self.sufficient_length as usize;
        let history_length = current - self.lowest_index;

        let first_repeat_code = literal_length_is_zero as u32;
        for repeat_code in first_repeat_code..REPEAT_OFFSET_COUNT + first_repeat_code {
            let repeat_offset = if repeat_code == REPEAT_OFFSET_COUNT {
                repeats[0].wrapping_sub(1)
            } else {
                repeats[repeat_code as usize]
            };
            let mut repeat_length = 0usize;
            if repeat_offset.wrapping_sub(1) < history_length {
                let repeat_position = position - repeat_offset as usize;
                let repeat_index = current - repeat_offset;
                let same_start = unsafe {
                    read_three_bytes_unchecked(input, position)
                        == read_three_bytes_unchecked(input, repeat_position)
                };
                if repeat_index >= window_low && same_start {
                    repeat_length = unsafe {
                        count_unchecked(
                            input,
                            position + SHORTEST_MATCH as usize,
                            repeat_position + SHORTEST_MATCH as usize,
                        )
                    } + SHORTEST_MATCH as usize;
                }
            }
            if repeat_length > best_length {
                best_length = repeat_length;
                candidates[candidate_count] = MatchCandidate {
                    offset_base: repeat_code - first_repeat_code + 1,
                    length: repeat_length as u32,
                };
                candidate_count += 1;
                if repeat_length > sufficient_length || position + repeat_length == input_end {
                    return candidate_count;
                }
            }
        }

        if best_length < SHORTEST_MATCH as usize {
            let match_index3 = unsafe {
                self.insert_hash3_and_find_first_unchecked(input, next_to_update3, position)
            };
            if match_index3 >= window_low && current - match_index3 < MAX_HASH3_DISTANCE {
                let match_length =
                    unsafe { count_unchecked(input, position, self.position_of(match_index3)) };
                if match_length >= SHORTEST_MATCH as usize {
                    best_length = match_length;
                    candidates[0] = MatchCandidate {
                        offset_base: current - match_index3 + REPEAT_OFFSET_COUNT,
                        length: match_length as u32,
                    };
                    candidate_count = 1;
                    if match_length > sufficient_length || position + match_length == input_end {
                        self.next_to_update = current + 1;
                        return 1;
                    }
                }
            }
        }

        let tree_mask = self.tree_mask;
        let index_offset = self.index_offset;
        unsafe { *self.hash_table.get_unchecked_mut(hash) = current };

        while compares_left > 0 && match_index >= window_low {
            let next_node = unsafe { tree.add(2 * (match_index & tree_mask) as usize) };
            let match_position = (match_index - index_offset) as usize;
            let mut match_length = smaller_common_length.min(larger_common_length);
            match_length += unsafe {
                count_unchecked(
                    input,
                    position + match_length,
                    match_position + match_length,
                )
            };

            if match_length > best_length {
                if match_length as u32 > match_end_index - match_index {
                    match_end_index = match_index + match_length as u32;
                }
                best_length = match_length;
                debug_assert!(candidate_count < candidates.len());
                unsafe {
                    *candidates.get_unchecked_mut(candidate_count) = MatchCandidate {
                        offset_base: current - match_index + REPEAT_OFFSET_COUNT,
                        length: match_length as u32,
                    }
                };
                candidate_count += 1;
                if match_length > LONGEST_OPTIMAL_MATCH as usize
                    || position + match_length == input_end
                {
                    break;
                }
            }

            debug_assert!(position + match_length < input_end);
            let match_byte = unsafe { *input.get_unchecked(match_position + match_length) };
            let current_byte = unsafe { *input.get_unchecked(position + match_length) };
            if match_byte < current_byte {
                unsafe { *smaller_slot = match_index };
                smaller_common_length = match_length;
                if match_index <= tree_low {
                    smaller_slot = &mut dummy;
                    break;
                }
                smaller_slot = unsafe { next_node.add(1) };
                match_index = unsafe { *next_node.add(1) };
            } else {
                unsafe { *larger_slot = match_index };
                larger_common_length = match_length;
                if match_index <= tree_low {
                    larger_slot = &mut dummy;
                    break;
                }
                larger_slot = next_node;
                match_index = unsafe { *next_node };
            }
            compares_left -= 1;
        }

        unsafe {
            *smaller_slot = 0;
            *larger_slot = 0;
        }
        self.next_to_update = match_end_index - 8;
        candidate_count
    }
}
