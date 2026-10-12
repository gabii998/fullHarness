use crate::domain::{AppState, Id, Message, RunState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::HashMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

type StoreResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Serialize, Deserialize)]
struct History {
    messages: Vec<Message>,
    wire_history: Vec<serde_json::Value>,
}

#[derive(Serialize)]
struct HistoryRef<'a> {
    messages: &'a [Message],
    wire_history: &'a [serde_json::Value],
}

struct HashedWriter<'a> {
    file: &'a mut fs::File,
    digest: Sha256,
}
impl Write for HashedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = self.file.write(bytes)?;
        self.digest.update(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

pub struct Store {
    root: PathBuf,
    histories: RefCell<HashMap<Id, String>>,
}

impl Store {
    pub fn new(root: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(root.join("histories"))?;
        Ok(Self {
            root,
            histories: RefCell::new(HashMap::new()),
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn history_path(&self, id: Id, name: &str) -> StoreResult<PathBuf> {
        let prefix = format!("{id}-");
        let hash = name
            .strip_prefix(&prefix)
            .and_then(|s| s.strip_suffix(".json"));
        if !hash.is_some_and(|h| h.len() == 64 && h.bytes().all(|c| c.is_ascii_hexdigit())) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Referencia de historial inválida",
            )
            .into());
        }
        Ok(self.root.join("histories").join(name))
    }

    pub fn load(&self) -> StoreResult<AppState> {
        let path = self.root.join("state.json");
        if !path.exists() {
            return Ok(AppState::default());
        }
        let mut state: AppState =
            serde_json::from_reader(io::BufReader::new(fs::File::open(path)?))?;
        let mut legacy = false;
        for chat in state.projects.iter_mut().flat_map(|p| &mut p.chats) {
            chat.history_loaded = chat.history_file.is_none();
            legacy |= chat.history_loaded;
            if let Some(name) = &chat.history_file {
                self.history_path(chat.id, name)?;
                self.histories.borrow_mut().insert(chat.id, name.clone());
            }
        }
        state.reconcile_after_restart();
        // Publish external payloads before replacing the legacy index. A failed
        // migration leaves the original inline history readable.
        if legacy {
            self.save(&state)?;
        }
        if let Some(id) = state.selected_chat {
            self.hydrate(&mut state, id)?;
        }
        self.release_inactive(&mut state);
        Ok(state)
    }

    pub fn hydrate(&self, state: &mut AppState, id: Id) -> StoreResult<()> {
        let Some(chat) = state.chat_mut(id) else {
            return Ok(());
        };
        if chat.history_loaded {
            return Ok(());
        }
        let name = chat.history_file.as_ref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "Historial sin referencia")
        })?;
        let path = self.history_path(id, name)?;
        let history: History = serde_json::from_reader(io::BufReader::new(fs::File::open(&path)?))?;
        chat.messages = history.messages;
        chat.wire_history = history.wire_history;
        chat.history_loaded = true;
        Ok(())
    }

    /// Only release payloads already committed to disk; running/approval chats
    /// remain resident even when their tab or project is hidden.
    pub fn release_inactive(&self, state: &mut AppState) {
        let selected = state.selected_chat;
        let files = self.histories.borrow();
        for chat in state.projects.iter_mut().flat_map(|p| &mut p.chats) {
            if Some(chat.id) == selected
                || matches!(
                    chat.state,
                    RunState::Running | RunState::WaitingForPermission | RunState::WaitingForInput
                )
            {
                continue;
            }
            if let Some(name) = files.get(&chat.id) {
                chat.history_file = Some(name.clone());
                chat.messages = Vec::new();
                chat.wire_history = Vec::new();
                chat.history_loaded = false;
            }
        }
    }

    pub fn save(&self, state: &AppState) -> StoreResult<()> {
        let mut metadata = state.metadata_snapshot();
        let mut next = HashMap::new();
        for chat in state.projects.iter().flat_map(|p| &p.chats) {
            let name = if chat.history_loaded {
                let mut file = tempfile::NamedTempFile::new_in(self.root.join("histories"))?;
                let mut writer = HashedWriter {
                    file: file.as_file_mut(),
                    digest: Sha256::new(),
                };
                {
                    let mut buffered = io::BufWriter::new(&mut writer);
                    serde_json::to_writer(
                        &mut buffered,
                        &HistoryRef {
                            messages: &chat.messages,
                            wire_history: &chat.wire_history,
                        },
                    )?;
                    buffered.flush()?;
                }
                let name = format!("{}-{:x}.json", chat.id, writer.digest.finalize());
                file.as_file().sync_all()?;
                let target = self.history_path(chat.id, &name)?;
                if !target.exists() {
                    file.persist_noclobber(target)?;
                }
                name
            } else {
                chat.history_file
                    .clone()
                    .or_else(|| self.histories.borrow().get(&chat.id).cloned())
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "No se puede guardar un historial sin cargar",
                        )
                    })?
            };
            metadata.chat_mut(chat.id).unwrap().history_file = Some(name.clone());
            next.insert(chat.id, name);
        }
        let mut file = tempfile::NamedTempFile::new_in(&self.root)?;
        {
            let mut buffered = io::BufWriter::new(file.as_file_mut());
            serde_json::to_writer(&mut buffered, &metadata)?;
            buffered.flush()?;
        }
        file.as_file().sync_all()?;
        file.persist(self.root.join("state.json"))?;
        let old = self.histories.replace(next.clone());
        for (id, name) in old {
            if next.get(&id) != Some(&name) {
                if let Ok(path) = self.history_path(id, &name) {
                    let _ = fs::remove_file(path);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inactive_payloads_are_released_but_running_chats_and_drafts_survive() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::new(temp.path().into()).unwrap();
        let mut state = AppState::default();
        let project = state.add_project("A".into(), "a".into());
        let first = state.selected_chat.unwrap();
        let text = "historial largo α\n".repeat(20_000);
        let chat = state.chat_mut(first).unwrap();
        chat.messages.push(Message {
            role: "Codex".into(),
            text: text.clone(),
        });
        chat.wire_history.push(serde_json::json!({"content": text}));
        chat.draft = "borrador sin enviar".into();
        chat.provider_session_id = Some("sesión persistida".into());
        let running = state.add_chat(project).unwrap();
        state.chat_mut(running).unwrap().state = RunState::WaitingForPermission;
        state.chat_mut(running).unwrap().messages.push(Message {
            role: "Codex".into(),
            text: "En curso".into(),
        });
        let selected = state.add_chat(project).unwrap();
        store.save(&state).unwrap();
        store.release_inactive(&mut state);
        assert!(!state.chat(first).unwrap().history_loaded);
        assert!(state.chat(first).unwrap().messages.is_empty());
        assert!(state.chat(first).unwrap().wire_history.is_empty());
        assert_eq!(state.chat(first).unwrap().draft, "borrador sin enviar");
        assert_eq!(state.chat(running).unwrap().messages[0].text, "En curso");
        assert!(state.chat(running).unwrap().history_loaded);
        assert!(state.chat(selected).unwrap().history_loaded);
        assert!(fs::metadata(temp.path().join("state.json")).unwrap().len() < 10_000);
        // Saving an unloaded chat must keep its payload, never replace it with [].
        store.save(&state).unwrap();
        store.hydrate(&mut state, first).unwrap();
        assert_eq!(state.chat(first).unwrap().messages[0].text, text);
        assert_eq!(state.chat(first).unwrap().wire_history[0]["content"], text);
        state.select_chat(first);
        store.save(&state).unwrap();
        let reopened = store.load().unwrap();
        assert!(reopened.chat(first).unwrap().history_loaded);
        assert_eq!(reopened.chat(first).unwrap().messages[0].text, text);
        assert_eq!(
            reopened.chat(first).unwrap().provider_session_id.as_deref(),
            Some("sesión persistida")
        );
        assert!(!reopened.chat(running).unwrap().history_loaded);
        assert_eq!(reopened.chat(running).unwrap().state, RunState::Interrupted);
    }

    #[test]
    fn legacy_inline_history_migrates_and_missing_payload_is_reported() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::new(temp.path().into()).unwrap();
        let mut state = AppState::default();
        let project = state.add_project("A".into(), "a".into());
        let first = state.selected_chat.unwrap();
        state.chat_mut(first).unwrap().messages.push(Message {
            role: "Vos".into(),
            text: "Mensaje anterior".into(),
        });
        state.add_chat(project);
        fs::write(
            temp.path().join("state.json"),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        let mut migrated = store.load().unwrap();
        assert!(!migrated.chat(first).unwrap().history_loaded);
        store.hydrate(&mut migrated, first).unwrap();
        assert_eq!(
            migrated.chat(first).unwrap().messages[0].text,
            "Mensaje anterior"
        );
        store.release_inactive(&mut migrated);
        let name = migrated.chat(first).unwrap().history_file.clone().unwrap();
        fs::remove_file(temp.path().join("histories").join(name)).unwrap();
        assert!(store.hydrate(&mut migrated, first).is_err());
        assert!(!migrated.chat(first).unwrap().history_loaded);
        assert!(store.history_path(first, "../../state.json").is_err());
    }

    #[test]
    fn saves_replace_previous_state_and_preserve_history() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::new(temp.path().into()).unwrap();
        let mut state = AppState::default();
        state.add_project("A".into(), "a".into());
        store.save(&state).unwrap();
        state.add_project("B".into(), "b".into());
        store.save(&state).unwrap();
        assert_eq!(store.load().unwrap().projects.len(), 2);
    }
}
