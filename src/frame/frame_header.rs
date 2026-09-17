use crate::error::DecodeError;

pub const ZSTD_MAGIC_NUMBER: u32 = 0xFD2FB528;
pub const COSMOZ_MAGIC_NUMBER: u32 = 0x4F4D534F;
pub const SKIPPABLE_MAGIC_MASK: u32 = 0xFFFFFFF0;
pub const SKIPPABLE_MAGIC_BASE: u32 = 0x184D2A50;
pub const MAX_WINDOW_SIZE: u64 = 1 << 31;
pub const ZSTD_CHECKSUM_LENGTH: usize = 4;
pub const COSMOZ_CHECKSUM_LENGTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFormat {
    Zstd,
    Cosmoz,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub format: FrameFormat,
    pub window_size: u64,
    pub content_size: Option<u64>,
    pub dictionary_id: u32,
    pub has_checksum: bool,
    pub single_segment: bool,
    pub header_length: usize,
}

impl FrameHeader {
    pub fn checksum_length(&self) -> usize {
        if !self.has_checksum {
            return 0;
        }
        match self.format {
            FrameFormat::Zstd => ZSTD_CHECKSUM_LENGTH,
            FrameFormat::Cosmoz => COSMOZ_CHECKSUM_LENGTH,
        }
    }
}

fn read_frame_format(magic_number: u32) -> Result<FrameFormat, DecodeError> {
    match magic_number {
        ZSTD_MAGIC_NUMBER => Ok(FrameFormat::Zstd),
        COSMOZ_MAGIC_NUMBER => Ok(FrameFormat::Cosmoz),
        _ => Err(DecodeError::BadMagicNumber),
    }
}

pub fn read_frame_header(input: &[u8]) -> Result<FrameHeader, DecodeError> {
    if input.len() < 4 {
        return Err(DecodeError::InputTooShort);
    }
    let magic_number = read_little_endian_u32(&input[0..4]);
    let format = read_frame_format(magic_number)?;
    if input.len() < 5 {
        return Err(DecodeError::InputTooShort);
    }
    let descriptor_byte = input[4];
    let content_size_flag = descriptor_byte >> 6;
    let single_segment = (descriptor_byte >> 5) & 1 != 0;
    let reserved_bit_set = (descriptor_byte >> 3) & 1 != 0;
    let has_checksum = (descriptor_byte >> 2) & 1 != 0;
    let dictionary_id_flag = descriptor_byte & 0b11;

    let window_descriptor_length = if single_segment { 0 } else { 1 };
    let dictionary_id_length = get_dictionary_id_field_length(dictionary_id_flag);
    let content_size_length = get_content_size_field_length(content_size_flag, single_segment);

    let header_length =
        4 + 1 + window_descriptor_length + dictionary_id_length + content_size_length;
    if input.len() < header_length {
        return Err(DecodeError::InputTooShort);
    }

    if reserved_bit_set {
        return Err(DecodeError::BadFrameHeader);
    }

    let mut position = 5;

    let window_size_from_descriptor = if single_segment {
        None
    } else {
        let window_size = read_window_size(input[position]);
        position += 1;
        Some(window_size)
    };

    let dictionary_id = read_little_endian_u32(&input[position..position + dictionary_id_length]);
    position += dictionary_id_length;

    let content_size = read_content_size(
        &input[position..position + content_size_length],
        content_size_flag,
    );

    if dictionary_id != 0 {
        return Err(DecodeError::DictionaryNotSupported);
    }

    let window_size = if single_segment {
        match content_size {
            Some(size) => size,
            None => return Err(DecodeError::BadFrameHeader),
        }
    } else {
        match window_size_from_descriptor {
            Some(size) => size,
            None => return Err(DecodeError::BadFrameHeader),
        }
    };

    if window_size > MAX_WINDOW_SIZE {
        return Err(DecodeError::WindowTooLarge);
    }

    if format == FrameFormat::Cosmoz && content_size.is_none() {
        return Err(DecodeError::BadFrameHeader);
    }

    Ok(FrameHeader {
        format,
        window_size,
        content_size,
        dictionary_id,
        has_checksum,
        single_segment,
        header_length,
    })
}

pub fn is_skippable_frame(input: &[u8]) -> bool {
    if input.len() < 4 {
        return false;
    }
    let magic_number = read_little_endian_u32(&input[0..4]);
    magic_number & SKIPPABLE_MAGIC_MASK == SKIPPABLE_MAGIC_BASE
}

