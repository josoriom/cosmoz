use alloc::{boxed::Box, vec, vec::Vec};

use crate::{
    block::block_writer::write_block,
    encode_error::EncodeError,
    encoder::{EncodeWorkspace, find_next_block_length},
    frame::{
        block_header::MAX_BLOCK_SIZE,
        frame_writer::{MAX_FRAME_HEADER_LENGTH, write_frame_header},
    },
    levels::{MAX_OFFSET_LOG, MatchFinder},
};
#[cfg(feature = "checksum")]
use crate::{
    frame::{frame_header::ZSTD_CHECKSUM_LENGTH, frame_writer::write_checksum},
    hash::xxhash64::XxHash64,
};

pub(crate) const STREAM_SEGMENT_SIZE: usize = 1 << MAX_OFFSET_LOG;
const BLOCK_OUTPUT_CAPACITY: usize = 2 * MAX_BLOCK_SIZE;

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockFlush {
    KeepLookahead,
    WholeSegment,
    FinalSegment,
}

pub(crate) struct StreamEncoder {
    workspace: Box<EncodeWorkspace>,
    segment: Vec<u8>,
    block_output: Vec<u8>,
    block_start: usize,
    savings: i64,
    header_written: bool,
    finished: bool,
    with_checksum: bool,
    #[cfg(feature = "checksum")]
    hasher: XxHash64,
}

impl StreamEncoder {
    pub(crate) fn new(level: u8, with_checksum: bool) -> Result<Self, EncodeError> {
        #[cfg(not(feature = "checksum"))]
        if with_checksum {
            return Err(EncodeError::BadOptions);
        }
        Ok(StreamEncoder {
            workspace: EncodeWorkspace::new_boxed_for_level(level)?,
            segment: Vec::new(),
            block_output: vec![0u8; BLOCK_OUTPUT_CAPACITY],
            block_start: 0,
            savings: 0,
            header_written: false,
            finished: false,
            with_checksum,
            #[cfg(feature = "checksum")]
            hasher: XxHash64::new(0),
        })
    }

    pub(crate) fn write(&mut self, input: &[u8], output: &mut Vec<u8>) -> Result<(), EncodeError> {
        if self.finished {
            return Err(EncodeError::BadOptions);
        }
        self.write_header_once(output)?;
        #[cfg(feature = "checksum")]
        if self.with_checksum {
            self.hasher.update(input);
        }

        let mut remaining = input;
        while !remaining.is_empty() {
            if self.segment.len() == STREAM_SEGMENT_SIZE {
                self.write_blocks(output, BlockFlush::WholeSegment)?;
                self.segment.clear();
                self.block_start = 0;
            }
            let taken = (STREAM_SEGMENT_SIZE - self.segment.len()).min(remaining.len());
            self.segment.extend_from_slice(&remaining[..taken]);
            remaining = &remaining[taken..];
            self.write_blocks(output, BlockFlush::KeepLookahead)?;
        }
        Ok(())
    }

    pub(crate) fn finish(&mut self, output: &mut Vec<u8>) -> Result<(), EncodeError> {
        if self.finished {
            return Err(EncodeError::BadOptions);
        }
        self.write_header_once(output)?;
        if self.block_start == self.segment.len() {
            self.segment.clear();
            self.block_start = 0;
        }
        self.write_blocks(output, BlockFlush::FinalSegment)?;
        #[cfg(feature = "checksum")]
        if self.with_checksum {
            let mut checksum = [0u8; ZSTD_CHECKSUM_LENGTH];
            write_checksum(&mut checksum, self.hasher.finish())?;
            output.extend_from_slice(&checksum);
        }
        self.finished = true;
        Ok(())
    }

    fn write_header_once(&mut self, output: &mut Vec<u8>) -> Result<(), EncodeError> {
        if self.header_written {
            return Ok(());
        }
        let window_log = self.workspace.level_parameters.window_log.min(MAX_OFFSET_LOG);
        let mut header = [0u8; MAX_FRAME_HEADER_LENGTH];
        let header_length = write_frame_header(&mut header, None, window_log, self.with_checksum)?;
        output.extend_from_slice(&header[..header_length]);
        self.header_written = true;
        Ok(())
    }

