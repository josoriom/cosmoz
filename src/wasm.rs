use crate::block::repeat_offsets::RepeatOffsets;
use crate::decoder::{DecodeWorkspace, decode_block_sequence, decompress, get_decompressed_size};
use crate::encode_error::EncodeError;
use crate::encoder::{
    CompressOptions, EncodeWorkspace, compress, compress_chunk, get_max_compressed_size,
};
use crate::error::DecodeError;
use crate::frame::chunk_index::{ChunkEntry, ChunkIndex};
use crate::frame::frame_header::{FrameFormat, read_frame_header};
use crate::frame::frame_writer::{write_checksum, write_chunk_index, write_frame_header};
use crate::hash::xxhash3;
use crate::match_finder::MatchFinder;
use core::cell::UnsafeCell;

#[cfg(not(feature = "std"))]
#[panic_handler]
fn handle_panic(_panic_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

struct SingleThreadCell<T>(UnsafeCell<T>);

unsafe impl<T> Sync for SingleThreadCell<T> {}

impl<T> SingleThreadCell<T> {
    const fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }
}

const HEAP_SIZE: usize = 16 * 1024 * 1024;
const HEAP_ALIGNMENT: usize = 8;

static WORKSPACE: SingleThreadCell<DecodeWorkspace> = SingleThreadCell::new(DecodeWorkspace::new());
static ENCODE_WORKSPACE: SingleThreadCell<EncodeWorkspace> =
    SingleThreadCell::new(EncodeWorkspace::new());
static HEAP: SingleThreadCell<[u8; HEAP_SIZE]> = SingleThreadCell::new([0u8; HEAP_SIZE]);
static HEAP_USED: SingleThreadCell<usize> = SingleThreadCell::new(0);

fn get_workspace() -> &'static mut DecodeWorkspace {
    unsafe { &mut *WORKSPACE.0.get() }
}

fn get_encode_workspace() -> &'static mut EncodeWorkspace {
    unsafe { &mut *ENCODE_WORKSPACE.0.get() }
}

fn get_heap() -> &'static mut [u8; HEAP_SIZE] {
    unsafe { &mut *HEAP.0.get() }
}

fn get_heap_used() -> &'static mut usize {
    unsafe { &mut *HEAP_USED.0.get() }
}

fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

#[unsafe(no_mangle)]
pub extern "C" fn osmo_allocate(length: usize) -> *mut u8 {
    let heap_used = get_heap_used();
    let aligned_start = align_up(*heap_used, HEAP_ALIGNMENT);
    let end = match aligned_start.checked_add(length) {
        Some(end) => end,
        None => return core::ptr::null_mut(),
    };
    if end > HEAP_SIZE {
        return core::ptr::null_mut();
    }
    *heap_used = end;
    let heap = get_heap();
    unsafe { heap.as_mut_ptr().add(aligned_start) }
}

