//! Mach-O ids as `macho-uuid` produces them: each slice's `LC_UUID`, lowercase.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use object::read::ReadRef;
use object::read::macho::{FatArch, MachHeader, MachOFatFile32, MachOFatFile64};
use object::{BigEndian, Endianness, FileKind, U32, macho};

use crate::data::Data;
use crate::sniff::{self, Format};
use crate::{DebugInfo, ObjectKind, Slice, file_name, hex_lower};

pub(crate) struct Thin {
    pub uuid: Option<String>,
    pub arch: Option<String>,
}

pub(crate) fn read_thin(data: &Data, offset: u64) -> Result<Thin, String> {
    let magic = data
        .read_at::<U32<BigEndian>>(offset)
        .map_err(|()| "the Mach-O header is cut short".to_string())?
        .get(BigEndian);
    match magic {
        macho::MH_MAGIC | macho::MH_CIGAM => {
            read_header::<macho::MachHeader32<Endianness>>(data, offset)
        }
        macho::MH_MAGIC_64 | macho::MH_CIGAM_64 => {
            read_header::<macho::MachHeader64<Endianness>>(data, offset)
        }
        _ => Err("not a Mach-O header".to_string()),
    }
}

fn read_header<Header: MachHeader<Endian = Endianness>>(
    data: &Data,
    offset: u64,
) -> Result<Thin, String> {
    let header = Header::parse(data, offset).map_err(|err| err.to_string())?;
    let endian = header.endian().map_err(|err| err.to_string())?;
    let uuid = header
        .uuid(endian, data, offset)
        .map_err(|err| err.to_string())?;
    Ok(Thin {
        uuid: uuid.map(|uuid| hex_lower(&uuid)),
        arch: arch(header.cputype(endian), header.cpusubtype(endian)),
    })
}

/// The slices of a fat file that have a UUID. `macho-uuid` drops the others, which is how a
/// fat static library yields nothing.
pub(crate) fn read_fat(data: &Data) -> Result<Vec<(Slice, Thin)>, String> {
    let ranges: Vec<(u64, u64)> = match FileKind::parse(data).map_err(|err| err.to_string())? {
        FileKind::MachOFat64 => MachOFatFile64::parse(data)
            .map_err(|err| err.to_string())?
            .arches()
            .iter()
            .map(FatArch::file_range)
            .collect(),
        _ => MachOFatFile32::parse(data)
            .map_err(|err| err.to_string())?
            .arches()
            .iter()
            .map(FatArch::file_range)
            .collect(),
    };
    Ok(ranges
        .into_iter()
        .filter_map(|(offset, size)| {
            let thin = read_thin(data, offset).ok()?;
            thin.uuid
                .is_some()
                .then_some((Slice { offset, size }, thin))
        })
        .collect())
}

pub(crate) fn read_bundle(root: &Path) -> io::Result<Vec<DebugInfo>> {
    let mut files = Vec::new();
    collect_files(root, &mut files)?;

    let mut unique: Vec<(DebugInfo, u64)> = Vec::new();
    for path in files {
        for (object, file_len) in read_objects(&path) {
            match unique
                .iter_mut()
                .find(|(existing, _)| existing.debug_id == object.debug_id)
            {
                None => unique.push((object, file_len)),
                Some(existing) => {
                    if replaces(&object.path, file_len, &existing.0.path, existing.1) {
                        *existing = (object, file_len);
                    }
                }
            }
        }
    }
    Ok(unique.into_iter().map(|(object, _)| object).collect())
}

/// An executable and its dSYM carry the same UUID. The dSYM has the symbols, so it wins;
/// between two files outside a dSYM, the larger one does.
fn replaces(path: &Path, file_len: u64, existing: &Path, existing_len: u64) -> bool {
    // A folder whose extension is dSYM, not any path that happens to contain the text.
    let in_dsym = |path: &Path| {
        path.ancestors().skip(1).any(|folder| {
            folder
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("dsym"))
        })
    };
    in_dsym(path) || (!in_dsym(existing) && file_len >= existing_len)
}

