//! Secrets at rest (docs/DECISIONS.md E25, docs/SECURITY.md).
//!
//! Each secret is one file, `<data_dir>/secrets/<name>.secret`. On Windows the
//! content is protected with DPAPI in machine scope, so the service account
//! can read what an administrator stored with `dentalclarity secret set`;
//! the installer limits the data directory to the service account and
//! Administrators, since any process on the machine could otherwise unwrap
//! it. On other systems DPAPI does not exist and the content is stored as is,
//! in a file only the owner can read; that is for development, and clinical
//! mode logs a warning when it uses such a store.
//!
//! Secret values never go to logs, the API or `Debug` output.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// File header: magic, then one byte naming the protection.
const MAGIC: &[u8; 4] = b"DCS1";
const PLAIN: u8 = 0;
const DPAPI: u8 = 1;

/// Secret names the server reads.
pub mod names {
    /// IAM access key id for Bedrock and Polly.
    pub const AWS_ACCESS_KEY_ID: &str = "aws-access-key-id";
    /// IAM secret access key for Bedrock and Polly.
    pub const AWS_SECRET_ACCESS_KEY: &str = "aws-secret-access-key";
    /// Private key of the generated local certificate authority, PEM.
    pub const TLS_CA_KEY: &str = "tls-ca-key";
}

#[derive(Debug, Clone)]
pub struct SecretStore {
    dir: PathBuf,
}

/// Whether this platform protects stored secrets with DPAPI.
pub const PROTECTED: bool = cfg!(windows);

impl SecretStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("secrets"),
        }
    }

    fn path(&self, name: &str) -> Result<PathBuf> {
        let valid = !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !valid {
            bail!("secret names use lowercase letters, digits and hyphens");
        }
        Ok(self.dir.join(format!("{name}.secret")))
    }

    /// The secret's value, or `None` when it is not stored.
    pub fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let path = self.path(name)?;
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading secret {name}")),
        };
        if bytes.len() < 5 || &bytes[..4] != MAGIC {
            bail!("secret {name} is not in the expected format");
        }
        let body = &bytes[5..];
        match bytes[4] {
            PLAIN => {
                if PROTECTED {
                    tracing::warn!(name, "a stored secret is not protected with DPAPI");
                }
                Ok(Some(body.to_vec()))
            }
            DPAPI => Ok(Some(
                dpapi::unprotect(body).with_context(|| format!("unwrapping secret {name}"))?,
            )),
            _ => bail!("secret {name} uses an unknown protection"),
        }
    }

    /// The secret as UTF-8 text with surrounding whitespace removed.
    pub fn get_string(&self, name: &str) -> Result<Option<String>> {
        self.get(name)?
            .map(|b| {
                String::from_utf8(b)
                    .map(|s| s.trim().to_owned())
                    .with_context(|| format!("secret {name} is not text"))
            })
            .transpose()
    }

    /// Stores `value` under `name`, replacing any earlier value. The file is
    /// written next to its final path and renamed, so a crash never leaves a
    /// half-written secret.
    pub fn set(&self, name: &str, value: &[u8]) -> Result<()> {
        let path = self.path(name)?;
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating {}", self.dir.display()))?;
        let mut out = MAGIC.to_vec();
        if PROTECTED {
            out.push(DPAPI);
            out.extend(dpapi::protect(value).context("protecting the secret with DPAPI")?);
        } else {
            out.push(PLAIN);
            out.extend_from_slice(value);
        }
        let tmp = path.with_extension("part");
        write_private(&tmp, &out)?;
        std::fs::rename(&tmp, &path).with_context(|| format!("saving secret {name}"))?;
        Ok(())
    }

    /// Removes the secret. Removing one that is not stored is not an error.
    pub fn delete(&self, name: &str) -> Result<bool> {
        match std::fs::remove_file(self.path(name)?) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).with_context(|| format!("removing secret {name}")),
        }
    }
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("writing {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    // On Windows the data directory's ACL, set by the installer, limits
    // access; the content itself is DPAPI-protected.
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

#[cfg(windows)]
mod dpapi {
    //! `CryptProtectData` and `CryptUnprotectData` in machine scope.

    use anyhow::{Result, bail};
    use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_LOCAL_MACHINE, CRYPTPROTECT_UI_FORBIDDEN,
        CryptProtectData, CryptUnprotectData,
    };

    /// Mixed into every protected blob, so a DPAPI blob made by another
    /// program for this machine does not unwrap here by accident.
    const ENTROPY: &[u8] = b"DentalClarity secret store v1";

    fn blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr().cast_mut(),
        }
    }

    fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        // SAFETY: DPAPI returned `cbData` bytes at `pbData`, allocated with
        // LocalAlloc; they are copied and then freed exactly once.
        unsafe {
            let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            LocalFree(out.pbData as HLOCAL);
            bytes
        }
    }

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>> {
        let input = blob(plain);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: the input blobs point at live slices for the whole call,
        // and `out` receives a buffer that `take` frees.
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_LOCAL_MACHINE | CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            bail!(
                "CryptProtectData failed: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(take(out))
    }

    pub fn unprotect(protected: &[u8]) -> Result<Vec<u8>> {
        let input = blob(protected);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: as in `protect`.
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            bail!(
                "CryptUnprotectData failed: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(take(out))
    }
}

#[cfg(not(windows))]
mod dpapi {
    use anyhow::{Result, bail};

    pub fn protect(_: &[u8]) -> Result<Vec<u8>> {
        bail!("DPAPI is only available on Windows")
    }

    pub fn unprotect(_: &[u8]) -> Result<Vec<u8>> {
        bail!("this secret was protected with DPAPI on Windows and cannot be read here")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (SecretStore, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("dc-secrets-{}", crate::credentials::new_token()));
        (SecretStore::new(&dir), dir)
    }

    #[test]
    fn round_trips_and_deletes() {
        let (store, dir) = store();
        assert_eq!(store.get("aws-access-key-id").unwrap(), None);
        store.set("aws-access-key-id", b" AKIAEXAMPLE \n").unwrap();
        assert_eq!(
            store.get_string("aws-access-key-id").unwrap().as_deref(),
            Some("AKIAEXAMPLE")
        );
        let raw = std::fs::read(dir.join("secrets/aws-access-key-id.secret")).unwrap();
        assert_eq!(&raw[..4], MAGIC);
        if PROTECTED {
            assert_eq!(raw[4], DPAPI);
            assert!(!raw.windows(11).any(|w| w == b"AKIAEXAMPLE"));
        }
        store.set("aws-access-key-id", b"second").unwrap();
        assert_eq!(store.get("aws-access-key-id").unwrap().unwrap(), b"second");
        assert!(store.delete("aws-access-key-id").unwrap());
        assert!(!store.delete("aws-access-key-id").unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_bad_names_and_files() {
        let (store, dir) = store();
        assert!(store.set("../x", b"v").is_err());
        assert!(store.set("Upper", b"v").is_err());
        std::fs::create_dir_all(dir.join("secrets")).unwrap();
        std::fs::write(dir.join("secrets/junk.secret"), b"nope").unwrap();
        assert!(store.get("junk").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
