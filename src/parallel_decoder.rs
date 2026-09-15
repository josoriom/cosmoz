use crate::decoder::{DecodeWorkspace, decode_block_sequence};
use crate::error::DecodeError;
use crate::frame::chunk_index::ChunkIndex;
use crate::frame::frame_header::FrameFormat;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;
use std::vec::Vec;

pub fn decode_chunks_in_parallel(
    chunks_input: &[u8],
    index: &ChunkIndex,
    output: &mut [u8],
) -> Result<(), DecodeError> {
    let chunk_count = index.chunk_count;

    let mut input_offsets = Vec::with_capacity(chunk_count);
    let mut compressed_lengths = Vec::with_capacity(chunk_count);
    let mut decompressed_lengths = Vec::with_capacity(chunk_count);
    let mut input_offset = 0usize;
    for chunk_number in 0..chunk_count {
        let entry = index.get_entry(chunk_number);
        input_offsets.push(input_offset);
        compressed_lengths.push(entry.compressed_length);
        decompressed_lengths.push(entry.decompressed_length);
        input_offset = input_offset
            .checked_add(entry.compressed_length)
            .ok_or(DecodeError::BadFrameHeader)?;
    }
    if input_offset > chunks_input.len() {
        return Err(DecodeError::InputTooShort);
    }

    let mut output_slots: Vec<Option<&mut [u8]>> = Vec::with_capacity(chunk_count);
    let mut remaining_output = output;
    for &decompressed_length in &decompressed_lengths {
        if decompressed_length > remaining_output.len() {
            return Err(DecodeError::OutputTooSmall);
        }
        let (chunk_output, rest) = remaining_output.split_at_mut(decompressed_length);
        output_slots.push(Some(chunk_output));
        remaining_output = rest;
    }

    let output_slots = Mutex::new(output_slots);
    let next_chunk_number = AtomicUsize::new(0);
    let first_error: Mutex<Option<DecodeError>> = Mutex::new(None);

    let thread_count = thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
        .min(chunk_count);

    thread::scope(|scope| {
        for _ in 0..thread_count {
            scope.spawn(|| {
                let mut workspace = DecodeWorkspace::new_boxed();
                loop {
                    let chunk_number = next_chunk_number.fetch_add(1, Ordering::Relaxed);
                    if chunk_number >= chunk_count {
                        break;
                    }

                    if first_error.lock().unwrap().is_some() {
                        break;
                    }

                    let chunk_input = &chunks_input[input_offsets[chunk_number]
                        ..input_offsets[chunk_number] + compressed_lengths[chunk_number]];
                    let chunk_output = output_slots.lock().unwrap()[chunk_number]
                        .take()
                        .expect("chunk output slot already taken");

                    workspace.block.reset_history(FrameFormat::Osmo);
                    let result =
                        decode_block_sequence(chunk_input, chunk_output, &mut workspace.block)
                            .and_then(|(bytes_consumed, bytes_written)| {
                                if bytes_consumed != chunk_input.len()
                                    || bytes_written != decompressed_lengths[chunk_number]
                                {
                                    Err(DecodeError::BadFrameHeader)
                                } else {
                                    Ok(())
                                }
                            });

                    if let Err(error) = result {
                        let mut first_error = first_error.lock().unwrap();
                        if first_error.is_none() {
                            *first_error = Some(error);
                        }
                        break;
                    }
                }
            });
        }
    });

    match first_error.into_inner().unwrap() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
