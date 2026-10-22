//! Recorded browser flows and safe, reviewable Playwright test export.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Step {
    pub action: String,
    pub target: String,
    pub value: String,
    pub locator: Value,
    pub ok: bool,
    pub included: bool,
    pub duration_ms: u64,
    pub artifact: String,
    pub error: String,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Flow {
    pub id: String,
    pub chat: u64,
    pub created: u64,
    pub steps: Vec<Step>,
}
impl Flow {
    pub fn new(chat: u64) -> Self {
        Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            chat,
            created: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            steps: Vec::new(),
        }
    }
    pub fn record(
        &mut self,
        args: &Value,
        output: &Value,
        artifact: &str,
        duration_ms: u64,
    ) -> Result<(), String> {
        if self.steps.len() >= 2000 {
            return Err("El flujo alcanzó el límite de 2000 pasos.".into());
        }
        let action = args["action"].as_str().unwrap_or_default().to_string();
        let value = if action == "fill" {
            format!("FLOW_VALUE_{}", self.steps.len() + 1)
        } else {
            args["value"].as_str().unwrap_or_default().into()
        };
        self.steps.push(Step {
            included: output["ok"] == true && !matches!(action.as_str(), "snapshot" | "close"),
            action,
            target: args["target"].as_str().unwrap_or_default().into(),
            value,
            locator: output.get("locator").cloned().unwrap_or(Value::Null),
            ok: output["ok"] == true,
            duration_ms,
            artifact: artifact.into(),
            error: if args["action"] == "fill" && output["ok"] != true {
                "Falló el formulario (detalle omitido para no persistir valores sensibles).".into()
            } else {
                output["error"]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .take(1000)
                    .collect()
            },
        });
        Ok(())
    }
}
fn directory(data: &Path, project: &Path) -> Result<PathBuf, String> {
    let root = project
        .canonicalize()
        .map_err(|_| "Proyecto inexistente.")?;
    let key = format!("{:x}", Sha256::digest(root.to_string_lossy().as_bytes()));
    Ok(data.join("browser-flows").join(key))
}
pub fn save(data: &Path, project: &Path, flow: &Flow) -> Result<(), String> {
    if flow.id.len() != 32 || !flow.id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("ID de flujo inválido.".into());
    }
    let dir = directory(data, project)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec(flow).map_err(|e| e.to_string())?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("Flujo demasiado grande.".into());
    }
    let temp = dir.join(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    fs::write(&temp, bytes).map_err(|e| e.to_string())?;
    let result =
        fs::rename(&temp, dir.join(format!("{}.json", flow.id))).map_err(|e| e.to_string());
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
pub fn list(data: &Path, project: &Path) -> Result<Vec<Flow>, String> {
    let dir = directory(data, project)?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries = fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter(|e| {
            e.path().extension().is_some_and(|x| x == "json")
                && e.file_type().is_ok_and(|t| t.is_file())
        })
        .take(5000)
        .collect::<Vec<_>>();
    entries.sort_by_key(|e| std::cmp::Reverse(e.metadata().ok().and_then(|m| m.modified().ok())));
    let mut flows = Vec::new();
    for entry in entries.into_iter().take(100) {
        if entry.metadata().is_ok_and(|m| m.len() <= 2 * 1024 * 1024) {
            if let Ok(bytes) = fs::read(entry.path()) {
                if let Ok(flow) = serde_json::from_slice::<Flow>(&bytes) {
                    if flow.id.len() == 32
                        && flow.id.chars().all(|c| c.is_ascii_hexdigit())
                        && flow.steps.len() <= 2000
                    {
                        flows.push(flow);
                    }
                }
            }
        }
    }
    Ok(flows)
}
fn literal(text: &str) -> String {
    serde_json::to_string(text).unwrap()
}
fn locator(step: &Step) -> Result<String, String> {
    let l = &step.locator;
    let value = l["value"].as_str().unwrap_or_default();
    match l["kind"].as_str() {
        Some("testid") => Ok(format!("page.getByTestId({})", literal(value))),
        Some("label") => Ok(format!("page.getByLabel({}, {{ exact: true }})", literal(value))),
        Some("css") => Ok(format!("page.locator({})", literal(value))),
        _ if !step.target.is_empty() && !step.target.starts_with('@') => Ok(format!("page.locator({})", literal(&step.target))),
        _ => Err("Referencia temporal sin localizador exportable; excluí el paso o repetí la interacción.".into()),
    }
}
fn url_expression(value: &str, base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if !base.is_empty() {
        if let Some(suffix) = value.strip_prefix(base).filter(|suffix| {
            suffix.is_empty()
                || suffix.starts_with('/')
                || suffix.starts_with('?')
                || suffix.starts_with('#')
        }) {
            return format!("baseURL + {}", literal(suffix));
        }
    }
    literal(value)
}
pub fn generate(flow: &Flow, title: &str, base_url: &str) -> Result<String, String> {
    if title.len() > 200 || base_url.len() > 2048 {
        return Err("Título o URL base demasiado largos.".into());
    }
    if !base_url.is_empty()
        && !(base_url.starts_with("http://") || base_url.starts_with("https://"))
    {
        return Err("La URL base debe ser HTTP/HTTPS.".into());
    }
    let mut code = format!(
        "import {{ test, expect }} from '@playwright/test';\n\n// Review selectors and provide FLOW_VALUE_* environment variables.\n// Run: npx playwright test <this-file>\nconst baseURL = process.env.FLOW_BASE_URL ?? {};\n\ntest({}, async ({{ page }}) => {{\n  await page.setViewportSize({{ width: 1280, height: 720 }});\n",
        literal(base_url.trim_end_matches('/')),
        literal(if title.is_empty() {
            "Browser flow"
        } else {
            title
        })
    );
    let mut count = 0;
    for (i, step) in flow.steps.iter().enumerate().filter(|(_, s)| s.included) {
        if !step.ok {
            return Err(format!(
                "El paso {} falló; excluilo antes de exportar.",
                i + 1
            ));
        }
        let value = literal(&step.value);
        let line = match step.action.as_str() {
            "navigate" => {
                if !(step.value.starts_with("http://") || step.value.starts_with("https://")) {
                    return Err("Solo se exportan navegaciones HTTP/HTTPS.".into());
                }
                format!(
                    "await page.goto({}, {{ waitUntil: 'domcontentloaded' }});",
                    url_expression(&step.value, base_url)
                )
            }
            "click" => format!("await {}.click();", locator(step)?),
            "fill" => {
                if step.value.is_empty()
                    || !step
                        .value
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    return Err("Variable de formulario inválida.".into());
                }
                format!(
                    "if (process.env[{value}] === undefined) throw new Error('Missing form environment variable');\n  await {}.fill(process.env[{value}]!);",
                    locator(step)?
                )
            }
            "press" => format!("await {}.press({value});", locator(step)?),
            "assert_text" => format!("await expect(page.locator('body')).toContainText({value});"),
            "assert_url" => format!(
                "await expect(page).toHaveURL({});",
                url_expression(&step.value, base_url)
            ),
            "assert_visible" => format!("await expect({}).toBeVisible();", locator(step)?),
            "screenshot" => format!(
                "await page.screenshot({{ path: test.info().outputPath('step-{}.png'), fullPage: false }});",
                i + 1
            ),
            "snapshot" | "close" => continue,
            _ => return Err(format!("Acción no exportable: {}", step.action)),
        };
        code.push_str(&format!("  // Step {}\n  {}\n", i + 1, line));
        count += 1;
    }
    if count == 0 {
        return Err("Seleccioná al menos un paso exportable.".into());
    }
    code.push_str("});\n");
    Ok(code)
}
/// Creates a new file only. Never overwrites existing tests or follows a final symlink.
pub fn export(project: &Path, relative: &str, code: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || relative.contains(':')
        || !relative.ends_with(".spec.ts")
        || code.len() > 512 * 1024
    {
        return Err("Usá una ruta relativa dentro del proyecto terminada en .spec.ts.".into());
    }
    let root = project.canonicalize().map_err(|e| e.to_string())?;
    let destination = root.join(path);
    let parent = destination
        .parent()
        .ok_or("Ruta inválida.")?
        .canonicalize()
        .map_err(|_| "El directorio destino debe existir.")?;
    if !parent.starts_with(&root) {
        return Err("El destino escapa del proyecto.".into());
    }
    let destination = parent.join(destination.file_name().ok_or("Nombre inválido.")?);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .map_err(|e| format!("No se pudo crear el test (no se sobrescriben archivos): {e}"))?;
    file.write_all(code.as_bytes()).map_err(|e| e.to_string())?;
    Ok(destination)
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn recording_redacts_form_values_and_exports_refs() {
        let mut flow = Flow::new(1);
        flow.record(
            &json!({"action":"fill","target":"@e1","value":"secret"}),
            &json!({"ok":true,"locator":{"kind":"testid","value":"email"}}),
            "ref",
            10,
        )
        .unwrap();
        assert!(!serde_json::to_string(&flow).unwrap().contains("secret"));
        let code = generate(&flow, "a' title", "http://localhost:3000").unwrap();
        assert!(code.contains("getByTestId(\"email\")"));
        assert!(code.contains("FLOW_VALUE_1"));
        assert!(!code.contains("@e1"));
    }
    #[test]
    fn unresolved_refs_are_not_exported_and_urls_are_portable() {
        let mut flow = Flow::new(1);
        flow.record(
            &json!({"action":"click","target":"@e2","value":""}),
            &json!({"ok":true}),
            "ref",
            1,
        )
        .unwrap();
        assert!(generate(&flow, "test", "").is_err());
        assert_eq!(
            url_expression("http://localhost:3000/login", "http://localhost:3000"),
            "baseURL + \"/login\""
        );
        assert_eq!(
            url_expression("http://localhost:3000.evil/login", "http://localhost:3000"),
            "\"http://localhost:3000.evil/login\""
        );
    }
    #[test]
    fn failed_form_errors_do_not_persist_input_values() {
        let mut flow = Flow::new(1);
        flow.record(
            &json!({"action":"fill","target":"input","value":"very-secret"}),
            &json!({"ok":false,"error":"Cannot fill very-secret"}),
            "ref",
            1,
        )
        .unwrap();
        assert!(
            !serde_json::to_string(&flow)
                .unwrap()
                .contains("very-secret")
        );
    }
    #[test]
    fn export_is_confined_and_never_overwrites() {
        let root = tempfile::tempdir().unwrap();
        assert!(export(root.path(), "../test.spec.ts", "x").is_err());
        assert!(export(root.path(), "test.js", "x").is_err());
        export(root.path(), "flow.spec.ts", "first").unwrap();
        assert!(export(root.path(), "flow.spec.ts", "second").is_err());
        assert_eq!(
            fs::read_to_string(root.path().join("flow.spec.ts")).unwrap(),
            "first"
        );
    }
    #[test]
    fn storage_is_project_scoped_and_failed_steps_block_export() {
        let data = tempfile::tempdir().unwrap();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let mut flow = Flow::new(2);
        flow.record(
            &json!({"action":"click","target":"button","value":""}),
            &json!({"ok":false}),
            "ref",
            1,
        )
        .unwrap();
        save(data.path(), a.path(), &flow).unwrap();
        assert_eq!(list(data.path(), a.path()).unwrap().len(), 1);
        assert!(list(data.path(), b.path()).unwrap().is_empty());
        flow.steps[0].included = true;
        assert!(generate(&flow, "test", "").is_err());
    }
}
