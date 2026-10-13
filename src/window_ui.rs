use crate::AppWindow;
use slint::{ComponentHandle, winit_030::WinitWindowAccessor};

pub fn configure(ui: &AppWindow, profiler: crate::scroll_profile::Handle) {
    let weak = ui.as_weak();
    ui.on_window_minimize(move || {
        if let Some(ui) = weak.upgrade() {
            ui.window().with_winit_window(|window| window.set_minimized(true));
        }
    });
    let weak = ui.as_weak();
    ui.on_window_maximize(move || {
        if let Some(ui) = weak.upgrade() {
            ui.window().with_winit_window(|window| window.set_maximized(!window.is_maximized()));
        }
    });
    let weak = ui.as_weak();
    ui.on_window_drag(move || {
        if let Some(ui) = weak.upgrade() {
            ui.window().with_winit_window(|window| { let _ = window.drag_window(); });
        }
    });
    let weak = ui.as_weak();
    ui.window().on_winit_window_event(move |window, event| {
        if profiler.enabled() {
            if let Some(ui) = weak.upgrade() { profiler.event(&ui, window, event); }
        }
        // Do not query native window state on every wheel/cursor/redraw event.
        if !matches!(event, slint::winit_030::winit::event::WindowEvent::Resized(_) | slint::winit_030::winit::event::WindowEvent::ScaleFactorChanged { .. }) {
            return slint::winit_030::EventResult::Propagate;
        }
        if let Some(ui) = weak.upgrade() {
                        window.with_winit_window(|native| {
                let maximized = native.is_maximized();
                if ui.get_window_maximized() != maximized {
                    ui.set_window_maximized(maximized);
                }
            });
        }
        slint::winit_030::EventResult::Propagate
    });
}

// Only prettify the caption; filesystem operations keep the original path.
pub fn display_path(path: &str) -> String {
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", unc)
    } else if let Some(local) = path.strip_prefix(r"\\?\") {
        local.to_owned()
    } else {
        path.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::display_path;
    #[test]
    fn caption_hides_extended_prefix_and_preserves_network_paths() {
        assert_eq!(display_path(r"\\?\C:\Users\project"), r"C:\Users\project");
        assert_eq!(display_path(r"\\?\UNC\server\share"), r"\\server\share");
        assert_eq!(display_path(r"C:\project"), r"C:\project");
        assert_eq!(display_path("/home/project"), "/home/project");
    }
}