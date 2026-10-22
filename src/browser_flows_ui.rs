use crate::{AppWindow, BrowserStepRow};
use ascurra_harness::{
    browser_flows::{self, Flow, Step},
    captures,
    runtime::Supervisor,
};
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Default)]
struct Selection {
    project: String,
    flows: Vec<Flow>,
    current: Option<Flow>,
    pending: Option<(String, String, String)>,
}
fn render(ui: &AppWindow, flow: &Flow) {
    ui.set_flow_steps(ModelRc::new(VecModel::from(
        flow.steps
            .iter()
            .enumerate()
            .map(|(index, s)| BrowserStepRow {
                index: index as i32,
                included: s.included,
                title: format!(
                    "{}. {} · {} · {} ms",
                    index + 1,
                    s.action,
                    if s.artifact.is_empty() {
                        "Manual"
                    } else if s.ok {
                        "OK"
                    } else {
                        "Error"
                    },
                    s.duration_ms
                )
                .into(),
                detail: if s.action == "fill" {
                    format!("{} ← env:{}", s.target, s.value).into()
                } else {
                    format!("{} {}", s.target, s.value).into()
                },
            })
            .collect::<Vec<_>>(),
    )));
    ui.set_flow_code("".into());
    ui.set_flow_preview_open(false);
}
pub fn configure(ui: &AppWindow, service: Arc<Supervisor>, data: PathBuf) {
    let selection = Arc::new(Mutex::new(Selection::default()));
    let generation = Arc::new(AtomicU64::new(0));
    let weak = ui.as_weak();
    let selected = selection.clone();
    let serial = generation.clone();
    let disk = data.clone();
    let executor = service.clone();
    ui.on_refresh_flows(move || {
        let Some(ui) = weak.upgrade() else { return };
        let project = ui.get_git_project().to_string();
        let id = serial.fetch_add(1, Ordering::Relaxed)+1;
        *selected.lock().unwrap() = Selection { project: project.clone(), ..Default::default() };
        ui.set_flow_names(ModelRc::default()); ui.set_flow_steps(ModelRc::default()); ui.set_flow_image(slint::Image::default());
        ui.set_flow_preview_open(false); ui.set_flow_detail("".into()); ui.set_flow_index(-1);
        if project.is_empty() { ui.set_flow_notice("Seleccioná un proyecto.".into()); return; }
        ui.set_flow_notice("Buscando flujos…".into());
        let handle = ui.as_weak(); let selected = selected.clone(); let disk = disk.clone(); let serial = serial.clone();
        executor.executor.spawn(async move {
            let path = PathBuf::from(&project);
            let loaded = tokio::task::spawn_blocking(move || browser_flows::list(&disk, &path)).await;
            let _ = handle.upgrade_in_event_loop(move |ui| {
                if serial.load(Ordering::Relaxed)!=id || ui.get_git_project().as_str()!=project || !ui.get_flows_open() {return;}
                match loaded {
                    Ok(Ok(flows)) => {
                        ui.set_flow_names(ModelRc::new(VecModel::from(flows.iter().map(|f| slint::SharedString::from(format!("Chat {} · {} pasos · {}",f.chat,f.steps.len(),&f.id[..f.id.len().min(8)]))).collect::<Vec<_>>())));
                        ui.set_flow_notice(format!("{} flujos (hasta 100). Actualizar descarta ediciones locales. Valores fill: variables de entorno.",flows.len()).into());
                        selected.lock().unwrap().flows = flows;
                        if !selected.lock().unwrap().flows.is_empty() { ui.set_flow_index(0); ui.invoke_pick_flow(0); }
                    },
                    Ok(Err(e)) => ui.set_flow_notice(e.into()),
                    Err(_) => ui.set_flow_notice("No se pudieron leer los flujos.".into()),
                }
            });
        });
    });
    let weak = ui.as_weak();
    let selected = selection.clone();
    let serial = generation.clone();
    ui.on_pick_flow(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        let mut state = selected.lock().unwrap();
        serial.fetch_add(1, Ordering::Relaxed);
        if state.project != ui.get_git_project().as_str() {
            return;
        }
        let Some(flow) = state.flows.get(index.max(0) as usize).cloned() else {
            return;
        };
        let first_url = flow
            .steps
            .iter()
            .find(|s| s.action == "navigate")
            .map(|s| s.value.as_str())
            .unwrap_or_default();
        let base = reqwest::Url::parse(first_url)
            .ok()
            .map(|u| u.origin().ascii_serialization())
            .unwrap_or_default();
        ui.set_flow_name(format!("Browser flow {}", &flow.id[..flow.id.len().min(8)]).into());
        ui.set_flow_path(
            format!("browser-flow-{}.spec.ts", &flow.id[..flow.id.len().min(8)]).into(),
        );
        ui.set_flow_base_url(base.into());
        ui.set_flow_detail(
            format!(
                "Chat {} · creado {} · {} pasos",
                flow.chat,
                flow.created,
                flow.steps.len()
            )
            .into(),
        );
        ui.set_flow_image(slint::Image::default());
        render(&ui, &flow);
        state.current = Some(flow);
        state.pending = None;
    });
    let weak = ui.as_weak();
    let selected = selection.clone();
    ui.on_toggle_flow_step(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        let mut state = selected.lock().unwrap();
        if state.project != ui.get_git_project().as_str() {
            return;
        }
        if let Some(flow) = &mut state.current {
            if let Some(step) = flow.steps.get_mut(index.max(0) as usize) {
                step.included = !step.included;
            }
            render(&ui, flow);
        }
        state.pending = None;
    });
    let weak = ui.as_weak();
    let selected = selection.clone();
    ui.on_add_flow_assertion(move || {
        let Some(ui) = weak.upgrade() else { return };
        let mut state = selected.lock().unwrap();
        if state.project != ui.get_git_project().as_str() {
            return;
        }
        let Some(flow) = &mut state.current else {
            return;
        };
        let value = ui.get_flow_assert_value().to_string();
        let target = ui.get_flow_assert_target().to_string();
        if value.len() > 8192 || target.len() > 1024 || flow.steps.len() >= 2000 {
            ui.set_flow_notice("Verificación demasiado grande.".into());
            return;
        }
        let action = match ui.get_flow_assert_kind() {
            1 => "assert_url",
            2 => "assert_visible",
            _ => "assert_text",
        };
        if (action == "assert_visible" && (target.is_empty() || target.starts_with('@')))
            || (action != "assert_visible" && value.is_empty())
        {
            ui.set_flow_notice(
                "Ingresá el valor esperado o un selector CSS estable (no @refs).".into(),
            );
            return;
        }
        flow.steps.push(Step {
            action: action.into(),
            target,
            value,
            locator: Value::Null,
            ok: true,
            included: true,
            duration_ms: 0,
            artifact: String::new(),
            error: "Verificación manual; se ejecutará en el test exportado.".into(),
        });
        render(&ui, flow);
        state.pending = None;
    });
    let weak = ui.as_weak();
    let selected = selection.clone();
    let disk = data.clone();
    let executor = service.clone();
    let serial = generation.clone();
    ui.on_pick_flow_step(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        let state = selected.lock().unwrap();
        if state.project != ui.get_git_project().as_str() {
            return;
        }
        let Some(flow) = &state.current else { return };
        let Some(step) = flow.steps.get(index.max(0) as usize) else {
            return;
        };
        ui.set_flow_detail(
            format!(
                "{}\nLocalizador: {}\n{}",
                step.action, step.locator, step.error
            )
            .into(),
        );
        ui.set_flow_image(slint::Image::default());
        let id = serial.fetch_add(1, Ordering::Relaxed) + 1;
        if step.action != "screenshot"
            || !step.ok
            || step.artifact.len() != 32
            || !step.artifact.chars().all(|c| c.is_ascii_hexdigit())
        {
            return;
        }
        let path = disk
            .join("artifacts")
            .join(flow.chat.to_string())
            .join(&step.artifact)
            .join("screenshot.png");
        let project = state.project.clone();
        let flow_id = flow.id.clone();
        let selected = selected.clone();
        let serial = serial.clone();
        let handle = ui.as_weak();
        executor.executor.spawn(async move {
            let loaded = tokio::task::spawn_blocking(move || captures::preview(&path)).await;
            let _ = handle.upgrade_in_event_loop(move |ui| {
                if !ui.get_flows_open()
                    || serial.load(Ordering::Relaxed) != id
                    || ui.get_git_project().as_str() != project
                    || !selected
                        .lock()
                        .unwrap()
                        .current
                        .as_ref()
                        .is_some_and(|f| f.id == flow_id)
                {
                    return;
                }
                if let Ok(Ok((pixels, w, h))) = loaded {
                    ui.set_flow_image(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
                        slint::Rgba8Pixel,
                    >::clone_from_slice(
                        &pixels, w, h
                    )));
                }
            });
        });
    });
    let weak = ui.as_weak();
    let selected = selection.clone();
    ui.on_preview_flow_test(move || {
        let Some(ui) = weak.upgrade() else { return };
        let mut state = selected.lock().unwrap();
        if state.project != ui.get_git_project().as_str() {
            return;
        }
        let Some(flow) = &state.current else {
            ui.set_flow_notice("Seleccioná un flujo.".into());
            return;
        };
        match browser_flows::generate(
            flow,
            ui.get_flow_name().as_str(),
            ui.get_flow_base_url().as_str(),
        ) {
            Ok(code) => {
                let path = ui.get_flow_path().to_string();
                state.pending = Some((state.project.clone(), path.clone(), code.clone()));
                ui.set_flow_code(code.into());
                ui.set_flow_export_target(path.into());
                ui.set_flow_preview_open(true);
            }
            Err(e) => ui.set_flow_notice(e.into()),
        }
    });
    let weak = ui.as_weak();
    let selected = selection;
    ui.on_save_flow_test(move || {
        let Some(ui)=weak.upgrade() else {return};
        let mut state=selected.lock().unwrap();
        let Some((project,path,code))=state.pending.take() else {return};
        if project!=ui.get_git_project().as_str() || !ui.get_flow_preview_open() {return;}
        match browser_flows::export(&PathBuf::from(project),&path,&code) {
            Ok(path)=> {ui.set_flow_preview_open(false);ui.set_flow_notice(format!("Test guardado: {}. Requiere @playwright/test y las variables FLOW_VALUE_*.",path.display()).into());},
            Err(e)=> {state.pending=Some((state.project.clone(),path,code));ui.set_flow_notice(e.into());ui.set_flow_preview_open(false);},
        }
    });
}
