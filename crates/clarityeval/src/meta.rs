//! Run metadata helpers: UTC timestamps without a date crate, git state, tool versions.

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

/// The current commit and whether tracked files have uncommitted changes. Read-only.
pub fn git_state(root: &Path) -> (Option<String>, Option<bool>) {
    let sha = output("git", &["rev-parse", "HEAD"], root);
    let dirty = output(
        "git",
        &["status", "--porcelain", "--untracked-files=no"],
        root,
    )
    .map(|s| !s.is_empty());
    (sha, dirty)
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
}
