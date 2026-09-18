pub(crate) struct ForwardBitReader<'input> {
    input: &'input [u8],
    bit_position: usize,
}

impl<'input> ForwardBitReader<'input> {
    pub(crate) fn new(input: &'input [u8]) -> Self {
        Self {
            input,
            bit_position: 0,
        }
    }

    pub(crate) fn read_bits(&mut self, count: usize) -> u64 {
        let value = self.peek_bits(count);
        self.bit_position = self.bit_position.saturating_add(count);
        value
    }

    pub(crate) fn peek_bits(&self, count: usize) -> u64 {
        debug_assert!(count <= 56);
        extract_bits_starting_at(self.input, self.bit_position, count)
    }

    #[allow(dead_code)]
    pub(crate) fn skip_bits(&mut self, count: usize) {
        self.bit_position = self.bit_position.saturating_add(count);
    }

    pub(crate) fn bytes_used(&self) -> usize {
        self.bit_position.div_ceil(8)
    }
}

fn extract_bits_starting_at(input: &[u8], bit_position: usize, count: usize) -> u64 {
    if count == 0 {
        return 0;
    }
    let start_byte = bit_position / 8;
    let bit_offset = bit_position % 8;
    let word = read_little_endian_word_from(input, start_byte);
    let shifted = word >> bit_offset;
    shifted & low_bits_mask(count)
}

fn read_little_endian_word_from(input: &[u8], start_byte: usize) -> u64 {
    let available_bytes = input.get(start_byte..).unwrap_or(&[]);
    if let Some(full_word) = available_bytes.first_chunk::<8>() {
        return u64::from_le_bytes(*full_word);
    }
    let mut word_bytes = [0u8; 8];
    word_bytes[..available_bytes.len()].copy_from_slice(available_bytes);
    u64::from_le_bytes(word_bytes)
}

fn low_bits_mask(count: usize) -> u64 {
    (1u64 << count) - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lowest_bit_first_and_reports_bytes_used() {
        let mut reader = ForwardBitReader::new(&[0b0000_0101, 0b0000_0001]);
        assert_eq!(reader.read_bits(3), 5);
        assert_eq!(reader.read_bits(6), 0b100000);
        assert_eq!(reader.bytes_used(), 2);
    }

    #[test]
    fn reading_past_the_end_returns_zeros_and_reports_bytes_used_beyond_input_length() {
        let mut reader = ForwardBitReader::new(&[0xFF]);
        assert_eq!(reader.read_bits(20), 0xFF);
        assert_eq!(reader.bytes_used(), 3);
    }
}
