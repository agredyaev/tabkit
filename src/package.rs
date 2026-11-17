//! TWB/TWBX admission and source-preserving repack.
//!
//! Opening a package establishes a snapshot contract: its hash and, for TWBX,
//! entry metadata are retained so later writes can detect a stale source.
//! Archive paths, entry count, expanded size, compression ratio, symlinks, and
//! duplicate/case-colliding names are rejected before the embedded TWB is used.
//!
//! Repacking changes only the embedded TWB. Unrelated TWBX entries must retain
//! their order, names, sizes, compression, and compressed bytes.

use crate::{Error, Result, require};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

/// Hard admission limits shared by package I/O and REST transfer paths.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub xml_bytes: u64,
    pub file_bytes: u64,
    pub zip_entries: usize,
    pub zip_expanded_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            xml_bytes: 256 << 20,
            file_bytes: 4 << 30,
            zip_entries: 8192,
            zip_expanded_bytes: 16 << 30,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Twb,
    Twbx,
}

#[derive(Clone, Debug)]
struct Entry {
    name: String,
    size: u64,
    compressed_size: u64,
    compressed_sha256: String,
}

/// Snapshot of an admitted package used to enforce stale-base and preservation checks.
pub struct Package {
    pub path: PathBuf,
    pub kind: Kind,
    pub sha256: String,
    entries: Vec<Entry>,
    twb_entry: Option<usize>,
}

