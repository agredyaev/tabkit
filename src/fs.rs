use crate::error::{
    Error,
    Result,
    require
};
use sha2::{
    Digest,
    Sha256
};
use std::{
    fs::{
        File,
        OpenOptions
    },
    io::{
        Read,
        Write
    },
    path::{
        Component,
        Path,
        PathBuf
    }
};
/// Lowercase encoding of exactly one SHA-256 digest, preserving leading zeroes.
pub fn digest_hex(digest: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 15)] as char);
    }
    out
}
pub fn sha256(bytes: &[u8]) -> String {
    digest_hex(Sha256::digest(bytes).into())
}
pub fn hash_file(path: &Path, limit: u64) -> Result<String> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut total = 0u64;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total = total.checked_add(n as u64).ok_or_else(|| Error::new("LIMIT", "File size overflow"))?;
        require(total <= limit, "LIMIT", "File exceeds configured limit")?;
        h.update(&buf[..n]);
    }
    Ok(digest_hex(h.finalize().into()))
}
pub fn read_bounded(path: &Path, max: u64) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    File::open(path)?.take(max.saturating_add(1)).read_to_end(&mut data)?;
    require(data.len() as u64 <= max, "LIMIT", "File exceeds configured size")?;
    Ok(data)
}
#[derive(Clone)]
pub struct Workspace {
    root: PathBuf,
    state: PathBuf
}
impl Workspace {
    pub fn new(root: PathBuf) -> Result<Self> {
        let state = root.join(".tabkit");
        if state.exists() {
            require(!std::fs::symlink_metadata(&state)?.file_type().is_symlink(), "PATH", "State directory cannot be a symlink")?;
        }
        std::fs::create_dir_all(&state)?;
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            root,
            state
        })
    }
    pub fn state(&self) -> &Path {
        &self.state
    }
    /// Agent paths are relative, non-traversing, and may not point into internal state.
    fn lexical(&self, relative: &str) -> Result<PathBuf> {
        require(!relative.is_empty() && relative.len() <= 4096 && !relative.contains('\\') && !relative.contains('\0'), "PATH", "Use a nonempty workspace-relative path with '/' separators")?;
        let p = Path::new(relative);
        for part in p.components() {
            match part {
                Component::Normal(s) => {
                    let s = s.to_string_lossy();
                    let stem=s.split('.').next().unwrap_or("").to_ascii_uppercase();
                    let device=matches!(stem.as_str(),"CON"|"PRN"|"AUX"|"NUL"|"CLOCK$") ||
                    (stem.len()==4 && (stem.starts_with("COM")||stem.starts_with("LPT")) && matches!(stem.as_bytes()[3],b'1'..=b'9'));
                    require(!device && !s.chars().any(|c|c<' '), "PATH", "Reserved device/control path component")?;
                    require(!s.eq_ignore_ascii_case(".tabkit") && !s.contains(':') && !s.ends_with('.') && !s.ends_with(' '), "PATH", "Reserved or ambiguous path component")?;
                }
                _ => return Err(Error::new("PATH", "Absolute paths and traversal are forbidden")),
            }
        }
        Ok(self.root.join(p))
    }
    fn check_parents(&self, path: &Path) -> Result<()> {
        let rel = path.strip_prefix(&self.root).map_err(|_| Error::new("PATH", "Outside workspace"))?;
        let mut current = self.root.clone();
        for c in rel.components() {
            current.push(c);
            match std::fs::symlink_metadata(&current) {
                Ok(m) => require(!m.file_type().is_symlink(), "PATH", "Symlinks are not allowed in tool paths")?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                },
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
    pub fn input(&self, relative: &str) -> Result<PathBuf> {
        let path = self.lexical(relative)?;
        self.check_parents(&path)?;
        let canonical = path.canonicalize()?;
        require(canonical.starts_with(&self.root) && canonical.is_file(), "PATH", "Input must be a regular workspace file")?;
        Ok(canonical)
    }
    pub fn output(&self, relative: &str) -> Result<PathBuf> {
        let path = self.lexical(relative)?;
        self.check_parents(&path)?;
        require(!path.exists(), "OUTPUT_EXISTS", "Output exists; choose a new candidate path")?;
        let parent = path.parent().ok_or_else(|| Error::new("PATH", "No parent"))?.canonicalize()?;
        require(parent.starts_with(&self.root) && parent.is_dir(), "PATH", "Output parent must already exist inside workspace")?;
        Ok(path)
    }
    pub fn write_new(&self, relative: &str, bytes: &[u8]) -> Result<PathBuf> {
        let out = self.output(relative)?;
        atomic_new(&out, bytes)?;
        Ok(out)
    }
    pub fn internal_write(&self, name: &str, bytes: &[u8]) -> Result<PathBuf> {
        require(name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'), "INTERNAL", "Invalid state key")?;
        let path = self.state.join(name);
        atomic_new(&path, bytes)?;
        Ok(path)
    }
    pub fn internal_read(&self, name: &str, max: u64) -> Result<Vec<u8>> {
        require(name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'), "INTERNAL", "Invalid state key")?;
        let p = self.state.join(name);
        require(!std::fs::symlink_metadata(&p)?.file_type().is_symlink(), "PATH", "Invalid state file")?;
        read_bounded(&p, max)
    }
}
pub fn atomic_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| Error::new("PATH", "No output parent"))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist_noclobber(path).map_err(|e| Error::new("ATOMIC_WRITE", e.error.to_string()))?;
    sync_dir(parent).map_err(|_|Error::new("COMMIT_DURABILITY_UNKNOWN","File exists but directory durability could not be confirmed; do not retry blindly")
        .details(serde_json::json!({"output_may_exist":true,"sha256":sha256(bytes)})))?;
    Ok(())
}
pub fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)] {
        File::open(path)?.sync_all()?;
    }
    #[cfg(not(unix))] {
        let _ = path;
    }
    Ok(())
}
/// A create-new journal marker is a local replay guard, not distributed exactly-once.
pub fn claim(path: &Path) -> Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
    .map_err(|_| Error::new("ALREADY_ATTEMPTED", "Operation was already attempted; inspect its receipt before retrying"))
}
