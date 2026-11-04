//! The launcher window: wallpaper, clock, and one page per tile row (the main row and
//! each submenu), with keyboard and mouse navigation. Also theater and desktop mode:
//! the launcher is shown full screen, or hidden to leave the GNOME desktop.

use crate::catalog;
use crate::clock::Clock;
use crate::config::{Action, Builtin, Config, Tile};
use crate::diagnose;
use crate::grid::{self, TileGrid, ICON_SIZE, PAGE_MARGIN, TILE_MIN};
use crate::keybinding;
use crate::mode::{self, Mode};
use crate::sound::Sounds;
use crate::tiles::{self, Addable};
use crate::widgets::{hint, label};
use crate::{launch, order, webapp};
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

const LOGO_WIDTH: i32 = 200;
const POINTER_IDLE: Duration = Duration::from_secs(3);
const QUICK_FAILURE: Duration = Duration::from_secs(5);
/// Holding Enter on a tile this long opens its options.
const LONG_PRESS: Duration = Duration::from_millis(600);
/// Shown when the previous app was still running at the time limit.
const NOT_CLOSED: &str = "Couldn't close the previous app";
const PICKED_HINT: &str = "← → ↑ ↓  move  ·  Enter  drop  ·  Back  cancel";
/// Holding Back on the main row this long asks to reset the tile order.
const RESET_HOLD: Duration = Duration::from_secs(2);

/// What choosing a tile does. A configured tile does what its action says; the rest are
/// the launcher's own, which a configuration cannot ask for.
#[derive(Clone, Debug, PartialEq)]
enum Command {
    Config(Action),
    /// Leaves a confirmation page.
    Cancel,
    /// Puts the tiles back in the configured order. Its confirmation has it.
    ResetOrder,
    /// Opens a service of the catalog as a web app.
    Web(&'static catalog::App),
    /// Puts something on the launcher; a choice on the Add page.
    Add(Addable),
    /// Picks the tile of the options page up, to move it.
    Move,
    /// Takes the tile of the options page off the launcher. Its confirmation has it.
    Remove,
}

impl Command {
    /// Whether the app behind it is installed; only a configured app, or a web app
    /// without a browser (`browser` is the configured one, if any), can be missing.
    fn is_installed(&self, browser: Option<&str>) -> bool {
        match self {
            Command::Config(action) => launch::is_installed(action),
            Command::Web(_) => webapp::find_browser(browser).is_some(),
            Command::Cancel
            | Command::ResetOrder
            | Command::Add(_)
            | Command::Move
            | Command::Remove => true,
        }
    }
}

/// A tile as the launcher shows it: one from the configuration, or one of its own pages.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    name: String,
    /// An icon-theme name or an image file; see `config::Tile::icon`.
    icon: Option<String>,
    description: Option<String>,
    command: Command,
}

impl From<&Tile> for Entry {
    fn from(tile: &Tile) -> Entry {
        Entry {
            name: tile.name.clone(),
            icon: tile.icon.clone(),
            description: tile.description.clone(),
            command: Command::Config(tile.action.clone()),
        }
    }
}

/// A page of the launcher. Each has one name, [`Page::key`], which is its child name in
/// the stack and, for the pages whose tiles can be arranged, its key in `order.json`.
/// Existing files hold those strings, so they must not change.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Page {
    Main,
    /// The tiles a `submenu` tile opens, by the submenu's name.
    Submenu(String),
    Power,
    Confirm(Confirmable),
    /// What can be put on the launcher; made again each time it opens.
    Add,
    /// Move or remove the tile `Ui::options_for` names.
    TileOptions,
}

/// What a confirmation page asks about: the actions that power the machine off or
/// discard something, which are never taken on a single key press.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Confirmable {
    Shutdown,
    Restart,
    ResetOrder,
    RemoveTile,
}

impl Confirmable {
    /// The action `command` asks to confirm, if it is one that needs it.
    fn of(command: &Command) -> Option<Confirmable> {
        match command {
            Command::Config(Action::Builtin(Builtin::Shutdown)) => Some(Confirmable::Shutdown),
            Command::Config(Action::Builtin(Builtin::Restart)) => Some(Confirmable::Restart),
            Command::ResetOrder => Some(Confirmable::ResetOrder),
            Command::Remove => Some(Confirmable::RemoveTile),
            Command::Config(
                Action::Builtin(Builtin::DesktopMode)
                | Action::Command(_)
                | Action::Desktop(_)
                | Action::Flatpak(_)
                | Action::Submenu(_),
            )
            | Command::Cancel
            | Command::Web(_)
            | Command::Add(_)
            | Command::Move => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Confirmable::Shutdown => "shutdown",
            Confirmable::Restart => "restart",
            Confirmable::ResetOrder => "reset",
            Confirmable::RemoveTile => "remove",
        }
    }
}

impl Page {
    const MAIN: &'static str = "main";
    const POWER: &'static str = "power";
    const ADD: &'static str = "add";
    const TILE_OPTIONS: &'static str = "options";
    const SUBMENU_PREFIX: &'static str = "submenu:";
    const CONFIRM_PREFIX: &'static str = "confirm:";

    fn key(&self) -> String {
        match self {
            Page::Main => Self::MAIN.to_owned(),
            Page::Submenu(name) => format!("{}{name}", Self::SUBMENU_PREFIX),
            Page::Power => Self::POWER.to_owned(),
            Page::Add => Self::ADD.to_owned(),
            Page::TileOptions => Self::TILE_OPTIONS.to_owned(),
            Page::Confirm(confirm) => format!("{}{}", Self::CONFIRM_PREFIX, confirm.key()),
        }
    }

    /// The page named `key`, if there is one.
    fn from_key(key: &str) -> Option<Page> {
        if let Some(name) = key.strip_prefix(Self::SUBMENU_PREFIX) {
            return Some(Page::Submenu(name.to_owned()));
        }
        if let Some(action) = key.strip_prefix(Self::CONFIRM_PREFIX) {
            return [
                Confirmable::Shutdown,
                Confirmable::Restart,
                Confirmable::ResetOrder,
                Confirmable::RemoveTile,
            ]
            .into_iter()
            .find(|confirm| confirm.key() == action)
            .map(Page::Confirm);
        }
        match key {
            Self::MAIN => Some(Page::Main),
            Self::POWER => Some(Page::Power),
            Self::ADD => Some(Page::Add),
            Self::TILE_OPTIONS => Some(Page::TileOptions),
            _ => None,
        }
    }
}

thread_local! {
    static UI: RefCell<Option<Rc<Ui>>> = const { RefCell::new(None) };
}

/// Handles `vextheater` and its flags, run here or forwarded from another process to
/// this, the running instance.
pub fn command_line(
    application: &adw::Application,
    command_line: &gio::ApplicationCommandLine,
) -> glib::ExitCode {
    let arguments = command_line.arguments();
    let flags: Vec<_> = arguments
        .iter()
        .skip(1)
        .map(|a| a.to_string_lossy())
        .collect();
    match flags.as_slice() {
        [] => {
            activate(application);
            glib::ExitCode::SUCCESS
        }
        // What the autostart service runs: like no flag, but a launcher restarted
        // after a crash in desktop mode stays in desktop mode.
        [flag] if flag == "--resume" => {
            if mode::saved() == Some(Mode::Desktop) {
                ui(application);
            } else {
                activate(application);
            }
            glib::ExitCode::SUCCESS
        }
        [flag] if flag == "--home" => {
            if let Some(ui) = ui(application) {
                ui.home();
            }
            glib::ExitCode::SUCCESS
        }
        [flag] if flag == "--desktop" => {
            if let Some(ui) = ui(application) {
                ui.desktop_mode();
            }
            glib::ExitCode::SUCCESS
        }
        // Reports on the environment of this, the running instance, which is where a
        // service differs from a shell.
        [flag] if flag == "--diagnose" => {
            let ui = ui(application);
            let config = ui.as_ref().map(|ui| ui.config.borrow());
            let (report, all_ok) = diagnose::run(config.as_deref());
            command_line.print_literal(&report);
            glib::ExitCode::from(u8::from(!all_ok))
        }
        [flag] if flag == "--reload" => {
            if let Some(ui) = ui(application) {
                ui.reload(true);
            }
            glib::ExitCode::SUCCESS
        }
        // Theater mode if the launcher is hidden, desktop mode if it is shown, so one
        // key can switch between them.
        [flag] if flag == "--toggle" => {
            if let Some(ui) = ui(application) {
                if ui.window.is_visible() {
                    ui.desktop_mode();
                } else {
                    ui.show();
                }
            }
            glib::ExitCode::SUCCESS
        }
        _ => {
            command_line.printerr_literal(
                "usage: vextheater [--home | --desktop | --toggle | --reload | --diagnose | --resume | --check-config [FILE] | --print-order]\n  \
                 --home    close the open app and show the launcher\n  \
                 --desktop hide the launcher and show the GNOME desktop\n  \
                 --toggle  show the launcher if it is hidden, else hide it\n  \
                 --reload  read the configuration again and rebuild the tiles\n  \
                 --diagnose  check what the launcher needs from its session\n  \
                 --resume  show the launcher, unless it crashed in desktop mode\n  \
                 --check-config [FILE]  check a configuration file and exit (first argument)\n  \
                 --print-order  print the tiles in the order shown and exit (first argument)\n",
            );
            glib::ExitCode::from(2)
        }
    }
}

/// The tiles of the main row and of each submenu, in the order the launcher shows them:
/// the arrangement in `saved` over the configured order, with the services that were
/// added and without the tiles that were removed (`shown`). One page per heading, one
/// tile per line.
pub fn order_listing(config: &Config, saved: &crate::order::Saved, shown: &tiles::Shown) -> String {
    let mut listing = String::new();
    let main = (Page::Main, "main".to_owned(), &config.tiles);
    let submenus = config.submenus.iter().map(|(name, tiles)| {
        (
            Page::Submenu(name.clone()),
            format!("submenu {name}"),
            tiles,
        )
    });
    for (page, heading, page_tiles) in std::iter::once(main).chain(submenus) {
        let configured: Vec<&str> = page_tiles.iter().map(|tile| tile.name.as_str()).collect();
        let key = page.key();
        let listed = tiles::listed(&key, &configured, shown);
        let names: Vec<&str> = listed.iter().map(|(_, name)| name.as_str()).collect();
        let order = tiles::visible(&key, &names, shown, saved.get(&key));
        listing.push_str(&format!("{heading}:\n"));
        for (position, index) in order.into_iter().enumerate() {
            listing.push_str(&format!("  {}. {}\n", position + 1, names[index]));
        }
    }
    listing
}

