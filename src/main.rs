//! VexTheater — a full-screen TV launcher for GNOME.

mod clock;
mod config;
mod window;

use adw::prelude::*;

/// Also the gresource prefix.
pub const APP_ID: &str = "io.github.vextheater";

fn main() -> glib::ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    gio::resources_register_include!("compiled.gresource")
        .expect("the compiled gresource bundle should be embedded in the binary");

    let application = adw::Application::builder().application_id(APP_ID).build();

    application.connect_startup(|_| {
        adw::init().expect("libadwaita should initialise");
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
        load_stylesheet();
    });

    application.connect_activate(window::present);
    application.run()
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
