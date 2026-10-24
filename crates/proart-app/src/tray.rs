//! System tray / menu-bar icon. On Linux it needs GTK, so it is behind the
//! `linux-tray` cargo feature there.

use std::sync::mpsc::Receiver;

#[derive(Debug)]
pub enum TrayMsg {
    /// Left click: toggle the panel under the icon (physical pixels).
    Toggle { x: f64, y: f64, w: f64, h: f64, cursor_x: f64 },
    OpenMain,
    Quit,
}

#[cfg(any(not(target_os = "linux"), feature = "linux-tray"))]
pub struct TrayHandle {
    _icon: tray_icon::TrayIcon,
}

#[cfg(not(any(not(target_os = "linux"), feature = "linux-tray")))]
pub struct TrayHandle;

#[cfg(any(not(target_os = "linux"), feature = "linux-tray"))]
pub fn create() -> Option<(TrayHandle, Receiver<TrayMsg>)> {
    use std::sync::mpsc;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    #[cfg(target_os = "macos")]
    const ICON: &[u8] = include_bytes!("../../../assets/icons/tray-template.png");
    #[cfg(not(target_os = "macos"))]
    const ICON: &[u8] = include_bytes!("../../../assets/icons/tray-color.png");

    let img = image::load_from_memory(ICON).ok()?.to_rgba8();
    let (w, h) = (img.width(), img.height());
    let icon = Icon::from_rgba(img.into_raw(), w, h).ok()?;

    let open = MenuItem::new("Open ProArt", true, None);
    let quit = MenuItem::new("Quit ProArt", true, None);
    let menu = Menu::new();
    menu.append_items(&[&open, &PredefinedMenuItem::separator(), &quit]).ok()?;
    let (open_id, quit_id) = (open.id().clone(), quit.id().clone());

    let (tx, rx) = mpsc::channel();
    let tx_menu = tx.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == open_id {
            let _ = tx_menu.send(TrayMsg::OpenMain);
        } else if e.id == quit_id {
            let _ = tx_menu.send(TrayMsg::Quit);
        }
    }));
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, rect, position, .. } = e {
            let _ = tx.send(TrayMsg::Toggle {
                x: rect.position.x,
                y: rect.position.y,
                w: rect.size.width as f64,
                h: rect.size.height as f64,
                cursor_x: position.x,
            });
        }
    }));

    let builder = TrayIconBuilder::new().with_tooltip("ProArt").with_menu(Box::new(menu)).with_menu_on_left_click(false);
    // macOS tints a template icon to match the menu bar (light/dark)
    #[cfg(target_os = "macos")]
    let builder = builder.with_icon_templated(icon);
    #[cfg(not(target_os = "macos"))]
    let builder = builder.with_icon(icon);
    let icon = builder.build().ok()?;
    Some((TrayHandle { _icon: icon }, rx))
}

#[cfg(not(any(not(target_os = "linux"), feature = "linux-tray")))]
pub fn create() -> Option<(TrayHandle, Receiver<TrayMsg>)> {
    None
}

/// Points per physical pixel factor for tray rectangles (they arrive in
/// physical pixels, GPUI positions windows in points).
#[cfg(target_os = "macos")]
pub fn scale_for(cursor_x_physical: f64) -> f64 {
    // [NSEvent mouseLocation] is in points; the tray click position is in
    // physical pixels of the same x origin, so their ratio is the backing scale.
    use std::ffi::{c_char, c_void};
    #[repr(C)]
    struct NSPoint {
        x: f64,
        y: f64,
    }
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }
    unsafe {
        let class = objc_getClass(c"NSEvent".as_ptr());
        let sel = sel_registerName(c"mouseLocation".as_ptr());
        let send: unsafe extern "C" fn(*mut c_void, *mut c_void) -> NSPoint = std::mem::transmute(objc_msgSend as *const ());
        let p = send(class, sel);
        if p.x > 1.0 && cursor_x_physical > 1.0 {
            let ratio = cursor_x_physical / p.x;
            if (0.9..=4.1).contains(&ratio) {
                return ratio.round().max(1.0);
            }
        }
    }
    2.0
}

#[cfg(target_os = "windows")]
pub fn scale_for(_cursor_x_physical: f64) -> f64 {
    // GetDpiForSystem() / 96
    #[link(name = "user32")]
    extern "system" {
        fn GetDpiForSystem() -> u32;
    }
    let dpi = unsafe { GetDpiForSystem() };
    if dpi >= 96 { dpi as f64 / 96.0 } else { 1.0 }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn scale_for(_cursor_x_physical: f64) -> f64 {
    1.0
}

/// Shows or hides the app in the Dock, the app switcher and Stage Manager.
/// GPUI forces the regular policy at launch (overriding `LSUIElement`), so
/// with only the tray left we switch to the accessory policy ourselves.
#[cfg(target_os = "macos")]
pub fn set_dock_visible(visible: bool) {
    use std::ffi::{c_char, c_void};
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }
    // NSApplicationActivationPolicyRegular = 0, ...Accessory = 1
    let policy: isize = if visible { 0 } else { 1 };
    unsafe {
        let class = objc_getClass(c"NSApplication".as_ptr());
        let shared: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void = std::mem::transmute(objc_msgSend as *const ());
        let app = shared(class, sel_registerName(c"sharedApplication".as_ptr()));
        if app.is_null() {
            return;
        }
        let set: unsafe extern "C" fn(*mut c_void, *mut c_void, isize) -> i8 = std::mem::transmute(objc_msgSend as *const ());
        set(app, sel_registerName(c"setActivationPolicy:".as_ptr()), policy);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_dock_visible(_visible: bool) {}