/// Shows the launcher (theater mode), e.g. from "Theater Mode" in the app grid.
pub fn activate(application: &adw::Application) {
    if let Some(ui) = ui(application) {
        ui.show();
    }
}

/// The launcher, built on first use. None when the configuration cannot be read; the
/// window then shows the error instead, and every later call reads the configuration
/// again, so fixing the file and asking again brings the launcher up.
fn ui(application: &adw::Application) -> Option<Rc<Ui>> {
    if let Some(ui) = UI.with_borrow(Clone::clone) {
        return Some(ui);
    }
    // Only the error page leaves a window without a `Ui`, so a window already there
    // means the configuration is being read again after an error.
    let earlier_error = application
        .windows()
        .first()
        .and_then(|window| window.downcast_ref::<adw::ApplicationWindow>().cloned());
    let retried = earlier_error.is_some();
    let window = earlier_error.unwrap_or_else(|| {
        adw::ApplicationWindow::builder()
            .application(application)
            .title("VexTheater")
            .build()
    });

    match Config::load_default() {
        Ok(config) => {
            let ui = Ui::build(&window, config);
            UI.set(Some(ui.clone()));
            Some(ui)
        }
        Err(error) => {
            log::error!("{error:#}");
            show_error(application, &window, &error, retried);
            None
        }
    }
}

/// The configuration error, with a way out: try again, or leave for the desktop. A
/// `retried` error says it is still there and when it was last checked, so a retry that
/// changes nothing can be told from one that did not run.
fn show_error(
    application: &adw::Application,
    window: &adw::ApplicationWindow,
    error: &anyhow::Error,
    retried: bool,
) {
    let retry = gtk::Button::builder()
        .label("Retry")
        .css_classes(["pill", "suggested-action"])
        .build();
    retry.connect_clicked({
        let application = application.clone();
        move |_| activate(&application)
    });
    let desktop = gtk::Button::builder()
        .label("Desktop Mode")
        .css_classes(["pill"])
        .build();
    desktop.connect_clicked({
        let window = window.clone();
        move |_| {
            mode::enter_desktop();
            window.set_visible(false);
        }
    });
    let buttons = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(24)
        .halign(gtk::Align::Center)
        .build();
    buttons.append(&retry);
    buttons.append(&desktop);

    let page = adw::StatusPage::builder()
        .icon_name("dialog-error-symbolic")
        .title("Configuration error")
        // The description is markup, and tile names and paths can hold `&` or `<`.
        .description(glib::markup_escape_text(&if retried {
            let checked = glib::DateTime::now_local()
                .and_then(|now| now.format("%H:%M:%S"))
                .map(|time| format!(" (checked {time})"))
                .unwrap_or_default();
            format!("Still invalid{checked}:\n{error:#}")
        } else {
            format!("{error:#}")
        }))
        .child(&buttons)
        .build();
    // On the page, not the window: each failed retry replaces the page and its controllers.
    hide_idle_pointer(&page, |_| {});
    // GTK buttons ignore a remote's select key, which is how the buttons are reached.
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed({
        let window = window.clone();
        move |_, key, _, _| {
            if !is_select(key) {
                return glib::Propagation::Proceed;
            }
            if let Some(focus) = GtkWindowExt::focus(&window) {
                focus.activate();
            }
            glib::Propagation::Stop
        }
    });
    page.add_controller(keys);
    window.set_content(Some(&page));
    window.fullscreen();
    window.present();
    retry.grab_focus();
}

/// What to do once the open app is closed.
enum Request {
    /// Stay on the launcher.
    Home,
    /// Hide the launcher for desktop mode.
    Desktop,
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

    /// Whether a close is running.
    fn is_busy(&self) -> bool {
        self.closing.get()
    }
}

/// The tiles of a page the user can rearrange: the main row and the configured submenus.
struct Arrangeable {
    grid: Rc<TileGrid>,
    /// Each shown tile with its index for `Ui::tile`, in that order. A tile that was
    /// removed has no button.
    buttons: Vec<(usize, gtk::Button)>,
}

/// The tile picked up for moving.
struct Picked {
    page: Page,
    button: gtk::Button,
    /// Its place among the tiles when picked up, to put it back on cancel.
    from: usize,
}

struct Ui {
    /// Replaced when the configuration is reloaded, as are the tiles made from it.
    config: RefCell<Config>,
    /// The tiles of the main row and of each configured submenu.
    main: RefCell<Vec<Entry>>,
    submenus: RefCell<BTreeMap<String, Vec<Entry>>>,
    /// Holds the background, which a reload may change.
    overlay: gtk::Overlay,
    /// The size the tiles were last fitted to, for the pages of a reload.
    width: Cell<i32>,
    height: Cell<i32>,
    /// What a reload on showing the launcher last complained about, so a file that
    /// stays broken is reported once.
    reload_error: RefCell<Option<String>>,
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    /// The app started from a tile, if it is still running.
    running: RefCell<Option<launch::Running>>,
    /// The main-row tile that led to `running`: the tile itself, or the one that opens
    /// the submenu it is in. Selected when the launcher comes back; kept in memory only.
    origin: RefCell<Option<glib::WeakRef<gtk::Button>>>,
    /// What the user added to the launcher and removed from it.
    shown: RefCell<tiles::Shown>,
    /// The round buttons beside the clock; "+" is selected when the main row has no
    /// tiles.
    plus_button: gtk::Button,
    power_button: gtk::Button,
    /// The tile the options page is about: its page, and its index for `Ui::tile`.
    options_for: RefCell<Option<(Page, usize)>>,
    /// The tiles of the Add page, made each time it opens.
    add_entries: RefCell<Vec<Entry>>,
    /// The title label of each page that has one, for the pages whose title changes.
    titles: RefCell<HashMap<Page, gtk::Label>>,
    /// Serialises closes; see `close_then`.
    closes: CloseQueue<Request>,
    stack: gtk::Stack,
    /// Pages the launcher adds itself, beside the configured ones.
    own_pages: Vec<OwnPage>,
    /// First tile of each page, focused when the page opens.
    first_tiles: RefCell<HashMap<Page, gtk::Button>>,
    /// The name and description labels under each page's row.
    labels: RefCell<HashMap<Page, (gtk::Label, gtk::Label)>>,
    /// Tiles (page and configured index) whose app was not installed when last checked.
    missing: RefCell<HashSet<(Page, usize)>>,
    /// The tiles of each page, resized to the window by `fit_tiles`.
    rows: RefCell<Vec<Rc<TileGrid>>>,
    /// Pages left by opening a submenu, with the tile that opened each.
    history: RefCell<Vec<(Page, gtk::Button)>>,
    /// Hover selects a tile only while the pointer is shown, so a tile moving under a
    /// hidden pointer cannot steal the keyboard's selection.
    pointer_visible: Cell<bool>,
    sounds: Sounds,
    /// A navigation key or the pointer is moving the selection. Selection also moves
    /// when a page opens or the window is shown again, and those stay silent.
    navigating: Cell<bool>,
    clock: Clock,
    arrangeable: RefCell<HashMap<Page, Arrangeable>>,
    /// The arranged order, as saved to disk.
    saved_order: RefCell<order::Saved>,
    picked: RefCell<Option<Picked>>,
    /// Enter is held down. Keyboard auto-repeat sends more presses while it is, and
    /// they must not pick a tile up or drop it again.
    enter_down: Cell<bool>,
    /// Counts down a long press of Enter on a tile.
    enter_timer: RefCell<Option<glib::SourceId>>,
    /// A long press of Enter opened the options; the rest of the press, its keyboard
    /// auto-repeat, must not reach the page that opened.
    enter_swallow: Cell<bool>,
    /// An Enter press is under way (down since its first press, auto-repeat included),
    /// to tell the repeat of a press that something else dealt with, which must not
    /// act on the page that press opened, from a new press.
    enter_held: Cell<bool>,
    /// Back is held down on the main row; like `enter_down`, so auto-repeat does not
    /// start the timer again.
    back_down: Cell<bool>,
    /// Counts down a long press of Back on the main row.
    back_timer: RefCell<Option<glib::SourceId>>,
    /// What the keys do while a tile is picked up.
    arrange_hint: gtk::Label,
}

