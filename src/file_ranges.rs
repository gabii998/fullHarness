//! Bounded, streaming line-range reads. Skipped lines are never allocated.
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

pub struct Fragment {
    pub text: String,
    pub next_line: u64,
    pub next_offset: u64,
    pub partial_last_line: bool,
    pub more: bool,
}

pub fn read(path: &Path, start: u64, count: u64, budget: usize) -> Result<Fragment, String> {
    let file = File::open(path).map_err(|_| "No se pudo abrir el archivo.")?;
    if !file
        .metadata()
        .map_err(|_| "No se pudo leer el archivo.")?
        .is_file()
    {
        return Err("La ruta debe ser un archivo regular.".into());
    }
    let mut reader = BufReader::new(file);
    let mut line = 1u64;
    let mut offset = 0u64;
    // Skip using borrowed chunks, even for arbitrarily long lines.
    while line < start {
        let buffer = reader
            .fill_buf()
            .map_err(|_| "No se pudo leer el archivo.")?;
        if buffer.is_empty() {
            return Ok(Fragment {
                text: String::new(),
                next_line: line,
                next_offset: offset,
                partial_last_line: false,
                more: false,
            });
        }
        let mut used = 0;
        for &byte in buffer {
            used += 1;
            if byte == b'\n' {
                line += 1;
            }
            if line == start {
                break;
            }
        }
        offset += used as u64;
        reader.consume(used);
    }
    let start_offset = offset;
    let mut bytes = Vec::with_capacity(budget + 1);
    let mut completed = 0u64;
    // One extra byte distinguishes an exact fit from a truncated range.
    while completed < count && bytes.len() <= budget {
        let buffer = reader
            .fill_buf()
            .map_err(|_| "No se pudo leer el archivo.")?;
        if buffer.is_empty() {
            break;
        }
        let mut used = 0;
        for &byte in buffer {
            bytes.push(byte);
            used += 1;
            if byte == b'\n' {
                completed += 1;
            }
            if completed == count || bytes.len() > budget {
                break;
            }
        }
        reader.consume(used);
    }
    let more = bytes.len() > budget;
    bytes.truncate(budget);
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text.to_owned(),
        Err(error) if more && error.error_len().is_none() => {
            // A budget boundary may bisect a code point; never return broken UTF-8.
            std::str::from_utf8(&bytes[..error.valid_up_to()])
                .unwrap()
                .to_owned()
        }
        Err(_) => return Err("El fragmento no es UTF-8 válido.".into()),
    };
    let newlines = text.bytes().filter(|&b| b == b'\n').count() as u64;
    let partial = !text.is_empty() && !text.ends_with('\n');
    Ok(Fragment {
        next_line: start + newlines + u64::from(partial && !more),
        next_offset: start_offset + text.len() as u64,
        partial_last_line: partial && more,
        text,
        more,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(bytes: &[u8]) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static ID: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "harness-ranges-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, bytes).unwrap();
        path
    }
    #[test]
    fn lines_crlf_eof_and_exact_budget() {
        let path = fixture(b"one\r\ntwo\r\nlast");
        let f = read(&path, 2, 1, 5).unwrap();
        assert_eq!(f.text, "two\r\n");
        assert_eq!((f.next_line, f.next_offset, f.more), (3, 10, false));
        let f = read(&path, 3, 10, 100).unwrap();
        assert_eq!(f.text, "last");
        assert_eq!((f.next_line, f.next_offset, f.more), (4, 14, false));
        assert!(read(&path, 99, 1, 100).unwrap().text.is_empty());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn utf8_boundary_and_long_lines_are_bounded() {
        let path = fixture("á🙂end\nnext".as_bytes());
        let f = read(&path, 1, 2, 4).unwrap();
        assert_eq!(f.text, "á");
        assert_eq!(
            (f.next_offset, f.next_line, f.more, f.partial_last_line),
            (2, 1, true, true)
        );
        std::fs::remove_file(path).unwrap();
        let mut data = vec![b'x'; 100_000];
        data.extend_from_slice(b"\nsecond\n");
        let path = fixture(&data);
        assert_eq!(read(&path, 1, 1, 8192).unwrap().text.len(), 8192);
        assert_eq!(read(&path, 2, 1, 8192).unwrap().text, "second\n");
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn empty_and_invalid_utf8() {
        let path = fixture(b"");
        assert!(!read(&path, 1, 1, 8).unwrap().more);
        std::fs::remove_file(path).unwrap();
        let path = fixture(&[0xff, b'\n']);
        assert!(read(&path, 1, 1, 8).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
