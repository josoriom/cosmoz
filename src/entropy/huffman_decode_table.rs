use crate::bits::backward_bit_reader::BackwardBitReader;
use crate::entropy::fse_decode_table::{
    FseDecodeState, FseDecodeTable, read_fse_table_description,
};
use crate::error::DecodeError;

pub const MAX_HUFFMAN_BITS: usize = 11;
pub const MAX_HUFFMAN_TABLE_SIZE: usize = 1 << MAX_HUFFMAN_BITS;
pub const MAX_WEIGHT_COUNT: usize = 256;
pub const MAX_WEIGHT_ACCURACY_LOG: usize = 6;

#[derive(Clone, Copy, Default)]
pub struct HuffmanDecodeEntry {
    pub symbol: u8,
    pub bit_count: u8,
}

pub struct HuffmanDecodeTable {
    pub entries: [HuffmanDecodeEntry; MAX_HUFFMAN_TABLE_SIZE],
    pub max_bits: u8,
    pub is_ready: bool,
}

impl HuffmanDecodeTable {
    pub const fn new() -> Self {
        Self {
            entries: [HuffmanDecodeEntry {
                symbol: 0,
                bit_count: 0,
            }; MAX_HUFFMAN_TABLE_SIZE],
            max_bits: 0,
            is_ready: false,
        }
    }
}

impl Default for HuffmanDecodeTable {
    fn default() -> Self {
        Self::new()
    }
}

pub fn read_huffman_table(
    input: &[u8],
    table: &mut HuffmanDecodeTable,
    weight_fse_table: &mut FseDecodeTable,
) -> Result<usize, DecodeError> {
    table.is_ready = false;
    let header_byte = *input.first().ok_or(DecodeError::InputTooShort)?;
    if header_byte >= 128 {
        return read_huffman_table_from_direct_weights(input, table);
    }
    let compressed_size = header_byte as usize;
    let weights_input = input.get(1..).ok_or(DecodeError::InputTooShort)?;
    let mut weights = [0u8; MAX_WEIGHT_COUNT];
    let weight_count = read_fse_weights(
        weights_input,
        compressed_size,
        &mut weights,
        weight_fse_table,
    )?;
    let symbol_count = add_last_weight(&mut weights, weight_count)?;
    build_huffman_decode_table(&weights[..symbol_count], symbol_count, table)?;
    Ok(1 + compressed_size)
}

fn read_huffman_table_from_direct_weights(
    input: &[u8],
    table: &mut HuffmanDecodeTable,
) -> Result<usize, DecodeError> {
    table.is_ready = false;
    let header_byte = *input.first().ok_or(DecodeError::InputTooShort)?;
    if header_byte < 128 {
        return Err(DecodeError::BadHuffmanWeights);
    }
    let weight_count = (header_byte - 127) as usize;
    let weights_input = input.get(1..).ok_or(DecodeError::InputTooShort)?;
    let mut weights = [0u8; MAX_WEIGHT_COUNT];
    let weight_bytes_used = read_direct_weights(weights_input, weight_count, &mut weights)?;
    let symbol_count = add_last_weight(&mut weights, weight_count)?;
    build_huffman_decode_table(&weights[..symbol_count], symbol_count, table)?;
    Ok(1 + weight_bytes_used)
}

fn read_fse_weights(
    input: &[u8],
    compressed_size: usize,
    weights: &mut [u8],
    weight_fse_table: &mut FseDecodeTable,
) -> Result<usize, DecodeError> {
    let section = input
        .get(..compressed_size)
        .ok_or(DecodeError::InputTooShort)?;
    let description_bytes =
        read_fse_table_description(section, MAX_WEIGHT_ACCURACY_LOG, 255, weight_fse_table)
            .map_err(|_| DecodeError::BadHuffmanWeights)?;
    let bitstream = section
        .get(description_bytes..)
        .ok_or(DecodeError::BadHuffmanWeights)?;
    let mut reader =
        BackwardBitReader::new(bitstream).map_err(|_| DecodeError::BadHuffmanWeights)?;
    let mut state_1 = FseDecodeState::new(&mut reader, weight_fse_table);
    let mut state_2 = FseDecodeState::new(&mut reader, weight_fse_table);
    if reader.has_overflowed() {
        return Err(DecodeError::BadHuffmanWeights);
    }
    let mut weight_count = 0usize;
    loop {
        push_weight(
            weights,
            &mut weight_count,
            state_1.get_symbol(weight_fse_table),
        )?;
        state_1.update(&mut reader, weight_fse_table);
        if reader.has_overflowed() {
            push_weight(
                weights,
                &mut weight_count,
                state_2.get_symbol(weight_fse_table),
            )?;
            break;
        }
        push_weight(
            weights,
            &mut weight_count,
            state_2.get_symbol(weight_fse_table),
        )?;
        state_2.update(&mut reader, weight_fse_table);
        if reader.has_overflowed() {
            push_weight(
                weights,
                &mut weight_count,
                state_1.get_symbol(weight_fse_table),
            )?;
            break;
        }
    }
    Ok(weight_count)
}

