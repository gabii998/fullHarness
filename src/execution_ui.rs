use crate::AppWindow;
use ascurra_harness::{
    project_execution::{self, Configuration, Manager},
    runtime::Supervisor,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

fn configurations(ui: &AppWindow, data: &Path) -> Result<Vec<Configuration>, String> {
    project_execution::load(data, Path::new(ui.get_git_project().as_str()))
}
fn load(ui: &AppWindow, data: &Path) {
    match configurations(ui, data) {
        Ok(c) => {
            let project = ui.get_git_project().to_string();
            if !project_execution::has_saved_configurations(data, Path::new(&project)) {
                if let Err(e) = project_execution::save(data, Path::new(&project), &c) {
                    ui.set_execution_message(e.into());
                    return;
                }
            }
            ui.set_execution_names(ModelRc::new(VecModel::from(
                c.iter().map(|c| c.name.clone().into()).collect::<Vec<_>>(),
            )));
            ui.set_execution_index(0);
            ui.set_execution_config_json(serde_json::to_string_pretty(&c).unwrap().into());
            ui.set_execution_message(
                format!(
                    "{} configuraciones · Node / Spring / Android. No se ejecutó ningún comando.",
                    c.len()
                )
                .into(),
            );
        }
        Err(e) => {
            ui.set_execution_names(ModelRc::default());
            ui.set_execution_message(e.into());
        }
    }
    refresh(ui);
}
fn refresh(ui: &AppWindow) {
    if !ui.get_execution_open() || ui.get_git_project().is_empty() {
        return;
    }
    let root = ui.get_git_project().to_string();
    let statuses = Manager::shared()
        .statuses(Path::new(&root))
        .unwrap_or_default();
    let ids = statuses
        .iter()
        .map(|s| slint::SharedString::from(s.id.clone()))
        .collect::<Vec<_>>();
    let names = statuses
        .iter()
        .map(|s| {
            format!(
                "{} · {} · {} s{}",
                s.configuration,
                s.state,
                s.elapsed_seconds,
                s.exit_code
                    .map(|c| format!(" · salida {c}"))
                    .unwrap_or_default()
            )
            .into()
        })
        .collect::<Vec<slint::SharedString>>();
    let previous = ui
        .get_execution_run_ids()
        .row_data(ui.get_execution_run_index().max(0) as usize);
    let index = previous
        .and_then(|id| ids.iter().position(|v| v == &id))
        .unwrap_or(ids.len().saturating_sub(1));
    ui.set_execution_run_ids(ModelRc::new(VecModel::from(ids)));
    ui.set_execution_run_names(ModelRc::new(VecModel::from(names)));
    ui.set_execution_run_index(index as i32);
    if let Some(s) = statuses.get(index) {
        let stream = if ui.get_execution_stderr() {
            "stderr"
        } else {
            "stdout"
        };
        let tail = Manager::shared()
            .tail(Path::new(&root), &s.id, stream)
            .unwrap_or_else(|e| e);
        if ui.get_execution_output().as_str() != tail {
            ui.set_execution_output(tail.into());
        }
    } else {
        ui.set_execution_output("Sin ejecuciones en este proyecto.".into());
    }
}
pub fn configure(ui: &AppWindow, supervisor: Arc<Supervisor>, data: PathBuf) {
    let timer = Rc::new(slint::Timer::default());
    let weak = ui.as_weak();
    let tick = ui.as_weak();
    let root = data.clone();
    let clock = timer.clone();
    ui.on_load_execution(move || {
        let Some(ui) = weak.upgrade() else { return };
        if ui.get_git_project().is_empty() {
            clock.stop();
            ui.set_execution_names(ModelRc::default());
            ui.set_execution_run_names(ModelRc::default());
            ui.set_execution_run_ids(ModelRc::default());
            ui.set_execution_output("Seleccioná un proyecto.".into());
            return;
        }
        load(&ui, &root);
        let tick = tick.clone();
        let data = root.clone();
        clock.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(500),
            move || {
                if let Some(ui) = tick.upgrade() {
                    if ui.get_execution_open() {
                        if let Ok(configs) = configurations(&ui, &data) {
                            let names = configs
                                .iter()
                                .map(|c| slint::SharedString::from(c.name.clone()))
                                .collect::<Vec<_>>();
                            if ui.get_execution_names().iter().collect::<Vec<_>>() != names {
                                ui.set_execution_names(ModelRc::new(VecModel::from(names)));
                                if ui.get_execution_index() as usize >= configs.len() {
                                    ui.set_execution_index(0);
                                }
                            }
                            if !ui.get_execution_config_open() {
                                ui.set_execution_config_json(
                                    serde_json::to_string_pretty(&configs).unwrap().into(),
                                );
                            }
                        }
                    }
                    refresh(&ui);
                }
            },
        );
    });
    let weak = ui.as_weak();
    let executor = supervisor.clone();
    ui.on_execution_devices(move || {
        let Some(ui) = weak.upgrade() else {return};
        let Some(adb) = ascurra_harness::logcat::detect_adb() else {ui.set_execution_message("No se encontró ADB. Configurá ANDROID_HOME/ANDROID_SDK_ROOT o PATH.".into());return};
        if ui.get_execution_busy() {return;}
        ui.set_execution_busy(true);
        let handle = ui.as_weak(); let project = ui.get_git_project().to_string();
        executor.executor.spawn(async move {
            let devices = ascurra_harness::logcat::devices(&adb).await;
            let _ = handle.upgrade_in_event_loop(move |ui| {
                ui.set_execution_busy(false);
                if ui.get_git_project().as_str() != project {return;}
                match devices {
                    Ok(devices) => {
                        let devices = devices.into_iter().filter(|d|d.available).collect::<Vec<_>>();
                        ui.set_execution_device_names(ModelRc::new(VecModel::from(devices.iter().map(|d|d.label.clone().into()).collect::<Vec<_>>())));
                        ui.set_execution_device_ids(ModelRc::new(VecModel::from(devices.iter().map(|d|d.serial.clone().into()).collect::<Vec<_>>())));
                        ui.set_execution_device_index(0);
                        ui.set_execution_android_device(devices.first().map(|d|d.serial.clone()).unwrap_or_default().into());
                        ui.set_execution_message(format!("{} dispositivos autorizados. Los offline/no autorizados no son seleccionables.",devices.len()).into());
                    },
                    Err(e) => ui.set_execution_message(e.into()),
                }
            });
        });
    });
    let weak = ui.as_weak();
    ui.on_choose_execution_device(move || {
        if let Some(ui) = weak.upgrade() {
            if let Some(id) = ui
                .get_execution_device_ids()
                .row_data(ui.get_execution_device_index().max(0) as usize)
            {
                ui.set_execution_android_device(id);
            }
        }
    });
    let weak = ui.as_weak();
    let root = data.clone();
    ui.on_save_execution(move || {
        let Some(ui) = weak.upgrade() else { return };
        let text = ui.get_execution_config_json().to_string();
        let result = if text.len() > 256 * 1024 {
            Err("Configuración demasiado grande.".into())
        } else {
            serde_json::from_str::<Vec<Configuration>>(&text)
                .map_err(|e| e.to_string())
                .and_then(|c| {
                    project_execution::save(&root, Path::new(ui.get_git_project().as_str()), &c)
                })
        };
        match result {
            Ok(()) => load(&ui, &root),
            Err(e) => ui.set_execution_message(e.into()),
        }
    });
    let weak = ui.as_weak();
    let root = data.clone();
    ui.on_detect_execution(move || {
        let Some(ui) = weak.upgrade() else { return };
        let project = ui.get_git_project().to_string();
        match project_execution::detect(Path::new(&project)).and_then(|detected| {
            let mut c = project_execution::load(&root, Path::new(&project))?;
            for d in detected {
                if !c.iter().any(|c| c.id == d.id) {
                    c.push(d);
                }
            }
            project_execution::save(&root, Path::new(&project), &c)
        }) {
            Ok(()) => load(&ui, &root),
            Err(e) => ui.set_execution_message(e.into()),
        }
    });
    let weak = ui.as_weak();
    let root = data.clone();
    ui.on_save_android_execution(move || {
        let Some(ui) = weak.upgrade() else { return };
        let project = ui.get_git_project().to_string();
        let result = (|| {
            let mut configs = configurations(&ui, &root)?;
            let base = configs
                .get(ui.get_execution_index().max(0) as usize)
                .cloned()
                .ok_or("Seleccioná una configuración Gradle Android.")?;
            if !base.program.contains("gradlew") {
                return Err("Seleccioná una configuración Gradle Android.".into());
            }
            let module = ui.get_execution_android_module().to_string();
            let variant = ui.get_execution_android_variant().to_string();
            let serial = ui.get_execution_android_device().to_string();
            let component = ui.get_execution_android_component().to_string();
            if !variant.chars().all(|c| c.is_ascii_alphanumeric())
                || variant.is_empty()
                || !module
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':' || c == '-')
            {
                return Err("Módulo o variante inválidos.".into());
            }
            let prefix = if module.is_empty() {
                String::new()
            } else {
                format!(":{}:", module.trim_matches(':'))
            };
            let mut adb = Vec::new();
            if !serial.is_empty() {
                adb.extend(["-s".into(), serial.clone()]);
            }
            let mut presets = Vec::new();
            for (key, task) in [
                ("build", format!("assemble{variant}")),
                ("install", format!("install{variant}")),
                ("test", format!("test{variant}UnitTest")),
                (
                    "instrumented-test",
                    format!("connected{variant}AndroidTest"),
                ),
            ] {
                let mut c = base.clone();
                c.id = format!("android:{key}");
                c.name = format!("Android · {module} · {task}");
                c.args = vec![format!("{prefix}{task}")];
                if !serial.is_empty() {
                    c.env.insert("ANDROID_SERIAL".into(), serial.clone());
                    c.args
                        .push(format!("-Pandroid.injected.device.serial={serial}"));
                }
                presets.push(c);
            }
            if !component.is_empty() {
                let package = component.split('/').next().unwrap_or("");
                if !component.contains('/') || component.starts_with('-') || package.is_empty() {
                    return Err("Componente esperado: paquete/actividad.".into());
                }
                for (key, args) in [
                    (
                        "launch",
                        vec!["shell", "am", "start", "-n", component.as_str()],
                    ),
                    ("stop-app", vec!["shell", "am", "force-stop", package]),
                ] {
                    let mut argv = adb.clone();
                    argv.extend(args.into_iter().map(String::from));
                    presets.push(Configuration {
                        id: format!("android:{key}"),
                        name: format!("Android · {key} · {component}"),
                        program: "adb".into(),
                        args: argv,
                        directory: base.directory.clone(),
                        env: Default::default(),
                    });
                }
            }
            for c in presets {
                configs.retain(|old| old.id != c.id);
                configs.push(c);
            }
            project_execution::save(&root, Path::new(&project), &configs)
        })();
        match result {
            Ok(()) => load(&ui, &root),
            Err(e) => ui.set_execution_message(e.into()),
        }
    });
    let weak = ui.as_weak();
    ui.on_execution_action(move |action| {
        let Some(ui) = weak.upgrade() else { return };
        if ui.get_execution_busy() {
            return;
        }
        let project = ui.get_git_project().to_string();
        let action = action.to_string();
        let configuration = if action == "stop" {
            None
        } else {
            match configurations(&ui, &data).and_then(|c| {
                c.get(ui.get_execution_index().max(0) as usize)
                    .cloned()
                    .ok_or("Seleccioná una configuración.".into())
            }) {
                Ok(c) => Some(c),
                Err(e) => {
                    ui.set_execution_message(e.into());
                    return;
                }
            }
        };
        let run = ui
            .get_execution_run_ids()
            .row_data(ui.get_execution_run_index().max(0) as usize)
            .map(|s| s.to_string());
        let handle = ui.as_weak();
        let data = data.clone();
        ui.set_execution_busy(true);
        supervisor.executor.spawn_blocking(move || {
            let m = Manager::shared();
            let result = (|| {
                if action == "stop" {
                    m.stop(
                        Path::new(&project),
                        run.as_deref().ok_or("Seleccioná una ejecución.")?,
                    )?;
                    return Ok("Deteniendo ejecución…".to_string());
                }
                let c = configuration.ok_or("Configuración ausente.")?;
                if action == "restart" {
                    for s in m.statuses(Path::new(&project))?.into_iter().filter(|s| {
                        s.configuration == c.id
                            && matches!(s.state.as_str(), "running" | "stopping")
                    }) {
                        m.stop(Path::new(&project), &s.id)?;
                    }
                    for _ in 0..100 {
                        if !m.statuses(Path::new(&project))?.iter().any(|s| {
                            s.configuration == c.id
                                && matches!(s.state.as_str(), "running" | "stopping")
                        }) {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                }
                m.start(Path::new(&project), &data, c)
                    .map(|id| format!("Ejecución iniciada: {id}"))
            })();
            let _ = handle.upgrade_in_event_loop(move |ui| {
                ui.set_execution_busy(false);
                if ui.get_git_project().as_str() == project {
                    ui.set_execution_message(result.unwrap_or_else(|e: String| e).into());
                    refresh(&ui);
                }
            });
        });
    });
}
