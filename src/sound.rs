//! The click sounds: a tick as the selection moves between tiles, and a click as a
//! tile is chosen. They follow GNOME's "event sounds" setting.

use gtk::prelude::*;

const SCHEMA: &str = "org.gnome.desktop.sound";
const KEY: &str = "event-sounds";

pub struct Sounds {
    moved: gtk::MediaFile,
    chosen: gtk::MediaFile,
    /// Read at each sound, so switching event sounds off or on takes effect at once.
    /// None where the setting does not exist (a bare dev shell): the sounds play.
    settings: Option<gio::Settings>,
}

impl Sounds {
    /// Needs GTK initialised. Each sound is loaded now, so the first one is not late.
    pub fn new() -> Self {
        Sounds {
            moved: load("move.wav"),
            chosen: load("select.wav"),
            settings: crate::gnome::settings(SCHEMA)
                .filter(|settings| settings.settings_schema().is_some_and(|s| s.has_key(KEY))),
        }
    }

    pub fn moved(&self) {
        if self.enabled() {
            play(&self.moved);
        }
    }

    pub fn chosen(&self) {
        if self.enabled() {
            play(&self.chosen);
        }
    }

    fn enabled(&self) -> bool {
        self.settings
            .as_ref()
            .is_none_or(|settings| settings.boolean(KEY))
    }
}

fn load(name: &'static str) -> gtk::MediaFile {
    let sound = gtk::MediaFile::for_resource(&format!("/io/github/vextheater/sounds/{name}"));
    // Without a GStreamer backend or an audio output the launcher stays silent: say why.
    sound.connect_error_notify(move |sound| {
        if let Some(error) = sound.error() {
            log::warn!("sound {name}: {error}");
        }
    });
    sound
}

/// Plays from the start. `play` does nothing while a sound is still playing, so a
/// quick second tick would be lost without the seek, which restarts it instead. The
/// seek does nothing until the sound is loaded, and then there is nothing to restart.
fn play(sound: &gtk::MediaFile) {
    sound.seek(0);
    sound.play();
}

#[cfg(test)]
mod tests {
    /// The sounds are made by `scripts/make-sounds.py`: 16-bit mono PCM, and short
    /// enough to be over before the next key press.
    #[test]
    fn the_bundled_sounds_are_short_wav_files() {
        for (name, wav) in [
            ("move", &include_bytes!("../data/sounds/move.wav")[..]),
            ("select", &include_bytes!("../data/sounds/select.wav")[..]),
        ] {
            assert_eq!(&wav[..4], b"RIFF", "{name}");
            assert_eq!(&wav[8..12], b"WAVE", "{name}");
            let rate = u32::from_le_bytes(wav[24..28].try_into().unwrap());
            let seconds = (wav.len() - 44) as f64 / 2.0 / f64::from(rate);
            assert!(seconds > 0.0 && seconds < 0.25, "{name}: {seconds} s");
        }
    }
}
