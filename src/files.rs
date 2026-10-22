//! Small state files that must survive a crash part way through being written.

use std::io::Write;
use std::path::{Path, PathBuf};

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
