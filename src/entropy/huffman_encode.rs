use core::ptr;

use crate::{encode_error::EncodeError, entropy::huffman_encode_table::HuffmanEncodeTable};

const MAX_STREAM_COUNT: usize = 8;

pub(crate) fn encode_stream_capacity(input_length: usize) -> usize {
    input_length * 11 / 8 + 16
}

pub(crate) fn encode_one_stream(
    input: &[u8],
    table: &HuffmanEncodeTable,
    output: &mut [u8],
) -> Result<usize, EncodeError> {
    let required = encode_stream_capacity(input.len());
    if output.len() < required {
        return Err(EncodeError::OutputTooSmall);
    }
    Ok(unsafe { encode_stream_unchecked(input, table, output.as_mut_ptr(), output.len()) })
}

fn pick_unroll_count(max_bits: u8) -> usize {
    if max_bits == 0 {
        return 4;
    }
    let max_bits = max_bits as usize;
    (56 / max_bits).clamp(4, 9)
}

unsafe fn encode_stream_unchecked(
    input: &[u8],
    table: &HuffmanEncodeTable,
    output_pointer: *mut u8,
    output_capacity: usize,
) -> usize {
    debug_assert!(output_capacity >= encode_stream_capacity(input.len()));
    unsafe {
        match pick_unroll_count(table.max_bits) {
            9 => {
                encode_stream_unrolled_unchecked::<9>(input, table, output_pointer, output_capacity)
            }
            8 => {
                encode_stream_unrolled_unchecked::<8>(input, table, output_pointer, output_capacity)
            }
            7 => {
                encode_stream_unrolled_unchecked::<7>(input, table, output_pointer, output_capacity)
            }
            6 => {
                encode_stream_unrolled_unchecked::<6>(input, table, output_pointer, output_capacity)
            }
            5 => {
                encode_stream_unrolled_unchecked::<5>(input, table, output_pointer, output_capacity)
            }
            _ => {
                encode_stream_unrolled_unchecked::<4>(input, table, output_pointer, output_capacity)
            }
        }
    }
}

unsafe fn encode_stream_unrolled_unchecked<const UNROLL: usize>(
    input: &[u8],
    table: &HuffmanEncodeTable,
    output_pointer: *mut u8,
    output_capacity: usize,
) -> usize {
    debug_assert!(output_capacity >= encode_stream_capacity(input.len()));
    debug_assert!(UNROLL * table.max_bits as usize + 7 < 64);
    let length = input.len();
    let codes = &table.codes;

    let mut container: u64 = 0;
    let mut bits_in_container: u32 = 0;
    let mut write_position: usize = 0;
    let mut index = length;

    let remainder = length % UNROLL;
    for _ in 0..remainder {
        index -= 1;
        unsafe {
            let symbol = *input.get_unchecked(index);
            let code = codes.get_unchecked(symbol as usize);
            container |= (code.code as u64) << bits_in_container;
            bits_in_container += code.bit_count as u32;
        }
    }
    if remainder > 0 {
        unsafe {
            flush_container_unchecked(
                output_pointer,
                output_capacity,
                &mut write_position,
                &mut container,
                &mut bits_in_container,
            );
        }
    }

    while index > 0 {
        index -= UNROLL;
        unsafe {
            for offset in (0..UNROLL).rev() {
                let symbol = *input.get_unchecked(index + offset);
                let code = codes.get_unchecked(symbol as usize);
                container |= (code.code as u64) << bits_in_container;
                bits_in_container += code.bit_count as u32;
            }
            flush_container_unchecked(
                output_pointer,
                output_capacity,
                &mut write_position,
                &mut container,
                &mut bits_in_container,
            );
        }
    }

    container |= 1u64 << bits_in_container;
    bits_in_container += 1;
    unsafe {
        flush_container_unchecked(
            output_pointer,
            output_capacity,
            &mut write_position,
            &mut container,
            &mut bits_in_container,
        );
        if bits_in_container > 0 {
            debug_assert!(write_position < output_capacity);
            *output_pointer.add(write_position) = container as u8;
            write_position += 1;
        }
    }

    write_position
}

