//! ELF ids as `symbol-upload`'s `tryGetElfUUID` produces them: up to 20 bytes of the
//! `.note.gnu.build-id` section after its 16-byte note header, else the first 20 bytes of
//! `.sce_special`.

use object::elf;
use object::read::ReadRef;
use object::read::elf::{FileHeader, SectionHeader};
use object::{Endianness, FileKind};

use crate::data::Data;
use crate::hex_lower;

const NOTE_HEADER_LEN: usize = 16;
const ID_LEN: usize = 20;

pub(crate) struct Elf {
    pub id: String,
    pub arch: Option<String>,
}

pub(crate) fn read(data: &Data) -> Result<Elf, String> {
    match FileKind::parse(data).map_err(|err| err.to_string())? {
        FileKind::Elf32 => read_class::<elf::FileHeader32<Endianness>>(data),
        FileKind::Elf64 => read_class::<elf::FileHeader64<Endianness>>(data),
        _ => Err("not an ELF file".to_string()),
    }
}

fn read_class<Header: FileHeader<Endian = Endianness>>(data: &Data) -> Result<Elf, String> {
    let header = Header::parse(data).map_err(|err| err.to_string())?;
    let endian = header.endian().map_err(|err| err.to_string())?;

    // The section is found by name and read whatever its type, as elfy does. A file whose
    // section table can't be read has no id, which is also what symbol-upload reports.
    let sections = header.sections(endian, data).ok();
    // Only the id's bytes are read; the section itself can be large. A section that runs
    // past the end of the file still counts as absent.
    let section = |name: &[u8], wanted: usize| {
        let (_, section) = sections.as_ref()?.section_by_name(endian, name)?;
        let offset: u64 = section.sh_offset(endian).into();
        let size: u64 = section.sh_size(endian).into();
        if offset.checked_add(size)? > data.len().ok()? {
            return None;
        }
        data.read_bytes_at(offset, size.min(wanted as u64)).ok()
    };

    let id = if let Some(note) = section(b".note.gnu.build-id", NOTE_HEADER_LEN + ID_LEN) {
        note.get(NOTE_HEADER_LEN..).map(first_id_bytes)
    } else {
        section(b".sce_special", ID_LEN).map(first_id_bytes)
    };

    Ok(Elf {
        id: hex_lower(id.unwrap_or_default()),
        arch: arch(header.e_machine(endian)),
    })
}

fn first_id_bytes(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes.len().min(ID_LEN)]
}

