use crate::{
    bits::backward_bit_writer::BackwardBitWriter,
    encode_error::EncodeError,
    entropy::{
        fse_encode_table::{
            FseEncodeState, FseEncodeTable, build_fse_encode_table, normalize_counts,
            pick_accuracy_log, write_fse_table_description,
        },
        histogram::{count_used_symbols, find_largest_symbol},
        huffman_decode_table::{MAX_HUFFMAN_BITS, MAX_WEIGHT_ACCURACY_LOG},
    },
};

const MAX_PACKAGE_MERGE_ITEMS: usize = 2 * 256 - 1;
const NO_LEAF: u16 = u16::MAX;
const MAX_DIRECT_WEIGHT_COUNT: usize = 128;
const MAX_WEIGHT_VALUE: usize = MAX_HUFFMAN_BITS;
const MIN_HUFFMAN_BITS: usize = 5;

#[derive(Clone, Copy, Default)]
pub(crate) struct HuffmanCode {
    pub code: u16,
    pub bit_count: u8,
}

pub(crate) struct HuffmanEncodeTable {
    pub codes: [HuffmanCode; 256],
    pub weights: [u8; 256],
    pub symbol_count: usize,
    pub max_bits: u8,
}

impl HuffmanEncodeTable {
    pub(crate) const fn new() -> Self {
        Self {
            codes: [HuffmanCode {
                code: 0,
                bit_count: 0,
            }; 256],
            weights: [0u8; 256],
            symbol_count: 0,
            max_bits: 0,
        }
    }
}

