use crate::{
    bits::backward_bit_reader::BackwardBitReader,
    entropy::{
        huffman_decode_fast::{decode_eight_streams_unchecked, decode_four_streams_unchecked},
        huffman_decode_table::{HuffmanDecodeEntry, HuffmanDecodeTable},
    },
    error::DecodeError,
};

pub fn decode_one_stream(
    input: &[u8],
    table: &HuffmanDecodeTable,
    output: &mut [u8],
) -> Result<(), DecodeError> {
    if !table.is_ready {
        return Err(DecodeError::BadHuffmanWeights);
    }
    let mut reader = BackwardBitReader::new(input)?;
    decode_symbols(&mut reader, table, output);
    if !reader.is_finished() {
        return Err(DecodeError::CorruptBitstream);
    }
    Ok(())
}

pub fn decode_many_streams(
    input: &[u8],
    table: &HuffmanDecodeTable,
    stream_count: usize,
    output: &mut [u8],
) -> Result<(), DecodeError> {
    let regenerated_size = output.len();
    decode_many_streams_with_slack(input, table, stream_count, output, regenerated_size)
}

pub fn decode_many_streams_with_slack(
    input: &[u8],
    table: &HuffmanDecodeTable,
    stream_count: usize,
    output: &mut [u8],
    regenerated_size: usize,
) -> Result<(), DecodeError> {
    if !table.is_ready {
        return Err(DecodeError::BadHuffmanWeights);
    }
    if stream_count != 4 && stream_count != 8 {
        return Err(DecodeError::CorruptBitstream);
    }
    if regenerated_size > output.len() {
        return Err(DecodeError::OutputTooSmall);
    }

    let jump_table_size = (stream_count - 1) * 2;
    let after_jump_table = input
        .get(jump_table_size..)
        .ok_or(DecodeError::CorruptBitstream)?;

    let segment_size = regenerated_size
        .checked_add(stream_count - 1)
        .ok_or(DecodeError::CorruptBitstream)?
        / stream_count;
    let leading_segments_size = segment_size
        .checked_mul(stream_count - 1)
        .ok_or(DecodeError::CorruptBitstream)?;
    if leading_segments_size > regenerated_size {
        return Err(DecodeError::CorruptBitstream);
    }

    let mut stream_slices: [&[u8]; 8] = [&[]; 8];
    let mut segment_lengths: [usize; 8] = [0; 8];
    let mut remaining_input = after_jump_table;
    let mut remaining_output_length = regenerated_size;
    for stream_index in 0..stream_count {
        let is_last_stream = stream_index == stream_count - 1;
        let stream = if is_last_stream {
            remaining_input
        } else {
            let stream_size = read_little_endian_u16(input, stream_index * 2)? as usize;
            let stream = remaining_input
                .get(..stream_size)
                .ok_or(DecodeError::CorruptBitstream)?;
            remaining_input = remaining_input
                .get(stream_size..)
                .ok_or(DecodeError::CorruptBitstream)?;
            stream
        };
        let segment_length = if is_last_stream {
            remaining_output_length
        } else {
            segment_size
        };
        if segment_length > remaining_output_length {
            return Err(DecodeError::CorruptBitstream);
        }
        if is_last_stream && segment_length == 0 {
            return Err(DecodeError::CorruptBitstream);
        }
        remaining_output_length -= segment_length;
        stream_slices[stream_index] = stream;
        segment_lengths[stream_index] = segment_length;
    }

    let streams_are_well_formed = stream_slices[..stream_count]
        .iter()
        .all(|stream| !stream.is_empty());
    if !streams_are_well_formed {
        return Err(DecodeError::InputTooShort);
    }

    let output_pointer = output.as_mut_ptr();
    if stream_count == 4 {
        let streams = [
            stream_slices[0],
            stream_slices[1],
            stream_slices[2],
            stream_slices[3],
        ];
        let segments = [
            segment_lengths[0],
            segment_lengths[1],
            segment_lengths[2],
            segment_lengths[3],
        ];
        unsafe { decode_four_streams_unchecked(streams, table, output_pointer, segments) }
    } else {
        let streams = [
            stream_slices[0],
            stream_slices[1],
            stream_slices[2],
            stream_slices[3],
            stream_slices[4],
            stream_slices[5],
            stream_slices[6],
            stream_slices[7],
        ];
        let segments = [
            segment_lengths[0],
            segment_lengths[1],
            segment_lengths[2],
            segment_lengths[3],
            segment_lengths[4],
            segment_lengths[5],
            segment_lengths[6],
            segment_lengths[7],
        ];
        unsafe { decode_eight_streams_unchecked(streams, table, output_pointer, segments) }
    }
}

