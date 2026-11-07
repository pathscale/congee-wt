use core::error::Error;
use core::fmt::{self, Debug, Display, Formatter};

#[derive(Debug)]
pub(crate) enum ArtError {
    VersionNotMatch,
    Locked,
    Oom,
}

/// An error while building a compact v2 snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CompactSetError {
    /// The snapshot exceeds the v2 offset range.
    OffsetOutOfRange,
    /// The N256 child offsets could not be encoded.
    N256Encoding(&'static str),
    /// The tree reported an allocation failure.
    OutOfMemory,
}

impl Display for CompactSetError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::OffsetOutOfRange => {
                f.write_str("compact v2 snapshot exceeds the u32 offset range")
            }
            Self::N256Encoding(message) => write!(f, "compact v2 N256 encoding failed: {message}"),
            Self::OutOfMemory => f.write_str("out of memory while building a compact snapshot"),
        }
    }
}

impl Error for CompactSetError {}

/// Out of memory error
pub struct OOMError {}

impl OOMError {
    pub(crate) fn new() -> Self {
        Self {}
    }
}

impl Debug for OOMError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Allocator is out of memory!").finish()
    }
}

impl Display for OOMError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Allocator is out of memory!")
    }
}

impl Error for OOMError {}
