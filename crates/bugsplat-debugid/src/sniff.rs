//! Which reader a file's first bytes call for.

use crate::data::{Data, bytes_up_to};
use crate::pdb::MSF_MAGIC;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    Msf,
    PortablePdb,
    /// An MZ executable, which is a PE only if its PE signature checks out.
    Mz,
    Elf,
    MachO,
    MachOFat,
    Sym,
}

const HEAD_LEN: u64 = 32;

pub(crate) fn head(data: &Data) -> &[u8] {
    bytes_up_to(data, 0, HEAD_LEN)
}

pub(crate) fn sniff(head: &[u8]) -> Option<Format> {
    if head.starts_with(MSF_MAGIC) {
        return Some(Format::Msf);
    }
    if head.starts_with(b"MODULE ") {
        return Some(Format::Sym);
    }
    if head.starts_with(b"MZ") {
        return Some(Format::Mz);
    }
    match head.get(..4)? {
        b"BSJB" => Some(Format::PortablePdb),
        b"\x7fELF" => Some(Format::Elf),
        [0xfe, 0xed, 0xfa, 0xce | 0xcf] | [0xce | 0xcf, 0xfa, 0xed, 0xfe] => Some(Format::MachO),
        [0xca, 0xfe, 0xba, 0xbe] => {
            // Java class files start with the same four bytes. Where a fat header counts its
            // slices, a class file has its version, which is 45 or more.
            let slices = u32::from_be_bytes(head.get(4..8)?.try_into().ok()?);
            (slices < 45).then_some(Format::MachOFat)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_each_format() {
        assert_eq!(sniff(MSF_MAGIC), Some(Format::Msf));
        assert_eq!(sniff(b"BSJB\x01\0\x01\0"), Some(Format::PortablePdb));
        assert_eq!(sniff(b"MZ\x90\0"), Some(Format::Mz));
        assert_eq!(sniff(b"\x7fELF\x02\x01\x01"), Some(Format::Elf));
        assert_eq!(sniff(b"\xcf\xfa\xed\xfe\x07\0\0\x01"), Some(Format::MachO));
        assert_eq!(sniff(b"\xce\xfa\xed\xfe\x07\0\0\0"), Some(Format::MachO));
        assert_eq!(sniff(b"\xfe\xed\xfa\xcf\x01\0\0\x12"), Some(Format::MachO));
        assert_eq!(sniff(b"\xca\xfe\xba\xbe\0\0\0\x02"), Some(Format::MachOFat));
        assert_eq!(sniff(b"MODULE Linux arm64 0 a"), Some(Format::Sym));
    }

    #[test]
    fn a_java_class_is_not_a_fat_binary() {
        assert_eq!(sniff(b"\xca\xfe\xba\xbe\0\0\0\x34\0\x10"), None);
    }

    #[test]
    fn other_bytes_are_nothing() {
        assert_eq!(sniff(b""), None);
        assert_eq!(sniff(b"MZ"), Some(Format::Mz));
        assert_eq!(sniff(b"not an exe!"), None);
        assert_eq!(sniff(b"\xca\xfe\xba\xbe"), None);
    }
}
