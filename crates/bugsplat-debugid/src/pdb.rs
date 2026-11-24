//! PDB ids, ported from `pdb-guid`: the GUID from the PDB info stream followed by the age
//! from the DBI stream for a native PDB, or by `FFFFFFFF` for a portable PDB.
//!
//! Both readers follow `pdb-guid`'s arithmetic step for step, including where it differs
//! from a full MSF reader (a deleted stream's size isn't treated as zero), so that a file
//! `pdb-guid` gives an id for gets the same one here.

use crate::data::{Data, bytes, bytes_up_to, u32_le};

pub(crate) const MSF_MAGIC: &[u8; 32] = b"Microsoft C/C++ MSF 7.00\r\n\x1aDS\0\0\0";

const PDB_INFO_STREAM: u64 = 1;
const DBI_STREAM: u64 = 3;
/// Version, signature and age, then the GUID.
const PDB_INFO_GUID_OFFSET: usize = 12;
const DBI_AGE_OFFSET: u64 = 8;
const PORTABLE_PDB_AGE: u32 = 0xffff_ffff;

pub(crate) fn msf_id(data: &Data) -> Result<String, String> {
    let superblock = MSF_MAGIC.len() as u64;
    let field = |index: u64, name: &str| match u32_le(data, superblock + index * 4) {
        Some(0) | None => Err(format!("the PDB superblock has no {name}")),
        Some(value) => Ok(u64::from(value)),
    };
    let block_size = field(0, "block size")?;
    let directory_bytes = field(3, "stream directory size")?;
    let block_map = field(5, "block map address")?;
    if directory_bytes % 4 != 0 {
        return Err("the PDB stream directory has a partial entry".to_string());
    }

    let directory = Directory {
        data,
        block_size,
        entries: directory_bytes / 4,
        block_map_offset: block_map * block_size,
    };
    let streams = u64::from(directory.entry(0)?);
    let blocks = |stream: u64| -> Result<u64, String> {
        if stream >= streams {
            return Err(format!("the PDB has no stream {stream}"));
        }
        Ok(u64::from(directory.entry(1 + stream)?).div_ceil(block_size))
    };
    let first_block = |stream: u64| -> Result<Option<u64>, String> {
        let mut entry = 1 + streams;
        for earlier in 0..stream {
            entry += blocks(earlier)?;
        }
        if blocks(stream)? == 0 {
            return Ok(None);
        }
        Ok(Some(u64::from(directory.entry(entry)?)))
    };

    let info_block = first_block(PDB_INFO_STREAM)?.ok_or("the PDB info stream is empty")?;
    let info =
        bytes(data, info_block * block_size, 28).ok_or("the PDB info stream is cut short")?;
    let guid = &info[PDB_INFO_GUID_OFFSET..];

    // The DBI stream's age, not the info stream's, is the one debuggers match on. A
    // compiler PDB such as vc140.pdb has an empty DBI stream and so no age.
    let age = match first_block(DBI_STREAM)? {
        Some(block) => u32_le(data, block * block_size + DBI_AGE_OFFSET)
            .ok_or("the PDB DBI stream is cut short")?,
        None => 0,
    };

    Ok(format_id(guid, age))
}

/// The stream directory: the stream count, each stream's size, then each stream's blocks.
/// It is spread over the blocks the block map lists, and read one entry at a time because
/// a large PDB's directory runs to megabytes.
struct Directory<'a> {
    data: &'a Data,
    block_size: u64,
    entries: u64,
    block_map_offset: u64,
}

impl Directory<'_> {
    fn entry(&self, index: u64) -> Result<u32, String> {
        let cut_short = || "the PDB stream directory is cut short".to_string();
        if index >= self.entries {
            return Err(cut_short());
        }
        let byte = index * 4;
        let block = u32_le(
            self.data,
            self.block_map_offset + byte / self.block_size * 4,
        )
        .ok_or_else(cut_short)?;
        u32_le(
            self.data,
            u64::from(block) * self.block_size + byte % self.block_size,
        )
        .ok_or_else(cut_short)
    }
}

pub(crate) fn portable_id(data: &Data) -> Result<String, String> {
    let cut_short = || "the portable PDB metadata is cut short".to_string();
    let version_len = u64::from(u32_le(data, 12).ok_or_else(cut_short)?);
    let streams_header = 16 + version_len.div_ceil(4) * 4;
    let header = bytes(data, streams_header, 4).ok_or_else(cut_short)?;
    let streams = u16::from_le_bytes([header[2], header[3]]);

    let mut offset = streams_header + 4;
    for _ in 0..streams {
        let stream_offset = u64::from(u32_le(data, offset).ok_or_else(cut_short)?);
        let stream_size = u32_le(data, offset + 4).ok_or_else(cut_short)?;
        let name = bytes_up_to(data, offset + 8, 64);
        let name = &name[..name.iter().position(|b| *b == 0).unwrap_or(name.len())];

        if name == b"#Pdb" {
            if stream_size < 20 {
                return Err("the portable PDB #Pdb stream is too small".to_string());
            }
            let guid = bytes(data, stream_offset, 16).ok_or_else(cut_short)?;
            return Ok(format_id(guid, PORTABLE_PDB_AGE));
        }

        let name_units = String::from_utf8_lossy(name).encode_utf16().count() as u64;
        offset += 8 + (name_units + 1).div_ceil(4) * 4;
    }
    Err("the portable PDB has no #Pdb stream".to_string())
}

