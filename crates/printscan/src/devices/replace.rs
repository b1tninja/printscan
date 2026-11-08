//! Replacing a file so that a reader sees the old contents or the new ones, never a mix, and so that the new
//! contents and the rename itself survive a crash or a power cut.
//!
//! The steps, and which platform each one is for:
//!
//! 1. The new contents are written to a staging file beside the target (same folder, so the rename cannot cross a
//!    file system) and flushed with [`std::fs::File::sync_all`]. On Apple targets std implements `sync_all` with
//!    `fcntl(F_FULLFSYNC)`, which asks the drive to empty its own cache too; a plain `fsync` there only hands the
//!    data to the drive. No extra call is made for it.
//! 2. The staging file is renamed over the target. On Windows that rename is refused while another process (an
//!    indexer, an antivirus scanner, a backup tool) has the target open without sharing delete access, which such
//!    tools usually do only for a moment, so a refusal with `ERROR_SHARING_VIOLATION` or `ERROR_ACCESS_DENIED` is
//!    retried a few times with a short, growing wait ([`RETRY_DELAYS_MS`], about 0.6 s in all). Any other error,
//!    or the same one after the last wait, is returned and the staging file is removed.
//! 3. On Unix the folder is opened and synced after the rename, because the rename is a change to the folder,
//!    not to the file, and is not durable until the folder is. Windows cannot open a folder as a `File`, and its
//!    file systems journal the rename with the file's metadata, so the step is skipped there.

use std::io;
use std::path::Path;
use std::time::Duration;

/// The waits, in milliseconds, before each retry of a rename that another process refused. The total bounds how
/// long a write can stall on a file someone else holds.
const RETRY_DELAYS_MS: [u64; 7] = [5, 10, 20, 40, 80, 160, 300];

/// Replace `dir/name` with `bytes`, atomically and durably (see the module notes).
pub(crate) fn replace_in(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let path = dir.join(name);
    let mut staged = tempfile::NamedTempFile::new_in(dir)?;
    io::Write::write_all(&mut staged, bytes)?;
    staged.as_file().sync_all()?;
    retry_while_held(
        staged,
        |staged| staged.persist(&path).map(drop).map_err(|refused| (refused.error, refused.file)),
        held_by_another_process,
        std::thread::sleep,
    )?;
    sync_dir(dir)
}

/// Run `rename` on `staged`, and again after each wait in [`RETRY_DELAYS_MS`] while it fails with an error
/// `retryable` accepts. A failed `rename` hands `staged` back for the next attempt; after the last one it is
/// dropped with the error returned.
fn retry_while_held<T>(
    mut staged: T,
    mut rename: impl FnMut(T) -> Result<(), (io::Error, T)>,
    retryable: impl Fn(&io::Error) -> bool,
    mut sleep: impl FnMut(Duration),
) -> io::Result<()> {
    let mut delays = RETRY_DELAYS_MS.iter();
    loop {
        let (error, back) = match rename(staged) {
            Ok(()) => return Ok(()),
            Err(refused) => refused,
        };
        match delays.next() {
            Some(&millis) if retryable(&error) => {
                sleep(Duration::from_millis(millis));
                staged = back;
            }
            _ => return Err(error),
        }
    }
}

/// Whether a refused rename on Windows means another process has the target open, which it usually stops
/// doing soon.
#[cfg(windows)]
fn held_by_another_process(error: &io::Error) -> bool {
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    matches!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION))
}

/// Elsewhere a rename is not refused because another process has the target open, so nothing is retried.
#[cfg(not(windows))]
fn held_by_another_process(_error: &io::Error) -> bool {
    false
}

/// Make the rename in `dir` durable by syncing the folder itself. A file system that cannot sync a folder
/// (`EINVAL`, `ENOTSUP`) is taken to have nothing to sync.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    match std::fs::File::open(dir).and_then(|folder| folder.sync_all()) {
        Ok(()) => Ok(()),
        Err(error) if matches!(error.kind(), io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported) => Ok(()),
        Err(error) => Err(io::Error::new(error.kind(), format!("the file was replaced, but syncing its folder failed: {error}"))),
    }
}

