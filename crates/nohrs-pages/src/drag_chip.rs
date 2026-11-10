//! The chip that follows the pointer while a tab is dragged.
//!
//! gpui draws a drag's preview inside the window it started in, so the tab
//! vanished at the window's edge — just where a tab is taken to tear it off.
//! This is a small window of its own instead: on top of everything, never
//! focused, moved to the pointer every frame, so it crosses the window's
//! edge and the monitors. It closes itself once the mouse button is up,
//! wherever that happens.

use std::cell::RefCell;
use std::time::Duration;

use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Task, Window, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions, div, point, px, rgb, size,
};
use nohrs_ui::theme::theme;

thread_local! {
    static CHIP: RefCell<Option<(AnyWindowHandle, Task<()>)>> = const { RefCell::new(None) };
}

struct Chip {
    title: SharedString,
}

impl Render for Chip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .px(px(3.0))
            .py(px(4.0))
            .flex()
            .items_center()
            .child(
                div()
                    .w_full()
                    .h_full()
                    .px(px(12.0))
                    .rounded(px(8.0))
                    .flex()
                    .items_center()
                    .bg(rgb(theme::toolbar_active_bg()))
                    .text_color(rgb(theme::toolbar_active_text()))
                    .text_sm()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(self.title.clone()),
            )
    }
}

/// Shows the chip for a tab called `title` under the pointer.
pub fn show(title: SharedString, cx: &mut App) {
    hide(cx);
    let Some(cursor) = platform::cursor() else {
        return;
    };
    // Wide enough for the name; the chip's own box is drawn inside it.
    let width = (title.chars().count() as f32 * 8.5 + 34.0).clamp(72.0, 360.0);
    let scale = platform::scale_at(cursor).max(1.0);
    let origin = point(
        px(cursor.0 as f32 / scale + 14.0),
        px(cursor.1 as f32 / scale + 10.0),
    );
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            origin,
            size(px(width), px(38.0)),
        ))),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        display_id: cx
            .displays()
            .iter()
            .find(|screen| screen.bounds().contains(&origin))
            .map(|screen| screen.id()),
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        platform::keep_on_top(window);
        cx.new(|_| Chip { title })
    });
    let Ok(handle) = opened else {
        return;
    };
    let handle: AnyWindowHandle = handle.into();
    let follow = cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(12))
                .await;
            if !platform::left_button_down() {
                cx.update(hide).ok();
                return;
            }
            let moved = handle.update(cx, |_, window, _| platform::follow_cursor(window));
            if moved.is_err() {
                return;
            }
        }
    });
    CHIP.with(|chip| *chip.borrow_mut() = Some((handle, follow)));
}

/// Closes the chip, if one is shown.
pub fn hide(cx: &mut App) {
    let Some((handle, _follow)) = CHIP.with(|chip| chip.borrow_mut().take()) else {
        return;
    };
    if handle
        .update(cx, |_, window, _| window.remove_window())
        .is_err()
    {
        tracing::debug!("the drag chip was already closed");
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod platform {
    use windows::Win32::Foundation::{HWND, POINT};
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOSIZE, SetWindowPos,
    };

    pub fn cursor() -> Option<(i32, i32)> {
        let mut point = POINT::default();
        unsafe { GetCursorPos(&mut point) }.ok()?;
        Some((point.x, point.y))
    }

    // The scale of the monitor under `cursor`, to turn its device pixels
    // into the logical ones gpui places windows in.
    pub fn scale_at(cursor: (i32, i32)) -> f32 {
        let monitor = unsafe {
            MonitorFromPoint(
                POINT {
                    x: cursor.0,
                    y: cursor.1,
                },
                MONITOR_DEFAULTTONEAREST,
            )
        };
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        if unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }.is_err()
        {
            return 1.0;
        }
        dpi_x as f32 / 96.0
    }

    pub fn left_button_down() -> bool {
        (unsafe { GetAsyncKeyState(i32::from(VK_LBUTTON.0)) } as u16 & 0x8000) != 0
    }

    fn hwnd(window: &gpui::Window) -> Option<HWND> {
        crate::ipc::window_hwnd(window).map(|handle| HWND(handle as *mut _))
    }

    pub fn keep_on_top(window: &gpui::Window) {
        let Some(hwnd) = hwnd(window) else { return };
        if let Err(error) = unsafe {
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE,
            )
        } {
            tracing::debug!("drag chip on top: {error}");
        }
    }

    // In device pixels, which is what the cursor and SetWindowPos both use,
    // so the chip stays on the pointer across monitors of any scale.
    pub fn follow_cursor(window: &gpui::Window) {
        let (Some(hwnd), Some((x, y))) = (hwnd(window), cursor()) else {
            return;
        };
        let scale = scale_at((x, y));
        let (dx, dy) = ((14.0 * scale) as i32, (10.0 * scale) as i32);
        if let Err(error) = unsafe {
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                x + dx,
                y + dy,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE,
            )
        } {
            tracing::debug!("drag chip move: {error}");
        }
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn cursor() -> Option<(i32, i32)> {
        None
    }
    pub fn scale_at(_cursor: (i32, i32)) -> f32 {
        1.0
    }
    pub fn left_button_down() -> bool {
        false
    }
    pub fn keep_on_top(_window: &gpui::Window) {}
    pub fn follow_cursor(_window: &gpui::Window) {}
}
