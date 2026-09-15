use crate::bits::backward_bit_writer::BackwardBitWriter;
use crate::encode_error::EncodeError;
use crate::entropy::huffman_encode_table::HuffmanEncodeTable;

const MAX_STREAM_COUNT: usize = 8;

pub fn encode_one_stream(
    input: &[u8],
    table: &HuffmanEncodeTable,
    output: &mut [u8],
) -> Result<usize, EncodeError> {
    let mut writer = BackwardBitWriter::new(output);
    for &symbol in input.iter().rev() {
        let code = table.codes[symbol as usize];
        writer.add_bits(code.code as u64, code.bit_count as usize)?;
    }
    writer.finish()
}

pub fn encode_many_streams(
    input: &[u8],
    table: &HuffmanEncodeTable,
    stream_count: usize,
    output: &mut [u8],
) -> Result<usize, EncodeError> {
    if stream_count == 0 || stream_count > MAX_STREAM_COUNT {
        return Err(EncodeError::BadOptions);
    }
    let segment_size = get_segment_size(input.len(), stream_count);
    let jump_table_size = (stream_count - 1) * 2;
    if output.len() < jump_table_size {
        return Err(EncodeError::OutputTooSmall);
    }
    let mut stream_sizes = [0u16; MAX_STREAM_COUNT];

    let mut position = jump_table_size;
    let mut input_position = 0usize;
    for (stream_index, stream_size_slot) in stream_sizes[..stream_count].iter_mut().enumerate() {
        let is_last_stream = stream_index == stream_count - 1;
        let segment_end = if is_last_stream {
            input.len()
        } else {
            (input_position + segment_size).min(input.len())
        };
        let segment = &input[input_position..segment_end];

        let stream_output = output
            .get_mut(position..)
            .ok_or(EncodeError::OutputTooSmall)?;
        let stream_size = encode_one_stream(segment, table, stream_output)?;
        if !is_last_stream {
            if stream_size > u16::MAX as usize {
                return Err(EncodeError::TableNotUsable);
            }
            *stream_size_slot = stream_size as u16;
        }
        position += stream_size;
        input_position = segment_end;
    }

    let jump_table = &mut output[..jump_table_size];
    for (stream_index, &size) in stream_sizes[..stream_count - 1].iter().enumerate() {
        let offset = stream_index * 2;
        jump_table[offset..offset + 2].copy_from_slice(&size.to_le_bytes());
    }

    Ok(position)
}

fn get_segment_size(input_length: usize, stream_count: usize) -> usize {
    input_length.div_ceil(stream_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::fse_decode_table::FseDecodeTable;
    use crate::entropy::histogram::count_symbols;
    use crate::entropy::huffman_decode::{decode_many_streams, decode_one_stream};
    use crate::entropy::huffman_decode_table::{HuffmanDecodeTable, read_huffman_table};
    use crate::entropy::huffman_encode_table::{build_huffman_encode_table, write_direct_weights};

    fn sample_text() -> Vec<u8> {
        let mut text = Vec::new();
        while text.len() < 300 {
            text.extend_from_slice(
                b"the quick brown fox jumps over the lazy dog while the sun sets slowly",
            );
        }
        text.truncate(300);
        text
    }

    fn build_encode_table(input: &[u8]) -> HuffmanEncodeTable {
        let mut counts = [0u32; 256];
        count_symbols(input, &mut counts);
        let mut table = HuffmanEncodeTable::new();
        build_huffman_encode_table(&counts, &mut table).unwrap();
        table
    }

    fn build_decode_table(encode_table: &HuffmanEncodeTable) -> HuffmanDecodeTable {
        let mut weights_output = [0u8; 256];
        let bytes_written = write_direct_weights(&mut weights_output, encode_table).unwrap();
        let mut decode_table = HuffmanDecodeTable::new();
        let mut weight_fse_table = FseDecodeTable::new();
        read_huffman_table(
            &weights_output[..bytes_written],
            &mut decode_table,
            &mut weight_fse_table,
        )
        .unwrap();
        decode_table
    }

    #[test]
    fn encodes_and_decodes_one_stream() {
        let text = sample_text();
        let encode_table = build_encode_table(&text);
        let decode_table = build_decode_table(&encode_table);

        let mut encoded = [0u8; 1024];
        let bytes_written = encode_one_stream(&text, &encode_table, &mut encoded).unwrap();

        let mut decoded = vec![0u8; text.len()];
        decode_one_stream(&encoded[..bytes_written], &decode_table, &mut decoded).unwrap();

        assert_eq!(decoded, text);
    }

    #[test]
    fn encodes_and_decodes_four_streams() {
        let text = sample_text();
        let encode_table = build_encode_table(&text);
        let decode_table = build_decode_table(&encode_table);

        let mut encoded = [0u8; 2048];
        let bytes_written = encode_many_streams(&text, &encode_table, 4, &mut encoded).unwrap();

        let mut decoded = vec![0u8; text.len()];
        decode_many_streams(&encoded[..bytes_written], &decode_table, 4, &mut decoded).unwrap();

        assert_eq!(decoded, text);
    }

    #[test]
    fn encodes_and_decodes_eight_streams() {
        let text = sample_text();
        let encode_table = build_encode_table(&text);
        let decode_table = build_decode_table(&encode_table);

        let mut encoded = [0u8; 2048];
        let bytes_written = encode_many_streams(&text, &encode_table, 8, &mut encoded).unwrap();

        let mut decoded = vec![0u8; text.len()];
        decode_many_streams(&encoded[..bytes_written], &decode_table, 8, &mut decoded).unwrap();

        assert_eq!(decoded, text);
    }
}
