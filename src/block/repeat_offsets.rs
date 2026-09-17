pub struct RepeatOffsets {
    pub first: u32,
    pub second: u32,
    pub third: u32,
}

impl RepeatOffsets {
    pub fn new() -> Self {
        RepeatOffsets {
            first: 1,
            second: 4,
            third: 8,
        }
    }

    pub fn get_offset(&mut self, offset_value: u32, literal_length: u32) -> u32 {
        if offset_value > 3 {
            let offset = offset_value - 3;
            self.third = self.second;
            self.second = self.first;
            self.first = offset;
            return offset;
        }

        if literal_length != 0 {
            match offset_value {
                1 => self.first,
                2 => {
                    core::mem::swap(&mut self.first, &mut self.second);
                    self.first
                }
                _ => {
                    let offset = self.third;
                    self.third = self.second;
                    self.second = self.first;
                    self.first = offset;
                    offset
                }
            }
        } else {
            match offset_value {
                1 => {
                    core::mem::swap(&mut self.first, &mut self.second);
                    self.first
                }
                2 => {
                    let offset = self.third;
                    self.third = self.second;
                    self.second = self.first;
                    self.first = offset;
                    offset
                }
                _ => {
                    let offset = self.first - 1;
                    self.third = self.second;
                    self.second = self.first;
                    self.first = offset;
                    offset
                }
            }
        }
    }
}

impl Default for RepeatOffsets {
    fn default() -> Self {
        Self::new()
    }
}

impl RepeatOffsets {
    #[inline]
    pub fn get_offset_value(&mut self, offset: u32, literal_length: u32) -> u32 {
        debug_assert!(offset != 0, "offset must not be zero");

        let matching_code = if literal_length != 0 {
            if offset == self.first {
                Some(1)
            } else if offset == self.second {
                Some(2)
            } else if offset == self.third {
                Some(3)
            } else {
                None
            }
        } else if offset == self.second {
            Some(1)
        } else if offset == self.third {
            Some(2)
        } else if self.first > 0 && offset == self.first - 1 {
            Some(3)
        } else {
            None
        };

        let non_repeat_value = if offset == 0 { 4 } else { offset + 3 };
        let chosen_value = matching_code.unwrap_or(non_repeat_value);
        let resolved_offset = self.get_offset(chosen_value, literal_length);
        debug_assert_eq!(resolved_offset, offset);
        chosen_value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_driven_steps() {
        let mut repeat_offsets = RepeatOffsets::new();
        assert_eq!(repeat_offsets.first, 1);
        assert_eq!(repeat_offsets.second, 4);
        assert_eq!(repeat_offsets.third, 8);

        let offset = repeat_offsets.get_offset(10, 1);
        assert_eq!(offset, 7);
        assert_eq!(
            (
                repeat_offsets.first,
                repeat_offsets.second,
                repeat_offsets.third
            ),
            (7, 1, 4)
        );

        let offset = repeat_offsets.get_offset(1, 1);
        assert_eq!(offset, 7);
        assert_eq!(
            (
                repeat_offsets.first,
                repeat_offsets.second,
                repeat_offsets.third
            ),
            (7, 1, 4)
        );

        let offset = repeat_offsets.get_offset(2, 1);
        assert_eq!(offset, 1);
        assert_eq!(
            (
                repeat_offsets.first,
                repeat_offsets.second,
                repeat_offsets.third
            ),
            (1, 7, 4)
        );

        let offset = repeat_offsets.get_offset(3, 1);
        assert_eq!(offset, 4);
        assert_eq!(
            (
                repeat_offsets.first,
                repeat_offsets.second,
                repeat_offsets.third
            ),
            (4, 1, 7)
        );

        let offset = repeat_offsets.get_offset(1, 0);
        assert_eq!(offset, 1);
        assert_eq!(
            (
                repeat_offsets.first,
                repeat_offsets.second,
                repeat_offsets.third
            ),
            (1, 4, 7)
        );

        let offset = repeat_offsets.get_offset(2, 0);
        assert_eq!(offset, 7);
        assert_eq!(
            (
                repeat_offsets.first,
                repeat_offsets.second,
                repeat_offsets.third
            ),
            (7, 1, 4)
        );

        let offset = repeat_offsets.get_offset(3, 0);
        assert_eq!(offset, 6);
        assert_eq!(
            (
                repeat_offsets.first,
                repeat_offsets.second,
                repeat_offsets.third
            ),
            (6, 7, 1)
        );

        let mut zero_first = RepeatOffsets {
            first: 1,
            second: 4,
            third: 8,
        };
        let offset = zero_first.get_offset(3, 0);
        assert_eq!(offset, 0);
        assert_eq!(
            (zero_first.first, zero_first.second, zero_first.third),
            (0, 1, 4)
        );

        let offset = repeat_offsets.get_offset(9, 1);
        assert_eq!(offset, 6);
        assert_eq!(
            (
                repeat_offsets.first,
                repeat_offsets.second,
                repeat_offsets.third
            ),
            (6, 6, 7)
        );
    }

    fn next_pseudo_random_number(state: &mut u32) -> u32 {
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        *state
    }

    #[test]
    fn offset_value_round_trips_through_decoder_history() {
        let mut encoder_history = RepeatOffsets::new();
        let mut decoder_history = RepeatOffsets::new();
        let mut random_state = 0x1234_5678u32;

        for step in 0..100_000u32 {
            let literal_length = if next_pseudo_random_number(&mut random_state).is_multiple_of(3) {
                0
            } else {
                1
            };

            let choice = next_pseudo_random_number(&mut random_state) % 5;
            let offset = match choice {
                0 => encoder_history.first,
                1 => encoder_history.second,
                2 => encoder_history.third,
                3 => {
                    if literal_length == 0 && encoder_history.first > 0 {
                        encoder_history.first - 1
                    } else {
                        encoder_history.first
                    }
                }
                _ => 1000 + step * 97 + (next_pseudo_random_number(&mut random_state) % 31),
            };

            if offset == 0 {
                continue;
            }

            let code = encoder_history.get_offset_value(offset, literal_length);
            let decoded_offset = decoder_history.get_offset(code, literal_length);

            assert_eq!(decoded_offset, offset);
            assert_eq!(
                (
                    encoder_history.first,
                    encoder_history.second,
                    encoder_history.third
                ),
                (
                    decoder_history.first,
                    decoder_history.second,
                    decoder_history.third
                )
            );
        }
    }
}
