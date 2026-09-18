use crate::block::repeat_offsets::RepeatOffsets;
#[cfg(feature = "checksum")]
use crate::hash::{xxhash3::XxHash3, xxhash64::XxHash64};
use crate::{
    block::block_decoder::{BlockWorkspace, decode_block, decode_compressed_block_pair},
    error::DecodeError,
    frame::{
        block_header::{BLOCK_HEADER_LENGTH, BlockHeader, BlockType, read_block_header},
        chunk_index::ChunkIndex,
        frame_header::{
            FrameFormat, FrameHeader, get_skippable_frame_length, is_skippable_frame,
            read_frame_header,
        },
    },
};

pub(crate) struct DecodeWorkspace {
    pub block: BlockWorkspace,
}

impl DecodeWorkspace {
    pub(crate) const fn new() -> Self {
        Self {
            block: BlockWorkspace::new(),
        }
    }
}

impl Default for DecodeWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

impl DecodeWorkspace {
    pub(crate) fn new_boxed() -> alloc::boxed::Box<Self> {
        let layout = core::alloc::Layout::new::<Self>();
        unsafe {
            let raw = alloc::alloc::alloc_zeroed(layout) as *mut Self;
            if raw.is_null() {
                alloc::alloc::handle_alloc_error(layout);
            }
            write_initial_values_unchecked(raw);
            alloc::boxed::Box::from_raw(raw)
        }
    }
}

unsafe fn write_initial_values_unchecked(target: *mut DecodeWorkspace) {
    unsafe {
        let block = core::ptr::addr_of_mut!((*target).block);
        core::ptr::write(
            core::ptr::addr_of_mut!((*block).repeat_offsets),
            RepeatOffsets {
                first: 1,
                second: 4,
                third: 8,
            },
        );
        core::ptr::write(
            core::ptr::addr_of_mut!((*block).frame_format),
            FrameFormat::Zstd,
        );
        let fast_sequence_tables = core::ptr::addr_of_mut!((*block).fast_sequence_tables);
        core::ptr::write(
            core::ptr::addr_of_mut!((*fast_sequence_tables).literal_length_dirty),
            true,
        );
        core::ptr::write(
            core::ptr::addr_of_mut!((*fast_sequence_tables).offset_dirty),
            true,
        );
        core::ptr::write(
            core::ptr::addr_of_mut!((*fast_sequence_tables).match_length_dirty),
            true,
        );
    }
}

#[cfg(test)]
mod new_boxed_tests {
    use super::*;

    #[test]
    fn new_boxed_matches_new_field_by_field() {
        let stack_workspace = DecodeWorkspace::new();
        let boxed_workspace = DecodeWorkspace::new_boxed();

        assert_eq!(
            boxed_workspace.block.repeat_offsets.first,
            stack_workspace.block.repeat_offsets.first
        );
        assert_eq!(
            boxed_workspace.block.repeat_offsets.second,
            stack_workspace.block.repeat_offsets.second
        );
        assert_eq!(
            boxed_workspace.block.repeat_offsets.third,
            stack_workspace.block.repeat_offsets.third
        );
        assert_eq!(
            boxed_workspace.block.frame_format,
            stack_workspace.block.frame_format
        );
        assert_eq!(
            boxed_workspace.block.huffman_tables[0].is_ready,
            stack_workspace.block.huffman_tables[0].is_ready
        );
        assert_eq!(
            boxed_workspace.block.sequence_tables.literal_length_ready,
            stack_workspace.block.sequence_tables.literal_length_ready
        );
        assert_eq!(
            boxed_workspace.block.sequence_tables.offset_ready,
            stack_workspace.block.sequence_tables.offset_ready
        );
        assert_eq!(
            boxed_workspace.block.sequence_tables.match_length_ready,
            stack_workspace.block.sequence_tables.match_length_ready
        );
    }
}

#[cfg(feature = "checksum")]
#[allow(clippy::large_enum_variant)]
enum ContentHasher {
    Zstd(XxHash64),
    Cosmoz(XxHash3),
}

#[cfg(feature = "checksum")]
impl ContentHasher {
    fn new(format: FrameFormat) -> Self {
        match format {
            FrameFormat::Zstd => ContentHasher::Zstd(XxHash64::new(0)),
            FrameFormat::Cosmoz => ContentHasher::Cosmoz(XxHash3::new()),
        }
    }

    fn update(&mut self, input: &[u8]) {
        match self {
            ContentHasher::Zstd(hasher) => hasher.update(input),
            ContentHasher::Cosmoz(hasher) => hasher.update(input),
        }
    }

