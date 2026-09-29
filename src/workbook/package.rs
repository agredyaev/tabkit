use crate::{
    config::Limits,
    error::{
        Error,
        Result,
        require
    },
    fs::{
        hash_file,
        sync_dir
    },
    xml::Xml
};
use serde::Serialize;
use sha2::{
    Digest,
    Sha256
};
use std::{
    collections::BTreeSet,
    fs::File,
    io::{
        Read,
        Seek,
        SeekFrom,
        Write
    },
    path::{
        Path,
        PathBuf
    }
};
use zip::{
    ZipArchive,
    ZipWriter,
    write::SimpleFileOptions
};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all="lowercase")]
pub enum Kind {
    Twb,
    Twbx
}
#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub name: String,
    pub size: u64,
    pub compressed_size: u64,
    pub compressed_sha256: String,
}
pub struct Package {
    pub path: PathBuf,
    pub kind: Kind,
    pub sha256: String,
    pub twb_sha256: String,
    pub entries: Vec<Entry>,
    pub twb_entry: Option<usize>,
}
impl Package {
    pub fn open(path: &Path, limits: &Limits) -> Result<(Self, Xml)> {
        let size = std::fs::metadata(path)?.len();
        require(size <= limits.file_bytes, "LIMIT", "Workbook package is too large")?;
        let extension=path.extension().and_then(|e|e.to_str()).map(|s|s.to_ascii_lowercase());
        match extension.as_deref() {
            Some("twb") => {
                let twb = crate::fs::read_bounded(path, limits.xml_bytes)?;
                let parsed=Xml::parse(twb, limits)?;
                require(parsed.tag(crate::xml::NodeId(0))=="workbook","FORMAT","Package XML root is not a workbook")?;
                // For a plain TWB the package bytes are exactly the admitted XML bytes.
                // Reuse that stable digest; the independent post-read file hash remains the freshness check.
                let sha=parsed.sha256.clone();
                require(hash_file(path, limits.file_bytes)? == sha, "STALE_BASE", "Input changed while reading")?;
                let twb_sha256 = sha.clone();
                Ok((Self {
                    path: path.into(),
                    kind: Kind::Twb,
                    sha256: sha,
                    twb_sha256,
                    entries: Vec::new(),
                    twb_entry: None
                }, parsed))
            }
            Some("twbx") => {
                let sha=hash_file(path,limits.file_bytes)?;
                let mut archive = ZipArchive::new(File::open(path)?)?;
                require(archive.len() <= limits.zip_entries, "ZIP_LIMIT", "Too many archive entries")?;
                let mut names = BTreeSet::new();
                let mut total = 0u64;
                let mut entries = Vec::new();
                let mut candidate = None;
                let mut raw = File::open(path)?;
                for i in 0..archive.len() {
                    let f = archive.by_index(i)?;
                    let name = f.name().to_string();
                    safe_entry(&name)?;
                    require(names.insert(name.to_ascii_lowercase()), "ZIP_DUPLICATE", "Duplicate or case-colliding ZIP entry")?;
                    require(f.unix_mode().map(|m| m & 0o170000 != 0o120000).unwrap_or(true), "ZIP_SYMLINK", "Symlink ZIP entries are forbidden")?;
                    total = total.checked_add(f.size()).ok_or_else(|| Error::new("ZIP_LIMIT", "Expanded size overflow"))?;
                    require(total <= limits.zip_expanded_bytes, "ZIP_LIMIT", "Expanded archive exceeds limit")?;
                    require(f.size() <= f.compressed_size().saturating_mul(5000).saturating_add(1<<20), "ZIP_LIMIT", "Suspicious compression ratio")?;
                    let compressed_sha256 = hash_region(&mut raw, f.data_start(), f.compressed_size())?;
                    entries.push(Entry {
                        name: name.clone(),
                        size: f.size(),
                        compressed_size: f.compressed_size(),
                        compressed_sha256
                    });
                    if name.to_ascii_lowercase().ends_with(".twb") {
                        require(candidate.is_none(), "AMBIGUOUS_PACKAGE", "Package contains multiple TWBs; select/repackage it explicitly outside this tool")?;
                        candidate = Some(i);
                    }
                }
                let twb_entry = candidate.ok_or_else(|| Error::new("FORMAT", "TWBX contains no TWB"))?;
                let mut twb = Vec::new();
                archive.by_index(twb_entry)?.take(limits.xml_bytes + 1).read_to_end(&mut twb)?;
                require(twb.len() as u64 <= limits.xml_bytes, "LIMIT", "Embedded TWB exceeds limit")?;
                let parsed=Xml::parse(twb, limits)?;
                require(parsed.tag(crate::xml::NodeId(0))=="workbook","FORMAT","Package XML root is not a workbook")?;
                require(hash_file(path, limits.file_bytes)? == sha, "STALE_BASE", "Input changed while indexing")?;
                let twb_sha256 = parsed.sha256.clone();
                Ok((Self {
                    path: path.into(),
                    kind: Kind::Twbx,
                    sha256: sha,
                    twb_sha256,
                    entries,
                    twb_entry: Some(twb_entry)
                }, parsed))
            }
            _ => Err(Error::new("FORMAT", "Only .twb and .twbx are accepted")),
        }
    }
    #[cfg(any(test, feature = "dev-tools"))]
    pub fn write_candidate(&self, output: &Path, candidate: &Xml, limits: &Limits) -> Result<String> {
        require(candidate.tag(crate::xml::NodeId(0)) == "workbook", "FORMAT", "Candidate must be an admitted workbook")?;
        self.write_candidate_bytes(output,candidate.text.as_bytes(),&candidate.sha256,limits)
    }
    pub fn write_planned_candidate(&self, output:&Path, candidate:&crate::edit::Candidate,
        limits:&Limits)->Result<String>{
        self.write_candidate_bytes(output,candidate.text().as_bytes(),candidate.sha256(),limits)
    }
    fn write_candidate_bytes(&self, output:&Path, twb:&[u8], twb_sha256:&str,
        limits:&Limits)->Result<String>{
        require(twb.len() as u64 <= limits.xml_bytes, "LIMIT", "Candidate XML exceeds limit")?;
        let expected_ext = if self.kind == Kind::Twb {
            "twb"
        } else {
            "twbx"
        };
        require(output.extension().and_then(|e| e.to_str()).map(|s| s.eq_ignore_ascii_case(expected_ext)).unwrap_or(false), "FORMAT", "Candidate must retain the original TWB/TWBX format")?;
        require(hash_file(&self.path, limits.file_bytes)? == self.sha256, "STALE_BASE", "Input changed before apply")?;
        let parent = output.parent().ok_or_else(|| Error::new("PATH", "No output parent"))?;
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        if twb_sha256 == self.twb_sha256 {
            std::io::copy(&mut File::open(&self.path)?, &mut tmp)?;
        } else if self.kind == Kind::Twb {
            tmp.write_all(twb)?;
        }
        else {
            let mut input = ZipArchive::new(File::open(&self.path)?)?;
            let mut writer = ZipWriter::new(tmp.as_file_mut());
            for i in 0..input.len() {
                let f = input.by_index(i)?;
                if Some(i) == self.twb_entry {
                    let mut opts = SimpleFileOptions::default().compression_method(f.compression());
                    if let Some(t) = f.last_modified() {
                        opts = opts.last_modified_time(t);
                    }
                    if let Some(mode) = f.unix_mode() {
                        opts = opts.unix_permissions(mode);
                    }
                    writer.start_file(f.name(), opts)?;
                    writer.write_all(twb)?;
                } else {
                    writer.raw_copy_file(f)?;
                }
            }
            writer.finish()?;
        }
        tmp.as_file_mut().flush()?;
        tmp.as_file().sync_all()?;
        require(tmp.as_file().metadata()?.len() <= limits.file_bytes, "LIMIT", "Candidate exceeds limit")?;
        if self.kind == Kind::Twbx {
            self.verify_package(tmp.path(),twb)?;
        }
        require(hash_file(&self.path, limits.file_bytes)? == self.sha256, "STALE_BASE", "Input changed during apply")?;
        let hash = hash_file(tmp.path(), limits.file_bytes)?;
        tmp.persist_noclobber(output).map_err(|e| Error::new("OUTPUT_EXISTS", e.error.to_string()))?;
        sync_dir(parent).map_err(|e| Error::new("COMMIT_DURABILITY_UNKNOWN", e.message)
            .details(serde_json::json!({"output_may_exist": true, "sha256": hash})))?;
        Ok(hash)
    }
    fn verify_package(&self, path: &Path, expected_twb:&[u8]) -> Result<()> {
        let mut output = ZipArchive::new(File::open(path)?)?;
        let mut raw = File::open(path)?;
        require(output.len() == self.entries.len(), "PRESERVATION", "Package entry count changed")?;
        for (i, old) in self.entries.iter().enumerate() {
            let mut f = output.by_index(i)?;
            require(f.name() == old.name, "PRESERVATION", "Package entry order/name changed")?;
            if Some(i) != self.twb_entry {
                require(f.size() == old.size && f.compressed_size() == old.compressed_size, "PRESERVATION", "Unrelated archive entry size changed")?;
                require(hash_region(&mut raw, f.data_start(), f.compressed_size())? == old.compressed_sha256, "PRESERVATION", "Unrelated compressed entry bytes changed")?;
            } else {
                let mut actual=Vec::new();
                f.by_ref().take(expected_twb.len() as u64+1).read_to_end(&mut actual)?;
                require(actual==expected_twb,"PRESERVATION","Repacked TWB differs from the validated candidate")?;
            }
        }
        Ok(())
    }
    pub fn extract_hyper(&self, entry: &str, output: &Path, limits: &Limits) -> Result<String> {
        require(self.kind == Kind::Twbx && entry.to_ascii_lowercase().ends_with(".hyper"), "FORMAT", "Choose a .hyper entry from a TWBX")?;
        require(output.extension().and_then(|e| e.to_str()) == Some("hyper"), "FORMAT", "Hyper output must use .hyper")?;
        safe_entry(entry)?;
        let mut archive = ZipArchive::new(File::open(&self.path)?)?;
        let mut f = archive.by_name(entry)?;
        require(f.size() <= limits.file_bytes, "LIMIT", "Extract exceeds file limit")?;
        let parent = output.parent().ok_or_else(|| Error::new("PATH", "No output parent"))?;
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        let size = std::io::copy(&mut f.by_ref().take(limits.file_bytes+1), &mut tmp)?;
        require(size <= limits.file_bytes, "LIMIT", "Extract exceeds limit")?;
        tmp.as_file().sync_all()?;
        require(hash_file(&self.path, limits.file_bytes)? == self.sha256, "STALE_BASE", "Package changed")?;
        let hash = hash_file(tmp.path(), limits.file_bytes)?;
        tmp.persist_noclobber(output).map_err(|e| Error::new("OUTPUT_EXISTS", e.error.to_string()))?;
        sync_dir(parent).map_err(|e| Error::new("COMMIT_DURABILITY_UNKNOWN", e.message)
            .details(serde_json::json!({"output_may_exist": true, "sha256": hash})))?;
        Ok(hash)
    }
}
fn safe_entry(name: &str) -> Result<()> {
    require(!name.is_empty() && !name.starts_with('/') && !name.contains(['\\',':','\0']), "ZIP_PATH", "Unsafe ZIP entry path")?;
    require(name.trim_end_matches('/').split('/').all(|p| !p.is_empty() && p != "." && p != ".."), "ZIP_PATH", "ZIP path traversal or empty component")
}
fn hash_region(file: &mut File, start: u64, size: u64) -> Result<String> {
    file.seek(SeekFrom::Start(start))?;
    let mut r = file.take(size);
    let mut h = Sha256::new();
    let mut n = 0u64;
    let mut b = [0u8; 65536];
    loop {
        let k = r.read(&mut b)?;
        if k == 0 {
            break;
        }
        h.update(&b[..k]);
        n += k as u64;
    }
    require(n == size, "ZIP", "Truncated compressed entry")?;
    Ok(crate::fs::digest_hex(h.finalize().into()))
}
