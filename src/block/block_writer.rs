#[cfg(all(feature = "alloc", feature = "levels"))]
use alloc::vec::Vec;

#[cfg(all(feature = "alloc", feature = "levels"))]
use crate::block::block_splitter;
use crate::{
    block::{
        literals_writer::write_literals,
        sequence_record::SequenceRecord,
        sequence_writer::{SequenceEncodeTables, write_sequences},
    },
    encode_error::EncodeError,
    encoder::EncodeWorkspace,
    entropy::huffman_encode_table::HuffmanEncodeTable,
    frame::{block_header::BlockType, frame_header::FrameFormat, frame_writer::write_block_header},
    match_finder::MatchFinder,
};

#[cfg(all(feature = "alloc", feature = "levels"))]
const MIN_LEVEL_FOR_BLOCK_SPLITTING: u8 = 6;

pub fn write_block(
    input: &[u8],
    block_start: usize,
    format: FrameFormat,
    is_last: bool,
    output: &mut [u8],
    workspace: &mut EncodeWorkspace,
) -> Result<usize, EncodeError> {
    if block_start == 0 {
        workspace.huffman_table = HuffmanEncodeTable::new();
        workspace.sequence_tables = SequenceEncodeTables::new();
    }

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
    let saved_huffman_table = snapshot_huffman_table(&workspace.huffman_table);
    let saved_sequence_tables = workspace.sequence_tables.snapshot();

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

    let table_reuse_allowed = block_start != 0;

    #[cfg(all(feature = "alloc", feature = "levels"))]
    if workspace.level >= MIN_LEVEL_FOR_BLOCK_SPLITTING {
        let block_split = block_splitter::split_block(
            &workspace.sequences[..sequence_count],
            &workspace.literals[..literal_count],
            format,
        );

        if block_split.split_points.is_empty() {
            return commit_single_block(
                block_content,
                format,
                is_last,
                output,
                workspace,
                literal_count,
                sequence_count,
                table_reuse_allowed,
                saved_first_offset,
                saved_second_offset,
                saved_third_offset,
                saved_huffman_table,
                saved_sequence_tables,
                block_split.piece_literal_counts[0].as_ref(),
            );
        } else {
            let mut boundaries: Vec<usize> = Vec::with_capacity(block_split.split_points.len() + 2);
            boundaries.push(0);
            boundaries.extend_from_slice(&block_split.split_points);
            boundaries.push(sequence_count);

            match write_block_as_split_pieces(
                block_content,
                format,
                is_last,
                output,
                workspace,
                &boundaries,
                &block_split.piece_literal_counts,
                literal_count,
                table_reuse_allowed,
            ) {
                Ok(Some(split_total)) => {
                    return Ok(split_total);
                }
                Ok(_) => {
                    workspace.huffman_table = snapshot_huffman_table(&saved_huffman_table);
                    workspace.sequence_tables = saved_sequence_tables.snapshot();
                }
                Err(error) => return Err(error),
            }
        }
    }

    commit_single_block(
        block_content,
        format,
        is_last,
        output,
        workspace,
        literal_count,
        sequence_count,
        table_reuse_allowed,
        saved_first_offset,
        saved_second_offset,
        saved_third_offset,
        saved_huffman_table,
        saved_sequence_tables,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn commit_single_block(
    block_content: &[u8],
    format: FrameFormat,
    is_last: bool,
    output: &mut [u8],
    workspace: &mut EncodeWorkspace,
    literal_count: usize,
    sequence_count: usize,
    table_reuse_allowed: bool,
    saved_first_offset: u32,
    saved_second_offset: u32,
    saved_third_offset: u32,
    saved_huffman_table: HuffmanEncodeTable,
    saved_sequence_tables: SequenceEncodeTables,
    literal_counts: Option<&[u32; 256]>,
) -> Result<usize, EncodeError> {
    let compressed_body_length = write_compressed_block_body(
        &workspace.literals[..literal_count],
        &workspace.sequences[..sequence_count],
        format,
        &mut workspace.block_scratch,
        &mut workspace.huffman_table,
        &mut workspace.weight_fse_table,
        &mut workspace.sequence_tables,
        table_reuse_allowed,
        &mut workspace.entropy_scratch,
        literal_counts,
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
            workspace.huffman_table = saved_huffman_table;
            workspace.sequence_tables = saved_sequence_tables;
            write_raw_block(block_content, is_last, output)
        }
    }
}

#[cfg(all(feature = "alloc", feature = "levels"))]
#[allow(clippy::too_many_arguments)]
fn write_block_as_split_pieces(
    block_content: &[u8],
    format: FrameFormat,
    is_last: bool,
    output: &mut [u8],
    workspace: &mut EncodeWorkspace,
    boundaries: &[usize],
    piece_literal_counts: &[Option<[u32; 256]>],
    literal_count: usize,
    table_reuse_allowed: bool,
) -> Result<Option<usize>, EncodeError> {
    let sequence_count = match boundaries.last() {
        Some(sequence_count) => *sequence_count,
        None => return Err(EncodeError::TableNotUsable),
    };

    let mut literal_prefix = alloc::vec![0u32; sequence_count + 1];
    let mut byte_prefix = alloc::vec![0u32; sequence_count + 1];
    let mut literal_accumulator = 0u32;
    let mut byte_accumulator = 0u32;
    for (index, sequence) in workspace.sequences[..sequence_count].iter().enumerate() {
        literal_accumulator += sequence.literal_length;
        byte_accumulator += sequence.literal_length + sequence.match_length;
        literal_prefix[index + 1] = literal_accumulator;
        byte_prefix[index + 1] = byte_accumulator;
    }

    let mut output_position = 0usize;
    let mut content_position = 0usize;

    for (piece_index, window) in boundaries.windows(2).enumerate() {
        let sequence_start = window[0];
        let sequence_end = window[1];
        let is_last_piece = sequence_end == sequence_count;
        let is_last_block = is_last_piece && is_last;

        let piece_literal_start = literal_prefix[sequence_start] as usize;
        let piece_literal_end = if is_last_piece {
            literal_count
        } else {
            literal_prefix[sequence_end] as usize
        };
        let piece_byte_length = if is_last_piece {
            block_content.len() - content_position
        } else {
            (byte_prefix[sequence_end] - byte_prefix[sequence_start]) as usize
        };
        let piece_bytes = &block_content[content_position..content_position + piece_byte_length];
        let piece_table_reuse_allowed = if piece_index == 0 {
            table_reuse_allowed
        } else {
            true
        };

        let body_length = write_compressed_block_body(
            &workspace.literals[piece_literal_start..piece_literal_end],
            &workspace.sequences[sequence_start..sequence_end],
            format,
            &mut workspace.block_scratch,
            &mut workspace.huffman_table,
            &mut workspace.weight_fse_table,
            &mut workspace.sequence_tables,
            piece_table_reuse_allowed,
            &mut workspace.entropy_scratch,
            piece_literal_counts[piece_index].as_ref(),
        );

        let body_length = match body_length {
            Ok(length) if length < piece_bytes.len() => length,
            _ => return Ok(None),
        };

        let header_length = write_block_header(
            output
                .get_mut(output_position..)
                .ok_or(EncodeError::OutputTooSmall)?,
            BlockType::Compressed,
            body_length,
            is_last_block,
        )?;
        let total_piece_length = header_length
            .checked_add(body_length)
            .ok_or(EncodeError::OutputTooSmall)?;
        let destination = output
            .get_mut(output_position + header_length..output_position + total_piece_length)
            .ok_or(EncodeError::OutputTooSmall)?;
        destination.copy_from_slice(&workspace.block_scratch[..body_length]);

        output_position += total_piece_length;
        content_position += piece_byte_length;
    }

    Ok(Some(output_position))
}

fn snapshot_huffman_table(table: &HuffmanEncodeTable) -> HuffmanEncodeTable {
    HuffmanEncodeTable {
        codes: table.codes,
        weights: table.weights,
        symbol_count: table.symbol_count,
        max_bits: table.max_bits,
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
        if literal_end + 16 <= literals.len() && input_position + literal_length + 16 <= input.len()
        {
            unsafe {
                crate::simd::copy_bytes::copy_bytes_overshoot_unchecked(
                    input.as_ptr().add(input_position),
                    literals.as_mut_ptr().add(literal_position),
                    literal_length,
                );
            }
        } else {
            literals[literal_position..literal_end]
                .copy_from_slice(&input[input_position..input_position + literal_length]);
        }
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
    huffman_table: &mut HuffmanEncodeTable,
    weight_fse_table: &mut crate::entropy::fse_encode_table::FseEncodeTable,
    sequence_tables: &mut SequenceEncodeTables,
    table_reuse_allowed: bool,
    scratch: &mut [u8],
    literal_counts: Option<&[u32; 256]>,
) -> Result<usize, EncodeError> {
    let literals_length = write_literals(
        literals,
        format,
        output,
        huffman_table,
        weight_fse_table,
        table_reuse_allowed,
        literal_counts,
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
    use crate::{
        block::block_decoder::BlockWorkspace,
        decoder::decode_block_sequence,
        frame::block_header::{MAX_BLOCK_SIZE, read_block_header},
    };

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
            FrameFormat::Osmos,
            true,
            &mut output,
            &mut workspace,
        )
        .unwrap();
        assert!(written < input.len());
        let decoded = decode_one_block(&output[..written], input.len(), FrameFormat::Osmos);
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
            FrameFormat::Osmos,
            true,
            &mut output,
            &mut workspace,
        )
        .unwrap();
        let decoded = decode_one_block(&output[..written], input.len(), FrameFormat::Osmos);
        assert_eq!(decoded, input);
    }
}
