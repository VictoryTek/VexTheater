//! The time and date for the top-right corner, and the greeting on the main row.

use crate::widgets::label;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

pub struct Clock {
    pub widget: gtk::Box,
    time: gtk::Label,
    date: gtk::Label,
    /// Weak, so the greeting of a page that was replaced is not kept alive.
    greetings: Greetings,
    time_format: &'static str,
}

impl Clock {
    pub fn new() -> Self {
        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
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
        if let Ok(now) = glib::DateTime::now_local() {
            greeting.set_label(greeting_for(&now));
        }
        self.greetings.borrow_mut().push(greeting.downgrade());
        greeting
    }

    /// Shows the current time, then refreshes it as each minute begins. The clock lives
    /// as long as the window, so the timer is never removed.
    pub fn start(&self) {
        tick(
            self.time.clone(),
            self.date.clone(),
            self.greetings.clone(),
            self.time_format,
        );
    }

    /// Shows the current time now. The timer counts time the machine is awake, so after
    /// a suspend it can be up to a minute behind until it next fires.
    pub fn refresh(&self) {
        update(&self.time, &self.date, &self.greetings, self.time_format);
    }
}

type Greetings = Rc<RefCell<Vec<glib::WeakRef<gtk::Label>>>>;

/// Updates the labels, and again just after the next minute begins, which is when the
/// time, and at the turn of an hour the greeting, can change.
fn tick(time: gtk::Label, date: gtk::Label, greetings: Greetings, format: &'static str) {
    update(&time, &date, &greetings, format);
    let wait = glib::DateTime::now_local()
        .map(|now| millis_until_next_minute(now.second(), now.microsecond()))
        .unwrap_or(5_000);
    glib::timeout_add_local_once(std::time::Duration::from_millis(wait), move || {
        tick(time, date, greetings, format);
    });
}

/// How long until the next minute begins, at `second` and `microsecond` into this one,
/// plus a few milliseconds so the update lands after the boundary, not before it.
fn millis_until_next_minute(second: i32, microsecond: i32) -> u64 {
    const MARGIN: i64 = 50;
    let into_minute = i64::from(second) * 1000 + i64::from(microsecond) / 1000;
    (60_000 - into_minute.clamp(0, 59_999) + MARGIN) as u64
}

fn update(
    time: &gtk::Label,
    date: &gtk::Label,
    greetings: &RefCell<Vec<glib::WeakRef<gtk::Label>>>,
    format: &str,
) {
    let Ok(now) = glib::DateTime::now_local() else {
        return;
    };
    if let Ok(text) = now.format(format) {
        time.set_label(&text);
    }
    if let Ok(text) = now.format("%A, %B %-d") {
        date.set_label(&text);
    }
    let greeting = greeting_for(&now);
    greetings.borrow_mut().retain(|label| {
        let Some(label) = label.upgrade() else {
            return false;
        };
        label.set_label(greeting);
        true
    });
}

fn greeting_for(now: &glib::DateTime) -> &'static str {
    match now.hour() {
        0..=11 => "Good morning",
        12..=17 => "Good afternoon",
        _ => "Good evening",
    }
}

/// Follows GNOME's clock setting.
fn uses_24h_clock() -> bool {
    crate::gnome::settings("org.gnome.desktop.interface")
        .is_some_and(|settings| settings.string("clock-format") == "24h")
}

#[cfg(test)]
mod tests {
    use super::millis_until_next_minute;

    // The update lands just after the minute begins, never before it.
    #[test]
    fn the_next_update_is_just_after_the_next_minute_begins() {
        assert_eq!(millis_until_next_minute(0, 0), 60_050);
        assert_eq!(millis_until_next_minute(30, 500_000), 29_550);
        assert_eq!(millis_until_next_minute(59, 999_000), 51);
        assert_eq!(millis_until_next_minute(59, 0), 1_050);
    }

    #[test]
    fn out_of_range_readings_still_give_a_sane_wait() {
        assert_eq!(millis_until_next_minute(-5, -1), 60_050);
        assert_eq!(millis_until_next_minute(61, 2_000_000), 51);
        for second in 0..60 {
            let wait = millis_until_next_minute(second, 0);
            assert!((1_000..=61_000).contains(&wait), "{second}: {wait}");
        }
    }
}