unsafe fn flush_container_unchecked(
    output_pointer: *mut u8,
    output_capacity: usize,
    write_position: &mut usize,
    container: &mut u64,
    bits_in_container: &mut u32,
) {
    debug_assert!(*bits_in_container < 64);
    debug_assert!(*write_position + 8 <= output_capacity);
    let byte_count = (*bits_in_container / 8) as usize;
    unsafe {
        ptr::write_unaligned(
            output_pointer.add(*write_position) as *mut u64,
            container.to_le(),
        );
    }
    *write_position += byte_count;
    let bit_count = byte_count as u32 * 8;
    *bits_in_container -= bit_count;
    *container = if bit_count >= 64 {
        0
    } else {
        *container >> bit_count
    };
}

pub(crate) fn encode_many_streams(
    input: &[u8],
    table: &HuffmanEncodeTable,
    stream_count: usize,
    output: &mut [u8],
) -> Result<usize, EncodeError> {
    if stream_count == 0 || stream_count > MAX_STREAM_COUNT {
        return Err(EncodeError::BadOptions);
    }
    let segment_size = get_segment_size(input.len(), stream_count);
    let jump_table_size = (stream_count - 1) * 2;
    if output.len() < jump_table_size {
        return Err(EncodeError::OutputTooSmall);
    }
    let mut stream_sizes = [0u16; MAX_STREAM_COUNT];

    let mut position = jump_table_size;
    let mut input_position = 0usize;
    for (stream_index, stream_size_slot) in stream_sizes[..stream_count].iter_mut().enumerate() {
        let is_last_stream = stream_index == stream_count - 1;
        let segment_end = if is_last_stream {
            input.len()
        } else {
            (input_position + segment_size).min(input.len())
        };
        let segment = &input[input_position..segment_end];

        let stream_output = output
            .get_mut(position..)
            .ok_or(EncodeError::OutputTooSmall)?;
        let stream_size = encode_one_stream(segment, table, stream_output)?;
        if !is_last_stream {
            if stream_size > u16::MAX as usize {
                return Err(EncodeError::TableNotUsable);
            }
            *stream_size_slot = stream_size as u16;
        }
        position += stream_size;
        input_position = segment_end;
    }

    let jump_table = &mut output[..jump_table_size];
    for (stream_index, &size) in stream_sizes[..stream_count - 1].iter().enumerate() {
        let offset = stream_index * 2;
        jump_table[offset..offset + 2].copy_from_slice(&size.to_le_bytes());
    }

    Ok(position)
}

