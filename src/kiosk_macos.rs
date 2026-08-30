//! Takes over the screen the way macOS's own lock screen does.
//!
//! Getting this right took a few separate pieces, each closing a different
//! escape hatch:
//!
//! 1. `Fullscreen::Borderless` + `WindowLevel::AlwaysOnTop` only resizes the
//!    window to the screen and floats it above *normal* windows -- the menu
//!    bar and Dock sit at higher `NSWindowLevel`s than "floating", so they
//!    stayed visible and clickable on top of it, and it never actually
//!    covered the display the way a real fullscreen app does. Fixed by
//!    promoting the window into a genuine native-fullscreen Space via
//!    `NSWindow.toggleFullScreen` -- the same mechanism the green traffic-
//!    light button uses -- which is what actually, reliably owns the whole
//!    display on modern macOS (empirically verified: on macOS 26, plain
//!    `NSApplicationPresentationOptions.HideMenuBar/.HideDock` without a
//!    real fullscreen transition do *not* reliably hide the menu bar).
//! 2. `NSWindow.toggleFullScreen` needs a `Regular`-policy, frontmost app
//!    (see window.rs for why the app is normally `Accessory`), so this
//!    switches to `Regular` + activates before transitioning, and back to
//!    `Accessory` after exiting so the app returns to hiding in the Dock.
//! 3. Cmd+Tab and Cmd+Opt+Esc are not affected by fullscreen on their own --
//!    apps can still be switched away from while fullscreen -- so
//!    `NSApplicationPresentationOptions` (`DisableProcessSwitching` /
//!    `DisableForceQuit` / `DisableSessionTermination`) still need to be
//!    set. Verified empirically that these survive the fullscreen
//!    transition (macOS merges them with its own `.FullScreen` flag rather
//!    than replacing them) as long as they're set *before* calling
//!    `toggleFullScreen`.
//! 4. Cmd+Q still worked on top of all that, because winit installs its own
//!    default app menu with a "Quit" item bound to `terminate:` / Cmd+Q,
//!    and that binding is a menu key equivalent, independent of both
//!    fullscreen state and `DisableProcessSwitching` (which only covers
//!    Cmd+Tab). Fixed by finding that menu item and disabling it while
//!    locked.
//!
//! Deliberately *not* touched: actual process termination. `kill`,
//! Activity Monitor's Force Quit button, and safe mode all bypass Cocoa's
//! menu/presentation-options/fullscreen layer entirely, which is exactly
//! the escape hatch this app's security design intentionally keeps open --
//! see the README.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationPresentationOptions, NSMenuItem,
    NSView, NSWindow, NSWindowCollectionBehavior,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// Above `kCGPopUpMenuWindowLevel` (101) and `kCGHelpWindowLevel` (200), on
/// par with where screen savers and screen-lock UIs place themselves. Only
/// matters for the brief moment before the native-fullscreen transition
/// below completes; the dedicated fullscreen Space handles the rest.
const SCREEN_SAVER_WINDOW_LEVEL: isize = 1000;

/// Call once the lock window is created.
pub fn engage(window: &Window) {
    let mtm = MainThreadMarker::new().expect("must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);

    // Must happen before both `setPresentationOptions` and
    // `toggleFullScreen` below -- an `Accessory` app can't reliably take
    // over the menu bar/Dock or enter a real fullscreen Space.
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.activate();

    // Set before the fullscreen transition: these flags survive it (merged
    // with the `.FullScreen` flag macOS adds itself), but setting them only
    // *after* would race the transition's own bookkeeping.
    app.setPresentationOptions(
        NSApplicationPresentationOptions::HideDock
            | NSApplicationPresentationOptions::HideMenuBar
            | NSApplicationPresentationOptions::DisableProcessSwitching
            | NSApplicationPresentationOptions::DisableForceQuit
            | NSApplicationPresentationOptions::DisableSessionTermination
            | NSApplicationPresentationOptions::DisableHideApplication,
    );

    if let Some(quit_item) = find_quit_menu_item(&app) {
        quit_item.setEnabled(false);
    }

    if let Some(ns_window) = ns_window(window) {
        ns_window.setLevel(SCREEN_SAVER_WINDOW_LEVEL);
        enter_native_fullscreen(&ns_window);
    }
}

/// Call when the lock window is about to close (before dropping it), so
/// normal desktop use resumes. Closing a window straight out of native
/// fullscreen is normal, supported AppKit behavior (the same thing happens
/// any time a fullscreen app is quit), so this doesn't bother calling
/// `toggleFullScreen` to exit first -- one less animated, asynchronous
/// transition to race against the window actually going away.
pub fn disengage() {
    let mtm = MainThreadMarker::new().expect("must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setPresentationOptions(NSApplicationPresentationOptions::Default);
    if let Some(quit_item) = find_quit_menu_item(&app) {
        quit_item.setEnabled(true);
    }
    // Back to not showing up in the Dock / Cmd+Tab while idling in the tray.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
}

fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    // SAFETY: `handle.ns_view` is a valid, live `NSView*` for as long as
    // `window` is alive, which outlives this call.
    let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
    view.window()
}

fn enter_native_fullscreen(ns_window: &NSWindow) {
    let behavior = ns_window.collectionBehavior();
    ns_window.setCollectionBehavior(behavior | NSWindowCollectionBehavior::FullScreenPrimary);
    let sender: Option<&AnyObject> = None;
    ns_window.toggleFullScreen(sender);
}

/// Walks the app's main menu (winit installs a standard one with an
/// "About/Hide/Quit" structure -- see winit's `platform_impl::macos::menu`)
/// looking for the item bound to the `terminate:` action, i.e. "Quit" /
/// Cmd+Q, regardless of its exact title (which is localized and includes
/// the process name).
fn find_quit_menu_item(app: &NSApplication) -> Option<Retained<NSMenuItem>> {
    let terminate: Sel = sel!(terminate:);
    let main_menu = app.mainMenu()?;
    for i in 0..main_menu.numberOfItems() {
        let item = main_menu.itemAtIndex(i)?;
        let Some(submenu) = item.submenu() else {
            continue;
        };
        for j in 0..submenu.numberOfItems() {
            let sub_item = submenu.itemAtIndex(j)?;
            if sub_item.action() == Some(terminate) {
                return Some(sub_item);
            }
        }
    }
    None
}
