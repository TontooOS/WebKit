//! Minimal GTK4-only TontooWebKit example (no UIKit).
//!
//! Shows the smallest possible embedding: create a [`webkit::WebView`], pack
//! its widget into a window and navigate to a start URL.
//!
//! Run with: `cargo run --example minimal`

use gtk::prelude::*;
use webkit::{WebKitConfiguration, WebView};

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("org.tontoo.webkit.example")
        .build();

    app.connect_activate(|app| {
        let web_view = WebView::new(
            WebKitConfiguration::new().start_url("https://example.com"),
        )
        .expect("failed to create web view");

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("TontooWebKit")
            .default_width(900)
            .default_height(640)
            .build();
        window.set_child(Some(&web_view.widget()));
        window.present();
    });

    app.run()
}