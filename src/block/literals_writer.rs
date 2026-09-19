use crate::{
    block::literals::LiteralsType,
    encode_error::EncodeError,
    entropy::{
        fse_encode_table::FseEncodeTable,
        histogram::count_symbols,
        huffman_encode::{encode_four_streams, encode_one_stream},
        huffman_encode_table::{
            HuffmanEncodeTable, build_huffman_encode_table, write_huffman_table,
        },
    },
};

pub(crate) const ONE_STREAM_MAX_SIZE: usize = 1023;
pub(crate) const MULTI_STREAM_MIN_SIZE: usize = 256;

pub(crate) fn write_literals(
    input: &[u8],
    output: &mut [u8],
    huffman_table: &mut HuffmanEncodeTable,
    weight_fse_table: &mut FseEncodeTable,
    table_reuse_allowed: bool,
    known_counts: Option<&[u32; 256]>,
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
        output,
        huffman_table,
        weight_fse_table,
        table_reuse_allowed,
        known_counts,
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

fn estimate_huffman_bit_cost(counts: &[u32; 256], table: &HuffmanEncodeTable) -> Option<usize> {
    let mut total_bits = 0usize;
    for (symbol, &count) in counts.iter().enumerate() {
        if count == 0 {
            continue;
        }
        let bit_count = table.codes[symbol].bit_count;
        if bit_count == 0 {
            return None;
        }
        total_bits += count as usize * bit_count as usize;
    }
    Some(total_bits)
}

fn write_compressed_literals(
    input: &[u8],
    output: &mut [u8],
    huffman_table: &mut HuffmanEncodeTable,
    weight_fse_table: &mut FseEncodeTable,
    table_reuse_allowed: bool,
    known_counts: Option<&[u32; 256]>,
) -> Result<usize, EncodeError> {
    let counts = match known_counts {
        Some(known_counts) => *known_counts,
        None => {
            let mut counts = [0u32; 256];
            count_symbols(input, &mut counts);
            counts
        }
    };

    let treeless_bit_cost = if table_reuse_allowed {
        estimate_huffman_bit_cost(&counts, huffman_table)
    } else {
        None
    };

    let mut candidate_table = HuffmanEncodeTable::new();
    build_huffman_encode_table(&counts, &mut candidate_table)?;
    let candidate_bit_cost =
        estimate_huffman_bit_cost(&counts, &candidate_table).ok_or(EncodeError::TableNotUsable)?;

    let mut table_description_scratch = [0u8; 512];
    let candidate_table_bytes = write_huffman_table(
        &mut table_description_scratch,
        &candidate_table,
        weight_fse_table,
    )?;

    let use_treeless = match treeless_bit_cost {
        Some(treeless_bits) => treeless_bits < candidate_bit_cost + candidate_table_bytes * 8,
        None => false,
    };

    let regenerated_size = input.len();
    let stream_count = pick_stream_count(regenerated_size);
    let size_format = pick_size_format_from_regenerated_size(stream_count, regenerated_size)
        .ok_or(EncodeError::TableNotUsable)?;
    let literals_type = if use_treeless {
        LiteralsType::Treeless
    } else {
        LiteralsType::Compressed
    };
    let header_length = literals_header_length(literals_type, size_format);

    let body = output
        .get_mut(header_length..)
        .ok_or(EncodeError::OutputTooSmall)?;

    let table_bytes = if use_treeless {
        0
    } else {
        let destination = body
            .get_mut(..candidate_table_bytes)
            .ok_or(EncodeError::OutputTooSmall)?;
        destination.copy_from_slice(&table_description_scratch[..candidate_table_bytes]);
        candidate_table_bytes
    };

    let encode_table: &HuffmanEncodeTable = if use_treeless {
        &*huffman_table
    } else {
        &candidate_table
    };

    let stream_output = body
        .get_mut(table_bytes..)
        .ok_or(EncodeError::OutputTooSmall)?;
    let stream_bytes = if stream_count == 1 {
        encode_one_stream(input, encode_table, stream_output)?
    } else {
        encode_four_streams(input, encode_table, stream_output)?
    };

    let compressed_size = table_bytes + stream_bytes;
    let size_bits = literals_size_bits(size_format);
    if compressed_size >= (1usize << size_bits) {
        return Err(EncodeError::TableNotUsable);
    }

    let total_size = header_length + compressed_size;

    let raw_size_format = pick_raw_size_format(regenerated_size)?;
    let raw_header_length = literals_header_length(LiteralsType::Raw, raw_size_format);
    let raw_total_size = raw_header_length + regenerated_size;

    if total_size >= raw_total_size {
        return Err(EncodeError::TableNotUsable);
    }

    if !use_treeless {
        *huffman_table = candidate_table;
    }

    write_literals_header(
        output,
        literals_type,
        size_format,
        regenerated_size,
        compressed_size,
    )?;
    Ok(total_size)
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

fn pick_stream_count(length: usize) -> usize {
    if length < MULTI_STREAM_MIN_SIZE {
        return 1;
    }
    let segment = length.div_ceil(4);
    if 3 * segment > length { 1 } else { 4 }
}

fn pick_size_format_from_regenerated_size(
    stream_count: usize,
    regenerated_size: usize,
) -> Option<u8> {
    if stream_count == 1 {
        if regenerated_size <= ONE_STREAM_MAX_SIZE {
            Some(0)
        } else {
            None
        }
    } else {
        const LIMITS: [(u8, usize); 3] = [(1, 1023), (2, 16383), (3, 262143)];
        for &(candidate_format, limit) in LIMITS.iter() {
            if regenerated_size <= limit {
                return Some(candidate_format);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        block::literals::{LiteralSource, decode_literals, read_literals_header},
        entropy::{fse_decode_table::FseDecodeTable, huffman_decode_table::HuffmanDecodeTable},
    };

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

    fn round_trip(input: &[u8]) -> Vec<u8> {
        let mut huffman_encode_table = HuffmanEncodeTable::new();
        let mut weight_fse_table = FseEncodeTable::new();
        let mut output = vec![0u8; input.len() * 2 + 1024];

        let bytes_written = write_literals(
            input,
            &mut output,
            &mut huffman_encode_table,
            &mut weight_fse_table,
            false,
            None,
        )
        .unwrap();

        let mut huffman_decode_table = HuffmanDecodeTable::new();
        let mut weight_fse_table = FseDecodeTable::new();
        let mut decoded = vec![0u8; input.len()];

        let (source, bytes_consumed) = decode_literals(
            &output[..bytes_written],
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
            LiteralSource::InOutput { .. } => unreachable!(),
        }
    }

    #[test]
    fn round_trips_a_hundred_bytes_with_one_stream() {
        let input = repeating_text(100);
        let decoded = round_trip(&input);
        assert_eq!(decoded, input);
    }

    #[test]
    fn round_trips_twenty_kilobytes_as_zstd_with_four_streams() {
        let input = repeating_text(20 * 1024);

        let stream_count = pick_stream_count(input.len());
        assert_eq!(stream_count, 4);
        assert_eq!((stream_count - 1) * 2, 6);

        let mut huffman_encode_table = HuffmanEncodeTable::new();
        let mut weight_fse_table = FseEncodeTable::new();
        let mut output = vec![0u8; input.len() * 2 + 1024];
        let bytes_written = write_literals(
            &input,
            &mut output,
            &mut huffman_encode_table,
            &mut weight_fse_table,
            false,
            None,
        )
        .unwrap();

        let header = read_literals_header(&output[..bytes_written]).unwrap();
        assert_eq!(header.literals_type, LiteralsType::Compressed);
        let size_format = (output[0] >> 2) & 0b11;
        assert_ne!(size_format, 0);

        let decoded = round_trip(&input);
        assert_eq!(decoded, input);
    }

    #[test]
    fn writes_rle_for_five_kilobytes_of_one_byte() {
        let input = vec![0x37u8; 5 * 1024];
        let decoded = round_trip(&input);
        assert_eq!(decoded, input);

        let mut huffman_encode_table = HuffmanEncodeTable::new();
        let mut weight_fse_table = FseEncodeTable::new();
        let mut output = vec![0u8; input.len() * 2];
        let bytes_written = write_literals(
            &input,
            &mut output,
            &mut huffman_encode_table,
            &mut weight_fse_table,
            false,
            None,
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

        let decoded = round_trip(&input);
        assert_eq!(decoded, input);

        let mut huffman_encode_table = HuffmanEncodeTable::new();
        let mut weight_fse_table = FseEncodeTable::new();
        let mut output = vec![0u8; input.len() * 2];
        let bytes_written = write_literals(
            &input,
            &mut output,
            &mut huffman_encode_table,
            &mut weight_fse_table,
            false,
            None,
        )
        .unwrap();
        let header = read_literals_header(&output[..bytes_written]).unwrap();
        assert_eq!(header.literals_type, LiteralsType::Raw);
    }
}