impl Package {
    /// Admits a TWB/TWBX file and captures the snapshot required for safe edits.
    ///
    /// For TWB the file is read under `xml_bytes` and re-hashed to reject a
    /// concurrent change. For TWBX every archive entry is checked before the
    /// single embedded TWB is returned, and the package is re-hashed after
    /// indexing. A successful `Package` therefore represents the exact source
    /// bytes observed during admission.
    pub fn open(path: &Path, limits: &Limits) -> Result<(Self, Vec<u8>)> {
        require(
            std::fs::metadata(path)?.len() <= limits.file_bytes,
            "LIMIT",
            "Workbook package is too large",
        )?;

        match path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("twb") => {
                let twb = read_bounded(path, limits.xml_bytes)?;
                let sha256 = sha256(&twb);
                require(
                    hash_file(path, limits.file_bytes)? == sha256,
                    "STALE_BASE",
                    "Input changed while reading",
                )?;
                Ok((
                    Self {
                        path: path.to_owned(),
                        kind: Kind::Twb,
                        sha256,
                        entries: Vec::new(),
                        twb_entry: None,
                    },
                    twb,
                ))
            }
            Some("twbx") => {
                let package_hash = hash_file(path, limits.file_bytes)?;
                let mut archive = ZipArchive::new(File::open(path)?)?;
                require(
                    archive.len() <= limits.zip_entries,
                    "ZIP_LIMIT",
                    "Too many archive entries",
                )?;

                let mut names = BTreeSet::new();
                let mut expanded = 0u64;
                let mut entries = Vec::with_capacity(archive.len());
                let mut twb_entry = None;
                let mut raw = File::open(path)?;

                for index in 0..archive.len() {
                    let file = archive.by_index(index)?;
                    let name = file.name()?.into_owned();
                    safe_entry(&name)?;
                    require(
                        names.insert(name.to_ascii_lowercase()),
                        "ZIP_DUPLICATE",
                        "Duplicate or case-colliding ZIP entry",
                    )?;
                    require(
                        file.unix_mode()
                            .map(|mode| mode & 0o170000 != 0o120000)
                            .unwrap_or(true),
                        "ZIP_SYMLINK",
                        "Symlink ZIP entries are forbidden",
                    )?;
                    expanded = expanded
                        .checked_add(file.size())
                        .ok_or_else(|| Error::new("ZIP_LIMIT", "Expanded size overflow"))?;
                    require(
                        expanded <= limits.zip_expanded_bytes,
                        "ZIP_LIMIT",
                        "Expanded archive exceeds configured limit",
                    )?;
                    require(
                        file.size()
                            <= file
                                .compressed_size()
                                .saturating_mul(5000)
                                .saturating_add(1 << 20),
                        "ZIP_LIMIT",
                        "Suspicious compression ratio",
                    )?;

                    entries.push(Entry {
                        name: name.clone(),
                        size: file.size(),
                        compressed_size: file.compressed_size(),
                        compressed_sha256: hash_region(
                            &mut raw,
                            file.data_start().ok_or_else(|| {
                                Error::new("ZIP", "ZIP entry data offset is unavailable")
                            })?,
                            file.compressed_size(),
                        )?,
                    });

                    if name.to_ascii_lowercase().ends_with(".twb") {
                        require(
                            twb_entry.is_none(),
                            "AMBIGUOUS_PACKAGE",
                            "TWBX contains multiple TWB files",
                        )?;
                        twb_entry = Some(index);
                    }
                }

                let twb_entry =
                    twb_entry.ok_or_else(|| Error::new("FORMAT", "TWBX contains no TWB"))?;
                let mut twb = Vec::new();
                archive
                    .by_index(twb_entry)?
                    .take(limits.xml_bytes + 1)
                    .read_to_end(&mut twb)?;
                require(
                    twb.len() as u64 <= limits.xml_bytes,
                    "LIMIT",
                    "Embedded TWB exceeds configured limit",
                )?;
                require(
                    hash_file(path, limits.file_bytes)? == package_hash,
                    "STALE_BASE",
                    "Input changed while indexing",
                )?;

                Ok((
                    Self {
                        path: path.to_owned(),
                        kind: Kind::Twbx,
                        sha256: package_hash,
                        entries,
                        twb_entry: Some(twb_entry),
                    },
                    twb,
                ))
            }
            _ => Err(Error::new("FORMAT", "Only .twb and .twbx are supported")),
        }
    }

    /// Writes a candidate only if the admitted source snapshot is still current.
    ///
    /// The output must keep the source package type and must not already exist.
    /// TWBX output replaces only the embedded TWB; all unrelated compressed entry
    /// bytes are verified unchanged before the temporary file is persisted.
    /// The source hash is checked both before and after repacking, so a concurrent
    /// source modification fails with `STALE_BASE` instead of producing a mixed
    /// candidate.
    pub fn write_candidate(&self, output: &Path, twb: &[u8], limits: &Limits) -> Result<Self> {
        require(
            twb.len() as u64 <= limits.xml_bytes,
            "LIMIT",
            "Candidate TWB exceeds configured limit",
        )?;
        let expected_extension = match self.kind {
            Kind::Twb => "twb",
            Kind::Twbx => "twbx",
        };
        require(
            output
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case(expected_extension)),
            "FORMAT",
            "Candidate must retain the source package type",
        )?;
        require(
            !output.exists(),
            "OUTPUT_EXISTS",
            "Refusing to overwrite output",
        )?;
        require(
            hash_file(&self.path, limits.file_bytes)? == self.sha256,
            "STALE_BASE",
            "Input changed before candidate write",
        )?;

        let parent = output
            .parent()
            .ok_or_else(|| Error::new("PATH", "Output has no parent directory"))?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;

        match self.kind {
            Kind::Twb => temp.write_all(twb)?,
            Kind::Twbx => {
                let mut input = ZipArchive::new(File::open(&self.path)?)?;
                let mut writer = ZipWriter::new(temp.as_file_mut());
                for index in 0..input.len() {
                    let file = input.by_index(index)?;
                    if Some(index) == self.twb_entry {
                        let mut options =
                            SimpleFileOptions::default().compression_method(file.compression());
                        if let Some(time) = file.last_modified() {
                            options = options.last_modified_time(time);
                        }
                        if let Some(mode) = file.unix_mode() {
                            options = options.unix_permissions(mode);
                        }
                        let name = file.name()?.into_owned();
                        writer.start_file(name, options)?;
                        writer.write_all(twb)?;
                    } else {
                        writer.raw_copy_file(file)?;
                    }
                }
                writer.finish()?;
            }
        }

        temp.as_file_mut().flush()?;
        temp.as_file().sync_all()?;
        require(
            temp.as_file().metadata()?.len() <= limits.file_bytes,
            "LIMIT",
            "Candidate package exceeds configured limit",
        )?;

        if self.kind == Kind::Twbx {
            self.verify_package(temp.path(), twb)?;
        }
        require(
            hash_file(&self.path, limits.file_bytes)? == self.sha256,
            "STALE_BASE",
            "Input changed during candidate write",
        )?;

        let hash = hash_file(temp.path(), limits.file_bytes)?;
        temp.persist_noclobber(output)
            .map_err(|error| Error::new("OUTPUT_EXISTS", error.error.to_string()))?;
        Ok(Self {
            path: output.to_owned(),
            kind: self.kind,
            sha256: hash,
            entries: self.entries.clone(),
            twb_entry: self.twb_entry,
        })
    }

    fn verify_package(&self, path: &Path, expected_twb: &[u8]) -> Result<()> {
        let mut output = ZipArchive::new(File::open(path)?)?;
        let mut raw = File::open(path)?;
        require(
            output.len() == self.entries.len(),
            "PRESERVATION",
            "Package entry count changed",
        )?;

        for (index, old) in self.entries.iter().enumerate() {
            let mut file = output.by_index(index)?;
            let name = file.name()?.into_owned();
            require(
                name == old.name,
                "PRESERVATION",
                "Package entry order or name changed",
            )?;
            if Some(index) == self.twb_entry {
                let mut actual = Vec::new();
                file.by_ref()
                    .take(expected_twb.len() as u64 + 1)
                    .read_to_end(&mut actual)?;
                require(
                    actual == expected_twb,
                    "PRESERVATION",
                    "Repacked TWB differs from candidate",
                )?;
            } else {
                require(
                    file.size() == old.size && file.compressed_size() == old.compressed_size,
                    "PRESERVATION",
                    "Unrelated archive entry size changed",
                )?;
                require(
                    hash_region(
                        &mut raw,
                        file.data_start().ok_or_else(|| {
                            Error::new("ZIP", "ZIP entry data offset is unavailable")
                        })?,
                        file.compressed_size(),
                    )? == old.compressed_sha256,
                    "PRESERVATION",
                    "Unrelated compressed archive bytes changed",
                )?;
            }
        }
        Ok(())
    }
}

