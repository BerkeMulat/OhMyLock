use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Fullscreen, Window, WindowId, WindowLevel};

use crate::autostart;
use crate::enroll;
use crate::face_engine::FaceEngine;
use crate::lock::{self, AbsenceSentinel};
use crate::render::{self, LockStatus};
use crate::settings_render::{self, EnrollButtonState, Hitboxes, SettingsState};
use crate::storage::{self, EnrolledFace, Settings};
use crate::tray::{self, Tray};

/// Minimum time the lock window must stay up before it's allowed to close.
/// `kiosk_macos::engage` enters native fullscreen via an async, animated
/// `NSWindow.toggleFullScreen` transition; tearing the window down while
/// that's still in flight -- which happens whenever the enrolled face is
/// already looking at the camera the moment the lock screen opens, so the
/// matcher's very first poll is already a confident match -- leaves the
/// fullscreen Space in a broken state (a stuck lock screen) instead of
/// cleanly closing. This pads the close out past the animation so it never
/// races it.
///
/// Set generously because the *very first* `toggleFullScreen` of the
/// process -- i.e. exactly the "app just launched and starts locked, face
/// already in frame" case -- is slower than later ones: it's bundled with
/// the `Accessory` -> `Regular` activation-policy switch and the Dock/menu
/// bar hide, both done for the first time. A steady-state re-lock later in
/// the session settles well under this, but that just means the pad is a
/// no-op then -- it only ever costs time on the (rare) instant-match path,
/// never during a normal few-second wait for a face.
const MIN_LOCK_DURATION_BEFORE_CLOSE: Duration = Duration::from_millis(1500);

pub enum UserEvent {
    Status(LockStatus),
    Unlocked,
    LockRequested,
    OpenSettingsRequested,
    EnrollProgress(u32),
    EnrollFinished(std::result::Result<(), String>),
    QuitRequested,
}

