//! The bridge between the engine and the webview. The UI reads state with the
//! commands below, and rereads when `engine-changed` fires. It holds no logic of
//! its own, the same as the CLI.

// Don't open a console window next to the app on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::ffi::OsString;
use std::fs;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use engine::{Device, Engine, Update};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use protocol::{Channel, Command, CommandOutput, MachineId, SessionEvent, SessionRef};
use settings::{Binding, InstalledFont, Scheme, Settings, Store, Value};
#[cfg(target_os = "macos")]
use tauri::menu::{
    AboutMetadata, HELP_SUBMENU_ID, Menu, PredefinedMenuItem, Submenu, WINDOW_SUBMENU_ID,
};
use tauri::{AppHandle, Emitter, Manager, State, Theme, Window, WindowEvent, window::Color};
use tokio::sync::broadcast::{self, error::RecvError, error::TryRecvError};

/// Streaming output arrives token by token, so wake the UI at most once per frame.
const FRAME: Duration = Duration::from_millis(16);

/// One save can be several filesystem events, so let them finish before rereading.
const SETTLE: Duration = Duration::from_millis(50);

type SharedSettings = Arc<Mutex<Store>>;

/// The theme being tried out in a list, which the window shows until the list closes. It's
/// never saved, so it stays out of the settings.
#[derive(Default)]
struct TriedTheme(Mutex<Option<String>>);

fn lock(store: &SharedSettings) -> MutexGuard<'_, Store> {
    store.lock().unwrap_or_else(PoisonError::into_inner)
}

#[tauri::command]
fn devices(engine: State<'_, Engine>) -> Vec<Device> {
    engine.devices()
}

#[tauri::command]
async fn request(
    engine: State<'_, Engine>,
    machine_id: MachineId,
    command: Command,
) -> Result<CommandOutput, String> {
    engine
        .request(&machine_id, command)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn open_session(engine: State<'_, Engine>, session: SessionRef) {
    engine.open_session(&session);
}

#[tauri::command]
fn events_after(
    engine: State<'_, Engine>,
    session: SessionRef,
    after_seq: u64,
) -> Vec<SessionEvent> {
    engine.events_after(&session, after_seq)
}

#[tauri::command]
fn settings(store: State<'_, SharedSettings>) -> Settings {
    lock(&store).settings()
}

#[tauri::command]
fn set_keybind(
    store: State<'_, SharedSettings>,
    id: String,
    bindings: Vec<Binding>,
) -> Result<Settings, String> {
    let mut store = lock(&store);
    store.set_keybind(&id, bindings)?;
    Ok(store.settings())
}

#[tauri::command]
fn reset_keybind(store: State<'_, SharedSettings>, id: String) -> Result<Settings, String> {
    let mut store = lock(&store);
    store.reset_keybind(&id)?;
    Ok(store.settings())
}

#[tauri::command]
fn set_appearance(
    app: AppHandle,
    store: State<'_, SharedSettings>,
    id: String,
    value: Value,
) -> Result<Settings, String> {
    // Let go before repainting, which needs this lock and can run right here.
    let settings = {
        let mut store = lock(&store);
        store.set_appearance(&id, value)?;
        store.settings()
    };
    repaint_windows(&app);
    Ok(settings)
}

#[tauri::command]
fn reset_appearance(
    app: AppHandle,
    store: State<'_, SharedSettings>,
    id: String,
) -> Result<Settings, String> {
    let settings = {
        let mut store = lock(&store);
        store.reset_appearance(&id)?;
        store.settings()
    };
    repaint_windows(&app);
    Ok(settings)
}

/// Shows a theme in what the webview can't draw while it's being tried out. Nothing goes back
/// to the settings until this gets None. A theme that doesn't exist shows the settings' own.
#[tauri::command]
fn try_window_theme(app: AppHandle, tried: State<'_, TriedTheme>, theme: Option<String>) {
    *tried.0.lock().unwrap_or_else(PoisonError::into_inner) = theme;
    repaint_windows(&app);
}

/// Reading every font file takes a while, so this stays off the threads that answer the webview.
#[tauri::command]
async fn installed_fonts() -> Vec<InstalledFont> {
    tauri::async_runtime::spawn_blocking(settings::installed_fonts)
        .await
        .unwrap_or_default()
}

/// Long blocks take a while, so this stays off the threads that answer the webview.
#[tauri::command]
async fn highlight_code(code: String, language: String) -> Vec<highlight::Span> {
    tauri::async_runtime::spawn_blocking(move || highlight::highlight(&code, &language))
        .await
        .unwrap_or_default()
}

/// Opens a link from an agent's message in the browser. Agents write those links, so only
/// web addresses get through: other schemes could launch any app that registered one.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    let parsed = tauri::Url::parse(&url).map_err(|error| error.to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("not a web address: {url}"));
    }
    open::that_detached(parsed.as_str()).map_err(|error| error.to_string())
}