/// `pdb-guid`'s `PdbGuid.toString`: the GUID's three little-endian fields and eight bytes
/// as uppercase hex, then the age without padding. An age of zero is left out.
fn format_id(guid: &[u8], age: u32) -> String {
    let data1 = u32::from_le_bytes([guid[0], guid[1], guid[2], guid[3]]);
    let data2 = u16::from_le_bytes([guid[4], guid[5]]);
    let data3 = u16::from_le_bytes([guid[6], guid[7]]);
    let mut id = format!("{data1:08X}{data2:04X}{data3:04X}");
    for byte in &guid[8..16] {
        id.push_str(&format!("{byte:02X}"));
    }
    if age != 0 {
        id.push_str(&format!("{age:X}"));
    }
    id
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    const GUID: [u8; 16] = [1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1];

    #[test]
    fn formats_like_pdb_guid() {
        assert_eq!(format_id(&GUID, 1), "000000010001000100000000000000011");
        assert_eq!(
            format_id(&GUID, 0xffff_ffff),
            "00000001000100010000000000000001FFFFFFFF"
        );
        assert_eq!(format_id(&GUID, 0), "00000001000100010000000000000001");
        assert_eq!(format_id(&GUID, 0x1c), "000000010001000100000000000000011C");
    }

    /// An MSF file whose block map is block 100 and whose stream directory follows it.
    /// `streams` holds each stream's size and blocks; `blocks` holds block contents.
    fn msf(block_size: u32, streams: &[(u32, &[u32])], blocks: &[(u32, &[u8])]) -> Vec<u8> {
        let mut directory = vec![streams.len() as u32];
        directory.extend(streams.iter().map(|(size, _)| *size));
        directory.extend(
            streams
                .iter()
                .flat_map(|(_, blocks)| blocks.iter().copied()),
        );
        let directory: Vec<u8> = directory.iter().flat_map(|v| v.to_le_bytes()).collect();

        let size = block_size as usize;
        let directory_blocks = directory.len().div_ceil(size) as u32;
        let block_map = 100;
        let first_directory_block = block_map + 1;

        let mut file = vec![0u8; (first_directory_block + directory_blocks) as usize * size];
        file[..32].copy_from_slice(MSF_MAGIC);
        let superblock = [block_size, 0, 0, directory.len() as u32, 0, block_map];
        for (i, value) in superblock.iter().enumerate() {
            file[32 + i * 4..36 + i * 4].copy_from_slice(&value.to_le_bytes());
        }
        for i in 0..directory_blocks {
            let at = block_map as usize * size + i as usize * 4;
            file[at..at + 4].copy_from_slice(&(first_directory_block + i).to_le_bytes());
        }
        let at = first_directory_block as usize * size;
        file[at..at + directory.len()].copy_from_slice(&directory);
        for (block, content) in blocks {
            let at = *block as usize * size;
            file[at..at + content.len()].copy_from_slice(content);
        }
        file
    }

    fn info_stream() -> Vec<u8> {
        let mut info = vec![0u8; 12];
        info.extend(GUID);
        info
    }

    fn id_of(file: &[u8]) -> Result<String, String> {
        let mut temp = tempfile::tempfile().unwrap();
        temp.write_all(file).unwrap();
        msf_id(&Data::new(temp))
    }

    #[test]
    fn reads_the_guid_and_the_dbi_age() {
        let mut dbi = vec![0u8; 8];
        dbi.extend(0x2a_u32.to_le_bytes());
        let file = msf(
            64,
            &[(48, &[9]), (28, &[20]), (130, &[30, 31, 32]), (12, &[40])],
            &[(20, &info_stream()), (40, &dbi)],
        );
        assert_eq!(id_of(&file).unwrap(), "000000010001000100000000000000012A");
    }

    #[test]
    fn an_empty_dbi_stream_means_no_age() {
        let file = msf(
            64,
            &[(48, &[9]), (28, &[20]), (130, &[30, 31, 32]), (0, &[])],
            &[(20, &info_stream())],
        );
        assert_eq!(id_of(&file).unwrap(), "00000001000100010000000000000001");
    }

    #[test]
    fn follows_a_directory_that_spans_blocks() {
        let mut dbi = vec![0u8; 8];
        dbi.extend(7_u32.to_le_bytes());
        let old_directory: Vec<u32> = (50..60).collect();
        let file = msf(
            32,
            &[(320, &old_directory), (28, &[20]), (0, &[]), (12, &[40])],
            &[(20, &info_stream()), (40, &dbi)],
        );
        assert_eq!(id_of(&file).unwrap(), "000000010001000100000000000000017");
    }

    #[test]
    fn a_pdb_without_a_dbi_stream_is_an_error() {
        let file = msf(
            64,
            &[(48, &[9]), (28, &[20]), (0, &[])],
            &[(20, &info_stream())],
        );
        assert_eq!(id_of(&file).unwrap_err(), "the PDB has no stream 3");
    }

    #[test]
    fn a_truncated_pdb_is_an_error() {
        let file = msf(
            64,
            &[(48, &[9]), (28, &[20]), (0, &[]), (0, &[])],
            &[(20, &info_stream())],
        );
        assert!(id_of(&file[..40]).is_err());
        assert!(id_of(&file[..100 * 64]).is_err());
    }
}
