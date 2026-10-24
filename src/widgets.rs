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