impl Default for HuffmanEncodeTable {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn get_cheap_depth(counts: &[u32; 256], total: usize) -> usize {
    if total <= 1 {
        return MAX_HUFFMAN_BITS;
    }
    let largest_symbol = find_largest_symbol(counts).max(1);
    let depth_needed_for_symbols = (highest_bit(total) + 1).min(highest_bit(largest_symbol) + 2);
    let depth_worth_paying_for = highest_bit(total - 1).saturating_sub(1);
    depth_worth_paying_for
        .min(MAX_HUFFMAN_BITS)
        .max(depth_needed_for_symbols)
        .clamp(MIN_HUFFMAN_BITS, MAX_HUFFMAN_BITS)
}

fn highest_bit(value: usize) -> usize {
    (usize::BITS - 1 - (value | 1).leading_zeros()) as usize
}

pub(crate) fn build_huffman_encode_table(
    counts: &[u32; 256],
    table: &mut HuffmanEncodeTable,
    depth_limit: usize,
) -> Result<(), EncodeError> {
    if count_used_symbols(counts) < 2 {
        return Err(EncodeError::TableNotUsable);
    }

    let mut lengths = [0u8; 256];
    let max_bits = build_code_lengths(counts, &mut lengths, depth_limit.min(MAX_HUFFMAN_BITS));

    table.symbol_count = find_largest_symbol(counts) + 1;
    table.max_bits = max_bits;
    for (weight, &length) in table.weights.iter_mut().zip(lengths.iter()) {
        *weight = if length == 0 {
            0
        } else {
            max_bits + 1 - length
        };
    }
    for code in table.codes.iter_mut() {
        *code = HuffmanCode::default();
    }

    assign_codes(table);
    Ok(())
}

const SORT_RANK_COUNT: usize = 65;

fn sort_used_symbols_by_count(used_symbols: &mut [(u64, u8); 256], used_symbol_count: usize) {
    let mut rank_of_symbol = [0u8; 256];
    let mut bucket_sizes = [0usize; SORT_RANK_COUNT];
    for index in 0..used_symbol_count {
        let count = used_symbols[index].0;
        let rank = (u64::BITS - count.leading_zeros()) as usize;
        rank_of_symbol[index] = rank as u8;
        bucket_sizes[rank] += 1;
    }

    let mut bucket_start = [0usize; SORT_RANK_COUNT + 1];
    for rank in 0..SORT_RANK_COUNT {
        bucket_start[rank + 1] = bucket_start[rank] + bucket_sizes[rank];
    }

    let mut cursor = bucket_start;
    let mut sorted: [(u64, u8); 256] = [(0, 0); 256];
    for index in 0..used_symbol_count {
        let rank = rank_of_symbol[index] as usize;
        sorted[cursor[rank]] = used_symbols[index];
        cursor[rank] += 1;
    }

    for rank in 0..SORT_RANK_COUNT {
        let bucket = &mut sorted[bucket_start[rank]..bucket_start[rank + 1]];
        insertion_sort(bucket);
    }

    used_symbols[..used_symbol_count].copy_from_slice(&sorted[..used_symbol_count]);
}

fn insertion_sort(items: &mut [(u64, u8)]) {
    for unsorted_start in 1..items.len() {
        let mut position = unsorted_start;
        while position > 0 && items[position - 1] > items[position] {
            items.swap(position - 1, position);
            position -= 1;
        }
    }
}

fn build_code_lengths(counts: &[u32; 256], lengths: &mut [u8; 256], depth_limit: usize) -> u8 {
    for length in lengths.iter_mut() {
        *length = 0;
    }

    let mut used_symbols: [(u64, u8); 256] = [(0, 0); 256];
    let mut used_symbol_count = 0usize;
    for (symbol, &count) in counts.iter().enumerate() {
        if count > 0 {
            used_symbols[used_symbol_count] = (count as u64, symbol as u8);
            used_symbol_count += 1;
        }
    }
    sort_used_symbols_by_count(&mut used_symbols, used_symbol_count);

    let mut level_markers = [[NO_LEAF; MAX_PACKAGE_MERGE_ITEMS]; MAX_HUFFMAN_BITS];
    let mut level_sizes = [0usize; MAX_HUFFMAN_BITS];

    let mut previous_weights = [0u64; MAX_PACKAGE_MERGE_ITEMS];
    let mut previous_size = 0usize;

    for level_index in 0..depth_limit {
        let mut current_weights = [0u64; MAX_PACKAGE_MERGE_ITEMS];
        let mut current_markers = [NO_LEAF; MAX_PACKAGE_MERGE_ITEMS];
        let mut current_size = 0usize;

        let package_count = previous_size / 2;
        let mut leaf_index = 0usize;
        let mut package_index = 0usize;

        while leaf_index < used_symbol_count || package_index < package_count {
            let take_leaf = if leaf_index >= used_symbol_count {
                false
            } else if package_index >= package_count {
                true
            } else {
                let leaf_weight = used_symbols[leaf_index].0;
                let package_weight =
                    previous_weights[2 * package_index] + previous_weights[2 * package_index + 1];
                leaf_weight <= package_weight
            };

            if take_leaf {
                current_weights[current_size] = used_symbols[leaf_index].0;
                current_markers[current_size] = leaf_index as u16;
                leaf_index += 1;
            } else {
                current_weights[current_size] =
                    previous_weights[2 * package_index] + previous_weights[2 * package_index + 1];
                package_index += 1;
            }
            current_size += 1;
        }

        level_markers[level_index] = current_markers;
        level_sizes[level_index] = current_size;
        previous_weights = current_weights;
        previous_size = current_size;
    }

    let mut prefix_size = 2 * (used_symbol_count - 1);
    for level_index in (0..depth_limit).rev() {
        let size = level_sizes[level_index];
        let take = prefix_size.min(size);
        let mut package_count_in_prefix = 0usize;
        for marker in level_markers[level_index][..take].iter().copied() {
            if marker == NO_LEAF {
                package_count_in_prefix += 1;
            } else {
                let symbol = used_symbols[marker as usize].1;
                lengths[symbol as usize] += 1;
            }
        }
        prefix_size = 2 * package_count_in_prefix;
    }

    lengths.iter().copied().max().unwrap_or(0)
}

fn assign_codes(table: &mut HuffmanEncodeTable) {
    let max_bits = table.max_bits;
    let mut position: u32 = 0;
    for weight in 1..=max_bits {
        let bit_count = max_bits + 1 - weight;
        let shift_amount = (max_bits - bit_count) as u32;
        for symbol in 0..table.symbol_count {
            if table.weights[symbol] != weight {
                continue;
            }
            table.codes[symbol] = HuffmanCode {
                code: (position >> shift_amount) as u16,
                bit_count,
            };
            position += 1u32 << (weight - 1);
        }
    }
}

pub(crate) fn write_direct_weights(
    output: &mut [u8],
    table: &HuffmanEncodeTable,
) -> Result<usize, EncodeError> {
    if table.symbol_count == 0 {
        return Err(EncodeError::TableNotUsable);
    }
    let weight_count = table.symbol_count - 1;
    if weight_count == 0 || weight_count > MAX_DIRECT_WEIGHT_COUNT {
        return Err(EncodeError::TableNotUsable);
    }

    let byte_count = weight_count.div_ceil(2);
    let total_size = 1 + byte_count;
    let output = output
        .get_mut(..total_size)
        .ok_or(EncodeError::OutputTooSmall)?;

    output[0] = 127 + weight_count as u8;
    for pair_index in 0..byte_count {
        let first_symbol_index = pair_index * 2;
        let high_nibble = table.weights[first_symbol_index] & 0x0F;
        let second_symbol_index = first_symbol_index + 1;
        let low_nibble = if second_symbol_index < weight_count {
            table.weights[second_symbol_index] & 0x0F
        } else {
            0
        };
        output[1 + pair_index] = (high_nibble << 4) | low_nibble;
    }

    Ok(total_size)
}

pub(crate) fn write_huffman_table(
    output: &mut [u8],
    table: &HuffmanEncodeTable,
    weight_fse_table: &mut FseEncodeTable,
) -> Result<usize, EncodeError> {
    if table.symbol_count < 2 {
        return Err(EncodeError::TableNotUsable);
    }
    let weight_count = table.symbol_count - 1;

    let direct_size = if weight_count == 0 || weight_count > MAX_DIRECT_WEIGHT_COUNT {
        None
    } else {
        Some(1 + weight_count.div_ceil(2))
    };

    let fse_size = write_fse_weights(output, table, weight_fse_table, weight_count).ok();

    match (fse_size, direct_size) {
        (Some(fse_size), Some(direct_size)) if fse_size < direct_size => Ok(fse_size),
        (_, Some(_)) => write_direct_weights(output, table),
        (Some(fse_size), None) => Ok(fse_size),
        (None, None) => Err(EncodeError::InputTooLarge),
    }
}

fn write_fse_weights(
    output: &mut [u8],
    table: &HuffmanEncodeTable,
    weight_fse_table: &mut FseEncodeTable,
    weight_count: usize,
) -> Result<usize, EncodeError> {
    if weight_count < 2 {
        return Err(EncodeError::TableNotUsable);
    }

    let weights = &table.weights[..weight_count];
    let mut counts = [0u32; MAX_WEIGHT_VALUE + 1];
    let mut max_weight_value = 0usize;
    for &weight in weights {
        let weight = weight as usize;
        if weight > MAX_WEIGHT_VALUE {
            return Err(EncodeError::TableNotUsable);
        }
        counts[weight] += 1;
        if weight > max_weight_value {
            max_weight_value = weight;
        }
    }
    let symbol_count = max_weight_value + 1;

    // With a single distinct weight value that value owns every state of the FSE
    // table, so each weight costs zero bits and the stream carries no record of
    // how many weights there are. A decoder stops when its bit reader overruns,
    // which then lands nowhere near the true count, so the table has to go out in
    // the direct form however much larger that is.
    let distinct_weight_values = counts[..symbol_count]
        .iter()
        .filter(|&&count| count > 0)
        .count();
    if distinct_weight_values < 2 {
        return Err(EncodeError::TableNotUsable);
    }

    let accuracy_log = pick_accuracy_log(weight_count, symbol_count, MAX_WEIGHT_ACCURACY_LOG);

    let mut normalized_counts = [0i16; MAX_WEIGHT_VALUE + 1];
    normalize_counts(
        &counts[..symbol_count],
        weight_count,
        accuracy_log,
        &mut normalized_counts[..symbol_count],
    )
    .map_err(|_| EncodeError::TableNotUsable)?;

    build_fse_encode_table(
        &normalized_counts[..symbol_count],
        accuracy_log,
        weight_fse_table,
    )
    .map_err(|_| EncodeError::TableNotUsable)?;

    let body = output.get_mut(1..).ok_or(EncodeError::OutputTooSmall)?;
    let description_bytes = write_fse_table_description(body, weight_fse_table)
        .map_err(|_| EncodeError::TableNotUsable)?;

    let stream_output = body
        .get_mut(description_bytes..)
        .ok_or(EncodeError::OutputTooSmall)?;
    let stream_bytes = write_interleaved_weight_stream(stream_output, weight_fse_table, weights)?;

    let compressed_size = description_bytes + stream_bytes;
    if compressed_size >= 128 {
        return Err(EncodeError::TableNotUsable);
    }
    output[0] = compressed_size as u8;
    Ok(1 + compressed_size)
}

fn write_interleaved_weight_stream(
    output: &mut [u8],
    table: &FseEncodeTable,
    weights: &[u8],
) -> Result<usize, EncodeError> {
    let weight_count = weights.len();
    let last_even_index = if (weight_count - 1).is_multiple_of(2) {
        weight_count - 1
    } else {
        weight_count - 2
    };
    let last_odd_index = if (weight_count - 1).is_multiple_of(2) {
        weight_count - 2
    } else {
        weight_count - 1
    };

    let mut writer = BackwardBitWriter::new(output);
    let mut state_1 = FseEncodeState::new(table, weights[last_even_index]);
    let mut state_2 = FseEncodeState::new(table, weights[last_odd_index]);

    if weight_count >= 3 {
        for global_index in (0..=weight_count - 3).rev() {
            if global_index % 2 == 0 {
                state_1.encode_symbol(&mut writer, table, weights[global_index])?;
            } else {
                state_2.encode_symbol(&mut writer, table, weights[global_index])?;
            }
        }
    }

    state_2.flush(&mut writer, table)?;
    state_1.flush(&mut writer, table)?;
    writer.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::{
        fse_decode_table::FseDecodeTable,
        huffman_decode_table::{HuffmanDecodeTable, read_huffman_table},
    };

    /// A table whose weights are all equal cannot be coded through FSE: that one
    /// weight value owns every state, so each weight costs zero bits and the
    /// stream carries no record of how many weights there are. The encoder used
    /// to pick that form anyway, because it measured shorter, and produced a
    /// description neither this decoder nor the reference implementation could
    /// read.
    #[test]
    fn a_table_with_one_distinct_weight_goes_out_in_the_direct_form() {
        let mut counts = [0u32; 256];
        for count in counts[..16].iter_mut() {
            *count = 100;
        }

        let mut table = HuffmanEncodeTable::new();
        build_huffman_encode_table(&counts, &mut table, MAX_HUFFMAN_BITS).unwrap();

        let weights = &table.weights[..table.symbol_count];
        assert!(
            weights.iter().all(|&weight| weight == weights[0]),
            "this shape is meant to produce one distinct weight"
        );

        let mut weight_fse_table = FseEncodeTable::new();
        let mut written = [0u8; 512];
        let length = write_huffman_table(&mut written, &table, &mut weight_fse_table).unwrap();

        assert!(
            written[0] >= 128,
            "expected the direct form, got a compressed size of {}",
            written[0]
        );

        let mut decode_table = HuffmanDecodeTable::new();
        let mut scratch = FseDecodeTable::new();
        let read = read_huffman_table(&written[..length], &mut decode_table, &mut scratch);
        assert_eq!(read, Ok(length), "the description must be readable");
        assert_eq!(decode_table.max_bits, table.max_bits);
    }

    const SAMPLE_TEXT: &[u8] = b"the quick brown fox jumps over the lazy dog while the sun sets \
slowly behind the distant hills and the wind carries the scent of rain across the quiet valley";

    fn counts_for(input: &[u8]) -> [u32; 256] {
        let mut counts = [0u32; 256];
        for &byte in input {
            counts[byte as usize] += 1;
        }
        counts
    }

    #[test]
    fn builds_hand_computed_table_for_three_symbols() {
        let mut counts = [0u32; 256];
        counts[0] = 5;
        counts[1] = 3;
        counts[2] = 1;

        let mut table = HuffmanEncodeTable::new();
        build_huffman_encode_table(&counts, &mut table, MAX_HUFFMAN_BITS).unwrap();

        assert_eq!(table.max_bits, 2);
        assert_eq!(table.symbol_count, 3);
        assert_eq!(table.weights[0], 2);
        assert_eq!(table.weights[1], 1);
        assert_eq!(table.weights[2], 1);

        assert_eq!(table.codes[0].bit_count, 1);
        assert_eq!(table.codes[0].code, 1);
        assert_eq!(table.codes[1].bit_count, 2);
        assert_eq!(table.codes[1].code, 0);
        assert_eq!(table.codes[2].bit_count, 2);
        assert_eq!(table.codes[2].code, 1);
    }

    #[test]
    fn package_merge_cost_equals_unlimited_huffman_when_depth_fits() {
        use std::{cmp::Reverse, collections::BinaryHeap};

        let counts = counts_for(SAMPLE_TEXT);

        let mut heap: BinaryHeap<Reverse<u64>> = BinaryHeap::new();
        for &count in counts.iter() {
            if count > 0 {
                heap.push(Reverse(count as u64));
            }
        }
        let mut unlimited_cost: u64 = 0;
        while heap.len() > 1 {
            let Reverse(first) = heap.pop().unwrap();
            let Reverse(second) = heap.pop().unwrap();
            let merged = first + second;
            unlimited_cost += merged;
            heap.push(Reverse(merged));
        }

        let mut lengths = [0u8; 256];
        let max_bits = build_code_lengths(&counts, &mut lengths, MAX_HUFFMAN_BITS);
        assert!(max_bits as usize <= MAX_HUFFMAN_BITS);

        let package_merge_cost: u64 = (0..256)
            .map(|symbol| counts[symbol] as u64 * lengths[symbol] as u64)
            .sum();

        assert_eq!(package_merge_cost, unlimited_cost);
    }

    #[test]
    fn limits_lengths_to_eleven_bits_with_exact_kraft_sum() {
        let mut counts = [0u32; 256];
        for (symbol, count) in counts.iter_mut().enumerate().take(20) {
            *count = 1u32 << symbol;
        }

        let mut lengths = [0u8; 256];
        let max_bits = build_code_lengths(&counts, &mut lengths, MAX_HUFFMAN_BITS);

        assert!(max_bits as usize <= MAX_HUFFMAN_BITS);

        let kraft_sum: u64 = (0..20)
            .map(|symbol| 1u64 << (max_bits - lengths[symbol]))
            .sum();
        assert_eq!(kraft_sum, 1u64 << max_bits);

        for smaller in 0..20usize {
            for larger in (smaller + 1)..20usize {
                assert!(counts[smaller] <= counts[larger]);
                assert!(lengths[smaller] >= lengths[larger]);
            }
        }
    }

    #[test]
    fn written_table_round_trips_through_decoder() {
        let counts = counts_for(SAMPLE_TEXT);
        let mut table = HuffmanEncodeTable::new();
        build_huffman_encode_table(&counts, &mut table, MAX_HUFFMAN_BITS).unwrap();

        let mut output = [0u8; 256];
        let bytes_written = write_direct_weights(&mut output, &table).unwrap();

        let mut decode_table = HuffmanDecodeTable::new();
        let mut weight_fse_table = FseDecodeTable::new();
        let bytes_consumed =
            read_huffman_table(&output, &mut decode_table, &mut weight_fse_table).unwrap();

        assert_eq!(bytes_consumed, bytes_written);
        assert!(decode_table.is_ready);
        assert_eq!(decode_table.max_bits, table.max_bits);

        for symbol in 0..table.symbol_count {
            if table.weights[symbol] == 0 {
                continue;
            }
            let code = table.codes[symbol];
            let cell_index = (code.code as usize) << (table.max_bits - code.bit_count);
            let cell = decode_table.entries[cell_index];
            assert_eq!(cell.symbol as usize, symbol);
            assert_eq!(cell.bit_count, code.bit_count);
        }
    }

    #[test]
    fn rejects_tables_the_direct_form_cannot_express() {
        let mut single_symbol_counts = [0u32; 256];
        single_symbol_counts[0] = 10;
        let mut table = HuffmanEncodeTable::new();
        assert_eq!(
            build_huffman_encode_table(&single_symbol_counts, &mut table, MAX_HUFFMAN_BITS),
            Err(EncodeError::TableNotUsable)
        );

        let mut many_symbol_counts = [0u32; 256];
        for count in many_symbol_counts.iter_mut().take(200) {
            *count = 1;
        }
        let mut wide_table = HuffmanEncodeTable::new();
        build_huffman_encode_table(&many_symbol_counts, &mut wide_table, MAX_HUFFMAN_BITS).unwrap();
        let mut output = [0u8; 256];
        assert_eq!(
            write_direct_weights(&mut output, &wide_table),
            Err(EncodeError::TableNotUsable)
        );

        let mut small_counts = [0u32; 256];
        small_counts[0] = 5;
        small_counts[1] = 3;
        small_counts[2] = 1;
        let mut small_table = HuffmanEncodeTable::new();
        build_huffman_encode_table(&small_counts, &mut small_table, MAX_HUFFMAN_BITS).unwrap();
        let mut tiny_output = [0u8; 1];
        assert_eq!(
            write_direct_weights(&mut tiny_output, &small_table),
            Err(EncodeError::OutputTooSmall)
        );
    }

    fn assert_decode_table_agrees_with_encode_table(
        encode_table: &HuffmanEncodeTable,
        output: &[u8],
    ) {
        let mut decode_table = HuffmanDecodeTable::new();
        let mut weight_fse_table = FseDecodeTable::new();
        let bytes_consumed =
            read_huffman_table(output, &mut decode_table, &mut weight_fse_table).unwrap();
        assert_eq!(bytes_consumed, output.len());
        assert!(decode_table.is_ready);
        assert_eq!(decode_table.max_bits, encode_table.max_bits);

        for symbol in 0..encode_table.symbol_count {
            if encode_table.weights[symbol] == 0 {
                continue;
            }
            let code = encode_table.codes[symbol];
            let cell_index = (code.code as usize) << (encode_table.max_bits - code.bit_count);
            let cell = decode_table.entries[cell_index];
            assert_eq!(cell.symbol as usize, symbol);
            assert_eq!(cell.bit_count, code.bit_count);
        }
    }

    #[test]
    fn write_huffman_table_picks_fse_weights_for_a_varied_256_symbol_histogram() {
        let mut random_state = 0x9E3779B97F4A7C15u64;
        let mut next_random = || {
            random_state ^= random_state << 13;
            random_state ^= random_state >> 7;
            random_state ^= random_state << 17;
            random_state
        };

        let mut input = Vec::new();
        input.extend_from_slice(SAMPLE_TEXT);
        input.extend_from_slice(SAMPLE_TEXT);
        for symbol in 0..256usize {
            let repeats = 1 + (next_random() % 40) as usize;
            for _ in 0..repeats {
                input.push(symbol as u8);
            }
        }

        let counts = counts_for(&input);
        let mut table = HuffmanEncodeTable::new();
        build_huffman_encode_table(&counts, &mut table, MAX_HUFFMAN_BITS).unwrap();

        let mut output = [0u8; 4096];
        let mut weight_fse_table = FseEncodeTable::new();
        let bytes_written =
            write_huffman_table(&mut output, &table, &mut weight_fse_table).unwrap();

        assert!(output[0] < 128);
        assert_decode_table_agrees_with_encode_table(&table, &output[..bytes_written]);
    }

    #[test]
    fn write_huffman_table_picks_direct_weights_for_a_small_text_histogram() {
        let mut counts = [0u32; 256];
        counts[0] = 20;
        counts[1] = 3;
        let mut table = HuffmanEncodeTable::new();
        build_huffman_encode_table(&counts, &mut table, MAX_HUFFMAN_BITS).unwrap();
        assert_eq!(table.symbol_count - 1, 1);

        let mut output = [0u8; 256];
        let mut weight_fse_table = FseEncodeTable::new();
        let bytes_written =
            write_huffman_table(&mut output, &table, &mut weight_fse_table).unwrap();

        assert!(output[0] >= 128);
        assert_decode_table_agrees_with_encode_table(&table, &output[..bytes_written]);
    }
}