impl Ui {
    fn build(window: &adw::ApplicationWindow, config: Config) -> Rc<Self> {
        let overlay = gtk::Overlay::new();
        overlay.add_css_class("theater");

        let background = background_widget(config.background.as_deref());
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
        let plus_button = plus_button();
        let corner = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(40)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(64)
            .margin_end(PAGE_MARGIN)
            .build();
        corner.append(&plus_button);
        corner.append(&power_button);
        corner.append(&clock.widget);
        overlay.add_overlay(&corner);

        let arrange_hint = label("arrange-hint");
        arrange_hint.set_halign(gtk::Align::Center);
        arrange_hint.set_valign(gtk::Align::Start);
        // In the header, between the greeting and the buttons: the tiles may fill the
        // rest of the screen.
        arrange_hint.set_margin_top(84);
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

        let shown = tiles::Shown::load();
        let main = main_entries(&config, &shown);
        let submenus = submenu_entries(&config);
        let ui = Rc::new(Ui {
            config: RefCell::new(config),
            main: RefCell::new(main),
            submenus: RefCell::new(submenus),
            overlay: overlay.clone(),
            width: Cell::new(0),
            height: Cell::new(0),
            reload_error: RefCell::default(),
            window: window.clone(),
            toasts,
            running: RefCell::default(),
            origin: RefCell::default(),
            shown: RefCell::new(shown),
            plus_button: plus_button.clone(),
            power_button: power_button.clone(),
            options_for: RefCell::default(),
            add_entries: RefCell::default(),
            titles: RefCell::default(),
            closes: CloseQueue::new(),
            stack,
            own_pages: own_pages(),
            first_tiles: RefCell::default(),
            labels: RefCell::default(),
            missing: RefCell::default(),
            rows: RefCell::default(),
            history: RefCell::default(),
            pointer_visible: Cell::new(false),
            sounds: Sounds::new(),
            navigating: Cell::new(false),
            clock,
            arrangeable: RefCell::default(),
            saved_order: RefCell::new(order::load()),
            picked: RefCell::default(),
            enter_down: Cell::new(false),
            enter_timer: RefCell::default(),
            enter_swallow: Cell::new(false),
            enter_held: Cell::new(false),
            back_down: Cell::new(false),
            back_timer: RefCell::default(),
            arrange_hint,
        });

        ui.add_config_pages();
        for page in &ui.own_pages {
            ui.add_page(page.page.clone(), Some(page.title), &page.tiles, false);
        }
        ui.clock.start();

        power_button.connect_clicked({
            let ui = ui.clone();
            move |button| {
                ui.sounds.chosen();
                // Arranging belongs to the rows: leaving for the power menu ends it, and
                // puts a tile that is picked up back, so it does not come back with it.
                ui.stop_arranging();
                ui.open(Page::Power, button);
            }
        });
        plus_button.connect_clicked({
            let ui = ui.clone();
            move |button| {
                ui.sounds.chosen();
                ui.stop_arranging();
                ui.open_add(button);
            }
        });
        // The round buttons belong to the main row only.
        ui.stack.connect_visible_child_name_notify({
            let (ui, power_button, plus_button) =
                (ui.clone(), power_button.clone(), plus_button.clone());
            move |stack| {
                // The hint says what Back does, which depends on the page.
                ui.update_arrange_hint();
                let on_main = stack
                    .visible_child_name()
                    .and_then(|name| Page::from_key(&name))
                    == Some(Page::Main);
                power_button.set_visible(on_main);
                plus_button.set_visible(on_main);
            }
        });

        // GTK's arrow keys do not reach the buttons from the tiles, so Up and Down
        // cross between them here: Up from the top row selects "+", Left and Right
        // move between it and the power button. Down goes back to the tile Up left.
        let keys = gtk::EventControllerKey::new();
        let last_tile: Rc<RefCell<Option<gtk::Widget>>> = Rc::default();
        keys.connect_key_pressed({
            let ui = ui.clone();
            move |_, key, _, _| match key {
                key if is_back(key) && ui.back() => {
                    ui.sounds.chosen();
                    glib::Propagation::Stop
                }
                // GTK buttons activate on Enter and space, not on a remote's select key.
                key if is_select(key) => {
                    if let Some(focus) = GtkWindowExt::focus(&ui.window) {
                        focus.activate();
                    }
                    glib::Propagation::Stop
                }
                // A remote's home button, when GNOME leaves it to the launcher.
                gtk::gdk::Key::HomePage => {
                    ui.main_row();
                    ui.sounds.chosen();
                    glib::Propagation::Stop
                }
                // From the top row only: Up in a lower row moves up a row.
                gtk::gdk::Key::Up
                    if ui.on_main_page()
                        && !power_button.is_focus()
                        && !plus_button.is_focus()
                        && ui.in_top_row() =>
                {
                    last_tile.replace(GtkWindowExt::focus(&ui.window));
                    plus_button.grab_focus();
                    ui.sounds.moved();
                    glib::Propagation::Stop
                }
                gtk::gdk::Key::Down if power_button.is_focus() || plus_button.is_focus() => {
                    let first = ui.first_tiles.borrow().get(&Page::Main).cloned();
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
                } else if is_back(key) {
                    ui.back_released();
                }
            }
        });
        window.add_controller(arranging);

        // The window is as big as the screen, whatever its resolution: the tiles are
        // sized to that once the compositor reports it.
        window.connect_realize({
            let ui = ui.clone();
            move |window| {
                if let Some(surface) = window.surface() {
                    let ui = ui.clone();
                    surface.connect_layout(move |_, width, height| ui.fit_tiles(width, height));
                }
            }
        });

        // An app that takes the focus while Enter is down gets its release, not the
        // launcher: forget the press, or the next one would be taken for a repeat.
        window.connect_is_active_notify({
            let ui = ui.clone();
            move |window| {
                if !window.is_active() {
                    ui.enter_held.set(false);
                    ui.enter_swallow.set(false);
                    ui.enter_down.set(false);
                    if let Some(timer) = ui.enter_timer.take() {
                        timer.remove();
                    }
                }
            }
        });

        hide_idle_pointer(window, {
            let ui = ui.clone();
            move |visible| ui.pointer_visible.set(visible)
        });
        window.set_content(Some(&ui.toasts));
        keybinding::sync(ui.config.borrow().home_key.as_deref());
        let first = ui.first_tiles.borrow().get(&Page::Main).cloned();
        GtkWindowExt::set_focus(window, first.as_ref().or(Some(&ui.plus_button)));
        ui
    }

    /// Reads the configuration again and, if it changed, rebuilds the pages made from
    /// it. Nothing else changes: the open app, the close queue and the toasts stay, and
    /// a configuration that cannot be read leaves the launcher as it is. `announce` is
    /// for a reload asked for, which says so when there was nothing to do; one on
    /// showing the launcher says something only when it changed something or found the
    /// file broken (once, so a file that stays broken is not reported at every show).
    fn reload(self: &Rc<Self>, announce: bool) {
        if self.closes.is_busy() || self.picked.borrow().is_some() {
            if announce {
                self.notify("Not reloaded now: an app is closing or a tile is picked up");
            }
            return;
        }
        match Config::load_default() {
            Ok(config) => {
                self.reload_error.take();
                if config == *self.config.borrow() {
                    if announce {
                        self.notify("The configuration has not changed");
                    }
                    return;
                }
                self.rebuild(Some(config));
                self.notify("Configuration reloaded");
            }
            Err(error) => {
                log::error!("{error:#}");
                let reason = error.root_cause().to_string();
                let new = self.reload_error.replace(Some(reason.clone())).as_ref() != Some(&reason);
                if announce || new {
                    self.notify(&format!(
                        "Not reloaded, the configuration is invalid: {reason}"
                    ));
                }
            }
        }
    }

    /// Replaces the pages made from the configuration with ones made from `config`. The
    /// other pages and everything about the open app stay.
    fn rebuild(self: &Rc<Self>, config: Option<Config>) {
        // Leaves any submenu and arranging first, so no stale button is left selected.
        self.main_row();
        let transition = self.stack.transition_type();
        self.stack
            .set_transition_type(gtk::StackTransitionType::None);

        let from_config = |page: &Page| matches!(page, Page::Main | Page::Submenu(_));
        let mut stale = Vec::new();
        let mut child = self.stack.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            let page = self
                .stack
                .page(&widget)
                .name()
                .and_then(|name| Page::from_key(&name));
            if page.is_some_and(|page| from_config(&page)) {
                stale.push(widget);
            }
        }
        // A page that stays is shown while the others are replaced.
        self.stack.set_visible_child_name(&Page::Power.key());
        for widget in stale {
            self.stack.remove(&widget);
        }
        self.first_tiles
            .borrow_mut()
            .retain(|page, _| !from_config(page));
        self.labels
            .borrow_mut()
            .retain(|page, _| !from_config(page));
        self.missing
            .borrow_mut()
            .retain(|(page, _)| !from_config(page));
        self.titles
            .borrow_mut()
            .retain(|page, _| !from_config(page));
        self.arrangeable
            .borrow_mut()
            .retain(|page, _| !from_config(page));
        self.rows
            .borrow_mut()
            .retain(|row| row.widget().root().is_some());

        if let Some(config) = config {
            let home_key_changed = config.home_key != self.config.borrow().home_key;
            if config.background != self.config.borrow().background {
                let background = background_widget(config.background.as_deref());
                self.overlay.set_child(Some(&background));
            }
            *self.config.borrow_mut() = config;
            if home_key_changed {
                keybinding::sync(self.config.borrow().home_key.as_deref());
            }
        }
        *self.main.borrow_mut() = main_entries(&self.config.borrow(), &self.shown.borrow());
        *self.submenus.borrow_mut() = submenu_entries(&self.config.borrow());

