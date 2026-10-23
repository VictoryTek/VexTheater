//! The launcher window: wallpaper, clock, and one page per tile row (the main row and
//! each submenu), with keyboard and mouse navigation. Also theater and desktop mode:
//! the launcher is shown full screen, or hidden to leave the GNOME desktop.

use crate::clock::Clock;
use crate::config::{Action, Builtin, Config, Tile};
use crate::mode::{self, Mode};
use crate::{gnome, launch};
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

const MAIN_PAGE: &str = "main";
const PAGE_MARGIN: i32 = 96;
const TILE_SPACING: i32 = 40;
/// Tiles are this big when the screen has room, and shrink to fit the row on it.
const TILE_MAX: i32 = 200;
const TILE_MIN: i32 = 100;
const ICON_SIZE: i32 = 96;
const LOGO_WIDTH: i32 = 200;
const POINTER_IDLE: Duration = Duration::from_secs(3);
const QUICK_FAILURE: Duration = Duration::from_secs(5);

thread_local! {
    static UI: RefCell<Option<Rc<Ui>>> = const { RefCell::new(None) };
}

/// Handles `vextheater` and `vextheater --home`, run here or forwarded from another
/// process to this, the running instance.
pub fn command_line(
    application: &adw::Application,
    command_line: &gio::ApplicationCommandLine,
) -> i32 {
    let arguments = command_line.arguments();
    let flags: Vec<_> = arguments
        .iter()
        .skip(1)
        .map(|a| a.to_string_lossy())
        .collect();
    match flags.as_slice() {
        [] => {
            activate(application);
            0
        }
        // What the autostart service runs: like no flag, but a launcher restarted
        // after a crash in desktop mode stays in desktop mode.
        [flag] if flag == "--resume" => {
            if mode::saved() == Some(Mode::Desktop) {
                ui(application);
            } else {
                activate(application);
            }
            0
        }
        [flag] if flag == "--home" => {
            if let Some(ui) = ui(application) {
                ui.home();
            }
            0
        }
        _ => {
            command_line.printerr_literal(
                "usage: vextheater [--home | --resume]\n  \
                 --home    close the open app and show the launcher\n  \
                 --resume  show the launcher, unless it crashed in desktop mode\n",
            );
            2
        }
    }
}

/// Shows the launcher (theater mode), e.g. from "Theater Mode" in the app grid.
pub fn activate(application: &adw::Application) {
    if let Some(ui) = ui(application) {
        ui.show();
    }
}

/// The launcher, built on first use. None when the configuration cannot be read; the
/// window then shows the error instead.
fn ui(application: &adw::Application) -> Option<Rc<Ui>> {
    if let Some(ui) = UI.with_borrow(Clone::clone) {
        return Some(ui);
    }
    if let Some(window) = application.windows().first() {
        window.present();
        return None;
    }

    let window = adw::ApplicationWindow::builder()
        .application(application)
        .title("VexTheater")
        .build();

    match Config::load_default() {
        Ok(config) => {
            let ui = Ui::build(&window, config);
            UI.set(Some(ui.clone()));
            Some(ui)
        }
        Err(error) => {
            log::error!("{error:#}");
            window.set_content(Some(
                &adw::StatusPage::builder()
                    .icon_name("dialog-error-symbolic")
                    .title("Configuration error")
                    .description(format!("{error:#}"))
                    .build(),
            ));
            window.fullscreen();
            window.present();
            None
        }
    }
}

/// What to do once the open app is closed.
enum Request {
    /// Stay on the launcher.
    Home,
    /// Start this tile's app (tile name, action).
    Tile(String, Action),
}

/// Lets one close run at a time. A request made while a close is running only replaces
/// the pending one, so when the close finishes, just the latest request is acted on.
struct CloseQueue<T> {
    closing: Cell<bool>,
    pending: RefCell<Option<T>>,
}

impl<T> CloseQueue<T> {
    fn new() -> Self {
        CloseQueue {
            closing: Cell::new(false),
            pending: RefCell::new(None),
        }
    }

    /// Makes `request` the pending one. True if no close was running, so the caller
    /// starts one.
    fn submit(&self, request: T) -> bool {
        self.pending.replace(Some(request));
        !self.closing.replace(true)
    }

