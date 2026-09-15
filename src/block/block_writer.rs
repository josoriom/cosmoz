use crate::block::literals_writer::write_literals;
use crate::block::sequence_record::SequenceRecord;
use crate::block::sequence_writer::write_sequences;
use crate::encode_error::EncodeError;
use crate::encoder::EncodeWorkspace;
use crate::frame::block_header::BlockType;
use crate::frame::frame_header::FrameFormat;
use crate::frame::frame_writer::write_block_header;
use crate::match_finder::MatchFinder;

pub fn write_block(
    input: &[u8],
    block_start: usize,
    format: FrameFormat,
    is_last: bool,
    output: &mut [u8],
    workspace: &mut EncodeWorkspace,
) -> Result<usize, EncodeError> {
    let block_content = &input[block_start..];

    if block_content.is_empty() {
        return write_raw_block(block_content, is_last, output);
    }

    let first_byte = block_content[0];
    if block_content.iter().all(|&byte| byte == first_byte) {
        return write_rle_block(first_byte, block_content.len(), is_last, output);
    }

    let saved_first_offset = workspace.repeat_offsets.first;
    let saved_second_offset = workspace.repeat_offsets.second;
    let saved_third_offset = workspace.repeat_offsets.third;

    let (sequence_count, tail_literal_count) = workspace.match_finder.find_sequences(
        input,
        block_start,
        &mut workspace.sequences,
        &mut workspace.repeat_offsets,
    );

    let literal_count = collect_literals(
        block_content,
        &workspace.sequences[..sequence_count],
        tail_literal_count,
        &mut workspace.literals,
    );

    let compressed_body_length = write_compressed_block_body(
        &workspace.literals[..literal_count],
        &workspace.sequences[..sequence_count],
        format,
        &mut workspace.block_scratch,
        &mut workspace.huffman_table,
        &mut workspace.weight_fse_table,
        &mut workspace.sequence_tables,
        &mut workspace.entropy_scratch,
    );

    match compressed_body_length {
        Ok(body_length) if body_length < block_content.len() => {
            let header_length =
                write_block_header(output, BlockType::Compressed, body_length, is_last)?;
            let total_length = header_length
                .checked_add(body_length)
                .ok_or(EncodeError::OutputTooSmall)?;
            let destination = output
                .get_mut(header_length..total_length)
                .ok_or(EncodeError::OutputTooSmall)?;
            destination.copy_from_slice(&workspace.block_scratch[..body_length]);
            Ok(total_length)
        }
        _ => {
            workspace.repeat_offsets.first = saved_first_offset;
            workspace.repeat_offsets.second = saved_second_offset;
            workspace.repeat_offsets.third = saved_third_offset;
            write_raw_block(block_content, is_last, output)
        }
    }
}

fn write_raw_block(input: &[u8], is_last: bool, output: &mut [u8]) -> Result<usize, EncodeError> {
    let header_length = write_block_header(output, BlockType::Raw, input.len(), is_last)?;
    let total_length = header_length
        .checked_add(input.len())
        .ok_or(EncodeError::OutputTooSmall)?;
    let destination = output
        .get_mut(header_length..total_length)
        .ok_or(EncodeError::OutputTooSmall)?;
    destination.copy_from_slice(input);
    Ok(total_length)
}

fn write_rle_block(
    byte: u8,
    length: usize,
    is_last: bool,
    output: &mut [u8],
) -> Result<usize, EncodeError> {
    let header_length = write_block_header(output, BlockType::Rle, length, is_last)?;
    let total_length = header_length
        .checked_add(1)
        .ok_or(EncodeError::OutputTooSmall)?;
    let destination = output
        .get_mut(header_length..total_length)
        .ok_or(EncodeError::OutputTooSmall)?;
    destination[0] = byte;
    Ok(total_length)
}

fn collect_literals(
    input: &[u8],
    sequences: &[SequenceRecord],
    tail_literal_count: usize,
    literals: &mut [u8],
) -> usize {
    let mut input_position = 0usize;
    let mut literal_position = 0usize;

    for sequence in sequences {
        let literal_length = sequence.literal_length as usize;
        let literal_end = literal_position + literal_length;
        literals[literal_position..literal_end]
            .copy_from_slice(&input[input_position..input_position + literal_length]);
        literal_position = literal_end;
        input_position += literal_length + sequence.match_length as usize;
    }

    let literal_end = literal_position + tail_literal_count;
    literals[literal_position..literal_end]
        .copy_from_slice(&input[input_position..input_position + tail_literal_count]);
    literal_end
}