fn push_weight(
    weights: &mut [u8],
    weight_count: &mut usize,
    weight: u8,
) -> Result<(), DecodeError> {
    if *weight_count >= MAX_WEIGHT_COUNT - 1 {
        return Err(DecodeError::BadHuffmanWeights);
    }
    weights[*weight_count] = weight;
    *weight_count += 1;
    Ok(())
}

fn read_direct_weights(
    input: &[u8],
    weight_count: usize,
    weights: &mut [u8],
) -> Result<usize, DecodeError> {
    if weight_count == 0 || weight_count > weights.len() {
        return Err(DecodeError::BadHuffmanWeights);
    }
    let byte_count = weight_count.div_ceil(2);
    let weight_bytes = input.get(..byte_count).ok_or(DecodeError::InputTooShort)?;
    for (pair_index, byte) in weight_bytes.iter().enumerate() {
        let first_symbol_index = pair_index * 2;
        weights[first_symbol_index] = byte >> 4;
        let second_symbol_index = first_symbol_index + 1;
        if second_symbol_index < weight_count {
            weights[second_symbol_index] = byte & 0x0F;
        }
    }
    Ok(byte_count)
}

fn add_last_weight(weights: &mut [u8], weight_count: usize) -> Result<usize, DecodeError> {
    if weight_count == 0 || weight_count >= weights.len() {
        return Err(DecodeError::BadHuffmanWeights);
    }
    let mut weighted_sum: usize = 0;
    for weight in weights[..weight_count].iter().copied() {
        if weight as usize > MAX_HUFFMAN_BITS {
            return Err(DecodeError::BadHuffmanWeights);
        }
        if weight > 0 {
            let weight_unit = 1usize << (weight as usize - 1);
            weighted_sum = weighted_sum
                .checked_add(weight_unit)
                .ok_or(DecodeError::BadHuffmanWeights)?;
        }
    }
    if weighted_sum == 0 {
        return Err(DecodeError::BadHuffmanWeights);
    }
    let max_bits = find_highest_set_bit_position(weighted_sum) + 1;
    if max_bits > MAX_HUFFMAN_BITS {
        return Err(DecodeError::BadHuffmanWeights);
    }
    let remainder = (1usize << max_bits) - weighted_sum;
    let remainder_highest_bit = find_highest_set_bit_position(remainder);
    if remainder != 1usize << remainder_highest_bit {
        return Err(DecodeError::BadHuffmanWeights);
    }
    weights[weight_count] = (remainder_highest_bit + 1) as u8;
    Ok(weight_count + 1)
}

fn build_huffman_decode_table(
    weights: &[u8],
    symbol_count: usize,
    table: &mut HuffmanDecodeTable,
) -> Result<(), DecodeError> {
    table.is_ready = false;
    if symbol_count == 0 || symbol_count > MAX_WEIGHT_COUNT || symbol_count > weights.len() {
        return Err(DecodeError::BadHuffmanWeights);
    }

    let mut total_weight_units: usize = 0;
    for weight in weights[..symbol_count].iter().copied() {
        if weight as usize > MAX_HUFFMAN_BITS {
            return Err(DecodeError::BadHuffmanWeights);
        }
        if weight > 0 {
            let weight_unit = 1usize << (weight as usize - 1);
            total_weight_units = total_weight_units
                .checked_add(weight_unit)
                .ok_or(DecodeError::BadHuffmanWeights)?;
        }
    }
    if total_weight_units == 0 {
        return Err(DecodeError::BadHuffmanWeights);
    }

    let max_bits = find_highest_set_bit_position(total_weight_units);
    if max_bits > MAX_HUFFMAN_BITS {
        return Err(DecodeError::BadHuffmanWeights);
    }
    let table_size = 1usize << max_bits;
    if total_weight_units != table_size {
        return Err(DecodeError::BadHuffmanWeights);
    }

    let mut position = 0usize;
    for weight in 1..=max_bits {
        let bit_count = max_bits + 1 - weight;
        let block_size = 1usize << (weight - 1);
        for (symbol_index, symbol_weight) in weights[..symbol_count].iter().copied().enumerate() {
            if symbol_weight as usize != weight {
                continue;
            }
            let block_end = position
                .checked_add(block_size)
                .ok_or(DecodeError::BadHuffmanWeights)?;
            if block_end > table_size {
                return Err(DecodeError::BadHuffmanWeights);
            }
            let filled_entry = HuffmanDecodeEntry {
                symbol: symbol_index as u8,
                bit_count: bit_count as u8,
            };
            if let Some(cells) = table.entries.get_mut(position..block_end) {
                for cell in cells.iter_mut() {
                    *cell = filled_entry;
                }
            } else {
                return Err(DecodeError::BadHuffmanWeights);
            }
            position = block_end;
        }
    }
    if position != table_size {
        return Err(DecodeError::BadHuffmanWeights);
    }

    table.max_bits = max_bits as u8;
    table.is_ready = true;
    Ok(())
}

