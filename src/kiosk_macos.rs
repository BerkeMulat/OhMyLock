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
//! 5. Mission Control (F3, or its Control+Up/Control+Down alternates for
//!    Mission Control / App Exposé) is *not* covered by any
//!    `NSApplicationPresentationOptions` flag -- unlike Cmd+Tab, there's no
//!    documented AppKit-level switch for it. Invoking it doesn't close the
//!    lock window, but it does let someone swipe away to another Space and
//!    use the rest of the machine while the lock window sits fullscreen in
//!    the background, which defeats the point just as much as actually
//!    closing it would. The only way to stop it is lower-level: a
//!    `CGEventTap` (see `ensure_key_tap_enabled` below) installed at
//!    `kCGHIDEventTap`, right at the start of the event pipeline, that
//!    swallows the F3 / Control+Up / Control+Down key-down before macOS's
//!    own symbolic-hotkey dispatch ever sees it -- the same category of
//!    mechanism Cmd+Tab's own system-level binding uses internally, just
//!    not one AppKit exposes a flag for. This needs the process to be
//!    trusted for Accessibility (System Settings > Privacy & Security >
//!    Accessibility); without it, `CGEventTapCreate` simply returns `NULL`
//!    and this step silently no-ops, leaving every other piece of hardening
//!    on this list intact.
//!
//! Deliberately *not* touched: actual process termination. `kill`,
//! Activity Monitor's Force Quit button, and safe mode all bypass Cocoa's
//! menu/presentation-options/fullscreen layer entirely, which is exactly
//! the escape hatch this app's security design intentionally keeps open --
//! see the README.

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationPresentationOptions, NSMenuItem,
    NSView, NSWindow, NSWindowCollectionBehavior,
};
use objc2_core_foundation::{CFMachPort, CFRetained, CFRunLoop, kCFRunLoopCommonModes};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventMask, CGEventTapLocation, CGEventTapOptions,
    CGEventTapPlacement, CGEventTapProxy, CGEventType,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// Carbon `kVK_F3` -- the physical key macOS ships bound to Mission Control
/// by default (its glyph on Apple keyboards since the Aluminum era).
const KEYCODE_F3: i64 = 0x63;
/// Carbon `kVK_UpArrow` / `kVK_DownArrow` -- Control+Up (Mission Control)
/// and Control+Down (App Exposé) are the other out-of-the-box shortcuts for
/// the same "show me every window" overlays F3 opens, so both are blocked
/// alongside it rather than leaving them as an unblocked side door.
const KEYCODE_UP_ARROW: i64 = 0x7E;
const KEYCODE_DOWN_ARROW: i64 = 0x7D;

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

    ensure_key_tap_enabled();

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
    disable_key_tap();
    // Back to not showing up in the Dock / Cmd+Tab while idling in the tray.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
}

thread_local! {
    /// The Mission-Control key tap, created lazily on the first `engage()`
    /// and then kept for the life of the process -- only *enabled* state
    /// tracks lock/unlock (see `ensure_key_tap_enabled`/`disable_key_tap`),
    /// mirroring how `window.rs` keeps the `FaceEngine` loaded once rather
    /// than reloading it every lock cycle. `thread_local!` (rather than a
    /// `static`) is enough synchronization here because every call site --
    /// `engage`, `disengage`, and the tap's own callback -- runs on the
    /// main thread, the same one `MainThreadMarker` already gates the rest
    /// of this module on.
    static KEY_TAP: RefCell<Option<CFRetained<CFMachPort>>> = const { RefCell::new(None) };
}

/// Installs (on first call) and enables the Mission-Control key tap
/// described in point 5 of the module docs above. Safe to call every time
/// the lock window opens: once installed the tap is kept and just
/// re-enabled, and if installation failed for lack of Accessibility
/// permission, this retries it -- so granting that permission while the app
/// is already running takes effect on the very next lock, no restart
/// needed.
fn ensure_key_tap_enabled() {
    KEY_TAP.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(tap) = slot.as_ref() {
            CGEvent::tap_enable(tap, true);
            return;
        }

        let events_of_interest: CGEventMask = 1u64 << CGEventType::KeyDown.0;
        // SAFETY: `key_tap_callback` matches `CGEventTapCallBack`'s exact
        // signature and never unwinds (it's a plain `extern "C-unwind" fn`
        // with no panicking code path); `user_info` is unused so a null
        // pointer is valid.
        let tap = unsafe {
            CGEvent::tap_create(
                CGEventTapLocation::HIDEventTap,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                events_of_interest,
                Some(key_tap_callback),
                std::ptr::null_mut(),
            )
        };
        let Some(tap) = tap else {
            eprintln!(
                "OhMyLock: Mission Control (F3) key blocker could not be installed -- grant Accessibility access in System Settings > Privacy & Security > Accessibility, then lock again. Every other kiosk protection (fullscreen, Cmd+Tab, Cmd+Q) is unaffected."
            );
            return;
        };
        if let Some(source) = CFMachPort::new_run_loop_source(None, Some(&tap), 0)
            && let Some(run_loop) = CFRunLoop::current()
        {
            // SAFETY: reading this extern static just copies an immutable,
            // process-lifetime `CFRunLoopMode` reference; nothing mutates it.
            run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
        }
        CGEvent::tap_enable(&tap, true);
        *slot = Some(tap);
    });
}

/// Disables (without destroying) the Mission-Control key tap. Called from
/// `disengage` so normal desktop use -- including Mission Control itself --
/// works again as soon as the lock window closes.
fn disable_key_tap() {
    KEY_TAP.with(|slot| {
        if let Some(tap) = slot.borrow().as_ref() {
            CGEvent::tap_enable(tap, false);
        }
    });
}

/// The tap callback: runs on the main thread's run loop for every key-down
/// system-wide while the tap is enabled. Returns `NULL` to swallow an event
/// (Apple's documented way to remove it from the stream) or the event
/// pointer unchanged to let it continue on to its normal destination.
unsafe extern "C-unwind" fn key_tap_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: NonNull<CGEvent>,
    _user_info: *mut c_void,
) -> *mut CGEvent {
    if event_type == CGEventType::TapDisabledByTimeout
        || event_type == CGEventType::TapDisabledByUserInput
    {
        // macOS auto-disables a tap it judges too slow to keep up with
        // (or that was disabled programmatically) -- re-enable immediately
        // so a transient stall can't permanently reopen this escape hatch
        // for the rest of the lock session.
        KEY_TAP.with(|slot| {
            if let Some(tap) = slot.borrow().as_ref() {
                CGEvent::tap_enable(tap, true);
            }
        });
        return event.as_ptr();
    }

    if event_type == CGEventType::KeyDown {
        // SAFETY: the tap callback contract guarantees `event` is a valid,
        // live `CGEvent` for the duration of this call.
        let event_ref = unsafe { event.as_ref() };
        let keycode = CGEvent::integer_value_field(Some(event_ref), CGEventField::KeyboardEventKeycode);
        let flags = CGEvent::flags(Some(event_ref));
        let is_mission_control = keycode == KEYCODE_F3
            || (flags.contains(CGEventFlags::MaskControl)
                && (keycode == KEYCODE_UP_ARROW || keycode == KEYCODE_DOWN_ARROW));
        if is_mission_control {
            return std::ptr::null_mut();
        }
    }

    event.as_ptr()
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
