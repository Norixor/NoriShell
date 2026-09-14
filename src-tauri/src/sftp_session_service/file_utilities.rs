use std::{
    collections::BTreeSet,
    io::{Cursor, Read, Write},
};

use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

use super::{SftpRuntimeError, SftpRuntimeResult};

pub(super) const MAX_ARCHIVE_ENTRIES: usize = 10_000;
pub(super) const MAX_ARCHIVE_DEPTH: usize = 32;
pub(super) const MAX_ARCHIVE_INPUT_BYTES: usize = 256 * 1024 * 1024;
pub(super) const MAX_ARCHIVE_OUTPUT_BYTES: usize = 256 * 1024 * 1024;
pub(super) const MAX_EXTRACTED_BYTES: usize = 512 * 1024 * 1024;
pub(super) const MAX_NETWORK_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ArchiveEntry {
    pub(super) name: String,
    pub(super) is_directory: bool,
    pub(super) bytes: Vec<u8>,
}

pub(super) fn validate_archive_name(name: &str) -> SftpRuntimeResult<()> {
    if name.is_empty()
        || name.len() > 4096
        || name.starts_with('/')
        || name.ends_with('/')
        || name.contains('\\')
        || name.bytes().any(|byte| byte == 0)
    {
        return Err(SftpRuntimeError::InvalidInput);
    }
    let mut depth = 0usize;
    for component in name.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(SftpRuntimeError::InvalidInput);
        }
        depth = depth.saturating_add(1);
    }
    if depth > MAX_ARCHIVE_DEPTH {
        return Err(SftpRuntimeError::InvalidInput);
    }
    Ok(())
}

pub(super) fn encode_zip(entries: Vec<ArchiveEntry>) -> SftpRuntimeResult<Vec<u8>> {
    if entries.is_empty() || entries.len() > MAX_ARCHIVE_ENTRIES {
        return Err(SftpRuntimeError::InvalidInput);
    }
    let mut names = BTreeSet::new();
    let mut total_bytes = 0usize;
    for entry in &entries {
        validate_archive_name(entry.name.trim_end_matches('/'))?;
        if !names.insert(entry.name.clone()) {
            return Err(SftpRuntimeError::Conflict);
        }
        total_bytes = total_bytes
            .checked_add(entry.bytes.len())
            .ok_or(SftpRuntimeError::InvalidInput)?;
        if total_bytes > MAX_ARCHIVE_INPUT_BYTES {
            return Err(SftpRuntimeError::InvalidInput);
        }
    }

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);
    let directory_options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o755);
    for entry in entries {
        if entry.is_directory {
            let directory_name = format!("{}/", entry.name.trim_end_matches('/'));
            writer
                .add_directory(directory_name, directory_options)
                .map_err(|_| SftpRuntimeError::InvalidInput)?;
        } else {
            writer
                .start_file(entry.name, options)
                .map_err(|_| SftpRuntimeError::InvalidInput)?;
            writer
                .write_all(&entry.bytes)
                .map_err(|_| SftpRuntimeError::InvalidState)?;
        }
    }
    let bytes = writer
        .finish()
        .map_err(|_| SftpRuntimeError::InvalidState)?
        .into_inner();
    if bytes.is_empty() || bytes.len() > MAX_ARCHIVE_OUTPUT_BYTES {
        return Err(SftpRuntimeError::InvalidInput);
    }
    Ok(bytes)
}

pub(super) fn decode_zip(bytes: Vec<u8>) -> SftpRuntimeResult<Vec<ArchiveEntry>> {
    if bytes.is_empty() || bytes.len() > MAX_ARCHIVE_OUTPUT_BYTES {
        return Err(SftpRuntimeError::InvalidInput);
    }
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|_| SftpRuntimeError::InvalidInput)?;
    if archive.is_empty() || archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(SftpRuntimeError::InvalidInput);
    }
    let mut entries = Vec::with_capacity(archive.len());
    let mut names = BTreeSet::new();
    let mut total_bytes = 0usize;
    for index in 0..archive.len() {
        let file = archive
            .by_index(index)
            .map_err(|_| SftpRuntimeError::InvalidInput)?;
        if file.encrypted() {
            return Err(SftpRuntimeError::InvalidInput);
        }
        let raw_name = file.name().trim_end_matches('/');
        validate_archive_name(raw_name)?;
        let normalized_name = file
            .enclosed_name()
            .and_then(|path| path.to_str().map(ToOwned::to_owned))
            .ok_or(SftpRuntimeError::InvalidInput)?
            .trim_end_matches('/')
            .to_owned();
        validate_archive_name(&normalized_name)?;
        if raw_name != normalized_name || !names.insert(normalized_name.clone()) {
            return Err(SftpRuntimeError::InvalidInput);
        }
        if file
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(SftpRuntimeError::InvalidInput);
        }
        let is_directory = file.is_dir();
        let declared_size =
            usize::try_from(file.size()).map_err(|_| SftpRuntimeError::InvalidInput)?;
        total_bytes = total_bytes
            .checked_add(declared_size)
            .ok_or(SftpRuntimeError::InvalidInput)?;
        if total_bytes > MAX_EXTRACTED_BYTES {
            return Err(SftpRuntimeError::InvalidInput);
        }
        let mut entry_bytes = Vec::with_capacity(declared_size.min(64 * 1024));
        if !is_directory {
            file.take((declared_size as u64).saturating_add(1))
                .read_to_end(&mut entry_bytes)
                .map_err(|_| SftpRuntimeError::InvalidInput)?;
            if entry_bytes.len() != declared_size {
                return Err(SftpRuntimeError::LengthMismatch);
            }
        }
        entries.push(ArchiveEntry {
            name: normalized_name,
            is_directory,
            bytes: entry_bytes,
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_round_trip_preserves_files_and_directories() {
        let archive = encode_zip(vec![
            ArchiveEntry {
                name: "folder".to_owned(),
                is_directory: true,
                bytes: Vec::new(),
            },
            ArchiveEntry {
                name: "folder/readme.txt".to_owned(),
                is_directory: false,
                bytes: b"hello".to_vec(),
            },
        ])
        .unwrap();
        let extracted = decode_zip(archive).unwrap();
        assert_eq!(extracted.len(), 2);
        assert_eq!(extracted[1].bytes, b"hello");
    }

    #[test]
    fn archive_names_reject_traversal_and_backslashes() {
        for name in ["../secret", "a/../../secret", "/absolute", "a\\b"] {
            assert_eq!(
                validate_archive_name(name),
                Err(SftpRuntimeError::InvalidInput)
            );
        }
    }
}
