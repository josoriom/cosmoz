use crate::{
    block::{
        block_decoder_fast::decode_sequences_fast_path_unchecked,
        literals::{LiteralSource, decode_literal_sections_pair_unchecked, decode_literals},
        repeat_offsets::RepeatOffsets,
        sequence_tables_fast::FastSequenceTables,
        sequences::{
            SequenceDecoder, SequenceTables, TableMode, read_sequence_tables, read_sequences_header,
        },
    },
    entropy::{fse_decode_table::FseDecodeTable, huffman_decode_table::HuffmanDecodeTable},
    error::DecodeError,
    frame::block_header::{BlockHeader, BlockType, MAX_BLOCK_SIZE},
    simd::copy_bytes::{copy_bytes, copy_match_overshoot_unchecked},
};

pub(crate) const FAST_PATH_SLACK: usize = 32;
const LITERALS_BUFFER_LENGTH: usize = MAX_BLOCK_SIZE + FAST_PATH_SLACK;

/// The fast path copies literals in whole 16-byte vectors, so it may read up to 15 bytes
/// past the last literal it needs.
const LITERALS_READ_SLACK: usize = 16;

/// Raw literals are borrowed from the block payload, which is itself borrowed from the
/// caller's input buffer, so nothing guarantees the fast path's overshoot stays inside a
/// live allocation. Only hand them to it when the payload itself has the slack to spare.
/// Decoded literals live in `BlockWorkspace::literals`, which is oversized by
/// `FAST_PATH_SLACK`, so they never need this check.
fn raw_literals_have_read_slack(payload_length: usize, literals_end: usize) -> bool {
    payload_length - literals_end >= LITERALS_READ_SLACK
}

pub(crate) struct BlockWorkspace {
    pub literals: [u8; LITERALS_BUFFER_LENGTH],
    pub huffman_tables: [HuffmanDecodeTable; 2],
    pub current_huffman_table: usize,
    pub weight_fse_table: FseDecodeTable,
    pub sequence_tables: SequenceTables,
    pub fast_sequence_tables: FastSequenceTables,
    pub repeat_offsets: RepeatOffsets,
}

impl BlockWorkspace {
    pub(crate) const fn new() -> Self {
        Self {
            literals: [0u8; LITERALS_BUFFER_LENGTH],
            huffman_tables: [HuffmanDecodeTable::new(), HuffmanDecodeTable::new()],
            current_huffman_table: 0,
            weight_fse_table: FseDecodeTable::new(),
            sequence_tables: SequenceTables::new(),
            fast_sequence_tables: FastSequenceTables::new(),
            repeat_offsets: RepeatOffsets {
                first: 1,
                second: 4,
                third: 8,
            },
        }
    }

    pub(crate) fn reset_history(&mut self) {
        self.huffman_tables[0].is_ready = false;
        self.huffman_tables[1].is_ready = false;
        self.current_huffman_table = 0;
        self.sequence_tables.literal_length_ready = false;
        self.sequence_tables.offset_ready = false;
        self.sequence_tables.match_length_ready = false;
        self.fast_sequence_tables.literal_length_dirty = true;
        self.fast_sequence_tables.offset_dirty = true;
        self.fast_sequence_tables.match_length_dirty = true;
        self.repeat_offsets = RepeatOffsets::new();
    }
}

impl Default for BlockWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn decode_block(
    input: &[u8],
    header: &BlockHeader,
    output: &mut [u8],
    output_position: usize,
    workspace: &mut BlockWorkspace,
) -> Result<usize, DecodeError> {
    match header.block_type {
        BlockType::Raw => decode_raw_block(input, output, output_position),
        BlockType::Rle => decode_rle_block(input, header.block_size, output, output_position),
        BlockType::Compressed => decode_compressed_block(input, output, output_position, workspace),
    }
}

fn decode_raw_block(
    input: &[u8],
    output: &mut [u8],
    output_position: usize,
) -> Result<usize, DecodeError> {
    let new_position = output_position
        .checked_add(input.len())
        .ok_or(DecodeError::OutputTooSmall)?;
    let destination = output
        .get_mut(output_position..new_position)
        .ok_or(DecodeError::OutputTooSmall)?;
    destination.copy_from_slice(input);
    Ok(new_position)
}

fn decode_rle_block(
    input: &[u8],
    size: usize,
    output: &mut [u8],
    output_position: usize,
) -> Result<usize, DecodeError> {
    let repeated_byte = *input.first().ok_or(DecodeError::InputTooShort)?;
    let new_position = output_position
        .checked_add(size)
        .ok_or(DecodeError::OutputTooSmall)?;
    let destination = output
        .get_mut(output_position..new_position)
        .ok_or(DecodeError::OutputTooSmall)?;
    destination.fill(repeated_byte);
    Ok(new_position)
}

