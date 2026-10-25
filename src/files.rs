//! Small state files that must survive a crash part way through being written, and the
//! one way the launcher finds, reads, writes and removes them.

use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

/// A file of state that outlives the session, in `~/.local/state/vextheater`.
pub fn state_file(name: &str) -> PathBuf {
    glib::user_state_dir().join("vextheater").join(name)
}

/// A file of state that goes with the session, in the runtime directory, which is
/// cleared when the session ends.
pub fn runtime_file(name: &str) -> PathBuf {
    glib::user_runtime_dir().join("vextheater").join(name)
}

/// Replaces `path` with `value` as JSON, as [`write_atomic`] does.
pub fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> std::io::Result<()> {
    let json = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    write_atomic(path, &json)
}

/// The JSON in `path`. None for no file, which is the normal empty state and says
/// nothing; and for one that cannot be read or parsed, which is logged (`what` names the
/// content), since whatever is saved next overwrites it.
pub fn read_json<T: DeserializeOwned>(path: &Path, what: &str) -> Option<T> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return None,
        Err(error) => {
            log::warn!("cannot read {what} from {}: {error}", path.display());
            return None;
        }
    };
    serde_json::from_str(&text)
        .inspect_err(|error| log::warn!("ignoring {what} in {}: {error}", path.display()))
        .ok()
}

/// Removes `path`. A file that is not there is as good as removed; any other failure is
/// the caller's to report.
pub fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// Replaces `path` with `bytes`, so that a crash at any point leaves either the old
/// file or the new one, never a mix.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_file(path, bytes, |file, bytes| file.write_all(bytes))
}

/// Writes `bytes` to a temporary file beside `path` through `write` (which tests
/// replace to simulate a crash part way through), renames it over `path`, then syncs
/// the directory so the rename itself survives a power cut.
fn write_file(
    path: &Path,
    bytes: &[u8],
    write: impl FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);

    let mut file = std::fs::File::create(&temporary)?;
    write(&mut file, bytes)?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)?;
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vextheater-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn state_and_runtime_files_are_in_a_directory_of_the_launchers() {
        assert!(state_file("order.json").ends_with("vextheater/order.json"));
        assert!(runtime_file("mode").ends_with("vextheater/mode"));
    }

    #[test]
    fn json_is_read_back_as_it_was_written() {
        let path = scratch("roundtrip-values.json");
        write_json(&path, &vec![("B".to_owned(), 0u32), ("A".to_owned(), 1)]).unwrap();
        let read: Option<Vec<(String, u32)>> = read_json(&path, "the test values");
        assert_eq!(read, Some(vec![("B".to_owned(), 0), ("A".to_owned(), 1)]));
    }

    // No file is the normal empty state; so are a file that is not JSON, one of another
    // shape, and one that is not a file at all, though those are logged.
    #[test]
    fn json_that_is_missing_or_unusable_reads_as_none() {
        let missing = scratch("never-written.json");
        let _ = std::fs::remove_file(&missing);
        assert_eq!(read_json::<Vec<u32>>(&missing, "x"), None);

        let garbage = scratch("garbage-read.json");
        std::fs::write(&garbage, "{ not json").unwrap();
        assert_eq!(read_json::<Vec<u32>>(&garbage, "x"), None);

        let wrong_shape = scratch("wrong-shape.json");
        std::fs::write(&wrong_shape, r#"{ "a": 1 }"#).unwrap();
        assert_eq!(read_json::<Vec<u32>>(&wrong_shape, "x"), None);

        let directory = scratch("a-directory.json");
        std::fs::create_dir_all(&directory).unwrap();
        assert_eq!(read_json::<Vec<u32>>(&directory, "x"), None);
    }

    #[test]
    fn removing_a_file_that_is_not_there_is_not_an_error() {
        let path = scratch("remove-me.json");
        std::fs::write(&path, "x").unwrap();
        assert!(remove(&path).is_ok());
        assert!(!path.exists());
        assert!(remove(&path).is_ok(), "already gone");
    }

    // A failure other than "not there" reaches the caller: here the directory cannot be
    // written to, so the file cannot be unlinked.
    #[test]
    fn a_file_that_cannot_be_removed_is_an_error() {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            return; // root can remove it anyway
        }
        let dir = scratch("read-only-dir");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("order.json");
        std::fs::write(&path, "x").unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = remove(&path);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err(), "{result:?}");
        assert!(path.exists());
    }

    #[test]
    fn replaces_the_file() {
        let path = scratch("replace.json");
        std::fs::write(&path, "old").unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    }

    #[test]
    fn an_interrupted_write_leaves_the_previous_file_intact() {
        let path = scratch("interrupted.json");
        std::fs::write(&path, "previous").unwrap();

        let crash_half_way = |file: &mut std::fs::File, bytes: &[u8]| {
            file.write_all(&bytes[..bytes.len() / 2])?;
            Err(std::io::Error::other("simulated crash"))
        };
        assert!(write_file(&path, b"replacement", crash_half_way).is_err());

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "previous");
    }
}
