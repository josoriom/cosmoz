use std::io::{Error, Result, Write};

use crate::stream_encoder::StreamEncoder;

pub struct StreamWriter<W: Write> {
    encoder: StreamEncoder,
    writer: W,
    pending: Vec<u8>,
}

impl<W: Write> StreamWriter<W> {
    pub fn new(writer: W, level: u8) -> Result<Self> {
        let encoder = StreamEncoder::new(level, false)
            .map_err(|error| Error::other(format!("cosmoz stream start: {error:?}")))?;
        Ok(StreamWriter {
            encoder,
            writer,
            pending: Vec::new(),
        })
    }

    pub fn finish(mut self) -> Result<W> {
        self.encoder
            .finish(&mut self.pending)
            .map_err(|error| Error::other(format!("cosmoz stream finish: {error:?}")))?;
        self.writer.write_all(&self.pending)?;
        self.writer.flush()?;
        Ok(self.writer)
    }
}

impl<W: Write> Write for StreamWriter<W> {
    fn write(&mut self, input: &[u8]) -> Result<usize> {
        self.encoder
            .write(input, &mut self.pending)
            .map_err(|error| Error::other(format!("cosmoz stream write: {error:?}")))?;
        let written = self.writer.write_all(&self.pending);
        self.pending.clear();
        written?;
        Ok(input.len())
    }

    fn flush(&mut self) -> Result<()> {
        self.writer.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::{DecodeWorkspace, decompress};

    #[test]
    fn io_writer_output_decodes_with_the_zstd_cli() {
        let mut input = Vec::new();
        for index in 0..40_000u32 {
            input.extend_from_slice(format!("<offset idRef=\"scan={index}\">{}</offset>\n", index * 37).as_bytes());
        }
        let mut writer = StreamWriter::new(Vec::new(), 22).unwrap();
        for piece in input.chunks(65_537) {
            writer.write_all(piece).unwrap();
        }
        let frame = writer.finish().unwrap();

        let mut workspace = DecodeWorkspace::new_boxed();
        let mut decoded = vec![0u8; input.len()];
        assert_eq!(decompress(&frame, &mut decoded, &mut workspace).unwrap(), input.len());
        assert_eq!(decoded, input);

        let Ok(mut child) = std::process::Command::new("zstd")
            .args(["-d", "-c", "-q"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
        else {
            return;
        };
        child.stdin.take().unwrap().write_all(&frame).unwrap();
        let cli_output = child.wait_with_output().unwrap();
        assert!(cli_output.status.success());
        assert_eq!(cli_output.stdout, input);
    }
}
