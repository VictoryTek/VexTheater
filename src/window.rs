//! The launcher window: wallpaper, clock, and one page per tile row (the main row and
//! each submenu), with keyboard and mouse navigation.

use crate::clock::Clock;
use crate::config::{Action, Config, Tile};
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

const MAIN_PAGE: &str = "main";
const ICON_SIZE: i32 = 96;
const POINTER_IDLE: Duration = Duration::from_secs(3);

pub fn present(application: &adw::Application) {
    if let Some(window) = application.active_window() {
        window.present();
        return;
    }

    let window = adw::ApplicationWindow::builder()
        .application(application)
        .title("VexTheater")
        .build();

    match Config::load_default() {
        Ok(config) => Ui::build(&window, config),
        Err(error) => {
            log::error!("{error:#}");
            window.set_content(Some(
                &adw::StatusPage::builder()
                    .icon_name("dialog-error-symbolic")
                    .title("Configuration error")
                    .description(format!("{error:#}"))
                    .build(),
            ));
        }
    }

    window.fullscreen();
    window.present();
}

struct Ui {
    config: Config,
    stack: gtk::Stack,
    /// First tile of each page, focused when the page opens.
    first_tiles: RefCell<HashMap<String, gtk::Button>>,
    /// Pages left by opening a submenu, with the tile that opened each.
    history: RefCell<Vec<(String, gtk::Button)>>,
    /// Hover selects a tile only while the pointer is shown, so a tile moving under a
    /// hidden pointer cannot steal the keyboard's selection.
    pointer_visible: Cell<bool>,
    clock: Clock,
}

