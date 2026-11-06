//! Small widgets the windows and the clock build the same way.

use gtk::prelude::*;

/// A label with the CSS class `class`.
pub fn label(class: &str) -> gtk::Label {
    gtk::Label::builder().css_classes([class]).build()
}

/// A key cap followed by what the key does.
pub fn hint(key: &str, action: &str) -> gtk::Box {
    let hint = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    hint.add_css_class("hint");
    let cap = label("keycap");
    cap.set_label(key);
    let text = gtk::Label::new(Some(action));
    hint.append(&cap);
    hint.append(&text);
    hint
}

/// What a key cap shows for the GNOME accelerator `accelerator` (the `homeKey` setting):
/// a friendly name for the keys of a remote, else GTK's own label (`F10`, `Super+H`),
/// else the text as written.
pub fn key_label(accelerator: &str) -> String {
    if let Some(name) = friendly_name(accelerator) {
        return name.to_owned();
    }
    match gtk::accelerator_parse(accelerator) {
        Some((key, modifiers)) => gtk::accelerator_get_label(key, modifiers).to_string(),
        None => accelerator.to_owned(),
    }
}

/// The friendly name of a keysym that GTK would label with its raw name.
fn friendly_name(keysym: &str) -> Option<&'static str> {
    match keysym {
        "XF86HomePage" => Some("Home"),
        "XF86Back" => Some("Back"),
        "XF86Tools" => Some("Settings"),
        "Super_L" | "Super_R" => Some("Super"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_keys_get_friendly_names() {
        assert_eq!(friendly_name("XF86HomePage"), Some("Home"));
        assert_eq!(friendly_name("XF86Back"), Some("Back"));
        assert_eq!(friendly_name("XF86Tools"), Some("Settings"));
        assert_eq!(friendly_name("Super_L"), Some("Super"));
        assert_eq!(friendly_name("Super_R"), Some("Super"));
    }

    #[test]
    fn other_keys_have_no_friendly_name() {
        // These go on to GTK's own label, and the names are case sensitive.
        for name in ["F10", "<Super>h", "xf86homepage", "Home", ""] {
            assert_eq!(friendly_name(name), None, "{name}");
        }
    }
}
