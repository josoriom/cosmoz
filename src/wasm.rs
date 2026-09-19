use core::cell::UnsafeCell;

use crate::{
    decoder::{DecodeWorkspace, decompress, get_decompressed_size},
    error::DecodeError,
};
#[cfg(feature = "compression")]
use crate::{
    encode_error::EncodeError,
    encoder::{CompressOptions, EncodeWorkspace, compress, get_max_compressed_size},
};

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

const PAGE_SIZE: usize = 64 * 1024;
const HEAP_ALIGNMENT: usize = 8;

static WORKSPACE: SingleThreadCell<DecodeWorkspace> = SingleThreadCell::new(DecodeWorkspace::new());
#[cfg(feature = "compression")]
static ENCODE_WORKSPACE: SingleThreadCell<Option<alloc::boxed::Box<EncodeWorkspace>>> =
    SingleThreadCell::new(None);
static HEAP_START: SingleThreadCell<usize> = SingleThreadCell::new(0);
static HEAP_USED: SingleThreadCell<usize> = SingleThreadCell::new(0);
static HEAP_END: SingleThreadCell<usize> = SingleThreadCell::new(0);

fn get_workspace() -> &'static mut DecodeWorkspace {
    unsafe { &mut *WORKSPACE.0.get() }
}

#[cfg(feature = "compression")]
fn get_encode_workspace() -> &'static mut EncodeWorkspace {
    let slot = unsafe { &mut *ENCODE_WORKSPACE.0.get() };
    slot.get_or_insert_with(EncodeWorkspace::new_boxed)
}

fn get_heap_start() -> &'static mut usize {
    unsafe { &mut *HEAP_START.0.get() }
}

fn get_heap_end() -> &'static mut usize {
    unsafe { &mut *HEAP_END.0.get() }
}

fn get_memory_end() -> usize {
    core::arch::wasm32::memory_size(0) * PAGE_SIZE
}

fn get_heap_used() -> &'static mut usize {
    unsafe { &mut *HEAP_USED.0.get() }
}

fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

#[unsafe(no_mangle)]
pub(crate) extern "C" fn cosmoz_allocate(length: usize) -> *mut u8 {
    let heap_used = get_heap_used();
    let heap_end = get_heap_end();
    let mut start = align_up(*heap_used, HEAP_ALIGNMENT);
    let mut end = match start.checked_add(length) {
        Some(end) => end,
        None => return core::ptr::null_mut(),
    };
    if *heap_end == 0 || end > *heap_end {
        let memory_end = get_memory_end();
        if *heap_end != memory_end {
            start = memory_end;
            end = match start.checked_add(length) {
                Some(end) => end,
                None => return core::ptr::null_mut(),
            };
            *get_heap_start() = start;
        }
        let pages = (end - memory_end).div_ceil(PAGE_SIZE);
        if core::arch::wasm32::memory_grow(0, pages) == usize::MAX {
            return core::ptr::null_mut();
        }
        *heap_end = memory_end + pages * PAGE_SIZE;
    }
    *heap_used = end;
    start as *mut u8
}

#[unsafe(no_mangle)]
pub(crate) extern "C" fn cosmoz_free(pointer: *mut u8, length: usize) {
    let heap_used = get_heap_used();
    let block_start = pointer as usize;
    if block_start >= *get_heap_start() && block_start + length == *heap_used {
        *heap_used = block_start;
    }
}

#[unsafe(no_mangle)]
pub(crate) extern "C" fn cosmoz_reset_heap() {
    *get_heap_used() = *get_heap_start();
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
pub(crate) unsafe extern "C" fn cosmoz_get_decompressed_size(
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
pub(crate) unsafe extern "C" fn cosmoz_decompress(
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

#[cfg(feature = "compression")]
fn encode_error_to_error_code(error: EncodeError) -> i64 {
    -(1 + error as i64)
}

#[cfg(feature = "compression")]
#[unsafe(no_mangle)]
pub(crate) extern "C" fn cosmoz_get_max_compressed_size(input_length: usize) -> i64 {
    let options = CompressOptions {
        with_checksum: true,
        level: 1,
    };
    get_max_compressed_size(input_length, &options) as i64
}

#[cfg(feature = "compression")]
#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub(crate) unsafe extern "C" fn cosmoz_compress(
    input_pointer: *const u8,
    input_length: usize,
    output_pointer: *mut u8,
    output_length: usize,
    with_checksum: u32,
) -> i64 {
    let input = match unsafe { slice_from_raw_parts_checked(input_pointer, input_length) } {
        Some(input) => input,
        None => return encode_error_to_error_code(EncodeError::OutputTooSmall),
    };
    let output = match unsafe { slice_from_raw_parts_mut_checked(output_pointer, output_length) } {
        Some(output) => output,
        None => return encode_error_to_error_code(EncodeError::OutputTooSmall),
    };
    let options = CompressOptions {
        with_checksum: with_checksum != 0,
        level: 1,
    };
    let workspace = get_encode_workspace();
    match compress(input, output, &options, workspace) {
        Ok(bytes_written) => bytes_written as i64,
        Err(error) => encode_error_to_error_code(error),
    }
}

#[unsafe(no_mangle)]
#[allow(unused_assignments)]
pub(crate) extern "C" fn cosmoz_run_self_tests() -> i64 {
    let mut kernel_index: i64 = 0;
    macro_rules! run_kernel {
        ($kernel:expr) => {{
            if let Some(test_number) = $kernel() {
                return 1000 * kernel_index + test_number as i64;
            }
            kernel_index += 1;
        }};
    }
    run_kernel!(crate::simd::copy_bytes::run_self_tests);
    #[cfg(feature = "compression")]
    run_kernel!(crate::simd::count_matching_bytes::run_self_tests);
    #[cfg(feature = "compression")]
    run_kernel!(crate::simd::histogram::run_self_tests);
    run_kernel!(crate::simd::row_tag_match::run_self_tests);
    0
}
