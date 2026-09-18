use crate::error::DecodeError;

pub(crate) const CHUNK_COUNT_LENGTH: usize = 4;
pub(crate) const CHUNK_ENTRY_LENGTH: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChunkEntry {
    pub compressed_length: usize,
    pub decompressed_length: usize,
}

pub(crate) struct ChunkIndex<'input> {
    input: &'input [u8],
    pub chunk_count: usize,
}

impl<'input> ChunkIndex<'input> {
    pub(crate) fn read(input: &'input [u8]) -> Result<Self, DecodeError> {
        if input.len() < CHUNK_COUNT_LENGTH {
            return Err(DecodeError::InputTooShort);
        }
        let chunk_count = read_little_endian_u32(&input[0..CHUNK_COUNT_LENGTH]) as usize;
        if chunk_count == 0 {
            return Err(DecodeError::BadFrameHeader);
        }
        let entries_length = chunk_count
            .checked_mul(CHUNK_ENTRY_LENGTH)
            .ok_or(DecodeError::BadFrameHeader)?;
        let index_length = CHUNK_COUNT_LENGTH
            .checked_add(entries_length)
            .ok_or(DecodeError::BadFrameHeader)?;
        if input.len() < index_length {
            return Err(DecodeError::InputTooShort);
        }
        Ok(Self { input, chunk_count })
    }

    pub(crate) fn index_length(&self) -> usize {
        CHUNK_COUNT_LENGTH + self.chunk_count * CHUNK_ENTRY_LENGTH
    }

    pub(crate) fn get_entry(&self, chunk_number: usize) -> ChunkEntry {
        let entry_start = CHUNK_COUNT_LENGTH + chunk_number * CHUNK_ENTRY_LENGTH;
        let compressed_length =
            read_little_endian_u32(&self.input[entry_start..entry_start + 4]) as usize;
        let decompressed_length =
            read_little_endian_u32(&self.input[entry_start + 4..entry_start + 8]) as usize;
        ChunkEntry {
            compressed_length,
            decompressed_length,
        }
    }

    pub(crate) fn total_decompressed_length(&self) -> Result<u64, DecodeError> {
        let mut total: u64 = 0;
        for chunk_number in 0..self.chunk_count {
            let entry = self.get_entry(chunk_number);
            total = total
                .checked_add(entry.decompressed_length as u64)
                .ok_or(DecodeError::BadFrameHeader)?;
        }
        Ok(total)
    }

    pub(crate) fn total_compressed_length(&self) -> Result<usize, DecodeError> {
        let mut total: usize = 0;
        for chunk_number in 0..self.chunk_count {
            let entry = self.get_entry(chunk_number);
            total = total
                .checked_add(entry.compressed_length)
                .ok_or(DecodeError::BadFrameHeader)?;
        }
        Ok(total)
    }
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

    fn build_index(entries: &[(u32, u32)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for (compressed_length, decompressed_length) in entries {
            bytes.extend_from_slice(&compressed_length.to_le_bytes());
            bytes.extend_from_slice(&decompressed_length.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn reads_entries_and_totals() {
        let bytes = build_index(&[(10, 100), (20, 200), (5, 50)]);
        let index = ChunkIndex::read(&bytes).unwrap();
        assert_eq!(index.chunk_count, 3);
        assert_eq!(index.index_length(), 4 + 3 * 8);
        assert_eq!(
            index.get_entry(1),
            ChunkEntry {
                compressed_length: 20,
                decompressed_length: 200,
            }
        );
        assert_eq!(index.total_compressed_length(), Ok(35));
        assert_eq!(index.total_decompressed_length(), Ok(350));
    }

    #[test]
    fn rejects_zero_chunk_count() {
        let bytes = build_index(&[]);
        match ChunkIndex::read(&bytes) {
            Err(DecodeError::BadFrameHeader) => {}
            other => panic!("unexpected result: {:?}", other.map(|_| ())),
        }
    }

    #[test]
    fn rejects_truncated_index() {
        let bytes = build_index(&[(10, 100), (20, 200)]);
        match ChunkIndex::read(&bytes[..bytes.len() - 1]) {
            Err(DecodeError::InputTooShort) => {}
            other => panic!("unexpected result: {:?}", other.map(|_| ())),
        }
    }
}
