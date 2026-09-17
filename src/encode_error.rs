#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    OutputTooSmall,
    InputTooLarge,
    BadOptions,
    TableNotUsable,
    OutOfMemory,
}
