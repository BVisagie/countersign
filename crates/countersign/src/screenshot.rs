//! Renders the interface to PNG files with the fictional demo vault.
//!
//! `COUNTERSIGN_SCREENSHOT=<folder> countersign` draws each view offscreen
//! from GTK's own widget tree, then exits. Windows are placed on the
//! background layer without keyboard focus, so nothing interrupts the
//! desktop, and the output does not depend on the screen being unlocked.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use countersign_ipc::Request;
use gtk::glib;
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};

use crate::app::App;
use crate::ui::{quick, window};

fn save(widget: &impl IsA<gtk::Widget>, path: &Path) -> Result<(), String> {
    let widget = widget.as_ref();
    let (width, height) = (widget.width(), widget.height());
    if width == 0 || height == 0 {
        return Err("the widget has no size yet".into());
    }
    const SCALE: f32 = 2.0;
    let paintable = gtk::WidgetPaintable::new(Some(widget));
    let snapshot = gtk::Snapshot::new();
    snapshot.scale(SCALE, SCALE);
    paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
    let node = snapshot.to_node().ok_or("nothing was drawn")?;
    // A standalone renderer works on every display backend, including the
    // headless Broadway server.
    let renderer = gtk::gsk::CairoRenderer::new();
    renderer.realize_for_display(&widget.display()).map_err(|e| e.to_string())?;
    let bounds = gtk::graphene::Rect::new(0.0, 0.0, width as f32 * SCALE, height as f32 * SCALE);
    let texture = renderer.render_texture(&node, Some(&bounds));
    renderer.unrealize();
    texture.save_to_png(path).map_err(|e| e.to_string())
}

fn background(window: &gtk::Window) {
    if gtk4_layer_shell::is_supported() {
        window.init_layer_shell();
        window.set_layer(Layer::Background);
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_namespace(Some("countersign-screenshot"));
    }
}

async fn settle() {
    glib::timeout_future(Duration::from_millis(1200)).await;
}

pub fn run(app: &Rc<App>, dir: PathBuf) {
    let app = app.clone();
    glib::spawn_future_local(async move {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("cannot create {}: {e}", dir.display());
            app.gtk.quit();
            return;
        }
        // Stack transitions wait for animation frames, which a headless
        // display may never deliver.
        if let Some(settings) = gtk::Settings::default() {
            settings.set_gtk_enable_animations(false);
        }
        app.start().await;
        app.reload_vault().await;
        let shot = |widget: &gtk::Widget, name: &str| {
            let path = dir.join(format!("{name}.png"));
            match save(widget, &path) {
                Ok(()) => println!("{}", path.display()),
                Err(e) => eprintln!("{name}: {e}"),
            }
        };

        let main = window::get_or_create(&app);
        main.window.set_default_size(1280, 800);
        background(main.window.upcast_ref());
        main.window.present();
        settle().await;
        main.list.select_by_id("00000000-0000-4000-8000-000000000001");
        settle().await;
        shot(main.window.upcast_ref(), "main");

        main.search.set_text("git");
        settle().await;
        shot(main.window.upcast_ref(), "main-search");
        main.search.set_text("");

        main.list.select_by_id("00000000-0000-4000-8000-000000000030");
        settle().await;
        shot(main.window.upcast_ref(), "main-card");

        main.window.set_default_size(820, 800);
        settle().await;
        main.list.select_by_id("00000000-0000-4000-8000-000000000001");
        settle().await;
        shot(main.window.upcast_ref(), "main-medium");

        let _ = WidgetExt::activate_action(&main.window, "win.generator", None);
        settle().await;
        shot(main.window.upcast_ref(), "generator");

        let quick = quick::get_or_create(&app);
        background(quick.window.upcast_ref());
        quick.show();
        if gtk4_layer_shell::is_supported() {
            quick.window.set_keyboard_mode(KeyboardMode::None);
        }
        settle().await;
        shot(quick.window.child().as_ref().expect("child"), "quick");
        quick.search.set_text("git");
        settle().await;
        shot(quick.window.child().as_ref().expect("child"), "quick-search");

        let _ = app.request(Request::Lock).await;
        settle().await;
        shot(quick.window.child().as_ref().expect("child"), "quick-locked");
        main.window.set_default_size(1280, 800);
        settle().await;
        shot(main.window.upcast_ref(), "unlock");
        app.gtk.quit();
    });
}
