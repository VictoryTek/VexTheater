//! The launcher window: wallpaper, clock, and one page per tile row (the main row and
//! each submenu), with keyboard and mouse navigation. Also theater and desktop mode:
//! the launcher is shown full screen, or hidden to leave the GNOME desktop.

use crate::clock::Clock;
use crate::config::{Action, Builtin, Config, Tile};
use crate::mode::{self, Mode};
use crate::sound::Sounds;
use crate::{gnome, launch, order};
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

const MAIN_PAGE: &str = "main";
const POWER_PAGE: &str = "power";
const CONFIRM_PREFIX: &str = "confirm:";
const PAGE_MARGIN: i32 = 96;
const TILE_SPACING: i32 = 40;
/// Tiles are this big when the screen has room, and shrink to fit the row on it.
const TILE_MAX: i32 = 200;
const TILE_MIN: i32 = 100;
const ICON_SIZE: i32 = 96;
const LOGO_WIDTH: i32 = 200;
const POINTER_IDLE: Duration = Duration::from_secs(3);
const QUICK_FAILURE: Duration = Duration::from_secs(5);
/// Holding Enter on a tile this long picks it up.
const LONG_PRESS: Duration = Duration::from_millis(600);
const PICKED_HINT: &str = "← →  move  ·  Enter  drop  ·  Back  cancel";
const ARRANGING_HINT: &str = "Enter  pick up a tile  ·  Back  done";

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

/// A row of tiles the user can rearrange: the main row and the configured submenus.
struct Arrangeable {
    row: gtk::Box,
    /// In configured order, so a button's position here is its index for `Ui::tile`.
    buttons: Vec<gtk::Button>,
}

/// The tile picked up for moving.
struct Picked {
    page: String,
    button: gtk::Button,
    /// Its place in the row when picked up, to put it back on cancel.
    from: usize,
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
    /// Pages the launcher adds itself, beside the configured ones.
    own_pages: Vec<OwnPage>,
    /// First tile of each page, focused when the page opens.
    first_tiles: RefCell<HashMap<String, gtk::Button>>,
    /// The row of tiles on each page, resized to the window by `fit_tiles`.
    rows: RefCell<Vec<gtk::Box>>,
    /// Pages left by opening a submenu, with the tile that opened each.
    history: RefCell<Vec<(String, gtk::Button)>>,
    /// Hover selects a tile only while the pointer is shown, so a tile moving under a
    /// hidden pointer cannot steal the keyboard's selection.
    pointer_visible: Cell<bool>,
    sounds: Sounds,
    /// A navigation key or the pointer is moving the selection. Selection also moves
    /// when a page opens or the window is shown again, and those stay silent.
    navigating: Cell<bool>,
    clock: Clock,
    arrangeable: RefCell<HashMap<String, Arrangeable>>,
    /// The arranged order, as saved to disk.
    saved_order: RefCell<order::Saved>,
    picked: RefCell<Option<Picked>>,
    /// Enter picks a tile up instead of opening it, until Back. Set from the power menu,
    /// for a remote that cannot hold Enter or press Menu.
    arranging: Cell<bool>,
    /// Enter is held down. Keyboard auto-repeat sends more presses while it is, and
    /// they must not pick a tile up or drop it again.
    enter_down: Cell<bool>,
    /// Counts down a long press of Enter on a tile.
    enter_timer: RefCell<Option<glib::SourceId>>,
    /// What the keys do while a tile is picked up or arranging.
    arrange_hint: gtk::Label,
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
        let power_button = power_button();
        let corner = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(40)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(64)
            .margin_end(PAGE_MARGIN)
            .build();
        corner.append(&power_button);
        corner.append(&clock.widget);
        overlay.add_overlay(&corner);

        let arrange_hint = label("arrange-hint");
        arrange_hint.set_halign(gtk::Align::Center);
        arrange_hint.set_valign(gtk::Align::Start);
        arrange_hint.set_margin_top(180);
        arrange_hint.set_visible(false);
        arrange_hint.set_can_target(false);
        overlay.add_overlay(&arrange_hint);

