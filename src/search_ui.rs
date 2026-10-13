use crate::{AppWindow, SearchRow};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{path::PathBuf, sync::{Arc, atomic::{AtomicU64, Ordering}}};

pub fn configure(ui: &AppWindow) {
    let generation = Arc::new(AtomicU64::new(0));
    let weak = ui.as_weak();
    ui.on_run_search(move || {
        let Some(ui) = weak.upgrade() else { return };
        let id = generation.fetch_add(1, Ordering::Relaxed) + 1;
        let root = ui.get_git_project().to_string();
        let query = ui.get_search_query().to_string().to_lowercase();
        let mode = ui.get_search_mode();
        ui.set_search_rows(ModelRc::new(VecModel::default()));
        if query.trim().is_empty() { ui.set_search_status("Escribe un texto para buscar.".into()); return; }
        ui.set_search_status("Buscando…".into());
        let weak = ui.as_weak();
        let generation = generation.clone();
        std::thread::spawn(move || {
            let mut rows = Vec::new();
            let mut pending = vec![PathBuf::from(&root)];
            let mut visited = 0;
            let mut skipped = 0;
            while let Some(dir) = pending.pop() {
                if generation.load(Ordering::Relaxed) != id { return; }
                let Ok(entries) = std::fs::read_dir(dir) else { skipped += 1; continue };
                for entry in entries.flatten() {
                    visited += 1;
                    if visited > 100_000 || rows.len() >= 500 { break; }
                    let Ok(kind) = entry.file_type() else { skipped += 1; continue };
                    let path = entry.path();
                    if kind.is_dir() {
                        if ![".git", "node_modules", "target", "vendor", ".idea"].contains(&entry.file_name().to_string_lossy().as_ref()) { pending.push(path); }
                        continue;
                    }
                    if !kind.is_file() { continue; }
                    let relative = path.strip_prefix(&root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                    if mode != 1 && relative.to_lowercase().contains(&query) {
                        rows.push((relative.clone(), 0, "Nombre de archivo".to_string()));
                    }
                    if mode == 0 { continue; }
                    if entry.metadata().map_or(true, |m| m.len() > 2 * 1024 * 1024) { skipped += 1; continue; }
                    let Ok(text) = std::fs::read_to_string(&path) else { skipped += 1; continue };
                    if text.contains('\0') { continue; }
                    for (line, text) in text.lines().enumerate() {
                        if text.to_lowercase().contains(&query) {
                            rows.push((relative.clone(), (line + 1) as i32, text.chars().take(240).collect()));
                            if rows.len() >= 500 { break; }
                        }
                    }
                }
                if visited > 100_000 || rows.len() >= 500 { break; }
            }
            let status = format!("{} resultados{} · {} archivos omitidos (binarios, grandes o inaccesibles)", rows.len(), if rows.len() >= 500 || visited > 100_000 { " (límite alcanzado)" } else { "" }, skipped);
            let _ = weak.upgrade_in_event_loop(move |ui| {
                if generation.load(Ordering::Relaxed) != id || ui.get_git_project().as_str() != root { return; }
                ui.set_search_rows(ModelRc::new(VecModel::from(rows.into_iter().map(|(path,line,text)| SearchRow { path: path.into(), line, text: text.into() }).collect::<Vec<_>>())));
                ui.set_search_status(status.into());
            });
        });
    });
}