fn get_segment_size(input_length: usize, stream_count: usize) -> usize {
    input_length.div_ceil(stream_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::{
        fse_decode_table::FseDecodeTable,
        histogram::count_symbols,
        huffman_decode::{decode_many_streams, decode_one_stream},
        huffman_decode_table::{HuffmanDecodeTable, read_huffman_table},
        huffman_encode_table::{build_huffman_encode_table, write_direct_weights},
    };

    fn sample_text() -> Vec<u8> {
        let mut text = Vec::new();
        while text.len() < 300 {
            text.extend_from_slice(
                b"the quick brown fox jumps over the lazy dog while the sun sets slowly",
            );
        }
        text.truncate(300);
        text
    }

    fn build_encode_table(input: &[u8]) -> HuffmanEncodeTable {
        let mut counts = [0u32; 256];
        count_symbols(input, &mut counts);
        let mut table = HuffmanEncodeTable::new();
        build_huffman_encode_table(&counts, &mut table).unwrap();
        table
    }

    fn build_decode_table(encode_table: &HuffmanEncodeTable) -> HuffmanDecodeTable {
        let mut weights_output = [0u8; 256];
        let bytes_written = write_direct_weights(&mut weights_output, encode_table).unwrap();
        let mut decode_table = HuffmanDecodeTable::new();
        let mut weight_fse_table = FseDecodeTable::new();
        read_huffman_table(
            &weights_output[..bytes_written],
            &mut decode_table,
            &mut weight_fse_table,
        )
        .unwrap();
        decode_table
    }

    #[test]
    fn encodes_and_decodes_one_stream() {
        let text = sample_text();
        let encode_table = build_encode_table(&text);
        let decode_table = build_decode_table(&encode_table);

        let mut encoded = [0u8; 1024];
        let bytes_written = encode_one_stream(&text, &encode_table, &mut encoded).unwrap();

        let mut decoded = vec![0u8; text.len()];
        decode_one_stream(&encoded[..bytes_written], &decode_table, &mut decoded).unwrap();

        assert_eq!(decoded, text);
    }

    #[test]
    fn encodes_and_decodes_four_streams() {
        let text = sample_text();
        let encode_table = build_encode_table(&text);
        let decode_table = build_decode_table(&encode_table);

        let mut encoded = [0u8; 2048];
        let bytes_written = encode_many_streams(&text, &encode_table, 4, &mut encoded).unwrap();

        let mut decoded = vec![0u8; text.len()];
        decode_many_streams(&encoded[..bytes_written], &decode_table, 4, &mut decoded).unwrap();

        assert_eq!(decoded, text);
    }

    struct XorshiftRandom {
        state: u64,
    }

    impl XorshiftRandom {
        fn new(seed: u64) -> Self {
            Self { state: seed | 1 }
        }

        fn next_u64(&mut self) -> u64 {
            self.state ^= self.state << 13;
            self.state ^= self.state >> 7;
            self.state ^= self.state << 17;
            self.state
        }

        fn next_range(&mut self, bound: usize) -> usize {
            (self.next_u64() % bound as u64) as usize
        }
    }

    fn reference_encode_one_stream(
        input: &[u8],
        table: &HuffmanEncodeTable,
        output: &mut [u8],
    ) -> Result<usize, EncodeError> {
        use crate::bits::backward_bit_writer::BackwardBitWriter;

        let mut writer = BackwardBitWriter::new(output);
        for &symbol in input.iter().rev() {
            let code = table.codes[symbol as usize];
            writer.add_bits(code.code as u64, code.bit_count as usize)?;
        }
        writer.finish()
    }

    #[test]
    fn fast_stream_matches_reference_encoder_and_round_trips_for_random_tables_and_inputs() {
        let mut random = XorshiftRandom::new(0xD1B54A32D192ED03);

        for _ in 0..200 {
            let mut counts = [0u32; 256];
            let used_symbol_count = 2 + random.next_range(127);
            let mut used_symbols = Vec::new();
            while used_symbols.len() < used_symbol_count {
                let symbol = random.next_range(128) as u8;
                if !used_symbols.contains(&symbol) {
                    used_symbols.push(symbol);
                }
            }
            for &symbol in &used_symbols {
                counts[symbol as usize] = 1 + random.next_range(500) as u32;
            }

            let mut table = HuffmanEncodeTable::new();
            build_huffman_encode_table(&counts, &mut table).unwrap();

            let input_length = random.next_range(5001);
            let mut input = Vec::with_capacity(input_length);
            for _ in 0..input_length {
                let symbol_index = random.next_range(used_symbols.len());
                input.push(used_symbols[symbol_index]);
            }

            let mut fast_output = vec![0u8; encode_stream_capacity(input.len())];
            let fast_bytes = encode_one_stream(&input, &table, &mut fast_output).unwrap();

            let mut reference_output = vec![0u8; encode_stream_capacity(input.len()) + 64];
            let reference_bytes =
                reference_encode_one_stream(&input, &table, &mut reference_output).unwrap();

            assert_eq!(
                &fast_output[..fast_bytes],
                &reference_output[..reference_bytes]
            );

            let decode_table = build_decode_table(&table);
            let mut decoded = vec![0u8; input.len()];
            decode_one_stream(&fast_output[..fast_bytes], &decode_table, &mut decoded).unwrap();
            assert_eq!(decoded, input);
        }
    }

    #[test]
    fn encodes_and_decodes_eight_streams() {
        let text = sample_text();
        let encode_table = build_encode_table(&text);
        let decode_table = build_decode_table(&encode_table);

        let mut encoded = [0u8; 2048];
        let bytes_written = encode_many_streams(&text, &encode_table, 8, &mut encoded).unwrap();

        let mut decoded = vec![0u8; text.len()];
        decode_many_streams(&encoded[..bytes_written], &decode_table, 8, &mut decoded).unwrap();

        assert_eq!(decoded, text);
    }
}
