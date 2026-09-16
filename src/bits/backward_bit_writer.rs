use crate::{bits::forward_bit_writer::ForwardBitWriter, encode_error::EncodeError};

pub struct BackwardBitWriter<'output> {
    bit_writer: ForwardBitWriter<'output>,
}

impl<'output> BackwardBitWriter<'output> {
    pub fn new(output: &'output mut [u8]) -> Self {
        Self {
            bit_writer: ForwardBitWriter::new(output),
        }
    }

    pub fn add_bits(&mut self, value: u64, count: usize) -> Result<(), EncodeError> {
        self.bit_writer.add_bits(value, count)
    }

    pub fn finish(mut self) -> Result<usize, EncodeError> {
        self.bit_writer.add_bits(1, 1)?;
        self.bit_writer.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::backward_bit_reader::BackwardBitReader;

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

    #[test]
    fn backward_writer_output_is_read_back_in_reverse_call_order() {
        let mut small_output = [0u8; 2];
        let mut small_writer = BackwardBitWriter::new(&mut small_output);
        small_writer.add_bits(0b101, 3).unwrap();
        small_writer.add_bits(0b11010, 5).unwrap();
        let small_length = small_writer.finish().unwrap();
        let mut small_reader = BackwardBitReader::new(&small_output[..small_length]).unwrap();
        assert_eq!(small_reader.read_bits(5), 0b11010);
        assert_eq!(small_reader.read_bits(3), 0b101);
        assert!(small_reader.is_finished());

        let mut random = XorshiftRandom::new(0x9E3779B97F4A7C15);
        let mut widths_and_values = Vec::new();
        for _ in 0..500 {
            let width = (random.next_u64() % 56) as usize + 1;
            let mask = (1u64 << width) - 1;
            let value = random.next_u64() & mask;
            widths_and_values.push((width, value));
        }

        let mut large_output = [0u8; 4096];
        let mut large_writer = BackwardBitWriter::new(&mut large_output);
        for (width, value) in &widths_and_values {
            large_writer.add_bits(*value, *width).unwrap();
        }
        let large_length = large_writer.finish().unwrap();

        let mut large_reader = BackwardBitReader::new(&large_output[..large_length]).unwrap();
        for (width, value) in widths_and_values.iter().rev() {
            assert_eq!(large_reader.read_bits(*width), *value);
        }
        assert!(large_reader.is_finished());
    }

    #[test]
    fn writers_report_output_too_small() {
        let mut exact_output = [0u8; 1];
        let mut exact_writer = BackwardBitWriter::new(&mut exact_output);
        exact_writer.add_bits(0b1111111, 7).unwrap();
        assert!(exact_writer.finish().is_ok());

        let mut short_output = [0u8; 1];
        let mut short_writer = BackwardBitWriter::new(&mut short_output[..0]);
        short_writer.add_bits(0b1111111, 7).unwrap();
        assert_eq!(short_writer.finish(), Err(EncodeError::OutputTooSmall));

        let mut too_small_for_flush = [0u8; 7];
        let mut flush_writer = BackwardBitWriter::new(&mut too_small_for_flush);
        for _ in 0..8 {
            flush_writer.add_bits(0xFF, 8).unwrap();
        }
        assert_eq!(
            flush_writer.add_bits(0xFF, 8),
            Err(EncodeError::OutputTooSmall)
        );

        let mut just_enough_for_flush = [0u8; 8];
        let mut flush_writer_with_room = BackwardBitWriter::new(&mut just_enough_for_flush);
        for _ in 0..8 {
            flush_writer_with_room.add_bits(0xFF, 8).unwrap();
        }
        assert!(flush_writer_with_room.add_bits(0xFF, 8).is_ok());
    }

    #[test]
    fn backward_writer_finish_on_empty_stream_writes_only_the_padding_byte() {
        let mut output = [0u8; 1];
        let writer = BackwardBitWriter::new(&mut output);
        let length = writer.finish().unwrap();
        assert_eq!(length, 1);
        assert_eq!(output, [0x01]);
        let reader = BackwardBitReader::new(&output[..length]).unwrap();
        assert_eq!(reader.bits_left(), 0);
        assert!(reader.is_finished());
    }
}
