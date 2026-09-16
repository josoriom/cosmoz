use crate::{
    encode_error::EncodeError,
    frame::{
        block_header::{BLOCK_HEADER_LENGTH, BlockType, MAX_BLOCK_SIZE},
        chunk_index::{CHUNK_COUNT_LENGTH, CHUNK_ENTRY_LENGTH, ChunkEntry},
        frame_header::{
            FrameFormat, OSMOS_CHECKSUM_LENGTH, OSMOS_MAGIC_NUMBER, ZSTD_CHECKSUM_LENGTH,
            ZSTD_MAGIC_NUMBER,
        },
    },
};

pub const MAX_FRAME_HEADER_LENGTH: usize = 14;

pub fn write_frame_header(
    output: &mut [u8],
    format: FrameFormat,
    content_size: u64,
    window_log: u8,
    with_checksum: bool,
) -> Result<usize, EncodeError> {
    if !(10..=31).contains(&window_log) {
        return Err(EncodeError::BadOptions);
    }

    let content_size_flag = get_content_size_flag(content_size);
    let content_size_length = get_content_size_field_length(content_size_flag);
    let header_length = 4 + 1 + 1 + content_size_length;

    if output.len() < header_length {
        return Err(EncodeError::OutputTooSmall);
    }

    let magic_number = match format {
        FrameFormat::Zstd => ZSTD_MAGIC_NUMBER,
        FrameFormat::Osmos => OSMOS_MAGIC_NUMBER,
    };
    output[0..4].copy_from_slice(&magic_number.to_le_bytes());

    let checksum_bit: u8 = if with_checksum { 1 } else { 0 };
    output[4] = (content_size_flag << 6) | (checksum_bit << 2);

    output[5] = write_window_descriptor(window_log);

    let content_size_output = &mut output[6..6 + content_size_length];
    match content_size_flag {
        1 => {
            let stored_value = (content_size - 256) as u16;
            content_size_output.copy_from_slice(&stored_value.to_le_bytes());
        }
        2 => {
            let stored_value = content_size as u32;
            content_size_output.copy_from_slice(&stored_value.to_le_bytes());
        }
        _ => {
            content_size_output.copy_from_slice(&content_size.to_le_bytes());
        }
    }

    Ok(header_length)
}

pub fn write_block_header(
    output: &mut [u8],
    block_type: BlockType,
    block_size: usize,
    is_last: bool,
) -> Result<usize, EncodeError> {
    if block_size > MAX_BLOCK_SIZE {
        return Err(EncodeError::InputTooLarge);
    }
    if output.len() < BLOCK_HEADER_LENGTH {
        return Err(EncodeError::OutputTooSmall);
    }

    let type_bits = get_block_type_bits(block_type);
    let last_bit: u32 = if is_last { 1 } else { 0 };
    let raw_value = ((block_size as u32) << 3) | (type_bits << 1) | last_bit;
    output[0..BLOCK_HEADER_LENGTH]
        .copy_from_slice(&raw_value.to_le_bytes()[0..BLOCK_HEADER_LENGTH]);

    Ok(BLOCK_HEADER_LENGTH)
}

pub fn write_chunk_index(output: &mut [u8], entries: &[ChunkEntry]) -> Result<usize, EncodeError> {
    if entries.is_empty() {
        return Err(EncodeError::BadOptions);
    }

    let chunk_count = entries.len();
    if chunk_count > u32::MAX as usize {
        return Err(EncodeError::InputTooLarge);
    }

    for entry in entries {
        if entry.compressed_length > u32::MAX as usize
            || entry.decompressed_length > u32::MAX as usize
        {
            return Err(EncodeError::InputTooLarge);
        }
    }

    let entries_length = chunk_count
        .checked_mul(CHUNK_ENTRY_LENGTH)
        .ok_or(EncodeError::InputTooLarge)?;
    let index_length = CHUNK_COUNT_LENGTH
        .checked_add(entries_length)
        .ok_or(EncodeError::InputTooLarge)?;

    if output.len() < index_length {
        return Err(EncodeError::OutputTooSmall);
    }

    output[0..CHUNK_COUNT_LENGTH].copy_from_slice(&(chunk_count as u32).to_le_bytes());

    let mut position = CHUNK_COUNT_LENGTH;
    for entry in entries {
        output[position..position + 4]
            .copy_from_slice(&(entry.compressed_length as u32).to_le_bytes());
        output[position + 4..position + 8]
            .copy_from_slice(&(entry.decompressed_length as u32).to_le_bytes());
        position += CHUNK_ENTRY_LENGTH;
    }

    Ok(index_length)
}

