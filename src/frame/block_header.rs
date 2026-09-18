use crate::error::DecodeError;

pub(crate) const MAX_BLOCK_SIZE: usize = 128 * 1024;
pub(crate) const BLOCK_HEADER_LENGTH: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BlockType {
    Raw,
    Rle,
    Compressed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockHeader {
    pub block_type: BlockType,
    pub block_size: usize,
    pub is_last: bool,
}

pub(crate) fn read_block_header(input: &[u8]) -> Result<BlockHeader, DecodeError> {
    if input.len() < BLOCK_HEADER_LENGTH {
        return Err(DecodeError::InputTooShort);
    }
    let raw_value = read_little_endian_u32(&input[0..BLOCK_HEADER_LENGTH]);
    let is_last = raw_value & 1 != 0;
    let type_bits = (raw_value >> 1) & 0b11;
    let block_size = (raw_value >> 3) as usize;

    let block_type = match type_bits {
        0 => BlockType::Raw,
        1 => BlockType::Rle,
        2 => BlockType::Compressed,
        _ => return Err(DecodeError::ReservedBlockType),
    };

    if block_size > MAX_BLOCK_SIZE {
        return Err(DecodeError::BlockTooLarge);
    }

    Ok(BlockHeader {
        block_type,
        block_size,
        is_last,
    })
}

fn read_little_endian_u32(bytes: &[u8]) -> u32 {
    let mut value: u32 = 0;
    for (index, byte) in bytes.iter().enumerate() {
        value |= (*byte as u32) << (8 * index);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_raw_block_of_size_zero_marked_last() {
        let header = read_block_header(&[0x01, 0x00, 0x00]).unwrap();
        assert_eq!(
            header,
            BlockHeader {
                block_type: BlockType::Raw,
                block_size: 0,
                is_last: true,
            }
        );
    }

    #[test]
    fn reads_rle_block_with_regenerated_size_one_marked_last() {
        let header = read_block_header(&[0x0B, 0x00, 0x00]).unwrap();
        assert_eq!(
            header,
            BlockHeader {
                block_type: BlockType::Rle,
                block_size: 1,
                is_last: true,
            }
        );
    }

    #[test]
    fn reads_real_block_header_from_zstd_cli() {
        let header = read_block_header(&[0xCD, 0x01, 0x00]).unwrap();
        assert_eq!(
            header,
            BlockHeader {
                block_type: BlockType::Compressed,
                block_size: 57,
                is_last: true,
            }
        );
    }

    #[test]
    fn rejects_reserved_block_type() {
        assert_eq!(
            read_block_header(&[0x07, 0x00, 0x00]),
            Err(DecodeError::ReservedBlockType)
        );
    }

    #[test]
    fn accepts_block_size_at_maximum_and_rejects_one_above() {
        let maximum_size_raw_value = (MAX_BLOCK_SIZE as u32) << 3;
        let maximum_size_bytes = maximum_size_raw_value.to_le_bytes();
        let header = read_block_header(&maximum_size_bytes[0..3]).unwrap();
        assert_eq!(header.block_size, MAX_BLOCK_SIZE);

        let one_above_maximum_raw_value = (MAX_BLOCK_SIZE as u32 + 1) << 3;
        let one_above_maximum_bytes = one_above_maximum_raw_value.to_le_bytes();
        assert_eq!(
            read_block_header(&one_above_maximum_bytes[0..3]),
            Err(DecodeError::BlockTooLarge)
        );
    }

    #[test]
    fn rejects_input_shorter_than_block_header_length() {
        assert_eq!(read_block_header(&[]), Err(DecodeError::InputTooShort));
        assert_eq!(read_block_header(&[0x00]), Err(DecodeError::InputTooShort));
        assert_eq!(
            read_block_header(&[0x00, 0x00]),
            Err(DecodeError::InputTooShort)
        );
    }
}
