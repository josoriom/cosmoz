use crate::algorithms::binary_tree_matcher::{
    BinaryTreeMatcher, INPUT_TAIL_RESERVE, LONGEST_OPTIMAL_MATCH, MAX_HASH3_LOG, MatchCandidate,
};
use crate::algorithms::symbol_prices::SymbolPrices;
use crate::block::{repeat_offsets::RepeatOffsets, sequence_record::SequenceRecord};
use crate::levels::{
    MatchFinder,
    level_table::{LevelParameters, get_level_parameters_for_input_length},
};

pub(crate) const OPTIMAL_TABLE_LENGTH: usize = LONGEST_OPTIMAL_MATCH as usize + 3;
const MAX_PRICE: i32 = 1 << 30;
const SHORTEST_MATCH: u32 = 3;
const MIN_COMPRESSIBLE_BLOCK_LENGTH: usize = 7;
const MIN_SEEDING_BLOCK_LENGTH: usize = 9;
const MAX_INPUT_LENGTH: usize = 3 << 30;
const NODE_WORDS: usize = 7;
const CANDIDATE_WORDS: usize = 2;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct OptimalNode {
    price: i32,
    offset_base: u32,
    match_length: u32,
    literal_length: u32,
    repeats: [u32; 3],
}

pub(crate) struct Ultra2Finder<'tables> {
    matcher: BinaryTreeMatcher<'tables>,
    prices: SymbolPrices,
    nodes: &'tables mut [OptimalNode],
    candidates: &'tables mut [MatchCandidate],
    level: u8,
    #[allow(dead_code)]
    window_log: u8,
}

pub(crate) fn get_optimal_table_length(parameters: LevelParameters) -> usize {
    (1usize << parameters.window_log.min(MAX_HASH3_LOG))
        + OPTIMAL_TABLE_LENGTH * (NODE_WORDS + CANDIDATE_WORDS)
}

fn get_updated_repeats(
    repeats: [u32; 3],
    offset_base: u32,
    literal_length_is_zero: bool,
) -> [u32; 3] {
    let mut updated = repeats;
    if offset_base > 3 {
        updated[2] = updated[1];
        updated[1] = updated[0];
        updated[0] = offset_base - 3;
        return updated;
    }
    let repeat_code = offset_base - 1 + literal_length_is_zero as u32;
    if repeat_code > 0 {
        let offset = if repeat_code == 3 {
            updated[0] - 1
        } else {
            updated[repeat_code as usize]
        };
        if repeat_code >= 2 {
            updated[2] = updated[1];
        }
        updated[1] = updated[0];
        updated[0] = offset;
    }
    updated
}

fn split_words<T>(
    words: &mut [u32],
    count: usize,
    words_per_item: usize,
) -> (&mut [T], &mut [u32]) {
    debug_assert_eq!(core::mem::size_of::<T>(), words_per_item * 4);
    debug_assert_eq!(core::mem::align_of::<T>(), 4);
    let (item_words, rest) = words.split_at_mut(count * words_per_item);
    let items =
        unsafe { core::slice::from_raw_parts_mut(item_words.as_mut_ptr() as *mut T, count) };
    (items, rest)
}

impl<'tables> Ultra2Finder<'tables> {
    pub(crate) fn new_over_zeroed_tables(
        hash_table: &'tables mut [u32],
        tree_table: &'tables mut [u32],
        optimal_table: &'tables mut [u32],
        level: u8,
        parameters: LevelParameters,
    ) -> Self {
        let buffer_words = OPTIMAL_TABLE_LENGTH * (NODE_WORDS + CANDIDATE_WORDS);
        let usable = optimal_table.len() >= buffer_words;
        let item_count = if usable { OPTIMAL_TABLE_LENGTH } else { 0 };
        let (nodes, rest) = split_words::<OptimalNode>(optimal_table, item_count, NODE_WORDS);
        let (candidates, hash3_table) =
            split_words::<MatchCandidate>(rest, item_count, CANDIDATE_WORDS);
        Ultra2Finder {
            matcher: BinaryTreeMatcher::new_over_zeroed_tables(hash_table, tree_table, hash3_table),
            prices: SymbolPrices::new(),
            nodes,
            candidates,
            level,
            window_log: parameters.window_log,
        }
    }