    fn matches_checksum(&self, expected_bytes: &[u8]) -> bool {
        match self {
            ContentHasher::Zstd(hasher) => {
                let low_32_bits = (hasher.finish() & 0xFFFF_FFFF) as u32;
                expected_bytes == low_32_bits.to_le_bytes()
            }
            ContentHasher::Cosmoz(hasher) => expected_bytes == hasher.finish().to_le_bytes(),
        }
    }
}

fn block_payload_length(block_type: BlockType, block_size: usize) -> usize {
    match block_type {
        BlockType::Rle => 1,
        BlockType::Raw | BlockType::Compressed => block_size,
    }
}

pub(crate) fn decode_block_sequence(
    input: &[u8],
    output: &mut [u8],
    workspace: &mut BlockWorkspace,
) -> Result<(usize, usize), DecodeError> {
    let mut input_position = 0usize;
    let mut output_position = 0usize;

    loop {
        let block_header_input = input
            .get(input_position..)
            .ok_or(DecodeError::InputTooShort)?;
        let block_header = read_block_header(block_header_input)?;
        input_position += BLOCK_HEADER_LENGTH;

        let payload_length = block_payload_length(block_header.block_type, block_header.block_size);
        let payload = input
            .get(input_position..input_position + payload_length)
            .ok_or(DecodeError::InputTooShort)?;
        input_position += payload_length;

        if block_header.block_type == BlockType::Compressed
            && !block_header.is_last
            && let Some((next_header, next_payload)) =
                read_next_compressed_block(input, input_position)?
            && let Some(position) = decode_compressed_block_pair(
                payload,
                next_payload,
                output,
                output_position,
                workspace,
            )?
        {
            output_position = position;
            input_position += BLOCK_HEADER_LENGTH + next_header.block_size;
            if next_header.is_last {
                break;
            }
            continue;
        }

        output_position = decode_block(payload, &block_header, output, output_position, workspace)?;

        if block_header.is_last {
            break;
        }
    }

    Ok((input_position, output_position))
}

fn read_next_compressed_block(
    input: &[u8],
    input_position: usize,
) -> Result<Option<(BlockHeader, &[u8])>, DecodeError> {
    let header_input = input
        .get(input_position..)
        .ok_or(DecodeError::InputTooShort)?;
    let header = read_block_header(header_input)?;
    if header.block_type != BlockType::Compressed {
        return Ok(None);
    }
    let payload_start = input_position + BLOCK_HEADER_LENGTH;
    Ok(input
        .get(payload_start..payload_start + header.block_size)
        .map(|payload| (header, payload)))
}

fn skip_frame_body(input: &[u8], header: &FrameHeader) -> Result<usize, DecodeError> {
    match header.format {
        FrameFormat::Zstd => skip_zstd_frame_body(input, header),
        FrameFormat::Cosmoz => skip_cosmoz_frame_body(input, header),
    }
}

fn skip_zstd_frame_body(input: &[u8], header: &FrameHeader) -> Result<usize, DecodeError> {
    let mut position = header.header_length;

    loop {
        let block_header_input = input.get(position..).ok_or(DecodeError::InputTooShort)?;
        let block_header = read_block_header(block_header_input)?;
        let payload_length = block_payload_length(block_header.block_type, block_header.block_size);
        position += BLOCK_HEADER_LENGTH;
        if input.len() < position + payload_length {
            return Err(DecodeError::InputTooShort);
        }
        position += payload_length;
        if block_header.is_last {
            break;
        }
    }

    let checksum_length = header.checksum_length();
    if input.len() < position + checksum_length {
        return Err(DecodeError::InputTooShort);
    }
    position += checksum_length;

    Ok(position)
}

fn skip_cosmoz_frame_body(input: &[u8], header: &FrameHeader) -> Result<usize, DecodeError> {
    let index_input = input
        .get(header.header_length..)
        .ok_or(DecodeError::InputTooShort)?;
    let index = ChunkIndex::read(index_input)?;
    let total_compressed_length = index.total_compressed_length()?;

    let mut position = header
        .header_length
        .checked_add(index.index_length())
        .ok_or(DecodeError::BadFrameHeader)?;
    position = position
        .checked_add(total_compressed_length)
        .ok_or(DecodeError::BadFrameHeader)?;

    let checksum_length = header.checksum_length();
    if input.len() < position + checksum_length {
        return Err(DecodeError::InputTooShort);
    }
    position += checksum_length;

    Ok(position)
}

