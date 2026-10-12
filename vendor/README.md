# Local Slint patch

`i-slint-backend-winit` is vendored from the crates.io release 1.18.1. Original
source headers, manifest and license files are preserved. Cargo selects this
copy through `[patch.crates-io]`; the global Cargo cache is not modified.

One upstream source change is in `renderer/sw.rs`: Windows always presents
the complete existing software buffer on a requested redraw, including when the
Slint dirty region is empty. Windows GDI can clip a presentation while the
taskbar covers the window. A later expose repaint must restore those pixels
even when the model has not changed.

Partial scene rasterization is retained. The patch does not schedule additional
redraws, timers or continuous rendering. Other platforms retain upstream damage
presentation. This is a presentation fix, not an increase in UI frame rate.

`renderer/femtovg/glcontext.rs` also requests no window depth buffer on Windows
and prefers opaque GL configurations for opaque windows. FemtoVG disables depth
testing for its 2D renderer; stencil remains enabled for path fills. The picker
prefers fewer samples and fewer depth bits without changing other platforms.
This reduces unnecessary surface allocations, but does not eliminate the native
graphics driver's base memory cost.

The headless UI test verifies layout and pixel coverage only; it does not exercise
Windows taskbar occlusion. Native validation requires covering the window with
the taskbar, exposing it, and checking the previously covered area. Also check
minimize/restore and covering the application with another window.

Reassess this patch before upgrading Slint; remove the override when upstream
handles native expose repaints in the software presenter.