    unsafe fn compress_block_unchecked(
        &mut self,
        input: &[u8],
        block_start: usize,
        repeats: &mut [u32; 3],
        sequences: &mut [SequenceRecord],
    ) -> (usize, usize) {
        let Ultra2Finder {
            matcher,
            prices,
            nodes,
            candidates,
            ..
        } = self;
        let input_end = input.len();
        let input_limit = input_end.saturating_sub(INPUT_TAIL_RESERVE);
        let sufficient_length = matcher.sufficient_length();
        let mut next_to_update3 = matcher.next_to_update();
        let mut position = block_start;
        let mut anchor = block_start;
        let mut sequence_count = 0usize;
        let mut last_stretch = OptimalNode::default();

        prices.prepare_for_block(&input[block_start..]);
        if matcher.is_history_start(position) {
            position += 1;
        }

        'parse: while position < input_limit {
            let (mut current, last_position) = 'forward: {
                let literal_length = (position - anchor) as u32;
                let match_count = unsafe {
                    matcher.find_all_matches_unchecked(
                        input,
                        &mut next_to_update3,
                        position,
                        repeats,
                        literal_length == 0,
                        candidates,
                    )
                };
                if match_count == 0 {
                    position += 1;
                    continue 'parse;
                }

                nodes[0].match_length = 0;
                nodes[0].literal_length = literal_length;
                nodes[0].price = prices.get_literal_length_price(literal_length);
                nodes[0].repeats = *repeats;

                let longest = candidates[match_count - 1];
                if longest.length > sufficient_length {
                    last_stretch.literal_length = 0;
                    last_stretch.match_length = longest.length;
                    last_stretch.offset_base = longest.offset_base;
                    break 'forward (0, longest.length as usize);
                }

                let zero_literal_length_price = prices.get_literal_length_price(0);
                let first_price = nodes[0].price;
                let mut node_position = 1usize;
                while node_position < SHORTEST_MATCH as usize {
                    nodes[node_position].price = MAX_PRICE;
                    nodes[node_position].match_length = 0;
                    nodes[node_position].literal_length = literal_length + node_position as u32;
                    node_position += 1;
                }
                for candidate in &candidates[..match_count] {
                    while node_position <= candidate.length as usize {
                        let match_price =
                            prices.get_match_price(candidate.offset_base, node_position as u32);
                        let node = &mut nodes[node_position];
                        node.match_length = node_position as u32;
                        node.offset_base = candidate.offset_base;
                        node.literal_length = 0;
                        node.price = first_price + match_price + zero_literal_length_price;
                        node_position += 1;
                    }
                }
                let mut last_position = node_position - 1;
                nodes[node_position].price = MAX_PRICE;

                let mut current = 1usize;
                while current <= last_position {
                    let current_position = position + current;

                    let literal_length = nodes[current - 1].literal_length + 1;
                    let price = nodes[current - 1].price
                        + prices.get_literal_price(input[current_position - 1])
                        + prices.get_literal_length_increase_price(literal_length);
                    if price <= nodes[current].price {
                        let previous_match = nodes[current];
                        nodes[current] = nodes[current - 1];
                        nodes[current].literal_length = literal_length;
                        nodes[current].price = price;
                        if previous_match.literal_length == 0
                            && prices.get_literal_length_increase_price(1) < 0
                            && current_position < input_end
                        {
                            let next_literal_price =
                                prices.get_literal_price(input[current_position]);
                            let with_one_literal = previous_match.price
                                + next_literal_price
                                + prices.get_literal_length_increase_price(1);
                            let with_more_literals = price
                                + next_literal_price
                                + prices.get_literal_length_increase_price(literal_length + 1);
                            if with_one_literal < with_more_literals
                                && with_one_literal < nodes[current + 1].price
                            {
                                let previous = current - previous_match.match_length as usize;
                                let updated_repeats = get_updated_repeats(
                                    nodes[previous].repeats,
                                    previous_match.offset_base,
                                    nodes[previous].literal_length == 0,
                                );
                                nodes[current + 1] = previous_match;
                                nodes[current + 1].repeats = updated_repeats;
                                nodes[current + 1].literal_length = 1;
                                nodes[current + 1].price = with_one_literal;
                                last_position = last_position.max(current + 1);
                            }
                        }
                    }

                    if nodes[current].literal_length == 0 {
                        let previous = current - nodes[current].match_length as usize;
                        nodes[current].repeats = get_updated_repeats(
                            nodes[previous].repeats,
                            nodes[current].offset_base,
                            nodes[previous].literal_length == 0,
                        );
                    }

                    if current_position > input_limit {
                        current += 1;
                        continue;
                    }
                    if current == last_position {
                        break;
                    }

                    let base_price = nodes[current].price + zero_literal_length_price;
                    let current_repeats = nodes[current].repeats;
                    let match_count = unsafe {
                        matcher.find_all_matches_unchecked(
                            input,
                            &mut next_to_update3,
                            current_position,
                            &current_repeats,
                            nodes[current].literal_length == 0,
                            candidates,
                        )
                    };
                    if match_count == 0 {
                        current += 1;
                        continue;
                    }

                    let longest = candidates[match_count - 1];
                    if longest.length > sufficient_length
                        || current + longest.length as usize >= LONGEST_OPTIMAL_MATCH as usize
                        || current_position + longest.length as usize >= input_end
                    {
                        last_stretch.match_length = longest.length;
                        last_stretch.offset_base = longest.offset_base;
                        last_stretch.literal_length = 0;
                        break 'forward (current, current + longest.length as usize);
                    }

                    for candidate_number in 0..match_count {
                        let candidate = candidates[candidate_number];
                        let start_length = if candidate_number > 0 {
                            candidates[candidate_number - 1].length + 1
                        } else {
                            SHORTEST_MATCH
                        };
                        let mut length = candidate.length;
                        while length >= start_length {
                            let node_position = current + length as usize;
                            let price =
                                base_price + prices.get_match_price(candidate.offset_base, length);
                            if node_position > last_position || price < nodes[node_position].price {
                                while last_position < node_position {
                                    last_position += 1;
                                    nodes[last_position].price = MAX_PRICE;
                                    nodes[last_position].literal_length = 1;
                                }
                                let node = &mut nodes[node_position];
                                node.match_length = length;
                                node.offset_base = candidate.offset_base;
                                node.literal_length = 0;
                                node.price = price;
                            }
                            length -= 1;
                        }
                    }
                    nodes[last_position + 1].price = MAX_PRICE;
                    current += 1;
                }

                last_stretch = nodes[last_position];
                (
                    last_position - last_stretch.match_length as usize,
                    last_position,
                )
            };

