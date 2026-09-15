use crate::block::literals::LiteralsType;
use crate::encode_error::EncodeError;
use crate::entropy::fse_encode_table::FseEncodeTable;
use crate::entropy::histogram::count_symbols;
use crate::entropy::huffman_encode::{encode_many_streams, encode_one_stream};
use crate::entropy::huffman_encode_table::{
    HuffmanEncodeTable, build_huffman_encode_table, write_huffman_table,
};
use crate::frame::frame_header::FrameFormat;

pub const ONE_STREAM_MAX_SIZE: usize = 1023;
pub const MULTI_STREAM_MIN_SIZE: usize = 256;

pub fn write_literals(
    input: &[u8],
    format: FrameFormat,
    output: &mut [u8],
    huffman_table: &mut HuffmanEncodeTable,
    weight_fse_table: &mut FseEncodeTable,
    scratch: &mut [u8],
) -> Result<usize, EncodeError> {
    if input.is_empty() {
        return write_raw_literals(input, output);
    }

    let first_byte = input[0];
    if input.iter().all(|&byte| byte == first_byte) {
        return write_rle_literals(first_byte, input.len(), output);
    }

    match write_compressed_literals(
        input,
        format,
        output,
        huffman_table,
        weight_fse_table,
        scratch,
    ) {
        Ok(written) => Ok(written),
        Err(_) => write_raw_literals(input, output),
    }
}

fn write_raw_literals(input: &[u8], output: &mut [u8]) -> Result<usize, EncodeError> {
    let size_format = pick_raw_size_format(input.len())?;
    let header_length = write_literals_header(
        output,
        LiteralsType::Raw,
        size_format,
        input.len(),
        input.len(),
    )?;
    let total_size = header_length
        .checked_add(input.len())
        .ok_or(EncodeError::OutputTooSmall)?;
    let destination = output
        .get_mut(header_length..total_size)
        .ok_or(EncodeError::OutputTooSmall)?;
    destination.copy_from_slice(input);
    Ok(total_size)
}

fn write_rle_literals(byte: u8, length: usize, output: &mut [u8]) -> Result<usize, EncodeError> {
    let size_format = pick_raw_size_format(length)?;
    let header_length = write_literals_header(output, LiteralsType::Rle, size_format, length, 1)?;
    let total_size = header_length
        .checked_add(1)
        .ok_or(EncodeError::OutputTooSmall)?;
    let destination = output
        .get_mut(header_length..total_size)
        .ok_or(EncodeError::OutputTooSmall)?;
    destination[0] = byte;
    Ok(total_size)
}

fn write_compressed_literals(
    input: &[u8],
    format: FrameFormat,
    output: &mut [u8],
    huffman_table: &mut HuffmanEncodeTable,
    weight_fse_table: &mut FseEncodeTable,
    scratch: &mut [u8],
) -> Result<usize, EncodeError> {
    let mut counts = [0u32; 256];
    count_symbols(input, &mut counts);
    build_huffman_encode_table(&counts, huffman_table)?;

    let table_bytes = write_huffman_table(scratch, huffman_table, weight_fse_table)?;

    let stream_count = pick_stream_count(format, input.len());
    let stream_output = scratch
        .get_mut(table_bytes..)
        .ok_or(EncodeError::OutputTooSmall)?;
    let stream_bytes = if stream_count == 1 {
        encode_one_stream(input, huffman_table, stream_output)?
    } else {
        encode_many_streams(input, huffman_table, stream_count, stream_output)?
    };

    let regenerated_size = input.len();
    let compressed_size = table_bytes + stream_bytes;

    let size_format = pick_size_format(stream_count, regenerated_size, compressed_size)
        .ok_or(EncodeError::TableNotUsable)?;

    let header_length = literals_header_length(LiteralsType::Compressed, size_format);
    let total_size = header_length + compressed_size;

    let raw_size_format = pick_raw_size_format(regenerated_size)?;
    let raw_header_length = literals_header_length(LiteralsType::Raw, raw_size_format);
    let raw_total_size = raw_header_length + regenerated_size;

    if total_size >= raw_total_size {
        return Err(EncodeError::TableNotUsable);
    }

    let written_header_length = write_literals_header(
        output,
        LiteralsType::Compressed,
        size_format,
        regenerated_size,
        compressed_size,
    )?;
    let destination_end = written_header_length
        .checked_add(compressed_size)
        .ok_or(EncodeError::OutputTooSmall)?;
    let destination = output
        .get_mut(written_header_length..destination_end)
        .ok_or(EncodeError::OutputTooSmall)?;
    destination.copy_from_slice(&scratch[..compressed_size]);
    Ok(destination_end)
}

