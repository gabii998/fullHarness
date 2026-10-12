use crate::{AppWindow, FileRow, FileTabRow, PreviewLine};
use ascurra_harness::{
    explorer::{self, Tree},
    runtime::Supervisor,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    rc::Rc,
    sync::Arc,
};

const LINE_CACHE_LIMIT: usize = 128;
pub fn bounded_lines(text: slint::SharedString) -> (ModelRc<PreviewLine>, i32) {
    let model = PreviewLines::new(text);
    let columns = model.columns.min(i32::MAX as usize) as i32;
    (ModelRc::new(Rc::new(model)), columns)
}

// The source text is shared with its tab. Index newline offsets, and materialize
// only the rows requested by the virtualized ListView, with a bounded scroll cache.
struct PreviewLines {
    text: slint::SharedString,
    offsets: Vec<u32>,
    columns: usize,
    kinds: Vec<u8>,
    cache: RefCell<(HashMap<usize, PreviewLine>, VecDeque<usize>)>,
    tracker: slint::ModelNotify,
}

impl PreviewLines {
    fn new(text: slint::SharedString) -> Self {
        Self::with_diff(text, false)
    }
    fn with_diff(text: slint::SharedString, diff: bool) -> Self {
        let mut offsets = vec![0];
        let mut columns = 0;
        let mut current = 0;
        for (offset, ch) in text.char_indices() {
            if ch == '\n' {
                offsets.push((offset + 1) as u32);
                columns = columns.max(current);
                current = 0;
            } else if ch == '\t' {
                current += 4 - current % 4;
            } else if ch != '\r' {
                current += if ch.is_ascii() { 1 } else { 2 };
            }
        }
        let mut in_hunk = false;
        let kinds = if diff {
            text.split('\n')
                .map(|line| {
                    if line.starts_with("diff --git ") {
                        in_hunk = false;
                        return 3;
                    }
                    if line.starts_with("@@ ") || line.starts_with("@@@ ") {
                        in_hunk = true;
                        return 3;
                    }
                    if in_hunk && line.starts_with('+') {
                        1
                    } else if in_hunk && line.starts_with('-') {
                        2
                    } else if !in_hunk && (line.starts_with("+++ ") || line.starts_with("--- ")) {
                        3
                    } else {
                        0
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        Self {
            text,
            kinds,
            offsets,
            columns: columns.max(current),
            cache: RefCell::new((HashMap::new(), VecDeque::new())),
            tracker: Default::default(),
        }
    }
}

impl Model for PreviewLines {
    type Data = PreviewLine;
    fn row_count(&self) -> usize {
        self.offsets.len()
    }
    fn row_data(&self, row: usize) -> Option<Self::Data> {
        let start = *self.offsets.get(row)? as usize;
        let mut cache = self.cache.borrow_mut();
        if let Some(text) = cache.0.get(&row) {
            return Some(text.clone());
        }
        let end = self
            .offsets
            .get(row + 1)
            .map(|offset| (*offset as usize).saturating_sub(1))
            .unwrap_or(self.text.len());
        let line = self.text[start..end].trim_end_matches('\r');
        let expanded: slint::SharedString = if line.contains('\t') {
            let mut text = String::new();
            let mut column = 0;
            for ch in line.chars() {
                if ch == '\t' {
                    let spaces = 4 - column % 4;
                    text.extend(std::iter::repeat_n(' ', spaces));
                    column += spaces;
                } else {
                    text.push(ch);
                    column += if ch.is_ascii() { 1 } else { 2 };
                }
            }
            text.into()
        } else {
            slint::SharedString::from(line)
        };
        if cache.0.len() >= LINE_CACHE_LIMIT {
            if let Some(old) = cache.1.pop_front() {
                cache.0.remove(&old);
            }
        }
        let expanded = PreviewLine {
            text: expanded,
            kind: self.kinds.get(row).copied().unwrap_or(0) as i32,
        };
        cache.0.insert(row, expanded.clone());
        cache.1.push_back(row);
        Some(expanded)
    }
    fn model_tracker(&self) -> &dyn slint::ModelTracker {
        &self.tracker
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_colors_only_patch_lines_and_keeps_file_headers_distinct() {
        let text: slint::SharedString = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n+++literal content\n context\ndiff --git a/b b/b\n--- a/b\n+++ b/b".into();
        let diff = PreviewLines::with_diff(text.clone(), true);
        let kinds = (0..diff.row_count())
            .map(|row| diff.row_data(row).unwrap().kind)
            .collect::<Vec<_>>();
        assert_eq!(kinds, vec![3, 3, 3, 3, 2, 1, 1, 0, 3, 3, 3]);
        let plain = PreviewLines::new(text);
        assert!((0..plain.row_count()).all(|row| plain.row_data(row).unwrap().kind == 0));
    }
    #[test]
    fn preview_preserves_unicode_tabs_and_bounds_cache_after_scrolling() {
        let model = PreviewLines::new("á\tx\r\n\nfinal\n".into());
        assert_eq!(model.row_count(), 4);
        assert_eq!(model.row_data(0).unwrap().text, "á  x");
        assert_eq!(model.row_data(1).unwrap().text, "");
        assert_eq!(model.row_data(2).unwrap().text, "final");
        assert_eq!(model.row_data(3).unwrap().text, "");
        assert!(model.row_data(4).is_none());

        let text = (0..12_000)
            .map(|row| format!("fila {row}\n"))
            .collect::<String>();
        let model = PreviewLines::new(text.into());
        assert!(
            model.cache.borrow().0.is_empty(),
            "Abrir no materializa todas las líneas"
        );
        for row in 0..12_000 {
            assert_eq!(
                model.row_data(row).unwrap().text.as_str(),
                format!("fila {row}")
            );
        }
        assert!(model.cache.borrow().0.len() <= LINE_CACHE_LIMIT);
        assert_eq!(
            model.row_data(0).unwrap().text,
            "fila 0",
            "Una fila desalojada puede volverse a leer"
        );
    }
}

fn display(ui: &AppWindow, tree: &Tree) {
    ui.set_explorer_rows(ModelRc::new(VecModel::from(
        tree.entries
            .iter()
            .map(|entry| FileRow {
                path: entry.path.clone().into(),
                name: entry.name.clone().into(),
                directory: entry.directory,
                expanded: entry.expanded,
                depth: entry.depth as i32,
            })
            .collect::<Vec<_>>(),
    )));
    ui.set_explorer_status(if tree.entries.is_empty() {
        "Carpeta vacía".into()
    } else {
        format!(
            "{} entradas visibles · clic en un archivo para leerlo",
            tree.entries.len()
        )
        .into()
    });
}

fn show_file(ui: &AppWindow, tab: &FileTabRow) {
    ui.set_file_target_line(0);
    ui.invoke_activate_file_project(tab.project.clone());
    ui.set_file_preview_project(tab.project.clone());
    ui.set_file_preview_path(tab.path.clone());
    set_preview_text(ui, tab.text.clone());
    ui.set_file_preview_status(tab.status.clone());
    ui.set_file_preview_open(true);
}

pub(crate) fn update_open_file(ui: &AppWindow, tab: &FileTabRow) {
    if ui.get_file_preview_open()
        && ui.get_file_preview_project() == tab.project
        && ui.get_file_preview_path() == tab.path
    {
        set_preview_text(ui, tab.text.clone());
        ui.set_file_preview_status(tab.status.clone());
    }
}

fn set_preview_text(ui: &AppWindow, text: slint::SharedString) {
    let project = ui.get_file_preview_project();
    let project = project.strip_prefix(r"\\?\").unwrap_or(&project);
    ui.set_file_preview_location(
        std::path::Path::new(project)
            .join(ui.get_file_preview_path().as_str())
            .to_string_lossy()
            .replace('/', "\\")
            .into(),
    );
    let lines = if ui.get_file_preview_path().starts_with("[Git]/") {
        PreviewLines::with_diff(text.clone(), true)
    } else {
        PreviewLines::new(text.clone())
    };
    ui.set_file_preview_columns(lines.columns as i32);
    ui.set_file_preview_lines(ModelRc::new(lines));
    ui.set_file_preview_text(text);
}

fn query(ui: &AppWindow, service: &Arc<Supervisor>, path: String, kind: &'static str) {
    ui.set_explorer_busy(true);
    ui.set_explorer_status("Cargando…".into());
    let root = ui.get_git_project().to_string();
    let handle = ui.as_weak();
    service.executor.spawn_blocking(move || {
        let result = if kind == "preview" {
            explorer::preview(std::path::Path::new(&root), &path)
                .and_then(|preview| serde_json::to_value(preview).map_err(|e| e.to_string()))
        } else {
            explorer::list(std::path::Path::new(&root), &path)
                .and_then(|entries| serde_json::to_value(entries).map_err(|e| e.to_string()))
        };
        let event = match result {
            Ok(result) => serde_json::json!({"root":root,"path":path,"kind":kind,"result":result}),
            Err(error) => serde_json::json!({"root":root,"path":path,"kind":kind,"error":error}),
        }
        .to_string();
        let _ = handle.upgrade_in_event_loop(move |ui| ui.invoke_explorer_event(event.into()));
    });
}

pub fn configure(ui: &AppWindow, service: Arc<Supervisor>) {
    let weak = ui.as_weak();
    ui.on_select_file_tab(move |project, path| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if let Some(tab) = ui
            .get_file_tabs()
            .iter()
            .find(|tab| tab.project == project && tab.path == path)
        {
            show_file(&ui, &tab);
        }
    });
    let weak = ui.as_weak();
    ui.on_close_file_tab(move |project, path| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let active = ui.get_file_preview_open()
            && ui.get_file_preview_project() == project
            && ui.get_file_preview_path() == path;
        let tabs = ui
            .get_file_tabs()
            .iter()
            .filter(|tab| tab.project != project || tab.path != path)
            .collect::<Vec<_>>();
        ui.set_file_tabs(ModelRc::new(VecModel::from(tabs.clone())));
        if active {
            if let Some(tab) = tabs.last() {
                show_file(&ui, tab);
            } else {
                ui.set_file_preview_open(false);
                set_preview_text(&ui, "".into());
            }
        }
    });
    let trees = Rc::new(RefCell::new(HashMap::<String, Tree>::new()));
    let pending = Rc::new(Cell::new(false));
    let weak = ui.as_weak();
    let cache = trees.clone();
    let queued = pending.clone();
    let executor = service.clone();
    ui.on_refresh_explorer(move |reload| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if !ui.get_files_open() {
            return;
        }
        let root = ui.get_git_project().to_string();
        if root.is_empty() {
            ui.set_explorer_status("Abrí un proyecto para explorar sus archivos.".into());
            return;
        }
        if ui.get_explorer_busy() {
            queued.set(true);
            return;
        }
        if !reload {
            if let Some(tree) = cache.borrow().get(&root) {
                display(&ui, tree);
                return;
            }
        }
        ui.set_explorer_rows(ModelRc::new(VecModel::from(Vec::<FileRow>::new())));
        query(&ui, &executor, String::new(), "root");
    });

    let weak = ui.as_weak();
    let cache = trees.clone();
    let executor = service.clone();
    ui.on_open_problem_file({
        let weak = ui.as_weak();
        let service = service.clone();
        move |path, line| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let root = ui.get_git_project().to_string();
            let full = std::path::Path::new(&root).join(path.as_str());
            let Ok(canonical) = full.canonicalize() else {
                ui.set_lint_status("No se encontró el archivo.".into());
                return;
            };
            let Ok(base) = std::path::Path::new(&root).canonicalize() else {
                return;
            };
            let Ok(relative) = canonical.strip_prefix(&base) else {
                ui.set_lint_status("El diagnóstico apunta fuera del proyecto.".into());
                return;
            };
            let path = relative.to_string_lossy().replace('\\', "/");
            let mut tabs = ui.get_file_tabs().iter().collect::<Vec<_>>();
            let tab = if let Some(tab) = tabs
                .iter()
                .find(|t| t.project.as_str() == root && t.path.as_str() == path)
            {
                tab.clone()
            } else {
                let tab = FileTabRow {
                    project: root.into(),
                    path: path.clone().into(),
                    title: canonical
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .as_ref()
                        .into(),
                    text: "".into(),
                    status: "Leyendo archivo…".into(),
                };
                tabs.push(tab.clone());
                ui.set_file_tabs(ModelRc::new(VecModel::from(tabs)));
                tab
            };
            show_file(&ui, &tab);
            ui.set_file_target_line(line);
            if tab.text.is_empty() {
                query(&ui, &service, path, "preview");
            }
        }
    });
    ui.on_select_file(move |path| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_explorer_busy() {
            return;
        }
        let root = ui.get_git_project().to_string();
        let path = path.to_string();
        let mut cache = cache.borrow_mut();
        let Some(tree) = cache.get_mut(&root) else {
            return;
        };
        let Some(entry) = tree
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .cloned()
        else {
            return;
        };
        if entry.directory {
            if tree.collapse(&path) {
                display(&ui, tree);
                return;
            }
            if entry.depth >= 24 {
                ui.set_explorer_status("La vista admite hasta 24 niveles de carpetas.".into());
                return;
            }
            query(&ui, &executor, path, "directory");
        } else {
            let mut tabs = ui.get_file_tabs().iter().collect::<Vec<_>>();
            if let Some(tab) = tabs
                .iter()
                .find(|tab| tab.project.as_str() == root && tab.path.as_str() == path)
            {
                show_file(&ui, tab);
                return;
            }
            let tab = FileTabRow {
                project: root.clone().into(),
                path: path.clone().into(),
                title: entry.name.into(),
                text: "".into(),
                status: "Leyendo archivo…".into(),
            };
            tabs.push(tab.clone());
            ui.set_file_tabs(ModelRc::new(VecModel::from(tabs)));
            show_file(&ui, &tab);
            query(&ui, &executor, path, "preview");
        }
    });

    let weak = ui.as_weak();
    ui.on_explorer_event(move |event| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Ok(event) = serde_json::from_str::<serde_json::Value>(&event) else {
            return;
        };
        ui.set_explorer_busy(false);
        let root = event["root"].as_str().unwrap_or("");
        let path = event["path"].as_str().unwrap_or("");
        let current = ui.get_git_project().as_str() == root;
        let preview = event["kind"].as_str() == Some("preview");
        if preview {
            let mut tabs = ui.get_file_tabs().iter().collect::<Vec<_>>();
            if let Some(tab) = tabs
                .iter_mut()
                .find(|tab| tab.project.as_str() == root && tab.path.as_str() == path)
            {
                if let Some(error) = event["error"].as_str() {
                    tab.status = error.into();
                } else if let Ok(preview) =
                    serde_json::from_value::<explorer::Preview>(event["result"].clone())
                {
                    tab.text = preview.text.into();
                    tab.status = if preview.truncated {
                        "Solo lectura · vista limitada a los primeros 128 KiB"
                    } else {
                        "Solo lectura · texto UTF-8"
                    }
                    .into();
                }
                if ui.get_file_preview_project().as_str() == root
                    && ui.get_file_preview_path().as_str() == path
                    && ui.get_file_preview_open()
                {
                    set_preview_text(&ui, tab.text.clone());
                    ui.set_file_preview_status(tab.status.clone());
                }
            }
            ui.set_file_tabs(ModelRc::new(VecModel::from(tabs)));
            if current {
                if let Some(tree) = trees.borrow().get(root) {
                    display(&ui, tree);
                }
            }
        } else if let Some(error) = event["error"].as_str() {
            if current {
                ui.set_explorer_status(error.into());
            }
        } else if let Ok(entries) =
            serde_json::from_value::<Vec<explorer::Entry>>(event["result"].clone())
        {
            let mut cache = trees.borrow_mut();
            let result = if event["kind"].as_str() == Some("root") {
                cache.insert(root.to_string(), Tree { entries });
                Ok(())
            } else {
                cache
                    .get_mut(root)
                    .ok_or("El proyecto ya no está en la vista.".to_string())
                    .and_then(|tree| tree.insert(path, entries))
            };
            if current {
                match result {
                    Ok(()) => {
                        if let Some(tree) = cache.get(root) {
                            display(&ui, tree);
                        }
                    }
                    Err(error) => ui.set_explorer_status(error.into()),
                }
            }
        }
        if pending.replace(false) || !current {
            ui.invoke_refresh_explorer(false);
        }
    });

    let weak = ui.as_weak();
    ui.on_open_project_folder(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let path = ui.get_git_project().to_string();
        if path.is_empty() {
            return;
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            if std::process::Command::new("explorer.exe")
                .arg(path)
                .creation_flags(0x08000000)
                .spawn()
                .is_err()
            {
                ui.set_explorer_status("No se pudo abrir el Explorador de Windows.".into());
            }
        }
    });
}