/// The system clipboard's text, for vim's p. WebKit asks before letting a page read it,
/// with a Paste button for every read, so it comes through here instead. Anything that
/// isn't text reads as empty.
#[tauri::command]
fn read_clipboard() -> Result<String, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|error| error.to_string())?;
    match clipboard.get_text() {
        Ok(text) => Ok(text),
        Err(arboard::Error::ContentNotAvailable) => Ok(String::new()),
        Err(error) => Err(error.to_string()),
    }
}

/// Puts what vim yanked on the system clipboard, next to reading it, so both go one way.
#[tauri::command]
fn write_clipboard(text: String) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|error| error.to_string())?;
    clipboard.set_text(text).map_err(|error| error.to_string())
}

/// Applies the settings file whenever something else saves it. Saving usually replaces
/// the file, so this watches its folder. If it's a symlink, the folder it points into too.
fn watch_settings(app: AppHandle, store: SharedSettings) -> notify::Result<RecommendedWatcher> {
    let path = lock(&store).path().to_owned();
    let real = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    let names: Vec<OsString> = [&path, &real]
        .into_iter()
        .filter_map(|path| path.file_name().map(OsString::from))
        .collect();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        let about_settings = event
            .paths
            .iter()
            .filter_map(|path| path.file_name())
            .any(|name| names.iter().any(|ours| ours == name));
        if !about_settings {
            return;
        }
        std::thread::sleep(SETTLE);
        // Let go before emitting: sync commands wait for this lock on the main thread,
        // and emitting can need the main thread.
        let changed = {
            let mut store = lock(&store);
            store.reload().then(|| store.settings())
        };
        if let Some(settings) = changed {
            repaint_windows(&app);
            let _ = app.emit("settings-changed", settings);
        }
    })?;
    let mut folders: Vec<_> = [path.parent(), real.parent()]
        .into_iter()
        .flatten()
        .collect();
    folders.dedup();
    for folder in folders {
        watcher.watch(folder, RecursiveMode::NonRecursive)?;
    }
    Ok(watcher)
}

/// Fires `engine-changed` after changes, with whether the device tree needs
/// rereading. Session events aren't sent along: the UI asks for everything after
/// the last one it has, so it can't miss any.
async fn forward_updates(app: AppHandle, mut updates: broadcast::Receiver<Update>) {
    loop {
        let mut devices = match updates.recv().await {
            Ok(Update::SessionEvent { .. }) => false,
            Ok(Update::DeviceChanged { .. }) | Err(RecvError::Lagged(_)) => true,
            Err(RecvError::Closed) => return,
        };
        tokio::time::sleep(FRAME).await;
        loop {
            match updates.try_recv() {
                Ok(Update::SessionEvent { .. }) => {}
                Ok(Update::DeviceChanged { .. }) | Err(TryRecvError::Lagged(_)) => devices = true,
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            }
        }
        let _ = app.emit("engine-changed", devices);
    }
}

