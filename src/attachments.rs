use base64::{Engine, engine::general_purpose::STANDARD};
use image::{ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
};

pub const MAX_IMAGES: usize = 4;
const MAX_BYTES: usize = 5 * 1024 * 1024;
const PREFIX: &str = "harness-image:";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub bytes: usize,
}
pub fn path(data: &Path, chat: u64, id: &str) -> Result<PathBuf, String> {
    if id.len() != 32 || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("Referencia de imagen inválida.".into());
    }
    Ok(data
        .join("attachments")
        .join(chat.to_string())
        .join(format!("{id}.bin")))
}
fn checked(bytes: &[u8]) -> Result<ImageFormat, String> {
    if bytes.len() > MAX_BYTES {
        return Err("Cada imagen puede ocupar hasta 5 MiB.".into());
    }
    let format = image::guess_format(bytes).map_err(|_| "Imagen inválida.")?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
        return Err("Se admiten imágenes PNG y JPEG.".into());
    }
    let (w, h) = ImageReader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .map_err(|_| "No se pudo leer la imagen.")?;
    if w == 0 || h == 0 || w > 8192 || h > 8192 || u64::from(w) * u64::from(h) > 16 * 1024 * 1024 {
        return Err("La imagen supera 16 megapíxeles.".into());
    }
    Ok(format)
}
pub fn save(data: &Path, chat: u64, name: &str, bytes: &[u8]) -> Result<Attachment, String> {
    let format = checked(bytes)?;
    let decoded = ImageReader::with_format(Cursor::new(bytes), format)
        .decode()
        .map_err(|_| "La imagen está dañada.")?;
    let image = Attachment {
        id: uuid::Uuid::new_v4().simple().to_string(),
        name: name.chars().take(120).collect(),
        mime: if format == ImageFormat::Png {
            "image/png"
        } else {
            "image/jpeg"
        }
        .into(),
        bytes: bytes.len(),
    };
    let target = path(data, chat, &image.id)?;
    fs::create_dir_all(target.parent().unwrap())
        .map_err(|_| "No se pudo crear el directorio de adjuntos.")?;
    let mut file = tempfile::NamedTempFile::new_in(target.parent().unwrap())
        .map_err(|_| "No se pudo guardar la imagen.")?;
    file.write_all(bytes)
        .map_err(|_| "No se pudo guardar la imagen.")?;
    file.persist_noclobber(&target)
        .map_err(|_| "No se pudo guardar la imagen.")?;
    decoded
        .thumbnail(96, 72)
        .save_with_format(thumbnail_path(data, chat, &image.id)?, ImageFormat::Png)
        .map_err(|_| "No se pudo guardar la miniatura.")?;
    Ok(image)
}
pub fn thumbnail_path(data: &Path, chat: u64, id: &str) -> Result<PathBuf, String> {
    Ok(path(data, chat, id)?.with_extension("thumb.png"))
}
pub fn from_file(data: &Path, chat: u64, source: &Path) -> Result<Attachment, String> {
    let mut bytes = Vec::new();
    fs::File::open(source)
        .map_err(|_| "No se pudo abrir la imagen.")?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "No se pudo leer la imagen.")?;
    save(
        data,
        chat,
        &source.file_name().unwrap_or_default().to_string_lossy(),
        &bytes,
    )
}
pub fn from_clipboard(
    data: &Path,
    chat: u64,
    width: usize,
    height: usize,
    bytes: &[u8],
) -> Result<Attachment, String> {
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || width.saturating_mul(height) > 16 * 1024 * 1024
        || width.checked_mul(height).and_then(|v| v.checked_mul(4)) != Some(bytes.len())
    {
        return Err("La imagen del portapapeles supera el tamaño permitido.".into());
    }
    let image = image::RgbaImage::from_raw(width as u32, height as u32, bytes.to_vec())
        .ok_or("Imagen del portapapeles inválida.")?;
    let mut output = Cursor::new(Vec::new());
    image
        .write_to(&mut output, ImageFormat::Png)
        .map_err(|_| "No se pudo convertir la imagen del portapapeles.")?;
    save(data, chat, "Imagen pegada.png", output.get_ref())
}
fn encoded(data: &Path, chat: u64, id: &str) -> Result<(String, String, usize), String> {
    let path = path(data, chat, id)?;
    if fs::symlink_metadata(&path)
        .map_err(|_| "No se encontró una imagen adjunta.")?
        .file_type()
        .is_symlink()
    {
        return Err("El adjunto no puede ser un enlace.".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| "No se pudo abrir el adjunto.")?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "No se pudo leer el adjunto.")?;
    let format = checked(&bytes)?;
    let mime = if format == ImageFormat::Png {
        "image/png"
    } else {
        "image/jpeg"
    };
    Ok((mime.into(), STANDARD.encode(&bytes), bytes.len()))
}
/// Persist only local references in history; hydrate them at the transport boundary.
pub fn codex_message(text: &str, images: &[Attachment]) -> Value {
    if images.is_empty() {
        return json!({"role":"user","content":text});
    }
    let mut parts = Vec::new();
    if !text.is_empty() {
        parts.push(json!({"type":"input_text","text":text}));
    }
    parts.extend(images.iter().map(
        |i| json!({"type":"input_image","image_url":format!("{PREFIX}{}",i.id),"detail":"auto"}),
    ));
    json!({"role":"user","content":parts})
}
pub fn codex_input(history: &[Value], data: &Path, chat: u64) -> Result<Vec<Value>, String> {
    let mut result = history.to_vec();
    let mut total = 0usize;
    for message in &mut result {
        if let Some(parts) = message.get_mut("content").and_then(Value::as_array_mut) {
            for part in parts {
                if part["type"] == "input_image" {
                    if let Some(id) = part["image_url"]
                        .as_str()
                        .and_then(|s| s.strip_prefix(PREFIX))
                    {
                        let (mime, encoded, bytes) = encoded(data, chat, id)?;
                        total += bytes;
                        if total > 20 * 1024 * 1024 {
                            return Err(
                                "Las imágenes del contexto superan 20 MiB. Abrí un chat nuevo."
                                    .into(),
                            );
                        }
                        part["image_url"] = json!(format!("data:{mime};base64,{encoded}"));
                    }
                }
            }
        }
    }
    Ok(result)
}
pub fn claude_content(
    text: &str,
    images: &[Attachment],
    data: &Path,
    chat: u64,
) -> Result<Value, String> {
    if images.is_empty() {
        return Ok(json!(text));
    }
    if images.len() > MAX_IMAGES {
        return Err("Se admiten hasta cuatro imágenes por mensaje.".into());
    }
    let mut parts = Vec::new();
    for image in images {
        let (mime, encoded, _) = encoded(data, chat, &image.id)?;
        parts.push(
            json!({"type":"image","source":{"type":"base64","media_type":mime,"data":encoded}}),
        );
    }
    if !text.is_empty() {
        parts.push(json!({"type":"text","text":text}));
    }
    Ok(json!(parts))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transport_preserves_reasoning_and_tool_items_without_inserting_content() {
        let history = vec![
            json!({"role":"user","content":"Buscá archivos"}),
            json!({"type":"reasoning","encrypted_content":"fixture","summary":[]}),
            json!({"type":"function_call","name":"list_directory","arguments":"{}","call_id":"call_1"}),
            json!({"type":"function_call_output","call_id":"call_1","output":"ok"}),
        ];
        let temp = tempfile::tempdir().unwrap();
        let wire = codex_input(&history, temp.path(), 1).unwrap();
        assert_eq!(wire, history);
        assert!(wire[1..].iter().all(|item| item.get("content").is_none()));
    }
    #[test]
    fn images_persist_as_references_and_materialize_only_for_transport() {
        let temp = tempfile::tempdir().unwrap();
        let store = crate::storage::Store::new(temp.path().join("data")).unwrap();
        let mut state = crate::domain::AppState::default();
        state.add_project("Test".into(), temp.path().to_string_lossy().into());
        let chat = state.selected_chat.unwrap();
        let image =
            from_clipboard(store.root(), chat, 4, 3, &[20, 30, 40, 255].repeat(12)).unwrap();
        state
            .chat_mut(chat)
            .unwrap()
            .draft_images
            .push(image.clone());
        store.save(&state).unwrap();
        let mut reloaded = store.load().unwrap();
        assert_eq!(
            reloaded.chat(chat).unwrap().draft_images,
            vec![image.clone()]
        );
        reloaded.chat_mut(chat).unwrap().draft_images.clear();
        reloaded
            .chat_mut(chat)
            .unwrap()
            .message_images
            .insert(0, vec![image.clone()]);
        store.save(&reloaded).unwrap();
        let metadata = fs::read_to_string(store.root().join("state.json")).unwrap();
        assert!(metadata.contains(&image.id));
        assert!(!metadata.contains("iVBORw0KGgo"));
        let local = codex_message("Analizá", &[image.clone()]);
        assert!(local.to_string().contains(PREFIX));
        let wire = codex_input(&[local.clone()], store.root(), chat).unwrap();
        let data = wire[0]["content"][1]["image_url"]
            .as_str()
            .unwrap()
            .strip_prefix("data:image/png;base64,")
            .unwrap();
        assert_eq!(
            STANDARD.decode(data).unwrap(),
            fs::read(path(store.root(), chat, &image.id).unwrap()).unwrap()
        );
        assert!(!local.to_string().contains("iVBORw0KGgo"));
        let claude = claude_content("Analizá", &[image.clone()], store.root(), chat).unwrap();
        assert_eq!(claude[0]["source"]["media_type"], "image/png");
        assert_eq!(claude[0]["source"]["data"], data);
        assert_eq!(claude[1]["text"], "Analizá");
        assert!(codex_input(&[local], store.root(), chat + 1).is_err());
        assert!(path(store.root(), chat, "../../private").is_err());
        assert!(from_clipboard(store.root(), chat, 10000, 1, &[]).is_err());
        assert!(save(store.root(), chat, "fake.png", b"not an image").is_err());
        assert_eq!(
            codex_message("", &[image])["content"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
}
