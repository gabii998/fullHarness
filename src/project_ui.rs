use crate::{AppWindow, BuildCommandRow};
use ascurra_harness::{
    domain::{AppState, Provider},
    project_profile::{BuildCommand, ProjectProfile, import_rules},
    storage::Store,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{cell::RefCell, path::Path, rc::Rc};

fn load_rules(ui: &AppWindow, path: &Path, source: i32) {
    let providers: &[Provider] = match source {
        0 => &[Provider::Codex],
        1 => &[Provider::Claude],
        _ => &[Provider::Codex, Provider::Claude],
    };
    match import_rules(path, providers, &ui.get_project_rules()) {
        Ok((rules, added)) => {
            ui.set_project_rules(rules.into());
            ui.set_project_config_message(if added > 0 {
                "Reglas cargadas en el campo. Podés editarlas y guardar.".into()
            } else {
                "No hay reglas nuevas para cargar.".into()
            });
        }
        Err(error) => ui.set_project_config_message(error.into()),
    }
}

pub fn configure(ui: &AppWindow, state: Rc<RefCell<AppState>>, store: Rc<Store>) {
    let weak = ui.as_weak();
    let data = state.clone();
    ui.on_open_project_config(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let state = data.borrow();
        let Some(project) = state
            .selected_project
            .and_then(|id| state.projects.iter().find(|p| p.id == id))
        else {
            return;
        };
        let profile = &project.profile;
        ui.set_project_description(profile.description.as_str().into());
        ui.set_project_test_command(profile.test_command.as_str().into());
        ui.set_project_rules(profile.rules.as_str().into());
        ui.set_project_build_commands(ModelRc::new(VecModel::from(
            profile
                .builds
                .iter()
                .map(|b| BuildCommandRow {
                    command: b.command.as_str().into(),
                    purpose: b.purpose.as_str().into(),
                })
                .collect::<Vec<_>>(),
        )));
        ui.set_project_config_message("".into());
        if profile.rules.is_empty() {
            load_rules(&ui, Path::new(&project.path), 2);
        }
        ui.set_project_settings_view(true);
        ui.set_project_tasks_view(false);
    });
    let weak = ui.as_weak();
    let data = state.clone();
    ui.on_import_project_rules(move |source| {
        if let Some(ui) = weak.upgrade() {
            let state = data.borrow();
            if let Some(project) = state
                .selected_project
                .and_then(|id| state.projects.iter().find(|p| p.id == id))
            {
                load_rules(&ui, Path::new(&project.path), source);
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_add_build_command(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut rows = ui.get_project_build_commands().iter().collect::<Vec<_>>();
        if rows.len() >= 16 {
            ui.set_project_config_message("Máximo: 16 comandos.".into());
            return;
        }
        rows.push(BuildCommandRow::default());
        ui.set_project_build_commands(ModelRc::new(VecModel::from(rows)));
    });
    let weak = ui.as_weak();
    ui.on_remove_build_command(move |index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut rows = ui.get_project_build_commands().iter().collect::<Vec<_>>();
        if index >= 0 && (index as usize) < rows.len() {
            rows.remove(index as usize);
        }
        ui.set_project_build_commands(ModelRc::new(VecModel::from(rows)));
    });
    let weak = ui.as_weak();
    ui.on_edit_build_command(move |index, command, purpose| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let model = ui.get_project_build_commands();
        if let Some(rows) = model.as_any().downcast_ref::<VecModel<BuildCommandRow>>() {
            if index >= 0 && (index as usize) < rows.row_count() {
                rows.set_row_data(index as usize, BuildCommandRow { command, purpose });
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_save_project_config(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut profile = ProjectProfile {
            description: ui.get_project_description().to_string(),
            test_command: ui.get_project_test_command().to_string(),
            rules: ui.get_project_rules().to_string(),
            builds: ui
                .get_project_build_commands()
                .iter()
                .map(|r| BuildCommand {
                    command: r.command.to_string(),
                    purpose: r.purpose.to_string(),
                })
                .collect(),
        };
        if let Err(error) = profile.validate() {
            ui.set_project_config_message(error.into());
            return;
        }
        let mut state = state.borrow_mut();
        let Some(id) = state.selected_project else {
            return;
        };
        let Some(project) = state.projects.iter_mut().find(|p| p.id == id) else {
            return;
        };
        let before = std::mem::replace(&mut project.profile, profile);
        if let Err(error) = store.save(&state) {
            state
                .projects
                .iter_mut()
                .find(|p| p.id == id)
                .unwrap()
                .profile = before;
            ui.set_project_config_message(format!("No se pudo guardar: {error}").into());
        } else {
            ui.set_overview_description(
                state
                    .projects
                    .iter()
                    .find(|p| p.id == id)
                    .unwrap()
                    .profile
                    .description
                    .as_str()
                    .into(),
            );
            ui.set_project_config_message(
                "Configuración guardada. Se aplicará en la próxima ejecución.".into(),
            );
        }
    });
}