        // Bottom-right, clear of the tile row and the key hints, like GNOME's
        // background logo. A picture is as wide as its image whatever size is
        // requested, so it sits in a window that has the size and no scroll bars.
        let logo_picture = gtk::Picture::for_resource("/io/github/vextheater/vexos-logo.png");
        logo_picture.set_content_fit(gtk::ContentFit::Contain);
        logo_picture.set_can_shrink(true);
        let logo = gtk::ScrolledWindow::builder()
            .child(&logo_picture)
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::External)
            .min_content_width(LOGO_WIDTH)
            .min_content_height(LOGO_WIDTH * 164 / 437)
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .margin_end(PAGE_MARGIN)
            .margin_bottom(52)
            .can_target(false)
            .focusable(false)
            .css_classes(["vexos-logo"])
            .build();
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
            own_pages: own_pages(),
            first_tiles: RefCell::default(),
            rows: RefCell::default(),
            history: RefCell::default(),
            pointer_visible: Cell::new(false),
            sounds: Sounds::new(),
            navigating: Cell::new(false),
            clock,
            arrangeable: RefCell::default(),
            saved_order: RefCell::new(order::load()),
            picked: RefCell::default(),
            arranging: Cell::new(false),
            enter_down: Cell::new(false),
            enter_timer: RefCell::default(),
            arrange_hint,
        });

        ui.add_page(MAIN_PAGE, None, &ui.config.tiles, true);
        for (name, tiles) in &ui.config.submenus {
            ui.add_page(&submenu_page(name), Some(name), tiles, true);
        }
        for page in &ui.own_pages {
            ui.add_page(page.key, Some(page.title), &page.tiles, false);
        }
        ui.clock.start();

        power_button.connect_clicked({
            let ui = ui.clone();
            move |button| {
                ui.sounds.chosen();
                ui.open(POWER_PAGE, button);
            }
        });
        // The power button belongs to the main row only.
        ui.stack.connect_visible_child_name_notify({
            let power_button = power_button.clone();
            move |stack| {
                power_button.set_visible(stack.visible_child_name().as_deref() == Some(MAIN_PAGE));
            }
        });

        // GTK's arrow keys do not reach the button from the tiles, so Up and Down
        // cross between them here. Down goes back to the tile Up left.
        let keys = gtk::EventControllerKey::new();
        let last_tile: Rc<RefCell<Option<gtk::Widget>>> = Rc::default();
        keys.connect_key_pressed({
            let ui = ui.clone();
            move |_, key, _, _| match key {
                gtk::gdk::Key::Escape | gtk::gdk::Key::BackSpace if ui.back() => {
                    ui.sounds.chosen();
                    glib::Propagation::Stop
                }
                gtk::gdk::Key::Up if ui.on_main_page() && !power_button.is_focus() => {
                    last_tile.replace(GtkWindowExt::focus(&ui.window));
                    power_button.grab_focus();
                    ui.sounds.moved();
                    glib::Propagation::Stop
                }
                gtk::gdk::Key::Down if power_button.is_focus() => {
                    let first = ui.first_tiles.borrow().get(MAIN_PAGE).cloned();
                    if let Some(tile) = last_tile.take().or(first.map(Cast::upcast)) {
                        tile.grab_focus();
                    }
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        window.add_controller(keys);

        // Before the keys above and GTK's own handling move the selection, mark that
        // a key is moving it; the mark ends once the key press is dealt with.
        let navigation = gtk::EventControllerKey::new();
        navigation.set_propagation_phase(gtk::PropagationPhase::Capture);
        navigation.connect_key_pressed({
            let ui = ui.clone();
            move |_, key, _, _| {
                if is_navigation(key) {
                    ui.navigating.set(true);
                    let ui = ui.clone();
                    glib::idle_add_local_once(move || ui.navigating.set(false));
                }
                glib::Propagation::Proceed
            }
        });
        window.add_controller(navigation);

        // Picking tiles up and moving them. Capture phase, so a picked tile's arrow keys
        // never reach GTK's focus movement, and Enter can wait for its release to tell
        // a tap (open) from a long press (pick up).
        let arranging = gtk::EventControllerKey::new();
        arranging.set_propagation_phase(gtk::PropagationPhase::Capture);
        arranging.connect_key_pressed({
            let ui = ui.clone();
            move |_, key, _, _| {
                if ui.arrange_key(key) {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        });
        arranging.connect_key_released({
            let ui = ui.clone();
            move |_, key, _, _| {
                if is_enter(key) {
                    ui.enter_released();
                }
            }
        });
        window.add_controller(arranging);

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
        self.main_row();
        self.show();
    }

    /// Hides the launcher. A tile or home chosen during a close that is still running
    /// is cancelled, so no app starts over the desktop once it finishes.
    fn desktop_mode(&self) {
        // Back on the main row when the launcher comes back, not in the power menu.
        self.main_row();
        self.closes.cancel();
        mode::save(Mode::Desktop);
        gnome::restore();
        self.window.set_visible(false);
    }

    /// Leaves every submenu and selects the first tile, not the power button a
    /// submenu may have been opened from.
    fn main_row(&self) {
        self.stop_arranging();
        while self.back() {}
        if let Some(first) = self.first_tiles.borrow().get(MAIN_PAGE) {
            first.grab_focus();
        }
    }

    /// One row of tiles with its header, selected-tile title and key hints. The tiles of
    /// an `arrangeable` page are shown in the order the user saved for it.
    fn add_page(
        self: &Rc<Self>,
        page: &str,
        submenu: Option<&str>,
        tiles: &[Tile],
        arrangeable: bool,
    ) {
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

        let positions = if arrangeable {
            let names: Vec<_> = tiles.iter().map(|tile| tile.name.as_str()).collect();
            order::arrange(&names, self.saved_order.borrow().get(page))
        } else {
            (0..tiles.len()).collect()
        };
        let mut buttons = Vec::new();
        for (position, index) in positions.into_iter().enumerate() {
            let tile = &tiles[index];
            let button = tile_button(tile);
            if position == 0 {
                self.first_tiles
                    .borrow_mut()
                    .insert(page.to_owned(), button.clone());
                show_selected(&name, &description, tile);
            }

            let focus = gtk::EventControllerFocus::new();
            focus.connect_enter({
                let (name, description) = (name.clone(), description.clone());
                let (ui, page, index) = (self.clone(), page.to_owned(), index);
                move |_| {
                    show_selected(&name, &description, ui.tile(&page, index));
                    if ui.navigating.get() {
                        ui.sounds.moved();
                    }
                }
            });
            button.add_controller(focus);

            let hover = gtk::EventControllerMotion::new();
            hover.connect_enter({
                let (ui, button) = (self.clone(), button.downgrade());
                move |_, _, _| {
                    // A picked tile keeps the selection until it is dropped.
                    let steady = ui.pointer_visible.get() && ui.picked.borrow().is_none();
                    if let Some(button) = button.upgrade().filter(|_| steady) {
                        ui.navigating.set(true);
                        button.grab_focus();
                        ui.navigating.set(false);
                    }
                }
            });
            button.add_controller(hover);

            button.connect_clicked({
                let (ui, page) = (self.clone(), page.to_owned());
                move |button| {
                    if ui.click_while_arranging(&page, button) {
                        return;
                    }
                    ui.sounds.chosen();
                    ui.activate(&page, ui.tile(&page, index), button);
                }
            });

            row.append(&button);
            buttons.push((index, button));
        }

        if arrangeable {
            buttons.sort_by_key(|(index, _)| *index);
            self.arrangeable.borrow_mut().insert(
                page.to_owned(),
                Arrangeable {
                    row: row.clone(),
                    buttons: buttons.into_iter().map(|(_, button)| button).collect(),
                },
            );
        }
        self.stack.add_named(&column, Some(page));
        self.rows.borrow_mut().push(row);
    }

    /// The tile with keyboard focus on the visible page, if that page can be rearranged.
    fn focused_tile(&self) -> Option<(String, gtk::Button)> {
        let page = self.stack.visible_child_name()?.to_string();
        let focus = GtkWindowExt::focus(&self.window)?;
        let arrangeable = self.arrangeable.borrow();
        let tiles = arrangeable.get(&page)?;
        let button = tiles
            .buttons
            .iter()
            .find(|b| b.upcast_ref::<gtk::Widget>() == &focus)?;
        Some((page, button.clone()))
    }

    /// Handles a key press that picks tiles up, moves or drops them. True if it did.
    fn arrange_key(self: &Rc<Self>, key: gtk::gdk::Key) -> bool {
        use gtk::gdk::Key;
        if is_enter(key) {
            return self.enter_pressed();
        }
        if self.picked.borrow().is_some() {
            match key {
                Key::Left => self.move_picked(-1),
                Key::Right => self.move_picked(1),
                Key::Menu | Key::F2 => self.drop_picked(),
                Key::Escape | Key::BackSpace => self.cancel_picked(),
                // The picked tile keeps the selection.
                Key::Up | Key::Down | Key::Tab | Key::ISO_Left_Tab => {}
                _ => return false,
            }
            return true;
        }
        match key {
            Key::Menu | Key::F2 => match self.focused_tile() {
                Some((page, button)) => {
                    self.pick_up(&page, &button);
                    true
                }
                None => false,
            },
            Key::Escape | Key::BackSpace if self.arranging.get() && self.on_main_page() => {
                self.stop_arranging();
                self.sounds.chosen();
                true
            }
            _ => false,
        }
    }

    /// Enter on a tile of a rearrangeable row: while arranging it picks the tile up at
    /// once; otherwise the tile opens when Enter is released, unless it is held for
    /// [`LONG_PRESS`], which picks the tile up. With a tile picked up, Enter drops it.
    /// True if the press is dealt with here, false to leave it to GTK.
    fn enter_pressed(self: &Rc<Self>) -> bool {
        let picked = self.picked.borrow().is_some();
        let tile = if picked { None } else { self.focused_tile() };
        if !picked && tile.is_none() {
            return false;
        }
        if self.enter_down.replace(true) {
            // Auto-repeat of a press that is already dealt with.
            return true;
        }
        match tile {
            None => self.drop_picked(),
            Some((page, button)) if self.arranging.get() => self.pick_up(&page, &button),
            Some((page, button)) => {
                let ui = self.clone();
                self.enter_timer.replace(Some(glib::timeout_add_local_once(
                    LONG_PRESS,
                    move || {
                        ui.enter_timer.take();
                        ui.pick_up(&page, &button);
                    },
                )));
            }
        }
        true
    }

    /// A tap of Enter on a tile opens it now that the key is up; after a long press the
    /// tile was already picked up, and there is nothing to do.
    fn enter_released(&self) {
        if !self.enter_down.replace(false) {
            return;
        }
        if let Some(timer) = self.enter_timer.take() {
            timer.remove();
            if let Some((_, button)) = self.focused_tile() {
                button.emit_clicked();
            }
        }
    }

    /// A click on a tile while one is picked up drops it, and while arranging picks the
    /// tile up. True if the click was used for that, so it does not open the tile.
    fn click_while_arranging(&self, page: &str, button: &gtk::Button) -> bool {
        if self.picked.borrow().is_some() {
            self.drop_picked();
            return true;
        }
        if self.arranging.get() && self.arrangeable.borrow().contains_key(page) {
            self.pick_up(page, button);
            return true;
        }
        false
    }

    fn pick_up(&self, page: &str, button: &gtk::Button) {
        let from = {
            let arrangeable = self.arrangeable.borrow();
            let Some(tiles) = arrangeable.get(page) else {
                return;
            };
            children(&tiles.row)
                .iter()
                .position(|child| child == button.upcast_ref::<gtk::Widget>())
        };
        let Some(from) = from else {
            return;
        };
        button.add_css_class("picked");
        self.picked.replace(Some(Picked {
            page: page.to_owned(),
            button: button.clone(),
            from,
        }));
        self.sounds.chosen();
        self.update_arrange_hint();
    }

    /// Moves the picked tile one place along its row; it stays where it is at either end.
    fn move_picked(&self, delta: isize) {
        let picked = self.picked.borrow();
        let arrangeable = self.arrangeable.borrow();
        let Some((picked, tiles)) = picked
            .as_ref()
            .and_then(|picked| Some((picked, arrangeable.get(&picked.page)?)))
        else {
            return;
        };
        let row = children(&tiles.row);
        let Some(position) = row
            .iter()
            .position(|c| c == picked.button.upcast_ref::<gtk::Widget>())
        else {
            return;
        };
        if let Some(target) = order::step(position, delta, row.len()) {
            place(&tiles.row, &picked.button, target);
            self.sounds.moved();
        }
    }

    /// Puts the picked tile down where it is, and saves the order.
    fn drop_picked(&self) {
        let Some(picked) = self.picked.take() else {
            return;
        };
        picked.button.remove_css_class("picked");
        self.save_order(&picked.page);
        self.refresh_first(&picked.page);
        // Focus stays on the tile, which only changed places.
        picked.button.grab_focus();
        self.sounds.chosen();
        self.update_arrange_hint();
    }

    /// Puts the picked tile back where it was picked up.
    fn cancel_picked(&self) {
        let Some(picked) = self.picked.take() else {
            return;
        };
        picked.button.remove_css_class("picked");
        if let Some(tiles) = self.arrangeable.borrow().get(&picked.page) {
            place(&tiles.row, &picked.button, picked.from);
        }
        self.refresh_first(&picked.page);
        picked.button.grab_focus();
        self.sounds.chosen();
        self.update_arrange_hint();
    }

    /// Ends everything the arranging keys started: a tile picked up goes back, the
    /// arranging mode and a pending long press end.
    fn stop_arranging(&self) {
        self.cancel_picked();
        self.arranging.set(false);
        self.enter_down.set(false);
        if let Some(timer) = self.enter_timer.take() {
            timer.remove();
        }
        self.update_arrange_hint();
    }

    /// The tile order of the page, as the row shows it now.
    fn save_order(&self, page: &str) {
        let names: Vec<order::Identity> = {
            let arrangeable = self.arrangeable.borrow();
            let Some(tiles) = arrangeable.get(page) else {
                return;
            };
            let configured: Vec<_> = (0..tiles.buttons.len())
                .map(|index| self.tile(page, index).name.as_str())
                .collect();
            let identities = order::identities(&configured);
            children(&tiles.row)
                .iter()
                .filter_map(|child| {
                    tiles
                        .buttons
                        .iter()
                        .position(|button| button.upcast_ref::<gtk::Widget>() == child)
                })
                .map(|index| identities[index].clone())
                .collect()
        };
        let mut saved = self.saved_order.borrow_mut();
        saved.insert(page.to_owned(), names);
        order::save(&saved);
    }

    /// The first tile of a page is what is selected when it opens.
    fn refresh_first(&self, page: &str) {
        let first = self.arrangeable.borrow().get(page).and_then(|tiles| {
            tiles
                .row
                .first_child()
                .and_then(|child| child.downcast::<gtk::Button>().ok())
        });
        if let Some(first) = first {
            self.first_tiles.borrow_mut().insert(page.to_owned(), first);
        }
    }

    /// From the power menu: back to the main row, where Enter picks tiles up.
    fn start_arranging(&self) {
        self.main_row();
        self.arranging.set(true);
        self.update_arrange_hint();
    }

    /// Puts every row back in the configured order and forgets the saved one.
    fn reset_order(&self) {
        order::clear();
        self.saved_order.borrow_mut().clear();
        let pages: Vec<String> = self.arrangeable.borrow().keys().cloned().collect();
        for page in &pages {
            if let Some(tiles) = self.arrangeable.borrow().get(page) {
                for (index, button) in tiles.buttons.iter().enumerate() {
                    place(&tiles.row, button, index);
                }
            }
            self.refresh_first(page);
        }
        self.main_row();
        self.toasts.add_toast(adw::Toast::new("Tile order reset"));
    }

    fn update_arrange_hint(&self) {
        let text = if self.picked.borrow().is_some() {
            Some(PICKED_HINT)
        } else if self.arranging.get() {
            Some(ARRANGING_HINT)
        } else {
            None
        };
        if let Some(text) = text {
            self.arrange_hint.set_label(text);
        }
        self.arrange_hint.set_visible(text.is_some());
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
            None => match self.own_pages.iter().find(|own| own.key == page) {
                Some(own) => &own.tiles,
                None => &self.config.tiles,
            },
        };
        &tiles[index]
    }

    /// `page` is the page the tile is on: a shutdown or restart asks for
    /// confirmation first, so a stray key press cannot power the machine off.
    fn activate(self: &Rc<Self>, page: &str, tile: &Tile, button: &gtk::Button) {
        let confirming = page.starts_with(CONFIRM_PREFIX);
        let result = match &tile.action {
            Action::Submenu(name) => {
                self.open(&submenu_page(name), button);
                Ok(())
            }
            Action::Builtin(Builtin::DesktopMode) => {
                self.desktop_mode();
                Ok(())
            }
            Action::Builtin(
                builtin @ (Builtin::Shutdown | Builtin::Restart | Builtin::ResetOrder),
            ) if !confirming => {
                self.open(&confirm_page(*builtin), button);
                Ok(())
            }
            Action::Builtin(Builtin::Shutdown) => self.power(tile, "poweroff"),
            Action::Builtin(Builtin::Restart) => self.power(tile, "reboot"),
            Action::Builtin(Builtin::Arrange) => {
                self.start_arranging();
                Ok(())
            }
            Action::Builtin(Builtin::ResetOrder) => {
                self.reset_order();
                Ok(())
            }
            Action::Builtin(Builtin::Cancel) => {
                self.back();
                Ok(())
            }
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

    fn on_main_page(&self) -> bool {
        self.stack.visible_child_name().as_deref() == Some(MAIN_PAGE)
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

/// The children of `row`, left to right.
fn children(row: &gtk::Box) -> Vec<gtk::Widget> {
    let mut children = Vec::new();
    let mut child = row.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        children.push(widget);
    }
    children
}

/// Moves `button` to `index` in `row`, the others keeping their order.
fn place(row: &gtk::Box, button: &gtk::Button, index: usize) {
    let others: Vec<_> = children(row)
        .into_iter()
        .filter(|child| child != button.upcast_ref::<gtk::Widget>())
        .collect();
    row.reorder_child_after(button, index.checked_sub(1).and_then(|i| others.get(i)));
}

fn is_enter(key: gtk::gdk::Key) -> bool {
    use gtk::gdk::Key;
    matches!(key, Key::Return | Key::KP_Enter | Key::ISO_Enter)
}

/// Keys that move the selection from tile to tile.
fn is_navigation(key: gtk::gdk::Key) -> bool {
    use gtk::gdk::Key;
    matches!(
        key,
        Key::Left | Key::Right | Key::Up | Key::Down | Key::Tab | Key::ISO_Left_Tab
    )
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

/// A page the launcher adds itself: the power menu and its confirmations.
struct OwnPage {
    key: &'static str,
    title: &'static str,
    tiles: Vec<Tile>,
}

fn builtin_tile(name: &str, icon: &str, description: &str, builtin: Builtin) -> Tile {
    Tile {
        name: name.to_owned(),
        icon: icon.to_owned(),
        description: Some(description.to_owned()),
        action: Action::Builtin(builtin),
    }
}

/// The power menu, and a confirmation for each of its actions that powers the machine
/// off. Cancel comes first, so it is what is selected when a confirmation opens.
fn own_pages() -> Vec<OwnPage> {
    let cancel = || {
        builtin_tile(
            "Cancel",
            "window-close-symbolic",
            "Go back",
            Builtin::Cancel,
        )
    };
    vec![
        OwnPage {
            key: POWER_PAGE,
            title: "Power",
            tiles: vec![
                builtin_tile(
                    "Desktop Mode",
                    "video-display-symbolic",
                    "Hide the launcher and show the GNOME desktop",
                    Builtin::DesktopMode,
                ),
                builtin_tile(
                    "Arrange Tiles",
                    "view-list-symbolic",
                    "Pick tiles up and move them",
                    Builtin::Arrange,
                ),
                builtin_tile(
                    "Reset Tile Order",
                    "edit-undo-symbolic",
                    "Put the tiles back in the configured order",
                    Builtin::ResetOrder,
                ),
                builtin_tile(
                    "Restart",
                    "system-reboot-symbolic",
                    "Restart the HTPC",
                    Builtin::Restart,
                ),
                builtin_tile(
                    "Shutdown",
                    "system-shutdown-symbolic",
                    "Turn the HTPC off",
                    Builtin::Shutdown,
                ),
            ],
        },
        OwnPage {
            key: "confirm:reset",
            title: "Reset tile order?",
            tiles: vec![
                cancel(),
                builtin_tile(
                    "Reset",
                    "edit-undo-symbolic",
                    "Use the configured tile order again",
                    Builtin::ResetOrder,
                ),
            ],
        },
        OwnPage {
            key: "confirm:shutdown",
            title: "Shut down?",
            tiles: vec![
                cancel(),
                builtin_tile(
                    "Shut down",
                    "system-shutdown-symbolic",
                    "Turn the HTPC off now",
                    Builtin::Shutdown,
                ),
            ],
        },
        OwnPage {
            key: "confirm:restart",
            title: "Restart?",
            tiles: vec![
                cancel(),
                builtin_tile(
                    "Restart",
                    "system-reboot-symbolic",
                    "Restart the HTPC now",
                    Builtin::Restart,
                ),
            ],
        },
    ]
}

/// The page asking whether to go ahead with `builtin`: a shutdown, restart or reset of
/// the tile order.
fn confirm_page(builtin: Builtin) -> String {
    let action = match builtin {
        Builtin::Restart => "restart",
        Builtin::ResetOrder => "reset",
        _ => "shutdown",
    };
    format!("{CONFIRM_PREFIX}{action}")
}

/// Top-right, next to the clock: the way into the power menu.
fn power_button() -> gtk::Button {
    let icon = gtk::Image::from_icon_name("system-shutdown-symbolic");
    icon.set_pixel_size(32);
    let button = gtk::Button::builder()
        .child(&icon)
        .css_classes(["power-button"])
        .valign(gtk::Align::Start)
        .margin_top(8)
        .build();
    button.update_property(&[gtk::accessible::Property::Label("Power")]);
    button
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
    use super::{
        confirm_page, is_navigation, own_pages, tile_size, CloseQueue, CONFIRM_PREFIX, TILE_MAX,
        TILE_MIN,
    };
    use crate::config::{Action, Builtin};
    use gtk::gdk::Key;

    #[test]
    fn only_the_keys_that_move_the_selection_are_navigation() {
        for key in [Key::Left, Key::Right, Key::Up, Key::Down, Key::Tab] {
            assert!(is_navigation(key));
        }
        for key in [Key::Return, Key::Escape, Key::BackSpace, Key::F10] {
            assert!(!is_navigation(key));
        }
    }

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
    fn the_power_menu_offers_desktop_mode_arranging_reset_restart_and_shutdown() {
        let pages = own_pages();
        let power = pages
            .iter()
            .find(|page| page.key == super::POWER_PAGE)
            .unwrap();
        let actions: Vec<_> = power.tiles.iter().map(|tile| &tile.action).collect();
        assert_eq!(
            actions,
            [
                &Action::Builtin(Builtin::DesktopMode),
                &Action::Builtin(Builtin::Arrange),
                &Action::Builtin(Builtin::ResetOrder),
                &Action::Builtin(Builtin::Restart),
                &Action::Builtin(Builtin::Shutdown),
            ]
        );
    }

    #[test]
    fn every_power_off_or_reset_action_has_a_confirmation_that_selects_cancel_first() {
        let pages = own_pages();
        for builtin in [Builtin::Shutdown, Builtin::Restart, Builtin::ResetOrder] {
            let key = confirm_page(builtin);
            assert!(key.starts_with(CONFIRM_PREFIX));
            let page = pages.iter().find(|page| page.key == key).unwrap();
            assert_eq!(page.tiles[0].action, Action::Builtin(Builtin::Cancel));
            assert_eq!(page.tiles[1].action, Action::Builtin(builtin));
            assert_eq!(page.tiles.len(), 2);
        }
    }

    #[test]
    fn desktop_mode_cancels_the_pending_request() {
        let queue = CloseQueue::new();
        assert!(queue.submit("brave"));
        queue.cancel();
        assert_eq!(queue.finish(), None);
    }
}
