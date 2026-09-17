use crate::{
    entropy::{
        fse_decode_table::FseDecodeTable,
        huffman_decode::{decode_many_streams_with_slack, decode_one_stream, split_streams},
        huffman_decode_fast::decode_two_sections_unchecked,
        huffman_decode_table::{HuffmanDecodeTable, read_huffman_table},
    },
    error::DecodeError,
    frame::{block_header::MAX_BLOCK_SIZE, frame_header::FrameFormat},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LiteralsType {
    Raw,
    Rle,
    Compressed,
    Treeless,
}

#[derive(PartialEq, Eq)]
#[cfg_attr(test, derive(Debug))]
pub enum LiteralSource<'input, 'workspace> {
    Raw(&'input [u8]),
    Rle { byte: u8, count: usize },
    Decoded(&'workspace [u8]),
    InOutput { start: usize, count: usize },
}

impl<'input, 'workspace> LiteralSource<'input, 'workspace> {
    pub fn len(&self) -> usize {
        match self {
            LiteralSource::Raw(bytes) => bytes.len(),
            LiteralSource::Rle { count, .. } => *count,
            LiteralSource::Decoded(bytes) => bytes.len(),
            LiteralSource::InOutput { count, .. } => *count,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct LiteralsHeader {
    pub literals_type: LiteralsType,
    pub regenerated_size: usize,
    pub compressed_size: usize,
    pub stream_count: u8,
    pub header_length: usize,
}

pub fn read_literals_header(input: &[u8]) -> Result<LiteralsHeader, DecodeError> {
    let header_byte = *input.first().ok_or(DecodeError::InputTooShort)?;
    let type_bits = header_byte & 0b11;
    let size_format = (header_byte >> 2) & 0b11;

    let literals_type = match type_bits {
        0 => LiteralsType::Raw,
        1 => LiteralsType::Rle,
        2 => LiteralsType::Compressed,
        _ => LiteralsType::Treeless,
    };

    match literals_type {
        LiteralsType::Raw | LiteralsType::Rle => {
            let (header_length, regenerated_size) = match size_format {
                0 | 2 => (1usize, (header_byte >> 3) as usize),
                1 => {
                    let value = read_little_endian(input, 2)?;
                    (2usize, ((value >> 4) & 0xFFF) as usize)
                }
                _ => {
                    let value = read_little_endian(input, 3)?;
                    (3usize, ((value >> 4) & 0xFFFFF) as usize)
                }
            };
            let compressed_size = if literals_type == LiteralsType::Raw {
                regenerated_size
            } else {
                1
            };
            Ok(LiteralsHeader {
                literals_type,
                regenerated_size,
                compressed_size,
                stream_count: 1,
                header_length,
            })
        }
        LiteralsType::Compressed | LiteralsType::Treeless => {
            let (header_length, stream_count, size_bits) = match size_format {
                0 => (3usize, 1u8, 10u32),
                1 => (3usize, 4u8, 10u32),
                2 => (4usize, 4u8, 14u32),
                _ => (5usize, 4u8, 18u32),
            };
            let value = read_little_endian(input, header_length)?;
            let size_mask = (1u64 << size_bits) - 1;
            let regenerated_size = ((value >> 4) & size_mask) as usize;
            let compressed_size = ((value >> (4 + size_bits)) & size_mask) as usize;
            Ok(LiteralsHeader {
                literals_type,
                regenerated_size,
                compressed_size,
                stream_count,
                header_length,
            })
        }
    }
}

pub fn decode_literals<'input, 'workspace>(
    input: &'input [u8],
    format: FrameFormat,
    huffman_table: &mut HuffmanDecodeTable,
    weight_fse_table: &mut FseDecodeTable,
    workspace: &'workspace mut [u8],
) -> Result<(LiteralSource<'input, 'workspace>, usize), DecodeError> {
    let header = read_literals_header(input)?;
    if header.regenerated_size > MAX_BLOCK_SIZE {
        return Err(DecodeError::BadLiteralsHeader);
    }
    if header.regenerated_size > workspace.len() {
        return Err(DecodeError::OutputTooSmall);
    }
    let stream_count = resolve_stream_count(format, header.stream_count);

    match header.literals_type {
        LiteralsType::Raw => {
            let literal_bytes = input
                .get(header.header_length..header.header_length + header.regenerated_size)
                .ok_or(DecodeError::InputTooShort)?;
            Ok((
                LiteralSource::Raw(literal_bytes),
                header.header_length + header.regenerated_size,
            ))
        }
        LiteralsType::Rle => {
            let repeated_byte = *input
                .get(header.header_length)
                .ok_or(DecodeError::InputTooShort)?;
            Ok((
                LiteralSource::Rle {
                    byte: repeated_byte,
                    count: header.regenerated_size,
                },
                header.header_length + 1,
            ))
        }
        LiteralsType::Compressed => {
            let compressed_input = input
                .get(header.header_length..header.header_length + header.compressed_size)
                .ok_or(DecodeError::InputTooShort)?;
            let huffman_table_bytes =
                read_huffman_table(compressed_input, huffman_table, weight_fse_table)?;
            let stream_input = compressed_input
                .get(huffman_table_bytes..)
                .ok_or(DecodeError::InputTooShort)?;
            decode_streams(
                stream_count,
                stream_input,
                huffman_table,
                &mut *workspace,
                header.regenerated_size,
            )?;
            Ok((
                LiteralSource::Decoded(&workspace[..header.regenerated_size]),
                header.header_length + header.compressed_size,
            ))
        }
        LiteralsType::Treeless => {
            if !huffman_table.is_ready {
                return Err(DecodeError::BadLiteralsHeader);
            }
            let compressed_input = input
                .get(header.header_length..header.header_length + header.compressed_size)
                .ok_or(DecodeError::InputTooShort)?;
            decode_streams(
                stream_count,
                compressed_input,
                huffman_table,
                &mut *workspace,
                header.regenerated_size,
            )?;
            Ok((
                LiteralSource::Decoded(&workspace[..header.regenerated_size]),
                header.header_length + header.compressed_size,
            ))
        }
    }
}

pub struct LiteralSectionPair {
    pub first_count: usize,
    pub first_bytes_used: usize,
    pub second_count: usize,
    pub second_bytes_used: usize,
}

fn is_four_stream_huffman(header: &LiteralsHeader) -> bool {
    header.stream_count == 4
        && matches!(
            header.literals_type,
            LiteralsType::Compressed | LiteralsType::Treeless
        )
}

fn read_section_table<'input>(
    input: &'input [u8],
    header: &LiteralsHeader,
    table: &mut HuffmanDecodeTable,
    weight_fse_table: &mut FseDecodeTable,
) -> Result<&'input [u8], DecodeError> {
    if header.regenerated_size > MAX_BLOCK_SIZE {
        return Err(DecodeError::BadLiteralsHeader);
    }
    let compressed_input = input
        .get(header.header_length..header.header_length + header.compressed_size)
        .ok_or(DecodeError::InputTooShort)?;
    if header.literals_type == LiteralsType::Treeless {
        if !table.is_ready {
            return Err(DecodeError::BadLiteralsHeader);
        }
        return Ok(compressed_input);
    }
    let table_bytes = read_huffman_table(compressed_input, table, weight_fse_table)?;
    compressed_input
        .get(table_bytes..)
        .ok_or(DecodeError::InputTooShort)
}

/// # Safety
///
/// `second_output` must be valid for writes of `MAX_BLOCK_SIZE` bytes and must not overlap
/// `first_output` or either input.
pub unsafe fn decode_literal_sections_pair_unchecked(
    first_input: &[u8],
    second_input: &[u8],
    huffman_tables: &mut [HuffmanDecodeTable; 2],
    current_table: &mut usize,
    weight_fse_table: &mut FseDecodeTable,
    first_output: &mut [u8],
    second_output: *mut u8,
) -> Result<Option<LiteralSectionPair>, DecodeError> {
    let first_header = read_literals_header(first_input)?;
    let second_header = read_literals_header(second_input)?;
    if !is_four_stream_huffman(&first_header) || !is_four_stream_huffman(&second_header) {
        return Ok(None);
    }
    if first_header.regenerated_size > first_output.len() {
        return Err(DecodeError::OutputTooSmall);
    }
    let first_table = *current_table & 1;
    let first_streams = read_section_table(
        first_input,
        &first_header,
        &mut huffman_tables[first_table],
        weight_fse_table,
    )?;
    let second_table = if second_header.literals_type == LiteralsType::Compressed {
        1 - first_table
    } else {
        first_table
    };
    let second_streams = read_section_table(
        second_input,
        &second_header,
        &mut huffman_tables[second_table],
        weight_fse_table,
    )?;
    let (first_slices, first_segments) =
        split_streams(first_streams, 4, first_header.regenerated_size)?;
    let (second_slices, second_segments) =
        split_streams(second_streams, 4, second_header.regenerated_size)?;
    unsafe {
        decode_two_sections_unchecked(
            [
                first_slices[0],
                first_slices[1],
                first_slices[2],
                first_slices[3],
            ],
            &huffman_tables[first_table],
            first_output.as_mut_ptr(),
            [
                first_segments[0],
                first_segments[1],
                first_segments[2],
                first_segments[3],
            ],
            [
                second_slices[0],
                second_slices[1],
                second_slices[2],
                second_slices[3],
            ],
            &huffman_tables[second_table],
            second_output,
            [
                second_segments[0],
                second_segments[1],
                second_segments[2],
                second_segments[3],
            ],
        )?;
    }
    *current_table = second_table;
    Ok(Some(LiteralSectionPair {
        first_count: first_header.regenerated_size,
        first_bytes_used: first_header.header_length + first_header.compressed_size,
        second_count: second_header.regenerated_size,
        second_bytes_used: second_header.header_length + second_header.compressed_size,
    }))
}

fn resolve_stream_count(format: FrameFormat, header_stream_count: u8) -> usize {
    if header_stream_count == 1 {
        1
    } else {
        match format {
            FrameFormat::Zstd => 4,
            FrameFormat::Cosmoz => 8,
        }
    }
}

fn decode_streams(
    stream_count: usize,
    input: &[u8],
    huffman_table: &HuffmanDecodeTable,
    output: &mut [u8],
    regenerated_size: usize,
) -> Result<(), DecodeError> {
    if stream_count == 1 {
        let output_slice = output
            .get_mut(..regenerated_size)
            .ok_or(DecodeError::OutputTooSmall)?;
        decode_one_stream(input, huffman_table, output_slice)
    } else {
        decode_many_streams_with_slack(input, huffman_table, stream_count, output, regenerated_size)
    }
}

fn read_little_endian(input: &[u8], byte_count: usize) -> Result<u64, DecodeError> {
    let bytes = input.get(..byte_count).ok_or(DecodeError::InputTooShort)?;
    let mut value = 0u64;
    for (index, byte) in bytes.iter().enumerate() {
        value |= (*byte as u64) << (8 * index);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_raw_header_with_a_one_byte_five_bit_size() {
        let header = read_literals_header(&[0x50]).unwrap();
        assert_eq!(header.literals_type, LiteralsType::Raw);
        assert_eq!(header.regenerated_size, 10);
        assert_eq!(header.compressed_size, 10);
        assert_eq!(header.header_length, 1);
    }

    #[test]
    fn reads_raw_header_with_a_two_byte_twelve_bit_size() {
        let header = read_literals_header(&[0xC4, 0x12]).unwrap();
        assert_eq!(header.literals_type, LiteralsType::Raw);
        assert_eq!(header.regenerated_size, 300);
        assert_eq!(header.compressed_size, 300);
        assert_eq!(header.header_length, 2);
    }

    #[test]
    fn reads_compressed_header_format_one_with_four_ten_bit_streams() {
        let header = read_literals_header(&[0xC6, 0x2B, 0x4B]).unwrap();
        assert_eq!(header.literals_type, LiteralsType::Compressed);
        assert_eq!(header.regenerated_size, 700);
        assert_eq!(header.compressed_size, 300);
        assert_eq!(header.stream_count, 4);
        assert_eq!(header.header_length, 3);
    }

    #[test]
    fn reads_compressed_header_format_three_with_eighteen_bit_sizes() {
        let header = read_literals_header(&[0x0E, 0x6A, 0x18, 0xD4, 0x30]).unwrap();
        assert_eq!(header.literals_type, LiteralsType::Compressed);
        assert_eq!(header.regenerated_size, 100000);
        assert_eq!(header.compressed_size, 50000);
        assert_eq!(header.stream_count, 4);
        assert_eq!(header.header_length, 5);
    }

    #[test]
    fn decodes_raw_literals_as_a_slice_into_the_input() {
        let input = [0x50u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let mut huffman_table = HuffmanDecodeTable::new();
        let mut weight_fse_table = FseDecodeTable::new();
        let mut workspace = [0u8; 16];

        let (source, bytes_consumed) = decode_literals(
            &input,
            FrameFormat::Zstd,
            &mut huffman_table,
            &mut weight_fse_table,
            &mut workspace,
        )
        .unwrap();

        assert_eq!(bytes_consumed, 11);
        assert_eq!(source.len(), 10);
        match source {
            LiteralSource::Raw(bytes) => {
                assert_eq!(bytes, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
                assert_eq!(bytes.as_ptr(), input[1..].as_ptr());
            }
            _ => panic!("expected a raw literal source"),
        }
    }

    #[test]
    fn decodes_rle_literals_as_a_byte_and_a_count() {
        let header_byte = 1 | (5 << 3);
        let input = [header_byte, 0x42];
        let mut huffman_table = HuffmanDecodeTable::new();
        let mut weight_fse_table = FseDecodeTable::new();
        let mut workspace = [0u8; 8];

        let (source, bytes_consumed) = decode_literals(
            &input,
            FrameFormat::Zstd,
            &mut huffman_table,
            &mut weight_fse_table,
            &mut workspace,
        )
        .unwrap();

        assert_eq!(bytes_consumed, 2);
        assert_eq!(source.len(), 5);
        assert_eq!(
            source,
            LiteralSource::Rle {
                byte: 0x42,
                count: 5
            }
        );
    }

    #[test]
    fn treeless_literals_with_a_table_not_ready_is_rejected() {
        let header_byte = 3u8;
        let input = [header_byte, 0x00, 0x00];
        let mut huffman_table = HuffmanDecodeTable::new();
        let mut weight_fse_table = FseDecodeTable::new();
        let mut output = [0u8; 8];

        let result = decode_literals(
            &input,
            FrameFormat::Zstd,
            &mut huffman_table,
            &mut weight_fse_table,
            &mut output,
        );

        assert_eq!(result, Err(DecodeError::BadLiteralsHeader));
    }

    #[test]
    fn decodes_a_real_zstd_19_compressed_literals_section_with_fse_compressed_huffman_weights() {
        const LITERALS_SECTION: [u8; 338] = [
            0x06, 0xd9, 0x53, 0x1f, 0x10, 0xad, 0x07, 0x6f, 0x8a, 0x6e, 0xb3, 0x99, 0xb5, 0x95,
            0x27, 0xaa, 0x6c, 0xe3, 0x3d, 0xc8, 0x91, 0x36, 0xe1, 0xf2, 0xd6, 0xfe, 0x7c, 0x6c,
            0x40, 0x8a, 0x00, 0x00, 0x00, 0x00, 0x04, 0x4a, 0x00, 0x4b, 0x00, 0x4a, 0x00, 0xb9,
            0xd1, 0xca, 0xe9, 0x39, 0x9c, 0xa7, 0xdc, 0xd6, 0xbc, 0x5a, 0x5d, 0x4b, 0xdb, 0xcf,
            0xf2, 0x3b, 0xf4, 0xac, 0x58, 0x08, 0xee, 0xfd, 0xbf, 0xcc, 0x05, 0xaa, 0x0b, 0xb9,
            0x39, 0x30, 0x26, 0xbf, 0x41, 0x47, 0x67, 0xb6, 0x5a, 0xa1, 0x36, 0x2d, 0x9b, 0xa7,
            0x5c, 0xac, 0xa9, 0xa2, 0x40, 0xda, 0x4d, 0x2e, 0x15, 0x28, 0x18, 0x0f, 0xd2, 0x21,
            0xe6, 0x57, 0x92, 0xe1, 0xa9, 0x93, 0xaa, 0x80, 0xa0, 0xc9, 0x54, 0x41, 0x0c, 0x12,
            0xea, 0xd0, 0x1c, 0x0c, 0xb1, 0x69, 0xbb, 0x2f, 0x7c, 0x03, 0x39, 0x69, 0x06, 0xa7,
            0x4b, 0x0d, 0xfe, 0x5a, 0x75, 0xe9, 0xcb, 0xbd, 0xee, 0xf6, 0xe8, 0xeb, 0xa2, 0x5b,
            0x43, 0xb3, 0xd8, 0x3d, 0xe9, 0x5f, 0xd2, 0x83, 0x48, 0x6b, 0x44, 0x04, 0x12, 0x22,
            0x3b, 0xcd, 0x86, 0xcd, 0xb2, 0x96, 0x94, 0x03, 0x1a, 0x9e, 0x1c, 0xab, 0xba, 0xcd,
            0x72, 0x92, 0x8d, 0x39, 0x54, 0x07, 0x8e, 0x20, 0xc0, 0x4a, 0x24, 0x1e, 0xc4, 0x98,
            0x72, 0x70, 0x5f, 0x86, 0x86, 0x55, 0x0d, 0x16, 0xdf, 0xb9, 0x4f, 0xfe, 0x9f, 0x5f,
            0x75, 0x59, 0x88, 0x32, 0x12, 0x53, 0xea, 0xa8, 0x00, 0x3a, 0xf7, 0xee, 0x40, 0x27,
            0x36, 0x9e, 0x3b, 0x9d, 0x97, 0x34, 0xf8, 0x87, 0x7e, 0x06, 0x31, 0xa9, 0x38, 0xcc,
            0xf3, 0xb6, 0x72, 0xaf, 0x1b, 0xa0, 0x38, 0x3a, 0x26, 0xc5, 0xef, 0xd1, 0xf1, 0xcb,
            0x20, 0x91, 0xd1, 0x05, 0x2d, 0x2e, 0x00, 0x1f, 0x6a, 0xe5, 0xc8, 0xff, 0xad, 0x94,
            0x18, 0x31, 0x85, 0x6a, 0x0f, 0x71, 0x01, 0x33, 0xa6, 0x44, 0x3e, 0x0e, 0xdb, 0x47,
            0xa8, 0xce, 0xa6, 0xd5, 0x24, 0x53, 0x11, 0x99, 0xa7, 0xc7, 0x2f, 0x4a, 0x1c, 0x3e,
            0x04, 0x7c, 0x21, 0x38, 0xcb, 0x9c, 0xa4, 0xe5, 0x58, 0x83, 0x87, 0x48, 0x2f, 0xd7,
            0xb4, 0x21, 0xad, 0x4e, 0x59, 0x01, 0x0e, 0x98, 0xab, 0xba, 0x50, 0xf1, 0x5d, 0x97,
            0xac, 0xf0, 0xc6, 0xdd, 0x14, 0x1d, 0xa5, 0x70, 0x0a, 0x6c, 0x2e, 0xd9, 0x6c, 0x34,
            0xcf, 0xdc, 0x1d, 0x98, 0x01, 0x3f, 0xc2, 0xaf, 0x85, 0x12, 0x6e, 0x3a, 0xbc, 0x97,
            0x21, 0x50,
        ];
        const EXPECTED_TEXT: &[u8] = b"EqV8ib8HDy88YtDtXbiufMdI8X2Y4rUmer.BH3M1,XS0DRdJuPnBIKpi99lSi0tcL21pffWQJ,EeNajnez0LHtfROUrWW6Xn,I3EM3HMRb1OcWrhQ7TTJ chcVG6MOwUxOVHMWndqN,CIEPx3mnPQC4vkRB5ICpeyOxJRkSq1LI7S1L10e0tza93Ce6KRDiKpFfez3gb9pv,MEc0goRqG9hTCzppvEJqa ZgIFI2g8Pahqfpgi9el, OuOjSXXMUHyQ2pqaWkwfV6Wf3gV.O116cFBIj2C2qdPVHp7pWnOna8sEXflmWwdRpdo9KMleEnmhPxjExF6YGVYS1kW,E0u19tZtum.94xrIzsODL0IBNcI9WzwUEP9s19A7d9jZf7DEiBGEyHrxeGvfO";

        let header = read_literals_header(&LITERALS_SECTION).unwrap();
        assert_eq!(header.literals_type, LiteralsType::Compressed);
        assert_eq!(header.regenerated_size, 400);
        assert_eq!(header.compressed_size, 335);
        assert_eq!(header.stream_count, 4);
        assert_eq!(header.header_length, 3);
        assert_eq!(EXPECTED_TEXT.len(), header.regenerated_size);

        let mut huffman_table = HuffmanDecodeTable::new();
        let mut weight_fse_table = FseDecodeTable::new();
        let mut workspace = [0u8; 512];

        let (source, bytes_consumed) = decode_literals(
            &LITERALS_SECTION,
            FrameFormat::Zstd,
            &mut huffman_table,
            &mut weight_fse_table,
            &mut workspace,
        )
        .unwrap();

        assert_eq!(bytes_consumed, LITERALS_SECTION.len());
        assert_eq!(source.len(), 400);
        match source {
            LiteralSource::Decoded(bytes) => assert_eq!(bytes, EXPECTED_TEXT),
            _ => panic!("expected a decoded literal source"),
        }
    }
}