pub fn get_skippable_frame_length(input: &[u8]) -> Result<usize, DecodeError> {
    if input.len() < 8 {
        return Err(DecodeError::InputTooShort);
    }
    let content_length = read_little_endian_u32(&input[4..8]) as usize;
    let total_length = match 8usize.checked_add(content_length) {
        Some(length) => length,
        None => return Err(DecodeError::InputTooShort),
    };
    if total_length > input.len() {
        return Err(DecodeError::InputTooShort);
    }
    let magic_number = read_little_endian_u32(&input[0..4]);
    if magic_number & SKIPPABLE_MAGIC_MASK != SKIPPABLE_MAGIC_BASE {
        return Err(DecodeError::BadMagicNumber);
    }
    Ok(total_length)
}

fn read_window_size(descriptor_byte: u8) -> u64 {
    let exponent = (descriptor_byte >> 3) as u32;
    let mantissa = (descriptor_byte & 0b111) as u64;
    let window_base = 1u64 << (10 + exponent);
    window_base + (window_base / 8) * mantissa
}

fn get_content_size_field_length(flag: u8, single_segment: bool) -> usize {
    match flag {
        0 => {
            if single_segment {
                1
            } else {
                0
            }
        }
        1 => 2,
        2 => 4,
        3 => 8,
        _ => 0,
    }
}

fn get_dictionary_id_field_length(flag: u8) -> usize {
    match flag {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => 4,
        _ => 0,
    }
}

fn read_content_size(bytes: &[u8], flag: u8) -> Option<u64> {
    if bytes.is_empty() {
        return None;
    }
    let raw_value = read_little_endian_u64(bytes);
    if flag == 1 {
        Some(raw_value + 256)
    } else {
        Some(raw_value)
    }
}

fn read_little_endian_u32(bytes: &[u8]) -> u32 {
    let mut value: u32 = 0;
    for (index, byte) in bytes.iter().enumerate() {
        value |= (*byte as u32) << (8 * index);
    }
    value
}