/// The same setting as the CLI. There's no UI for this yet, because daemons
/// only listen on loopback until pairing exists.
fn daemon_urls() -> Vec<String> {
    let urls: Vec<String> = std::env::var("TECTR_DAEMONS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_owned)
        .collect();
    if urls.is_empty() {
        vec![protocol::local_daemon_url()]
    } else {
        urls
    }
}

/// In full screen, macOS draws the title bar in its own window, which ignores the
/// color of ours. Coloring `NSTitlebarView` and hiding `NSTitlebarBackgroundView` is
/// what Ghostty does, and Tauri has no API for it. Those are private AppKit classes,
/// so if a macOS update renames them this does nothing and the bar goes back to the
/// system's color.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)] // The only exception in this crate. See `[lints]` in Cargo.toml.
fn color_title_bar(window: &Window, color: Color) {
    use objc2::{
        MainThreadMarker,
        encode::{Encoding, RefEncode},
        msg_send,
        rc::Retained,
        runtime::{AnyClass, AnyObject},
    };

    #[repr(C)]
    struct CGColor([u8; 0]);
    // Safety: this is how the Objective-C runtime describes a `CGColorRef`.
    unsafe impl RefEncode for CGColor {
        const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGColor", &[]));
    }

    const NS_WINDOW_CLOSE_BUTTON: usize = 0;

    let Ok(ns_window) = window.ns_window() else {
        return;
    };
    let (Some(_), Some(bar_class), Some(background_class)) = (
        MainThreadMarker::new(),
        AnyClass::get(c"NSTitlebarView"),
        AnyClass::get(c"NSTitlebarBackgroundView"),
    ) else {
        return;
    };
    let channel = |value: u8| f64::from(value) / 255.0;

    // Safety: `ns_window` is this live window's NSWindow, and we checked above that
    // this is the main thread, where AppKit has to be used. Every selector is public
    // AppKit or Core Animation API with the types given here, which objc2 checks at
    // runtime in debug builds.
    unsafe {
        let ns_window = &*ns_window.cast::<AnyObject>();
        let close: Option<Retained<AnyObject>> =
            msg_send![ns_window, standardWindowButton: NS_WINDOW_CLOSE_BUTTON];
        let Some(close) = close else { return };
        let bar: Option<Retained<AnyObject>> = msg_send![&*close, superview];
        let Some(bar) = bar else { return };
        let is_bar: bool = msg_send![&*bar, isKindOfClass: bar_class];
        if !is_bar {
            return;
        }

        let () = msg_send![&*bar, setWantsLayer: true];
        let layer: Option<Retained<AnyObject>> = msg_send![&*bar, layer];
        if let Some(layer) = layer {
            let ns_color: Retained<AnyObject> = msg_send![
                objc2::class!(NSColor),
                colorWithRed: channel(color.0),
                green: channel(color.1),
                blue: channel(color.2),
                alpha: channel(color.3)
            ];
            let cg_color: *mut CGColor = msg_send![&*ns_color, CGColor];
            let () = msg_send![&*layer, setBackgroundColor: cg_color];
        }

        let container: Option<Retained<AnyObject>> = msg_send![&*bar, superview];
        let Some(container) = container else { return };
        let siblings: Retained<AnyObject> = msg_send![&*container, subviews];
        let count: usize = msg_send![&*siblings, count];
        for index in 0..count {
            let view: Retained<AnyObject> = msg_send![&*siblings, objectAtIndex: index];
            let is_background: bool = msg_send![&*view, isKindOfClass: background_class];
            if is_background {
                let () = msg_send![&*view, setHidden: true];
            }
        }
    }
}

fn tried_theme(manager: &impl Manager<tauri::Wry>) -> Option<String> {
    manager
        .try_state::<TriedTheme>()?
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// The native title bar is transparent, so it shows the window's own color, which is the
/// `surface.base` of the theme in use. Runs on the main thread, where AppKit has to be used.
fn paint_window(window: &Window) {
    let Some(store) = window.try_state::<SharedSettings>() else {
        return;
    };
    let system = match window.theme() {
        Ok(Theme::Light) => Scheme::Light,
        _ => Scheme::Dark,
    };
    let tried = tried_theme(window);
    let [red, green, blue] = {
        let store = lock(&store);
        tried.and_then(|id| store.theme_window(&id)).map_or_else(
            || store.window_color(store.forced_scheme().unwrap_or(system)),
            |(_, color)| color,
        )
    };
    let color = Color(red, green, blue, 0xff);
    let _ = window.set_background_color(Some(color));
    #[cfg(target_os = "macos")]
    color_title_bar(window, color);
}

/// Applies the appearance to what the webview can't draw: the window behind it, and whether
/// macOS draws the title bar's text and buttons for dark or for light.
fn repaint_windows(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(store) = handle.try_state::<SharedSettings>() else {
            return;
        };
        let tried = tried_theme(&handle);
        let scheme = {
            let store = lock(&store);
            tried
                .and_then(|id| store.theme_window(&id))
                .map(|(scheme, _)| scheme)
                .or_else(|| store.forced_scheme())
        };
        let theme = scheme.map(|scheme| match scheme {
            Scheme::Dark => Theme::Dark,
            Scheme::Light => Theme::Light,
        });
        for webview in handle.webview_windows().into_values() {
            let window = webview.as_ref().window();
            let _ = window.set_theme(theme);
            paint_window(&window);
        }
    });
}