    /// The running close has finished: the latest request, if still wanted.
    fn finish(&self) -> Option<T> {
        self.closing.set(false);
        self.pending.take()
    }

    /// Drops the pending request, e.g. when the launcher hides for desktop mode.
    fn cancel(&self) {
        self.pending.replace(None);
    }
}

struct Ui {
    config: Config,
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    /// The app started from a tile, if it is still running.
    running: RefCell<Option<launch::Running>>,
    /// Serialises closes; see `close_then`.
    closes: CloseQueue<Request>,
    stack: gtk::Stack,
    /// First tile of each page, focused when the page opens.
    first_tiles: RefCell<HashMap<String, gtk::Button>>,
    /// The row of tiles on each page, resized to the window by `fit_tiles`.
    rows: RefCell<Vec<gtk::Box>>,
    /// Pages left by opening a submenu, with the tile that opened each.
    history: RefCell<Vec<(String, gtk::Button)>>,
    /// Hover selects a tile only while the pointer is shown, so a tile moving under a
    /// hidden pointer cannot steal the keyboard's selection.
    pointer_visible: Cell<bool>,
    clock: Clock,
}

impl Ui {
    fn build(window: &adw::ApplicationWindow, config: Config) -> Rc<Self> {
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

        // Bottom-right, clear of the tile row and the key hints, like GNOME's
        // background logo.
        let logo = gtk::Picture::for_resource("/io/github/vextheater/vexos-logo.png");
        logo.add_css_class("vexos-logo");
        logo.set_content_fit(gtk::ContentFit::Contain);
        logo.set_can_shrink(true);
        logo.set_size_request(LOGO_WIDTH, LOGO_WIDTH * 164 / 437);
        logo.set_halign(gtk::Align::End);
        logo.set_valign(gtk::Align::End);
        logo.set_margin_end(96);
        logo.set_margin_bottom(52);
        logo.set_can_target(false);
        overlay.add_overlay(&logo);

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&overlay));

        let ui = Rc::new(Ui {
            config,
            window: window.clone(),
            toasts,
            running: RefCell::default(),
            closes: CloseQueue::new(),
            stack,
            first_tiles: RefCell::default(),
            rows: RefCell::default(),
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

        // The window is as wide as the screen, whatever its resolution: the tiles are
        // sized to that once the compositor reports it.
        window.connect_realize({
            let ui = ui.clone();
            move |window| {
                if let Some(surface) = window.surface() {
                    let ui = ui.clone();
                    surface.connect_layout(move |_, width, _| ui.fit_tiles(width));
                }
            }
        });

        ui.hide_idle_pointer(window);
        window.set_content(Some(&ui.toasts));
        let first = ui.first_tiles.borrow().get(MAIN_PAGE).cloned();
        GtkWindowExt::set_focus(window, first.as_ref());
        ui
    }

    /// Theater mode: the launcher full screen over the desktop.
    fn show(&self) {
        mode::save(Mode::Theater);
        gnome::theater_mode();
        self.window.fullscreen();
        self.window.present();
    }

    /// Closes the app started from a tile, or left over from a launcher that crashed,
    /// and returns to the main row.
    fn home(self: &Rc<Self>) {
        self.close_then(Request::Home);
        while self.back() {}
        self.show();
    }

    /// Hides the launcher. A tile or home chosen during a close that is still running
    /// is cancelled, so no app starts over the desktop once it finishes.
    fn desktop_mode(&self) {
        self.closes.cancel();
        mode::save(Mode::Desktop);
        gnome::restore();
        self.window.set_visible(false);
    }

    /// One row of tiles with its header, selected-tile title and key hints.
    fn add_page(self: &Rc<Self>, page: &str, submenu: Option<&str>, tiles: &[Tile]) {
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_top(64)
            .margin_bottom(52)
            .margin_start(PAGE_MARGIN)
            .margin_end(PAGE_MARGIN)
            .build();

        column.append(&self.header(submenu));

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(TILE_SPACING)
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
        self.rows.borrow_mut().push(row);
    }

    /// Sizes every row's tiles to fit a window `width` logical pixels wide, so the
    /// last tile is never pushed off a small screen.
    fn fit_tiles(&self, width: i32) {
        for row in self.rows.borrow().iter() {
            let mut tiles = Vec::new();
            let mut child = row.first_child();
            while let Some(tile) = child {
                child = tile.next_sibling();
                tiles.push(tile);
            }
            let size = tile_size(width, tiles.len());
            for tile in tiles {
                tile.set_size_request(size, size);
                if let Some(icon) = tile
                    .downcast_ref::<gtk::Button>()
                    .and_then(|button| button.child())
                    .and_then(|child| child.downcast::<gtk::Image>().ok())
                {
                    icon.set_pixel_size(size * ICON_SIZE / TILE_MAX);
                }
            }
        }
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

    fn activate(self: &Rc<Self>, tile: &Tile, button: &gtk::Button) {
        let result = match &tile.action {
            Action::Submenu(name) => {
                self.open(&submenu_page(name), button);
                Ok(())
            }
            Action::Builtin(Builtin::DesktopMode) => {
                self.desktop_mode();
                Ok(())
            }
            Action::Builtin(Builtin::Shutdown) => self.power(tile, "poweroff"),
            Action::Builtin(Builtin::Restart) => self.power(tile, "reboot"),
            action => {
                self.launch(tile, action);
                Ok(())
            }
        };
        if let Err(error) = result {
            self.failed(&tile.name, &error);
        }
    }

    fn failed(&self, name: &str, error: &anyhow::Error) {
        log::error!("{name}: {error:#}");
        self.toasts
            .add_toast(adw::Toast::new(&format!("Couldn't open {name}")));
    }

    /// systemctl fails when logind refuses, e.g. while another user is logged in.
    fn power(self: &Rc<Self>, tile: &Tile, verb: &str) -> anyhow::Result<()> {
        let process = launch::systemctl(verb)?;
        let (ui, name) = (self.clone(), tile.name.clone());
        process.wait_check_async(None::<&gio::Cancellable>, move |result| {
            if let Err(error) = result {
                ui.failed(&name, &error.into());
            }
        });
        Ok(())
    }

    /// One app at a time: the previous one is closed, and the new one starts once it
    /// is gone. A tile chosen while that is pending replaces the pending one.
    fn launch(self: &Rc<Self>, tile: &Tile, action: &Action) {
        self.close_then(Request::Tile(tile.name.clone(), action.clone()));
    }

    /// Closes the open app (and any left by a crashed launcher), then acts on the
    /// request. At most one close runs at a time: a tile or home chosen while one is
    /// running only replaces the pending request, and the close acts on the latest
    /// one when it finishes. Nothing starts while a close runs, so it never races an
    /// app it should leave alone, and only it and `start` touch the Flatpak record.
    ///
    /// The close is handed the app started last, so it can close that app by its own
    /// process even if the app has not registered with systemd or Flatpak yet.
    fn close_then(self: &Rc<Self>, request: Request) {
        if !self.closes.submit(request) {
            // Nothing has started since that close began, so `running` is empty.
            return;
        }
        let launched = self.running.take();
        let ui = self.clone();
        glib::spawn_future_local(async move {
            let closed = launch::stop_all(launched).await;
            let request = ui.closes.finish();
            match closed {
                Ok(()) => {}
                Err(launch::StopError::Timeout) => ui
                    .toasts
                    .add_toast(adw::Toast::new("Couldn't close the previous app")),
                Err(launch::StopError::GaveUp(apps)) => {
                    log::warn!("giving up on {apps:?}: they will not close");
                    ui.toasts.add_toast(adw::Toast::new(&format!(
                        "{} won't close; giving up on it",
                        apps.join(", ")
                    )));
                }
                Err(launch::StopError::Unknown(launched)) => {
                    // The previous app may still be running: start nothing over it.
                    // Choosing the tile again retries the close, still knowing the
                    // app started last (`running` is empty while a close runs).
                    ui.running.replace(launched);
                    ui.toasts.add_toast(adw::Toast::new(
                        "Couldn't check whether the previous app closed",
                    ));
                    return;
                }
            }
            if let Some(Request::Tile(name, action)) = request {
                ui.start(&name, &action);
            }
        });
    }

    fn start(self: &Rc<Self>, name: &str, action: &Action) {
        let running = match launch::start(action) {
            Ok(running) => running,
            Err(error) => return self.failed(name, &error),
        };
        let (ui, launch_id, name) = (self.clone(), running.id.clone(), name.to_owned());
        running
            .process
            .wait_check_async(None::<&gio::Cancellable>, move |result| {
                ui.app_exited(&launch_id, &name, result)
            });
        self.running.replace(Some(running));
    }

    /// GNOME focuses the launcher once the app's window is gone; presenting it again
    /// covers an app that leaves other windows behind. An app that fails within
    /// [`QUICK_FAILURE`] (not found, not installed) is reported. Apps the launcher
    /// closed itself are no longer in `running`, so they are not.
    fn app_exited(&self, launch_id: &str, name: &str, result: Result<(), glib::Error>) {
        let finished = {
            let mut running = self.running.borrow_mut();
            match running.as_ref() {
                Some(r) if r.id == launch_id => running.take(),
                _ => None,
            }
        };
        let Some(finished) = finished else {
            return;
        };
        if finished.started.elapsed() < QUICK_FAILURE {
            // A Flatpak whose launched process is gone within moments while Flatpak
            // still lists the app: log it, so a broken process-id assumption is seen.
            if let Some(record) = finished.flatpak_app() {
                glib::spawn_future_local(launch::check_early_exit(record));
            }
            if let Err(error) = result {
                self.failed(name, &error.into());
            }
        }
        if self.window.is_visible() {
            self.window.present();
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

/// The side of a square tile for a row of `count` tiles in a window `width` wide:
/// [`TILE_MAX`] if the row fits, else smaller, down to [`TILE_MIN`].
fn tile_size(width: i32, count: usize) -> i32 {
    let count = i32::try_from(count.max(1)).unwrap_or(i32::MAX);
    let room = width - 2 * PAGE_MARGIN - TILE_SPACING * (count - 1);
    (room / count).clamp(TILE_MIN, TILE_MAX)
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

    // Small until `fit_tiles` knows the window width: a larger minimum would make the
    // window wider than a small screen.
    let button = gtk::Button::builder()
        .child(&icon)
        .css_classes(["tile"])
        .valign(gtk::Align::Center)
        .width_request(TILE_MIN)
        .height_request(TILE_MIN)
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

#[cfg(test)]
mod tests {
    use super::{tile_size, CloseQueue, TILE_MAX, TILE_MIN};

    #[test]
    fn tiles_keep_full_size_on_a_wide_screen() {
        assert_eq!(tile_size(1600, 6), TILE_MAX);
        assert_eq!(tile_size(1920, 6), TILE_MAX);
        assert_eq!(tile_size(3840, 6), TILE_MAX);
    }

    #[test]
    fn tiles_shrink_so_the_row_fits_a_small_screen() {
        for width in [1280, 1366, 1440] {
            let size = tile_size(width, 6);
            assert!(size < TILE_MAX, "{width}: tiles shrink");
            assert!(6 * size + 5 * 40 + 2 * 96 <= width, "{width}: the row fits");
        }
    }

    #[test]
    fn tiles_never_shrink_below_the_minimum_or_divide_by_zero() {
        assert_eq!(tile_size(400, 6), TILE_MIN);
        assert_eq!(tile_size(1920, 0), TILE_MAX);
    }

    #[test]
    fn rapid_picks_start_one_close_and_act_only_on_the_last() {
        let queue = CloseQueue::new();
        assert!(queue.submit("brave"), "the first pick starts the close");
        assert!(!queue.submit("freetube"), "later picks only replace it");
        assert!(!queue.submit("kodi"));
        assert_eq!(queue.finish(), Some("kodi"));
        assert_eq!(queue.finish(), None, "a request is acted on once");
        assert!(queue.submit("brave"), "the next pick starts a new close");
    }

    #[test]
    fn desktop_mode_cancels_the_pending_request() {
        let queue = CloseQueue::new();
        assert!(queue.submit("brave"));
        queue.cancel();
        assert_eq!(queue.finish(), None);
    }
}
