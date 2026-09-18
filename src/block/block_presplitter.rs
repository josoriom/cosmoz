use crate::frame::block_header::MAX_BLOCK_SIZE;

const CHUNK_LENGTH: usize = 8 * 1024;
const FINGERPRINT_LOG: u32 = 10;
const FINGERPRINT_SIZE: usize = 1 << FINGERPRINT_LOG;
const PAIR_LENGTH: usize = 2;
const HASH_MULTIPLIER: u32 = 0x9E37_79B9;
const THRESHOLD_RATE: u64 = 16;
const THRESHOLD_BASE: u64 = THRESHOLD_RATE - 2;
const START_PENALTY: u64 = 3;
const MIN_SAVINGS_TO_SPLIT: i64 = 3;

struct Fingerprint {
    events: [u32; FINGERPRINT_SIZE],
    event_count: u64,
}

impl Fingerprint {
    const fn new() -> Self {
        Fingerprint {
            events: [0; FINGERPRINT_SIZE],
            event_count: 0,
        }
    }

    fn record(&mut self, chunk: &[u8]) {
        self.events = [0; FINGERPRINT_SIZE];
        let pair_count = chunk.len() - PAIR_LENGTH + 1;
        for pair in chunk.windows(PAIR_LENGTH) {
            let value = u16::from_le_bytes([pair[0], pair[1]]) as u32;
            let hash = value.wrapping_mul(HASH_MULTIPLIER) >> (32 - FINGERPRINT_LOG);
            self.events[hash as usize] += 1;
        }
        self.event_count = pair_count as u64;
    }

    fn merge(&mut self, other: &Fingerprint) {
        for (event, other_event) in self.events.iter_mut().zip(other.events.iter()) {
            *event += other_event;
        }
        self.event_count += other.event_count;
    }

    fn is_too_different(&self, newer: &Fingerprint, penalty: u64) -> bool {
        let mut distance = 0u64;
        for (&event, &newer_event) in self.events.iter().zip(newer.events.iter()) {
            let scaled = event as i64 * newer.event_count as i64;
            let newer_scaled = newer_event as i64 * self.event_count as i64;
            distance += scaled.abs_diff(newer_scaled);
        }
        let expected = self.event_count * newer.event_count;
        distance >= expected * (THRESHOLD_BASE + penalty) / THRESHOLD_RATE
    }
}

pub(crate) fn find_block_length(remaining: &[u8], savings: i64) -> usize {
    if remaining.len() < MAX_BLOCK_SIZE {
        return remaining.len();
    }
    if savings < MIN_SAVINGS_TO_SPLIT {
        return MAX_BLOCK_SIZE;
    }
    let mut past = Fingerprint::new();
    let mut newer = Fingerprint::new();
    let mut penalty = START_PENALTY;
    past.record(&remaining[..CHUNK_LENGTH]);
    let mut position = CHUNK_LENGTH;
    while position <= MAX_BLOCK_SIZE - CHUNK_LENGTH {
        newer.record(&remaining[position..position + CHUNK_LENGTH]);
        if past.is_too_different(&newer, penalty) {
            return position;
        }
        past.merge(&newer);
        penalty = penalty.saturating_sub(1);
        position += CHUNK_LENGTH;
    }
    MAX_BLOCK_SIZE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_where_the_byte_statistics_change() {
        let mut input = Vec::new();
        while input.len() < 40 * 1024 {
            input.extend_from_slice(b"<peak mz=\"412.2\" intensity=\"8812\"/>\n");
        }
        input.truncate(40 * 1024);
        let mut state = 0x1234_5678u32;
        while input.len() < MAX_BLOCK_SIZE + 1000 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            input.push(state as u8);
        }
        assert_eq!(find_block_length(&input, 100), 40 * 1024);
        assert_eq!(find_block_length(&input, 0), MAX_BLOCK_SIZE);
        assert_eq!(find_block_length(&input[..5000], 100), 5000);
    }
}