/// Tauri's default macOS menu without Close Window, whose ⌘W would otherwise close the app
/// before the webview hears about it. That key closes a tab instead. The window's own close
/// button still works. Everything here keeps its usual system shortcut.
#[cfg(target_os = "macos")]
fn app_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let info = app.package_info();
    let config = app.config();
    let about = AboutMetadata {
        name: Some(info.name.clone()),
        version: Some(info.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config.bundle.publisher.clone().map(|name| vec![name]),
        ..Default::default()
    };
    Menu::with_items(
        app,
        &[
            &Submenu::with_items(
                app,
                info.name.clone(),
                true,
                &[
                    &PredefinedMenuItem::about(app, None, Some(about))?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::services(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::hide(app, None)?,
                    &PredefinedMenuItem::hide_others(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::quit(app, None)?,
                ],
            )?,
            &Submenu::with_items(
                app,
                "Edit",
                true,
                &[
                    &PredefinedMenuItem::undo(app, None)?,
                    &PredefinedMenuItem::redo(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::cut(app, None)?,
                    &PredefinedMenuItem::copy(app, None)?,
                    &PredefinedMenuItem::paste(app, None)?,
                    &PredefinedMenuItem::select_all(app, None)?,
                ],
            )?,
            &Submenu::with_items(
                app,
                "View",
                true,
                &[&PredefinedMenuItem::fullscreen(app, None)?],
            )?,
            &Submenu::with_id_and_items(
                app,
                WINDOW_SUBMENU_ID,
                "Window",
                true,
                &[
                    &PredefinedMenuItem::minimize(app, None)?,
                    &PredefinedMenuItem::maximize(app, None)?,
                ],
            )?,
            &Submenu::with_id_and_items(app, HELP_SUBMENU_ID, "Help", true, &[])?,
        ],
    )
}

fn main() {
    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder.menu(app_menu);
    builder
        .on_window_event(|window, event| match event {
            // AppKit can reset the title bar's views around a full screen transition,
            // which fires a resize, and when the window comes back to the front.
            WindowEvent::ThemeChanged(_) | WindowEvent::Focused(true) | WindowEvent::Resized(_) => {
                paint_window(window);
            }
            _ => {}
        })
        .setup(|app| {
            // The engine's connections run on Tauri's Tokio runtime.
            let engine = tauri::async_runtime::block_on(async {
                let engine = Engine::new("tectr-dsk");
                for url in daemon_urls() {
                    engine.add_daemon(&url);
                }
                engine
            });
            tauri::async_runtime::spawn(forward_updates(app.handle().clone(), engine.updates()));
            app.manage(engine);

            let home = dirs::home_dir().ok_or("couldn't find your home folder")?;
            // Dev builds keep their own file, so settings only dev knows don't show up as
            // problems in the installed app.
            let path = home
                .join(".config")
                .join(Channel::current().dir_name())
                .join("dsk.toml");
            let store = Arc::new(Mutex::new(Store::open(path)));
            match watch_settings(app.handle().clone(), store.clone()) {
                // Dropping the watcher would stop it, so it lives as long as the app.
                Ok(watcher) => {
                    app.manage(watcher);
                }
                Err(error) => lock(&store).add_notice(format!(
                    "Changes to the settings file need a restart to apply: {error}"
                )),
            }
            app.manage(store);
            app.manage(TriedTheme::default());
            // Running from source next to the installed app, the title says which is which.
            if Channel::current() == Channel::Dev {
                for window in app.webview_windows().into_values() {
                    let _ = window.set_title("tectr dev");
                }
            }
            repaint_windows(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            devices,
            request,
            open_session,
            events_after,
            settings,
            set_keybind,
            reset_keybind,
            set_appearance,
            reset_appearance,
            try_window_theme,
            installed_fonts,
            highlight_code,
            open_url,
            read_clipboard,
            write_clipboard
        ])
        .run(tauri::generate_context!())
        .expect("failed to run the tectr desktop app");
}