fn write_literals_header(
    output: &mut [u8],
    literals_type: LiteralsType,
    size_format: u8,
    regenerated_size: usize,
    compressed_size: usize,
) -> Result<usize, EncodeError> {
    let type_bits: u64 = match literals_type {
        LiteralsType::Raw => 0,
        LiteralsType::Rle => 1,
        LiteralsType::Compressed => 2,
        LiteralsType::Treeless => 3,
    };
    let header_length = literals_header_length(literals_type, size_format);

    let value: u64 = match literals_type {
        LiteralsType::Raw | LiteralsType::Rle => match size_format {
            0 => type_bits | ((regenerated_size as u64) << 3),
            1 => type_bits | (1u64 << 2) | ((regenerated_size as u64) << 4),
            _ => type_bits | (3u64 << 2) | ((regenerated_size as u64) << 4),
        },
        LiteralsType::Compressed | LiteralsType::Treeless => {
            let size_bits = literals_size_bits(size_format);
            type_bits
                | ((size_format as u64) << 2)
                | ((regenerated_size as u64) << 4)
                | ((compressed_size as u64) << (4 + size_bits))
        }
    };

    let destination = output
        .get_mut(..header_length)
        .ok_or(EncodeError::OutputTooSmall)?;
    for (index, byte) in destination.iter_mut().enumerate() {
        *byte = ((value >> (8 * index)) & 0xFF) as u8;
    }
    Ok(header_length)
}

fn literals_header_length(literals_type: LiteralsType, size_format: u8) -> usize {
    match literals_type {
        LiteralsType::Raw | LiteralsType::Rle => match size_format {
            0 => 1,
            1 => 2,
            _ => 3,
        },
        LiteralsType::Compressed | LiteralsType::Treeless => match size_format {
            0 | 1 => 3,
            2 => 4,
            _ => 5,
        },
    }
}

fn literals_size_bits(size_format: u8) -> u32 {
    match size_format {
        0 | 1 => 10,
        2 => 14,
        _ => 18,
    }
}

fn pick_raw_size_format(length: usize) -> Result<u8, EncodeError> {
    if length < 32 {
        Ok(0)
    } else if length < 4096 {
        Ok(1)
    } else if length < (1 << 20) {
        Ok(3)
    } else {
        Err(EncodeError::InputTooLarge)
    }
}

fn pick_stream_count(format: FrameFormat, length: usize) -> usize {
    if length < MULTI_STREAM_MIN_SIZE {
        return 1;
    }
    let stream_count = match format {
        FrameFormat::Zstd => 4,
        FrameFormat::Osmo => 8,
    };
    let segment = length.div_ceil(stream_count);
    if (stream_count - 1) * segment > length {
        1
    } else {
        stream_count
    }
}