        self.add_config_pages();
        if self.width.get() > 0 {
            self.fit_tiles(self.width.get(), self.height.get());
        }
        self.stack.set_visible_child_name(&Page::Main.key());
        self.stack.set_transition_type(transition);
        self.main_row();
    }

    /// Theater mode: the launcher full screen over the desktop.
    fn show(self: &Rc<Self>) {
        mode::enter_theater();
        self.clock.refresh();
        self.reload(false);
        self.refresh_installed();
        self.window.fullscreen();
        self.window.present();
    }

    /// Closes the app started from a tile, or left over from a launcher that crashed,
    /// and returns to the main row.
    fn home(self: &Rc<Self>) {
        self.close_then(Request::Home);
        self.main_row_at_origin();
        self.show();
    }

    /// Hides the launcher. During a close that is still running it waits for the close,
    /// replacing any pending tile, so the app being closed is not uncovered and no app
    /// starts over the desktop once the close finishes.
    fn desktop_mode(&self) {
        // Back on the main row when the launcher comes back, not in the power menu.
        self.main_row();
        if self.closes.is_busy() {
            self.closes.submit(Request::Desktop);
            return;
        }
        self.hide_for_desktop();
    }

    fn hide_for_desktop(&self) {
        mode::enter_desktop();
        self.window.set_visible(false);
    }

    /// Leaves every submenu and selects the first tile, not the power button a
    /// submenu may have been opened from. With no tiles, the power button.
    fn main_row(&self) {
        self.stop_arranging();
        while self.back() {}
        match self.first_tiles.borrow().get(&Page::Main) {
            Some(first) => first.grab_focus(),
            None => self.plus_button.grab_focus(),
        };
    }

    /// Like [`Ui::main_row`], but selects the tile that led to the app that was open,
    /// if there was one; the app is gone or going, so it is forgotten.
    fn main_row_at_origin(&self) {
        self.main_row();
        let origin = self.origin.take().and_then(|origin| origin.upgrade());
        if let Some(button) = origin {
            button.grab_focus();
        }
    }

    /// Notes which main-row tile `button`, a tile of `page`, belongs to, for
    /// [`Ui::main_row_at_origin`]. The first submenu left is the one that was opened
    /// from the main row.
    fn remember_origin(&self, page: &Page, button: &gtk::Button) {
        let origin = if *page == Page::Main {
            Some(button.clone())
        } else {
            self.history
                .borrow()
                .first()
                .map(|(_, opener)| opener.clone())
        };
        self.origin.replace(origin.map(|button| button.downgrade()));
    }

    /// The pages made from the configuration: the main row and each submenu.
    fn add_config_pages(self: &Rc<Self>) {
        let main = self.main.borrow().clone();
        self.add_page(Page::Main, None, &main, true);
        let submenus = self.submenus.borrow().clone();
        for (name, tiles) in &submenus {
            self.add_page(Page::Submenu(name.clone()), Some(name), tiles, true);
        }
    }

    /// One row of tiles with its header, selected-tile title and key hints. The tiles of
    /// an `arrangeable` page are shown in the order the user saved for it.
    fn add_page(
        self: &Rc<Self>,
        page: Page,
        submenu: Option<&str>,
        tiles: &[Entry],
        arrangeable: bool,
    ) {
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_top(64)
            .margin_bottom(52)
            .margin_start(PAGE_MARGIN)
            .margin_end(PAGE_MARGIN)
            .build();

        column.append(&self.header(&page, submenu));

        let row = TileGrid::new(&column);
        column.append(row.widget());

        let positions: Vec<usize> = if arrangeable {
            let names: Vec<&str> = tiles.iter().map(|tile| tile.name.as_str()).collect();
            tiles::visible(
                &page.key(),
                &names,
                &self.shown.borrow(),
                self.saved_order.borrow().get(&page.key()),
            )
        } else {
            (0..tiles.len()).collect()
        };

        let name = label("tile-name");
        name.set_margin_top(44);
        let description = label("tile-description");
        description.set_margin_top(10);
        if positions.is_empty() {
            if let Some((title, detail)) = empty_page_text(&page) {
                name.set_label(title);
                description.set_label(detail);
            }
        }
        self.labels
            .borrow_mut()
            .insert(page.clone(), (name.clone(), description.clone()));
        column.append(&name);
        column.append(&description);
        column.append(&self.hints(submenu.is_some(), !positions.is_empty()));

        let mut buttons = Vec::new();
        for (position, index) in positions.into_iter().enumerate() {
            let tile = &tiles[index];
            let button = tile_button(tile);
            self.check_installed(&page, index, &button);
            if position == 0 {
                self.first_tiles
                    .borrow_mut()
                    .insert(page.clone(), button.clone());
                self.show_tile(&page, index);
            }

            let focus = gtk::EventControllerFocus::new();
            focus.connect_enter({
                let (ui, page, index) = (self.clone(), page.clone(), index);
                move |_| {
                    ui.show_tile(&page, index);
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

            // A right click opens the tile's options, as Menu does.
            if arrangeable {
                let secondary = gtk::GestureClick::builder()
                    .button(gtk::gdk::BUTTON_SECONDARY)
                    .build();
                secondary.connect_pressed({
                    let (ui, page, button) = (self.clone(), page.clone(), button.downgrade());
                    move |_, _, _, _| {
                        let Some(button) = button.upgrade() else {
                            return;
                        };
                        if ui.picked.borrow().is_none() {
                            button.grab_focus();
                            ui.open_options(&page, &button);
                        }
                    }
                });
                button.add_controller(secondary);
            }

            button.connect_clicked({
                let (ui, page) = (self.clone(), page.clone());
                move |button| {
                    if ui.click_while_picked() {
                        return;
                    }
                    ui.sounds.chosen();
                    ui.activate(&page, &ui.tile(&page, index), button);
                }
            });

            row.push(&button);
            buttons.push((index, button));
        }

        if arrangeable {
            buttons.sort_by_key(|(index, _)| *index);
            self.arrangeable.borrow_mut().insert(
                page.clone(),
                Arrangeable {
                    grid: row.clone(),
                    buttons,
                },
            );
        }
        self.stack.add_named(&column, Some(&page.key()));
        self.rows.borrow_mut().push(row);
    }

    /// Whether the selected tile of the main page is in its top row (or nothing there is
    /// selected), so Up leaves the tiles for the buttons above them.
    fn in_top_row(&self) -> bool {
        let focus =
            GtkWindowExt::focus(&self.window).and_then(|f| f.downcast::<gtk::Button>().ok());
        let arrangeable = self.arrangeable.borrow();
        match (focus, arrangeable.get(&Page::Main)) {
            (Some(button), Some(tiles)) => tiles.grid.row_of(&button).is_none_or(|row| row == 0),
            _ => true,
        }
    }

    /// The tile with keyboard focus on the visible page, if that page can be rearranged.
    fn focused_tile(&self) -> Option<(Page, gtk::Button)> {
        let page = self.current_page();
        let focus = GtkWindowExt::focus(&self.window)?;
        let arrangeable = self.arrangeable.borrow();
        let tiles = arrangeable.get(&page)?;
        let (_, button) = tiles
            .buttons
            .iter()
            .find(|(_, b)| b.upcast_ref::<gtk::Widget>() == &focus)?;
        Some((page, button.clone()))
    }

    /// Handles a key press that opens a tile's options, or moves or drops a picked tile.
    /// True if it did.
    fn arrange_key(self: &Rc<Self>, key: gtk::gdk::Key) -> bool {
        use gtk::gdk::Key;
        if is_enter(key) {
            return self.enter_pressed();
        }
        if self.picked.borrow().is_some() {
            match key {
                Key::Left => self.move_picked(-1),
                Key::Right => self.move_picked(1),
                Key::Up => self.move_picked_row(-1),
                Key::Down => self.move_picked_row(1),
                Key::Menu | Key::F2 => self.drop_picked(),
                key if is_back(key) => self.cancel_picked(),
                // The picked tile keeps the selection.
                Key::Tab | Key::ISO_Left_Tab => {}
                _ => return false,
            }
            return true;
        }
        match key {
            Key::Menu | Key::F2 => match self.focused_tile() {
                Some((page, button)) => {
                    self.open_options(&page, &button);
                    true
                }
                None => false,
            },
            // Holding Back on the main row asks to reset the tile order. Back does
            // nothing else there, so the press is dealt with here.
            key if is_back(key) && self.on_main_page() => {
                self.back_pressed();
                true
            }
            _ => false,
        }
    }

    /// Back on the main row: after [`RESET_HOLD`] the reset confirmation opens, unless
    /// Back is released first.
    fn back_pressed(self: &Rc<Self>) {
        if self.back_down.replace(true) {
            // Auto-repeat of a press that is already dealt with.
            return;
        }
        let ui = self.clone();
        self.back_timer
            .replace(Some(glib::timeout_add_local_once(RESET_HOLD, move || {
                ui.back_timer.take();
                let opener = GtkWindowExt::focus(&ui.window)
                    .and_then(|focus| focus.downcast::<gtk::Button>().ok())
                    .unwrap_or_else(|| ui.power_button.clone());
                ui.sounds.chosen();
                ui.open(Page::Confirm(Confirmable::ResetOrder), &opener);
            })));
    }

    fn back_released(&self) {
        self.back_down.set(false);
        if let Some(timer) = self.back_timer.take() {
            timer.remove();
        }
    }

    /// Enter on a tile of a rearrangeable row: the tile opens when Enter is released,
    /// unless it is held for [`LONG_PRESS`], which opens its options. With a tile picked
    /// up, Enter drops it.
    /// True if the press is dealt with here, false to leave it to GTK.
    fn enter_pressed(self: &Rc<Self>) -> bool {
        if self.enter_swallow.get() {
            return true;
        }
        // A repeat of a press that was not ours (GTK activated a button with it, and the
        // page it opened has the focus now): it must not activate that page's tile too.
        if self.enter_held.replace(true) && !self.enter_down.get() {
            return true;
        }
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
            Some((page, button)) => {
                let ui = self.clone();
                self.enter_timer.replace(Some(glib::timeout_add_local_once(
                    LONG_PRESS,
                    move || {
                        ui.enter_timer.take();
                        ui.enter_swallow.set(true);
                        ui.open_options(&page, &button);
                    },
                )));
            }
        }
        true
    }

    /// A tap of Enter on a tile opens it now that the key is up; after a long press the
    /// tile was already picked up, and there is nothing to do.
    fn enter_released(&self) {
        self.enter_swallow.set(false);
        self.enter_held.set(false);
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

    /// A click on a tile while one is picked up drops it. True if the click was used for
    /// that, so it does not open the tile.
    fn click_while_picked(&self) -> bool {
        if self.picked.borrow().is_some() {
            self.drop_picked();
            return true;
        }
        false
    }

    fn pick_up(&self, page: &Page, button: &gtk::Button) {
        let from = {
            let arrangeable = self.arrangeable.borrow();
            let Some(tiles) = arrangeable.get(page) else {
                return;
            };
            tiles.grid.position(button)
        };
        let Some(from) = from else {
            return;
        };
        button.add_css_class("picked");
        self.picked.replace(Some(Picked {
            page: page.clone(),
            button: button.clone(),
            from,
        }));
        self.sounds.chosen();
        self.update_arrange_hint();
    }

    /// Moves the picked tile one place along its row (continuing onto the next or
    /// previous row); it stays where it is at either end.
    fn move_picked(&self, delta: isize) {
        self.move_picked_to(|position, grid| order::step(position, delta, grid.len()));
    }

    /// Moves the picked tile one row up (-1) or down (+1), staying in its column.
    fn move_picked_row(&self, rows: isize) {
        self.move_picked_to(|position, grid| {
            grid::step_row(position, rows, grid.columns(), grid.len())
        });
    }

    /// Moves the picked tile to the place `target` gives from where it is now.
    fn move_picked_to(&self, target: impl Fn(usize, &TileGrid) -> Option<usize>) {
        let picked = self.picked.borrow();
        let arrangeable = self.arrangeable.borrow();
        let Some((picked, tiles)) = picked
            .as_ref()
            .and_then(|picked| Some((picked, arrangeable.get(&picked.page)?)))
        else {
            return;
        };
        let Some(position) = tiles.grid.position(&picked.button) else {
            return;
        };
        if let Some(target) = target(position, &tiles.grid) {
            tiles.grid.move_to(&picked.button, target);
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
            tiles.grid.move_to(&picked.button, picked.from);
        }
        self.refresh_first(&picked.page);
        picked.button.grab_focus();
        self.sounds.chosen();
        self.update_arrange_hint();
    }

    /// Ends everything the arranging keys started: a tile picked up goes back, and a
    /// pending long press of Enter or Back ends.
    fn stop_arranging(&self) {
        self.cancel_picked();
        self.enter_down.set(false);
        if let Some(timer) = self.enter_timer.take() {
            timer.remove();
        }
        self.back_down.set(false);
        if let Some(timer) = self.back_timer.take() {
            timer.remove();
        }
        self.update_arrange_hint();
    }

    /// The names of a page's tiles in its list, which every index of the page refers to.
    fn listed_names(&self, page: &Page) -> Vec<String> {
        match page {
            Page::Main => self.main.borrow().iter().map(|e| e.name.clone()).collect(),
            Page::Submenu(name) => self
                .submenus
                .borrow()
                .get(name)
                .map(|tiles| tiles.iter().map(|e| e.name.clone()).collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// The tile order of the page, as the row shows it now; the tiles that were removed
    /// keep the places they had.
    fn save_order(&self, page: &Page) {
        let names = self.listed_names(page);
        let configured: Vec<&str> = names.iter().map(String::as_str).collect();
        let identities = order::identities(&configured);
        let shown: Vec<order::Identity> = {
            let arrangeable = self.arrangeable.borrow();
            let Some(tiles) = arrangeable.get(page) else {
                return;
            };
            tiles
                .grid
                .buttons()
                .iter()
                .filter_map(|shown| tiles.buttons.iter().find(|(_, button)| button == shown))
                .map(|(index, _)| identities[*index].clone())
                .collect()
        };
        let mut saved = self.saved_order.borrow_mut();
        let key = page.key();
        let before = saved.get(&key).unwrap_or(&identities);
        let order = tiles::order_to_save(before, shown, self.shown.borrow().hidden_on(&key));
        saved.insert(key, order);
        order::save(&saved);
    }

    /// The first tile of a page is what is selected when it opens.
    fn refresh_first(&self, page: &Page) {
        let first = self
            .arrangeable
            .borrow()
            .get(page)
            .and_then(|tiles| tiles.grid.first());
        if let Some(first) = first {
            self.first_tiles.borrow_mut().insert(page.clone(), first);
        }
    }

    /// Puts every row back in the configured order and forgets the saved one.
    fn reset_order(&self) {
        // If the saved order cannot be removed it would come back at the next start, so
        // nothing is reset and the user is told, not shown a success.
        if let Err(error) = order::clear() {
            log::warn!("cannot reset the tile order: {error}");
            self.notify(&format!("Couldn't reset the tile order: {error}"));
            return;
        }
        self.saved_order.borrow_mut().clear();
        let pages: Vec<Page> = self.arrangeable.borrow().keys().cloned().collect();
        for page in &pages {
            if let Some(tiles) = self.arrangeable.borrow().get(page) {
                let configured = tiles.buttons.iter().map(|(_, b)| b.clone()).collect();
                tiles.grid.set_order(configured);
            }
            self.refresh_first(page);
        }
        self.main_row();
        self.notify("Tile order reset");
    }

    fn update_arrange_hint(&self) {
        let text = self.picked.borrow().is_some().then_some(PICKED_HINT);
        if let Some(text) = text {
            self.arrange_hint.set_label(text);
        }
        self.arrange_hint.set_visible(text.is_some());
    }

    /// Sizes and places every page's tiles to fit a window `width` by `height` logical
    /// pixels, so none is pushed off a small screen: they wrap onto more rows.
    fn fit_tiles(&self, width: i32, height: i32) {
        self.width.set(width);
        self.height.set(height);
        for row in self.rows.borrow().iter() {
            row.fit(width, height);
        }
    }

    fn header(&self, page: &Page, submenu: Option<&str>) -> gtk::Box {
        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .halign(gtk::Align::Start)
            .build();

        match submenu {
            None => {
                header.append(&self.clock.greeting());
                if let Some(title) = &self.config.borrow().title {
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
                self.titles.borrow_mut().insert(page.clone(), title.clone());
                header.append(&title);
            }
        }
        header
    }

    /// The key hints under a page; without `has_tiles` there is nothing to move over or
    /// open.
    fn hints(&self, in_submenu: bool, has_tiles: bool) -> gtk::Box {
        let hints = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(32)
            .margin_top(48)
            .build();
        if has_tiles {
            hints.append(&hint("← →", "Move"));
        }
        if in_submenu {
            hints.append(&hint("Enter", "Select"));
            hints.append(&hint("Esc", "Back"));
        } else {
            if has_tiles {
                hints.append(&hint("Enter", "Open"));
            }
            if let Some(key) = &self.config.borrow().home_key {
                hints.append(&hint(key, "Home (closes the open app)"));
            }
        }
        hints
    }

    /// Whether the app behind `command` is installed.
    fn installed(&self, command: &Command) -> bool {
        command.is_installed(self.config.borrow().browser.as_deref())
    }

    /// Marks the tile `button` (tile `index` of `page`) as missing, dimmed, if its app is
    /// not installed, and as present otherwise.
    fn check_installed(&self, page: &Page, index: usize, button: &gtk::Button) {
        let key = (page.clone(), index);
        if self.installed(&self.tile(page, index).command) {
            self.missing.borrow_mut().remove(&key);
            button.remove_css_class("unavailable");
        } else {
            self.missing.borrow_mut().insert(key);
            button.add_css_class("unavailable");
        }
    }

    /// Checks every configured tile again, for an app installed or removed since, and
    /// brings the description of the selected tile up to date.
    fn refresh_installed(&self) {
        for (page, tiles) in self.arrangeable.borrow().iter() {
            for (index, button) in tiles.buttons.iter() {
                self.check_installed(page, *index, button);
                if button.is_focus() {
                    self.show_tile(page, *index);
                }
            }
        }
    }

    /// Shows the name and description of a tile under its page's row. A tile whose app
    /// is not installed says so instead of its description.
    fn show_tile(&self, page: &Page, index: usize) {
        let labels = self.labels.borrow();
        let Some((name, description)) = labels.get(page) else {
            return;
        };
        let tile = self.tile(page, index);
        if self.missing.borrow().contains(&(page.clone(), index)) {
            name.set_label(&tile.name);
            description.set_label("Not installed");
        } else {
            show_selected(name, description, &tile);
        }
    }

    fn tile(&self, page: &Page, index: usize) -> Entry {
        match page {
            Page::Main => self.main.borrow()[index].clone(),
            Page::Submenu(name) => self.submenus.borrow()[name][index].clone(),
            Page::Add => self.add_entries.borrow()[index].clone(),
            Page::Power | Page::Confirm(_) | Page::TileOptions => self
                .own_pages
                .iter()
                .find(|own| own.page == *page)
                .expect("the launcher has a page of its own for each of its pages")
                .tiles[index]
                .clone(),
        }
    }

    /// `page` is the page the tile is on: a shutdown or restart asks for
    /// confirmation first, so a stray key press cannot power the machine off.
    fn activate(self: &Rc<Self>, page: &Page, entry: &Entry, button: &gtk::Button) {
        let confirming = matches!(page, Page::Confirm(_));
        if !confirming {
            if let Some(confirm) = Confirmable::of(&entry.command) {
                self.open(Page::Confirm(confirm), button);
                return;
            }
        }
        let result = match &entry.command {
            Command::Config(Action::Submenu(name)) => {
                self.open(Page::Submenu(name.clone()), button);
                Ok(())
            }
            Command::Config(Action::Builtin(Builtin::DesktopMode)) => {
                self.desktop_mode();
                Ok(())
            }
            Command::Config(Action::Builtin(Builtin::Shutdown)) => self.power(entry, "poweroff"),
            Command::Config(Action::Builtin(Builtin::Restart)) => self.power(entry, "reboot"),
            Command::ResetOrder => {
                self.reset_order();
                Ok(())
            }
            Command::Add(offer) => {
                self.add_tile(*offer);
                Ok(())
            }
            Command::Move => {
                self.begin_move();
                Ok(())
            }
            // Only the confirmation's own tile gets here: the options page asks first.
            Command::Remove => {
                self.remove_selected();
                Ok(())
            }
            Command::Web(_) if !self.installed(&entry.command) => {
                self.notify(&format!(
                    "{} needs a browser, and none is installed",
                    entry.name
                ));
                Ok(())
            }
            Command::Web(app) => self.open_web_app(page, entry, app, button),
            Command::Cancel => {
                self.back();
                Ok(())
            }
            // Checked again here, not read from `missing`: the app may have been installed
            // since. Nothing is closed for an app that cannot start.
            Command::Config(Action::Command(_) | Action::Desktop(_) | Action::Flatpak(_))
                if !self.installed(&entry.command) =>
            {
                self.notify(&format!("{} isn't installed", entry.name));
                Ok(())
            }
            Command::Config(
                action @ (Action::Command(_) | Action::Desktop(_) | Action::Flatpak(_)),
            ) => {
                self.remember_origin(page, button);
                self.launch(&entry.name, action);
                Ok(())
            }
        };
        if let Err(error) = result {
            self.failed(&entry.name, &error);
        }
    }

    /// A message over the tiles. Plain text: names and errors in it can hold `&` or `<`,
    /// which a toast would otherwise read as markup.
    fn notify(&self, text: &str) {
        self.toasts
            .add_toast(adw::Toast::builder().title(text).use_markup(false).build());
    }

    fn failed(&self, name: &str, error: &anyhow::Error) {
        log::error!("{name}: {error:#}");
        self.notify(&format!("Couldn't open {name}"));
    }

    /// systemctl fails when logind refuses, e.g. while another user is logged in.
    fn power(self: &Rc<Self>, entry: &Entry, verb: &str) -> anyhow::Result<()> {
        let process = launch::systemctl(verb)?;
        let (ui, name) = (self.clone(), entry.name.clone());
        process.wait_check_async(None::<&gio::Cancellable>, move |result| {
            if let Err(error) = result {
                ui.failed(&name, &error.into());
            }
        });
        Ok(())
    }

    /// One app at a time: the previous one is closed, and the new one starts once it
    /// is gone. A tile chosen while that is pending replaces the pending one.
    fn launch(self: &Rc<Self>, name: &str, action: &Action) {
        self.close_then(Request::Tile(name.to_owned(), action.clone()));
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
                Err(launch::StopError::Timeout) => {
                    ui.notify(NOT_CLOSED);
                }
                Err(launch::StopError::GaveUp {
                    apps,
                    others_still_running,
                }) => {
                    log::warn!("giving up on {apps:?}: they will not close");
                    ui.notify(&format!("{} won't close; giving up on it", apps.join(", ")));
                    if others_still_running {
                        ui.notify(NOT_CLOSED);
                    }
                }
                Err(launch::StopError::Unknown(launched)) => {
                    // The previous app may still be running: start nothing over it.
                    // Choosing the tile again retries the close, still knowing the
                    // app started last (`running` is empty while a close runs).
                    ui.running.replace(launched);
                    ui.notify("Couldn't check whether the previous app closed");
                    return;
                }
            }
            match request {
                Some(Request::Tile(name, action)) => ui.start(&name, &action),
                Some(Request::Desktop) => ui.hide_for_desktop(),
                Some(Request::Home) | None => {}
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
        self.main_row_at_origin();
        if self.window.is_visible() {
            self.window.present();
        }
    }

    /// Opens the options (move, remove) of the tile `button` of `page`.
    fn open_options(&self, page: &Page, button: &gtk::Button) {
        let index = self.arrangeable.borrow().get(page).and_then(|tiles| {
            tiles
                .buttons
                .iter()
                .find(|(_, shown)| shown == button)
                .map(|(index, _)| *index)
        });
        let Some(index) = index else {
            return;
        };
        let name = self.tile(page, index).name;
        self.options_for.replace(Some((page.clone(), index)));
        self.set_title(&Page::TileOptions, &name);
        self.set_title(
            &Page::Confirm(Confirmable::RemoveTile),
            &format!("Remove {name}?"),
        );
        self.sounds.chosen();
        self.open(Page::TileOptions, button);
    }

    fn set_title(&self, page: &Page, text: &str) {
        if let Some(title) = self.titles.borrow().get(page) {
            title.set_label(text);
        }
    }

    /// Back from the options page to the tile, picked up for moving.
    fn begin_move(&self) {
        let Some((page, opener)) = self.history.borrow().last().cloned() else {
            return;
        };
        if !matches!(page, Page::Main | Page::Submenu(_)) {
            return;
        }
        self.options_for.take();
        self.back();
        self.pick_up(&page, &opener);
    }

    /// Takes the tile of the options page off the launcher: a service that was added is
    /// forgotten, a configured tile is hidden. Either can be put back from "+".
    fn remove_selected(self: &Rc<Self>) {
        let Some((page, index)) = self.options_for.take() else {
            return;
        };
        let entry = self.tile(&page, index);
        let position = self.arrangeable.borrow().get(&page).and_then(|tiles| {
            let (_, button) = tiles.buttons.iter().find(|(i, _)| *i == index)?;
            tiles.grid.position(button)
        });
        match entry.command {
            Command::Web(app) => self.shown.borrow_mut().remove(app.id),
            _ => {
                let names = self.listed_names(&page);
                let names: Vec<&str> = names.iter().map(String::as_str).collect();
                let identity = order::identities(&names)[index].clone();
                self.shown.borrow_mut().hide(&page.key(), identity);
            }
        }
        self.save_shown();
        self.rebuild(None);
        self.notify(&format!("Removed {}", entry.name));
        // Back on the page it was removed from, on the tile that took its place, or the
        // last one.
        if self.reopen(&page) {
            let button = self.arrangeable.borrow().get(&page).and_then(|tiles| {
                let shown = tiles.grid.buttons();
                shown.get(position.unwrap_or(0)).or(shown.last()).cloned()
            });
            if let Some(button) = button {
                button.grab_focus();
            }
        }
    }

    /// Shows `page` again after the pages were rebuilt, which leaves them for the main
    /// row: a submenu is opened from the main-row tile that leads to it. False if it
    /// could not be, e.g. that tile was removed.
    fn reopen(&self, page: &Page) -> bool {
        let Page::Submenu(name) = page else {
            return *page == Page::Main;
        };
        let opener = self
            .arrangeable
            .borrow()
            .get(&Page::Main)
            .and_then(|tiles| {
                tiles
                    .buttons
                    .iter()
                    .find(|(index, _)| {
                        self.tile(&Page::Main, *index).command
                            == Command::Config(Action::Submenu(name.clone()))
                    })
                    .map(|(_, button)| button.clone())
            });
        match opener {
            Some(opener) => {
                self.open(page.clone(), &opener);
                true
            }
            None => false,
        }
    }

    /// Opens the Add page, made for what can be added now.
    fn open_add(self: &Rc<Self>, opener: &gtk::Button) {
        let key = Page::Add.key();
        if let Some(old) = self.stack.child_by_name(&key) {
            self.stack.remove(&old);
        }
        self.first_tiles.borrow_mut().remove(&Page::Add);
        self.labels.borrow_mut().remove(&Page::Add);
        self.titles.borrow_mut().remove(&Page::Add);
        self.missing
            .borrow_mut()
            .retain(|(page, _)| *page != Page::Add);
        self.rows
            .borrow_mut()
            .retain(|row| row.widget().root().is_some());

        let entries = self.addable_entries();
        *self.add_entries.borrow_mut() = entries.clone();
        self.add_page(Page::Add, Some("Add a tile"), &entries, false);
        if self.width.get() > 0 {
            self.fit_tiles(self.width.get(), self.height.get());
        }
        self.open(Page::Add, opener);
    }

    /// What the Add page offers: removed tiles first, then the catalog.
    fn addable_entries(&self) -> Vec<Entry> {
        let pages = configured_pages(&self.config.borrow());
        let names: Vec<(&str, Vec<&str>)> = pages
            .iter()
            .map(|(key, _, entries)| {
                (
                    key.as_str(),
                    entries.iter().map(|entry| entry.name.as_str()).collect(),
                )
            })
            .collect();
        tiles::addable(&names, &self.shown.borrow())
            .into_iter()
            .map(|offer| match offer {
                Addable::Hidden { page, index } => {
                    let (_, label, entries) = &pages[page];
                    let tile = &entries[index];
                    Entry {
                        description: Some(format!("Put back on {label}")),
                        command: Command::Add(offer),
                        ..tile.clone()
                    }
                }
                Addable::Catalog(app) => Entry {
                    command: Command::Add(offer),
                    ..catalog_entry(app)
                },
            })
            .collect()
    }

    /// Puts a choice of the Add page on the launcher, and selects it if it is on the
    /// main row.
    fn add_tile(self: &Rc<Self>, offer: Addable) {
        let (name, page, toast) = match offer {
            Addable::Catalog(app) => {
                self.shown.borrow_mut().add(app.id);
                (
                    app.name.to_owned(),
                    Page::Main,
                    format!("Added {}", app.name),
                )
            }
            Addable::Hidden { page, index } => {
                let pages = configured_pages(&self.config.borrow());
                let (key, label, entries) = &pages[page];
                let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
                let identity = order::identities(&names)[index].clone();
                self.shown.borrow_mut().unhide(key, &identity);
                let name = entries[index].name.clone();
                let toast = format!("Put {name} back on {label}");
                (name, Page::from_key(key).unwrap_or(Page::Main), toast)
            }
        };
        self.save_shown();
        self.rebuild(None);
        self.notify(&toast);
        if page == Page::Main {
            let button = self.arrangeable.borrow().get(&page).and_then(|tiles| {
                tiles
                    .buttons
                    .iter()
                    .find(|(index, _)| self.tile(&page, *index).name == name)
                    .map(|(_, button)| button.clone())
            });
            if let Some(button) = button {
                button.grab_focus();
            }
        }
    }

    fn save_shown(&self) {
        if !self.shown.borrow().save() {
            self.notify("Couldn't save the change; it is lost at the next start");
        }
    }

    /// Opens a service of the catalog in the browser, closing the open app first.
    fn open_web_app(
        self: &Rc<Self>,
        page: &Page,
        entry: &Entry,
        app: &'static catalog::App,
        button: &gtk::Button,
    ) -> anyhow::Result<()> {
        let browser = webapp::find_browser(self.config.borrow().browser.as_deref())
            .ok_or_else(|| anyhow::anyhow!("no browser"))?;
        let line = webapp::prepare(app, &browser)?;
        self.remember_origin(page, button);
        self.launch(&entry.name, &Action::Command(line));
        Ok(())
    }

    fn open(&self, page: Page, opener: &gtk::Button) {
        self.history
            .borrow_mut()
            .push((self.current_page(), opener.clone()));
        self.stack.set_visible_child_name(&page.key());
        if let Some(first) = self.first_tiles.borrow().get(&page) {
            first.grab_focus();
        }
    }

    /// The page being shown.
    fn current_page(&self) -> Page {
        self.stack
            .visible_child_name()
            .and_then(|name| Page::from_key(&name))
            .unwrap_or(Page::Main)
    }

    fn on_main_page(&self) -> bool {
        self.current_page() == Page::Main
    }

    /// Returns to the page this submenu was opened from; false on the main page.
    fn back(&self) -> bool {
        let Some((page, opener)) = self.history.borrow_mut().pop() else {
            return false;
        };
        self.stack.set_visible_child_name(&page.key());
        opener.grab_focus();
        true
    }
}

/// Hides the pointer over `widget` after [`POINTER_IDLE`] without movement, and from the
/// start: a TV is driven by keyboard or remote until the mouse moves. `on_change` is told
/// whenever the pointer is shown or hidden.
fn hide_idle_pointer(widget: &impl IsA<gtk::Widget>, on_change: impl Fn(bool) + Clone + 'static) {
    let widget = widget.clone().upcast::<gtk::Widget>();
    widget.set_cursor_from_name(Some("none"));
    let visible = Rc::new(Cell::new(false));
    let last = Cell::new((f64::NAN, f64::NAN));
    let timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();

    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion({
        // Weak: the controller belongs to the widget, and a strong reference from here
        // would keep a replaced widget alive.
        let widget = widget.downgrade();
        move |_, x, y| {
            let Some(widget) = widget.upgrade() else {
                return;
            };
            // Widgets moving under a still pointer also report motion.
            let (last_x, last_y) = last.replace((x, y));
            if (x - last_x).abs() < 1.0 && (y - last_y).abs() < 1.0 {
                return;
            }
            if !visible.replace(true) {
                on_change(true);
                widget.set_cursor(None);
            }
            if let Some(pending) = timer.take() {
                pending.remove();
            }
            let (widget, on_change, visible, slot) = (
                widget.downgrade(),
                on_change.clone(),
                visible.clone(),
                timer.clone(),
            );
            timer.replace(Some(glib::timeout_add_local_once(
                POINTER_IDLE,
                move || {
                    slot.take();
                    visible.set(false);
                    on_change(false);
                    if let Some(widget) = widget.upgrade() {
                        widget.set_cursor_from_name(Some("none"));
                    }
                },
            )));
        }
    });
    widget.add_controller(motion);
}

/// Enter, and the select key of a TV remote.
fn is_enter(key: gtk::gdk::Key) -> bool {
    use gtk::gdk::Key;
    matches!(key, Key::Return | Key::KP_Enter | Key::ISO_Enter) || is_select(key)
}

/// `XF86OK`, the keysym of the evdev `KEY_OK`, which a CEC remote's OK button sends.
/// gdk4 has no constant for it.
const XF86_OK: u32 = 0x1008_1160;

/// The select or OK button of a remote: evdev `KEY_SELECT` is `XF86Select`
/// ([`Key::SelectButton`]), `KEY_OK` is `XF86OK`, and `Select` is the old X keysym.
fn is_select(key: gtk::gdk::Key) -> bool {
    use gtk::gdk::Key;
    matches!(key, Key::Select | Key::SelectButton)
        || glib::translate::IntoGlib::into_glib(key) == XF86_OK
}

/// Escape, Backspace, and the back key of a TV remote.
fn is_back(key: gtk::gdk::Key) -> bool {
    use gtk::gdk::Key;
    matches!(key, Key::Escape | Key::BackSpace | Key::Back)
}

/// Keys that move the selection from tile to tile.
fn is_navigation(key: gtk::gdk::Key) -> bool {
    use gtk::gdk::Key;
    matches!(
        key,
        Key::Left | Key::Right | Key::Up | Key::Down | Key::Tab | Key::ISO_Left_Tab
    )
}

/// A page the launcher adds itself: the power menu and its confirmations.
struct OwnPage {
    page: Page,
    title: &'static str,
    tiles: Vec<Entry>,
}

fn entry(name: &str, icon: &str, description: &str, command: Command) -> Entry {
    Entry {
        name: name.to_owned(),
        icon: Some(icon.to_owned()),
        description: Some(description.to_owned()),
        command,
    }
}

/// The power menu, and a confirmation for each of its actions that powers the machine
/// off. Cancel comes first, so it is what is selected when a confirmation opens.
fn own_pages() -> Vec<OwnPage> {
    let cancel = || {
        entry(
            "Cancel",
            "window-close-symbolic",
            "Go back",
            Command::Cancel,
        )
    };
    vec![
        OwnPage {
            page: Page::Power,
            title: "Power",
            tiles: vec![
                entry(
                    "Desktop Mode",
                    "video-display-symbolic",
                    "Hide the launcher and show the GNOME desktop",
                    Command::Config(Action::Builtin(Builtin::DesktopMode)),
                ),
                entry(
                    "Restart",
                    "system-reboot-symbolic",
                    "Restart the HTPC",
                    Command::Config(Action::Builtin(Builtin::Restart)),
                ),
                entry(
                    "Shutdown",
                    "system-shutdown-symbolic",
                    "Turn the HTPC off",
                    Command::Config(Action::Builtin(Builtin::Shutdown)),
                ),
            ],
        },
        // The title is the tile's name, set when the page opens.
        OwnPage {
            page: Page::TileOptions,
            title: "Tile options",
            tiles: vec![
                cancel(),
                entry(
                    "Move",
                    "list-drag-handle-symbolic",
                    "Pick the tile up and put it somewhere else",
                    Command::Move,
                ),
                entry(
                    "Remove",
                    "user-trash-symbolic",
                    "Take the tile off the launcher",
                    Command::Remove,
                ),
            ],
        },
        OwnPage {
            page: Page::Confirm(Confirmable::RemoveTile),
            title: "Remove?",
            tiles: vec![
                cancel(),
                entry(
                    "Remove",
                    "user-trash-symbolic",
                    "It can be added back with +",
                    Command::Remove,
                ),
            ],
        },
        OwnPage {
            page: Page::Confirm(Confirmable::ResetOrder),
            title: "Reset tile order?",
            tiles: vec![
                cancel(),
                entry(
                    "Reset",
                    "edit-undo-symbolic",
                    "Use the configured tile order again",
                    Command::ResetOrder,
                ),
            ],
        },
        OwnPage {
            page: Page::Confirm(Confirmable::Shutdown),
            title: "Shut down?",
            tiles: vec![
                cancel(),
                entry(
                    "Shut down",
                    "system-shutdown-symbolic",
                    "Turn the HTPC off now",
                    Command::Config(Action::Builtin(Builtin::Shutdown)),
                ),
            ],
        },
        OwnPage {
            page: Page::Confirm(Confirmable::Restart),
            title: "Restart?",
            tiles: vec![
                cancel(),
                entry(
                    "Restart",
                    "system-reboot-symbolic",
                    "Restart the HTPC now",
                    Command::Config(Action::Builtin(Builtin::Restart)),
                ),
            ],
        },
    ]
}

/// The tiles of the main row: the configured ones, then the services that were added.
fn main_entries(config: &Config, shown: &tiles::Shown) -> Vec<Entry> {
    let names: Vec<&str> = config.tiles.iter().map(|tile| tile.name.as_str()).collect();
    tiles::listed(tiles::MAIN, &names, shown)
        .into_iter()
        .map(|(item, _)| match item {
            tiles::Item::Configured(index) => Entry::from(&config.tiles[index]),
            tiles::Item::Catalog(app) => catalog_entry(app),
        })
        .collect()
}

fn submenu_entries(config: &Config) -> BTreeMap<String, Vec<Entry>> {
    config
        .submenus
        .iter()
        .map(|(name, tiles)| (name.clone(), tiles.iter().map(Entry::from).collect()))
        .collect()
}

fn catalog_entry(app: &'static catalog::App) -> Entry {
    Entry {
        name: app.name.to_owned(),
        icon: Some(app.icon()),
        description: Some(app.description.to_owned()),
        command: Command::Web(app),
    }
}

/// The pages tiles can be taken off, main row first: the page's key, what it is called
/// ("Put back on ..."), and its configured tiles, with the icon each shows.
fn configured_pages(config: &Config) -> Vec<(String, String, Vec<Entry>)> {
    let entries = |tiles: &[Tile]| -> Vec<Entry> {
        tiles
            .iter()
            .map(|tile| Entry {
                icon: resolved_icon(tile),
                ..Entry::from(tile)
            })
            .collect()
    };
    let main = (
        Page::Main.key(),
        "the main row".to_owned(),
        entries(&config.tiles),
    );
    let submenus = config.submenus.iter().map(|(name, tiles)| {
        (
            Page::Submenu(name.clone()).key(),
            name.clone(),
            entries(tiles),
        )
    });
    std::iter::once(main).chain(submenus).collect()
}

/// The icon a configured tile shows, as a name or path, for a copy of the tile that has
/// another command: the one it was given, else its application's.
fn resolved_icon(tile: &Tile) -> Option<String> {
    if let Some(icon) = &tile.icon {
        return Some(icon.clone());
    }
    match &tile.action {
        Action::Desktop(id) => {
            let icon = gio_unix::DesktopAppInfo::new(id)?.icon()?;
            match icon.downcast::<gio::ThemedIcon>() {
                Ok(themed) => themed.names().first().map(|name| name.to_string()),
                Err(icon) => gio::prelude::IconExt::to_string(&icon).map(|name| name.to_string()),
            }
        }
        Action::Flatpak(id) => Some(id.clone()),
        _ => None,
    }
}

/// What an empty page says instead of a selected tile's name and description.
fn empty_page_text(page: &Page) -> Option<(&'static str, &'static str)> {
    match page {
        Page::Main => Some(("No tiles yet", "Press + to add some")),
        Page::Submenu(_) => Some(("No tiles here", "Put them back with + on the main row")),
        Page::Add => Some(("Everything is already on the launcher", "")),
        _ => None,
    }
}

/// Top-right, left of the power button: the way into the Add page.
fn plus_button() -> gtk::Button {
    let icon = gtk::Image::from_icon_name("list-add-symbolic");
    icon.set_pixel_size(32);
    let button = gtk::Button::builder()
        .child(&icon)
        .css_classes(["power-button"])
        .valign(gtk::Align::Start)
        .margin_top(8)
        .build();
    button.update_property(&[gtk::accessible::Property::Label("Add a tile")]);
    button
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

/// The picture behind the tiles, or a plain dark background without one.
fn background_widget(path: Option<&std::path::Path>) -> gtk::Widget {
    match path {
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
    }
}

/// Shown for a tile whose icon cannot be found.
const PLACEHOLDER_ICON: &str = "application-x-executable";

/// The image of a tile. A configured icon is used as written; without one, a `desktop`
/// tile shows its application's icon and a `flatpak` tile the icon named like its
/// application id. An icon that cannot be found (no desktop file, no icon in it, a name
/// the icon theme lacks, a file that is not there) shows [`PLACEHOLDER_ICON`] and logs
/// a warning, so a tile is never blank.
fn tile_image(tile: &Entry) -> gtk::Image {
    let theme = gtk::gdk::Display::default().map(|display| gtk::IconTheme::for_display(&display));
    match find_icon(tile, theme.as_ref()) {
        Ok(icon) => gtk::Image::from_gicon(&icon),
        Err(reason) => {
            log::warn!(
                "tile \"{}\": {reason}; showing a placeholder icon",
                tile.name
            );
            gtk::Image::from_icon_name(PLACEHOLDER_ICON)
        }
    }
}

fn find_icon(tile: &Entry, theme: Option<&gtk::IconTheme>) -> Result<gio::Icon, String> {
    let icon: gio::Icon = match (&tile.icon, &tile.command) {
        (Some(icon), _) if icon.starts_with('/') => {
            gio::FileIcon::new(&gio::File::for_path(icon)).upcast()
        }
        (Some(icon), _) => gio::ThemedIcon::new(icon).upcast(),
        (None, Command::Config(Action::Desktop(id))) => gio_unix::DesktopAppInfo::new(id)
            .ok_or_else(|| format!("no desktop file {id}"))?
            .icon()
            .ok_or_else(|| format!("{id} names no icon"))?,
        (None, Command::Config(Action::Flatpak(id))) => gio::ThemedIcon::new(id).upcast(),
        (None, _) => return Err("no icon".into()),
    };
    if icon_resolves(&icon, theme) {
        Ok(icon)
    } else {
        Err(format!(
            "the icon {} cannot be found",
            gio::prelude::IconExt::to_string(&icon).unwrap_or_default()
        ))
    }
}

/// Whether `icon` is in the icon theme, or is a regular file. Any other kind of icon
/// is taken as it comes.
fn icon_resolves(icon: &gio::Icon, theme: Option<&gtk::IconTheme>) -> bool {
    if let Some(themed) = icon.downcast_ref::<gio::ThemedIcon>() {
        return theme.is_none_or(|theme| themed.names().iter().any(|name| theme.has_icon(name)));
    }
    if let Some(file) = icon.downcast_ref::<gio::FileIcon>() {
        // A directory would show nothing. (A file that is no image still would; the
        // loaders, not this check, know which files they can decode.)
        let kind = file
            .file()
            .query_file_type(gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE);
        return kind == gio::FileType::Regular;
    }
    true
}

fn tile_button(tile: &Entry) -> gtk::Button {
    let icon = tile_image(tile);
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

fn show_selected(name: &gtk::Label, description: &gtk::Label, tile: &Entry) {
    name.set_label(&tile.name);
    description.set_label(tile.description.as_deref().unwrap_or(""));
}

#[cfg(test)]
mod tests {
    use super::{
        is_back, is_enter, is_navigation, order_listing, own_pages, CloseQueue, Command,
        Confirmable, Page, Request,
    };
    use crate::config::{Action, Builtin};
    use gtk::gdk::Key;

    const ALL_CONFIRMABLE: [Confirmable; 4] = [
        Confirmable::Shutdown,
        Confirmable::Restart,
        Confirmable::ResetOrder,
        Confirmable::RemoveTile,
    ];

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
    fn remote_keys_act_like_their_keyboard_counterparts() {
        for key in [
            Key::Return,
            Key::KP_Enter,
            Key::ISO_Enter,
            Key::Select,
            Key::SelectButton,
            // SAFETY: a key is only a keyval; gdk has no name for this one.
            unsafe { glib::translate::FromGlib::from_glib(super::XF86_OK) },
        ] {
            assert!(is_enter(key));
            assert!(!is_back(key));
        }
        for key in [Key::Escape, Key::BackSpace, Key::Back] {
            assert!(is_back(key));
            assert!(!is_enter(key));
        }
        assert!(!is_enter(Key::HomePage) && !is_back(Key::HomePage));
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
    fn the_power_menu_offers_desktop_mode_restart_and_shutdown() {
        let pages = own_pages();
        let power = pages.iter().find(|own| own.page == Page::Power).unwrap();
        let commands: Vec<_> = power.tiles.iter().map(|tile| &tile.command).collect();
        assert_eq!(
            commands,
            [
                &Command::Config(Action::Builtin(Builtin::DesktopMode)),
                &Command::Config(Action::Builtin(Builtin::Restart)),
                &Command::Config(Action::Builtin(Builtin::Shutdown)),
            ]
        );
    }

    /// The command of the tile that goes ahead with `confirm`.
    fn go_ahead(confirm: Confirmable) -> Command {
        match confirm {
            Confirmable::Shutdown => Command::Config(Action::Builtin(Builtin::Shutdown)),
            Confirmable::Restart => Command::Config(Action::Builtin(Builtin::Restart)),
            Confirmable::ResetOrder => Command::ResetOrder,
            Confirmable::RemoveTile => Command::Remove,
        }
    }

    #[test]
    fn every_power_off_or_reset_action_has_a_confirmation_that_selects_cancel_first() {
        let pages = own_pages();
        for confirm in ALL_CONFIRMABLE {
            // What asks for the confirmation is what the confirmation then goes ahead with.
            assert_eq!(Confirmable::of(&go_ahead(confirm)), Some(confirm));
            let page = pages
                .iter()
                .find(|own| own.page == Page::Confirm(confirm))
                .unwrap();
            assert_eq!(page.tiles[0].command, Command::Cancel);
            assert_eq!(page.tiles[1].command, go_ahead(confirm));
            assert_eq!(page.tiles.len(), 2);
        }
    }

    #[test]
    fn only_the_commands_that_discard_or_power_off_need_a_confirmation() {
        let config = |action| Command::Config(action);
        for (command, needs) in [
            (config(Action::Builtin(Builtin::Shutdown)), true),
            (config(Action::Builtin(Builtin::Restart)), true),
            (Command::ResetOrder, true),
            (Command::Remove, true),
            (Command::Move, false),
            (
                Command::Web(crate::catalog::find("netflix").unwrap()),
                false,
            ),
            (
                Command::Add(crate::tiles::Addable::Catalog(
                    crate::catalog::find("netflix").unwrap(),
                )),
                false,
            ),
            (config(Action::Builtin(Builtin::DesktopMode)), false),
            (config(Action::Command("mpv".into())), false),
            (config(Action::Desktop("a.desktop".into())), false),
            (config(Action::Flatpak("a.b".into())), false),
            (config(Action::Submenu("Games".into())), false),
            (Command::Cancel, false),
        ] {
            assert_eq!(Confirmable::of(&command).is_some(), needs, "{command:?}");
        }
    }

    // What `--print-order` prints is what the rows show: the saved arrangement, by the keys
    // the saved file holds, over the configured order; tiles added since go last.
    #[test]
    fn the_listing_follows_the_saved_order_of_each_page() {
        let config = crate::config::Config::parse(
            r#"{ "tiles": [
                    { "name": "A", "icon": "a", "command": "a" },
                    { "name": "B", "icon": "b", "command": "b" },
                    { "name": "Games", "icon": "g", "submenu": "Games" },
                    { "name": "New", "icon": "n", "command": "n" } ],
                "submenus": { "Games": [
                    { "name": "X", "icon": "x", "command": "x" },
                    { "name": "Y", "icon": "y", "command": "y" } ] } }"#,
        )
        .unwrap();
        let saved: crate::order::Saved = serde_json::from_str(
            r#"{"main":[["B",0],["A",0],["Games",0]],"submenu:Games":[["Y",0],["X",0]]}"#,
        )
        .unwrap();
        assert_eq!(
            order_listing(&config, &saved, &crate::tiles::Shown::default()),
            "main:\n  1. B\n  2. A\n  3. Games\n  4. New\nsubmenu Games:\n  1. Y\n  2. X\n"
        );
        // With nothing saved, the configured order.
        assert_eq!(
            order_listing(
                &config,
                &crate::order::Saved::new(),
                &crate::tiles::Shown::default()
            ),
            "main:\n  1. A\n  2. B\n  3. Games\n  4. New\nsubmenu Games:\n  1. X\n  2. Y\n"
        );
    }

    // The launcher adds exactly one page for the power menu and for each confirmation,
    // and none other, so `Ui::tile` always finds the page of its own.
    #[test]
    fn the_launcher_has_one_page_of_its_own_for_each_of_its_pages() {
        let pages: Vec<_> = own_pages().into_iter().map(|own| own.page).collect();
        let mut expected = vec![Page::Power, Page::TileOptions];
        expected.extend(ALL_CONFIRMABLE.map(Page::Confirm));
        for page in &expected {
            assert_eq!(pages.iter().filter(|p| *p == page).count(), 1, "{page:?}");
        }
        assert_eq!(pages.len(), expected.len());
    }

    // `order.json` files and the stack name pages by these strings, so they are fixed.
    #[test]
    fn page_keys_are_the_names_existing_files_hold() {
        assert_eq!(Page::Main.key(), "main");
        assert_eq!(Page::Power.key(), "power");
        assert_eq!(Page::Submenu("Games".into()).key(), "submenu:Games");
        assert_eq!(
            Page::Confirm(Confirmable::Shutdown).key(),
            "confirm:shutdown"
        );
        assert_eq!(Page::Confirm(Confirmable::Restart).key(), "confirm:restart");
        assert_eq!(
            Page::Confirm(Confirmable::ResetOrder).key(),
            "confirm:reset"
        );
        // The pages that came with adding and removing tiles.
        assert_eq!(Page::Add.key(), "add");
        assert_eq!(Page::TileOptions.key(), "options");
        assert_eq!(
            Page::Confirm(Confirmable::RemoveTile).key(),
            "confirm:remove"
        );
        // The main row's key is the one `tiles.json` names it by.
        assert_eq!(Page::Main.key(), crate::tiles::MAIN);
    }

    #[test]
    fn a_page_is_found_again_from_its_key() {
        let mut pages = vec![Page::Main, Page::Power, Page::Add, Page::TileOptions];
        // Submenu names are the user's, whatever they hold.
        for name in [
            "Games",
            "a:b",
            "",
            "main",
            "power",
            "confirm:shutdown",
            "submenu:x",
        ] {
            pages.push(Page::Submenu(name.to_owned()));
        }
        pages.extend(ALL_CONFIRMABLE.map(Page::Confirm));
        for page in pages {
            assert_eq!(Page::from_key(&page.key()), Some(page.clone()), "{page:?}");
        }
        for key in ["", "nope", "confirm:", "confirm:sleep", "Main"] {
            assert_eq!(Page::from_key(key), None, "{key:?}");
        }
    }

    // An `order.json` written before pages were typed, by the launcher itself, still
    // applies: its keys are the ones `Page::key` gives.
    #[test]
    fn an_order_file_saved_before_pages_were_typed_still_applies() {
        let saved: crate::order::Saved = serde_json::from_str(
            r#"{"main":[["B",0],["A",0],["Games",0]],"submenu:Games":[["Y",0],["X",0]]}"#,
        )
        .unwrap();
        let main = saved.get(&Page::Main.key());
        assert_eq!(crate::order::arrange(&["A", "B", "Games"], main), [1, 0, 2]);
        let games = saved.get(&Page::Submenu("Games".into()).key());
        assert_eq!(crate::order::arrange(&["X", "Y"], games), [1, 0]);
    }

    #[test]
    fn the_options_page_offers_cancel_first_then_move_and_remove() {
        let pages = own_pages();
        let options = pages
            .iter()
            .find(|own| own.page == Page::TileOptions)
            .unwrap();
        let commands: Vec<_> = options.tiles.iter().map(|tile| &tile.command).collect();
        assert_eq!(
            commands,
            [&Command::Cancel, &Command::Move, &Command::Remove]
        );
    }

    // `--print-order` shows what the screen shows: added services last (or where the
    // order put them), removed tiles gone.
    #[test]
    fn the_listing_has_the_added_services_and_not_the_removed_tiles() {
        let config = crate::config::Config::parse(
            r#"{ "tiles": [
                    { "name": "A", "icon": "a", "command": "a" },
                    { "name": "B", "icon": "b", "command": "b" },
                    { "name": "C", "icon": "c", "command": "c" } ] }"#,
        )
        .unwrap();
        let shown: crate::tiles::Shown = serde_json::from_str(
            r#"{ "added": ["netflix", "hulu"], "hidden": { "main": [["B", 0]] } }"#,
        )
        .unwrap();
        let saved: crate::order::Saved =
            serde_json::from_str(r#"{ "main": [["Hulu", 0], ["A", 0]] }"#).unwrap();
        assert_eq!(
            order_listing(&config, &saved, &shown),
            "main:\n  1. Hulu\n  2. A\n  3. C\n  4. Netflix\n"
        );
    }

    // A tile asks for a catalog service by the name the Add page lists it under.
    #[test]
    fn a_catalog_service_has_a_tile_that_opens_it_with_its_own_icon() {
        let config = crate::config::Config::default();
        let shown: crate::tiles::Shown =
            serde_json::from_str(r#"{ "added": ["netflix"] }"#).unwrap();
        let entries = super::main_entries(&config, &shown);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Netflix");
        assert_eq!(entries[0].icon.as_deref(), Some("vextheater-netflix"));
        assert!(matches!(entries[0].command, Command::Web(app) if app.id == "netflix"));
    }

    // A reload waits while a close runs, so the queue says whether one does.
    #[test]
    fn the_queue_says_whether_a_close_is_running() {
        let queue = CloseQueue::new();
        assert!(!queue.is_busy());
        assert!(queue.submit("brave"));
        assert!(queue.is_busy());
        queue.finish();
        assert!(!queue.is_busy());
    }

    #[test]
    fn desktop_mode_during_a_close_replaces_the_pending_request() {
        let queue = CloseQueue::new();
        assert!(queue.submit(Request::Tile(
            "Brave".into(),
            Action::Command("brave".into())
        )));
        assert!(!queue.submit(Request::Desktop));
        assert!(matches!(queue.finish(), Some(Request::Desktop)));
    }
}
