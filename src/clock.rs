//! The time and date in the top-right corner, and the greeting on the main row.

use crate::window::label;
use gtk::prelude::*;

pub struct Clock {
    pub widget: gtk::Box,
    time: gtk::Label,
    date: gtk::Label,
    greetings: std::cell::RefCell<Vec<gtk::Label>>,
    time_format: &'static str,
}

impl Clock {
    pub fn new() -> Self {
        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(64)
            .margin_end(96)
            .can_target(false)
            .build();
        let time = label("clock");
        time.set_halign(gtk::Align::End);
        let date = label("date");
        date.set_halign(gtk::Align::End);
        widget.append(&time);
        widget.append(&date);

        let time_format = if uses_24h_clock() {
            "%H:%M"
        } else {
            "%-l:%M %p"
        };
        Self {
            widget,
            time,
            date,
            greetings: Default::default(),
            time_format,
        }
    }

    /// A label that shows "Good morning" etc., kept current by the clock.
    pub fn greeting(&self) -> gtk::Label {
        let greeting = label("greeting");
        greeting.set_halign(gtk::Align::Start);
        self.greetings.borrow_mut().push(greeting.clone());
        greeting
    }

    /// Shows the current time, then refreshes it every few seconds. The clock lives
    /// as long as the window, so the timer is never removed.
    pub fn start(&self) {
        let (time, date, greetings, format) = (
            self.time.clone(),
            self.date.clone(),
            self.greetings.borrow().clone(),
            self.time_format,
        );
        update(&time, &date, &greetings, format);
        glib::timeout_add_seconds_local(5, move || {
            update(&time, &date, &greetings, format);
            glib::ControlFlow::Continue
        });
    }
}

fn update(time: &gtk::Label, date: &gtk::Label, greetings: &[gtk::Label], format: &str) {
    let Ok(now) = glib::DateTime::now_local() else {
        return;
    };
    if let Ok(text) = now.format(format) {
        time.set_label(&text);
    }
    if let Ok(text) = now.format("%A, %B %-d") {
        date.set_label(&text);
    }
    let greeting = match now.hour() {
        0..=11 => "Good morning",
        12..=17 => "Good afternoon",
        _ => "Good evening",
    };
    for label in greetings {
        label.set_label(greeting);
    }
}

/// Follows GNOME's clock setting.
fn uses_24h_clock() -> bool {
    crate::gnome::settings("org.gnome.desktop.interface")
        .is_some_and(|settings| settings.string("clock-format") == "24h")
}
