//! On-disk storage of bundles.
//!
//! Identifiers coming from URLs are validated by [`validate_id`] before being
//! used to build any path: they can't contain `/`, can't start with `.` (so
//! they can neither escape the working directory nor collide with the
//! internal directories) and are limited to a conservative character set.
//!
//! Bodies are first streamed into `.tmp/` and atomically renamed to their
//! final location once completely received and synced to disk, so a reader
//! never sees a partial bundle and an aborted upload never leaves garbage.

use crate::error::{Error, Result};
use futures_util::StreamExt;
use std::{
    collections::HashMap,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
    time::SystemTime,
};
use tokio::io::AsyncWriteExt;

/// Directory (relative to the working dir) holding bundles being scanned.
pub const SCANS_DIR: &str = ".scans";
/// Directory (relative to the working dir) holding bodies being received.
pub const TMP_DIR: &str = ".tmp";
/// Directory (relative to the working dir) holding clamd's files.
pub const CLAMD_DIR: &str = ".clamd";

/// Maximum length of a user or bundle identifier.
const MAX_ID_LEN: usize = 128;

/// Size of the buffers used when writing received bodies and when
/// decompressing gzipped bundles.
const BUF_SIZE: usize = 1024 * 1024;

/// Check that an identifier taken from a URL is safe to use as a path
/// component.
///
/// Accepted: 1 to 128 characters among ASCII letters, digits, `-`, `_`,
/// `.`, `@` and `+`, not starting with a `.`.
pub fn validate_id(kind: &'static str, id: &str) -> Result<()> {
    let valid = !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'@' | b'+'));
    if valid {
        Ok(())
    } else {
        Err(Error::BadRequest(format!("invalid {kind} id")))
    }
}

/// Generate a new random bundle identifier.
///
/// With the `integration-tests` feature, the identifier is always
/// `bundle_test` so the tests can find the files deterministically.
pub fn new_bundle_id() -> String {
    if cfg!(feature = "integration-tests") {
        "bundle_test".to_string()
    } else {
        uuid::Uuid::new_v4().simple().to_string()
    }
}

/// How a stored bundle is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// Plain tar archive.
    Tar,
    /// Gzipped tar archive (`.tar.gz` or `.tgz`), served with
    /// `Content-Encoding: gzip`.
    Gzip,
}

/// A bundle found on disk.
#[derive(Debug)]
pub struct Bundle {
    pub path: PathBuf,
    pub encoding: Encoding,
    /// Size of the file on disk.
    pub len: u64,
    modified: Option<SystemTime>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct CacheKey {
    len: u64,
    modified: Option<SystemTime>,
}

/// Bundle storage rooted in the working directory.
pub struct Storage {
    root: PathBuf,
    max_body_size: Option<u64>,
    /// Uncompressed sizes of gzipped bundles, computing them requires
    /// decompressing the whole file. Invalidated when the file changes.
    size_cache: Mutex<HashMap<PathBuf, (CacheKey, u64)>>,
}

impl Storage {
    /// Prepare the working directory: create the internal directories and
    /// remove leftovers of a previous run.
    pub fn new(root: &Path, max_body_size: Option<u64>) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        let root = root.canonicalize()?;
        for dir in [SCANS_DIR, TMP_DIR] {
            let path = root.join(dir);
            if path.exists() {
                fs::remove_dir_all(&path)?;
            }
            fs::create_dir(&path)?;
        }
        fs::create_dir_all(root.join(CLAMD_DIR))?;
        Ok(Storage {
            root,
            max_body_size,
            size_cache: Mutex::new(HashMap::new()),
        })
    }

    /// Absolute path of the working directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory where bundles to analyze are stored and extracted.
    pub fn scans_dir(&self) -> PathBuf {
        self.root.join(SCANS_DIR)
    }

    /// Path of the tar file of a bundle submitted for analysis.
    pub fn scan_tar_path(&self, bundle_id: &str) -> PathBuf {
        self.scans_dir().join(format!("{bundle_id}.tar"))
    }

    /// Path where a bundle uploaded by a user is stored.
    ///
    /// Ids must have been checked with [`validate_id`].
    pub fn user_bundle_path(&self, user_id: &str, bundle_id: &str) -> PathBuf {
        self.root.join(user_id).join(format!("{bundle_id}.tar"))
    }

    /// Stream a request body to `dest`.
    ///
    /// The body is written to a temporary file, synced, then renamed to
    /// `dest` (whose parent directory is created if needed). On any error
    /// (client disconnecting, size limit exceeded, I/O error) the temporary
    /// file is removed and `dest` is left untouched.
    ///
    /// `content_length` is the value of the `Content-Length` header, if any:
    /// it allows rejecting too large bodies before receiving them.
    ///
    /// Returns the number of bytes written.
    pub async fn receive(
        &self,
        mut body: actix_web::web::Payload,
        content_length: Option<u64>,
        dest: &Path,
    ) -> Result<u64> {
        if let (Some(max), Some(len)) = (self.max_body_size, content_length)
            && len > max
        {
            return Err(Error::PayloadTooLarge(max));
        }

        let tmp_path = self
            .root
            .join(TMP_DIR)
            .join(format!("{}.part", uuid::Uuid::new_v4().simple()));
        let guard = RemoveOnDrop(Some(tmp_path.clone()));

        let file = tokio::fs::File::create(&tmp_path).await?;
        let mut writer = tokio::io::BufWriter::with_capacity(BUF_SIZE, file);
        let mut written: u64 = 0;
        while let Some(chunk) = body.next().await {
            let chunk = chunk?;
            written += chunk.len() as u64;
            if let Some(max) = self.max_body_size
                && written > max
            {
                return Err(Error::PayloadTooLarge(max));
            }
            writer.write_all(&chunk).await?;
        }
        if let Some(len) = content_length
            && written != len
        {
            return Err(Error::BadRequest(format!(
                "received {written} bytes, expected {len}"
            )));
        }
        writer.flush().await?;
        let file = writer.into_inner();
        file.sync_all().await?;
        drop(file);

        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::rename(&tmp_path, dest).await?;
        guard.disarm();
        Ok(written)
    }