/// The objects with a UUID in one file of a bundle, each with the file's length. Files that
/// aren't Mach-O, or can't be read, have none.
fn read_objects(path: &Path) -> Vec<(DebugInfo, u64)> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let file_len = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let data = &Data::new(file);
    let slices = match sniff::sniff(sniff::head(data)) {
        Some(Format::MachO) => read_thin(data, 0).map(|thin| vec![(None, thin)]),
        Some(Format::MachOFat) => read_fat(data).map(|slices| {
            slices
                .into_iter()
                .map(|(slice, thin)| (Some(slice), thin))
                .collect()
        }),
        _ => return Vec::new(),
    };
    slices
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(slice, thin)| {
            let object = DebugInfo {
                path: path.to_path_buf(),
                module_name: file_name(path),
                debug_id: thin.uuid?,
                kind: ObjectKind::MachO,
                arch: thin.arch,
                slice,
            };
            Some((object, file_len))
        })
        .collect()
}

/// Regular files under `dir`, in name order. Symlinks are skipped: a framework's links point
/// back into the bundle, at files the walk reaches anyway.
fn collect_files(dir: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut entries = std::fs::read_dir(dir)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files(&entry.path(), files)?;
        } else if file_type.is_file() {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn arch(cpu: macho::CpuType, subtype: macho::CpuSubtype) -> Option<String> {
    let subtype = subtype.id();
    let name = match cpu {
        macho::CPU_TYPE_X86 => "i386",
        macho::CPU_TYPE_X86_64 if subtype == macho::CPU_SUBTYPE_X86_64_H => "x86_64h",
        macho::CPU_TYPE_X86_64 => "x86_64",
        macho::CPU_TYPE_ARM64 if subtype == macho::CPU_SUBTYPE_ARM64E => "arm64e",
        macho::CPU_TYPE_ARM64 => "arm64",
        macho::CPU_TYPE_ARM64_32 => "arm64_32",
        macho::CPU_TYPE_ARM if subtype == macho::CPU_SUBTYPE_ARM_V6 => "armv6",
        macho::CPU_TYPE_ARM if subtype == macho::CPU_SUBTYPE_ARM_V7 => "armv7",
        macho::CPU_TYPE_ARM if subtype == macho::CPU_SUBTYPE_ARM_V7S => "armv7s",
        macho::CPU_TYPE_ARM if subtype == macho::CPU_SUBTYPE_ARM_V7K => "armv7k",
        macho::CPU_TYPE_ARM => "arm",
        macho::CPU_TYPE_POWERPC => "ppc",
        macho::CPU_TYPE_POWERPC64 => "ppc64",
        _ => return None,
    };
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dsym_copy_replaces_any_other() {
        let app = Path::new("a.xcarchive/Products/Applications/a.app/Contents/MacOS/a");
        let dsym = Path::new("a.xcarchive/dSYMs/a.app.dSYM/Contents/Resources/DWARF/a");
        assert!(replaces(dsym, 1, app, 100));
        assert!(!replaces(app, 100, dsym, 1));
        assert!(replaces(dsym, 1, dsym, 100));
    }

    #[test]
    fn outside_a_dsym_the_larger_file_wins() {
        let small = Path::new("a.app/Contents/MacOS/a");
        let large = Path::new("a.app/Contents/Frameworks/a.framework/a");
        assert!(replaces(large, 100, small, 1));
        assert!(!replaces(small, 1, large, 100));
        assert!(replaces(small, 100, large, 100));
    }

    #[test]
    fn only_a_folder_with_the_dsym_extension_counts_as_a_dsym() {
        let cache = Path::new("build.dsym-cache/a.app/Contents/MacOS/a");
        let app = Path::new("out/a.app/Contents/MacOS/a");
        assert!(!replaces(cache, 1, app, 100));
        assert!(replaces(app, 100, cache, 1));
    }
}