            if last_stretch.match_length == 0 {
                position += last_position;
                continue;
            }

            if last_stretch.literal_length == 0 {
                *repeats = get_updated_repeats(
                    nodes[current].repeats,
                    last_stretch.offset_base,
                    nodes[current].literal_length == 0,
                );
            } else {
                *repeats = last_stretch.repeats;
                current -= last_stretch.literal_length as usize;
            }

            let store_end = current + 2;
            let mut store_start = store_end;
            let mut stretch_position = current;
            nodes[store_end] = last_stretch;
            loop {
                let next_stretch = nodes[stretch_position];
                nodes[store_start].literal_length = next_stretch.literal_length;
                if next_stretch.match_length == 0 {
                    break;
                }
                store_start -= 1;
                nodes[store_start] = next_stretch;
                stretch_position -=
                    (next_stretch.literal_length + next_stretch.match_length) as usize;
            }

            for store_position in store_start..=store_end {
                let node = nodes[store_position];
                if node.match_length == 0 {
                    position = anchor + node.literal_length as usize;
                    continue;
                }
                let literal_end = anchor + node.literal_length as usize;
                prices.record_sequence(
                    &input[anchor..literal_end],
                    node.offset_base,
                    node.match_length,
                );
                sequences[sequence_count] = SequenceRecord {
                    literal_length: node.literal_length,
                    match_length: node.match_length,
                    offset_value: node.offset_base,
                };
                sequence_count += 1;
                anchor = literal_end + node.match_length as usize;
                position = anchor;
            }

            prices.update_sum_prices();
        }

        (sequence_count, input_end - anchor)
    }
}