fn pick_size_format(
    stream_count: usize,
    regenerated_size: usize,
    compressed_size: usize,
) -> Option<u8> {
    if stream_count == 1 {
        if regenerated_size <= ONE_STREAM_MAX_SIZE && compressed_size <= ONE_STREAM_MAX_SIZE {
            Some(0)
        } else {
            None
        }
    } else {
        const LIMITS: [(u8, usize); 3] = [(1, 1023), (2, 16383), (3, 262143)];
        for &(candidate_format, limit) in LIMITS.iter() {
            if regenerated_size <= limit && compressed_size <= limit {
                return Some(candidate_format);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::literals::{LiteralSource, decode_literals, read_literals_header};
    use crate::entropy::fse_decode_table::FseDecodeTable;
    use crate::entropy::huffman_decode_table::HuffmanDecodeTable;

    struct XorshiftRandom {
        state: u64,
    }

    impl XorshiftRandom {
        fn new(seed: u64) -> Self {
            Self { state: seed }
        }

        fn next_u64(&mut self) -> u64 {
            self.state ^= self.state << 13;
            self.state ^= self.state >> 7;
            self.state ^= self.state << 17;
            self.state
        }
    }

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

    fn round_trip(input: &[u8], format: FrameFormat) -> Vec<u8> {
        let mut huffman_encode_table = HuffmanEncodeTable::new();
        let mut weight_fse_table = FseEncodeTable::new();
        let mut scratch = vec![0u8; input.len() * 2 + 1024];
        let mut output = vec![0u8; input.len() * 2 + 1024];

        let bytes_written = write_literals(
            input,
            format,
            &mut output,
            &mut huffman_encode_table,
            &mut weight_fse_table,
            &mut scratch,
        )
        .unwrap();

        let mut huffman_decode_table = HuffmanDecodeTable::new();
        let mut weight_fse_table = FseDecodeTable::new();
        let mut decoded = vec![0u8; input.len()];

        let (source, bytes_consumed) = decode_literals(
            &output[..bytes_written],
            format,
            &mut huffman_decode_table,
            &mut weight_fse_table,
            &mut decoded,
        )
        .unwrap();

        assert_eq!(bytes_consumed, bytes_written);
        assert_eq!(source.len(), input.len());
        match source {
            LiteralSource::Raw(bytes) => bytes.to_vec(),
            LiteralSource::Decoded(bytes) => bytes.to_vec(),
            LiteralSource::Rle { byte, count } => vec![byte; count],
        }
    }

    #[test]
    fn round_trips_a_hundred_bytes_with_one_stream() {
        let input = repeating_text(100);
        let decoded = round_trip(&input, FrameFormat::Zstd);
        assert_eq!(decoded, input);
    }

    #[test]
    fn round_trips_twenty_kilobytes_as_zstd_with_four_streams() {
        let input = repeating_text(20 * 1024);

        let stream_count = pick_stream_count(FrameFormat::Zstd, input.len());
        assert_eq!(stream_count, 4);
        assert_eq!((stream_count - 1) * 2, 6);

        let mut huffman_encode_table = HuffmanEncodeTable::new();
        let mut weight_fse_table = FseEncodeTable::new();
        let mut scratch = vec![0u8; input.len() * 2 + 1024];
        let mut output = vec![0u8; input.len() * 2 + 1024];
        let bytes_written = write_literals(
            &input,
            FrameFormat::Zstd,
            &mut output,
            &mut huffman_encode_table,
            &mut weight_fse_table,
            &mut scratch,
        )
        .unwrap();

        let header = read_literals_header(&output[..bytes_written]).unwrap();
        assert_eq!(header.literals_type, LiteralsType::Compressed);
        let size_format = (output[0] >> 2) & 0b11;
        assert_ne!(size_format, 0);

        let decoded = round_trip(&input, FrameFormat::Zstd);
        assert_eq!(decoded, input);
    }

    #[test]
    fn round_trips_twenty_kilobytes_as_osmo_with_eight_streams() {
        let input = repeating_text(20 * 1024);

        let stream_count = pick_stream_count(FrameFormat::Osmo, input.len());
        assert_eq!(stream_count, 8);
        assert_eq!((stream_count - 1) * 2, 14);

        let mut huffman_encode_table = HuffmanEncodeTable::new();
        let mut weight_fse_table = FseEncodeTable::new();
        let mut scratch = vec![0u8; input.len() * 2 + 1024];
        let mut output = vec![0u8; input.len() * 2 + 1024];
        let bytes_written = write_literals(
            &input,
            FrameFormat::Osmo,
            &mut output,
            &mut huffman_encode_table,
            &mut weight_fse_table,
            &mut scratch,
        )
        .unwrap();

        let header = read_literals_header(&output[..bytes_written]).unwrap();
        assert_eq!(header.literals_type, LiteralsType::Compressed);
        let size_format = (output[0] >> 2) & 0b11;
        assert_ne!(size_format, 0);

        let decoded = round_trip(&input, FrameFormat::Osmo);
        assert_eq!(decoded, input);
    }

    #[test]
    fn writes_rle_for_five_kilobytes_of_one_byte() {
        let input = vec![0x37u8; 5 * 1024];
        let decoded = round_trip(&input, FrameFormat::Zstd);
        assert_eq!(decoded, input);

        let mut huffman_encode_table = HuffmanEncodeTable::new();
        let mut weight_fse_table = FseEncodeTable::new();
        let mut scratch = vec![0u8; input.len() * 2];
        let mut output = vec![0u8; input.len() * 2];
        let bytes_written = write_literals(
            &input,
            FrameFormat::Zstd,
            &mut output,
            &mut huffman_encode_table,
            &mut weight_fse_table,
            &mut scratch,
        )
        .unwrap();
        let header = read_literals_header(&output[..bytes_written]).unwrap();
        assert_eq!(header.literals_type, LiteralsType::Rle);
    }

    #[test]
    fn writes_raw_for_five_kilobytes_of_random_bytes() {
        let mut random = XorshiftRandom::new(0x2545F4914F6CDD1D);
        let mut input = vec![0u8; 5 * 1024];
        for byte in input.iter_mut() {
            *byte = (random.next_u64() & 0xFF) as u8;
        }

        let decoded = round_trip(&input, FrameFormat::Zstd);
        assert_eq!(decoded, input);

        let mut huffman_encode_table = HuffmanEncodeTable::new();
        let mut weight_fse_table = FseEncodeTable::new();
        let mut scratch = vec![0u8; input.len() * 2];
        let mut output = vec![0u8; input.len() * 2];
        let bytes_written = write_literals(
            &input,
            FrameFormat::Zstd,
            &mut output,
            &mut huffman_encode_table,
            &mut weight_fse_table,
            &mut scratch,
        )
        .unwrap();
        let header = read_literals_header(&output[..bytes_written]).unwrap();
        assert_eq!(header.literals_type, LiteralsType::Raw);
    }
}