pub fn write_checksum(
    output: &mut [u8],
    format: FrameFormat,
    hash: u64,
) -> Result<usize, EncodeError> {
    let checksum_length = match format {
        FrameFormat::Zstd => ZSTD_CHECKSUM_LENGTH,
        FrameFormat::Osmos => OSMOS_CHECKSUM_LENGTH,
    };

    if output.len() < checksum_length {
        return Err(EncodeError::OutputTooSmall);
    }

    match format {
        FrameFormat::Zstd => {
            let low_32_bits = (hash & 0xFFFF_FFFF) as u32;
            output[0..ZSTD_CHECKSUM_LENGTH].copy_from_slice(&low_32_bits.to_le_bytes());
        }
        FrameFormat::Osmos => {
            output[0..OSMOS_CHECKSUM_LENGTH].copy_from_slice(&hash.to_le_bytes());
        }
    }

    Ok(checksum_length)
}

fn get_content_size_flag(content_size: u64) -> u8 {
    if (256..=65535 + 256).contains(&content_size) {
        1
    } else if content_size <= u32::MAX as u64 {
        2
    } else {
        3
    }
}

fn get_content_size_field_length(content_size_flag: u8) -> usize {
    match content_size_flag {
        1 => 2,
        2 => 4,
        _ => 8,
    }
}

fn write_window_descriptor(window_log: u8) -> u8 {
    let exponent = window_log - 10;
    exponent << 3
}

fn get_block_type_bits(block_type: BlockType) -> u32 {
    match block_type {
        BlockType::Raw => 0,
        BlockType::Rle => 1,
        BlockType::Compressed => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::frame_header::read_frame_header;

    #[test]
    fn frame_header_writer_matches_reader_for_every_content_size_flag() {
        let sizes: [u64; 7] = [
            0,
            255,
            256,
            65791,
            65792,
            u32::MAX as u64,
            u32::MAX as u64 + 1,
        ];
        let formats = [FrameFormat::Zstd, FrameFormat::Osmos];
        let checksum_choices = [false, true];
        let window_log = 20u8;
        let mut max_written_length = 0usize;

        for &content_size in &sizes {
            for &format in &formats {
                for &with_checksum in &checksum_choices {
                    let mut output = [0u8; MAX_FRAME_HEADER_LENGTH];
                    let written = write_frame_header(
                        &mut output,
                        format,
                        content_size,
                        window_log,
                        with_checksum,
                    )
                    .unwrap();
                    max_written_length = max_written_length.max(written);

                    let header = read_frame_header(&output[..written]).unwrap();
                    assert_eq!(header.format, format);
                    assert_eq!(header.content_size, Some(content_size));
                    assert_eq!(header.window_size, 1u64 << window_log);
                    assert_eq!(header.has_checksum, with_checksum);
                    assert_eq!(header.header_length, written);
                }
            }
        }

        assert_eq!(max_written_length, MAX_FRAME_HEADER_LENGTH);
    }

    #[test]
    fn writers_reject_invalid_arguments() {
        let mut header_output = [0u8; MAX_FRAME_HEADER_LENGTH];
        assert_eq!(
            write_frame_header(&mut header_output, FrameFormat::Zstd, 100, 9, true),
            Err(EncodeError::BadOptions)
        );
        assert_eq!(
            write_frame_header(&mut header_output, FrameFormat::Zstd, 100, 32, true),
            Err(EncodeError::BadOptions)
        );

        let mut short_header_output = [0u8; 9];
        assert_eq!(
            write_frame_header(&mut short_header_output, FrameFormat::Zstd, 100, 20, true),
            Err(EncodeError::OutputTooSmall)
        );

        let mut block_output = [0u8; 4];
        assert_eq!(
            write_block_header(&mut block_output, BlockType::Raw, MAX_BLOCK_SIZE + 1, false),
            Err(EncodeError::InputTooLarge)
        );

        let mut short_block_output = [0u8; 2];
        assert_eq!(
            write_block_header(&mut short_block_output, BlockType::Raw, 10, false),
            Err(EncodeError::OutputTooSmall)
        );

        let mut index_output = [0u8; 16];
        assert_eq!(
            write_chunk_index(&mut index_output, &[]),
            Err(EncodeError::BadOptions)
        );

        let entries = [ChunkEntry {
            compressed_length: 5,
            decompressed_length: 5,
        }];
        let mut short_index_output = [0u8; 3];
        assert_eq!(
            write_chunk_index(&mut short_index_output, &entries),
            Err(EncodeError::OutputTooSmall)
        );

        let mut short_zstd_checksum_output = [0u8; 3];
        assert_eq!(
            write_checksum(&mut short_zstd_checksum_output, FrameFormat::Zstd, 1),
            Err(EncodeError::OutputTooSmall)
        );

        let mut short_osmos_checksum_output = [0u8; 7];
        assert_eq!(
            write_checksum(&mut short_osmos_checksum_output, FrameFormat::Osmos, 1),
            Err(EncodeError::OutputTooSmall)
        );
    }
}