    fn write_blocks(&mut self, output: &mut Vec<u8>, flush: BlockFlush) -> Result<(), EncodeError> {
        loop {
            let available = self.segment.len() - self.block_start;
            let has_ready_block = match flush {
                BlockFlush::KeepLookahead => available > MAX_BLOCK_SIZE,
                BlockFlush::WholeSegment => available > 0,
                BlockFlush::FinalSegment => available > 0 || self.segment.is_empty(),
            };
            if !has_ready_block {
                return Ok(());
            }
            if self.block_start == 0 {
                self.start_segment(flush)?;
            }
            let block_length = find_next_block_length(
                &self.segment[self.block_start..],
                self.savings,
                self.workspace.level_parameters,
            );
            let block_end = self.block_start + block_length;
            let is_last = flush == BlockFlush::FinalSegment && block_end == self.segment.len();
            let written = write_block(
                &self.segment[..block_end],
                self.block_start,
                is_last,
                &mut self.block_output,
                &mut self.workspace,
            )?;
            output.extend_from_slice(&self.block_output[..written]);
            self.savings += block_length as i64 - written as i64;
            self.block_start = block_end;
            if is_last {
                return Ok(());
            }
        }
    }

    fn start_segment(&mut self, flush: BlockFlush) -> Result<(), EncodeError> {
        let expected_length = match flush {
            BlockFlush::FinalSegment => self.segment.len(),
            BlockFlush::KeepLookahead | BlockFlush::WholeSegment => STREAM_SEGMENT_SIZE,
        };
        self.workspace.prepare_tables_for_input(expected_length)?;
        self.workspace.match_finder.reset(expected_length);
        self.savings = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::{DecodeWorkspace, decompress, get_frame_compressed_size};

    fn build_mixed_input(length: usize) -> Vec<u8> {
        let mut state = 0x2468_ace1u32;
        let mut bytes = Vec::with_capacity(length);
        let words: [&[u8]; 3] = [b"<cvParam accession=\"MS:1000511\" value=\"", b"\"/>\n", b"<binary>"];
        while bytes.len() < length {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            if state.is_multiple_of(4) {
                bytes.extend_from_slice(words[(state >> 8) as usize % words.len()]);
            } else {
                bytes.extend_from_slice(&state.to_le_bytes()[..1 + (state >> 20) as usize % 3]);
            }
        }
        bytes.truncate(length);
        bytes
    }

    fn encode_in_pieces(input: &[u8], level: u8, with_checksum: bool) -> Vec<u8> {
        let mut encoder = StreamEncoder::new(level, with_checksum).unwrap();
        let mut output = Vec::new();
        let mut position = 0usize;
        let mut piece = 1usize;
        while position < input.len() {
            let end = (position + piece).min(input.len());
            encoder.write(&input[position..end], &mut output).unwrap();
            position = end;
            piece = (piece * 7 + 3) % 300_000 + 1;
        }
        encoder.finish(&mut output).unwrap();
        output
    }

    fn decode(frame: &[u8], length: usize) -> Vec<u8> {
        let mut workspace = DecodeWorkspace::new_boxed();
        let mut decoded = vec![0u8; length];
        let written = decompress(frame, &mut decoded, &mut workspace).unwrap();
        assert_eq!(written, length);
        decoded
    }

    #[test]
    fn pieces_of_any_size_round_trip_across_segments() {
        let input = build_mixed_input(STREAM_SEGMENT_SIZE + 700_000);
        let frame = encode_in_pieces(&input, 1, true);
        assert_eq!(decode(&frame, input.len()), input);
        assert_eq!(get_frame_compressed_size(&frame).unwrap(), frame.len());
    }

    #[test]
    fn level_twenty_two_stream_round_trips() {
        let input = build_mixed_input(300_000);
        let frame = encode_in_pieces(&input, 22, false);
        assert_eq!(decode(&frame, input.len()), input);
    }

    #[test]
    fn empty_stream_is_a_valid_frame() {
        let mut encoder = StreamEncoder::new(9, true).unwrap();
        let mut output = Vec::new();
        encoder.finish(&mut output).unwrap();
        assert_eq!(decode(&output, 0), Vec::<u8>::new());
        assert_eq!(encoder.write(b"late", &mut output), Err(EncodeError::BadOptions));
    }
}
