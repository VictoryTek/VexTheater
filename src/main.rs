//! VexTheater — a full-screen TV launcher for GNOME.

mod clock;
mod config;
mod diagnose;
mod files;
mod gnome;
mod keybinding;
mod launch;
mod mode;
mod order;
mod sound;
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
            print!("{}", window::order_listing(&config, &order::load()));
            0
        }
        Err(error) => {
            eprintln!("vextheater: {error:#}");
            1
        }
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
