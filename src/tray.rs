use anyhow::{Context, Result};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};
use winit::event_loop::EventLoopProxy;

use crate::window::UserEvent;

/// Keeps the tray icon alive; dropping this removes the icon from the menu
/// bar / system tray. All the actual settings (autostart, anti-spoof,
/// absence-lock, re-enrollment) live in the custom settings window now
/// rather than as native menu items, so there's nothing else to hold here.
pub struct Tray {
    _icon: TrayIcon,
}

pub fn build(proxy: EventLoopProxy<UserEvent>) -> Result<Tray> {
    let menu = Menu::new();
    let lock_item = MenuItem::new("Kilitle", true, None);
    let settings_item = MenuItem::new("Ayarlar…", true, None);
    let quit_item = MenuItem::new("Çıkış", true, None);

    menu.append(&lock_item)
        .context("failed to build tray menu")?;
    menu.append(&PredefinedMenuItem::separator())
        .context("failed to build tray menu")?;
    menu.append(&settings_item)
        .context("failed to build tray menu")?;
    menu.append(&PredefinedMenuItem::separator())
        .context("failed to build tray menu")?;
    menu.append(&quit_item)
        .context("failed to build tray menu")?;

    let lock_id = lock_item.id().clone();
    let settings_id = settings_item.id().clone();
    let quit_id = quit_item.id().clone();

    // tray-icon delivers menu clicks on its own background channel; forward
    // them into the winit event loop as UserEvents so all app state (and
    // every mutation of the menu itself, which muda requires happen on the
    // main thread) lives in one place, the ApplicationHandler in window.rs.
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let mapped = if event.id == lock_id {
            Some(UserEvent::LockRequested)
        } else if event.id == settings_id {
            Some(UserEvent::OpenSettingsRequested)
        } else if event.id == quit_id {
            Some(UserEvent::QuitRequested)
        } else {
            None
        };
        if let Some(mapped) = mapped {
            let _ = proxy.send_event(mapped);
        }
    }));

    let (rgba, size) = build_icon_rgba();
    let icon = Icon::from_rgba(rgba, size, size).context("failed to build tray icon image")?;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("OhMyLock")
        .with_icon(icon)
        .with_icon_as_template(true)
        .build()
        .context("failed to create tray icon")?;

    Ok(Tray { _icon: tray })
}

/// Draws a small face-scan glyph (viewfinder corners around a face outline)
/// as a standalone RGBA bitmap for the tray/menu-bar icon -- deliberately
/// not a padlock, so the menu bar icon reads as "face recognition" and the
/// padlock is reserved for the lock screen itself. Rendered as a solid
/// white silhouette and registered as a template image (see
/// `with_icon_as_template` above), so macOS re-tints it automatically for
/// light/dark menu bars and the highlighted state.
pub(crate) fn build_icon_rgba() -> (Vec<u8>, u32) {
    let size: u32 = 32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];

    let set = |rgba: &mut [u8], x: i32, y: i32| {
        if x < 0 || y < 0 || x >= size as i32 || y >= size as i32 {
            return;
        }
        let idx = ((y as u32 * size + x as u32) * 4) as usize;
        rgba[idx] = 255;
        rgba[idx + 1] = 255;
        rgba[idx + 2] = 255;
        rgba[idx + 3] = 255;
    };
    let fill_rect = |rgba: &mut [u8], x0: i32, y0: i32, x1: i32, y1: i32| {
        for y in y0..y1 {
            for x in x0..x1 {
                set(rgba, x, y);
            }
        }
    };

    // Viewfinder corner brackets.
    let margin = 3;
    let bracket_len = 8;
    let thickness = 2;
    let s = size as i32;
    for (cx, cy, dx, dy) in [
        (margin, margin, 1, 1),
        (s - margin, margin, -1, 1),
        (margin, s - margin, 1, -1),
        (s - margin, s - margin, -1, -1),
    ] {
        // Horizontal arm.
        fill_rect(
            &mut rgba,
            cx.min(cx + dx * bracket_len),
            cy.min(cy + dy * thickness),
            cx.max(cx + dx * bracket_len),
            cy.max(cy + dy * thickness),
        );
        // Vertical arm.
        fill_rect(
            &mut rgba,
            cx.min(cx + dx * thickness),
            cy.min(cy + dy * bracket_len),
            cx.max(cx + dx * thickness),
            cy.max(cy + dy * bracket_len),
        );
    }

    // Face outline: a ring-shaped ellipse plus two eye dots, sitting inside
    // the brackets.
    let (fcx, fcy) = (size as f32 / 2.0, size as f32 / 2.0 + 1.0);
    let (rx, ry) = (6.0f32, 7.0f32);
    let ring_inner = 0.72f32;
    for y in 0..size as i32 {
        for x in 0..size as i32 {
            let dx = (x as f32 + 0.5 - fcx) / rx;
            let dy = (y as f32 + 0.5 - fcy) / ry;
            let d2 = dx * dx + dy * dy;
            if d2 <= 1.0 && d2 >= ring_inner * ring_inner {
                set(&mut rgba, x, y);
            }
        }
    }
    for eye_x in [fcx - 2.6, fcx + 2.6] {
        let eye_y = fcy - 1.5;
        for y in 0..size as i32 {
            for x in 0..size as i32 {
                let dx = x as f32 + 0.5 - eye_x;
                let dy = y as f32 + 0.5 - eye_y;
                if dx * dx + dy * dy <= 1.1 * 1.1 {
                    set(&mut rgba, x, y);
                }
            }
        }
    }

    (rgba, size)
}