fn decode_compressed_block(
    input: &[u8],
    output: &mut [u8],
    output_position: usize,
    workspace: &mut BlockWorkspace,
) -> Result<usize, DecodeError> {
    let (literal_source, literals_bytes_used) = decode_literals(
        input,
        &mut workspace.huffman_tables[workspace.current_huffman_table & 1],
        &mut workspace.weight_fse_table,
        &mut workspace.literals,
    )?;
    let mut sequence_state = SequenceState {
        sequence_tables: &mut workspace.sequence_tables,
        fast_sequence_tables: &mut workspace.fast_sequence_tables,
        repeat_offsets: &mut workspace.repeat_offsets,
    };
    decode_sequences_section(
        input,
        literals_bytes_used,
        literal_source,
        output,
        output_position,
        &mut sequence_state,
    )
}

const PAIRED_LITERALS_DISTANCE: usize = 2 * MAX_BLOCK_SIZE + 2 * FAST_PATH_SLACK;
const PAIRED_OUTPUT_ROOM: usize = PAIRED_LITERALS_DISTANCE + MAX_BLOCK_SIZE + LITERALS_READ_SLACK;

pub(crate) fn decode_compressed_block_pair(
    first_input: &[u8],
    second_input: &[u8],
    output: &mut [u8],
    output_position: usize,
    workspace: &mut BlockWorkspace,
) -> Result<Option<usize>, DecodeError> {
    if output.len() < output_position + PAIRED_OUTPUT_ROOM {
        return Ok(None);
    }
    let second_literals_position = output_position + PAIRED_LITERALS_DISTANCE;
    let second_literals = unsafe { output.as_mut_ptr().add(second_literals_position) };
    let Some(pair) = (unsafe {
        decode_literal_sections_pair_unchecked(
            first_input,
            second_input,
            &mut workspace.huffman_tables,
            &mut workspace.current_huffman_table,
            &mut workspace.weight_fse_table,
            &mut workspace.literals,
            second_literals,
        )?
    }) else {
        return Ok(None);
    };

    let mut sequence_state = SequenceState {
        sequence_tables: &mut workspace.sequence_tables,
        fast_sequence_tables: &mut workspace.fast_sequence_tables,
        repeat_offsets: &mut workspace.repeat_offsets,
    };
    let middle_position = decode_sequences_section(
        first_input,
        pair.first_bytes_used,
        LiteralSource::Decoded(&workspace.literals[..pair.first_count]),
        output,
        output_position,
        &mut sequence_state,
    )?;

    let end_position = decode_sequences_section(
        second_input,
        pair.second_bytes_used,
        LiteralSource::InOutput {
            start: second_literals_position,
            count: pair.second_count,
        },
        output,
        middle_position,
        &mut sequence_state,
    )?;
    Ok(Some(end_position))
}

struct SequenceState<'workspace> {
    sequence_tables: &'workspace mut SequenceTables,
    fast_sequence_tables: &'workspace mut FastSequenceTables,
    repeat_offsets: &'workspace mut RepeatOffsets,
}