fn arch(machine: elf::Machine) -> Option<String> {
    let name = match machine {
        elf::EM_386 => "x86",
        elf::EM_X86_64 => "x86_64",
        elf::EM_ARM => "arm",
        elf::EM_AARCH64 => "arm64",
        elf::EM_MIPS => "mips",
        elf::EM_PPC => "ppc",
        elf::EM_PPC64 => "ppc64",
        elf::EM_RISCV => "riscv",
        _ => return None,
    };
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    /// A little-endian ELF file with a null section, the given sections and a section name
    /// table, in that order.
    fn elf(class64: bool, sections: &[(&str, &[u8])]) -> Vec<u8> {
        let header_len = if class64 { 64 } else { 52 };
        let section_header_len = if class64 { 64 } else { 40 };
        let word = |value: u64| -> Vec<u8> {
            if class64 {
                value.to_le_bytes().to_vec()
            } else {
                (value as u32).to_le_bytes().to_vec()
            }
        };

        let mut names = vec![0u8];
        let mut name_offsets = Vec::new();
        for (name, _) in sections.iter().chain(&[(".shstrtab", &[][..])]) {
            name_offsets.push(names.len() as u32);
            names.extend(name.as_bytes());
            names.push(0);
        }

        let mut body: Vec<u8> = Vec::new();
        let mut placed = Vec::new();
        for (_, content) in sections {
            placed.push((header_len + body.len(), content.len()));
            body.extend(*content);
        }
        placed.push((header_len + body.len(), names.len()));
        body.extend(&names);
        let section_headers_offset = header_len + body.len();

        let mut file = vec![0x7f, b'E', b'L', b'F'];
        file.extend([if class64 { 2 } else { 1 }, 1, 1, 0]);
        file.extend([0u8; 8]);
        file.extend(2u16.to_le_bytes());
        file.extend(62u16.to_le_bytes());
        file.extend(1u32.to_le_bytes());
        file.extend(word(0));
        file.extend(word(0));
        file.extend(word(section_headers_offset as u64));
        file.extend(0u32.to_le_bytes());
        file.extend((header_len as u16).to_le_bytes());
        file.extend([0u8; 4]);
        file.extend((section_header_len as u16).to_le_bytes());
        file.extend((placed.len() as u16 + 1).to_le_bytes());
        file.extend((placed.len() as u16).to_le_bytes());
        assert_eq!(file.len(), header_len);
        file.extend(body);

        file.extend(vec![0u8; section_header_len]);
        for (index, (offset, len)) in placed.iter().enumerate() {
            file.extend(name_offsets[index].to_le_bytes());
            file.extend(1u32.to_le_bytes());
            file.extend(word(0));
            file.extend(word(0));
            file.extend(word(*offset as u64));
            file.extend(word(*len as u64));
            file.extend([0u8; 8]);
            file.extend(word(1));
            file.extend(word(0));
        }
        file
    }

    fn note(id: &[u8]) -> Vec<u8> {
        let mut note = Vec::new();
        note.extend(4u32.to_le_bytes());
        note.extend((id.len() as u32).to_le_bytes());
        note.extend(3u32.to_le_bytes());
        note.extend(b"GNU\0");
        note.extend(id);
        note
    }

    fn id_of(file: &[u8]) -> String {
        let mut temp = tempfile::tempfile().unwrap();
        temp.write_all(file).unwrap();
        read(&Data::new(temp)).unwrap().id
    }

    const SHA1: [u8; 20] = [
        0x85, 0xfe, 0x21, 0x6f, 0xc7, 0xdd, 0x44, 0x1f, 0x04, 0xc2, 0x37, 0x31, 0x0a, 0x56, 0x08,
        0x1f, 0xbf, 0x23, 0xc0, 0x82,
    ];

    #[test]
    fn reads_the_build_id_note() {
        let file = elf(
            true,
            &[(".text", &[0x90]), (".note.gnu.build-id", &note(&SHA1))],
        );
        assert_eq!(id_of(&file), "85fe216fc7dd441f04c237310a56081fbf23c082");
    }

    #[test]
    fn a_long_build_id_is_cut_to_20_bytes_and_a_short_one_kept() {
        let long: Vec<u8> = (0..32).collect();
        let file = elf(true, &[(".note.gnu.build-id", &note(&long))]);
        assert_eq!(id_of(&file), "000102030405060708090a0b0c0d0e0f10111213");

        let file = elf(true, &[(".note.gnu.build-id", &note(&long[..8]))]);
        assert_eq!(id_of(&file), "0001020304050607");
    }

    #[test]
    fn falls_back_to_sce_special() {
        let special: Vec<u8> = (0x40..0x60).collect();
        let file = elf(true, &[(".sce_special", &special)]);
        assert_eq!(id_of(&file), "404142434445464748494a4b4c4d4e4f50515253");
    }

    #[test]
    fn the_build_id_note_comes_before_sce_special() {
        let special: Vec<u8> = (0x40..0x60).collect();
        let file = elf(
            true,
            &[
                (".sce_special", &special),
                (".note.gnu.build-id", &note(&SHA1)),
            ],
        );
        assert_eq!(id_of(&file), "85fe216fc7dd441f04c237310a56081fbf23c082");
    }

    #[test]
    fn an_elf_without_either_section_has_no_id() {
        assert_eq!(id_of(&elf(true, &[(".text", &[0x90])])), "");
    }

    #[test]
    fn an_elf_with_a_truncated_section_table_has_no_id() {
        let file = elf(true, &[(".note.gnu.build-id", &note(&SHA1))]);
        assert_eq!(id_of(&file[..file.len() - 10]), "");
    }

    #[test]
    fn reads_a_32_bit_elf_the_same_way() {
        let file = elf(false, &[(".note.gnu.build-id", &note(&SHA1))]);
        assert_eq!(id_of(&file), "85fe216fc7dd441f04c237310a56081fbf23c082");
    }
}
