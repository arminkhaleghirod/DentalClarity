//! Run metadata helpers: UTC timestamps without a date crate, git state,
//! tool versions and content hashes of the inputs the scorers depend on.

use sha2::{Digest, Sha256};
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

fn parts(t: SystemTime) -> (i64, u32, u32, u64, u64, u64) {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (y, m, d) = civil((secs / 86_400) as i64);
    let rem = secs % 86_400;
    (y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// `2026-11-25T14:03:09Z`
pub fn rfc3339(t: SystemTime) -> String {
    let (y, m, d, h, mi, s) = parts(t);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// `20261125T140309Z`, safe in a directory name.
pub fn compact(t: SystemTime) -> String {
    let (y, m, d, h, mi, s) = parts(t);
    format!("{y:04}{m:02}{d:02}T{h:02}{mi:02}{s:02}Z")
}

fn output(cmd: &str, args: &[&str], dir: &Path) -> Option<String> {
    let out = Command::new(cmd)
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The current commit and whether the tree has uncommitted changes: any
/// tracked file, plus untracked (not ignored) files under `knowledge/` and
/// `eval/`, since a new knowledge entry or case changes results. Read-only.
pub fn git_state(root: &Path) -> (Option<String>, Option<bool>) {
    let sha = output("git", &["rev-parse", "HEAD"], root);
    let tracked = output(
        "git",
        &["status", "--porcelain", "--untracked-files=no"],
        root,
    );
    let untracked = output(
        "git",
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--",
            "knowledge",
            "eval",
        ],
        root,
    );
    let dirty = match (tracked, untracked) {
        (Some(t), Some(u)) => Some(!t.is_empty() || !u.is_empty()),
        _ => None,
    };
    (sha, dirty)
}

/// Whether a file is tracked by git and unmodified against HEAD (staged or
/// not). `None` when git cannot answer.
pub fn git_file_state(root: &Path, rel: &str) -> (Option<bool>, Option<bool>) {
    let tracked = Command::new("git")
        .args(["ls-files", "--error-unmatch", "--", rel])
        .current_dir(root)
        .output()
        .ok()
        .map(|o| o.status.success());
    let clean = output("git", &["status", "--porcelain", "--", rel], root).map(|s| s.is_empty());
    (tracked, clean)
}

/// sha256 over `<relative path> <file sha256>` lines of every file under
/// `dir`, sorted by path with `/` separators. `None` if the directory cannot
/// be read.
pub fn tree_sha256(dir: &Path) -> Option<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                walk(base, &path, out)?;
            } else {
                let rel = path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                let bytes = std::fs::read(&path)?;
                out.push(format!("{rel} {}\n", hex::encode(Sha256::digest(&bytes))));
            }
        }
        Ok(())
    }
    let mut lines = Vec::new();
    walk(dir, dir, &mut lines).ok()?;
    lines.sort();
    Some(hex::encode(Sha256::digest(lines.concat().as_bytes())))
}

/// The lexicons and text checks the visible scorer reuses, as compiled into
/// this binary.
const LEXICON_SOURCE: &str = include_str!("../../dc-generation/src/validate.rs");

/// sha256 of `dc-generation/src/validate.rs` as compiled into this binary.
pub fn lexicon_sha256() -> String {
    hex::encode(Sha256::digest(LEXICON_SOURCE.as_bytes()))
}

pub fn rustc_version(root: &Path) -> Option<String> {
    output("rustc", &["--version"], root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn formats_utc_times() {
        let t = UNIX_EPOCH + Duration::from_secs(1_795_615_389);
        assert_eq!(rfc3339(t), "2026-11-25T14:03:09Z");
        assert_eq!(compact(t), "20261125T140309Z");
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(rfc3339(leap), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn tree_hash_tracks_content_and_names() {
        let dir = std::env::temp_dir().join(format!("ce-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::write(dir.join("a/x.toml"), "one").unwrap();
        let first = tree_sha256(&dir).unwrap();
        assert_eq!(tree_sha256(&dir).unwrap(), first);
        std::fs::write(dir.join("a/x.toml"), "two").unwrap();
        let second = tree_sha256(&dir).unwrap();
        assert_ne!(second, first);
        std::fs::write(dir.join("y.toml"), "").unwrap();
        assert_ne!(tree_sha256(&dir).unwrap(), second);
        assert!(tree_sha256(&dir.join("missing")).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(lexicon_sha256().len(), 64);
    }
}
