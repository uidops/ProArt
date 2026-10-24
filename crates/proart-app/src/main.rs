//! ProArt: control ASUS ProArt monitors from the menu bar / tray (GPUI).

// no console window behind the GUI on Windows release builds
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod actions;
mod controls;
mod learn;
mod main_view;
mod panel_view;
mod state;
mod text_field;
mod theme;
mod tray;
mod ui;

use std::sync::Arc;
use std::time::Duration;

use gpui::{actions, point, prelude::*, px, App, Application, AsyncApp, Image, ImageFormat, KeyBinding, Menu, MenuItem, WindowHandle};
use proart_core::backend::default_backend;
use proart_core::{Manager, Settings};

use main_view::{MainView, Page};
use panel_view::PanelView;
use state::{Bus, Shared};
use tray::TrayMsg;

actions!(proart, [Quit]);

const LOGO: &[u8] = include_bytes!("../../../assets/icons/app-256.png");

struct Args {
    menubar: bool,
    no_tray: bool,
    panel: bool,
    page: Page,
}

fn parse_args() -> Args {
    let mut a = Args { menubar: false, no_tray: false, panel: false, page: Page::Overview };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--menubar" => a.menubar = true,
            "--no-tray" => a.no_tray = true,
            "--panel" => a.panel = true,
            "--page" => {
                if let Some(p) = it.next().and_then(|p| Page::parse(&p)) {
                    a.page = p;
                }
            }
            "-h" | "--help" => {
                println!(
                    "ProArt {}\n\nUsage: proart-app [--menubar] [--no-tray] [--panel] [--page NAME]\n\n  --menubar   start hidden in the menu bar / tray\n  --no-tray   no tray icon\n  --panel     open the quick panel at start\n  --page      overview, preset, blue, palette, image, sound, input, system, info, presets, displays, settings, about",
                    proart_core::VERSION
                );
                std::process::exit(0);
            }
            _ => {}
        }
    }
    a
}

/// Handles to the windows we may need to focus or close.
#[derive(Default)]
struct Windows {
    main: Option<WindowHandle<MainView>>,
    panel: Option<WindowHandle<PanelView>>,
}

impl gpui::Global for Windows {}

pub fn open_main(cx: &mut App, page: Page) {
    if let Some(h) = cx.global::<Windows>().main {
        if h.update(cx, |_, window, _| window.activate_window()).is_ok() {
            cx.activate(true);
            return;
        }
    }
    tray::set_dock_visible(true);
    let opts = main_view::window_options(cx);
    match cx.open_window(opts, move |window, cx| cx.new(|cx| MainView::new(page, window, cx))) {
        Ok(h) => cx.global_mut::<Windows>().main = Some(h),
        Err(e) => log::error!("cannot open the main window: {e}"),
    }
    cx.activate(true);
}

fn main_open(cx: &App) -> bool {
    cx.windows().iter().any(|w| w.downcast::<MainView>().is_some())
}

/// Cmd+Q: with a tray icon, close the windows and keep running in the tray;
/// without one, really quit (nothing could bring the app back).
fn quit_to_tray(cx: &mut App) {
    if !cx.global::<Shared>().tray {
        cx.quit();
        return;
    }
    // deferred: the action runs while the focused window is being updated,
    // and a window cannot be updated (closed) from inside its own update
    cx.defer(|cx| {
        close_panel(cx);
        for w in cx.windows() {
            if let Some(h) = w.downcast::<MainView>() {
                let _ = h.update(cx, |_, window, _| window.remove_window());
            }
        }
        tray::set_dock_visible(false);
    });
}

fn close_panel(cx: &mut App) -> bool {
    if let Some(h) = cx.global_mut::<Windows>().panel.take() {
        return h.update(cx, |_, window, _| window.remove_window()).is_ok();
    }
    false
}

