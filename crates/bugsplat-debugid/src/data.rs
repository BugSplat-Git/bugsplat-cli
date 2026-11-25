//! Reads by offset, so that a multi-gigabyte PDB costs a handful of small reads.

use std::fs::File;

use object::read::{ReadCache, ReadRef};

pub(crate) type Data = ReadCache<File>;

/// Exactly `size` bytes at `offset`, or `None` when the file ends first.
pub(crate) fn bytes(data: &Data, offset: u64, size: u64) -> Option<&[u8]> {
    data.read_bytes_at(offset, size).ok()
}

/// Up to `size` bytes at `offset`; fewer when the file ends first.
pub(crate) fn bytes_up_to(data: &Data, offset: u64, size: u64) -> &[u8] {
    let available = data.len().unwrap_or(0).saturating_sub(offset);
    bytes(data, offset, size.min(available)).unwrap_or_default()
}

pub(crate) fn u32_le(data: &Data, offset: u64) -> Option<u32> {
    let bytes = bytes(data, offset, 4)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}