    /// Look for a bundle of a user, trying the `.tar`, `.tar.gz` and `.tgz`
    /// extensions in that order.
    ///
    /// Ids must have been checked with [`validate_id`].
    pub async fn find_bundle(&self, user_id: &str, bundle_id: &str) -> Result<Bundle> {
        let base = self.root.join(user_id);
        for (ext, encoding) in [
            ("tar", Encoding::Tar),
            ("tar.gz", Encoding::Gzip),
            ("tgz", Encoding::Gzip),
        ] {
            let path = base.join(format!("{bundle_id}.{ext}"));
            match tokio::fs::metadata(&path).await {
                Ok(meta) if meta.is_file() => {
                    return Ok(Bundle {
                        path,
                        encoding,
                        len: meta.len(),
                        modified: meta.modified().ok(),
                    });
                }
                Ok(_) => continue,
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(Error::NotFound("bundle"))
    }

    /// Size of the bundle once decoded, i.e. the size of the tar archive.
    ///
    /// For gzipped bundles, the gzip trailer only stores the size modulo
    /// 2^32 (and only for the last member of a multi-member file), so the
    /// file is fully decompressed to count the bytes. This also verifies the
    /// integrity (CRC32 and size) of every member. The result is cached
    /// until the file changes.
    pub async fn uncompressed_size(&self, bundle: &Bundle) -> Result<u64> {
        if bundle.encoding == Encoding::Tar {
            return Ok(bundle.len);
        }
        let key = CacheKey {
            len: bundle.len,
            modified: bundle.modified,
        };
        if let Some((cached_key, size)) = self.cache().get(&bundle.path)
            && *cached_key == key
        {
            return Ok(*size);
        }

        let path = bundle.path.clone();
        let size = tokio::task::spawn_blocking(move || gzip_uncompressed_size(&path))
            .await
            .map_err(|err| Error::Internal(format!("size computation task failed: {err}")))?
            .map_err(|err| {
                Error::Internal(format!(
                    "couldn't decompress {}: {err}",
                    bundle.path.display()
                ))
            })?;

        log::debug!(
            "{}: {} bytes compressed, {size} bytes uncompressed",
            bundle.path.display(),
            bundle.len
        );
        let mut cache = self.cache();
        // Drop entries of files that disappeared to keep the cache bounded.
        cache.retain(|path, _| path.exists());
        cache.insert(bundle.path.clone(), (key, size));
        Ok(size)
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, (CacheKey, u64)>> {
        self.size_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Decompress a (possibly multi-member) gzip file and return the number of
/// decompressed bytes.
pub fn gzip_uncompressed_size(path: &Path) -> io::Result<u64> {
    let file = fs::File::open(path)?;
    let reader = io::BufReader::with_capacity(BUF_SIZE, file);
    let mut decoder = flate2::bufread::MultiGzDecoder::new(reader);
    let mut buf = vec![0u8; BUF_SIZE];
    let mut total: u64 = 0;
    loop {
        match decoder.read(&mut buf) {
            Ok(0) => return Ok(total),
            Ok(n) => total += n as u64,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
}

/// Removes a file when dropped, unless disarmed.
pub struct RemoveOnDrop(pub Option<PathBuf>);

impl RemoveOnDrop {
    pub fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if let Some(path) = self.0.take()
            && let Err(err) = fs::remove_file(&path)
            && err.kind() != io::ErrorKind::NotFound
        {
            log::warn!("couldn't remove {}: {err}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn ids() {
        for ok in ["Tartempion", "123456", "bundle_test", "a.b-c+d@REALM", "x"] {
            assert!(validate_id("test", ok).is_ok(), "{ok}");
        }
        let long = "a".repeat(MAX_ID_LEN + 1);
        for bad in [
            "",
            ".",
            "..",
            ".scans",
            "a/b",
            "../x",
            "a\\b",
            "a b",
            "é",
            "a\0",
            long.as_str(),
        ] {
            assert!(validate_id("test", bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn gzip_size_multi_member() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.gz");
        let mut out = fs::File::create(&path).unwrap();
        for chunk in [vec![1u8; 100_000], vec![2u8; 12_345]] {
            let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            enc.write_all(&chunk).unwrap();
            out.write_all(&enc.finish().unwrap()).unwrap();
        }
        drop(out);
        assert_eq!(gzip_uncompressed_size(&path).unwrap(), 112_345);
    }

    #[test]
    fn gzip_size_corrupted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.gz");
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&[3u8; 10_000]).unwrap();
        let mut data = enc.finish().unwrap();
        let len = data.len();
        data[len - 6] ^= 0xff; // corrupt the CRC
        fs::write(&path, data).unwrap();
        assert!(gzip_uncompressed_size(&path).is_err());
    }
}