#[allow(clippy::too_many_arguments)]
fn write_compressed_block_body(
    literals: &[u8],
    sequences: &[SequenceRecord],
    format: FrameFormat,
    output: &mut [u8],
    huffman_table: &mut crate::entropy::huffman_encode_table::HuffmanEncodeTable,
    weight_fse_table: &mut crate::entropy::fse_encode_table::FseEncodeTable,
    sequence_tables: &mut crate::block::sequence_writer::SequenceEncodeTables,
    scratch: &mut [u8],
) -> Result<usize, EncodeError> {
    let literals_length = write_literals(
        literals,
        format,
        output,
        huffman_table,
        weight_fse_table,
        scratch,
    )?;
    let sequences_output = output
        .get_mut(literals_length..)
        .ok_or(EncodeError::OutputTooSmall)?;
    let sequences_length = write_sequences(
        sequences,
        format,
        sequences_output,
        sequence_tables,
        scratch,
    )?;
    Ok(literals_length + sequences_length)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::block_decoder::BlockWorkspace;
    use crate::decoder::decode_block_sequence;
    use crate::frame::block_header::{MAX_BLOCK_SIZE, read_block_header};

    fn repeating_text(length: usize) -> Vec<u8> {
        let source = b"the quick brown fox jumps over the lazy dog while the sun sets slowly \
behind the distant hills and the wind carries the scent of rain across the quiet valley";
        let mut text = Vec::with_capacity(length);
        while text.len() < length {
            let remaining = length - text.len();
            let take = remaining.min(source.len());
            text.extend_from_slice(&source[..take]);
        }
        text
    }

    fn decode_one_block(payload: &[u8], expected_length: usize, format: FrameFormat) -> Vec<u8> {
        let mut workspace = Box::new(BlockWorkspace::new());
        workspace.reset_history(format);
        let mut output = vec![0u8; expected_length];
        let (_, written) = decode_block_sequence(payload, &mut output, &mut workspace).unwrap();
        output.truncate(written);
        output
    }

    #[test]
    fn writes_and_decodes_a_raw_block_for_empty_input() {
        let mut workspace = EncodeWorkspace::new_boxed();
        let mut output = [0u8; 16];
        let written =
            write_block(&[], 0, FrameFormat::Zstd, true, &mut output, &mut workspace).unwrap();
        assert_eq!(written, 3);
        let decoded = decode_one_block(&output[..written], 0, FrameFormat::Zstd);
        assert_eq!(decoded, Vec::<u8>::new());
    }

    #[test]
    fn writes_and_decodes_an_rle_block() {
        let input = vec![0x42u8; 5000];
        let mut workspace = EncodeWorkspace::new_boxed();
        let mut output = [0u8; 16];
        let written = write_block(
            &input,
            0,
            FrameFormat::Zstd,
            true,
            &mut output,
            &mut workspace,
        )
        .unwrap();
        assert_eq!(written, 4);
        let decoded = decode_one_block(&output[..written], input.len(), FrameFormat::Zstd);
        assert_eq!(decoded, input);
    }

    #[test]
    fn writes_and_decodes_a_compressed_block_smaller_than_raw() {
        let input = repeating_text(20 * 1024);
        let mut workspace = EncodeWorkspace::new_boxed();
        let mut output = vec![0u8; input.len() * 2 + 4096];
        let written = write_block(
            &input,
            0,
            FrameFormat::Osmo,
            true,
            &mut output,
            &mut workspace,
        )
        .unwrap();
        assert!(written < input.len());
        let decoded = decode_one_block(&output[..written], input.len(), FrameFormat::Osmo);
        assert_eq!(decoded, input);
    }

    #[test]
    fn falls_back_to_raw_when_random_input_does_not_compress() {
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut input = vec![0u8; 4096];
        for byte in input.iter_mut() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *byte = (state & 0xFF) as u8;
        }
        let mut workspace = EncodeWorkspace::new_boxed();
        let mut output = vec![0u8; input.len() * 2 + 4096];
        let written = write_block(
            &input,
            0,
            FrameFormat::Zstd,
            true,
            &mut output,
            &mut workspace,
        )
        .unwrap();
        let header = read_block_header(&output[..written]).unwrap();
        assert_eq!(
            header.block_type,
            crate::frame::block_header::BlockType::Raw
        );
        let decoded = decode_one_block(&output[..written], input.len(), FrameFormat::Zstd);
        assert_eq!(decoded, input);
    }

    #[test]
    fn respects_max_block_size() {
        let input = repeating_text(MAX_BLOCK_SIZE);
        let mut workspace = EncodeWorkspace::new_boxed();
        let mut output = vec![0u8; input.len() * 2 + 4096];
        let written = write_block(
            &input,
            0,
            FrameFormat::Osmo,
            true,
            &mut output,
            &mut workspace,
        )
        .unwrap();
        let decoded = decode_one_block(&output[..written], input.len(), FrameFormat::Osmo);
        assert_eq!(decoded, input);
    }
}
