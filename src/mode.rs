//! Which mode the launcher is in, kept in the runtime directory so a launcher restarted
//! after a crash can go back to it. The runtime directory is cleared when the session
//! ends, and the file is removed on a clean exit, so only a crash leaves it behind.

use crate::{files, gnome};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// The launcher is shown full screen.
    Theater,
    /// The launcher is hidden and the GNOME desktop is in use.
    Desktop,
}

fn file() -> PathBuf {
    files::runtime_file("mode")
}

/// Theater mode: the launcher is shown, and GNOME's hot corner, Super key and banners
/// are turned off until it leaves.
pub fn enter_theater() {
    save(Mode::Theater);
    gnome::theater_mode();
}

/// Desktop mode: GNOME's settings are put back.
pub fn enter_desktop() {
    save(Mode::Desktop);
    gnome::restore();
}

/// The launcher is quitting: GNOME's settings are put back and the mode forgotten.
pub fn exit() {
    gnome::restore();
    clear();
}

fn save(mode: Mode) {
    let name = match mode {
        Mode::Theater => "theater",
        Mode::Desktop => "desktop",
    };
    if let Err(error) = files::write_atomic(&file(), name.as_bytes()) {
        log::warn!("cannot save the mode: {error}");
    }
}

pub fn saved() -> Option<Mode> {
    parse(&std::fs::read_to_string(file()).ok()?)
}

/// Forgets the mode; called on a clean exit, which needs nothing restored.
pub fn clear() {
    if let Err(error) = files::remove(&file()) {
        log::warn!("cannot forget the mode: {error}");
    }
}

fn parse(text: &str) -> Option<Mode> {
    match text.trim() {
        "theater" => Some(Mode::Theater),
        "desktop" => Some(Mode::Desktop),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_modes_it_writes() {
        assert_eq!(parse("theater"), Some(Mode::Theater));
        assert_eq!(parse("desktop\n"), Some(Mode::Desktop));
        assert_eq!(parse(""), None);
        assert_eq!(parse("garbage"), None);
    }
}