impl Ui {
    fn build(window: &adw::ApplicationWindow, config: Config) {
        let overlay = gtk::Overlay::new();
        overlay.add_css_class("theater");

        let background: gtk::Widget = match &config.background {
            Some(path) => gtk::Picture::builder()
                .file(&gio::File::for_path(path))
                .content_fit(gtk::ContentFit::Cover)
                .can_shrink(true)
                .build()
                .upcast(),
            None => {
                let plain = gtk::Box::new(gtk::Orientation::Vertical, 0);
                plain.add_css_class("plain-background");
                plain.upcast()
            }
        };
        overlay.set_child(Some(&background));

        let scrim = gtk::Box::new(gtk::Orientation::Vertical, 0);
        scrim.add_css_class("scrim");
        scrim.set_can_target(false);
        overlay.add_overlay(&scrim);

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(200)
            .build();
        overlay.add_overlay(&stack);

        let clock = Clock::new();
        overlay.add_overlay(&clock.widget);

        let ui = Rc::new(Ui {
            config,
            stack,
            first_tiles: RefCell::default(),
            history: RefCell::default(),
            pointer_visible: Cell::new(false),
            clock,
        });

        ui.add_page(MAIN_PAGE, None, &ui.config.tiles);
        for (name, tiles) in &ui.config.submenus {
            ui.add_page(&submenu_page(name), Some(name), tiles);
        }
        ui.clock.start();

        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed({
            let ui = ui.clone();
            move |_, key, _, _| match key {
                gtk::gdk::Key::Escape | gtk::gdk::Key::BackSpace if ui.back() => {
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        window.add_controller(keys);

        ui.hide_idle_pointer(window);
        window.set_content(Some(&overlay));
        let first = ui.first_tiles.borrow().get(MAIN_PAGE).cloned();
        GtkWindowExt::set_focus(window, first.as_ref());
    }

    /// One row of tiles with its header, selected-tile title and key hints.
    fn add_page(self: &Rc<Self>, page: &str, submenu: Option<&str>, tiles: &[Tile]) {
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_top(64)
            .margin_bottom(52)
            .margin_start(96)
            .margin_end(96)
            .build();

        column.append(&self.header(submenu));

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(40)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::End)
            .vexpand(true)
            .build();
        column.append(&row);

        let name = label("tile-name");
        name.set_margin_top(44);
        let description = label("tile-description");
        description.set_margin_top(10);
        column.append(&name);
        column.append(&description);
        column.append(&self.hints(submenu.is_some()));

        for (index, tile) in tiles.iter().enumerate() {
            let button = tile_button(tile);
            if index == 0 {
                self.first_tiles
                    .borrow_mut()
                    .insert(page.to_owned(), button.clone());
                show_selected(&name, &description, tile);
            }

            let focus = gtk::EventControllerFocus::new();
            focus.connect_enter({
                let (name, description) = (name.clone(), description.clone());
                let (ui, page, index) = (self.clone(), page.to_owned(), index);
                move |_| show_selected(&name, &description, ui.tile(&page, index))
            });
            button.add_controller(focus);

            let hover = gtk::EventControllerMotion::new();
            hover.connect_enter({
                let (ui, button) = (self.clone(), button.downgrade());
                move |_, _, _| {
                    if let Some(button) = button.upgrade().filter(|_| ui.pointer_visible.get()) {
                        button.grab_focus();
                    }
                }
            });
            button.add_controller(hover);

            button.connect_clicked({
                let (ui, page) = (self.clone(), page.to_owned());
                move |button| ui.activate(ui.tile(&page, index), button)
            });

            row.append(&button);
        }

        self.stack.add_named(&column, Some(page));
    }

    fn header(&self, submenu: Option<&str>) -> gtk::Box {
        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .halign(gtk::Align::Start)
            .build();

        match submenu {
            None => {
                header.append(&self.clock.greeting());
                if let Some(title) = &self.config.title {
                    let subtitle = label("subtitle");
                    subtitle.set_label(title);
                    subtitle.set_halign(gtk::Align::Start);
                    header.append(&subtitle);
                }
            }
            Some(name) => {
                header.set_spacing(14);
                header.append(&hint("Esc", "Back"));
                let title = label("greeting");
                title.set_label(name);
                header.append(&title);
            }
        }
        header
    }

    fn hints(&self, in_submenu: bool) -> gtk::Box {
        let hints = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(32)
            .margin_top(48)
            .build();
        hints.append(&hint("← →", "Move"));
        if in_submenu {
            hints.append(&hint("Enter", "Select"));
            hints.append(&hint("Esc", "Back"));
        } else {
            hints.append(&hint("Enter", "Open"));
            if let Some(key) = &self.config.home_key {
                hints.append(&hint(key, "Home (closes the open app)"));
            }
        }
        hints
    }

    fn tile(&self, page: &str, index: usize) -> &Tile {
        let tiles = match page.strip_prefix("submenu:") {
            Some(name) => &self.config.submenus[name],
            None => &self.config.tiles,
        };
        &tiles[index]
    }

    fn activate(&self, tile: &Tile, button: &gtk::Button) {
        match &tile.action {
            Action::Submenu(name) => self.open(&submenu_page(name), button),
            action => log::warn!("launching is not implemented yet: {action:?}"),
        }
    }

    fn open(&self, page: &str, opener: &gtk::Button) {
        let current = self
            .stack
            .visible_child_name()
            .map_or_else(|| MAIN_PAGE.to_owned(), |name| name.to_string());
        self.history.borrow_mut().push((current, opener.clone()));
        self.stack.set_visible_child_name(page);
        if let Some(first) = self.first_tiles.borrow().get(page) {
            first.grab_focus();
        }
    }

    /// Returns to the page this submenu was opened from; false on the main page.
    fn back(&self) -> bool {
        let Some((page, opener)) = self.history.borrow_mut().pop() else {
            return false;
        };
        self.stack.set_visible_child_name(&page);
        opener.grab_focus();
        true
    }

    /// Hides the pointer after [`POINTER_IDLE`] without movement, and from the start:
    /// a TV is driven by keyboard or remote until the mouse moves.
    fn hide_idle_pointer(self: &Rc<Self>, window: &adw::ApplicationWindow) {
        window.set_cursor_from_name(Some("none"));
        let last = Cell::new((f64::NAN, f64::NAN));
        let timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();

        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion({
            let (ui, window) = (self.clone(), window.clone());
            move |_, x, y| {
                // Widgets moving under a still pointer also report motion.
                let (last_x, last_y) = last.replace((x, y));
                if (x - last_x).abs() < 1.0 && (y - last_y).abs() < 1.0 {
                    return;
                }
                if !ui.pointer_visible.replace(true) {
                    window.set_cursor(None);
                }
                if let Some(pending) = timer.take() {
                    pending.remove();
                }
                let (ui, window, slot) = (ui.clone(), window.clone(), timer.clone());
                timer.replace(Some(glib::timeout_add_local_once(
                    POINTER_IDLE,
                    move || {
                        slot.take();
                        ui.pointer_visible.set(false);
                        window.set_cursor_from_name(Some("none"));
                    },
                )));
            }
        });
        window.add_controller(motion);
    }
}

fn submenu_page(name: &str) -> String {
    format!("submenu:{name}")
}

fn tile_button(tile: &Tile) -> gtk::Button {
    let icon = if tile.icon_is_file() {
        gtk::Image::from_gicon(&gio::FileIcon::new(&gio::File::for_path(&tile.icon)))
    } else {
        gtk::Image::from_icon_name(&tile.icon)
    };
    icon.set_pixel_size(ICON_SIZE);

    let button = gtk::Button::builder()
        .child(&icon)
        .css_classes(["tile"])
        .valign(gtk::Align::Center)
        .build();
    button.update_property(&[gtk::accessible::Property::Label(&tile.name)]);
    button
}

fn show_selected(name: &gtk::Label, description: &gtk::Label, tile: &Tile) {
    name.set_label(&tile.name);
    description.set_label(tile.description.as_deref().unwrap_or(""));
}

pub fn label(class: &str) -> gtk::Label {
    gtk::Label::builder().css_classes([class]).build()
}

/// A key cap followed by what the key does.
fn hint(key: &str, action: &str) -> gtk::Box {
    let hint = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    hint.add_css_class("hint");
    let cap = label("keycap");
    cap.set_label(key);
    let text = gtk::Label::new(Some(action));
    hint.append(&cap);
    hint.append(&text);
    hint
}
