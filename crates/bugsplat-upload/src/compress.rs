use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom};
use std::path::Path;

use bugsplat_debugid::Slice;
use flate2::Compression;
use flate2::write::GzEncoder;
use tempfile::{NamedTempFile, TempPath};
use zip::write::SimpleFileOptions;

/// A compressed copy in the run's temporary folder, deleted when dropped.
pub(crate) struct Compressed {
    pub path: TempPath,
    pub size: u64,
    pub source_size: u64,
}

pub(crate) fn is_zip(path: &Path) -> bool {
    crate::find::has_extension(path, "zip")
}

/// The whole file, or the bytes of one architecture inside a fat Mach-O file.
pub(crate) fn open(source: &Path, slice: Option<Slice>) -> io::Result<impl Read> {
    let mut file = File::open(source)?;
    let length = match slice {
        Some(slice) => {
            if slice.offset.saturating_add(slice.size) > file.metadata()?.len() {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("{}: the file ends before its slice does", source.display()),
                ));
            }
            file.seek(SeekFrom::Start(slice.offset))?;
            slice.size
        }
        None => u64::MAX,
    };
    Ok(file.take(length))
}

pub(crate) fn gzip(source: &Path, slice: Option<Slice>, tmp_dir: &Path) -> io::Result<Compressed> {
    let mut reader = open(source, slice)?;
    let tmp = NamedTempFile::new_in(tmp_dir)?;
    let mut encoder = GzEncoder::new(BufWriter::new(tmp), Compression::default());
    let source_size = io::copy(&mut reader, &mut encoder)?;
    let tmp = encoder
        .finish()?
        .into_inner()
        .map_err(|error| error.into_error())?;
    finish(tmp, source_size)
}

pub(crate) fn zip(source: &Path, entry_name: &str, tmp_dir: &Path) -> io::Result<Compressed> {
    let mut reader = File::open(source)?;
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(reader.metadata()?.len() > u64::from(u32::MAX));
    let mut writer = zip::ZipWriter::new(NamedTempFile::new_in(tmp_dir)?);
    writer.start_file(entry_name, options)?;
    let source_size = io::copy(&mut reader, &mut writer)?;
    finish(writer.finish()?, source_size)
}

fn finish(tmp: NamedTempFile, source_size: u64) -> io::Result<Compressed> {
    let size = tmp.as_file().metadata()?.len();
    Ok(Compressed {
        path: tmp.into_temp_path(),
        size,
        source_size,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::read::GzDecoder;

    use super::*;

    fn source(contents: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.pdb");
        File::create(&path).unwrap().write_all(contents).unwrap();
        (dir, path)
    }

    fn gunzip(path: &Path) -> Vec<u8> {
        let mut out = Vec::new();
        GzDecoder::new(File::open(path).unwrap())
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    #[test]
    fn gzip_round_trips_the_whole_file() {
        let contents = b"symbols ".repeat(1000);
        let (dir, path) = source(&contents);
        let compressed = gzip(&path, None, dir.path()).unwrap();
        assert_eq!(gunzip(&compressed.path), contents);
        assert_eq!(
            compressed.size,
            std::fs::metadata(&compressed.path).unwrap().len()
        );
        assert!(compressed.size < contents.len() as u64);
    }

    #[test]
    fn gzip_of_a_slice_holds_only_that_slice() {
        let (dir, path) = source(b"0123456789abcdef");
        let slice = Slice { offset: 4, size: 6 };
        let compressed = gzip(&path, Some(slice), dir.path()).unwrap();
        assert_eq!(gunzip(&compressed.path), b"456789");
        assert_eq!(compressed.source_size, 6);
    }

    #[test]
    fn a_slice_past_the_end_of_the_file_is_an_error() {
        let (dir, path) = source(b"0123456789");
        let slice = Slice { offset: 8, size: 6 };
        assert!(gzip(&path, Some(slice), dir.path()).is_err());
    }

    #[test]
    fn zip_holds_one_entry_with_the_given_name() {
        let contents = br#"{"version":3}"#;
        let (dir, path) = source(contents);
        let compressed = zip(&path, "main.js.map", dir.path()).unwrap();
        let mut archive = zip::ZipArchive::new(File::open(&compressed.path).unwrap()).unwrap();
        assert_eq!(archive.len(), 1);
        let mut entry = archive.by_index(0).unwrap();
        assert_eq!(entry.name().unwrap(), "main.js.map");
        let mut out = Vec::new();
        entry.read_to_end(&mut out).unwrap();
        assert_eq!(out, contents);
    }

    #[test]
    fn the_temporary_file_is_deleted_when_dropped() {
        let (dir, path) = source(b"x");
        let compressed = gzip(&path, None, dir.path()).unwrap();
        let tmp = compressed.path.to_path_buf();
        assert!(tmp.exists());
        drop(compressed);
        assert!(!tmp.exists());
    }
}
