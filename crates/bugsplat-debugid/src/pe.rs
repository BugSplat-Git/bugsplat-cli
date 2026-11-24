//! PE ids as `pdb-guid`'s `PeFile` and `PeGuid` produce them: the COFF timestamp followed by
//! the size of the image.

use object::LittleEndian as LE;
use object::pe;
use object::read::ReadRef;

use crate::data::Data;

pub(crate) struct Pe {
    pub id: String,
    pub arch: Option<String>,
}

/// `None` when the file has no PE signature where its DOS header says to look.
pub(crate) fn read(data: &Data) -> Option<Pe> {
    let dos = pe::ImageDosHeader::parse(data).ok()?;
    // SizeOfImage sits at the same offset in the 32-bit and 64-bit optional headers, so the
    // 32-bit layout reads it from either.
    let nt = data
        .read_at::<pe::ImageNtHeaders32>(dos.nt_headers_offset().into())
        .ok()?;
    if nt.signature.get(LE) != pe::IMAGE_NT_SIGNATURE {
        return None;
    }
    Some(Pe {
        id: format_id(
            nt.file_header.time_date_stamp.get(LE),
            nt.optional_header.size_of_image.get(LE),
        ),
        arch: arch(nt.file_header.machine.get(LE)),
    })
}

/// A zero timestamp, which reproducible builds can write, is padded to 32 digits.
pub(crate) fn format_id(time_date_stamp: u32, size_of_image: u32) -> String {
    let width = if time_date_stamp == 0 { 32 } else { 8 };
    format!("{time_date_stamp:0width$X}{size_of_image:04X}")
}

fn arch(machine: pe::Machine) -> Option<String> {
    let name = match machine {
        pe::IMAGE_FILE_MACHINE_I386 => "x86",
        pe::IMAGE_FILE_MACHINE_AMD64 => "x86_64",
        pe::IMAGE_FILE_MACHINE_ARM64 => "arm64",
        pe::IMAGE_FILE_MACHINE_ARM64EC => "arm64ec",
        pe::IMAGE_FILE_MACHINE_ARM | pe::IMAGE_FILE_MACHINE_ARMNT => "arm",
        _ => return None,
    };
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_pe_guid() {
        assert_eq!(format_id(1, 1), "000000010001");
        assert_eq!(format_id(0x64FB82D5, 0x65000), "64FB82D565000");
        assert_eq!(
            format_id(0, 0x18000),
            "0000000000000000000000000000000018000"
        );
    }
}