/// Loads the enrolled face and runs the tray-icon app: idle in the menu bar
/// / system tray until "Kilitle" is chosen, then shows the fullscreen lock
/// screen until a verified face match closes it -- after which the app goes
/// back to idling in the tray, ready to lock again.
pub fn run(detector_path: PathBuf, embedder_path: PathBuf, antispoof_path: PathBuf) -> Result<()> {
    let enrolled =
        storage::load()?.context("no enrolled face found -- run with `--enroll` first")?;
    let settings = storage::load_settings().unwrap_or_default();

    let mut builder = EventLoop::<UserEvent>::with_user_event();
    // A menu-bar-only utility has no business in the Dock or Cmd+Tab
    // switcher; `Accessory` is what lets the tray icon and the fullscreen
    // lock window still work while keeping both hidden. Set here (rather
    // than relying solely on `LSUIElement` in an app bundle's Info.plist)
    // so it also applies when run as a bare binary during development.
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }
    let event_loop = builder.build().context("failed to create event loop")?;
    let proxy = event_loop.create_proxy();

    let mut app = App::new(
        proxy,
        detector_path,
        embedder_path,
        antispoof_path,
        enrolled,
        settings,
    );
    event_loop.run_app(&mut app).context("event loop error")?;
    Ok(())
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    detector_path: PathBuf,
    embedder_path: PathBuf,
    antispoof_path: PathBuf,
    enrolled: EnrolledFace,
    settings: Settings,
    /// Loaded lazily (on the first lock, or sooner if "Yüz görünmediğinde
    /// kilitle" is on) and kept alive for the rest of the process, instead
    /// of reloading the ONNX models on every use -- repeated load-then-drop
    /// churn was fragmenting the heap and creeping RSS up over the life of
    /// the process instead of returning to baseline.
    engine: Option<Arc<Mutex<FaceEngine>>>,
    /// Runs only while unlocked; stopped before any camera use that needs
    /// exclusive access (a lock session, a re-enrollment capture) and
    /// restarted afterward if the setting is still on.
    absence_sentinel: Option<AbsenceSentinel>,
    /// True for the duration of a re-enrollment capture -- blocks opening
    /// the lock screen or starting the absence sentinel, both of which
    /// would otherwise fight the capture for the camera.
    enrolling: bool,
    /// Set once the app has opened its startup lock screen, so `resumed`
    /// (which macOS can call more than once, e.g. on reactivation) only
    /// ever does that once.
    startup_lock_done: bool,

    tray: Option<Tray>,

    // Fullscreen lock screen window.
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    context: Option<softbuffer::Context<Rc<Window>>>,
    status: LockStatus,
    lock_started: Instant,
    /// When the status last changed, used to time the "wrong face" shake
    /// and flash so they read as a reaction rather than a constant wobble.
    status_started: Instant,
    /// Set once a verified match arrives; the lock window is torn down once
    /// `Instant::now()` passes this, instead of immediately. Checked on each
    /// event-loop tick rather than via `thread::sleep` on the main thread --
    /// blocking the main thread here would stall the run-loop-driven
    /// `NSWindow.toggleFullScreen` animation `kiosk_macos::engage` just
    /// kicked off, so `disengage` (called from `close_lock_window`) would
    /// tear the window down mid-animation and leave the fullscreen Space
    /// stuck instead of cleanly closing.
    pending_close: Option<Instant>,

    // Settings window.
    settings_window: Option<Rc<Window>>,
    settings_surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    settings_context: Option<softbuffer::Context<Rc<Window>>>,
    settings_cursor: (f32, f32),
    /// Where the last `redraw_settings` drew each control -- hit-tested
    /// against on click, so the click targets always match what's actually
    /// on screen instead of a second, driftable copy of the layout.
    settings_hitboxes: Option<Hitboxes>,
    enroll_state: EnrollButtonState,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    fn new(
        proxy: EventLoopProxy<UserEvent>,
        detector_path: PathBuf,
        embedder_path: PathBuf,
        antispoof_path: PathBuf,
        enrolled: EnrolledFace,
        settings: Settings,
    ) -> Self {
        Self {
            proxy,
            detector_path,
            embedder_path,
            antispoof_path,
            enrolled,
            settings,
            engine: None,
            absence_sentinel: None,
            enrolling: false,
            startup_lock_done: false,
            tray: None,
            window: None,
            surface: None,
            context: None,
            status: LockStatus::Scanning,
            lock_started: Instant::now(),
            status_started: Instant::now(),
            pending_close: None,
            settings_window: None,
            settings_surface: None,
            settings_context: None,
            settings_cursor: (0.0, 0.0),
            settings_hitboxes: None,
            enroll_state: EnrollButtonState::Idle,
        }
    }

    fn is_locked(&self) -> bool {
        self.window.is_some()
    }

    /// Returns the shared `FaceEngine`, loading it from disk on first use.
    /// Shared by the lock-screen matcher, the absence sentinel, and
    /// re-enrollment so the (fairly slow) ONNX session load only ever
    /// happens once per process.
    fn ensure_engine(&mut self) -> Option<Arc<Mutex<FaceEngine>>> {
        if let Some(engine) = &self.engine {
            return Some(engine.clone());
        }
        match FaceEngine::load(
            &self.detector_path,
            &self.embedder_path,
            &self.antispoof_path,
        ) {
            Ok(engine) => {
                let engine = Arc::new(Mutex::new(engine));
                self.engine = Some(engine.clone());
                Some(engine)
            }
            Err(err) => {
                eprintln!("failed to load face engine: {err:#}");
                None
            }
        }
    }

    /// Stops the absence sentinel and blocks until its camera handle is
    /// actually released -- called before any other camera use so two
    /// consumers never fight over the (typically single) camera device.
    fn stop_absence_sentinel(&mut self) {
        if let Some(sentinel) = self.absence_sentinel.take() {
            sentinel.stop();
        }
    }

    /// Starts the absence sentinel if the setting is on and nothing else
    /// currently needs the camera. Safe to call speculatively (e.g. after
    /// every state change that might newly allow it) since it no-ops if a
    /// sentinel is already running or the setting is off.
    fn start_absence_sentinel(&mut self) {
        if !self.settings.lock_on_absence
            || self.absence_sentinel.is_some()
            || self.is_locked()
            || self.enrolling
        {
            return;
        }
        let Some(engine) = self.ensure_engine() else {
            return;
        };
        self.absence_sentinel = Some(lock::spawn_absence_sentinel(engine, self.proxy.clone()));
    }

    fn open_lock_window(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() || self.enrolling {
            return;
        }
        self.stop_absence_sentinel();

        let attrs = Window::default_attributes()
            .with_title("")
            .with_fullscreen(Some(Fullscreen::Borderless(None)))
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_decorations(false)
            .with_resizable(false);
        let window = Rc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface =
            softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");
        window.set_cursor_visible(false);
        window.focus_window();
        // `Fullscreen::Borderless` + `WindowLevel::AlwaysOnTop` alone leaves
        // the menu bar and Dock visible and clickable on top of the lock
        // window (they sit at higher NSWindowLevels than "floating"), and
        // does nothing to stop Cmd+Tab -- so without this, the screen isn't
        // really fullscreen and the lock is trivially bypassable.
        #[cfg(target_os = "macos")]
        crate::kiosk_macos::engage(&window);

        self.window = Some(window);
        self.context = Some(context);
        self.surface = Some(surface);
        self.status = LockStatus::Scanning;
        self.lock_started = Instant::now();
        self.status_started = Instant::now();
        self.pending_close = None;

        let Some(engine) = self.ensure_engine() else {
            return;
        };
        lock::spawn_matcher(
            engine,
            self.enrolled.clone(),
            self.settings.clone(),
            self.proxy.clone(),
        );

        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(33),
        ));
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn close_lock_window(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(target_os = "macos")]
        crate::kiosk_macos::disengage();
        self.surface = None;
        self.context = None;
        self.window = None;
        // Idle in the tray: fully event-driven, no periodic wakeups needed
        // until the next "Kilitle" click or menu event arrives.
        event_loop.set_control_flow(ControlFlow::Wait);
        self.start_absence_sentinel();
    }

    fn redraw(&mut self) {
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return;
        };
        let size = window.inner_size();
        let (width, height) = (size.width.max(1), size.height.max(1));
        surface
            .resize(
                NonZeroU32::new(width).unwrap(),
                NonZeroU32::new(height).unwrap(),
            )
            .unwrap();

        let mut buffer = surface.buffer_mut().unwrap();
        let t = self.lock_started.elapsed().as_secs_f32();
        let status_t = self.status_started.elapsed().as_secs_f32();

        render::render(&mut buffer, width, height, self.status, t, status_t);

        buffer.present().unwrap();
    }

    fn open_settings_window(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.settings_window {
            window.focus_window();
            window.request_redraw();
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Ayarlar")
            .with_inner_size(winit::dpi::LogicalSize::new(420.0, 460.0))
            .with_resizable(false);
        let window = match event_loop.create_window(attrs) {
            Ok(window) => Rc::new(window),
            Err(err) => {
                eprintln!("failed to create settings window: {err:#}");
                return;
            }
        };
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface =
            softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");
        window.focus_window();
        self.settings_window = Some(window);
        self.settings_context = Some(context);
        self.settings_surface = Some(surface);
        self.settings_hitboxes = None;
        if let Some(window) = &self.settings_window {
            window.request_redraw();
        }
    }

    fn close_settings_window(&mut self) {
        self.settings_surface = None;
        self.settings_context = None;
        self.settings_window = None;
        self.settings_hitboxes = None;
    }

    fn redraw_settings(&mut self) {
        let (Some(window), Some(surface)) = (&self.settings_window, &mut self.settings_surface)
        else {
            return;
        };
        let size = window.inner_size();
        let (width, height) = (size.width.max(1), size.height.max(1));
        surface
            .resize(
                NonZeroU32::new(width).unwrap(),
                NonZeroU32::new(height).unwrap(),
            )
            .unwrap();

        let mut buffer = surface.buffer_mut().unwrap();
        let state = SettingsState {
            autostart: autostart::is_enabled(),
            antispoof_enabled: self.settings.antispoof_enabled,
            lock_on_absence: self.settings.lock_on_absence,
            enroll: self.enroll_state.clone(),
        };
        let hitboxes = settings_render::render(&mut buffer, width, height, &state);
        buffer.present().unwrap();
        self.settings_hitboxes = Some(hitboxes);
    }

    fn handle_settings_click(&mut self, x: f32, y: f32) {
        let Some(hitboxes) = &self.settings_hitboxes else {
            return;
        };

        if hitboxes.autostart_toggle.contains(x, y) {
            let want_enabled = !autostart::is_enabled();
            let result = if want_enabled {
                autostart::enable()
            } else {
                autostart::disable()
            };
            if let Err(err) = result {
                eprintln!("failed to update launch-at-login: {err:#}");
            }
            self.redraw_settings();
        } else if hitboxes.antispoof_toggle.contains(x, y) {
            self.settings.antispoof_enabled = !self.settings.antispoof_enabled;
            if let Err(err) = storage::save_settings(&self.settings) {
                eprintln!("failed to save settings: {err:#}");
            }
            self.redraw_settings();
        } else if hitboxes.lock_on_absence_toggle.contains(x, y) {
            self.settings.lock_on_absence = !self.settings.lock_on_absence;
            if let Err(err) = storage::save_settings(&self.settings) {
                eprintln!("failed to save settings: {err:#}");
            }
            if self.settings.lock_on_absence {
                self.start_absence_sentinel();
            } else {
                self.stop_absence_sentinel();
            }
            self.redraw_settings();
        } else if hitboxes.reenroll_button.contains(x, y) {
            self.start_reenroll();
        }
    }

    fn start_reenroll(&mut self) {
        if self.enrolling || self.is_locked() {
            return;
        }
        self.stop_absence_sentinel();
        let Some(engine) = self.ensure_engine() else {
            self.enroll_state = EnrollButtonState::Failed("Yüz modeli yüklenemedi".to_string());
            self.redraw_settings();
            return;
        };
        self.enrolling = true;
        self.enroll_state = EnrollButtonState::Verifying;
        self.redraw_settings();
        enroll::spawn_reenroll(engine, self.enrolled.clone(), self.proxy.clone());
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.tray.is_none() {
            match tray::build(self.proxy.clone()) {
                Ok(tray) => self.tray = Some(tray),
                Err(err) => eprintln!("failed to create tray icon: {err:#}"),
            }
        }
        event_loop.set_control_flow(ControlFlow::Wait);
        if !self.startup_lock_done {
            self.startup_lock_done = true;
            self.open_lock_window(event_loop);
        } else {
            self.start_absence_sentinel();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let is_lock_window = self.window.as_ref().map(|w| w.id()) == Some(id);
        let is_settings_window = self.settings_window.as_ref().map(|w| w.id()) == Some(id);

        match event {
            WindowEvent::CloseRequested => {
                if is_settings_window {
                    self.close_settings_window();
                }
                // Lock window: ignored on purpose. The lock screen only
                // closes via a verified face match. The OS's own process
                // manager (Task Manager / Activity Monitor / kill) is the
                // one thing that can always stop this app -- see README.
            }
            WindowEvent::RedrawRequested => {
                if is_lock_window {
                    self.redraw();
                    if self.is_locked() {
                        event_loop.set_control_flow(ControlFlow::WaitUntil(
                            Instant::now() + Duration::from_millis(33),
                        ));
                    }
                } else if is_settings_window {
                    self.redraw_settings();
                }
            }
            WindowEvent::Resized(_) => {
                if is_lock_window {
                    self.redraw();
                } else if is_settings_window {
                    self.redraw_settings();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if is_settings_window {
                    self.settings_cursor = (position.x as f32, position.y as f32);
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if is_settings_window {
                    let (x, y) = self.settings_cursor;
                    self.handle_settings_click(x, y);
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Status(status) => {
                if self.status != status {
                    self.status = status;
                    self.status_started = Instant::now();
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            UserEvent::Unlocked => {
                let target = self.lock_started + MIN_LOCK_DURATION_BEFORE_CLOSE;
                if Instant::now() >= target {
                    self.close_lock_window(event_loop);
                } else {
                    // Defer the close instead of blocking here: the redraw
                    // loop below (already ticking at ~33ms while locked)
                    // picks this up in `about_to_wait` once `target` passes,
                    // so the run loop keeps servicing the in-flight
                    // fullscreen animation instead of stalling it.
                    self.pending_close = Some(target);
                }
            }
            UserEvent::LockRequested => {
                self.open_lock_window(event_loop);
            }
            UserEvent::OpenSettingsRequested => {
                self.open_settings_window(event_loop);
            }
            UserEvent::EnrollProgress(captured) => {
                self.enroll_state = EnrollButtonState::InProgress {
                    captured,
                    total: enroll::SAMPLES as u32,
                };
                self.redraw_settings();
            }
            UserEvent::EnrollFinished(result) => {
                self.enrolling = false;
                match result {
                    Ok(()) => {
                        if let Ok(Some(face)) = storage::load() {
                            self.enrolled = face;
                        }
                        self.enroll_state = EnrollButtonState::Done;
                    }
                    Err(msg) => {
                        eprintln!("re-enrollment failed: {msg}");
                        self.enroll_state = EnrollButtonState::Failed(msg);
                    }
                }
                self.redraw_settings();
                self.start_absence_sentinel();
            }
            UserEvent::QuitRequested => {
                // Quitting the whole app while the lock screen is active
                // would be an easy bypass, so it's only honored while
                // unlocked.
                if !self.is_locked() {
                    event_loop.exit();
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(target) = self.pending_close
            && Instant::now() >= target
        {
            self.pending_close = None;
            self.close_lock_window(event_loop);
            return;
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}