pub(crate) fn get_frame_compressed_size(input: &[u8]) -> Result<usize, DecodeError> {
    if is_skippable_frame(input) {
        return get_skippable_frame_length(input);
    }
    let header = read_frame_header(input)?;
    skip_frame_body(input, &header)
}

pub(crate) fn get_decompressed_size(input: &[u8]) -> Result<Option<u64>, DecodeError> {
    let mut position = 0usize;
    let mut total_size = 0u64;

    while position < input.len() {
        let remaining = &input[position..];

        if is_skippable_frame(remaining) {
            position += get_skippable_frame_length(remaining)?;
            continue;
        }

        let header = read_frame_header(remaining)?;
        let content_size = match header.content_size {
            Some(size) => size,
            None => return Ok(None),
        };
        total_size = total_size
            .checked_add(content_size)
            .ok_or(DecodeError::BadFrameHeader)?;

        position += skip_frame_body(remaining, &header)?;
    }

    Ok(Some(total_size))
}

#[cfg(any(test, all(target_arch = "wasm32", feature = "wasm-exports")))]
pub(crate) fn decompress(
    input: &[u8],
    output: &mut [u8],
    workspace: &mut DecodeWorkspace,
) -> Result<usize, DecodeError> {
    decompress_checked(input, output, workspace, true)
}

pub(crate) fn decompress_checked(
    input: &[u8],
    output: &mut [u8],
    workspace: &mut DecodeWorkspace,
    verify_checksum: bool,
) -> Result<usize, DecodeError> {
    let mut input_position = 0usize;
    let mut output_position = 0usize;

    while input_position < input.len() {
        let remaining_input = &input[input_position..];

        if is_skippable_frame(remaining_input) {
            input_position += get_skippable_frame_length(remaining_input)?;
            continue;
        }

        let (bytes_consumed, new_output_position) =
            decode_frame(remaining_input, output, output_position, workspace, verify_checksum)?;
        input_position += bytes_consumed;
        output_position = new_output_position;
    }

    Ok(output_position)
}

#[cfg_attr(not(feature = "checksum"), allow(unused_variables))]
fn decode_frame(
    input: &[u8],
    output: &mut [u8],
    output_position: usize,
    workspace: &mut DecodeWorkspace,
    verify_checksum: bool,
) -> Result<(usize, usize), DecodeError> {
    let header = read_frame_header(input)?;
    let frame_start_output_position = output_position;

    let (mut input_position, position) = match header.format {
        FrameFormat::Zstd => decode_zstd_frame_body(
            input,
            &header,
            output,
            frame_start_output_position,
            workspace,
        )?,
        FrameFormat::Cosmoz => decode_cosmoz_frame_body(
            input,
            &header,
            output,
            frame_start_output_position,
            workspace,
        )?,
    };

    #[cfg(feature = "checksum")]
    {
        let checksum_length = header.checksum_length();
        if header.has_checksum && verify_checksum {
            let expected_bytes = input
                .get(input_position..input_position + checksum_length)
                .ok_or(DecodeError::InputTooShort)?;
            let mut hasher = ContentHasher::new(header.format);
            hasher.update(&output[frame_start_output_position..position]);
            check_content_checksum(expected_bytes, &hasher)?;
        }
        input_position += checksum_length;
    }
    #[cfg(not(feature = "checksum"))]
    {
        let checksum_length = header.checksum_length();
        if input.len() < input_position + checksum_length {
            return Err(DecodeError::InputTooShort);
        }
        input_position += checksum_length;
    }

    if let Some(content_size) = header.content_size {
        let bytes_written = (position - frame_start_output_position) as u64;
        if bytes_written != content_size {
            return Err(DecodeError::BadFrameHeader);
        }
    }

    Ok((input_position, position))
}

fn decode_zstd_frame_body(
    input: &[u8],
    header: &FrameHeader,
    output: &mut [u8],
    frame_start_output_position: usize,
    workspace: &mut DecodeWorkspace,
) -> Result<(usize, usize), DecodeError> {
    workspace.block.reset_history(header.format);
    let frame_output = output
        .get_mut(frame_start_output_position..)
        .ok_or(DecodeError::OutputTooSmall)?;
    let (bytes_consumed, bytes_written) = decode_block_sequence(
        &input[header.header_length..],
        frame_output,
        &mut workspace.block,
    )?;
    Ok((
        header.header_length + bytes_consumed,
        frame_start_output_position + bytes_written,
    ))
}

