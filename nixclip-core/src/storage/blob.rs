use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::Result;

/// Content-addressed blob storage on disk.
///
/// Blobs are stored in a two-level directory hierarchy derived from the hex
/// encoding of their 32-byte hash: `{hex[0..4]}/{full_hex}`.  Writes are
/// atomic: data is first written to a temporary file under `.tmp/`, fsynced,
/// then renamed to its final location.
pub struct BlobStore {
    pub(crate) base_dir: PathBuf,
}

impl BlobStore {
    /// Open or create a blob store rooted at `base_dir`.
    pub fn new(base_dir: &Path) -> Result<Self> {
        fs::create_dir_all(base_dir)?;
        fs::create_dir_all(base_dir.join(".tmp"))?;
        Ok(Self {
            base_dir: base_dir.to_path_buf(),
        })
    }

    /// Store `data` under the given 32-byte `hash`.
    ///
    /// Returns the relative path (e.g. `"a1b2/a1b2c3d4..."`).  If a blob with
    /// the same hash already exists, the write is skipped and the path is
    /// returned immediately.
    pub fn store(&self, hash: &[u8; 32], data: &[u8]) -> Result<String> {
        let hex = hex_encode(hash);
        let prefix = &hex[..4];
        let rel_path = format!("{prefix}/{hex}");
        let full_path = self.base_dir.join(&rel_path);

        // Already stored -- skip.
        if full_path.exists() {
            return Ok(rel_path);
        }

        // Ensure the prefix directory exists.
        fs::create_dir_all(self.base_dir.join(prefix))?;

        // Write to a temporary file, fsync, then atomically rename.
        let tmp_name = format!("{}.tmp", uuid_v4_hex());
        let tmp_path = self.base_dir.join(".tmp").join(&tmp_name);

        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);

        fs::rename(&tmp_path, &full_path)?;

        Ok(rel_path)
    }

    /// Load the blob at the given relative path.
    pub fn load(&self, rel_path: &str) -> Result<Vec<u8>> {
        let full_path = self.base_dir.join(rel_path);
        Ok(fs::read(full_path)?)
    }

    /// Delete the blob at the given relative path.
    pub fn delete(&self, rel_path: &str) -> Result<()> {
        let full_path = self.base_dir.join(rel_path);
        if full_path.exists() {
            fs::remove_file(full_path)?;
        }
        Ok(())
    }

    /// Check whether a blob exists at the given relative path.
    pub fn exists(&self, rel_path: &str) -> bool {
        self.base_dir.join(rel_path).exists()
    }

    /// Delete every blob file whose relative path is **not** in `valid_paths`.
    ///
    /// Returns the total number of bytes freed.
    pub fn cleanup_orphans(&self, valid_paths: &HashSet<String>) -> Result<u64> {
        let mut freed: u64 = 0;

        let entries = match fs::read_dir(&self.base_dir) {
            Ok(e) => e,
            Err(_) => return Ok(0),
        };

        for dir_entry in entries {
            let dir_entry = dir_entry?;
            let dir_name = dir_entry.file_name();
            let dir_name_str = dir_name.to_string_lossy();

            // Skip the .tmp directory and non-directories.
            if dir_name_str == ".tmp" || !dir_entry.file_type()?.is_dir() {
                continue;
            }

            let sub_entries = fs::read_dir(dir_entry.path())?;
            for file_entry in sub_entries {
                let file_entry = file_entry?;
                if !file_entry.file_type()?.is_file() {
                    continue;
                }

                let file_name = file_entry.file_name();
                let rel = format!("{}/{}", dir_name_str, file_name.to_string_lossy());

                if !valid_paths.contains(&rel) {
                    let size = file_entry.metadata().map(|m| m.len()).unwrap_or(0);
                    fs::remove_file(file_entry.path())?;
                    freed += size;
                }
            }
        }

        Ok(freed)
    }

    /// Walk the entire blob directory and sum file sizes.
    pub fn total_size(&self) -> Result<u64> {
        let mut total: u64 = 0;

        let entries = match fs::read_dir(&self.base_dir) {
            Ok(e) => e,
            Err(_) => return Ok(0),
        };

        for dir_entry in entries {
            let dir_entry = dir_entry?;
            let dir_name = dir_entry.file_name();

            if dir_name.to_string_lossy() == ".tmp" || !dir_entry.file_type()?.is_dir() {
                continue;
            }

            let sub_entries = fs::read_dir(dir_entry.path())?;
            for file_entry in sub_entries {
                let file_entry = file_entry?;
                if file_entry.file_type()?.is_file() {
                    total += file_entry.metadata().map(|m| m.len()).unwrap_or(0);
                }
            }
        }

        Ok(total)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Encode a byte slice as lowercase hexadecimal.
fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Generate a random hex string suitable for temporary file names.
///
/// Uses a simple scheme based on hashing the current thread id and timestamp
/// to avoid pulling in a full UUID crate.
fn uuid_v4_hex() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::SystemTime;

    let mut hasher = DefaultHasher::new();
    SystemTime::now().hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    // Mix in the address of a stack variable for extra entropy across calls.
    let stack_var: u8 = 0;
    let addr = &stack_var as *const u8 as usize;
    addr.hash(&mut hasher);
    let h = hasher.finish();
    format!("{h:016x}")
}