impl MatchFinder for Ultra2Finder<'_> {
    fn reset(&mut self, input_length: usize) {
        self.prices = SymbolPrices::new();
        if let Some(parameters) = get_level_parameters_for_input_length(self.level, input_length) {
            self.matcher.reset(parameters);
        }
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
        if input.len() > MAX_INPUT_LENGTH
            || block_length < MIN_COMPRESSIBLE_BLOCK_LENGTH
            || sequences.len() < block_length / SHORTEST_MATCH as usize + 1
            || self.nodes.is_empty()
            || !self.matcher.is_usable()
        {
            return (0, block_length);
        }

        self.matcher.prepare_for_block(block_start);
        let mut repeats = [
            repeat_offsets.first,
            repeat_offsets.second,
            repeat_offsets.third,
        ];

        if !self.prices.has_statistics()
            && self.matcher.is_history_start(block_start)
            && block_length >= MIN_SEEDING_BLOCK_LENGTH
        {
            let mut seeding_repeats = repeats;
            unsafe {
                self.compress_block_unchecked(input, block_start, &mut seeding_repeats, sequences)
            };
            self.matcher.forget_history_before(block_length);
        }

        let (sequence_count, tail_literal_count) =
            unsafe { self.compress_block_unchecked(input, block_start, &mut repeats, sequences) };
        repeat_offsets.first = repeats[0];
        repeat_offsets.second = repeats[1];
        repeat_offsets.third = repeats[2];
        (sequence_count, tail_literal_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::levels::level_table::get_level_parameters;

    fn build_finder_memory(parameters: LevelParameters) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
        (
            vec![0u32; 1 << parameters.hash_log],
            vec![0u32; 1 << parameters.chain_log],
            vec![0u32; get_optimal_table_length(parameters)],
        )
    }

    fn build_mixed_input(length: usize, seed: u32) -> Vec<u8> {
        let mut state = seed;
        let mut bytes = Vec::with_capacity(length);
        let words: [&[u8]; 4] = [
            b"<spectrum index=\"",
            b"\" mz=\"",
            b"intensity",
            b"</binary>\n",
        ];
        while bytes.len() < length {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            if state.is_multiple_of(3) {
                bytes.extend_from_slice(words[(state >> 8) as usize % words.len()]);
            } else {
                bytes.extend_from_slice(&state.to_le_bytes()[..1 + (state as usize >> 20) % 4]);
            }
        }
        bytes.truncate(length);
        bytes
    }

    #[test]
    fn sequences_rebuild_every_block_of_the_input() {
        let parameters = get_level_parameters(22).unwrap();
        for (length, seed) in [(9usize, 3u32), (4000, 7), (300_000, 11), (70_000, 13)] {
            let input = build_mixed_input(length, seed);
            let small = get_level_parameters_for_input_length(22, input.len()).unwrap();
            let (mut hash_table, mut tree_table, mut optimal_table) = build_finder_memory(small);
            let mut finder = Ultra2Finder::new_over_zeroed_tables(
                &mut hash_table,
                &mut tree_table,
                &mut optimal_table,
                22,
                parameters,
            );
            finder.reset(input.len());
            let mut encoder_repeats = RepeatOffsets::new();
            let mut decoder_repeats = RepeatOffsets::new();
            let mut sequences = vec![SequenceRecord::default(); 128 * 1024 / 3 + 1];
            let mut rebuilt: Vec<u8> = Vec::with_capacity(input.len());
            let mut block_start = 0usize;
            while block_start < input.len() {
                let block_end = (block_start + 128 * 1024).min(input.len());
                let (sequence_count, tail_literal_count) = finder.find_sequences(
                    &input[..block_end],
                    block_start,
                    &mut sequences,
                    &mut encoder_repeats,
                );
                let mut cursor = block_start;
                for sequence in &sequences[..sequence_count] {
                    let literal_end = cursor + sequence.literal_length as usize;
                    rebuilt.extend_from_slice(&input[cursor..literal_end]);
                    let offset = decoder_repeats
                        .get_offset(sequence.offset_value, sequence.literal_length)
                        as usize;
                    assert!(offset >= 1 && offset <= rebuilt.len(), "length {length}");
                    for _ in 0..sequence.match_length {
                        rebuilt.push(rebuilt[rebuilt.len() - offset]);
                    }
                    cursor = literal_end + sequence.match_length as usize;
                }
                assert_eq!(cursor + tail_literal_count, block_end, "length {length}");
                rebuilt.extend_from_slice(&input[cursor..block_end]);
                block_start = block_end;
            }
            assert_eq!(rebuilt, input, "length {length}");
        }
    }
}