fn read_little_endian_u64(bytes: &[u8]) -> u64 {
    let mut value: u64 = 0;
    for (index, byte) in bytes.iter().enumerate() {
        value |= (*byte as u64) << (8 * index);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZSTD_LEVEL_ONE_HEADER: [u8; 6] = [0x28, 0xB5, 0x2F, 0xFD, 0x24, 0x40];

    #[test]
    fn reads_real_header_from_zstd_cli_with_checksum() {
        let frame_header = read_frame_header(&ZSTD_LEVEL_ONE_HEADER).unwrap();
        assert_eq!(
            frame_header,
            FrameHeader {
                format: FrameFormat::Zstd,
                window_size: 64,
                content_size: Some(64),
                dictionary_id: 0,
                has_checksum: true,
                single_segment: true,
                header_length: 6,
            }
        );
    }

    #[test]
    fn reads_streamed_header_from_zstd_cli_with_window_descriptor_and_no_content_size() {
        let input = [0x28, 0xB5, 0x2F, 0xFD, 0x00, 0x58];
        let frame_header = read_frame_header(&input).unwrap();
        assert_eq!(
            frame_header,
            FrameHeader {
                format: FrameFormat::Zstd,
                window_size: 1 << 21,
                content_size: None,
                dictionary_id: 0,
                has_checksum: false,
                single_segment: false,
                header_length: 6,
            }
        );
    }

    #[test]
    fn reads_window_mantissa_and_four_byte_content_size() {
        let input = [0x28, 0xB5, 0x2F, 0xFD, 0x80, 0x13, 0x00, 0x00, 0x01, 0x00];
        let frame_header = read_frame_header(&input).unwrap();
        assert_eq!(frame_header.window_size, 4096 + 512 * 3);
        assert_eq!(frame_header.content_size, Some(65536));
        assert_eq!(frame_header.header_length, 10);
    }

    #[test]
    fn reads_single_segment_with_one_byte_content_size() {
        let input = [0x28, 0xB5, 0x2F, 0xFD, 0x20, 0x0A];
        let frame_header = read_frame_header(&input).unwrap();
        assert_eq!(
            frame_header,
            FrameHeader {
                format: FrameFormat::Zstd,
                window_size: 10,
                content_size: Some(10),
                dictionary_id: 0,
                has_checksum: false,
                single_segment: true,
                header_length: 6,
            }
        );
    }

    #[test]
    fn reads_content_size_flag_one_with_two_bytes_plus_two_hundred_fifty_six() {
        let input = [0x28, 0xB5, 0x2F, 0xFD, 0x60, 0x0A, 0x00];
        let frame_header = read_frame_header(&input).unwrap();
        assert_eq!(frame_header.content_size, Some(10 + 256));
        assert_eq!(frame_header.window_size, 10 + 256);
        assert_eq!(frame_header.header_length, 7);
    }

    #[test]
    fn reads_cosmoz_frame_header() {
        let input = [0x4F, 0x53, 0x4D, 0x4F, 0x24, 0x40];
        let frame_header = read_frame_header(&input).unwrap();
        assert_eq!(
            frame_header,
            FrameHeader {
                format: FrameFormat::Cosmoz,
                window_size: 64,
                content_size: Some(64),
                dictionary_id: 0,
                has_checksum: true,
                single_segment: true,
                header_length: 6,
            }
        );
        assert_eq!(frame_header.checksum_length(), 8);
    }

    #[test]
    fn checksum_length_depends_on_format_and_flag() {
        let zstd_with_checksum = read_frame_header(&ZSTD_LEVEL_ONE_HEADER).unwrap();
        assert_eq!(zstd_with_checksum.checksum_length(), 4);

        let zstd_without_checksum =
            read_frame_header(&[0x28, 0xB5, 0x2F, 0xFD, 0x00, 0x58]).unwrap();
        assert_eq!(zstd_without_checksum.checksum_length(), 0);

        let cosmoz_with_checksum =
            read_frame_header(&[0x4F, 0x53, 0x4D, 0x4F, 0x24, 0x40]).unwrap();
        assert_eq!(cosmoz_with_checksum.checksum_length(), 8);

        let cosmoz_without_checksum =
            read_frame_header(&[0x4F, 0x53, 0x4D, 0x4F, 0x20, 0x0A]).unwrap();
        assert_eq!(cosmoz_without_checksum.checksum_length(), 0);
    }

    #[test]
    fn rejects_magic_one_bit_away_from_cosmoz() {
        let input = [0x4E, 0x53, 0x4D, 0x4F, 0x24, 0x40];
        assert_eq!(read_frame_header(&input), Err(DecodeError::BadMagicNumber));
    }

    #[test]
    fn rejects_reserved_bit_set() {
        let input = [0x28, 0xB5, 0x2F, 0xFD, 0b0000_1000, 0x00];
        assert_eq!(read_frame_header(&input), Err(DecodeError::BadFrameHeader));
    }

    #[test]
    fn accepts_unused_bit_four_set() {
        let input = [0x28, 0xB5, 0x2F, 0xFD, 0b0011_0000, 0x00];
        let frame_header = read_frame_header(&input).unwrap();
        assert_eq!(frame_header.content_size, Some(0));
    }

    #[test]
    fn rejects_non_zero_dictionary_id() {
        let input = [0x28, 0xB5, 0x2F, 0xFD, 0x21, 0x0A, 0x05];
        assert_eq!(
            read_frame_header(&input),
            Err(DecodeError::DictionaryNotSupported)
        );
    }

    #[test]
    fn rejects_window_size_above_maximum() {
        let input = [0x28, 0xB5, 0x2F, 0xFD, 0x00, 0b1011_0000];
        assert_eq!(read_frame_header(&input), Err(DecodeError::WindowTooLarge));
    }

    #[test]
    fn rejects_every_truncated_prefix_of_a_valid_header() {
        let full_header = [0x28, 0xB5, 0x2F, 0xFD, 0x60, 0x0A, 0x00];
        for length in 0..full_header.len() {
            let prefix = &full_header[0..length];
            assert_eq!(read_frame_header(prefix), Err(DecodeError::InputTooShort));
        }
    }

    #[test]
    fn recognizes_skippable_frame_magic_range() {
        assert!(is_skippable_frame(&[0x50, 0x2A, 0x4D, 0x18]));
        assert!(is_skippable_frame(&[0x5F, 0x2A, 0x4D, 0x18]));
        assert!(!is_skippable_frame(&[0x28, 0xB5, 0x2F, 0xFD]));
        assert!(!is_skippable_frame(&[0x00, 0x00, 0x00]));
    }

    #[test]
    fn computes_skippable_frame_length() {
        let input = [0x50, 0x2A, 0x4D, 0x18, 0x03, 0x00, 0x00, 0x00, 1, 2, 3];
        assert_eq!(get_skippable_frame_length(&input), Ok(11));
    }

    #[test]
    fn rejects_skippable_frame_size_larger_than_remaining_input() {
        let input = [0x50, 0x2A, 0x4D, 0x18, 0xFF, 0xFF, 0xFF, 0xFF, 1, 2, 3];
        assert_eq!(
            get_skippable_frame_length(&input),
            Err(DecodeError::InputTooShort)
        );
    }
}
