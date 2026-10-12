use crate::domain::Provider;
use serde::{Deserialize, Serialize};
use std::{fs::File, io::Read, path::Path};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BuildCommand {
    pub command: String,
    pub purpose: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectProfile {
    pub description: String,
    pub builds: Vec<BuildCommand>,
    pub test_command: String,
    pub rules: String,
}
impl Default for ProjectProfile {
    fn default() -> Self {
        Self {
            description: String::new(),
            builds: Vec::new(),
            test_command: String::new(),
            rules: String::new(),
        }
    }
}
impl ProjectProfile {
    pub fn validate(&mut self) -> Result<(), String> {
        self.description = self.description.trim().into();
        self.test_command = self.test_command.trim().into();
        self.rules = self.rules.trim().into();
        if self.description.len() > 2048
            || self.rules.len() > 24 * 1024
            || self.test_command.len() > 2048
        {
            return Err(
                "La descripción admite 2 KB, las reglas 24 KB y el comando de tests 2 KB.".into(),
            );
        }
        if self.builds.len() > 16 {
            return Err("Podés definir hasta 16 comandos de compilación.".into());
        }
        for build in &mut self.builds {
            build.command = build.command.trim().into();
            build.purpose = build.purpose.trim().into();
            if build.command.is_empty() || build.purpose.is_empty() {
                return Err(
                    "Completá el comando de compilación y cuándo usarlo, o eliminá esa fila."
                        .into(),
                );
            }
            if build.command.len() > 2048 || build.purpose.len() > 1024 {
                return Err("El comando admite 2 KB y su propósito 1 KB.".into());
            }
        }
        Ok(())
    }

    pub fn context(&self, _root: &Path, _provider: Provider) -> Result<String, String> {
        let mut text = String::new();
        if !self.description.is_empty() {
            text.push_str(&format!("Descripción: {}\n", self.description));
        }
        for build in &self.builds {
            text.push_str(&format!(
                "Compilar ({}) : {}\n",
                build.purpose, build.command
            ));
        }
        if !self.test_command.is_empty() {
            text.push_str(&format!("Tests: {}\n", self.test_command));
        }
        if !self.rules.is_empty() {
            text.push_str(&format!("Reglas del proyecto:\n{}\n", self.rules));
        }
        if text.len() > 24 * 1024 {
            return Err("La configuración y las reglas superan 24 KB. Reducí el contenido antes de ejecutar.".into());
        }
        if text.is_empty() {
            return Ok(text);
        }
        Ok(format!(
            "\n\nProject configuration supplied by the user (commands are guidance, not an instruction to execute automatically; harness permissions still apply):\n{text}"
        ))
    }
}

/// Copy rules into the editable project field, preserving existing text.
/// Return atomically: an unreadable or oversized source never partially imports.
pub fn import_rules(
    root: &Path,
    providers: &[Provider],
    existing: &str,
) -> Result<(String, usize), String> {
    let mut text = existing.trim().to_string();
    let mut added = 0;
    for provider in providers {
        for file in rule_files(root, *provider) {
            if let Some(error) = file.error {
                return Err(format!("{}: {error}", file.path));
            }
            let content = file.content.trim();
            if content.is_empty() || text.contains(content) {
                continue;
            }
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&format!("# {}\n\n{}", file.path, content));
            added += 1;
        }
    }
    if text.len() > 24 * 1024 {
        return Err("Las reglas combinadas superan 24 KB. No se modificó el campo.".into());
    }
    Ok((text, added))
}

pub struct RuleFile {
    pub path: String,
    pub content: String,
    pub error: Option<String>,
}
pub fn rule_files(root: &Path, provider: Provider) -> Vec<RuleFile> {
    let names: &[&str] = match provider {
        Provider::Codex => {
            if root.join("AGENTS.override.md").exists() {
                &["AGENTS.override.md"]
            } else {
                &["AGENTS.md"]
            }
        }
        Provider::Claude => &["CLAUDE.md", ".claude/CLAUDE.md", "CLAUDE.local.md"],
    };
    names
        .iter()
        .filter_map(|name| {
            let path = root.join(name);
            if !path.exists() {
                return None;
            }
            let read = (|| -> Result<String, String> {
                let canonical_root = root.canonicalize().map_err(|e| e.to_string())?;
                if !path
                    .canonicalize()
                    .map_err(|e| e.to_string())?
                    .starts_with(&canonical_root)
                {
                    return Err("El archivo apunta fuera del proyecto.".into());
                }
                let mut bytes = Vec::new();
                File::open(path)
                    .map_err(|e| e.to_string())?
                    .take(8193)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() > 8192 {
                    return Err("Supera 8 KB; no se cargó parcialmente.".into());
                }
                String::from_utf8(bytes)
                    .map(|s| s.trim_start_matches('\u{feff}').to_string())
                    .map_err(|_| "El archivo debe usar UTF-8.".into())
            })();
            Some(match read {
                Ok(content) => RuleFile {
                    path: name.to_string(),
                    content,
                    error: None,
                },
                Err(error) => RuleFile {
                    path: name.to_string(),
                    content: String::new(),
                    error: Some(error),
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configuration_rules_are_bounded_provider_specific_and_live() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("AGENTS.md"), "Codex rules").unwrap();
        std::fs::write(root.join("CLAUDE.md"), "Claude rules").unwrap();
        let mut profile = ProjectProfile {
            description: " Desktop ".into(),
            builds: vec![BuildCommand {
                command: "cargo build".into(),
                purpose: "Development".into(),
            }],
            test_command: "cargo test".into(),
            rules: "Use small outputs".into(),
            ..Default::default()
        };
        profile.validate().unwrap();
        let codex = profile.context(root, Provider::Codex).unwrap();
        assert!(
            codex.contains("Development")
                && codex.contains("cargo test")
                && codex.contains("Use small outputs")
        );
        assert!(!codex.contains("Codex rules") && !codex.contains("Claude rules"));
        let (loaded, count) =
            import_rules(root, &[Provider::Codex, Provider::Claude], &profile.rules).unwrap();
        assert_eq!(count, 2);
        assert!(
            loaded.contains("Use small outputs")
                && loaded.contains("Codex rules")
                && loaded.contains("Claude rules")
        );
        let (again, count) =
            import_rules(root, &[Provider::Codex, Provider::Claude], &loaded).unwrap();
        assert_eq!(count, 0);
        assert_eq!(again, loaded);
        profile.rules = loaded;
        assert!(
            profile
                .context(root, Provider::Claude)
                .unwrap()
                .contains("Codex rules")
        );
        std::fs::write(root.join("AGENTS.override.md"), "Override rules").unwrap();
        let (overridden, _) = import_rules(root, &[Provider::Codex], "").unwrap();
        assert!(overridden.contains("Override rules") && !overridden.contains("Codex rules"));
        std::fs::write(root.join("CLAUDE.md"), "x".repeat(8193)).unwrap();
        assert!(import_rules(root, &[Provider::Codex, Provider::Claude], "Unchanged").is_err());
        assert!(profile.context(root, Provider::Claude).is_ok());
        profile.builds[0].purpose.clear();
        assert!(profile.validate().is_err());
    }
}
