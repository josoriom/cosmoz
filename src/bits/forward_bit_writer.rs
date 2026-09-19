use crate::encode_error::EncodeError;

pub(crate) struct ForwardBitWriter<'output> {
    output: &'output mut [u8],
    container: u64,
    bits_in_container: usize,
    position: usize,
    has_overflowed: bool,
}

impl<'output> ForwardBitWriter<'output> {
    pub(crate) fn new(output: &'output mut [u8]) -> Self {
        Self {
            output,
            container: 0,
            bits_in_container: 0,
            position: 0,
            has_overflowed: false,
        }
    }

    pub(crate) fn add_bits(&mut self, value: u64, count: usize) -> Result<(), EncodeError> {
        if count == 0 {
            return Ok(());
        }
        debug_assert!(count <= 56);
        if self.bits_in_container + count > 64 {
            self.flush_bytes()?;
        }
        self.container |= mask_to_bit_count(value, count) << self.bits_in_container;
        self.bits_in_container += count;
        Ok(())
    }

    fn flush_bytes(&mut self) -> Result<(), EncodeError> {
        let byte_count = self.bits_in_container / 8;
        if byte_count == 0 {
            return Ok(());
        }
        let end = self
            .position
            .checked_add(byte_count)
            .ok_or(EncodeError::OutputTooSmall)?;
        let destination = self
            .output
            .get_mut(self.position..end)
            .ok_or(EncodeError::OutputTooSmall)?;
        destination.copy_from_slice(&self.container.to_le_bytes()[..byte_count]);
        self.position = end;
        let bits_written = byte_count * 8;
        self.container = shift_right_by_bits(self.container, bits_written);
        self.bits_in_container -= bits_written;
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn add_bits_without_flush(&mut self, value: u64, count: usize) {
        debug_assert!(self.bits_in_container + count <= 64);
        let bits = mask_to_bit_count(value, count);
        self.container |= bits.wrapping_shl(self.bits_in_container as u32);
        self.bits_in_container += count;
    }

    #[inline(always)]
    pub(crate) fn flush_whole_bytes(&mut self) {
        let Some(destination) = self.output.get_mut(self.position..self.position + 8) else {
            self.has_overflowed |= self.flush_bytes().is_err();
            return;
        };
        destination.copy_from_slice(&self.container.to_le_bytes());
        let bits_written = self.bits_in_container & !7;
        self.position += bits_written / 8;
        self.container = shift_right_by_bits(self.container, bits_written);
        self.bits_in_container -= bits_written;
    }

    pub(crate) fn finish(mut self) -> Result<usize, EncodeError> {
        if self.has_overflowed {
            return Err(EncodeError::OutputTooSmall);
        }
        self.flush_bytes()?;
        if self.bits_in_container > 0 {
            let destination = self
                .output
                .get_mut(self.position)
                .ok_or(EncodeError::OutputTooSmall)?;
            *destination = self.container as u8;
            self.position += 1;
        }
        Ok(self.position)
    }
}

fn mask_to_bit_count(value: u64, count: usize) -> u64 {
    value & ((1u64 << count) - 1)
}

fn shift_right_by_bits(value: u64, bits: usize) -> u64 {
    if bits >= 64 { 0 } else { value >> bits }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::forward_bit_reader::ForwardBitReader;

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
    fn forward_writer_output_is_read_back_in_call_order() {
        let mut random = XorshiftRandom::new(0x9E3779B97F4A7C15);
        let mut widths_and_values = Vec::new();
        for _ in 0..500 {
            let width = (random.next_u64() % 56) as usize + 1;
            let mask = (1u64 << width) - 1;
            let value = random.next_u64() & mask;
            widths_and_values.push((width, value));
        }

        let mut output = [0u8; 4096];
        let mut writer = ForwardBitWriter::new(&mut output);
        for (width, value) in &widths_and_values {
            writer.add_bits(*value, *width).unwrap();
        }
        let length = writer.finish().unwrap();

        let mut reader = ForwardBitReader::new(&output[..length]);
        for (width, value) in &widths_and_values {
            assert_eq!(reader.read_bits(*width), *value);
        }
        assert_eq!(reader.bytes_used(), length);
    }

    #[test]
    fn writers_report_output_too_small() {
        let mut exact_output = [0u8; 1];
        let mut exact_writer = ForwardBitWriter::new(&mut exact_output);
        exact_writer.add_bits(0b1111111, 7).unwrap();
        assert!(exact_writer.finish().is_ok());

        let mut short_output = [0u8; 1];
        let mut short_writer = ForwardBitWriter::new(&mut short_output[..0]);
        short_writer.add_bits(0b1111111, 7).unwrap();
        assert_eq!(short_writer.finish(), Err(EncodeError::OutputTooSmall));

        let mut too_small_for_flush = [0u8; 7];
        let mut flush_writer = ForwardBitWriter::new(&mut too_small_for_flush);
        for _ in 0..8 {
            flush_writer.add_bits(0xFF, 8).unwrap();
        }
        assert_eq!(
            flush_writer.add_bits(0xFF, 8),
            Err(EncodeError::OutputTooSmall)
        );

        let mut just_enough_for_flush = [0u8; 8];
        let mut flush_writer_with_room = ForwardBitWriter::new(&mut just_enough_for_flush);
        for _ in 0..8 {
            flush_writer_with_room.add_bits(0xFF, 8).unwrap();
        }
        assert!(flush_writer_with_room.add_bits(0xFF, 8).is_ok());
    }
}
