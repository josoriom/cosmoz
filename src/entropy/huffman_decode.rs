use crate::bits::backward_bit_reader::BackwardBitReader;
use crate::entropy::huffman_decode_table::HuffmanDecodeTable;
use crate::error::DecodeError;

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

pub fn decode_four_streams(
    input: &[u8],
    table: &HuffmanDecodeTable,
    output: &mut [u8],
) -> Result<(), DecodeError> {
    if input.len() < 10 {
        return Err(DecodeError::CorruptBitstream);
    }

    let first_stream_size = read_little_endian_u16(input, 0)? as usize;
    let second_stream_size = read_little_endian_u16(input, 2)? as usize;
    let third_stream_size = read_little_endian_u16(input, 4)? as usize;

    let after_jump_table = input.get(6..).ok_or(DecodeError::CorruptBitstream)?;

    let first_stream = after_jump_table
        .get(..first_stream_size)
        .ok_or(DecodeError::CorruptBitstream)?;
    let after_first_stream = after_jump_table
        .get(first_stream_size..)
        .ok_or(DecodeError::CorruptBitstream)?;

    let second_stream = after_first_stream
        .get(..second_stream_size)
        .ok_or(DecodeError::CorruptBitstream)?;
    let after_second_stream = after_first_stream
        .get(second_stream_size..)
        .ok_or(DecodeError::CorruptBitstream)?;

    let third_stream = after_second_stream
        .get(..third_stream_size)
        .ok_or(DecodeError::CorruptBitstream)?;
    let fourth_stream = after_second_stream
        .get(third_stream_size..)
        .ok_or(DecodeError::CorruptBitstream)?;

    if fourth_stream.is_empty() {
        return Err(DecodeError::CorruptBitstream);
    }

    let output_length = output.len();
    let segment_size = output_length
        .checked_add(3)
        .ok_or(DecodeError::CorruptBitstream)?
        / 4;
    let first_three_segments_size = segment_size
        .checked_mul(3)
        .ok_or(DecodeError::CorruptBitstream)?;
    if first_three_segments_size > output_length {
        return Err(DecodeError::CorruptBitstream);
    }

    let (first_segment, remaining_after_first) = output.split_at_mut(segment_size);
    let (second_segment, remaining_after_second) = remaining_after_first.split_at_mut(segment_size);
    let (third_segment, fourth_segment) = remaining_after_second.split_at_mut(segment_size);

    decode_one_stream(first_stream, table, first_segment)?;
    decode_one_stream(second_stream, table, second_segment)?;
    decode_one_stream(third_stream, table, third_segment)?;
    decode_one_stream(fourth_stream, table, fourth_segment)?;

    Ok(())
}

fn decode_symbols(
    reader: &mut BackwardBitReader<'_>,
    table: &HuffmanDecodeTable,
    output: &mut [u8],
) {
    let max_bits = table.max_bits as usize;
    for output_byte in output.iter_mut() {
        let cell = reader.peek_bits(max_bits) as usize;
        let entry = table.entries[cell];
        *output_byte = entry.symbol;
        reader.skip_bits(entry.bit_count as usize);
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
}