fn decode_sequences_section(
    input: &[u8],
    literals_bytes_used: usize,
    literal_source: LiteralSource,
    output: &mut [u8],
    output_position: usize,
    state: &mut SequenceState,
) -> Result<usize, DecodeError> {
    let literal_count = literal_source.len();
    let sequences_input = input
        .get(literals_bytes_used..)
        .ok_or(DecodeError::InputTooShort)?;
    let sequences_header = read_sequences_header(sequences_input)?;
    if sequences_header.sequence_count == 0 {
        if sequences_input.len() != sequences_header.header_length {
            return Err(DecodeError::CorruptBitstream);
        }
        return copy_all_literals(&literal_source, output, output_position);
    }
    let table_input = sequences_input
        .get(sequences_header.header_length..)
        .ok_or(DecodeError::InputTooShort)?;
    let table_bytes_used =
        read_sequence_tables(table_input, &sequences_header, state.sequence_tables)?;
    let bitstream = table_input
        .get(table_bytes_used..)
        .ok_or(DecodeError::InputTooShort)?;

    if sequences_header.literal_length_mode != TableMode::Repeat {
        state.fast_sequence_tables.literal_length_dirty = true;
    }
    if sequences_header.offset_mode != TableMode::Repeat {
        state.fast_sequence_tables.offset_dirty = true;
    }
    if sequences_header.match_length_mode != TableMode::Repeat {
        state.fast_sequence_tables.match_length_dirty = true;
    }
    state.fast_sequence_tables.build_all(state.sequence_tables);

    let mut position = output_position;
    let mut literal_cursor = 0usize;

    if bitstream.len() >= 8 {
        let literals_bytes = match literal_source {
            LiteralSource::Raw(bytes) => {
                if raw_literals_have_read_slack(input.len(), literals_bytes_used) {
                    Some(bytes.as_ptr())
                } else {
                    None
                }
            }
            LiteralSource::Decoded(bytes) => Some(bytes.as_ptr()),
            LiteralSource::InOutput { start, .. } => {
                Some(unsafe { output.as_mut_ptr().add(start) } as *const u8)
            }
            LiteralSource::Rle { .. } => None,
        };
        if let Some(literals_bytes) = literals_bytes {
            return unsafe {
                decode_sequences_fast_path_unchecked(
                    bitstream,
                    state.fast_sequence_tables,
                    sequences_header.sequence_count,
                    literals_bytes,
                    literal_count,
                    output,
                    output_position,
                    state.repeat_offsets,
                )
            };
        }
    }

    let mut decoder = SequenceDecoder::new(
        bitstream,
        state.sequence_tables,
        sequences_header.sequence_count,
    )?;

    while let Some(sequence) = decoder.next_sequence(state.repeat_offsets) {
        let sequence = sequence?;

        let literal_length = sequence.literal_length as usize;
        position = take_literals(
            &literal_source,
            &mut literal_cursor,
            literal_length,
            output,
            position,
        )?;

        position = copy_match(
            output,
            position,
            sequence.offset as usize,
            sequence.match_length as usize,
        )?;
        if position - output_position > MAX_BLOCK_SIZE {
            return Err(DecodeError::BlockTooLarge);
        }
    }

    if !decoder.is_finished() {
        return Err(DecodeError::CorruptBitstream);
    }

    if literal_cursor < literal_count {
        let remaining_length = literal_count - literal_cursor;
        position = take_literals(
            &literal_source,
            &mut literal_cursor,
            remaining_length,
            output,
            position,
        )?;
        if position - output_position > MAX_BLOCK_SIZE {
            return Err(DecodeError::BlockTooLarge);
        }
    }

    Ok(position)
}

fn copy_all_literals(
    literal_source: &LiteralSource,
    output: &mut [u8],
    output_position: usize,
) -> Result<usize, DecodeError> {
    let mut literal_cursor = 0usize;
    let position = take_literals(
        literal_source,
        &mut literal_cursor,
        literal_source.len(),
        output,
        output_position,
    )?;
    if position - output_position > MAX_BLOCK_SIZE {
        return Err(DecodeError::BlockTooLarge);
    }
    Ok(position)
}

fn take_literals(
    source: &LiteralSource,
    cursor: &mut usize,
    length: usize,
    output: &mut [u8],
    output_position: usize,
) -> Result<usize, DecodeError> {
    match source {
        LiteralSource::Raw(bytes) | LiteralSource::Decoded(bytes) => {
            let end = cursor
                .checked_add(length)
                .ok_or(DecodeError::CorruptBitstream)?;
            let slice = bytes
                .get(*cursor..end)
                .ok_or(DecodeError::CorruptBitstream)?;
            let new_position = copy_literals(slice, output, output_position)?;
            *cursor = end;
            Ok(new_position)
        }
        LiteralSource::InOutput { start, count } => {
            let end = cursor
                .checked_add(length)
                .ok_or(DecodeError::CorruptBitstream)?;
            if end > *count {
                return Err(DecodeError::CorruptBitstream);
            }
            let new_position = output_position
                .checked_add(length)
                .ok_or(DecodeError::OutputTooSmall)?;
            if new_position > output.len() {
                return Err(DecodeError::OutputTooSmall);
            }
            output.copy_within(start + *cursor..start + end, output_position);
            *cursor = end;
            Ok(new_position)
        }
        LiteralSource::Rle { byte, count } => {
            let end = cursor
                .checked_add(length)
                .ok_or(DecodeError::CorruptBitstream)?;
            if end > *count {
                return Err(DecodeError::CorruptBitstream);
            }
            let new_position = output_position
                .checked_add(length)
                .ok_or(DecodeError::OutputTooSmall)?;
            let destination = output
                .get_mut(output_position..new_position)
                .ok_or(DecodeError::OutputTooSmall)?;
            destination.fill(*byte);
            *cursor = end;
            Ok(new_position)
        }
    }
}