/// Windows cannot open a folder as a `File`, and there is no folder to sync on targets without a file system.
#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A fresh, empty temp folder for the test `name`.
    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("printscan-replace-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn held() -> io::Error {
        io::Error::new(io::ErrorKind::PermissionDenied, "held")
    }

    #[test]
    fn a_held_target_is_retried_with_growing_waits_until_it_is_released() {
        let mut attempts = 0;
        let mut waits = Vec::new();
        let result = retry_while_held(
            "staged",
            |staged| {
                attempts += 1;
                if attempts <= 3 { Err((held(), staged)) } else { Ok(()) }
            },
            |error| error.kind() == io::ErrorKind::PermissionDenied,
            |wait| waits.push(wait),
        );
        assert!(result.is_ok());
        assert_eq!(attempts, 4);
        assert_eq!(waits, [5, 10, 20].map(Duration::from_millis));
    }

    #[test]
    fn another_error_is_returned_at_once() {
        let mut attempts = 0;
        let mut waits = 0;
        let error = retry_while_held(
            (),
            |staged| {
                attempts += 1;
                Err((io::Error::new(io::ErrorKind::NotFound, "gone"), staged))
            },
            |error| error.kind() == io::ErrorKind::PermissionDenied,
            |_| waits += 1,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!((attempts, waits), (1, 0));
    }

    #[test]
    fn a_target_held_for_good_is_given_up_on_after_the_last_wait() {
        let mut attempts = 0;
        let mut waited = Duration::ZERO;
        let error = retry_while_held(
            (),
            |staged| {
                attempts += 1;
                Err((held(), staged))
            },
            |_| true,
            |wait| waited += wait,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(attempts, RETRY_DELAYS_MS.len() + 1);
        assert_eq!(waited, Duration::from_millis(RETRY_DELAYS_MS.iter().sum()));
        assert!(waited < Duration::from_secs(1), "a write never stalls for long");
    }

    #[test]
    fn the_staged_value_is_handed_back_for_each_attempt() {
        let mut seen = Vec::new();
        let result = retry_while_held(
            String::from("journal"),
            |staged| {
                seen.push(staged.clone());
                if seen.len() < 2 { Err((held(), staged)) } else { Ok(()) }
            },
            |_| true,
            |_| {},
        );
        assert!(result.is_ok());
        assert_eq!(seen, ["journal", "journal"]);
    }

    #[cfg(windows)]
    #[test]
    fn only_sharing_and_access_refusals_count_as_held_on_windows() {
        assert!(held_by_another_process(&io::Error::from_raw_os_error(32)));
        assert!(held_by_another_process(&io::Error::from_raw_os_error(5)));
        assert!(!held_by_another_process(&io::Error::from_raw_os_error(2)));
        assert!(!held_by_another_process(&io::Error::other("no code")));
    }

    #[cfg(not(windows))]
    #[test]
    fn nothing_counts_as_held_off_windows() {
        assert!(!held_by_another_process(&io::Error::from_raw_os_error(32)));
        assert!(!held_by_another_process(&io::Error::from(io::ErrorKind::PermissionDenied)));
    }

    #[test]
    fn the_bytes_replace_the_file_and_no_staging_file_is_left() {
        let dir = temp("bytes");
        replace_in(&dir, "a.json", b"first").unwrap();
        replace_in(&dir, "a.json", b"second, longer").unwrap();
        assert_eq!(std::fs::read(dir.join("a.json")).unwrap(), b"second, longer");
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|entry| entry.file_name()).collect();
        assert_eq!(names, ["a.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Open `path` the way an indexer or a scanner might: without letting anyone else read, write or delete it.
    #[cfg(windows)]
    fn hold(path: &Path) -> std::fs::File {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new().read(true).share_mode(0).open(path).unwrap()
    }

    #[cfg(windows)]
    #[test]
    fn a_target_held_by_another_handle_is_replaced_once_it_is_closed() {
        let dir = temp("held-briefly");
        replace_in(&dir, "a.json", b"old").unwrap();
        let holder = hold(&dir.join("a.json"));
        let writer = {
            let dir = dir.clone();
            std::thread::spawn(move || replace_in(&dir, "a.json", b"new"))
        };
        std::thread::sleep(Duration::from_millis(40));
        drop(holder);
        writer.join().unwrap().unwrap();
        assert_eq!(std::fs::read(dir.join("a.json")).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn a_target_held_throughout_is_left_as_it_was_and_the_staging_file_is_removed() {
        let dir = temp("held-throughout");
        replace_in(&dir, "a.json", b"old").unwrap();
        let holder = hold(&dir.join("a.json"));
        let error = replace_in(&dir, "a.json", b"new").unwrap_err();
        assert!(held_by_another_process(&error), "the rename was refused because of the holder: {error}");
        drop(holder);
        assert_eq!(std::fs::read(dir.join("a.json")).unwrap(), b"old");
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|entry| entry.file_name()).collect();
        assert_eq!(names, ["a.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
