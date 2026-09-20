use crate::simd::copy_bytes::copy_bytes_overshoot_unchecked;

const SPARE_ROOM_FOR_FAST_COPY: usize = 16;

pub(crate) struct LiteralBuffer<'collected> {
    collected: &'collected mut [u8],
    count: usize,
}

impl<'collected> LiteralBuffer<'collected> {
    pub(crate) fn new(collected: &'collected mut [u8]) -> Self {
        LiteralBuffer {
            collected,
            count: 0,
        }
    }

    pub(crate) fn count(&self) -> usize {
        self.count
    }

    pub(crate) fn clear(&mut self) {
        self.count = 0;
    }

    #[inline(always)]
    pub(crate) fn add(&mut self, input: &[u8], start: usize, length: usize) {
        let end = self.count + length;
        debug_assert!(end <= self.collected.len());
        debug_assert!(start + length <= input.len());

        let has_spare_room = end + SPARE_ROOM_FOR_FAST_COPY <= self.collected.len()
            && start + length + SPARE_ROOM_FOR_FAST_COPY <= input.len();
        if has_spare_room {
            unsafe {
                copy_bytes_overshoot_unchecked(
                    input.as_ptr().add(start),
                    self.collected.as_mut_ptr().add(self.count),
                    length,
                );
            }
        } else {
            self.collected[self.count..end].copy_from_slice(&input[start..start + length]);
        }
        self.count = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_runs_land_back_to_back() {
        let input = b"the quick brown fox jumps over the lazy dog".to_vec();
        let mut collected = vec![0u8; input.len()];
        let mut literals = LiteralBuffer::new(&mut collected);

        literals.add(&input, 0, 3);
        literals.add(&input, 4, 5);
        let count = literals.count();

        assert_eq!(count, 8);
        assert_eq!(&collected[..count], b"thequick");
    }

    #[test]
    fn a_run_that_reaches_the_end_of_the_input_is_copied_exactly() {
        let input = vec![7u8; 40];
        let mut collected = vec![0u8; 40];
        let mut literals = LiteralBuffer::new(&mut collected);

        literals.add(&input, 30, 10);

        assert_eq!(literals.count(), 10);
        assert_eq!(&collected[..10], &input[30..]);
    }

    #[test]
    fn an_empty_run_adds_nothing() {
        let input = vec![1u8; 8];
        let mut collected = vec![0u8; 8];
        let mut literals = LiteralBuffer::new(&mut collected);

        literals.add(&input, 0, 0);

        assert_eq!(literals.count(), 0);
        assert_eq!(collected, vec![0u8; 8]);
    }
}
