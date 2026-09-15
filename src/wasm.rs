#[cfg(not(feature = "std"))]
#[panic_handler]
fn handle_panic(_panic_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
