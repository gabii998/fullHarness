use crate::{AppWindow, LogcatRow};
use ascurra_harness::{
    logcat::{self, Device, Line, StreamEvent},
    runtime::Supervisor,
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{cell::RefCell, collections::VecDeque, path::PathBuf, rc::Rc, sync::Arc, time::Duration};
use tokio::sync::{mpsc, watch};

struct Entry {
    id: u64,
    line: Line,
}
#[derive(Default)]
struct Buffer {
    lines: VecDeque<Entry>,
    bytes: usize,
    next: u64,
    removed: usize,
}
impl Buffer {
    fn push(&mut self, lines: Vec<Line>) {
        for line in lines {
            self.bytes += line.text.len();
            self.lines.push_back(Entry {
                id: self.next,
                line,
            });
            self.next += 1;
            while self.lines.len() > logcat::MAX_LINES || self.bytes > logcat::MAX_BYTES {
                if let Some(old) = self.lines.pop_front() {
                    self.bytes -= old.line.text.len();
                    self.removed += 1;
                }
            }
        }
    }
}
struct State {
    adb: Option<PathBuf>,
    devices: Vec<Device>,
    serial: String,
    buffer: Buffer,
    shown: VecDeque<u64>,
    rows: Rc<VecModel<LogcatRow>>,
    receiver: Option<mpsc::Receiver<StreamEvent>>,
    cancel: Option<watch::Sender<bool>>,
    timer: slint::Timer,
    query_timer: slint::Timer,
    filter: String,
    level: i32,
}
pub struct Control {
    state: RefCell<State>,
}
impl Control {
    pub fn stop(&self) {
        let mut state = self.state.borrow_mut();
        if let Some(stop) = state.cancel.take() {
            let _ = stop.send(true);
        }
        state.receiver = None;
        state.timer.stop();
    }
}
impl Drop for Control {
    fn drop(&mut self) {
        self.stop();
    }
}

fn display(ui: &AppWindow, state: &mut State) {
    let query = ui.get_logcat_filter().to_lowercase();
    let level = ui.get_logcat_level();
    if query != state.filter || level != state.level {
        state.rows.set_vec(Vec::new());
        state.shown.clear();
        state.filter = query;
        state.level = level;
    }
    let selected = state
        .buffer
        .lines
        .iter()
        .filter(|entry| {
            let severity = "VDIWEF".find(entry.line.level).unwrap_or(2) as i32;
            severity >= level
                && (state.filter.is_empty()
                    || entry.line.text.to_lowercase().contains(&state.filter))
        })
        .collect::<Vec<_>>();
    let first = selected.first().map(|entry| entry.id);
    while state.shown.front().is_some_and(|id| Some(*id) != first) {
        state.shown.pop_front();
        state.rows.remove(0);
    }
    for entry in selected.iter().skip(state.shown.len()) {
        state.shown.push_back(entry.id);
        state.rows.push(LogcatRow {
            text: entry.line.text.as_str().into(),
            level: entry.line.level.to_string().into(),
        });
    }
    ui.set_logcat_columns(
        selected
            .iter()
            .map(|e| e.line.text.chars().count())
            .max()
            .unwrap_or(40)
            .min(4096) as i32,
    );
}

pub fn configure(ui: &AppWindow, supervisor: Arc<Supervisor>) -> Rc<Control> {
    let rows = Rc::new(VecModel::default());
    ui.set_logcat_lines(ModelRc::from(rows.clone()));
    ui.set_logcat_devices(ModelRc::new(VecModel::from(vec![
        "Sin dispositivos".into(),
    ])));
    let control = Rc::new(Control {
        state: RefCell::new(State {
            adb: None,
            devices: Vec::new(),
            serial: String::new(),
            buffer: Buffer::default(),
            shown: VecDeque::new(),
            rows,
            receiver: None,
            cancel: None,
            timer: slint::Timer::default(),
            query_timer: slint::Timer::default(),
            filter: String::new(),
            level: 2,
        }),
    });
    let weak = ui.as_weak();
    let live = control.clone();
    let executor = supervisor.executor.clone();
    ui.on_refresh_logcat_devices(move || {
        let Some(ui) = weak.upgrade() else { return; };
        if ui.get_logcat_busy() || ui.get_logcat_running() { return; }
        let Some(adb) = logcat::detect_adb() else {
            ui.set_logcat_device_ready(false);
            live.state.borrow_mut().devices.clear();
            live.state.borrow_mut().adb = None;
            ui.set_logcat_devices(ModelRc::new(VecModel::from(vec!["Sin dispositivos".into()])));
            ui.set_logcat_error("No se encontró ADB. Instalá Android SDK Platform-Tools o agregá su carpeta al PATH y volvé a abrir este panel.".into());
            return;
        };
        ui.set_logcat_error("".into());
        ui.set_logcat_busy(true); ui.set_logcat_device_ready(false);
        ui.set_logcat_status("Buscando dispositivos…".into());
        let previous = live.state.borrow().devices.get(ui.get_logcat_device_index() as usize).map(|d| d.serial.clone());
        let handle = ui.as_weak(); let data = Rc::downgrade(&live);
        // Only the result crosses threads. Rc control stays on the UI thread.
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        executor.spawn(async move { let result = logcat::devices(&adb).await; let _ = tx.send((adb, result)); });
        live.state.borrow().query_timer.start(slint::TimerMode::Repeated, Duration::from_millis(100), move || {
            let (Some(ui), Some(live)) = (handle.upgrade(), data.upgrade()) else { return; };
            let result = match rx.try_recv() {
                Ok(result) => result,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(_) => { live.state.borrow().query_timer.stop(); ui.set_logcat_busy(false); ui.set_logcat_error("Se interrumpió la consulta de ADB.".into()); return; }
            };
            live.state.borrow().query_timer.stop();
            ui.set_logcat_busy(false);
            let (adb, result) = result;
            let mut state = live.state.borrow_mut(); state.adb = Some(adb);
            match result {
                Ok(devices) => {
                    let index = previous.as_ref().and_then(|serial| devices.iter().position(|d| &d.serial == serial))
                        .or_else(|| devices.iter().position(|d| d.available)).unwrap_or(0);
                    let labels = if devices.is_empty() { vec!["Sin dispositivos".into()] } else { devices.iter().map(|d| d.label.as_str().into()).collect() };
                    ui.set_logcat_devices(ModelRc::new(VecModel::from(labels))); ui.set_logcat_device_index(index as i32);
                    ui.set_logcat_device_ready(devices.get(index).is_some_and(|d| d.available));
                    ui.set_logcat_error(if !devices.is_empty() && !devices[index].available {
                        "Dispositivo offline o sin autorizar. Aceptá la depuración USB en Android y volvé a abrir el panel.".into()
                    } else { "".into() });
                    ui.set_logcat_status(if devices.is_empty() { "Conectá un dispositivo o iniciá un emulador y volvé a abrir el panel." }
                        else if !devices[index].available { "Dispositivo offline o sin autorizar. Aceptá la depuración USB en Android y volvé a abrir el panel." }
                        else { "Dispositivo listo. Iniciá el flujo en tiempo real." }.into());
                    state.devices = devices;
                }
                Err(error) => { state.devices.clear(); ui.set_logcat_error(error.into()); }
            }
        });
    });
    let weak = ui.as_weak();
    let live = control.clone();
    ui.on_select_logcat_device(move |index| {
        live.stop();
        if let Some(ui) = weak.upgrade() {
            ui.set_logcat_running(false);
            ui.set_logcat_device_ready(
                live.state
                    .borrow()
                    .devices
                    .get(index as usize)
                    .is_some_and(|d| d.available),
            );
            ui.set_logcat_error(if ui.get_logcat_device_ready() { "".into() } else {
                "Dispositivo offline o sin autorizar. Aceptá la depuración USB en Android y volvé a abrir el panel.".into()
            });
            ui.set_logcat_status("Seleccioná Iniciar para ver el dispositivo elegido.".into());
        }
    });
    let weak = ui.as_weak();
    let live = control.clone();
    ui.on_stop_logcat(move || {
        live.stop();
        if let Some(ui) = weak.upgrade() {
            ui.set_logcat_running(false);
            ui.set_logcat_status("Pausado · el buffer local se conserva.".into());
        }
    });
    let weak = ui.as_weak();
    let live = control.clone();
    ui.on_clear_logcat(move || {
        if let Some(ui) = weak.upgrade() {
            let mut state = live.state.borrow_mut();
            state.buffer = Buffer::default();
            state.shown.clear();
            state.rows.set_vec(Vec::new());
            display(&ui, &mut state);
            if !ui.get_logcat_running() {
                ui.set_logcat_status("Buffer local limpio.".into());
            }
        }
    });
    let weak = ui.as_weak();
    let live = control.clone();
    ui.on_filter_logcat(move || {
        if let Some(ui) = weak.upgrade() {
            display(&ui, &mut live.state.borrow_mut());
        }
    });
    let weak = ui.as_weak();
    let live = control.clone();
    ui.on_start_logcat(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_logcat_running() || ui.get_logcat_busy() || !ui.get_logcat_open() {
            return;
        }
        let mut state = live.state.borrow_mut();
        let Some(adb) = state.adb.clone() else {
            return;
        };
        let Some(device) = state
            .devices
            .get(ui.get_logcat_device_index() as usize)
            .filter(|d| d.available)
        else {
            return;
        };
        let serial = device.serial.clone();
        if state.serial != serial {
            state.buffer = Buffer::default();
            state.rows.set_vec(Vec::new());
            state.shown.clear();
        }
        state.serial = serial.clone();
        let (cancel, stop) = watch::channel(false);
        let (tx, rx) = mpsc::channel(2);
        state.cancel = Some(cancel);
        state.receiver = Some(rx);
        ui.set_logcat_error("".into());
        ui.set_logcat_running(true);
        ui.set_logcat_status("Conectando al flujo en vivo…".into());
        supervisor
            .executor
            .spawn(logcat::stream(adb, serial, stop, tx));
        let handle = ui.as_weak();
        let data = Rc::downgrade(&live);
        state.timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(100),
            move || {
                let (Some(ui), Some(live)) = (handle.upgrade(), data.upgrade()) else {
                    return;
                };
                let mut state = live.state.borrow_mut();
                let mut received = Vec::new();
                let mut ended = None;
                for _ in 0..2 {
                    let event = state.receiver.as_mut().map(|rx| rx.try_recv());
                    match event {
                        Some(Ok(StreamEvent::Lines(lines))) => received.extend(lines),
                        Some(Ok(StreamEvent::End(result))) => {
                            ended = Some(result);
                            break;
                        }
                        Some(Err(mpsc::error::TryRecvError::Disconnected)) => {
                            ended = Some(Err("ADB cerró el flujo.".into()));
                            break;
                        }
                        _ => break,
                    }
                }
                if !received.is_empty() {
                    state.buffer.push(received);
                    display(&ui, &mut state);
                }
                if let Some(result) = ended {
                    state.timer.stop();
                    state.receiver = None;
                    state.cancel = None;
                    ui.set_logcat_running(false);
                    match result {
                        Ok(()) => ui.set_logcat_status(
                            "El flujo terminó. Podés volver a iniciarlo.".into(),
                        ),
                        Err(error) => ui.set_logcat_error(error.into()),
                    }
                }
            },
        );
    });
    control
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn log_buffer_is_bounded_by_rows_and_bytes() {
        let mut buffer = Buffer::default();
        buffer.push(
            (0..2000)
                .map(|i| Line {
                    text: format!("I/App: {i}"),
                    level: 'I',
                })
                .collect(),
        );
        assert_eq!(buffer.lines.len(), logcat::MAX_LINES);
        assert_eq!(buffer.lines.front().unwrap().id, 1000);
        buffer.push(
            (0..1000)
                .map(|_| Line {
                    text: "x".repeat(4096),
                    level: 'E',
                })
                .collect(),
        );
        assert!(buffer.bytes <= logcat::MAX_BYTES);
        assert!(buffer.lines.len() <= logcat::MAX_LINES);
    }
}