fn find_highest_set_bit_position(value: usize) -> usize {
    (usize::BITS - 1 - value.leading_zeros()) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_table_from_rfc_worked_example() {
        let weights = [4u8, 3, 2, 0, 1, 1];
        let mut table = HuffmanDecodeTable::new();
        build_huffman_decode_table(&weights, weights.len(), &mut table).unwrap();

        assert!(table.is_ready);
        assert_eq!(table.max_bits, 4);

        assert_eq!(table.entries[0].symbol, 4);
        assert_eq!(table.entries[0].bit_count, 4);
        assert_eq!(table.entries[1].symbol, 5);
        assert_eq!(table.entries[1].bit_count, 4);

        for cell in 2..4 {
            assert_eq!(table.entries[cell].symbol, 2);
            assert_eq!(table.entries[cell].bit_count, 3);
        }
        for cell in 4..8 {
            assert_eq!(table.entries[cell].symbol, 1);
            assert_eq!(table.entries[cell].bit_count, 2);
        }
        for cell in 8..16 {
            assert_eq!(table.entries[cell].symbol, 0);
            assert_eq!(table.entries[cell].bit_count, 1);
        }
    }

    #[test]
    fn derives_last_weight_and_rejects_invalid_weight_sums() {
        let mut valid_weights = [2u8, 2, 2, 0];
        let symbol_count = add_last_weight(&mut valid_weights, 3).unwrap();
        assert_eq!(symbol_count, 4);
        assert_eq!(valid_weights[3], 2);

        let mut non_power_of_two_remainder = [3u8, 3, 2, 0];
        assert_eq!(
            add_last_weight(&mut non_power_of_two_remainder, 3),
            Err(DecodeError::BadHuffmanWeights)
        );

        let mut all_zero_weights = [0u8, 0, 0];
        assert_eq!(
            add_last_weight(&mut all_zero_weights, 2),
            Err(DecodeError::BadHuffmanWeights)
        );

        let mut weight_too_large = [12u8, 0];
        assert_eq!(
            add_last_weight(&mut weight_too_large, 1),
            Err(DecodeError::BadHuffmanWeights)
        );
    }

    #[test]
    fn reads_direct_weights_header() {
        let header_byte = 127 + 3;
        let input = [header_byte, 0x11, 0x25];
        let mut table = HuffmanDecodeTable::new();

        let bytes_consumed = read_huffman_table_from_direct_weights(&input, &mut table).unwrap();

        assert_eq!(bytes_consumed, 3);
        assert!(table.is_ready);
        assert_eq!(table.max_bits, 3);
        assert_eq!(table.entries[0].symbol, 0);
        assert_eq!(table.entries[0].bit_count, 3);
        assert_eq!(table.entries[1].symbol, 1);
        assert_eq!(table.entries[1].bit_count, 3);
        assert_eq!(table.entries[2].symbol, 2);
        assert_eq!(table.entries[2].bit_count, 2);
        assert_eq!(table.entries[3].symbol, 2);
        assert_eq!(table.entries[3].bit_count, 2);
        for cell in 4..8 {
            assert_eq!(table.entries[cell].symbol, 3);
            assert_eq!(table.entries[cell].bit_count, 1);
        }
    }

    #[test]
    fn rejects_header_below_direct_representation_threshold() {
        let input = [127u8, 0x00];
        let mut table = HuffmanDecodeTable::new();
        assert_eq!(
            read_huffman_table_from_direct_weights(&input, &mut table),
            Err(DecodeError::BadHuffmanWeights)
        );
    }

    #[test]
    fn reads_direct_weights_through_the_public_entry_point() {
        let header_byte = 127 + 3;
        let input = [header_byte, 0x11, 0x25];
        let mut table = HuffmanDecodeTable::new();
        let mut weight_fse_table = FseDecodeTable::new();

        let bytes_consumed = read_huffman_table(&input, &mut table, &mut weight_fse_table).unwrap();

        assert_eq!(bytes_consumed, 3);
        assert!(table.is_ready);
        assert_eq!(table.max_bits, 3);
    }

    #[test]
    fn reads_fse_compressed_huffman_weights_from_a_real_zstd_19_block() {
        const HUFFMAN_HEADER_AND_FSE_WEIGHTS: [u8; 32] = [
            0x1f, 0x10, 0xad, 0x07, 0x6f, 0x8a, 0x6e, 0xb3, 0x99, 0xb5, 0x95, 0x27, 0xaa, 0x6c,
            0xe3, 0x3d, 0xc8, 0x91, 0x36, 0xe1, 0xf2, 0xd6, 0xfe, 0x7c, 0x6c, 0x40, 0x8a, 0x00,
            0x00, 0x00, 0x00, 0x04,
        ];
        let mut table = HuffmanDecodeTable::new();
        let mut weight_fse_table = FseDecodeTable::new();

        let bytes_consumed = read_huffman_table(
            &HUFFMAN_HEADER_AND_FSE_WEIGHTS,
            &mut table,
            &mut weight_fse_table,
        )
        .unwrap();

        assert_eq!(bytes_consumed, HUFFMAN_HEADER_AND_FSE_WEIGHTS.len());
        assert!(table.is_ready);
        assert_eq!(table.max_bits, 7);
        assert!((table.max_bits as usize) <= MAX_HUFFMAN_BITS);
    }
}
