//! Local structured linter diagnostics. Never runs fixes or invokes an agent.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, sync::watch};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Kind {
    Eslint,
    Clippy,
    Ruff,
    Text,
    Json,
    Sarif,
    Checkstyle,
    Android,
    Phpstan,
    Rubocop,
    Swiftlint,
    Dart,
    Shellcheck,
}
impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Eslint => "ESLint",
            Self::Clippy => "Clippy",
            Self::Ruff => "Ruff",
            Self::Text => "Diagnósticos de texto",
            Self::Json => "JSON",
            Self::Sarif => "SARIF",
            Self::Checkstyle => "Checkstyle",
            Self::Android => "Android Lint",
            Self::Phpstan => "PHPStan",
            Self::Rubocop => "RuboCop",
            Self::Swiftlint => "SwiftLint",
            Self::Dart => "Dart Analyze",
            Self::Shellcheck => "ShellCheck",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tool {
    pub kind: Kind,
    pub program: PathBuf,
    pub args: Vec<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub reports: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Problem {
    pub path: String,
    pub line: u64,
    pub column: u64,
    pub error: bool,
    pub code: String,
    pub message: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Report {
    pub problems: Vec<Problem>,
    pub status: String,
}
pub fn config_bytes(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut data = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(limit as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() > limit {
        return Err(format!("{} supera {} KiB.", path.display(), limit / 1024));
    }
    Ok(data)
}
pub fn detect(root: &Path) -> Vec<Tool> {
    let mut tools = Vec::new();
    if root.join("Cargo.toml").is_file() {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from);
        let cargo = home
            .map(|h| {
                h.join(if cfg!(windows) {
                    ".cargo/bin/cargo.exe"
                } else {
                    ".cargo/bin/cargo"
                })
            })
            .filter(|p| p.is_file())
            .unwrap_or_else(|| "cargo".into());
        tools.push(Tool {
            name: String::new(),
            reports: false,
            kind: Kind::Clippy,
            program: cargo,
            args: ["clippy", "--all-targets", "--message-format=json"]
                .map(String::from)
                .into(),
        });
    }
    let source_list = source_files(
        root,
        &["c", "cpp", "cc", "cxx", "gradle", "kts", "xml", "sh"],
    )
    .unwrap_or_default();
    let eslint = root.join("node_modules/eslint/bin/eslint.js");
    let package: Value = config_bytes(&root.join("package.json"), 256 * 1024)
        .ok()
        .filter(|b| b.len() <= 256 * 1024)
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    if eslint.is_file()
        || package["devDependencies"]["eslint"].is_string()
        || package["dependencies"]["eslint"].is_string()
        || [
            "eslint.config.js",
            "eslint.config.mjs",
            "eslint.config.cjs",
            "eslint.config.ts",
            ".eslintrc.json",
            ".eslintrc.js",
            ".eslintrc.cjs",
            ".eslintrc.yml",
        ]
        .iter()
        .any(|p| root.join(p).is_file())
    {
        tools.push(Tool {
            name: String::new(),
            reports: false,
            kind: Kind::Eslint,
            program: "node".into(),
            args: vec![
                eslint
                    .to_string_lossy()
                    .strip_prefix("\\\\?\\")
                    .unwrap_or(&eslint.to_string_lossy())
                    .to_string(),
                ".".into(),
                "--format".into(),
                "json".into(),
            ],
        });
    }
    let ruff_config = ["ruff.toml", ".ruff.toml"]
        .iter()
        .any(|p| root.join(p).is_file())
        || config_bytes(&root.join("pyproject.toml"), 128 * 1024)
            .and_then(|b| String::from_utf8(b).map_err(|e| e.to_string()))
            .ok()
            .is_some_and(|s| s.contains("[tool.ruff"));
    if ruff_config {
        let ruff = root.join(if cfg!(windows) {
            ".venv/Scripts/ruff.exe"
        } else {
            ".venv/bin/ruff"
        });
        tools.push(Tool {
            name: String::new(),
            reports: false,
            kind: Kind::Ruff,
            program: if ruff.is_file() { ruff } else { "ruff".into() },
            args: ["check", "--output-format", "json", "."]
                .map(String::from)
                .into(),
        });
    }
    let make = |kind, name: &str, program: &str, args: &[&str], reports| Tool {
        kind,
        name: name.into(),
        program: program.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        reports,
    };
    if root.join("pom.xml").is_file() {
        tools.push(make(
            Kind::Checkstyle,
            "Java (Maven Checkstyle)",
            "mvn",
            &["--batch-mode", "checkstyle:checkstyle"],
            true,
        ));
    }
    if root.join("go.mod").is_file() {
        tools.push(make(Kind::Text, "Go vet", "go", &["vet", "./..."], false));
    }
    if root.join("pubspec.yaml").is_file() {
        tools.push(make(
            Kind::Dart,
            "Dart / Flutter",
            "dart",
            &["analyze", "--format", "machine"],
            false,
        ));
    }
    if root.join("composer.json").is_file() {
        tools.push(make(
            Kind::Phpstan,
            "PHPStan",
            "php",
            &[
                "vendor/phpstan/phpstan/phpstan",
                "analyse",
                "--error-format=json",
                "--no-progress",
            ],
            false,
        ));
    }
    if root.join("Gemfile").is_file() {
        tools.push(make(
            Kind::Rubocop,
            "RuboCop",
            "rubocop",
            &["--format", "json"],
            false,
        ));
    }
    if root.join("Package.swift").is_file() || root.join(".swiftlint.yml").is_file() {
        tools.push(make(
            Kind::Swiftlint,
            "SwiftLint",
            "swiftlint",
            &["lint", "--reporter", "json"],
            false,
        ));
    }
    if root.join("compile_commands.json").is_file()
        || root.join("build/compile_commands.json").is_file()
    {
        let sources = source_list
            .iter()
            .filter(|p| {
                Path::new(p)
                    .extension()
                    .is_some_and(|s| ["c", "cpp", "cc", "cxx"].iter().any(|e| s == *e))
            })
            .cloned()
            .collect::<Vec<_>>();
        if !sources.is_empty() {
            let mut tool = make(
                Kind::Text,
                "Clang-Tidy",
                "clang-tidy",
                &[
                    "-p",
                    if root.join("compile_commands.json").is_file() {
                        "."
                    } else {
                        "build"
                    },
                ],
                false,
            );
            tool.args.extend(sources);
            tools.push(tool);
        }
    }
    let cs = std::fs::read_dir(root).ok().is_some_and(|entries| {
        entries.flatten().any(|e| {
            e.path()
                .extension()
                .is_some_and(|s| s == "csproj" || s == "sln" || s == "slnx")
        })
    });
    if cs {
        tools.push(make(
            Kind::Text,
            "C# / .NET",
            "dotnet",
            &[
                "build",
                "--no-restore",
                "--nologo",
                "-clp:ForceConsoleColor;DisableConsoleColor",
            ],
            false,
        ));
    }
    if root.join("gradle/wrapper/gradle-wrapper.jar").is_file() {
        let files = &source_list;
        let android = files.iter().any(|p| p.ends_with("AndroidManifest.xml"))
            || files
                .iter()
                .filter(|p| p.ends_with("gradle") || p.ends_with("gradle.kts"))
                .any(|p| {
                    config_bytes(&root.join(p), 128 * 1024)
                        .and_then(|b| String::from_utf8(b).map_err(|e| e.to_string()))
                        .ok()
                        .is_some_and(|s| {
                            s.contains("com.android.application")
                                || s.contains("com.android.library")
                        })
                });
        let mut base = make(
            if android {
                Kind::Android
            } else {
                Kind::Checkstyle
            },
            if android {
                "Android Lint"
            } else {
                "Java / Kotlin (Gradle)"
            },
            "java",
            &[
                "-classpath",
                "gradle/wrapper/gradle-wrapper.jar",
                "org.gradle.wrapper.GradleWrapperMain",
                if android { "lint" } else { "check" },
                "--console=plain",
                "--no-daemon",
            ],
            true,
        );
        if !android {
            base.args[3] = if files.iter().any(|p| {
                config_bytes(&root.join(p), 128 * 1024)
                    .and_then(|b| String::from_utf8(b).map_err(|e| e.to_string()))
                    .ok()
                    .is_some_and(|s| s.contains("detekt"))
            }) {
                "detekt".into()
            } else {
                "checkstyleMain".into()
            };
        }
        tools.push(base);
    }
    let files = source_list
        .iter()
        .filter(|p| p.ends_with(".sh"))
        .cloned()
        .collect::<Vec<_>>();
    {
        if !files.is_empty() {
            let mut t = make(
                Kind::Shellcheck,
                "ShellCheck",
                "shellcheck",
                &["--format=json"],
                false,
            );
            t.args.extend(files);
            tools.push(t);
        }
    }
    if let Ok(bytes) = config_bytes(&root.join(".harness/lint.json"), 64 * 1024) {
        if bytes.len() <= 64 * 1024 {
            if let Ok(custom) = serde_json::from_slice::<Vec<Tool>>(&bytes) {
                tools.extend(custom.into_iter().take(16));
            }
        }
    }
    for t in &mut tools {
        if t.name.is_empty() {
            t.name = t.kind.name().into();
        }
    }
    tools
}
fn relative(root: &Path, path: &str) -> String {
    let path = Path::new(path);
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let full = full.canonicalize().unwrap_or(full);
    full.strip_prefix(&root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}
fn uri_path(uri: &str) -> String {
    let path = uri.strip_prefix("file://").unwrap_or(uri);
    let path = if path.as_bytes().get(2) == Some(&b':') && path.starts_with('/') {
        &path[1..]
    } else {
        path
    };
    let mut bytes = Vec::new();
    let input = path.as_bytes();
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' && index + 2 < input.len() {
            let hex = |byte: u8| (byte as char).to_digit(16).map(|n| n as u8);
            if let (Some(high), Some(low)) = (hex(input[index + 1]), hex(input[index + 2])) {
                let value = high * 16 + low;
                bytes.push(value);
                index += 3;
                continue;
            }
        }
        bytes.push(input[index]);
        index += 1;
    }
    String::from_utf8(bytes).unwrap_or_else(|_| path.into())
}
pub fn parse(root: &Path, kind: Kind, output: &str) -> Result<Vec<Problem>, String> {
    let mut rows = Vec::new();
    let mut add = |path: &str, line: u64, column: u64, error: bool, code: &str, message: &str| {
        rows.push(Problem {
            path: relative(root, path),
            line,
            column,
            error,
            code: code.into(),
            message: message.chars().take(8192).collect(),
        });
    };
    match kind {
        Kind::Eslint => {
            let files: Vec<Value> = serde_json::from_str(output)
                .map_err(|_| "ESLint no devolvió JSON válido. Revisá el log de ejecución.")?;
            for file in files {
                for message in file["messages"]
                    .as_array()
                    .ok_or("Respuesta ESLint sin messages.")?
                {
                    let severity = message["severity"].as_u64().unwrap_or(0);
                    if severity == 0 {
                        continue;
                    }
                    add(
                        file["filePath"]
                            .as_str()
                            .ok_or("Respuesta ESLint sin archivo.")?,
                        message["line"].as_u64().unwrap_or(0),
                        message["column"].as_u64().unwrap_or(0),
                        severity == 2,
                        message["ruleId"].as_str().unwrap_or("parse"),
                        message["message"]
                            .as_str()
                            .unwrap_or("Diagnóstico sin descripción"),
                    );
                }
            }
        }
        Kind::Ruff => {
            let messages: Vec<Value> = serde_json::from_str(output)
                .map_err(|_| "Ruff no devolvió JSON válido. Revisá el log de ejecución.")?;
            for message in messages {
                add(
                    message["filename"]
                        .as_str()
                        .ok_or("Respuesta Ruff sin archivo.")?,
                    message["location"]["row"].as_u64().unwrap_or(0),
                    message["location"]["column"].as_u64().unwrap_or(0),
                    true,
                    message["code"].as_str().unwrap_or(""),
                    message["message"]
                        .as_str()
                        .unwrap_or("Diagnóstico sin descripción"),
                );
            }
        }
        Kind::Json => {
            rows = serde_json::from_str(output).map_err(|_| "JSON de diagnósticos inválido.")?;
        }
        Kind::Text | Kind::Dart => {
            for line in output.lines() {
                if kind == Kind::Dart {
                    let p: Vec<_> = line.splitn(8, '|').collect();
                    if p.len() == 8 {
                        add(
                            p[3],
                            p[4].parse().unwrap_or(0),
                            p[5].parse().unwrap_or(0),
                            p[0] == "ERROR",
                            p[2],
                            p[7],
                        );
                    }
                } else if let Some((file, row, col, msg)) = text_location(line) {
                    if msg.starts_with("note:") {
                        continue;
                    }
                    add(
                        file,
                        row,
                        col,
                        !msg.to_lowercase().contains("warning"),
                        "",
                        msg,
                    );
                }
            }
        }
        Kind::Android | Kind::Checkstyle => {
            let doc = roxmltree::Document::parse(output)
                .map_err(|_| "El informe XML del linter no es válido.")?;
            for issue in doc
                .descendants()
                .filter(|n| n.has_tag_name("issue") || n.has_tag_name("error"))
            {
                if issue.has_tag_name("issue") {
                    let locations: Vec<_> = issue
                        .children()
                        .filter(|n| n.has_tag_name("location"))
                        .collect();
                    if locations.is_empty() {
                        add(
                            "",
                            0,
                            0,
                            matches!(issue.attribute("severity"), Some("Error" | "Fatal")),
                            issue.attribute("id").unwrap_or(""),
                            issue.attribute("message").unwrap_or(""),
                        );
                    }
                    for loc in locations {
                        add(
                            loc.attribute("file").unwrap_or(""),
                            loc.attribute("line").unwrap_or("0").parse().unwrap_or(0),
                            loc.attribute("column").unwrap_or("0").parse().unwrap_or(0),
                            matches!(issue.attribute("severity"), Some("Error" | "Fatal")),
                            issue.attribute("id").unwrap_or(""),
                            issue.attribute("message").unwrap_or(""),
                        );
                    }
                } else {
                    let file = issue
                        .ancestors()
                        .find(|n| n.has_tag_name("file"))
                        .and_then(|n| n.attribute("name"))
                        .unwrap_or("");
                    add(
                        file,
                        issue.attribute("line").unwrap_or("0").parse().unwrap_or(0),
                        issue
                            .attribute("column")
                            .unwrap_or("0")
                            .parse()
                            .unwrap_or(0),
                        issue.attribute("severity") != Some("warning"),
                        issue.attribute("source").unwrap_or(""),
                        issue.attribute("message").unwrap_or(""),
                    );
                }
            }
        }
        Kind::Phpstan => {
            let data: Value =
                serde_json::from_str(output).map_err(|_| "PHPStan no devolvió JSON válido.")?;
            if let Some(files) = data["files"].as_object() {
                for (file, info) in files {
                    if let Some(messages) = info["messages"].as_array() {
                        for m in messages {
                            add(
                                file,
                                m["line"].as_u64().unwrap_or(0),
                                0,
                                true,
                                m["identifier"].as_str().unwrap_or(""),
                                m["message"].as_str().unwrap_or(""),
                            );
                        }
                    }
                }
            }
            if let Some(errors) = data["errors"].as_array() {
                for e in errors {
                    add("", 0, 0, true, "", e.as_str().unwrap_or("Error PHPStan"));
                }
            }
        }
        Kind::Rubocop => {
            let data: Value =
                serde_json::from_str(output).map_err(|_| "RuboCop no devolvió JSON válido.")?;
            for file in data["files"]
                .as_array()
                .ok_or("Respuesta RuboCop sin archivos.")?
            {
                for m in file["offenses"]
                    .as_array()
                    .ok_or("Respuesta RuboCop sin offenses.")?
                {
                    add(
                        file["path"].as_str().unwrap_or(""),
                        m["location"]["start_line"].as_u64().unwrap_or(0),
                        m["location"]["start_column"].as_u64().unwrap_or(0),
                        matches!(m["severity"].as_str(), Some("error" | "fatal")),
                        m["cop_name"].as_str().unwrap_or(""),
                        m["message"].as_str().unwrap_or(""),
                    );
                }
            }
        }
        Kind::Swiftlint | Kind::Shellcheck => {
            let messages: Vec<Value> =
                serde_json::from_str(output).map_err(|_| "El linter no devolvió JSON válido.")?;
            for m in messages {
                add(
                    m["file"].as_str().unwrap_or(""),
                    m["line"].as_u64().unwrap_or(0),
                    m["column"]
                        .as_u64()
                        .unwrap_or(m["character"].as_u64().unwrap_or(0)),
                    m["severity"].as_str().or(m["level"].as_str()) == Some("error"),
                    m["rule_id"]
                        .as_str()
                        .map(String::from)
                        .unwrap_or_else(|| m["code"].to_string())
                        .as_str(),
                    m["message"].as_str().or(m["reason"].as_str()).unwrap_or(""),
                );
            }
        }
        Kind::Sarif => {
            let data: Value = serde_json::from_str(output).map_err(|_| "SARIF inválido.")?;
            for run in data["runs"].as_array().ok_or("SARIF sin runs.")? {
                for m in run["results"].as_array().unwrap_or(&Vec::new()) {
                    let l = &m["locations"][0]["physicalLocation"];
                    add(
                        &uri_path(l["artifactLocation"]["uri"].as_str().unwrap_or("")),
                        l["region"]["startLine"].as_u64().unwrap_or(0),
                        l["region"]["startColumn"].as_u64().unwrap_or(0),
                        m["level"].as_str().unwrap_or("warning") == "error",
                        m["ruleId"].as_str().unwrap_or(""),
                        m["message"]["text"].as_str().unwrap_or(""),
                    );
                }
            }
        }
        Kind::Clippy => {
            let mut finished = false;
            for line in output.lines().filter(|l| !l.trim().is_empty()) {
                let item: Value = serde_json::from_str(line)
                    .map_err(|_| "Cargo no devolvió JSON válido. Revisá el log de ejecución.")?;
                if item["reason"] == "build-finished" {
                    finished = true;
                }
                if item["reason"] != "compiler-message" {
                    continue;
                }
                let m = &item["message"];
                let level = m["level"].as_str().unwrap_or("");
                if !matches!(level, "warning" | "error" | "failure-note") {
                    continue;
                }
                let span = m["spans"]
                    .as_array()
                    .and_then(|s| s.iter().find(|s| s["is_primary"] == true).or(s.first()));
                add(
                    span.and_then(|s| s["file_name"].as_str()).unwrap_or(""),
                    span.and_then(|s| s["line_start"].as_u64()).unwrap_or(0),
                    span.and_then(|s| s["column_start"].as_u64()).unwrap_or(0),
                    level != "warning",
                    m["code"]["code"].as_str().unwrap_or(""),
                    m["message"]
                        .as_str()
                        .unwrap_or("Diagnóstico sin descripción"),
                );
            }
            if !finished {
                return Err(
                    "Cargo no terminó de informar los diagnósticos. Revisá el log de ejecución."
                        .into(),
                );
            }
        }
    }
    if rows.len() > 20000 {
        return Err(
            "El informe supera 20000 diagnósticos. Consultá la salida completa guardada.".into(),
        );
    }
    rows.sort_by(|a, b| {
        (!a.error, &a.path, a.line, a.column, &a.code, &a.message)
            .cmp(&(!b.error, &b.path, b.line, b.column, &b.code, &b.message))
    });
    rows.dedup();
    Ok(rows)
}
fn text_location(line: &str) -> Option<(&str, u64, u64, &str)> {
    // MSBuild / compiler form: path(line,column): severity code: message.
    if let Some((position, message)) = line.split_once("): ") {
        if let Some((file, coordinates)) = position.rsplit_once('(') {
            let (row, col) = coordinates.split_once(',').unwrap_or((coordinates, "0"));
            return Some((file.trim(), row.parse().ok()?, col.parse().ok()?, message));
        }
    }
    // Keep Windows drive colons in the path. The numeric location is unambiguous.
    let parts: Vec<_> = line.split(':').collect();
    for i in 1..parts.len() {
        if let Ok(row) = parts[i].parse::<u64>() {
            let col = parts.get(i + 1).and_then(|p| p.parse().ok());
            let path_len = parts[..i].iter().map(|p| p.len()).sum::<usize>() + i - 1;
            let start = path_len
                + 1
                + parts[i].len()
                + 1
                + if col.is_some() {
                    parts[i + 1].len() + 1
                } else {
                    0
                };
            if start <= line.len() {
                return Some((
                    &line[..path_len],
                    row,
                    col.unwrap_or(0),
                    line[start..].trim(),
                ));
            }
        }
    }
    None
}
fn source_files(root: &Path, extensions: &[&str]) -> Result<Vec<String>, String> {
    fn walk(
        root: &Path,
        dir: &Path,
        depth: usize,
        budget: &mut usize,
        ext: &[&str],
        files: &mut Vec<String>,
    ) -> Result<(), String> {
        if depth > 16 {
            return Err("El proyecto supera 16 niveles de carpetas.".into());
        }
        for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            *budget += 1;
            if *budget > 30000 {
                return Err(
                    "El proyecto supera el límite de exploración de 30000 entradas.".into(),
                );
            }
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if kind.is_dir() {
                if matches!(
                    name.as_ref(),
                    ".git"
                        | "node_modules"
                        | "target"
                        | "vendor"
                        | ".gradle"
                        | ".venv"
                        | ".idea"
                        | "obj"
                        | "bin"
                ) {
                    continue;
                }
                if dir.file_name().is_some_and(|p| p == "build") && name != "reports" {
                    continue;
                }
                walk(root, &path, depth + 1, budget, ext, files)?;
            } else if path
                .extension()
                .is_some_and(|s| ext.iter().any(|e| s == *e))
            {
                files.push(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(root, root, 0, &mut 0, extensions, &mut files)?;
    files.sort();
    Ok(files)
}
fn parse_reports(
    root: &Path,
    kind: Kind,
    failed: bool,
    started: std::time::SystemTime,
) -> Result<Vec<Problem>, String> {
    let mut files = source_files(root, &["xml", "sarif"])?;
    if kind == Kind::Checkstyle {
        let modules = files
            .iter()
            .filter(|p| p.ends_with("pom.xml"))
            .cloned()
            .collect::<Vec<_>>();
        for module in modules {
            let dir = Path::new(&module).parent().unwrap_or(Path::new(""));
            for name in ["target/checkstyle-result.xml", "target/site/checkstyle.xml"] {
                let path = dir.join(name);
                if root.join(&path).is_file() {
                    files.push(path.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    let mut problems = Vec::new();
    let mut bytes = 0u64;
    let mut found = false;
    for file in files {
        if (!file.contains("/reports/") && !file.contains("target/"))
            || (kind == Kind::Android
                && !file
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .starts_with("lint-results"))
            || (kind == Kind::Checkstyle
                && !file.contains("checkstyle")
                && !file.contains("detekt"))
        {
            continue;
        }
        let path = root.join(&file);
        let metadata = std::fs::metadata(&path).map_err(|e| e.to_string())?;
        if failed && metadata.modified().map_err(|e| e.to_string())? < started {
            continue;
        }
        bytes += metadata.len();
        if bytes > 8 * 1024 * 1024 {
            return Err("Los informes superan 8 MiB. El resultado está incompleto.".into());
        }
        found = true;
        problems.extend(parse(
            root,
            kind,
            &std::fs::read_to_string(path).map_err(|e| e.to_string())?,
        )?);
    }
    if !found {
        return Err("No se generó un informe XML compatible. Activá xmlReport en el linter o configurá otro adaptador.".into());
    }
    problems.sort_by(|a, b| {
        (&a.path, a.line, a.column, &a.code, &a.message)
            .cmp(&(&b.path, b.line, b.column, &b.code, &b.message))
    });
    problems.dedup();
    if problems.len() > 20000 {
        return Err("Más de 20000 diagnósticos; el informe está incompleto.".into());
    }
    Ok(problems)
}
pub fn folder(logs: &Path, root: &Path) -> PathBuf {
    logs.join("lint").join(format!(
        "{:x}",
        Sha256::digest(root.to_string_lossy().as_bytes())
    ))
}
fn resolve_program(root: &Path, program: &Path) -> PathBuf {
    if program.is_absolute() {
        return program.to_path_buf();
    }
    if program.components().count() > 1 {
        return root.join(program);
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let extensions = if cfg!(windows) {
                vec!["exe", "cmd", "bat", ""]
            } else {
                vec![""]
            };
            for extension in extensions {
                let path = if extension.is_empty() {
                    dir.join(program)
                } else {
                    dir.join(program).with_extension(extension)
                };
                if path.is_file() {
                    return path;
                }
            }
        }
    }
    program.to_path_buf()
}
pub fn cached(logs: &Path, root: &Path) -> Option<Report> {
    let path = folder(logs, root).join("report.json");
    if std::fs::metadata(&path).ok()?.len() > 16 * 1024 * 1024 {
        return None;
    }
    serde_json::from_reader(std::fs::File::open(path).ok()?).ok()
}
async fn read(
    mut pipe: impl tokio::io::AsyncRead + Unpin,
    limit: usize,
) -> Result<(String, bool), String> {
    let mut bytes = Vec::new();
    let mut block = [0; 8192];
    let mut overflow = false;
    loop {
        let n = pipe.read(&mut block).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        let keep = n.min(limit.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&block[..keep]);
        overflow |= keep < n;
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), overflow))
}
pub async fn run(
    root: &Path,
    tool: &Tool,
    logs: &Path,
    mut cancel: watch::Receiver<bool>,
) -> Report {
    let start = Instant::now();
    let mut command = tokio::process::Command::new(resolve_program(root, &tool.program));
    command
        .args(&tool.args)
        .current_dir(root)
        .env("NO_COLOR", "1")
        .env("CARGO_TERM_COLOR", "never")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let result = async {
        let mut child = command.spawn().map_err(|e| {
            format!(
                "No se pudo ejecutar {}: {e}. Verificá que esté instalado.",
                tool.name.as_str()
            )
        })?;
        #[cfg(windows)]
        let _job = crate::tool_runtime::ProcessJob::attach(child.id().ok_or("Proceso sin PID.")?)?;
        let out = child.stdout.take().ok_or("No hay stdout.")?;
        let err = child.stderr.take().ok_or("No hay stderr.")?;
        let (out, err) = tokio::try_join!(read(out, 8 * 1024 * 1024), read(err, 256 * 1024))?;
        let exit = child.wait().await.map_err(|e| e.to_string())?;
        Ok::<_, String>((out, err, exit.code()))
    };
    let result = tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(300),result) => result.unwrap_or_else(|_| Err("El linter superó 5 minutos; el informe está incompleto.".into())),
        _ = cancel.changed() => Err("Ejecución cancelada; no hay un informe completo.".into()),
    };
    let directory = folder(logs, root);
    let mut report = Report::default();
    let save = (|| -> Result<(), String> {
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let mut metadata = serde_json::json!({"linter":tool.name.as_str(),"program":tool.program,"args":tool.args,"elapsed_ms":start.elapsed().as_millis()});
        match result {
            Ok(((out, too_large), (err, err_large), exit)) => {
                std::fs::write(directory.join("stdout.log"), &out).map_err(|e| e.to_string())?;
                std::fs::write(directory.join("stderr.log"), &err).map_err(|e| e.to_string())?;
                metadata["exit_code"] = serde_json::json!(exit);
                metadata["truncated"] = serde_json::json!(too_large || err_large);
                let combined = format!("{out}\n{err}");
                report = if too_large || err_large {
                    Report { problems: Vec::new(), status: "Salida excesiva: el informe está incompleto. Consultá los logs acotados.".into() }
                } else {
                    match if tool.reports {
                        parse_reports(
                            root,
                            tool.kind,
                            exit != Some(0),
                            std::time::SystemTime::now() - start.elapsed(),
                        )
                    } else {
                        parse(
                            root,
                            tool.kind,
                            if matches!(tool.kind, Kind::Text | Kind::Dart) {
                                &combined
                            } else {
                                &out
                            },
                        )
                    } {
                        Ok(problems) if exit == Some(0) || !problems.is_empty() => {
                            let errors = problems.iter().filter(|p| p.error).count();
                            let warnings = problems.len() - errors;
                            Report {
                                problems,
                                status: format!(
                                    "{} · {errors} errores · {warnings} advertencias · {:.1}s{}",
                                    tool.name.as_str(),
                                    start.elapsed().as_secs_f32(),
                                    if exit != Some(0)
                                        && (tool.reports
                                            || matches!(tool.kind, Kind::Text | Kind::Clippy))
                                    {
                                        " · ejecución con fallo; el informe puede ser parcial"
                                    } else {
                                        ""
                                    }
                                ),
                            }
                        }
                        Ok(_) => Report {
                            problems: Vec::new(),
                            status: format!(
                                "{} falló sin devolver diagnósticos (salida {:?}). {}",
                                tool.name.as_str(),
                                exit,
                                err.chars().take(700).collect::<String>()
                            ),
                        },
                        Err(error) => Report {
                            problems: Vec::new(),
                            status: format!(
                                "{error}\n{}",
                                err.chars().take(700).collect::<String>()
                            ),
                        },
                    }
                };
            }
            Err(error) => {
                std::fs::write(directory.join("stdout.log"), "").map_err(|e| e.to_string())?;
                std::fs::write(directory.join("stderr.log"), &error).map_err(|e| e.to_string())?;
                report.status = error;
                metadata["error"] = serde_json::json!(report.status);
            }
        }
        std::fs::write(
            directory.join("run.json"),
            serde_json::to_vec_pretty(&metadata).unwrap(),
        )
        .map_err(|e| e.to_string())?;
        std::fs::write(
            directory.join("report.json"),
            serde_json::to_vec(&report).unwrap(),
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(error) = save {
        report
            .status
            .push_str(&format!("\nNo se pudo guardar el log: {error}"));
    }
    report
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_preserve_locations_severity_and_reject_broken_output() {
        let root = tempfile::tempdir().unwrap();
        let eslint = r#"[{"filePath":"a.ts","messages":[{"severity":2,"line":4,"column":7,"ruleId":"no-undef","message":"Undefined"},{"severity":1,"line":8,"message":"Unused"}]}]"#;
        let p = parse(root.path(), Kind::Eslint, eslint).unwrap();
        assert_eq!(p.len(), 2);
        assert!(p[0].error);
        assert_eq!((p[0].line, p[0].column), (4, 7));
        let p = parse(root.path(),Kind::Ruff,r#"[{"filename":"a.py","location":{"row":2,"column":1},"code":"F401","message":"unused"}]"#).unwrap();
        assert_eq!(p[0].code, "F401");
        let cargo = "{\"reason\":\"compiler-message\",\"message\":{\"level\":\"warning\",\"message\":\"Unused\",\"code\":{\"code\":\"clippy::unused\"},\"spans\":[{\"file_name\":\"src/lib.rs\",\"is_primary\":true,\"line_start\":3,\"column_start\":5}]}}\n{\"reason\":\"build-finished\",\"success\":true}";
        assert_eq!(
            parse(root.path(), Kind::Clippy, cargo).unwrap()[0].path,
            "src/lib.rs"
        );
        assert!(parse(root.path(), Kind::Clippy, "").is_err());
        assert!(parse(root.path(), Kind::Eslint, "broken").is_err());
    }
    #[test]
    fn detects_local_tools_without_executing_or_installing() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "").unwrap();
        std::fs::write(
            root.path().join("package.json"),
            r#"{"devDependencies":{"eslint":"9"}}"#,
        )
        .unwrap();
        std::fs::write(root.path().join("ruff.toml"), "").unwrap();
        let tools = detect(root.path());
        assert_eq!(tools.len(), 3);
        assert!(
            tools
                .iter()
                .all(|t| !t.args.iter().any(|a| a == "--fix" || a == "install"))
        );
    }
    #[test]
    fn android_checkstyle_go_dotnet_and_other_common_formats() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let android = r#"<issues><issue id="UnusedResources" severity="Warning" message="Unused &amp; unused"><location file="app/src/a.xml" line="4" column="8"/></issue></issues>"#;
        let rows = parse(root, Kind::Android, android).unwrap();
        assert_eq!(rows[0].line, 4);
        assert!(!rows[0].error);
        assert_eq!(rows[0].message, "Unused & unused");
        let checkstyle = r#"<checkstyle><file name="src/A.java"><error line="7" column="2" severity="error" message="Bad style" source="Rule"/></file></checkstyle>"#;
        assert_eq!(
            parse(root, Kind::Checkstyle, checkstyle).unwrap()[0].path,
            "src/A.java"
        );
        let rows=parse(root,Kind::Text,"# pkg\nC:\\repo\\main.go:12:8: bad call\nC:\\repo\\Program.cs(4,7): warning CS001: unused").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].line, 12);
        assert!(
            rows.iter()
                .any(|p| p.code.is_empty() && !p.error && p.column == 7)
        );
        assert_eq!(
            parse(
                root,
                Kind::Dart,
                "ERROR|COMPILE_TIME_ERROR|UNDEFINED_NAME|lib/main.dart|4|7|1|Undefined name"
            )
            .unwrap()[0]
                .code,
            "UNDEFINED_NAME"
        );
        assert_eq!(parse(root,Kind::Phpstan,r#"{"files":{"a.php":{"messages":[{"line":8,"message":"Undefined","identifier":"type"}]}},"errors":[]}"#).unwrap()[0].line,8);
        assert_eq!(parse(root,Kind::Rubocop,r#"{"files":[{"path":"a.rb","offenses":[{"severity":"warning","message":"Unused","cop_name":"Lint/Unused","location":{"start_line":3,"start_column":2}}]}]}"#).unwrap()[0].line,3);
        assert_eq!(parse(root,Kind::Swiftlint,r#"[{"file":"a.swift","line":2,"character":3,"severity":"warning","rule_id":"style","reason":"Style"}]"#).unwrap()[0].column,3);
        assert_eq!(parse(root,Kind::Shellcheck,r#"[{"file":"a.sh","line":2,"column":3,"level":"error","code":123,"message":"Bad"}]"#).unwrap()[0].code,"123");
        assert!(parse(root, Kind::Android, "broken").is_err());
        assert_eq!(uri_path("src/mi%20archivo.cs"), "src/mi archivo.cs");
        assert_eq!(uri_path("src/%aá.cs"), "src/%aá.cs");
        let sarif = r#"{"runs":[{"results":[{"ruleId":"CS001","level":"error","message":{"text":"Undefined"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"src/mi%20archivo.cs"},"region":{"startLine":2,"startColumn":4}}}]}]}]}"#;
        let p = parse(root, Kind::Sarif, sarif).unwrap();
        assert_eq!(p[0].path, "src/mi archivo.cs");
        assert_eq!(p[0].column, 4);
    }
}
