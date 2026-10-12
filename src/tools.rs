use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, Read, Write},
    path::Path,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolReply {
    Finished {
        ok: bool,
        #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
    },
    Pending {
        state: PendingState,
        #[serde(rename = "ref")]
        reference: String,
    },
    Stopped {
        state: StopState,
        #[serde(rename = "ref")]
        reference: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingState {
    Running,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopState {
    Cancelled,
    TimedOut,
}

pub const MAX_INSPECTION_BYTES: usize = 8 * 1024;

/// Capture bounded disk output while draining the whole stream. The returned
/// metadata is for the harness, never automatically added to model context.
pub fn capture_output(mut input: impl Read, path: &Path, max_bytes: u64) -> io::Result<bool> {
    let mut file = File::create(path)?;
    let mut buffer = [0u8; 8192];
    let mut stored = 0u64;
    let mut truncated = false;
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let keep = (max_bytes.saturating_sub(stored)).min(count as u64) as usize;
        file.write_all(&buffer[..keep])?;
        stored += keep as u64;
        truncated |= keep < count;
    }
    Ok(truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_is_bounded_on_disk_and_absent_from_reply() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("output.txt");
        assert!(capture_output(&b"private command output"[..], &path, 7).unwrap());
        assert_eq!(std::fs::read(path).unwrap(), b"private");
        let reply = ToolReply::Finished {
            ok: false,
            reference: Some("r42".into()),
        };
        assert_eq!(
            serde_json::to_string(&reply).unwrap(),
            r#"{"ok":false,"ref":"r42"}"#
        );
    }
}
