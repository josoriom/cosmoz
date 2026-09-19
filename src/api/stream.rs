#[cfg(feature = "compression")]
use crate::encode_error::EncodeError;

#[cfg(feature = "compression")]
use super::options::{CompressOptions, Format};

use crate::error::DecodeError;

use super::options::DecompressOptions;

use super::workspace::Decoder;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bytes;

#[cfg(feature = "compression")]
fn build_stream_encoder(
    options: &CompressOptions,
) -> Result<crate::stream_encoder::StreamEncoder, EncodeError> {
    if !matches!(options.format, Format::Zstd) {
        return Err(EncodeError::BadOptions);
    }
    crate::stream_encoder::StreamEncoder::new(options.level, options.checksum)
}

#[cfg(feature = "compression")]
pub struct Compressor<W = Bytes> {
    inner: crate::stream_encoder::StreamEncoder,
    #[allow(dead_code)]
    sink: W,
    #[allow(dead_code)]
    pending: alloc::vec::Vec<u8>,
}

#[cfg(feature = "compression")]
impl Compressor<Bytes> {
    pub fn new(options: &CompressOptions) -> Result<Self, EncodeError> {
        Ok(Self {
            inner: build_stream_encoder(options)?,
            sink: Bytes,
            pending: alloc::vec::Vec::new(),
        })
    }

    pub fn write(&mut self, input: &[u8], output: &mut alloc::vec::Vec<u8>) -> Result<(), EncodeError> {
        self.inner.write(input, output)
    }

    pub fn finish(&mut self, output: &mut alloc::vec::Vec<u8>) -> Result<(), EncodeError> {
        self.inner.finish(output)
    }
}

#[cfg(all(feature = "compression", feature = "std"))]
impl<W: std::io::Write> Compressor<W> {
    pub fn to(sink: W, options: &CompressOptions) -> std::io::Result<Compressor<W>> {
        let inner = build_stream_encoder(options)
            .map_err(|error| std::io::Error::other(alloc::format!("cosmoz compress start: {error}")))?;
        Ok(Compressor {
            inner,
            sink,
            pending: alloc::vec::Vec::new(),
        })
    }

    pub fn finish(mut self) -> std::io::Result<W> {
        self.inner
            .finish(&mut self.pending)
            .map_err(|error| std::io::Error::other(alloc::format!("cosmoz compress finish: {error}")))?;
        self.sink.write_all(&self.pending)?;
        self.sink.flush()?;
        Ok(self.sink)
    }
}

#[cfg(all(feature = "compression", feature = "std"))]
impl<W: std::io::Write> std::io::Write for Compressor<W> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.inner
            .write(buffer, &mut self.pending)
            .map_err(|error| std::io::Error::other(alloc::format!("cosmoz compress write: {error}")))?;
        let result = self.sink.write_all(&self.pending);
        self.pending.clear();
        result?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.sink.flush()
    }
}

pub struct Decompressor<R = Bytes> {
    options: DecompressOptions,
    decoder: alloc::boxed::Box<Decoder>,
    pending: alloc::vec::Vec<u8>,
    #[allow(dead_code)]
    source: R,
    #[allow(dead_code)]
    input_buffer: alloc::vec::Vec<u8>,
    #[allow(dead_code)]
    output_buffer: alloc::vec::Vec<u8>,
    #[allow(dead_code)]
    output_position: usize,
    #[allow(dead_code)]
    source_done: bool,
    #[allow(dead_code)]
    finished: bool,
}

#[cfg(feature = "std")]
const DECOMPRESSOR_INPUT_CHUNK_SIZE: usize = 64 * 1024;

impl<R> Decompressor<R> {
    fn drain_complete_frames(&mut self, output: &mut alloc::vec::Vec<u8>) -> Result<(), DecodeError> {
        let mut consumed = 0usize;

        loop {
            let remaining = &self.pending[consumed..];
            if remaining.is_empty() {
                break;
            }

            let frame_length = match super::one_shot::compressed_size(remaining) {
                Ok(frame_length) => frame_length,
                Err(DecodeError::InputTooShort) => break,
                Err(error) => return Err(error),
            };
            if remaining.len() < frame_length {
                break;
            }

            let frame = &self.pending[consumed..consumed + frame_length];
            let decoded = super::one_shot::allocate_and_decode(frame, &self.options, &mut self.decoder)?;
            output.extend_from_slice(&decoded);
            consumed += frame_length;
        }

        if consumed > 0 {
            self.pending.drain(..consumed);
        }
        Ok(())
    }

    fn push_input(&mut self, input: &[u8], output: &mut alloc::vec::Vec<u8>) -> Result<(), DecodeError> {
        self.pending.extend_from_slice(input);
        self.drain_complete_frames(output)
    }

    fn finish_frames(&mut self, output: &mut alloc::vec::Vec<u8>) -> Result<(), DecodeError> {
        self.drain_complete_frames(output)?;
        if self.pending.is_empty() {
            Ok(())
        } else {
            Err(DecodeError::TruncatedInput)
        }
    }
}

impl Decompressor<Bytes> {
    pub fn new(options: &DecompressOptions) -> Result<Self, DecodeError> {
        Ok(Self {
            options: *options,
            decoder: Decoder::new(),
            pending: alloc::vec::Vec::new(),
            source: Bytes,
            input_buffer: alloc::vec::Vec::new(),
            output_buffer: alloc::vec::Vec::new(),
            output_position: 0,
            source_done: false,
            finished: false,
        })
    }

