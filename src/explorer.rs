//! Lazy file browsing for the human UI. Contents never enter agent history.
use std::{
    fs,
    io::Read,
    path::{Component, Path},
};

const MAX_DIRECTORY_ENTRIES: usize = 1000;
const MAX_TREE_ENTRIES: usize = 4000;
const MAX_PREVIEW: usize = 128 * 1024;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub path: String,
    pub name: String,
    pub directory: bool,
    pub expanded: bool,
    pub depth: usize,
}

#[derive(Clone, Default)]
pub struct Tree {
    pub entries: Vec<Entry>,
}
impl Tree {
    pub fn collapse(&mut self, path: &str) -> bool {
        let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.path == path && entry.expanded)
        else {
            return false;
        };
        let depth = self.entries[index].depth;
        let end = self.entries[index + 1..]
            .iter()
            .position(|entry| entry.depth <= depth)
            .map(|offset| index + 1 + offset)
            .unwrap_or(self.entries.len());
        self.entries.drain(index + 1..end);
        self.entries[index].expanded = false;
        true
    }
    pub fn insert(&mut self, path: &str, mut children: Vec<Entry>) -> Result<(), String> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.path == path && entry.directory)
            .ok_or("La carpeta ya no está en la vista.")?;
        if self.entries.len() + children.len() > MAX_TREE_ENTRIES {
            return Err(
                "La vista llegó a 4000 entradas. Cerrá algunas carpetas para continuar.".into(),
            );
        }
        let depth = self.entries[index].depth + 1;
        if depth > 24 {
            return Err("La vista admite hasta 24 niveles de carpetas.".into());
        }
        for entry in &mut children {
            entry.depth = depth;
        }
        self.entries[index].expanded = true;
        self.entries.splice(index + 1..index + 1, children);
        Ok(())
    }
}

fn resolve(root: &Path, relative: &str) -> Result<std::path::PathBuf, String> {
    let path = Path::new(relative);
    if path
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
        && !relative.is_empty()
    {
        return Err("La ruta debe permanecer dentro del proyecto.".into());
    }
    let root = root
        .canonicalize()
        .map_err(|_| "La carpeta del proyecto no está disponible.")?;
    let resolved = root
        .join(path)
        .canonicalize()
        .map_err(|_| "No se pudo acceder al archivo o carpeta.")?;
    if !resolved.starts_with(&root) {
        return Err("Ese enlace apunta fuera del proyecto.".into());
    }
    Ok(resolved)
}

pub fn list(root: &Path, relative: &str) -> Result<Vec<Entry>, String> {
    let directory = resolve(root, relative)?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory).map_err(|_| "No se pudo leer la carpeta.")? {
        if entries.len() >= MAX_DIRECTORY_ENTRIES {
            return Err(
                "La carpeta supera 1000 entradas. Abrila desde el Explorador de Windows.".into(),
            );
        }
        let entry = entry.map_err(|_| "No se pudo leer una entrada de la carpeta.")?;
        let name = entry.file_name().into_string().map_err(
            |_| "La carpeta contiene un nombre que no se puede representar en la interfaz.",
        )?;
        let path = Path::new(relative)
            .join(&name)
            .to_string_lossy()
            .into_owned();
        entries.push(Entry {
            path,
            name,
            directory: entry.path().is_dir(),
            expanded: false,
            depth: 0,
        });
    }
    entries.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Preview {
    pub text: String,
    pub truncated: bool,
}
pub fn preview(root: &Path, relative: &str) -> Result<Preview, String> {
    let path = resolve(root, relative)?;
    if !path.is_file() {
        return Err("Seleccioná un archivo para ver su contenido.".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| "No se pudo abrir el archivo.")?
        .take((MAX_PREVIEW + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "No se pudo leer el archivo.")?;
    let truncated = bytes.len() > MAX_PREVIEW;
    bytes.truncate(MAX_PREVIEW);
    if bytes.contains(&0) {
        return Err("Archivo binario: la vista previa admite texto UTF-8.".into());
    }
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text.to_string(),
        Err(error) if truncated && error.error_len().is_none() => {
            String::from_utf8(bytes[..error.valid_up_to()].to_vec()).unwrap()
        }
        Err(_) => return Err("La vista previa admite archivos de texto UTF-8.".into()),
    };
    Ok(Preview { text, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lazy_tree_orders_folders_and_collapses_only_descendants() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        fs::create_dir(root.path().join("other")).unwrap();
        fs::write(root.path().join("src/main.rs"), "fixture").unwrap();
        fs::write(root.path().join("README.md"), "fixture").unwrap();
        let mut tree = Tree {
            entries: list(root.path(), "").unwrap(),
        };
        assert!(tree.entries[0].directory);
        tree.insert("src", list(root.path(), "src").unwrap())
            .unwrap();
        assert_eq!(
            tree.entries
                .iter()
                .find(|entry| entry.name == "main.rs")
                .unwrap()
                .depth,
            1
        );
        assert!(tree.collapse("src"));
        assert_eq!(tree.entries.len(), 3);
        assert!(tree.entries.iter().any(|entry| entry.name == "other"));
        assert!(list(root.path(), "../").is_err());
    }
    #[test]
    fn preview_is_bounded_preserves_partial_utf8_and_rejects_binary() {
        let root = tempfile::tempdir().unwrap();
        let text = format!("X{}", "á".repeat(MAX_PREVIEW));
        fs::write(root.path().join("large.txt"), &text).unwrap();
        let result = preview(root.path(), "large.txt").unwrap();
        assert!(result.truncated);
        assert!(result.text.len() <= MAX_PREVIEW);
        assert_eq!(result.text.len(), MAX_PREVIEW - 1);
        fs::write(root.path().join("binary"), [0, 1, 2]).unwrap();
        assert!(preview(root.path(), "binary").is_err());
        assert!(preview(root.path(), "../secret").is_err());
    }
}
