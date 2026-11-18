//! VexTheater — a full-screen TV launcher for GNOME.

mod bundle;
mod catalog;
mod clock;
mod config;
mod diagnose;
mod files;
mod gnome;
mod grid;
mod keybinding;
mod launch;
mod mode;
mod order;
mod sound;
mod tiles;
mod webapp;
mod widgets;
mod window;

use adw::prelude::*;

/// Also the gresource prefix.
pub const APP_ID: &str = "io.github.vextheater";

fn main() -> glib::ExitCode {
    // Before anything of GTK or GIO is set up: this must work with no display, as in
    // the sandbox of a Nix build.
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments
        .first()
        .is_some_and(|flag| flag == "--check-config")
    {
        std::process::exit(check_config(&arguments[1..]));
    }
    if arguments
        .first()
        .is_some_and(|flag| flag == "--print-order")
    {
        std::process::exit(print_order(&arguments[1..]));
    }
    if arguments.first().is_some_and(|flag| flag == "--list-tiles") {
        std::process::exit(list_tiles(&arguments[1..]));
    }
    if arguments
        .first()
        .is_some_and(|flag| flag == "--export-layout")
    {
        std::process::exit(export_layout(&arguments[1..]));
    }
    if arguments
        .first()
        .is_some_and(|flag| flag == "--import-layout")
    {
        std::process::exit(import_layout(&arguments[1..]));
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    gio::resources_register_include!("compiled.gresource")
        .expect("the compiled gresource bundle should be embedded in the binary");

    // Command lines go to the running instance, so `vextheater --home` from a keyboard
    // shortcut acts on the launcher that is already open.
    let application = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    application.connect_startup(|_| {
        adw::init().expect("libadwaita should initialise");
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
        load_stylesheet();
    });

    application.connect_command_line(window::command_line);
    application.connect_activate(window::activate);

    // Every way out ends here: quitting, closing the last window (Alt+F4), and the
    // signals below, which systemd sends when the session ends.
    application.connect_shutdown(|_| {
        mode::exit();
    });
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let application = application.clone();
        glib_unix::unix_signal_add_local(signal, move || {
            application.quit();
            glib::ControlFlow::Break
        });
    }

    application.run()
}

/// `--check-config [FILE]`: checks a configuration file, by default the one the
/// launcher would read. Exits 0, or 1 with the file and the field at fault on stderr.
fn check_config(files: &[String]) -> i32 {
    let path = match files {
        [] => config::Config::default_path(),
        [file] => file.into(),
        _ => {
            eprintln!("usage: vextheater --check-config [FILE]");
            return 2;
        }
    };
    match config::Config::load(&path) {
        Ok(_) => {
            println!("{}: ok", path.display());
            0
        }
        Err(error) => {
            eprintln!("vextheater: {error:#}");
            1
        }
    }
}

/// `--print-order`: prints the tiles of each page in the order the launcher shows them,
/// the saved arrangement over the configured order. Needs no display, and is for seeing
/// what an `order.json` does, or that a configuration change moved something.
fn print_order(extra: &[String]) -> i32 {
    if !extra.is_empty() {
        eprintln!("usage: vextheater --print-order");
        return 2;
    }
    match config::Config::load_default() {
        Ok(config) => {
            let listing = window::order_listing(&config, &order::load(), &tiles::Shown::load());
            print!("{listing}");
            0
        }
        Err(error) => {
            eprintln!("vextheater: {error:#}");
            1
        }
    }
}

/// `--list-tiles`: prints the tiles that start an app as JSON, for scripts that choose
/// one with `--launch`. Needs no display and no running launcher; it is what
/// `--print-order` shows, less the tiles that are not apps.
fn list_tiles(extra: &[String]) -> i32 {
    if !extra.is_empty() {
        eprintln!("usage: vextheater --list-tiles");
        return 2;
    }
    match config::Config::load_default() {
        Ok(config) => {
            print!(
                "{}",
                window::tile_listing(&config, &order::load(), &tiles::Shown::load())
            );
            0
        }
        Err(error) => {
            eprintln!("vextheater: {error:#}");
            1
        }
    }
}

/// `--export-layout [FILE]`: writes the layout the user made (tiles added and removed,
/// and the order of each page) as a bundle to FILE, or to standard output. Needs no
/// display.
fn export_layout(arguments: &[String]) -> i32 {
    let file = match arguments {
        [] => None,
        [file] => Some(file),
        _ => {
            eprintln!("usage: vextheater --export-layout [FILE]");
            return 2;
        }
    };
    let config = match config::Config::load_default() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("vextheater: {error:#}");
            return 1;
        }
    };
    let (shown, saved) = bundle::Targets::user().load();
    let text = bundle::Bundle::capture(&window::page_names(&config), &shown, &saved).to_text();
    match file {
        None => {
            print!("{text}");
            0
        }
        Some(file) => match files::write_atomic(std::path::Path::new(file), text.as_bytes()) {
            Ok(()) => {
                println!("{file}: layout exported");
                0
            }
            Err(error) => {
                eprintln!("vextheater: cannot write {file}: {error}");
                1
            }
        },
    }
}

/// `--import-layout FILE [--apply]`: checks a bundle and says what restoring it would
/// do, against the configuration and the current layout. Changes nothing unless
/// `--apply` is given, and then not while the launcher is running, which would write its
/// own layout over the restored one. Needs no display.
fn import_layout(arguments: &[String]) -> i32 {
    let usage = "usage: vextheater --import-layout FILE [--apply]";
    let apply = arguments.iter().any(|argument| argument == "--apply");
    let mut paths = arguments.iter().filter(|argument| *argument != "--apply");
    let (Some(file), None) = (paths.next(), paths.next()) else {
        eprintln!("{usage}");
        return 2;
    };
    let fail = |message: String| {
        eprintln!("vextheater: {message}");
        1
    };

    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) => return fail(format!("cannot read {file}: {error}")),
    };
    let bundle = match bundle::Bundle::parse(&text) {
        Ok(bundle) => bundle,
        Err(error) => return fail(format!("{file}: {error}")),
    };
    let config = match config::Config::load_default() {
        Ok(config) => config,
        Err(error) => return fail(format!("{error:#}")),
    };
    let pages = window::page_names(&config);
    let targets = bundle::Targets::user();
    let (shown, saved) = targets.load();
    print!("{}", bundle::preview(&bundle, &pages, (&shown, &saved)));
    if !apply {
        println!("Nothing was changed. Run it again with --apply to restore this layout.");
        return 0;
    }

    match bundle::launcher_running() {
        Ok(true) => {
            return fail(
                "the launcher is running; quit it first, or it would overwrite the restored layout"
                    .to_owned(),
            );
        }
        Ok(false) => {}
        Err(error) => {
            eprintln!("vextheater: warning: cannot tell whether the launcher is running ({error})")
        }
    }
    match bundle::apply(&bundle, &pages, &targets) {
        Ok(()) => {
            println!(
                "Layout restored. Your previous layout is in {}.",
                targets.previous.display()
            );
            0
        }
        Err(error) => fail(error),
    }
}

fn load_stylesheet() {
    let provider = gtk::CssProvider::new();
    provider.load_from_resource("/io/github/vextheater/style.css");
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("a display should be available"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