fn copy_literals(
    literals: &[u8],
    output: &mut [u8],
    output_position: usize,
) -> Result<usize, DecodeError> {
    let new_position = output_position
        .checked_add(literals.len())
        .ok_or(DecodeError::OutputTooSmall)?;
    let destination = output
        .get_mut(output_position..new_position)
        .ok_or(DecodeError::OutputTooSmall)?;
    copy_bytes(literals, destination);
    Ok(new_position)
}

pub(crate) fn copy_match(
    output: &mut [u8],
    output_position: usize,
    offset: usize,
    length: usize,
) -> Result<usize, DecodeError> {
    if offset == 0 || offset > output_position {
        return Err(DecodeError::BadOffset);
    }
    let new_position = output_position
        .checked_add(length)
        .ok_or(DecodeError::OutputTooSmall)?;
    if new_position > output.len() {
        return Err(DecodeError::OutputTooSmall);
    }

    let source_start = output_position - offset;

    if output.len() - new_position >= FAST_PATH_SLACK {
        unsafe {
            copy_match_overshoot_unchecked(
                output.as_mut_ptr().add(output_position),
                offset,
                length,
            );
        }
        return Ok(new_position);
    }

    if offset >= length {
        output.copy_within(source_start..source_start + length, output_position);
        return Ok(new_position);
    }

    output.copy_within(source_start..output_position, output_position);
    let mut written = offset;
    while written < length {
        let copy_size = written.min(length - written);
        output.copy_within(
            output_position..output_position + copy_size,
            output_position + written,
        );
        written += copy_size;
    }

    Ok(new_position)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{block_header::read_block_header, frame_header::read_frame_header};

    #[test]
    fn copy_match_fast_path_matches_byte_by_byte_reference() {
        let total_len = 4096usize;
        for length in 1..=300usize {
            for offset in 1..=40usize {
                for near_end in [false, true] {
                    let output_position = if near_end {
                        total_len - length - FAST_PATH_SLACK
                    } else {
                        total_len / 2
                    };

                    let mut seed = 0x9e37_79b9u32 ^ (length as u32) ^ ((offset as u32) << 16);
                    let mut reference = std::vec![0u8; total_len];
                    for byte in reference[..output_position].iter_mut() {
                        seed ^= seed << 13;
                        seed ^= seed >> 17;
                        seed ^= seed << 5;
                        *byte = (seed & 0xff) as u8;
                    }
                    let mut fast = reference.clone();

                    let source_start = output_position - offset;
                    let mut position = output_position;
                    let mut written = 0usize;
                    while written < length {
                        reference[position] = reference[source_start + written];
                        position += 1;
                        written += 1;
                    }

                    let new_position =
                        copy_match(&mut fast, output_position, offset, length).unwrap();

                    assert_eq!(new_position, output_position + length);
                    assert_eq!(
                        reference[..output_position + length],
                        fast[..output_position + length],
                        "length {} offset {} near_end {}",
                        length,
                        offset,
                        near_end
                    );
                }
            }
        }
    }

    #[test]
    fn copy_match_offset_one_repeats_the_last_byte() {
        let mut output = [0u8; 8];
        output[0] = b'a';
        let new_position = copy_match(&mut output, 1, 1, 5).unwrap();
        assert_eq!(new_position, 6);
        assert_eq!(&output[..6], b"aaaaaa");
    }

    #[test]
    fn copy_match_offset_less_than_length_repeats_the_pattern() {
        let mut output = [0u8; 16];
        output[..3].copy_from_slice(b"abc");
        let new_position = copy_match(&mut output, 3, 3, 8).unwrap();
        assert_eq!(new_position, 11);
        assert_eq!(&output[..11], b"abcabcabcab");
    }

    #[test]
    fn copy_match_offset_at_least_length_copies_in_one_step() {
        let mut output = [0u8; 8];
        output[..4].copy_from_slice(b"wxyz");
        let new_position = copy_match(&mut output, 4, 4, 2).unwrap();
        assert_eq!(new_position, 6);
        assert_eq!(&output[..6], b"wxyzwx");
    }

    #[test]
    fn copy_match_rejects_offset_larger_than_output_position() {
        let mut output = [0u8; 8];
        assert_eq!(
            copy_match(&mut output, 2, 3, 4),
            Err(DecodeError::BadOffset)
        );
    }

    #[test]
    fn copy_match_rejects_offset_zero() {
        let mut output = [0u8; 8];
        assert_eq!(
            copy_match(&mut output, 4, 0, 4),
            Err(DecodeError::BadOffset)
        );
    }

    #[test]
    fn decode_block_copies_a_raw_block() {
        let header = BlockHeader {
            block_type: BlockType::Raw,
            block_size: 4,
            is_last: true,
        };
        let mut workspace = Box::new(BlockWorkspace::new());
        let mut output = [0u8; 8];
        let new_position = decode_block(b"abcd", &header, &mut output, 0, &mut workspace).unwrap();
        assert_eq!(new_position, 4);
        assert_eq!(&output[..4], b"abcd");
    }

    #[test]
    fn decode_block_repeats_a_byte_for_an_rle_block() {
        let header = BlockHeader {
            block_type: BlockType::Rle,
            block_size: 5,
            is_last: true,
        };
        let mut workspace = Box::new(BlockWorkspace::new());
        let mut output = [0u8; 8];
        let new_position = decode_block(&[0x42], &header, &mut output, 0, &mut workspace).unwrap();
        assert_eq!(new_position, 5);
        assert_eq!(&output[..5], &[0x42; 5]);
    }

    #[test]
    fn decodes_a_real_zstd_1_compressed_block_with_sequences() {
        const FRAME: [u8; 64] = [
            0x28, 0xb5, 0x2f, 0xfd, 0x60, 0x0e, 0x00, 0xb5, 0x01, 0x00, 0xd4, 0x02, 0x54, 0x68,
            0x65, 0x20, 0x71, 0x75, 0x69, 0x63, 0x6b, 0x20, 0x62, 0x72, 0x6f, 0x77, 0x6e, 0x20,
            0x66, 0x6f, 0x78, 0x20, 0x6a, 0x75, 0x6d, 0x70, 0x73, 0x20, 0x6f, 0x76, 0x65, 0x72,
            0x20, 0x74, 0x68, 0x65, 0x20, 0x6c, 0x61, 0x7a, 0x79, 0x20, 0x64, 0x6f, 0x67, 0x2e,
            0x20, 0x01, 0x00, 0xf5, 0x42, 0x4a, 0x95, 0x01,
        ];
        let original_text = "The quick brown fox jumps over the lazy dog. \
The quick brown fox jumps over the lazy dog. \
The quick brown fox jumps over the lazy dog. \
The quick brown fox jumps over the lazy dog. \
The quick brown fox jumps over the lazy dog. \
The quick brown fox jumps over the lazy dog. "
            .as_bytes();
        let original_text = &original_text[..270];

        let frame_header = read_frame_header(&FRAME).unwrap();
        let block_header = read_block_header(&FRAME[frame_header.header_length..]).unwrap();
        let block_payload_start = frame_header.header_length + 3;
        let block_payload =
            &FRAME[block_payload_start..block_payload_start + block_header.block_size];

        let mut workspace = Box::new(BlockWorkspace::new());
        let mut output = [0u8; 512];
        let new_position =
            decode_block(block_payload, &block_header, &mut output, 0, &mut workspace).unwrap();

        assert_eq!(new_position, original_text.len());
        assert_eq!(&output[..new_position], original_text);
    }

    #[test]
    fn decodes_a_real_zstd_19_compressed_block_with_huffman_and_fse_sequences() {
        const FRAME: [u8; 505] = [
            0x28, 0xb5, 0x2f, 0xfd, 0x60, 0x00, 0x07, 0x7d, 0x0f, 0x00, 0x22, 0x48, 0x17, 0x11,
            0xb0, 0x3d, 0xb0, 0xad, 0x14, 0xb2, 0x88, 0x0e, 0xad, 0x29, 0x25, 0x3b, 0x49, 0x52,
            0x93, 0xa9, 0x08, 0x25, 0x3c, 0xab, 0x2e, 0xff, 0x64, 0x75, 0x5a, 0x6e, 0xde, 0x6b,
            0xc7, 0x94, 0x87, 0x0e, 0xab, 0x61, 0xeb, 0x97, 0xd8, 0xf5, 0x9b, 0x55, 0xe5, 0x8d,
            0x73, 0x92, 0xd8, 0x75, 0xf4, 0x12, 0x9e, 0x55, 0x27, 0xc0, 0xbf, 0xae, 0x85, 0x87,
            0x5e, 0x5a, 0x39, 0xa1, 0x60, 0x60, 0xdb, 0xef, 0x37, 0x86, 0xef, 0x62, 0x35, 0x06,
            0x2c, 0xbb, 0xa4, 0x3b, 0xb0, 0x94, 0xb9, 0xfb, 0xd5, 0xfb, 0x98, 0x82, 0x87, 0xb8,
            0x58, 0xf1, 0xaf, 0x6e, 0x97, 0xdf, 0xe9, 0x23, 0x80, 0xe9, 0xa8, 0x71, 0x27, 0x49,
            0x0a, 0x0a, 0x92, 0x69, 0x37, 0x10, 0x10, 0x62, 0x94, 0xb2, 0x33, 0x0f, 0x11, 0x20,
            0x5c, 0x22, 0xb5, 0x00, 0x45, 0x6c, 0xd2, 0x18, 0x25, 0xa0, 0x02, 0x02, 0x5a, 0x50,
            0x84, 0x34, 0x9c, 0xfc, 0x13, 0xe7, 0xbb, 0xb8, 0x32, 0x0a, 0x8a, 0xfb, 0x1f, 0x13,
            0xd3, 0x39, 0x24, 0x89, 0xcb, 0x1c, 0x4d, 0x7f, 0x81, 0x22, 0x59, 0x62, 0x31, 0x00,
            0x30, 0xd9, 0xbe, 0xd7, 0xc3, 0x7b, 0x00, 0x6e, 0xbc, 0x3b, 0xc2, 0xb4, 0xa1, 0x26,
            0xb0, 0x57, 0x40, 0x13, 0xf8, 0x40, 0xcb, 0xc2, 0xad, 0xbd, 0x98, 0xa7, 0xb2, 0x7a,
            0x28, 0x2f, 0x8a, 0x25, 0x34, 0x23, 0xa6, 0x0e, 0x21, 0x5f, 0xf9, 0x8a, 0x5a, 0xbd,
            0xf8, 0xa0, 0xac, 0x86, 0x5e, 0x6e, 0x47, 0xe9, 0x01, 0x02, 0xce, 0x54, 0x00, 0x93,
            0x2c, 0xab, 0x7f, 0xde, 0x23, 0x1a, 0x05, 0x66, 0xb5, 0x04, 0xb5, 0xc7, 0xed, 0x81,
            0xaa, 0x3c, 0x11, 0xcf, 0x9e, 0xfa, 0x5d, 0x8f, 0x94, 0x02, 0xc2, 0xd5, 0xcd, 0x66,
            0xbb, 0x52, 0x33, 0x15, 0x19, 0x1c, 0x75, 0xc0, 0xb5, 0x55, 0x05, 0x3e, 0xeb, 0x88,
            0x3b, 0x73, 0xce, 0xa6, 0x63, 0xf3, 0x5e, 0x77, 0x52, 0x3a, 0x79, 0x38, 0x7d, 0xf7,
            0x9a, 0x73, 0x2f, 0x0d, 0x95, 0xa0, 0xc4, 0x19, 0x3f, 0xbf, 0x4a, 0x1c, 0x70, 0xa6,
            0x20, 0xe0, 0x80, 0x4e, 0x35, 0x1f, 0x51, 0xe3, 0x03, 0xc1, 0x91, 0x16, 0xb3, 0x8e,
            0xa4, 0x75, 0x53, 0x2e, 0x37, 0x60, 0x50, 0x73, 0xc0, 0x28, 0xa3, 0x89, 0x4f, 0xd4,
            0x46, 0x72, 0xcf, 0xb4, 0x42, 0x33, 0x59, 0xdc, 0xcf, 0x19, 0x1a, 0xd5, 0x93, 0xc5,
            0x3a, 0x65, 0x1a, 0x72, 0xa0, 0xa7, 0x89, 0xc0, 0xb8, 0xf0, 0xf1, 0xb2, 0x51, 0xe1,
            0x7b, 0x85, 0x36, 0xb8, 0x65, 0x5a, 0x60, 0x4e, 0xcc, 0x2d, 0x4e, 0x7e, 0xde, 0xdc,
            0x56, 0xac, 0x99, 0xa8, 0x26, 0x04, 0x99, 0xec, 0x9b, 0x5f, 0xca, 0xcf, 0x71, 0x96,
            0x56, 0x71, 0x84, 0xb3, 0x84, 0x65, 0xc4, 0x19, 0x2a, 0x8c, 0x85, 0x8c, 0x62, 0x67,
            0x5e, 0x2c, 0x96, 0x10, 0xe4, 0xc0, 0x46, 0x3a, 0x62, 0x43, 0xf4, 0xad, 0xfd, 0x0f,
            0x40, 0xf6, 0x5c, 0xff, 0x72, 0x7d, 0xa2, 0x07, 0x07, 0xc5, 0x6a, 0x90, 0x86, 0xf2,
            0x8a, 0x86, 0xcb, 0x30, 0x07, 0x4e, 0x54, 0x22, 0x0b, 0x5e, 0xc0, 0xb1, 0x6e, 0x85,
            0x42, 0x5e, 0xd7, 0x4c, 0xfe, 0x34, 0xec, 0x32, 0x9e, 0x2f, 0xc9, 0xd5, 0xf2, 0x80,
            0xe5, 0x35, 0x32, 0x44, 0x74, 0x91, 0xf8, 0xc4, 0xc4, 0xcb, 0x31, 0x83, 0x07, 0x59,
            0xcb, 0x73, 0xb4, 0x18, 0xcd, 0x2f, 0xff, 0xd0, 0xb6, 0x5f, 0x67, 0xe2, 0xa7, 0xd9,
            0x84, 0x49, 0x95, 0xab, 0x0d, 0xa9, 0xac, 0x71, 0xeb, 0x5b, 0xd1, 0x20, 0x8d, 0x5d,
            0xf2, 0x4b, 0xfa, 0x3f, 0xc0, 0x09, 0x94, 0x2b, 0x80, 0x50, 0xf9, 0x34, 0x91, 0x01,
            0x35,
        ];
        const EXPECTED_TEXT: &[u8] = b"fox the zstd dog dog jumps fox entropy brown huffman offset quick the brown lazy dog decoder table the entropy lazy entropy offset dog match huffman zstd the over offset algorithm zstd jumps lazy algorithm fox brown literal fox sequence sequence table zstd quick match entropy fox literal brown entropy compression table sequence huffman lazy brown quick dog compression brown dog fox literal zstd match sequence over sequence sequence lazy zstd brown table over entropy dog over match literal zstd entropy dog algorithm quick dog quick algorithm literal zstd brown lazy huffman algorithm lazy length literal match jumps zstd jumps dog entropy entropy zstd huffman offset huffman literal sequence dog jumps decoder length brown quick fox jumps over offset table brown literal literal table match decoder zstd entropy the fox entropy zstd algorithm fox compression offset over match the zstd decoder over decoder fox compression decoder table lazy jumps sequence over entropy decoder the table algorithm length the fox sequence compression dog quick dog huffman brown brown length brown entropy jumps jumps length entropy over zstd decoder table offset lazy entropy lazy compression literal sequence match decoder match fox dog dog brown algorithm the huffman entropy dog huffman dog the brown quick dog brown quick algorithm brown decoder dog zstd length lazy entropy jumps huffman huffman length dog length offset lazy fox fox offset sequence offset offset match quick fox quick literal algorithm fox dog lazy lazy entropy match jumps offset over zstd match dog brown match entropy fox quick entropy the brown dog over offset length length lazy literal quick over literal the literal zstd match compression offset entropy length jumps lazy compression lazy quick huffman entropy quick algorithm quick quick huffman length decoder decoder over quick decoder brown over brown table brown dog literal fox huffman dog huffman table quick table brown offset huffman huffman decoder algorithm zstd lazy algorithm dog zstd literal jumps compression match";

        let frame_header = read_frame_header(&FRAME).unwrap();
        let block_header = read_block_header(&FRAME[frame_header.header_length..]).unwrap();
        let block_payload_start = frame_header.header_length + 3;
        let block_payload =
            &FRAME[block_payload_start..block_payload_start + block_header.block_size];

        let mut workspace = Box::new(BlockWorkspace::new());
        let mut output = [0u8; 4096];
        let new_position =
            decode_block(block_payload, &block_header, &mut output, 0, &mut workspace).unwrap();

        assert_eq!(new_position, EXPECTED_TEXT.len());
        assert_eq!(&output[..new_position], EXPECTED_TEXT);
    }

    fn round_trip_through_decode_block(input: &[u8]) -> std::vec::Vec<u8> {
        use crate::{
            decoder::{DecodeWorkspace, decompress},
            encoder::{CompressOptions, EncodeWorkspace, compress, get_max_compressed_size},
        };

        let options = CompressOptions::zstd();
        let mut encode_workspace = EncodeWorkspace::new_boxed();
        let mut compressed = std::vec![0u8; get_max_compressed_size(input.len(), &options)];
        let compressed_length =
            compress(input, &mut compressed, &options, &mut encode_workspace).unwrap();

        let mut decode_workspace = Box::new(DecodeWorkspace::new());
        let mut decoded = std::vec![0u8; input.len()];
        let decoded_length = decompress(
            &compressed[..compressed_length],
            &mut decoded,
            &mut decode_workspace,
        )
        .unwrap();

        assert_eq!(decoded_length, input.len());
        decoded
    }

    fn xorshift_bytes(count: usize, seed: u32) -> std::vec::Vec<u8> {
        let mut state = seed;
        let mut buffer = std::vec![0u8; count];
        for slot in buffer.iter_mut() {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *slot = (state & 0xff) as u8;
        }
        buffer
    }

    #[test]
    fn round_trips_random_bytes_that_decode_through_a_raw_literal_source() {
        let input = xorshift_bytes(8192, 0x1357_2468);
        let decoded = round_trip_through_decode_block(&input);
        assert_eq!(decoded, input);
    }

    #[test]
    fn round_trips_one_repeated_byte_that_decodes_through_an_rle_source() {
        let input = std::vec![0x37u8; 8192];
        let decoded = round_trip_through_decode_block(&input);
        assert_eq!(decoded, input);
    }

    #[test]
    fn raw_literal_fast_path_requires_sixteen_bytes_of_read_slack() {
        assert!(!raw_literals_have_read_slack(100, 100));
        assert!(!raw_literals_have_read_slack(100, 85));
        assert!(raw_literals_have_read_slack(100, 84));
        assert!(raw_literals_have_read_slack(100, 0));
    }

    /// Raw literals borrow from the caller's input buffer, and the fast path copies them
    /// with a 16-byte overshoot. These shapes put only ten bytes of sequences data behind
    /// the literals, so the fast path must decline them rather than read past the end.
    /// Run under AddressSanitizer to see a regression here as a heap-buffer-overflow.
    #[test]
    fn round_trips_raw_literal_blocks_that_end_close_to_the_payload_end() {
        use crate::{
            decoder::{DecodeWorkspace, decompress},
            encoder::{CompressOptions, EncodeWorkspace, compress, get_max_compressed_size},
        };

        let options = CompressOptions::zstd();
        let mut encode_workspace = EncodeWorkspace::new_boxed();
        let mut decode_workspace = DecodeWorkspace::new_boxed();
        let mut state = 7u32;

        for trial in 0..4000usize {
            let length = 100 + trial % 3000;
            let mut input = xorshift_bytes(length, state.max(1));
            state = state
                .wrapping_mul(2654435761)
                .wrapping_add(trial as u32)
                .max(1);
            for _ in 0..(1 + trial % 3) {
                let take = 8 + trial % 40;
                let repeat = input[..take].to_vec();
                input.extend_from_slice(&repeat);
            }

            let mut compressed = std::vec![0u8; get_max_compressed_size(input.len(), &options)];
            let compressed_length =
                compress(&input, &mut compressed, &options, &mut encode_workspace).unwrap();
            let frame = compressed[..compressed_length].to_vec();

            // Padded so the decoder takes its unchecked fast path.
            let mut decoded = std::vec![0u8; MAX_BLOCK_SIZE + FAST_PATH_SLACK + input.len()];
            let decoded_length = decompress(&frame, &mut decoded, &mut decode_workspace).unwrap();
            assert_eq!(decoded_length, input.len(), "trial {trial}");
            assert_eq!(&decoded[..decoded_length], &input[..], "trial {trial}");
        }
    }

    #[test]
    fn round_trips_into_exact_size_output() {
        use crate::{
            decoder::{DecodeWorkspace, decompress},
            encoder::{CompressOptions, EncodeWorkspace, compress, get_max_compressed_size},
        };

        let mut encode_workspace = EncodeWorkspace::new_boxed();
        let mut decode_workspace = DecodeWorkspace::new_boxed();
        let options = CompressOptions::zstd();
        for length in (1..400usize).chain([4096, 70_000, 200_000]) {
            let mut input = xorshift_bytes(length, length as u32);
            for index in 0..length {
                if index % 7 != 0 && index >= 1 + length % 5 {
                    input[index] = input[index - 1 - length % 5];
                }
            }
            let mut compressed = std::vec![0u8; get_max_compressed_size(length, &options)];
            let compressed_length =
                compress(&input, &mut compressed, &options, &mut encode_workspace).unwrap();
            let mut decoded = std::vec![0u8; length];
            let decoded_length = decompress(
                &compressed[..compressed_length],
                &mut decoded,
                &mut decode_workspace,
            )
            .unwrap();
            assert_eq!(decoded_length, length);
            assert_eq!(decoded, input, "length {length}");
        }
    }

    #[test]
    fn round_trips_text_that_decodes_through_a_decoded_huffman_source() {
        let source = b"the quick brown fox jumps over the lazy dog while the sun sets slowly \
behind the distant hills and the wind carries the scent of rain across the quiet valley, ";
        let mut input = std::vec::Vec::with_capacity(8192);
        while input.len() < 8192 {
            input.extend_from_slice(source);
        }
        input.truncate(8192);

        let decoded = round_trip_through_decode_block(&input);
        assert_eq!(decoded, input);
    }
}