/// Hashes a file while enforcing the same byte bound used by callers for admission.
///
/// The function fails once the observed stream exceeds `max_bytes`; it never
/// hashes an unbounded file and then checks the size afterward.
pub fn hash_file(path: &Path, max_bytes: u64) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];

    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or_else(|| Error::new("LIMIT", "File size overflow"))?;
        require(total <= max_bytes, "LIMIT", "File exceeds configured limit")?;
        hash.update(&buffer[..count]);
    }

    Ok(format!("{:x}", hash.finalize()))
}

fn read_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    File::open(path)?
        .take(max_bytes + 1)
        .read_to_end(&mut data)?;
    require(
        data.len() as u64 <= max_bytes,
        "LIMIT",
        "File exceeds configured limit",
    )?;
    Ok(data)
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn safe_entry(name: &str) -> Result<()> {
    require(
        !name.is_empty()
            && !name.starts_with('/')
            && !name.contains(['\\', ':', '\0'])
            && name
                .trim_end_matches('/')
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".."),
        "ZIP_PATH",
        "Unsafe ZIP entry path",
    )
}

fn hash_region(file: &mut File, start: u64, size: u64) -> Result<String> {
    file.seek(SeekFrom::Start(start))?;
    let mut region = file.take(size);
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];

    loop {
        let count = region.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        hash.update(&buffer[..count]);
    }
    require(total == size, "ZIP", "Truncated compressed ZIP entry")?;
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twbx_candidate_preserves_unrelated_entries() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let input = dir.path().join("input.twbx");
        {
            let file = File::create(&input)?;
            let mut zip = ZipWriter::new(file);
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("book.twb", options)?;
            zip.write_all(
                br#"<workbook version="26.2" source-build="test"><worksheets/></workbook>"#,
            )?;
            zip.start_file("Data/resource.txt", options)?;
            zip.write_all(b"resource-bytes-must-not-change")?;
            zip.finish()?;
        }

        let limits = Limits::default();
        let (package, twb) = Package::open(&input, &limits)?;
        let candidate = String::from_utf8(twb)
            .map_err(|_| Error::new("TEST", "Fixture TWB is not UTF-8"))?
            .replace(
                "<worksheets/>",
                "<worksheets><worksheet name=\"A\"><simple-id uuid=\"{00000000-0000-0000-0000-000000000001}\"/></worksheet></worksheets>",
            );

        let output = dir.path().join("output.twbx");
        let candidate_package = package.write_candidate(&output, candidate.as_bytes(), &limits)?;

        let second_candidate = candidate.replace("name=\"A\"", "name=\"B\"");
        let second_output = dir.path().join("output-2.twbx");
        candidate_package.write_candidate(&second_output, second_candidate.as_bytes(), &limits)?;

        let mut zip = ZipArchive::new(File::open(second_output)?)?;
        let mut resource = String::new();
        zip.by_name("Data/resource.txt")?
            .read_to_string(&mut resource)?;
        assert_eq!(resource, "resource-bytes-must-not-change");

        let mut twb = String::new();
        zip.by_name("book.twb")?.read_to_string(&mut twb)?;
        assert!(twb.contains("name=\"B\""));
        Ok(())
    }
}