    pub fn write(&mut self, input: &[u8], output: &mut alloc::vec::Vec<u8>) -> Result<(), DecodeError> {
        self.push_input(input, output)
    }

    pub fn finish(&mut self, output: &mut alloc::vec::Vec<u8>) -> Result<(), DecodeError> {
        self.finish_frames(output)
    }
}

#[cfg(feature = "std")]
impl<R: std::io::Read> Decompressor<R> {
    pub fn from(source: R, options: &DecompressOptions) -> std::io::Result<Decompressor<R>> {
        Ok(Decompressor {
            options: *options,
            decoder: Decoder::new(),
            pending: alloc::vec::Vec::new(),
            source,
            input_buffer: alloc::vec![0u8; DECOMPRESSOR_INPUT_CHUNK_SIZE],
            output_buffer: alloc::vec::Vec::new(),
            output_position: 0,
            source_done: false,
            finished: false,
        })
    }
}

#[cfg(feature = "std")]
impl<R: std::io::Read> std::io::Read for Decompressor<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self.output_position < self.output_buffer.len() {
                let available = &self.output_buffer[self.output_position..];
                let take = available.len().min(buffer.len());
                buffer[..take].copy_from_slice(&available[..take]);
                self.output_position += take;
                return Ok(take);
            }

            if self.finished {
                return Ok(0);
            }

            self.output_buffer.clear();
            self.output_position = 0;

            if self.source_done {
                let mut output_buffer = core::mem::take(&mut self.output_buffer);
                let result = self.finish_frames(&mut output_buffer);
                self.output_buffer = output_buffer;
                result.map_err(|error| std::io::Error::other(alloc::format!("cosmoz decompress finish: {error}")))?;
                self.finished = true;
                if self.output_buffer.is_empty() {
                    return Ok(0);
                }
                continue;
            }

            let read_length = self.source.read(&mut self.input_buffer)?;
            if read_length == 0 {
                self.source_done = true;
                continue;
            }

            let mut output_buffer = core::mem::take(&mut self.output_buffer);
            let input_buffer = core::mem::take(&mut self.input_buffer);
            let result = self.push_input(&input_buffer[..read_length], &mut output_buffer);
            self.output_buffer = output_buffer;
            self.input_buffer = input_buffer;
            result.map_err(|error| std::io::Error::other(alloc::format!("cosmoz decompress write: {error}")))?;
        }
    }
}

#[cfg(all(test, feature = "compression", feature = "std"))]
mod tests {
    use super::*;
    use crate::CompressOptions;
    use std::io::{Read, Write};

    fn build_text(length: usize) -> alloc::vec::Vec<u8> {
        let sentence = b"the quick brown fox jumps over the lazy dog. ";
        let mut text = alloc::vec::Vec::with_capacity(length + sentence.len());
        while text.len() < length {
            text.extend_from_slice(sentence);
        }
        text.truncate(length);
        text
    }

    fn compress_all(input: &[u8]) -> alloc::vec::Vec<u8> {
        let mut compressor = Compressor::new(&CompressOptions::default()).unwrap();
        let mut compressed = alloc::vec::Vec::new();
        for piece in input.chunks(4001) {
            compressor.write(piece, &mut compressed).unwrap();
        }
        compressor.finish(&mut compressed).unwrap();
        compressed
    }

    #[test]
    fn decompressor_splits_across_arbitrary_chunk_boundaries() {
        let input = build_text(500_000);
        let compressed = compress_all(&input);

        let mut decompressor = Decompressor::new(&DecompressOptions::default()).unwrap();
        let mut decoded = alloc::vec::Vec::new();
        for piece in compressed.chunks(53) {
            decompressor.write(piece, &mut decoded).unwrap();
        }
        decompressor.finish(&mut decoded).unwrap();
        assert_eq!(decoded, input);
    }

    #[test]
    fn decompressor_reports_a_truncated_stream() {
        let input = build_text(200_000);
        let mut compressed = compress_all(&input);
        compressed.truncate(compressed.len() - 16);

        let mut decompressor = Decompressor::new(&DecompressOptions::default()).unwrap();
        let mut decoded = alloc::vec::Vec::new();
        let _ = decompressor.write(&compressed, &mut decoded);
        assert_eq!(decompressor.finish(&mut decoded), Err(DecodeError::TruncatedInput));
    }

    #[test]
    fn compressor_to_a_sink_round_trips_with_decompressor_from_a_source() {
        let input = build_text(300_000);

        let mut compressor = Compressor::to(alloc::vec::Vec::new(), &CompressOptions::default()).unwrap();
        compressor.write_all(&input).unwrap();
        let compressed = compressor.finish().unwrap();

        let mut decompressor = Decompressor::from(compressed.as_slice(), &DecompressOptions::default()).unwrap();
        let mut decoded = alloc::vec::Vec::new();
        decompressor.read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, input);
    }

    #[test]
    fn decompressor_from_a_source_over_small_reads() {
        let input = build_text(300_000);
        let compressed = compress_all(&input);

        let mut decompressor = Decompressor::from(SmallReads(&compressed, 0), &DecompressOptions::default()).unwrap();
        let mut decoded = alloc::vec::Vec::new();
        decompressor.read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, input);
    }

    struct SmallReads<'a>(&'a [u8], usize);

    impl<'a> std::io::Read for SmallReads<'a> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let remaining = &self.0[self.1..];
            let take = remaining.len().min(buffer.len()).min(37);
            buffer[..take].copy_from_slice(&remaining[..take]);
            self.1 += take;
            Ok(take)
        }
    }
}