#[cfg(test)]
fn decode_many_streams_checked(
    output: &mut [u8],
    table: &HuffmanDecodeTable,
    stream_count: usize,
    stream_slices: [&[u8]; 8],
    segment_lengths: [usize; 8],
) -> Result<(), DecodeError> {
    if stream_count == 4 {
        let mut readers = [
            BackwardBitReader::new(stream_slices[0])?,
            BackwardBitReader::new(stream_slices[1])?,
            BackwardBitReader::new(stream_slices[2])?,
            BackwardBitReader::new(stream_slices[3])?,
        ];
        let (segment_0, rest) = output.split_at_mut(segment_lengths[0]);
        let (segment_1, rest) = rest.split_at_mut(segment_lengths[1]);
        let (segment_2, segment_3) = rest.split_at_mut(segment_lengths[2]);
        let mut segments = [segment_0, segment_1, segment_2, segment_3];
        decode_four_streams_interleaved(
            &mut readers,
            &mut segments,
            [
                segment_lengths[0],
                segment_lengths[1],
                segment_lengths[2],
                segment_lengths[3],
            ],
            table,
        );
        for reader in readers.iter() {
            if !reader.is_finished() {
                return Err(DecodeError::CorruptBitstream);
            }
        }
    } else {
        let mut readers = [
            BackwardBitReader::new(stream_slices[0])?,
            BackwardBitReader::new(stream_slices[1])?,
            BackwardBitReader::new(stream_slices[2])?,
            BackwardBitReader::new(stream_slices[3])?,
            BackwardBitReader::new(stream_slices[4])?,
            BackwardBitReader::new(stream_slices[5])?,
            BackwardBitReader::new(stream_slices[6])?,
            BackwardBitReader::new(stream_slices[7])?,
        ];
        let (segment_0, rest) = output.split_at_mut(segment_lengths[0]);
        let (segment_1, rest) = rest.split_at_mut(segment_lengths[1]);
        let (segment_2, rest) = rest.split_at_mut(segment_lengths[2]);
        let (segment_3, rest) = rest.split_at_mut(segment_lengths[3]);
        let (segment_4, rest) = rest.split_at_mut(segment_lengths[4]);
        let (segment_5, rest) = rest.split_at_mut(segment_lengths[5]);
        let (segment_6, segment_7) = rest.split_at_mut(segment_lengths[6]);
        let mut segments = [
            segment_0, segment_1, segment_2, segment_3, segment_4, segment_5, segment_6, segment_7,
        ];
        decode_eight_streams_interleaved(
            &mut readers,
            &mut segments,
            [
                segment_lengths[0],
                segment_lengths[1],
                segment_lengths[2],
                segment_lengths[3],
                segment_lengths[4],
                segment_lengths[5],
                segment_lengths[6],
                segment_lengths[7],
            ],
            table,
        );
        for reader in readers.iter() {
            if !reader.is_finished() {
                return Err(DecodeError::CorruptBitstream);
            }
        }
    }

    Ok(())
}

fn decode_entry_unchecked(
    entries: &[HuffmanDecodeEntry],
    index: usize,
    max_bits: usize,
) -> HuffmanDecodeEntry {
    debug_assert!(index < (1usize << max_bits));
    unsafe { *entries.get_unchecked(index) }
}

fn decode_symbol(
    reader: &mut BackwardBitReader<'_>,
    table: &HuffmanDecodeTable,
    max_bits: usize,
) -> u8 {
    reader.refill();
    let cell = reader.peek_bits(max_bits) as usize;
    let entry = decode_entry_unchecked(&table.entries, cell, max_bits);
    reader.skip_bits(entry.bit_count as usize);
    entry.symbol
}