fn decode_cosmoz_frame_body(
    input: &[u8],
    header: &FrameHeader,
    output: &mut [u8],
    frame_start_output_position: usize,
    workspace: &mut DecodeWorkspace,
) -> Result<(usize, usize), DecodeError> {
    let content_size = header.content_size.ok_or(DecodeError::BadFrameHeader)?;

    let index_input = input
        .get(header.header_length..)
        .ok_or(DecodeError::InputTooShort)?;
    let index = ChunkIndex::read(index_input)?;

    if index.total_decompressed_length()? != content_size {
        return Err(DecodeError::BadFrameHeader);
    }

    let total_compressed_length = index.total_compressed_length()?;
    let chunks_input_start = header
        .header_length
        .checked_add(index.index_length())
        .ok_or(DecodeError::BadFrameHeader)?;
    let chunks_input_end = chunks_input_start
        .checked_add(total_compressed_length)
        .ok_or(DecodeError::BadFrameHeader)?;
    let chunks_input = input
        .get(chunks_input_start..chunks_input_end)
        .ok_or(DecodeError::InputTooShort)?;

    let content_size_as_usize =
        usize::try_from(content_size).map_err(|_| DecodeError::OutputTooSmall)?;
    let output_end = frame_start_output_position
        .checked_add(content_size_as_usize)
        .ok_or(DecodeError::OutputTooSmall)?;
    let output_region = output
        .get_mut(frame_start_output_position..output_end)
        .ok_or(DecodeError::OutputTooSmall)?;

    decode_cosmoz_chunks(chunks_input, &index, output_region, workspace)?;

    Ok((chunks_input_end, output_end))
}

fn decode_cosmoz_chunks(
    chunks_input: &[u8],
    index: &ChunkIndex,
    output: &mut [u8],
    workspace: &mut DecodeWorkspace,
) -> Result<(), DecodeError> {
    #[cfg(feature = "parallel")]
    {
        if index.chunk_count >= 2 {
            return crate::parallel_decoder::decode_chunks_in_parallel(chunks_input, index, output);
        }
    }
    decode_cosmoz_chunks_sequentially(chunks_input, index, output, &mut workspace.block)
}

fn decode_cosmoz_chunks_sequentially(
    chunks_input: &[u8],
    index: &ChunkIndex,
    output: &mut [u8],
    block_workspace: &mut BlockWorkspace,
) -> Result<(), DecodeError> {
    let mut input_offset = 0usize;
    let mut output_offset = 0usize;

    for chunk_number in 0..index.chunk_count {
        let entry = index.get_entry(chunk_number);
        let chunk_input = chunks_input
            .get(input_offset..input_offset + entry.compressed_length)
            .ok_or(DecodeError::InputTooShort)?;
        let chunk_output = output
            .get_mut(output_offset..output_offset + entry.decompressed_length)
            .ok_or(DecodeError::OutputTooSmall)?;

        block_workspace.reset_history(FrameFormat::Cosmoz);
        let (bytes_consumed, bytes_written) =
            decode_block_sequence(chunk_input, chunk_output, block_workspace)?;
        if bytes_consumed != chunk_input.len() || bytes_written != entry.decompressed_length {
            return Err(DecodeError::BadFrameHeader);
        }

        input_offset += entry.compressed_length;
        output_offset += entry.decompressed_length;
    }

    Ok(())
}

