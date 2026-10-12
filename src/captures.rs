use image::{ImageFormat, ImageReader};
use std::{
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    time::SystemTime,
};

const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PIXELS: u64 = 16 * 1024 * 1024;
#[derive(Clone)]
pub struct Capture {
    pub path: PathBuf,
    pub name: String,
    pub chat: u64,
    pub modified: SystemTime,
}

fn bytes(path: &Path) -> Result<Vec<u8>, String> {
    let meta = fs::symlink_metadata(path).map_err(|_| "No se pudo leer la imagen.")?;
    if !meta.is_file() || linked(&meta) || meta.len() > MAX_BYTES {
        return Err("La imagen debe ser un archivo regular de hasta 16 MiB.".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| "No se pudo abrir la imagen.")?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "No se pudo leer la imagen.")?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("La imagen supera 16 MiB.".into());
    }
    Ok(bytes)
}
fn format(bytes: &[u8]) -> Result<ImageFormat, String> {
    let format = image::guess_format(bytes).map_err(|_| "Imagen inválida.")?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
        return Err("Se admiten imágenes PNG y JPEG.".into());
    }
    let (w, h) = ImageReader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .map_err(|_| "No se pudieron leer las dimensiones de la imagen.")?;
    if w == 0 || h == 0 || w > 8192 || h > 8192 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err("La imagen supera el límite de 16 megapíxeles.".into());
    }
    Ok(format)
}
pub fn import(source: &Path, directory: &Path) -> Result<PathBuf, String> {
    let content = bytes(source)?;
    let format = format(&content)?;
    fs::create_dir_all(directory).map_err(|_| "No se pudo crear el directorio de imágenes.")?;
    let stem: String = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("capture")
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
        .take(80)
        .collect();
    let stem = if stem.is_empty() { "capture" } else { &stem };
    let destination = directory.join(format!(
        "{}.{}",
        stem,
        if format == ImageFormat::Png {
            "png"
        } else {
            "jpg"
        }
    ));
    let mut file =
        tempfile::NamedTempFile::new_in(directory).map_err(|_| "No se pudo guardar la imagen.")?;
    use std::io::Write;
    file.write_all(&content)
        .map_err(|_| "No se pudo guardar la imagen.")?;
    file.persist_noclobber(&destination)
        .map_err(|_| "No se pudo guardar la imagen.")?;
    Ok(destination)
}
pub fn preview(path: &Path) -> Result<(Vec<u8>, u32, u32), String> {
    let content = bytes(path)?;
    let format = format(&content)?;
    let image = ImageReader::with_format(Cursor::new(content), format)
        .decode()
        .map_err(|_| "La imagen no se pudo decodificar.")?;
    let image = if image.width() > 1200 || image.height() > 900 {
        image.thumbnail(1200, 900)
    } else {
        image
    }
    .to_rgba8();
    let (w, h) = image.dimensions();
    Ok((image.into_raw(), w, h))
}
fn linked(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}
fn entries(path: &Path, limit: usize) -> Vec<fs::DirEntry> {
    if path
        .ancestors()
        .any(|p| fs::symlink_metadata(p).is_ok_and(|m| linked(&m)))
    {
        return Vec::new();
    }
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .take(limit)
        .collect()
}
/// Only inspect artifact directories belonging to the selected project's chats.
/// Images remain on disk; listing never decodes them or injects them into model context.
pub fn list(data: &Path, chats: &[u64]) -> (Vec<Capture>, bool) {
    let mut result = Vec::new();
    let mut visited = 0;
    let mut partial = false;
    'chats: for chat in chats {
        let mut directories = entries(&data.join("artifacts").join(chat.to_string()), 5001);
        partial |= directories.len() > 5000;
        directories
            .sort_by_key(|e| std::cmp::Reverse(e.metadata().ok().and_then(|m| m.modified().ok())));
        for directory in directories {
            visited += 1;
            if visited > 5000 {
                partial = true;
                break 'chats;
            }
            if !directory.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let files = entries(&directory.path(), 101);
            partial |= files.len() > 100;
            for file in files.into_iter().take(100) {
                let path = file.path();
                let extension = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if !matches!(extension.as_str(), "png" | "jpg" | "jpeg") {
                    continue;
                }
                if let Ok(meta) = fs::symlink_metadata(&path) {
                    if meta.is_file() && !linked(&meta) && meta.len() <= MAX_BYTES {
                        result.push(Capture {
                            name: file.file_name().to_string_lossy().into(),
                            path,
                            chat: *chat,
                            modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                        });
                    }
                }
            }
            if result.len() > 200 {
                partial = true;
                result.sort_by_key(|e| std::cmp::Reverse(e.modified));
                result.truncate(200);
            }
        }
    }
    result.sort_by_key(|e| std::cmp::Reverse(e.modified));
    partial |= result.len() > 200;
    result.truncate(200);
    (result, partial)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_import_preview_and_listing_are_scoped_and_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.png");
        image::RgbaImage::from_pixel(1400, 1000, image::Rgba([20, 30, 40, 255]))
            .save(&source)
            .unwrap();
        let registered = import(&source, &temp.path().join("artifacts/7/a")).unwrap();
        import(&source, &temp.path().join("artifacts/8/b")).unwrap();
        let (items, partial) = list(temp.path(), &[7]);
        assert!(!partial);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].chat, 7);
        let (pixels, w, h) = preview(&registered).unwrap();
        assert!(w <= 1200 && h <= 900);
        assert_eq!(pixels.len(), w as usize * h as usize * 4);
        fs::write(temp.path().join("invalid.png"), "not an image").unwrap();
        assert!(
            import(
                &temp.path().join("invalid.png"),
                &temp.path().join("artifacts/7/invalid")
            )
            .is_err()
        );
        assert!(!temp.path().join("artifacts/7/invalid").exists());
    }
}
