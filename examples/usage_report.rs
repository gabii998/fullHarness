//! Read-only verification against existing application data. No model/API requests.
use ascurra_harness::{storage::Store, usage_metrics};
use slint::ComponentHandle;
use std::{path::PathBuf, rc::Rc};
slint::include_modules!();
#[allow(dead_code)]
#[path = "../src/usage_ui.rs"]
mod usage_ui;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    let data = args
        .get(1)
        .map(PathBuf::from)
        .ok_or("Indicá el directorio de datos del harness.")?;
    let state = Store::new(data.clone())?.load()?;
    let project = state
        .projects
        .iter()
        .find(|p| {
            Some(p.id) == state.selected_project
                || p.chats.iter().any(|c| Some(c.id) == state.selected_chat)
        })
        .or_else(|| state.projects.first())
        .ok_or("No hay proyectos guardados.")?;
    let chats = project
        .chats
        .iter()
        .map(|c| (c.id, c.title.clone()))
        .collect::<Vec<_>>();
    let samples = project
        .chats
        .iter()
        .flat_map(|c| c.usage.iter().cloned().map(move |u| (c.id, u)))
        .collect::<Vec<_>>();
    let report = usage_metrics::scan(&data, &chats, &samples);
    let path = usage_metrics::save(&data, &project.path, "project", &report)?;
    println!(
        "{}",
        serde_json::json!({"project":project.name,"logs":report.files,"calls":report.calls.len(),"legacy_calls":report.legacy_calls,"models":report.models,"partial":report.partial,"invalid_lines":report.invalid_lines,"report":path,"top_tools":report.groups.iter().take(5).collect::<Vec<_>>()})
    );
    if args.iter().any(|a| a == "--preview") {
        slint::BackendSelector::new()
            .renderer_name("software".into())
            .select()?;
        let ui = AppWindow::new()?;
        ui.set_has_project(true);
        ui.set_overview_name(project.name.clone().into());
        ui.set_overview_path(project.path.clone().into());
        ui.set_usage_tab_open(true);
        ui.set_usage_open(true);
        usage_ui::display(&ui, &report);
        let weak = ui.as_weak();
        let timer = Rc::new(slint::Timer::default());
        timer.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_millis(800),
            move || {
                if let Some(ui) = weak.upgrade() {
                    if let Ok(snapshot) = ui.window().take_snapshot() {
                        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                            .join("target/ui-usage-real.png");
                        let _ = image::save_buffer(
                            path,
                            snapshot.as_bytes(),
                            snapshot.width(),
                            snapshot.height(),
                            image::ColorType::Rgba8,
                        );
                    }
                    let _ = ui.hide();
                }
                let _ = slint::quit_event_loop();
            },
        );
        ui.run()?;
    }
    Ok(())
}