#[cfg(feature = "checksum")]
fn check_content_checksum(
    expected_bytes: &[u8],
    hasher: &ContentHasher,
) -> Result<(), DecodeError> {
    if hasher.matches_checksum(expected_bytes) {
        Ok(())
    } else {
        Err(DecodeError::ChecksumMismatch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::xxhash3;

    const TEXT_500_WITH_CHECKSUM: [u8; 68] = [
        0x28, 0xb5, 0x2f, 0xfd, 0x64, 0xf4, 0x00, 0xb5, 0x01, 0x00, 0xd4, 0x02, 0x54, 0x68, 0x65,
        0x20, 0x71, 0x75, 0x69, 0x63, 0x6b, 0x20, 0x62, 0x72, 0x6f, 0x77, 0x6e, 0x20, 0x66, 0x6f,
        0x78, 0x20, 0x6a, 0x75, 0x6d, 0x70, 0x73, 0x20, 0x6f, 0x76, 0x65, 0x72, 0x20, 0x74, 0x68,
        0x65, 0x20, 0x6c, 0x61, 0x7a, 0x79, 0x20, 0x64, 0x6f, 0x67, 0x2e, 0x20, 0x01, 0x00, 0x25,
        0x86, 0xaa, 0x2a, 0x03, 0x3a, 0x83, 0xf3, 0x96,
    ];

    const TEXT_500_NO_CHECKSUM: [u8; 64] = [
        0x28, 0xb5, 0x2f, 0xfd, 0x60, 0xf4, 0x00, 0xb5, 0x01, 0x00, 0xd4, 0x02, 0x54, 0x68, 0x65,
        0x20, 0x71, 0x75, 0x69, 0x63, 0x6b, 0x20, 0x62, 0x72, 0x6f, 0x77, 0x6e, 0x20, 0x66, 0x6f,
        0x78, 0x20, 0x6a, 0x75, 0x6d, 0x70, 0x73, 0x20, 0x6f, 0x76, 0x65, 0x72, 0x20, 0x74, 0x68,
        0x65, 0x20, 0x6c, 0x61, 0x7a, 0x79, 0x20, 0x64, 0x6f, 0x67, 0x2e, 0x20, 0x01, 0x00, 0x25,
        0x86, 0xaa, 0x2a, 0x03,
    ];

    fn expected_text() -> Vec<u8> {
        let mut text = Vec::new();
        while text.len() < 500 {
            text.extend_from_slice(b"The quick brown fox jumps over the lazy dog. ");
        }
        text.truncate(500);
        text
    }

    #[test]
    fn decompresses_a_real_frame_with_checksum() {
        let mut workspace = DecodeWorkspace::new_boxed();
        let mut output = [0u8; 1024];
        let written = decompress(&TEXT_500_WITH_CHECKSUM, &mut output, &mut workspace).unwrap();
        assert_eq!(written, 500);
        assert_eq!(&output[..written], expected_text().as_slice());
        assert_eq!(
            get_decompressed_size(&TEXT_500_WITH_CHECKSUM).unwrap(),
            Some(500)
        );
    }

    #[test]
    fn rejects_flipped_checksum_byte() {
        let mut input = TEXT_500_WITH_CHECKSUM;
        let last_index = input.len() - 1;
        input[last_index] ^= 0xFF;
        let mut workspace = DecodeWorkspace::new_boxed();
        let mut output = [0u8; 1024];
        assert_eq!(
            decompress(&input, &mut output, &mut workspace),
            Err(DecodeError::ChecksumMismatch)
        );
    }

    #[test]
    fn rejects_flipped_payload_byte_without_panicking() {
        let mut input = TEXT_500_WITH_CHECKSUM;
        input[20] ^= 0xFF;
        let mut workspace = DecodeWorkspace::new_boxed();
        let mut output = [0u8; 1024];
        assert!(decompress(&input, &mut output, &mut workspace).is_err());
    }

    #[test]
    fn rejects_a_truncated_fse_table_description_without_panicking() {
        // A compressed block whose literal length table is declared FSE but carries a single
        // byte of description. Reading past that byte reported fifteen bytes consumed, and
        // the sequences reader sliced the table input with it.
        let frame = [
            0x28, 0xB5, 0x2F, 0xFD, 0x00, 0x50, // frame header, 1 MiB window, no content size
            0x25, 0x00, 0x00, // last block, compressed, four bytes of payload
            0x00, // raw literals, length zero
            0x01, // one sequence
            0x80, // literal lengths in FSE mode
            0x00, // the truncated table description
        ];
        let mut workspace = DecodeWorkspace::new_boxed();
        let mut output = [0u8; 64];
        assert_eq!(
            decompress(&frame, &mut output, &mut workspace),
            Err(DecodeError::InputTooShort)
        );
    }

    #[test]
    fn decompresses_a_frame_without_checksum() {
        let mut workspace = DecodeWorkspace::new_boxed();
        let mut output = [0u8; 1024];
        let written = decompress(&TEXT_500_NO_CHECKSUM, &mut output, &mut workspace).unwrap();
        assert_eq!(written, 500);
        assert_eq!(&output[..written], expected_text().as_slice());
    }

    #[test]
    fn skips_a_skippable_frame_before_a_real_frame() {
        let mut input = Vec::new();
        input.extend_from_slice(&[0x50, 0x2A, 0x4D, 0x18]);
        input.extend_from_slice(&[3, 0, 0, 0]);
        input.extend_from_slice(&[1, 2, 3]);
        input.extend_from_slice(&TEXT_500_WITH_CHECKSUM);

        let mut workspace = DecodeWorkspace::new_boxed();
        let mut output = [0u8; 1024];
        let written = decompress(&input, &mut output, &mut workspace).unwrap();
        assert_eq!(written, 500);
        assert_eq!(&output[..written], expected_text().as_slice());
    }

    #[test]
    fn decompresses_two_frames_back_to_back() {
        let mut input = Vec::new();
        input.extend_from_slice(&TEXT_500_WITH_CHECKSUM);
        input.extend_from_slice(&TEXT_500_WITH_CHECKSUM);

        let mut workspace = DecodeWorkspace::new_boxed();
        let mut output = [0u8; 2048];
        let written = decompress(&input, &mut output, &mut workspace).unwrap();
        assert_eq!(written, 1000);
        let text = expected_text();
        assert_eq!(&output[..500], text.as_slice());
        assert_eq!(&output[500..1000], text.as_slice());
    }

    fn build_cosmoz_frame() -> Vec<u8> {
        let zstd_checksum_length = 4;
        let header = read_frame_header(&TEXT_500_WITH_CHECKSUM).unwrap();
        let header_length = header.header_length;
        let content_size = header.content_size.unwrap();
        let blocks = &TEXT_500_WITH_CHECKSUM
            [header_length..TEXT_500_WITH_CHECKSUM.len() - zstd_checksum_length];

        let mut frame = Vec::new();
        frame.extend_from_slice(&[0x4F, 0x53, 0x4D, 0x4F]);
        frame.extend_from_slice(&TEXT_500_WITH_CHECKSUM[4..header_length]);

        frame.extend_from_slice(&1u32.to_le_bytes());
        frame.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
        frame.extend_from_slice(&(content_size as u32).to_le_bytes());

        frame.extend_from_slice(blocks);

        let checksum = xxhash3::hash_bytes(&expected_text());
        frame.extend_from_slice(&checksum.to_le_bytes());
        frame
    }

    #[test]
    fn decompresses_an_cosmoz_frame_with_checksum() {
        let frame = build_cosmoz_frame();
        let mut workspace = DecodeWorkspace::new_boxed();
        let mut output = [0u8; 1024];
        let written = decompress(&frame, &mut output, &mut workspace).unwrap();
        assert_eq!(written, 500);
        assert_eq!(&output[..written], expected_text().as_slice());
    }

    #[test]
    fn rejects_flipped_cosmoz_checksum_byte() {
        let mut frame = build_cosmoz_frame();
        let last_index = frame.len() - 1;
        frame[last_index] ^= 0xFF;
        let mut workspace = DecodeWorkspace::new_boxed();
        let mut output = [0u8; 1024];
        assert_eq!(
            decompress(&frame, &mut output, &mut workspace),
            Err(DecodeError::ChecksumMismatch)
        );
    }

    #[test]
    fn rejects_output_buffer_one_byte_too_small() {
        let mut workspace = DecodeWorkspace::new_boxed();
        let mut output = [0u8; 499];
        assert_eq!(
            decompress(&TEXT_500_WITH_CHECKSUM, &mut output, &mut workspace),
            Err(DecodeError::OutputTooSmall)
        );
    }

    #[test]
    fn decompresses_an_cosmoz_frame_with_empty_content() {
        let mut frame = Vec::new();
        frame.extend_from_slice(&[0x4F, 0x53, 0x4D, 0x4F]);
        frame.push(0x24);
        frame.push(0x00);

        frame.extend_from_slice(&1u32.to_le_bytes());
        frame.extend_from_slice(&3u32.to_le_bytes());
        frame.extend_from_slice(&0u32.to_le_bytes());

        frame.extend_from_slice(&[0x01, 0x00, 0x00]);

        let checksum = xxhash3::hash_bytes(&[]);
        frame.extend_from_slice(&checksum.to_le_bytes());

        let mut workspace = DecodeWorkspace::new_boxed();
        let mut output = [0u8; 16];
        let written = decompress(&frame, &mut output, &mut workspace).unwrap();
        assert_eq!(written, 0);
    }

    #[test]
    fn rejects_cosmoz_frame_without_content_size() {
        let input = [0x4F, 0x53, 0x4D, 0x4F, 0x00, 0x00];
        assert_eq!(read_frame_header(&input), Err(DecodeError::BadFrameHeader));
    }

    #[cfg(feature = "compression")]
    #[test]
    fn rejects_cosmoz_frame_with_wrong_chunk_sum() {
        use crate::encoder::{CompressFormat, CompressOptions, EncodeWorkspace, compress, get_max_compressed_size};
        use crate::frame::chunk_index::CHUNK_COUNT_LENGTH;

        fn make_text(length: usize) -> Vec<u8> {
            let sentence = b"the quick brown fox jumps over the lazy dog. ";
            let mut text = Vec::with_capacity(length + sentence.len());
            while text.len() < length {
                text.extend_from_slice(sentence);
            }
            text.truncate(length);
            text
        }

        let mut concatenated = make_text(10_000);
        concatenated.extend_from_slice(&make_text(20_000));

        let options = CompressOptions {
            format: CompressFormat::Cosmoz { chunk_size: 10_000 },
            with_checksum: true,
            level: 1,
        };
        let mut workspace = EncodeWorkspace::new_boxed();
        let mut frame = vec![0u8; get_max_compressed_size(concatenated.len(), &options)];
        let written = compress(&concatenated, &mut frame, &options, &mut workspace).unwrap();
        frame.truncate(written);

        let header = read_frame_header(&frame).unwrap();
        let compressed_length_field = 4;
        let first_entry_decompressed_length_offset =
            header.header_length + CHUNK_COUNT_LENGTH + compressed_length_field;
        frame[first_entry_decompressed_length_offset] =
            frame[first_entry_decompressed_length_offset].wrapping_add(1);

        let mut decode_workspace = DecodeWorkspace::new_boxed();
        let mut output = vec![0u8; concatenated.len() + 4096];
        assert_eq!(
            decompress(&frame, &mut output, &mut decode_workspace),
            Err(DecodeError::BadFrameHeader)
        );
    }

    const COPY_SEQUENCE_COUNT: usize = 22;

    fn write_raw_test_block(frame: &mut Vec<u8>, block_type: BlockType, payload: &[u8], is_last: bool) {
        let type_bits = match block_type {
            BlockType::Raw => 0,
            BlockType::Rle => 1,
            BlockType::Compressed => 2,
        };
        let header = (payload.len() << 3) | (type_bits << 1) | is_last as usize;
        frame.extend_from_slice(&header.to_le_bytes()[..3]);
        frame.extend_from_slice(payload);
    }

    fn write_copy_sequences_bitstream(payload: &mut Vec<u8>) {
        let offset_extra_bits = 0b011u128;
        let mut bits = 1u128;
        for _ in 0..COPY_SEQUENCE_COUNT {
            bits = (bits << 3) | offset_extra_bits;
        }
        let byte_count = (3 * COPY_SEQUENCE_COUNT + 1).div_ceil(8);
        payload.extend_from_slice(&bits.to_le_bytes()[..byte_count]);
    }

    fn build_frame_with_repeat_tables_after_an_empty_block() -> (Vec<u8>, Vec<u8>) {
        let all_rle_modes = 0b0101_0100u8;
        let all_repeat_modes = 0b1111_1100u8;
        let literal_length_code_zero = 0u8;
        let offset_code_three = 3u8;
        let match_length_code_for_eight = 5u8;
        let no_literals = 0u8;
        let eight_raw_literals = 8u8 << 3;
        let no_sequences = 0u8;

        let mut first_copies = vec![no_literals, COPY_SEQUENCE_COUNT as u8, all_rle_modes];
        first_copies.extend_from_slice(&[
            literal_length_code_zero,
            offset_code_three,
            match_length_code_for_eight,
        ]);
        write_copy_sequences_bitstream(&mut first_copies);

        let mut literals_only = vec![eight_raw_literals];
        literals_only.extend_from_slice(b"ijklmnop");
        literals_only.push(no_sequences);

        let mut second_copies = vec![no_literals, COPY_SEQUENCE_COUNT as u8, all_repeat_modes];
        write_copy_sequences_bitstream(&mut second_copies);

        let mut expected = b"abcdefgh".repeat(COPY_SEQUENCE_COUNT + 1);
        expected.extend_from_slice(&b"ijklmnop".repeat(COPY_SEQUENCE_COUNT + 1));

        let single_segment_with_two_byte_content_size = 0x60u8;
        let mut frame = vec![
            0x28,
            0xB5,
            0x2F,
            0xFD,
            single_segment_with_two_byte_content_size,
        ];
        frame.extend_from_slice(&((expected.len() - 256) as u16).to_le_bytes());
        write_raw_test_block(&mut frame, BlockType::Raw, b"abcdefgh", false);
        write_raw_test_block(&mut frame, BlockType::Compressed, &first_copies, false);
        write_raw_test_block(&mut frame, BlockType::Compressed, &literals_only, false);
        write_raw_test_block(&mut frame, BlockType::Compressed, &second_copies, true);
        (frame, expected)
    }

    fn decode_frame_for_test(frame: &[u8], output_length: usize, content_length: usize) -> Vec<u8> {
        let mut output = vec![0u8; output_length];
        let mut workspace = DecodeWorkspace::new_boxed();
        let written = decompress(frame, &mut output, &mut workspace).unwrap();
        assert_eq!(written, content_length);
        output.truncate(written);
        output
    }

    #[test]
    fn repeat_mode_after_a_block_without_sequences_reuses_the_earlier_tables() {
        use crate::frame::block_header::MAX_BLOCK_SIZE;

        let (frame, expected) = build_frame_with_repeat_tables_after_an_empty_block();

        let checked_path_output = decode_frame_for_test(&frame, expected.len(), expected.len());
        assert_eq!(checked_path_output, expected);

        let fast_path_output =
            decode_frame_for_test(&frame, expected.len() + MAX_BLOCK_SIZE + 64, expected.len());
        assert_eq!(fast_path_output, expected);
    }

    fn find_zstd_cli() -> Option<std::path::PathBuf> {
        let output = std::process::Command::new("which").arg("zstd").output().ok()?;
        let path = String::from_utf8(output.stdout).ok()?;
        let path = path.trim();
        if output.status.success() && !path.is_empty() {
            Some(std::path::PathBuf::from(path))
        } else {
            None
        }
    }

    fn run_zstd_cli(zstd_path: &std::path::Path, arguments: &[&str], input: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut child = std::process::Command::new(zstd_path)
            .args(arguments)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("failed to start zstd");
        child
            .stdin
            .take()
            .expect("failed to open zstd input")
            .write_all(input)
            .expect("failed to write to zstd");
        let output = child
            .wait_with_output()
            .expect("failed to read zstd output");
        assert!(output.status.success(), "zstd failed");
        output.stdout
    }

    fn has_repeat_tables_after_a_block_without_sequences(frame: &[u8]) -> bool {
        use crate::block::literals::read_literals_header;
        use crate::block::sequences::{TableMode, read_sequences_header};

        let mut position = read_frame_header(frame).unwrap().header_length;
        let mut previous_block_had_no_sequences = false;
        loop {
            let block_header = read_block_header(&frame[position..]).unwrap();
            position += 3;
            if block_header.block_type == BlockType::Compressed {
                let payload = &frame[position..position + block_header.block_size];
                let literals_header = read_literals_header(payload).unwrap();
                let sequences_input =
                    &payload[literals_header.header_length + literals_header.compressed_size..];
                let sequences_header = read_sequences_header(sequences_input).unwrap();
                let modes = [
                    sequences_header.literal_length_mode,
                    sequences_header.offset_mode,
                    sequences_header.match_length_mode,
                ];
                if sequences_header.sequence_count > 0
                    && previous_block_had_no_sequences
                    && modes.contains(&TableMode::Repeat)
                {
                    return true;
                }
                previous_block_had_no_sequences = sequences_header.sequence_count == 0;
            }
            if block_header.block_type == BlockType::Rle {
                position += 1;
            } else {
                position += block_header.block_size;
            }
            if block_header.is_last {
                return false;
            }
        }
    }

    const IRON_PATH: &str = "/Users/josorio/github/josoriom/szstd/data/iron_ultrairon_SER_MS-AI-HILPOS@fNMR_IROr20_IROp011_LTR_16.mzML";
    const IRON_SLICE_START: usize = 18_000_000;
    const IRON_SLICE_LENGTH: usize = 1_000_000;

    #[test]
    fn decodes_a_zstd_level_nine_frame_that_repeats_tables_after_a_block_without_sequences() {
        let Some(zstd_path) = find_zstd_cli() else {
            eprintln!("skipping: zstd CLI not found");
            return;
        };
        let Ok(iron) = std::fs::read(IRON_PATH) else {
            eprintln!("skipping: {IRON_PATH} not found");
            return;
        };
        let input = &iron[IRON_SLICE_START..IRON_SLICE_START + IRON_SLICE_LENGTH];
        let frame = run_zstd_cli(&zstd_path, &["-q", "-c", "-T1", "-9", "--no-check"], input);
        assert!(
            has_repeat_tables_after_a_block_without_sequences(&frame),
            "this zstd version no longer writes Repeat tables after a block without sequences"
        );

        let expected = run_zstd_cli(&zstd_path, &["-q", "-d", "-c"], &frame);
        assert_eq!(expected, input);

        let decoded = decode_frame_for_test(&frame, input.len(), input.len());
        assert_eq!(decoded, expected);
    }
}