fn decode_symbol_pair(
    reader: &mut BackwardBitReader<'_>,
    table: &HuffmanDecodeTable,
    max_bits: usize,
    window_bits: usize,
) -> (u8, u8) {
    reader.refill();
    let window = reader.peek_bits(window_bits);
    let first_index = (window >> max_bits) as usize;
    let first_entry = decode_entry_unchecked(&table.entries, first_index, max_bits);
    let first_bit_count = first_entry.bit_count as usize;
    let window_mask = (1u64 << window_bits) - 1;
    let shifted = (window << first_bit_count) & window_mask;
    let second_index = (shifted >> max_bits) as usize;
    let second_entry = decode_entry_unchecked(&table.entries, second_index, max_bits);
    reader.skip_bits(first_bit_count + second_entry.bit_count as usize);
    (first_entry.symbol, second_entry.symbol)
}

#[cfg(test)]
#[allow(clippy::needless_range_loop)]
fn decode_four_streams_interleaved(
    readers: &mut [BackwardBitReader<'_>; 4],
    segments: &mut [&mut [u8]; 4],
    lengths: [usize; 4],
    table: &HuffmanDecodeTable,
) {
    let max_bits = table.max_bits as usize;
    let window_bits = max_bits * 2;
    let common_length = lengths[0].min(lengths[1]).min(lengths[2]).min(lengths[3]);
    let paired_length = common_length - (common_length % 2);
    let (reader_0, reader_1, reader_2, reader_3) = {
        let [reader_0, reader_1, reader_2, reader_3] = readers;
        (reader_0, reader_1, reader_2, reader_3)
    };
    let mut position = 0;
    while position < paired_length {
        let (symbol_0, symbol_1) = decode_symbol_pair(reader_0, table, max_bits, window_bits);
        segments[0][position] = symbol_0;
        segments[0][position + 1] = symbol_1;
        let (symbol_0, symbol_1) = decode_symbol_pair(reader_1, table, max_bits, window_bits);
        segments[1][position] = symbol_0;
        segments[1][position + 1] = symbol_1;
        let (symbol_0, symbol_1) = decode_symbol_pair(reader_2, table, max_bits, window_bits);
        segments[2][position] = symbol_0;
        segments[2][position + 1] = symbol_1;
        let (symbol_0, symbol_1) = decode_symbol_pair(reader_3, table, max_bits, window_bits);
        segments[3][position] = symbol_0;
        segments[3][position + 1] = symbol_1;
        position += 2;
    }
    if position < common_length {
        segments[0][position] = decode_symbol(reader_0, table, max_bits);
        segments[1][position] = decode_symbol(reader_1, table, max_bits);
        segments[2][position] = decode_symbol(reader_2, table, max_bits);
        segments[3][position] = decode_symbol(reader_3, table, max_bits);
    }
    for stream_index in 0..4 {
        for position in common_length..lengths[stream_index] {
            segments[stream_index][position] =
                decode_symbol(&mut readers[stream_index], table, max_bits);
        }
    }
}

#[cfg(test)]
#[allow(clippy::needless_range_loop)]
fn decode_eight_streams_interleaved(
    readers: &mut [BackwardBitReader<'_>; 8],
    segments: &mut [&mut [u8]; 8],
    lengths: [usize; 8],
    table: &HuffmanDecodeTable,
) {
    let max_bits = table.max_bits as usize;
    let window_bits = max_bits * 2;
    let common_length = lengths.iter().copied().min().unwrap_or(0);
    let paired_length = common_length - (common_length % 2);
    let (reader_0, reader_1, reader_2, reader_3, reader_4, reader_5, reader_6, reader_7) = {
        let [
            reader_0,
            reader_1,
            reader_2,
            reader_3,
            reader_4,
            reader_5,
            reader_6,
            reader_7,
        ] = readers;
        (
            reader_0, reader_1, reader_2, reader_3, reader_4, reader_5, reader_6, reader_7,
        )
    };
    let mut position = 0;
    while position < paired_length {
        let (symbol_0, symbol_1) = decode_symbol_pair(reader_0, table, max_bits, window_bits);
        segments[0][position] = symbol_0;
        segments[0][position + 1] = symbol_1;
        let (symbol_0, symbol_1) = decode_symbol_pair(reader_1, table, max_bits, window_bits);
        segments[1][position] = symbol_0;
        segments[1][position + 1] = symbol_1;
        let (symbol_0, symbol_1) = decode_symbol_pair(reader_2, table, max_bits, window_bits);
        segments[2][position] = symbol_0;
        segments[2][position + 1] = symbol_1;
        let (symbol_0, symbol_1) = decode_symbol_pair(reader_3, table, max_bits, window_bits);
        segments[3][position] = symbol_0;
        segments[3][position + 1] = symbol_1;
        let (symbol_0, symbol_1) = decode_symbol_pair(reader_4, table, max_bits, window_bits);
        segments[4][position] = symbol_0;
        segments[4][position + 1] = symbol_1;
        let (symbol_0, symbol_1) = decode_symbol_pair(reader_5, table, max_bits, window_bits);
        segments[5][position] = symbol_0;
        segments[5][position + 1] = symbol_1;
        let (symbol_0, symbol_1) = decode_symbol_pair(reader_6, table, max_bits, window_bits);
        segments[6][position] = symbol_0;
        segments[6][position + 1] = symbol_1;
        let (symbol_0, symbol_1) = decode_symbol_pair(reader_7, table, max_bits, window_bits);
        segments[7][position] = symbol_0;
        segments[7][position + 1] = symbol_1;
        position += 2;
    }
    if position < common_length {
        segments[0][position] = decode_symbol(reader_0, table, max_bits);
        segments[1][position] = decode_symbol(reader_1, table, max_bits);
        segments[2][position] = decode_symbol(reader_2, table, max_bits);
        segments[3][position] = decode_symbol(reader_3, table, max_bits);
        segments[4][position] = decode_symbol(reader_4, table, max_bits);
        segments[5][position] = decode_symbol(reader_5, table, max_bits);
        segments[6][position] = decode_symbol(reader_6, table, max_bits);
        segments[7][position] = decode_symbol(reader_7, table, max_bits);
    }
    for stream_index in 0..8 {
        for position in common_length..lengths[stream_index] {
            segments[stream_index][position] =
                decode_symbol(&mut readers[stream_index], table, max_bits);
        }
    }
}

pub fn decode_four_streams(
    input: &[u8],
    table: &HuffmanDecodeTable,
    output: &mut [u8],
) -> Result<(), DecodeError> {
    decode_many_streams(input, table, 4, output)
}

pub fn decode_eight_streams(
    input: &[u8],
    table: &HuffmanDecodeTable,
    output: &mut [u8],
) -> Result<(), DecodeError> {
    decode_many_streams(input, table, 8, output)
}

fn decode_symbols(
    reader: &mut BackwardBitReader<'_>,
    table: &HuffmanDecodeTable,
    output: &mut [u8],
) {
    let max_bits = table.max_bits as usize;
    let window_bits = max_bits * 2;
    let output_length = output.len();
    let paired_length = output_length - (output_length % 2);
    let mut position = 0;
    while position < paired_length {
        let (symbol_0, symbol_1) = decode_symbol_pair(reader, table, max_bits, window_bits);
        output[position] = symbol_0;
        output[position + 1] = symbol_1;
        position += 2;
    }
    if position < output_length {
        output[position] = decode_symbol(reader, table, max_bits);
    }
}

fn read_little_endian_u16(input: &[u8], offset: usize) -> Result<u16, DecodeError> {
    let bytes = input
        .get(offset..offset + 2)
        .ok_or(DecodeError::CorruptBitstream)?;
    let byte_array: [u8; 2] = bytes
        .try_into()
        .map_err(|_| DecodeError::CorruptBitstream)?;
    Ok(u16::from_le_bytes(byte_array))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::huffman_decode_table::HuffmanDecodeEntry;

    fn build_rfc_worked_example_table() -> HuffmanDecodeTable {
        let mut table = HuffmanDecodeTable::new();
        table.max_bits = 4;
        table.is_ready = true;

        let assign = |table: &mut HuffmanDecodeTable,
                      range: core::ops::Range<usize>,
                      symbol: u8,
                      bit_count: u8| {
            for cell in range {
                table.entries[cell] = HuffmanDecodeEntry { symbol, bit_count };
            }
        };
        assign(&mut table, 0..1, 4, 4);
        assign(&mut table, 1..2, 5, 4);
        assign(&mut table, 2..4, 2, 3);
        assign(&mut table, 4..8, 1, 2);
        assign(&mut table, 8..16, 0, 1);

        table
    }

    #[test]
    fn decodes_one_stream_and_requires_exact_end() {
        let table = build_rfc_worked_example_table();

        let input = [0x30u8];
        let mut output = [0u8; 2];
        decode_one_stream(&input, &table, &mut output).unwrap();
        assert_eq!(output, [0, 4]);

        let input_with_extra_bit = [0x61u8];
        let mut output = [0u8; 2];
        assert_eq!(
            decode_one_stream(&input_with_extra_bit, &table, &mut output),
            Err(DecodeError::CorruptBitstream)
        );
    }

    #[test]
    fn decodes_four_streams_and_rejects_bad_jump_table() {
        let table = build_rfc_worked_example_table();

        let mut valid_input = vec![0x01, 0x00, 0x01, 0x00, 0x01, 0x00];
        valid_input.extend_from_slice(&[0x03, 0x03, 0x03, 0x03]);
        let mut output = [0u8; 4];
        decode_four_streams(&valid_input, &table, &mut output).unwrap();
        assert_eq!(output, [0, 0, 0, 0]);

        let mut jump_table_too_large = vec![0xFF, 0xFF, 0x01, 0x00, 0x01, 0x00];
        jump_table_too_large.extend_from_slice(&[0x03, 0x03, 0x03, 0x03]);
        let mut output = [0u8; 4];
        assert_eq!(
            decode_four_streams(&jump_table_too_large, &table, &mut output),
            Err(DecodeError::CorruptBitstream)
        );

        let mut output_length_one = [0u8; 1];
        assert_eq!(
            decode_four_streams(&valid_input, &table, &mut output_length_one),
            Err(DecodeError::CorruptBitstream)
        );

        let mut output_length_two = [0u8; 2];
        assert_eq!(
            decode_four_streams(&valid_input, &table, &mut output_length_two),
            Err(DecodeError::CorruptBitstream)
        );
    }

    #[test]
    fn decodes_four_streams_with_a_shorter_last_segment() {
        let table = build_rfc_worked_example_table();

        let mut valid_input = vec![0x01, 0x00, 0x01, 0x00, 0x01, 0x00];
        valid_input.extend_from_slice(&[0x07, 0x07, 0x07, 0x03]);
        let mut output = [0u8; 7];
        decode_four_streams(&valid_input, &table, &mut output).unwrap();
        assert_eq!(output, [0u8; 7]);
    }

    #[test]
    fn decodes_eight_streams_with_a_shorter_last_segment() {
        let table = build_rfc_worked_example_table();

        let mut valid_input = vec![
            0x01, 0x00, 0x01, 0x00, 0x01, 0x00, 0x01, 0x00, 0x01, 0x00, 0x01, 0x00, 0x01, 0x00,
        ];
        valid_input.extend_from_slice(&[0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x03]);
        let mut output = [0u8; 15];
        decode_eight_streams(&valid_input, &table, &mut output).unwrap();
        assert_eq!(output, [0u8; 15]);
    }

    #[test]
    fn returns_corrupt_bitstream_instead_of_panicking_when_a_stream_runs_out_of_bits() {
        let table = build_rfc_worked_example_table();

        let mut truncated_input = vec![0x01, 0x00, 0x01, 0x00, 0x01, 0x00];
        truncated_input.extend_from_slice(&[0x03, 0x07, 0x07, 0x03]);
        let mut output = [0u8; 7];
        assert_eq!(
            decode_four_streams(&truncated_input, &table, &mut output),
            Err(DecodeError::CorruptBitstream)
        );
    }

    fn split_streams(
        input: &[u8],
        stream_count: usize,
        regenerated_size: usize,
    ) -> ([&[u8]; 8], [usize; 8]) {
        let jump_table_size = (stream_count - 1) * 2;
        let mut stream_slices: [&[u8]; 8] = [&[]; 8];
        let mut segment_lengths: [usize; 8] = [0; 8];
        let segment_size = regenerated_size.div_ceil(stream_count);
        let mut remaining_input = &input[jump_table_size..];
        let mut remaining_output = regenerated_size;
        for stream_index in 0..stream_count {
            let is_last = stream_index == stream_count - 1;
            let stream = if is_last {
                remaining_input
            } else {
                let stream_size = read_little_endian_u16(input, stream_index * 2).unwrap() as usize;
                let stream = &remaining_input[..stream_size];
                remaining_input = &remaining_input[stream_size..];
                stream
            };
            let segment_length = if is_last {
                remaining_output
            } else {
                segment_size
            };
            remaining_output -= segment_length;
            stream_slices[stream_index] = stream;
            segment_lengths[stream_index] = segment_length;
        }
        (stream_slices, segment_lengths)
    }

    struct XorshiftRandom {
        state: u64,
    }

    impl XorshiftRandom {
        fn new(seed: u64) -> Self {
            Self { state: seed | 1 }
        }

        fn next_u64(&mut self) -> u64 {
            let mut value = self.state;
            value ^= value << 13;
            value ^= value >> 7;
            value ^= value << 17;
            self.state = value;
            value
        }

        fn next_range(&mut self, bound: usize) -> usize {
            (self.next_u64() % bound as u64) as usize
        }
    }

    #[test]
    fn checked_interleaved_oracle_matches_the_fast_path() {
        use crate::entropy::{
            huffman_encode::encode_many_streams,
            huffman_encode_table::{HuffmanEncodeTable, build_huffman_encode_table},
        };

        let mut random = XorshiftRandom::new(0x2545_F491_4F6C_DD1D);

        for case_index in 0..300 {
            let stream_count = if case_index % 2 == 0 { 4 } else { 8 };
            let symbol_count = 2 + random.next_range(40);
            let mut counts = [0u32; 256];
            for _ in 0..4000 {
                counts[random.next_range(symbol_count)] += 1;
            }
            let mut encode_table = HuffmanEncodeTable::new();
            if build_huffman_encode_table(&counts, &mut encode_table).is_err() {
                continue;
            }

            let usable_symbols: Vec<u8> = (0..=255u8)
                .filter(|&symbol| encode_table.codes[symbol as usize].bit_count > 0)
                .collect();
            let length = stream_count * (4 + random.next_range(500));
            let text: Vec<u8> = (0..length)
                .map(|_| usable_symbols[random.next_range(usable_symbols.len())])
                .collect();

            let mut weights_output = [0u8; 256];
            let bytes_written = match crate::entropy::huffman_encode_table::write_direct_weights(
                &mut weights_output,
                &encode_table,
            ) {
                Ok(bytes_written) => bytes_written,
                Err(_) => continue,
            };
            let mut decode_table = HuffmanDecodeTable::new();
            let mut weight_fse_table = crate::entropy::fse_decode_table::FseDecodeTable::new();
            if crate::entropy::huffman_decode_table::read_huffman_table(
                &weights_output[..bytes_written],
                &mut decode_table,
                &mut weight_fse_table,
            )
            .is_err()
            {
                continue;
            }

            let mut encoded = vec![0u8; length * 2 + 4096];
            let bytes_written =
                match encode_many_streams(&text, &encode_table, stream_count, &mut encoded) {
                    Ok(bytes_written) => bytes_written,
                    Err(_) => continue,
                };
            let encoded = &encoded[..bytes_written];

            let mut fast_output = vec![0u8; length];
            decode_many_streams(encoded, &decode_table, stream_count, &mut fast_output).unwrap();

            let (stream_slices, segment_lengths) = split_streams(encoded, stream_count, length);
            let mut checked_output = vec![0u8; length];
            decode_many_streams_checked(
                &mut checked_output,
                &decode_table,
                stream_count,
                stream_slices,
                segment_lengths,
            )
            .unwrap();

            assert_eq!(fast_output, checked_output, "case {case_index}");
        }
    }
}