#[unsafe(no_mangle)]
pub extern "C" fn osmo_free(pointer: *mut u8, length: usize) {
    let heap_used = get_heap_used();
    let base = get_heap().as_mut_ptr() as usize;
    let block_start = pointer as usize;
    if block_start < base {
        return;
    }
    let block_offset = block_start - base;
    if block_offset + length == *heap_used {
        *heap_used = block_offset;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn osmo_reset_heap() {
    let heap_used = get_heap_used();
    *heap_used = 0;
}

unsafe fn slice_from_raw_parts_checked<'a>(pointer: *const u8, length: usize) -> Option<&'a [u8]> {
    if length == 0 {
        Some(&[])
    } else if pointer.is_null() {
        None
    } else {
        Some(unsafe { core::slice::from_raw_parts(pointer, length) })
    }
}

unsafe fn slice_from_raw_parts_mut_checked<'a>(
    pointer: *mut u8,
    length: usize,
) -> Option<&'a mut [u8]> {
    if length == 0 {
        Some(&mut [])
    } else if pointer.is_null() {
        None
    } else {
        Some(unsafe { core::slice::from_raw_parts_mut(pointer, length) })
    }
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn osmo_get_decompressed_size(
    input_pointer: *const u8,
    input_length: usize,
) -> i64 {
    let input = match unsafe { slice_from_raw_parts_checked(input_pointer, input_length) } {
        Some(input) => input,
        None => return decode_error_to_size_error_code(DecodeError::InputTooShort),
    };
    match get_decompressed_size(input) {
        Ok(Some(size)) => size as i64,
        Ok(None) => -1,
        Err(error) => decode_error_to_size_error_code(error),
    }
}

fn decode_error_to_size_error_code(error: DecodeError) -> i64 {
    -(2 + error as i64)
}

fn decode_error_to_decompress_error_code(error: DecodeError) -> i64 {
    -(1 + error as i64)
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn osmo_decompress(
    input_pointer: *const u8,
    input_length: usize,
    output_pointer: *mut u8,
    output_length: usize,
) -> i64 {
    let input = match unsafe { slice_from_raw_parts_checked(input_pointer, input_length) } {
        Some(input) => input,
        None => return decode_error_to_decompress_error_code(DecodeError::InputTooShort),
    };
    let output = match unsafe { slice_from_raw_parts_mut_checked(output_pointer, output_length) } {
        Some(output) => output,
        None => return decode_error_to_decompress_error_code(DecodeError::OutputTooSmall),
    };
    let workspace = get_workspace();
    match decompress(input, output, workspace) {
        Ok(bytes_written) => bytes_written as i64,
        Err(error) => decode_error_to_decompress_error_code(error),
    }
}

fn frame_format_from_u32(format: u32) -> Option<FrameFormat> {
    match format {
        0 => Some(FrameFormat::Zstd),
        1 => Some(FrameFormat::Osmo),
        _ => None,
    }
}

fn encode_error_to_error_code(error: EncodeError) -> i64 {
    -(1 + error as i64)
}

#[unsafe(no_mangle)]
pub extern "C" fn osmo_get_max_compressed_size(input_length: usize, format: u32) -> i64 {
    let frame_format = match frame_format_from_u32(format) {
        Some(frame_format) => frame_format,
        None => return encode_error_to_error_code(EncodeError::BadOptions),
    };
    let options = CompressOptions {
        format: frame_format,
        with_checksum: true,
        chunk_size: crate::encoder::DEFAULT_CHUNK_SIZE,
        level: 1,
    };
    get_max_compressed_size(input_length, &options) as i64
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn osmo_compress(
    input_pointer: *const u8,
    input_length: usize,
    output_pointer: *mut u8,
    output_length: usize,
    format: u32,
    with_checksum: u32,
) -> i64 {
    let frame_format = match frame_format_from_u32(format) {
        Some(frame_format) => frame_format,
        None => return encode_error_to_error_code(EncodeError::BadOptions),
    };
    let input = match unsafe { slice_from_raw_parts_checked(input_pointer, input_length) } {
        Some(input) => input,
        None => return encode_error_to_error_code(EncodeError::OutputTooSmall),
    };
    let output = match unsafe { slice_from_raw_parts_mut_checked(output_pointer, output_length) } {
        Some(output) => output,
        None => return encode_error_to_error_code(EncodeError::OutputTooSmall),
    };
    let options = CompressOptions {
        format: frame_format,
        with_checksum: with_checksum != 0,
        chunk_size: crate::encoder::DEFAULT_CHUNK_SIZE,
        level: 1,
    };
    let workspace = get_encode_workspace();
    match compress(input, output, &options, workspace) {
        Ok(bytes_written) => bytes_written as i64,
        Err(error) => encode_error_to_error_code(error),
    }
}

const WASM_WINDOW_LOG: u8 = 20;
const MAX_WASM_CHUNK_INDEX_ENTRIES: usize = 4096;

#[unsafe(no_mangle)]
pub extern "C" fn osmo_run_self_tests() -> i64 {
    let kernels: [fn() -> Option<u32>; 5] = [
        crate::simd::copy_bytes::run_self_tests,
        crate::simd::count_matching_bytes::run_self_tests,
        crate::simd::hash_positions::run_self_tests,
        crate::simd::histogram::run_self_tests,
        crate::simd::xxhash3_stripes::run_self_tests,
    ];
    let mut kernel_index = 0i64;
    for kernel in kernels {
        if let Some(test_number) = kernel() {
            return 1000 * kernel_index + test_number as i64;
        }
        kernel_index += 1;
    }
    0
}

fn read_osmo_chunk_index(frame: &[u8]) -> Result<ChunkIndex<'_>, DecodeError> {
    let header = read_frame_header(frame)?;
    if header.format != FrameFormat::Osmo {
        return Err(DecodeError::BadFrameHeader);
    }
    let index_input = frame
        .get(header.header_length..)
        .ok_or(DecodeError::InputTooShort)?;
    ChunkIndex::read(index_input)
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn osmo_read_chunk_count(
    frame_pointer: *const u8,
    frame_length: usize,
) -> i64 {
    let frame = match unsafe { slice_from_raw_parts_checked(frame_pointer, frame_length) } {
        Some(frame) => frame,
        None => return decode_error_to_decompress_error_code(DecodeError::InputTooShort),
    };
    match read_osmo_chunk_index(frame) {
        Ok(index) => index.chunk_count as i64,
        Err(error) => decode_error_to_decompress_error_code(error),
    }
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn osmo_read_chunk_entry(
    frame_pointer: *const u8,
    frame_length: usize,
    chunk_number: usize,
    entry_pointer: *mut u32,
) -> i64 {
    let frame = match unsafe { slice_from_raw_parts_checked(frame_pointer, frame_length) } {
        Some(frame) => frame,
        None => return decode_error_to_decompress_error_code(DecodeError::InputTooShort),
    };
    let header = match read_frame_header(frame) {
        Ok(header) => header,
        Err(error) => return decode_error_to_decompress_error_code(error),
    };
    let index = match read_osmo_chunk_index(frame) {
        Ok(index) => index,
        Err(error) => return decode_error_to_decompress_error_code(error),
    };
    if chunk_number >= index.chunk_count {
        return decode_error_to_decompress_error_code(DecodeError::BadFrameHeader);
    }
    if entry_pointer.is_null() {
        return decode_error_to_decompress_error_code(DecodeError::OutputTooSmall);
    }

    let mut input_offset = header.header_length + index.index_length();
    let mut output_offset = 0usize;
    for earlier_chunk_number in 0..chunk_number {
        let earlier_entry = index.get_entry(earlier_chunk_number);
        input_offset += earlier_entry.compressed_length;
        output_offset += earlier_entry.decompressed_length;
    }

    let entry = index.get_entry(chunk_number);
    unsafe {
        entry_pointer.write(input_offset as u32);
        entry_pointer.add(1).write(entry.compressed_length as u32);
        entry_pointer.add(2).write(output_offset as u32);
        entry_pointer.add(3).write(entry.decompressed_length as u32);
    }

    0
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn osmo_decompress_chunk(
    chunk_pointer: *const u8,
    chunk_length: usize,
    output_pointer: *mut u8,
    output_length: usize,
) -> i64 {
    let chunk_input = match unsafe { slice_from_raw_parts_checked(chunk_pointer, chunk_length) } {
        Some(chunk_input) => chunk_input,
        None => return decode_error_to_decompress_error_code(DecodeError::InputTooShort),
    };
    let output = match unsafe { slice_from_raw_parts_mut_checked(output_pointer, output_length) } {
        Some(output) => output,
        None => return decode_error_to_decompress_error_code(DecodeError::OutputTooSmall),
    };
    let workspace = get_workspace();
    workspace.block.reset_history(FrameFormat::Osmo);
    match decode_block_sequence(chunk_input, output, &mut workspace.block) {
        Ok((bytes_consumed, bytes_written)) => {
            if bytes_consumed != chunk_input.len() {
                return decode_error_to_decompress_error_code(DecodeError::BadFrameHeader);
            }
            bytes_written as i64
        }
        Err(error) => decode_error_to_decompress_error_code(error),
    }
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn osmo_compress_chunk(
    input_pointer: *const u8,
    input_length: usize,
    output_pointer: *mut u8,
    output_length: usize,
) -> i64 {
    let input = match unsafe { slice_from_raw_parts_checked(input_pointer, input_length) } {
        Some(input) => input,
        None => return encode_error_to_error_code(EncodeError::OutputTooSmall),
    };
    let output = match unsafe { slice_from_raw_parts_mut_checked(output_pointer, output_length) } {
        Some(output) => output,
        None => return encode_error_to_error_code(EncodeError::OutputTooSmall),
    };
    let workspace = get_encode_workspace();
    workspace.match_finder.reset();
    workspace.repeat_offsets = RepeatOffsets::new();
    match compress_chunk(input, FrameFormat::Osmo, output, workspace) {
        Ok(bytes_written) => bytes_written as i64,
        Err(error) => encode_error_to_error_code(error),
    }
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn osmo_write_frame_header(
    output_pointer: *mut u8,
    output_length: usize,
    content_size_low: u32,
    content_size_high: u32,
    with_checksum: u32,
) -> i64 {
    let output = match unsafe { slice_from_raw_parts_mut_checked(output_pointer, output_length) } {
        Some(output) => output,
        None => return encode_error_to_error_code(EncodeError::OutputTooSmall),
    };
    let content_size = ((content_size_high as u64) << 32) | content_size_low as u64;
    match write_frame_header(
        output,
        FrameFormat::Osmo,
        content_size,
        WASM_WINDOW_LOG,
        with_checksum != 0,
    ) {
        Ok(bytes_written) => bytes_written as i64,
        Err(error) => encode_error_to_error_code(error),
    }
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn osmo_write_chunk_index(
    output_pointer: *mut u8,
    output_length: usize,
    entries_pointer: *const u32,
    chunk_count: usize,
) -> i64 {
    if chunk_count == 0 {
        return encode_error_to_error_code(EncodeError::BadOptions);
    }
    if chunk_count > MAX_WASM_CHUNK_INDEX_ENTRIES {
        return encode_error_to_error_code(EncodeError::InputTooLarge);
    }
    if entries_pointer.is_null() {
        return encode_error_to_error_code(EncodeError::OutputTooSmall);
    }
    let output = match unsafe { slice_from_raw_parts_mut_checked(output_pointer, output_length) } {
        Some(output) => output,
        None => return encode_error_to_error_code(EncodeError::OutputTooSmall),
    };

    let mut entries = [ChunkEntry {
        compressed_length: 0,
        decompressed_length: 0,
    }; MAX_WASM_CHUNK_INDEX_ENTRIES];
    for chunk_number in 0..chunk_count {
        let compressed_length = unsafe { entries_pointer.add(chunk_number * 2).read() } as usize;
        let decompressed_length =
            unsafe { entries_pointer.add(chunk_number * 2 + 1).read() } as usize;
        entries[chunk_number] = ChunkEntry {
            compressed_length,
            decompressed_length,
        };
    }

    match write_chunk_index(output, &entries[..chunk_count]) {
        Ok(bytes_written) => bytes_written as i64,
        Err(error) => encode_error_to_error_code(error),
    }
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn osmo_write_checksum(
    output_pointer: *mut u8,
    output_length: usize,
    hash_low: u32,
    hash_high: u32,
) -> i64 {
    let output = match unsafe { slice_from_raw_parts_mut_checked(output_pointer, output_length) } {
        Some(output) => output,
        None => return encode_error_to_error_code(EncodeError::OutputTooSmall),
    };
    let hash = ((hash_high as u64) << 32) | hash_low as u64;
    match write_checksum(output, FrameFormat::Osmo, hash) {
        Ok(bytes_written) => bytes_written as i64,
        Err(error) => encode_error_to_error_code(error),
    }
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn osmo_xxhash3(pointer: *const u8, length: usize) -> u64 {
    let input = match unsafe { slice_from_raw_parts_checked(pointer, length) } {
        Some(input) => input,
        None => return 0,
    };
    xxhash3::hash_bytes(input)
}
