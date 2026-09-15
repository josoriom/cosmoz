use crate::block::block_decoder::{BlockWorkspace, decode_block};
#[cfg(feature = "alloc")]
use crate::block::repeat_offsets::RepeatOffsets;
use crate::error::DecodeError;
use crate::frame::block_header::{BLOCK_HEADER_LENGTH, BlockType, read_block_header};
use crate::frame::chunk_index::ChunkIndex;
use crate::frame::frame_header::{
    FrameFormat, FrameHeader, get_skippable_frame_length, is_skippable_frame, read_frame_header,
};
use crate::hash::xxhash3::XxHash3;
use crate::hash::xxhash64::XxHash64;

pub struct DecodeWorkspace {
    pub block: BlockWorkspace,
}

impl DecodeWorkspace {
    pub const fn new() -> Self {
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

#[cfg(feature = "alloc")]
impl DecodeWorkspace {
    pub fn new_boxed() -> alloc::boxed::Box<Self> {
        let layout = core::alloc::Layout::new::<Self>();
        unsafe {
            let raw = alloc::alloc::alloc_zeroed(layout) as *mut Self;
            if raw.is_null() {
                alloc::alloc::handle_alloc_error(layout);
            }
            write_initial_values(raw);
            alloc::boxed::Box::from_raw(raw)
        }
    }
}

#[cfg(feature = "alloc")]
unsafe fn write_initial_values(target: *mut DecodeWorkspace) {
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
    }
}

#[cfg(all(test, feature = "alloc"))]
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
            boxed_workspace.block.huffman_table.is_ready,
            stack_workspace.block.huffman_table.is_ready
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

#[allow(clippy::large_enum_variant)]
enum ContentHasher {
    Zstd(XxHash64),
    Osmo(XxHash3),
}

impl ContentHasher {
    fn new(format: FrameFormat) -> Self {
        match format {
            FrameFormat::Zstd => ContentHasher::Zstd(XxHash64::new(0)),
            FrameFormat::Osmo => ContentHasher::Osmo(XxHash3::new()),
        }
    }

    fn update(&mut self, input: &[u8]) {
        match self {
            ContentHasher::Zstd(hasher) => hasher.update(input),
            ContentHasher::Osmo(hasher) => hasher.update(input),
        }
    }

    fn matches_checksum(&self, expected_bytes: &[u8]) -> bool {
        match self {
            ContentHasher::Zstd(hasher) => {
                let low_32_bits = (hasher.finish() & 0xFFFF_FFFF) as u32;
                expected_bytes == low_32_bits.to_le_bytes()
            }
            ContentHasher::Osmo(hasher) => expected_bytes == hasher.finish().to_le_bytes(),
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

        output_position = decode_block(payload, &block_header, output, output_position, workspace)?;

        if block_header.is_last {
            break;
        }
    }

    Ok((input_position, output_position))
}

fn skip_frame_body(input: &[u8], header: &FrameHeader) -> Result<usize, DecodeError> {
    match header.format {
        FrameFormat::Zstd => skip_zstd_frame_body(input, header),
        FrameFormat::Osmo => skip_osmo_frame_body(input, header),
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

fn skip_osmo_frame_body(input: &[u8], header: &FrameHeader) -> Result<usize, DecodeError> {
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

pub fn get_decompressed_size(input: &[u8]) -> Result<Option<u64>, DecodeError> {
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

pub fn decompress(
    input: &[u8],
    output: &mut [u8],
    workspace: &mut DecodeWorkspace,
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
            decode_frame(remaining_input, output, output_position, workspace)?;
        input_position += bytes_consumed;
        output_position = new_output_position;
    }

    Ok(output_position)
}

fn decode_frame(
    input: &[u8],
    output: &mut [u8],
    output_position: usize,
    workspace: &mut DecodeWorkspace,
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
        FrameFormat::Osmo => decode_osmo_frame_body(
            input,
            &header,
            output,
            frame_start_output_position,
            workspace,
        )?,
    };

    let mut hasher = ContentHasher::new(header.format);
    hasher.update(&output[frame_start_output_position..position]);

    let checksum_length = header.checksum_length();
    if header.has_checksum {
        let expected_bytes = input
            .get(input_position..input_position + checksum_length)
            .ok_or(DecodeError::InputTooShort)?;
        check_content_checksum(expected_bytes, &hasher)?;
    }
    input_position += checksum_length;

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

fn decode_osmo_frame_body(
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

    decode_osmo_chunks(chunks_input, &index, output_region, workspace)?;

    Ok((chunks_input_end, output_end))
}

fn decode_osmo_chunks(
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
    decode_osmo_chunks_sequentially(chunks_input, index, output, &mut workspace.block)
}

fn decode_osmo_chunks_sequentially(
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

        block_workspace.reset_history(FrameFormat::Osmo);
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

    fn build_osmo_frame() -> Vec<u8> {
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
    fn decompresses_an_osmo_frame_with_checksum() {
        let frame = build_osmo_frame();
        let mut workspace = DecodeWorkspace::new_boxed();
        let mut output = [0u8; 1024];
        let written = decompress(&frame, &mut output, &mut workspace).unwrap();
        assert_eq!(written, 500);
        assert_eq!(&output[..written], expected_text().as_slice());
    }

    #[test]
    fn rejects_flipped_osmo_checksum_byte() {
        let mut frame = build_osmo_frame();
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
    fn decompresses_an_osmo_frame_with_empty_content() {
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
}