fn open_panel(cx: &mut App, origin: Option<gpui::Point<gpui::Pixels>>) {
    let rows = {
        let sh = cx.global::<Shared>();
        sh.mgr.displays().first().map(|st| panel_view::quick_features(&sh.mgr, st).len()).unwrap_or(0)
    };
    let origin = origin.unwrap_or_else(|| {
        // no tray position (--panel): top right, under the menu bar
        let w = cx.primary_display().map(|d| f32::from(d.bounds().size.width)).unwrap_or(1440.0);
        point(px(w - panel_view::PANEL_W - 16.0), px(34.0))
    });
    match cx.open_window(panel_view::window_options(origin, rows), |window, cx| cx.new(|cx| PanelView::new(window, cx))) {
        Ok(h) => {
            cx.global_mut::<Windows>().panel = Some(h);
            cx.activate(true);
        }
        Err(e) => log::error!("cannot open the panel: {e}"),
    }
}

fn handle_tray(cx: &mut App, msg: TrayMsg) {
    match msg {
        TrayMsg::Toggle { x, y, w, h, cursor_x } => {
            if close_panel(cx) {
                return;
            }
            let scale = tray::scale_for(cursor_x) as f32;
            let (x, y, w, h) = (x as f32 / scale, y as f32 / scale, w as f32 / scale, h as f32 / scale);
            let rows = {
                let sh = cx.global::<Shared>();
                sh.mgr.displays().first().map(|st| panel_view::quick_features(&sh.mgr, st).len()).unwrap_or(0)
            };
            let origin = panel_view::origin_below(cx, x + w / 2.0, y, y + h, rows);
            open_panel(cx, Some(origin));
        }
        TrayMsg::OpenMain => open_main(cx, Page::Overview),
        TrayMsg::Quit => cx.quit(),
    }
}

fn main() {
    let args = parse_args();
    let bus = Arc::new(Bus::default());
    let settings = Settings::load();
    let mgr = {
        let bus = bus.clone();
        Manager::new(default_backend(), settings, Arc::new(move |_| bus.repaint()))
    };
    mgr.start();

    let mgr_run = mgr.clone();
    Application::new().run(move |cx: &mut App| {
        text_field::bind_keys(cx);
        cx.bind_keys([KeyBinding::new("secondary-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| quit_to_tray(cx));
        cx.set_menus(vec![Menu { name: "ProArt".into(), items: vec![MenuItem::action("Quit ProArt", Quit)] }]);

        let tray = if args.no_tray { None } else { tray::create() };
        let has_tray = tray.is_some();
        cx.set_global(Shared {
            mgr: mgr_run.clone(),
            bus: bus.clone(),
            logo: Arc::new(Image::from_bytes(ImageFormat::Png, LOGO.to_vec())),
            tray: has_tray,
        });
        cx.set_global(Windows::default());

        if !args.menubar || !has_tray {
            open_main(cx, args.page);
        } else {
            tray::set_dock_visible(false);
        }
        if args.panel {
            open_panel(cx, None);
        }

        if let Some((handle, rx)) = tray {
            // tray events arrive on the main thread through a channel: poll it
            cx.spawn(async move |cx: &mut AsyncApp| {
                let _keep_alive = handle;
                loop {
                    cx.background_executor().timer(Duration::from_millis(80)).await;
                    let msgs: Vec<TrayMsg> = rx.try_iter().collect();
                    for m in msgs {
                        if cx.update(|cx| handle_tray(cx, m)).is_err() {
                            return;
                        }
                    }
                }
            })
            .detach();
        }

        // without a tray icon, closing the last window quits (nothing could reopen it);
        // with one, closing the main window leaves only the tray icon
        cx.on_window_closed(|cx| {
            if !cx.global::<Shared>().tray {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            } else if !main_open(cx) {
                tray::set_dock_visible(false);
            }
        })
        .detach();
        let mgr_quit = mgr_run.clone();
        cx.on_app_quit(move |_| {
            mgr_quit.stop();
            mgr_quit.save_settings();
            async {}
        })
        .detach();
        cx.activate(true);
    });
}
