//! Read-only, deterministic Prisma schema diagrams. No processes or network.
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
struct Token {
    value: String,
    quoted: bool,
}
fn lex(source: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = source.chars().collect();
    let mut i = 0;
    let mut tokens = Vec::new();
    while i < chars.len() {
        let ch = chars[i];
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        if ch == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if ch == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            let mut depth = 1;
            while i < chars.len() && depth > 0 {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if depth > 0 {
                return Err("Comentario sin cerrar.".into());
            }
            continue;
        }
        if ch == '"' || ch == '\'' || ch == '`' {
            let mark = ch;
            i += 1;
            let mut value = String::new();
            while i < chars.len() && chars[i] != mark {
                if chars[i] == '\\' {
                    i += 1;
                    if i >= chars.len() {
                        return Err("Cadena sin cerrar.".into());
                    }
                }
                value.push(chars[i]);
                i += 1;
            }
            if i == chars.len() {
                return Err("Cadena sin cerrar.".into());
            }
            i += 1;
            tokens.push(Token {
                value,
                quoted: true,
            });
            continue;
        }
        let mut value = String::new();
        if ch.is_alphanumeric() || ch == '_' || ch == '$' {
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$')
            {
                value.push(chars[i]);
                i += 1;
            }
        } else {
            value.push(ch);
            i += 1;
        }
        tokens.push(Token {
            value,
            quoted: false,
        });
    }
    Ok(tokens)
}
fn at(tokens: &[Token], i: usize, value: &str) -> bool {
    tokens.get(i).is_some_and(|t| !t.quoted && t.value == value)
}
fn ident(token: &Token) -> bool {
    !token.quoted
        && token
            .value
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
}
fn group(tokens: &[Token], i: &mut usize) -> Result<Vec<Token>, String> {
    group_depth(tokens, i, 0)
}
fn group_depth(tokens: &[Token], i: &mut usize, depth: usize) -> Result<Vec<Token>, String> {
    if depth > 128 {
        return Err("El esquema contiene demasiados grupos anidados.".into());
    }
    let open = tokens.get(*i).ok_or("Grupo incompleto.")?.value.as_str();
    let close = match open {
        "(" => ")",
        "[" => "]",
        "{" => "}",
        _ => return Err("Grupo inválido.".into()),
    };
    *i += 1;
    let start = *i;
    while *i < tokens.len() {
        if at(tokens, *i, close) {
            let content = tokens[start..*i].to_vec();
            *i += 1;
            return Ok(content);
        }
        if !tokens[*i].quoted && matches!(tokens[*i].value.as_str(), "(" | "[" | "{") {
            group_depth(tokens, i, depth + 1)?;
        } else if !tokens[*i].quoted && matches!(tokens[*i].value.as_str(), ")" | "]" | "}") {
            return Err("Delimitadores incompatibles en el esquema.".into());
        } else {
            *i += 1;
        }
    }
    Err("Grupo sin cerrar en el esquema.".into())
}
#[derive(Clone, Debug, Default)]
pub struct Field {
    pub name: String,
    pub kind: String,
    pub optional: bool,
    pub list: bool,
    pub primary: bool,
    pub unique: bool,
    pub unique_composite: bool,
    pub foreign: bool,
    pub mapped: String,
    relation: String,
    keys: Vec<String>,
    references: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct Entity {
    pub name: String,
    pub mapped: String,
    pub kind: String,
    pub fields: Vec<Field>,
    pub x: f32,
    pub y: f32,
    pub height: f32,
}
#[derive(Clone, Debug)]
pub struct Relation {
    pub source: String,
    pub target: String,
    pub field: String,
    pub cardinality: String,
    pub keys: Vec<String>,
    pub references: Vec<String>,
    pub implicit: bool,
    pub x1: f32,
    pub y1: f32,
    pub mid: f32,
    pub x2: f32,
    pub y2: f32,
}
#[derive(Clone, Debug, Default)]
pub struct Diagram {
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
    pub width: f32,
    pub height: f32,
    pub warnings: Vec<String>,
}
fn list(args: &[Token], key: Option<&str>) -> Vec<String> {
    let start = key
        .and_then(|key| args.iter().position(|t| t.value == key).map(|i| i + 1))
        .unwrap_or(0);
    let Some(open) = args[start..]
        .iter()
        .position(|t| !t.quoted && t.value == "[")
        .map(|i| i + start)
    else {
        return Vec::new();
    };
    let mut result = Vec::new();
    let mut depth = 0usize;
    let mut first = true;
    for token in &args[open + 1..] {
        if !token.quoted && token.value == "]" && depth == 0 {
            break;
        }
        if depth == 0 && first && ident(token) {
            result.push(token.value.clone());
            first = false;
        }
        if !token.quoted {
            match token.value.as_str() {
                "(" | "[" => depth += 1,
                ")" | "]" => depth = depth.saturating_sub(1),
                "," if depth == 0 => first = true,
                _ => {}
            }
        }
    }
    result
}
fn quoted(args: &[Token]) -> String {
    args.iter()
        .find(|t| t.quoted)
        .map(|t| t.value.clone())
        .unwrap_or_default()
}
fn attributes(tokens: &[Token], i: &mut usize) -> Result<Vec<(bool, String, Vec<Token>)>, String> {
    let mut result = Vec::new();
    while at(tokens, *i, "@") {
        *i += 1;
        let block = at(tokens, *i, "@");
        if block {
            *i += 1;
        }
        let token = tokens
            .get(*i)
            .filter(|t| ident(t))
            .ok_or("Atributo incompleto.")?;
        let mut name = token.value.clone();
        *i += 1;
        while at(tokens, *i, ".") {
            *i += 1;
            name.push('.');
            name.push_str(&tokens.get(*i).ok_or("Atributo incompleto.")?.value);
            *i += 1;
        }
        let args = if at(tokens, *i, "(") {
            group(tokens, i)?
        } else {
            Vec::new()
        };
        result.push((block, name, args));
    }
    Ok(result)
}
pub fn parse(source: &str) -> Result<Diagram, String> {
    let tokens = lex(source)?;
    let mut i = 0;
    let mut entities = Vec::new();
    while i < tokens.len() {
        let kind = tokens[i].value.clone();
        i += 1;
        if !matches!(
            kind.as_str(),
            "model" | "view" | "enum" | "type" | "datasource" | "generator"
        ) {
            return Err(format!("Bloque desconocido: {kind}"));
        }
        let name = tokens
            .get(i)
            .filter(|t| ident(t))
            .ok_or("Falta el nombre de un bloque.")?
            .value
            .clone();
        i += 1;
        if !at(&tokens, i, "{") {
            return Err(format!("Falta {{ en {name}."));
        }
        let body = group(&tokens, &mut i)?;
        if matches!(kind.as_str(), "datasource" | "generator") {
            continue;
        }
        if entities.iter().any(|e: &Entity| e.name == name) {
            return Err(format!(
                "Modelo duplicado: {name}. Revisá qué carpeta de esquema seleccionaste."
            ));
        }
        let mut entity = Entity {
            name,
            mapped: String::new(),
            kind,
            fields: Vec::new(),
            x: 0.,
            y: 0.,
            height: 0.,
        };
        let mut k = 0;
        let mut block_keys = Vec::new();
        while k < body.len() {
            if at(&body, k, "@") {
                for (_, attr, args) in attributes(&body, &mut k)? {
                    if attr == "map" {
                        entity.mapped = quoted(&args);
                    }
                    if attr == "id" || attr == "unique" {
                        block_keys.push((attr, list(&args, None)));
                    }
                }
                continue;
            }
            let name = body
                .get(k)
                .filter(|t| ident(t))
                .ok_or("Nombre de campo inválido.")?
                .value
                .clone();
            k += 1;
            if entity.fields.iter().any(|f| f.name == name) {
                return Err(format!("Campo duplicado: {}.{name}", entity.name));
            }
            let mut field = Field {
                name,
                ..Default::default()
            };
            if entity.kind != "enum" {
                field.kind = body
                    .get(k)
                    .filter(|t| ident(t))
                    .ok_or("Falta el tipo de un campo.")?
                    .value
                    .clone();
                k += 1;
                if field.kind == "Unsupported" && at(&body, k, "(") {
                    field.kind = format!("Unsupported({})", quoted(&group(&body, &mut k)?));
                }
                if at(&body, k, "[") {
                    let values = group(&body, &mut k)?;
                    if !values.is_empty() {
                        return Err("Tipo de lista inválido.".into());
                    }
                    field.list = true;
                }
                if at(&body, k, "?") {
                    field.optional = true;
                    k += 1;
                }
            }
            for (block, attr, args) in attributes(&body, &mut k)? {
                if block {
                    if attr == "map" {
                        entity.mapped = quoted(&args);
                    }
                    if attr == "id" || attr == "unique" {
                        block_keys.push((attr, list(&args, None)));
                    }
                    continue;
                }
                match attr.as_str() {
                    "id" => field.primary = true,
                    "unique" => field.unique = true,
                    "map" => field.mapped = quoted(&args),
                    "relation" => {
                        field.relation = args
                            .first()
                            .filter(|t| t.quoted)
                            .map(|t| t.value.clone())
                            .unwrap_or_else(|| {
                                args.windows(3)
                                    .find(|w| {
                                        w[0].value == "name" && w[1].value == ":" && w[2].quoted
                                    })
                                    .map(|w| w[2].value.clone())
                                    .unwrap_or_default()
                            });
                        if args.iter().any(|t| t.value == "fields") {
                            field.keys = list(&args, Some("fields"));
                        }
                        if args.iter().any(|t| t.value == "references") {
                            field.references = list(&args, Some("references"));
                        }
                    }
                    _ => {}
                }
            }
            entity.fields.push(field);
        }
        for (kind, names) in block_keys {
            let composite = names.len() > 1;
            for name in names {
                let field = entity
                    .fields
                    .iter_mut()
                    .find(|f| f.name == name)
                    .ok_or_else(|| format!("Clave desconocida: {}.{name}", entity.name))?;
                if kind == "id" {
                    field.primary = true;
                } else {
                    if composite {
                        field.unique_composite = true;
                    } else {
                        field.unique = true;
                    }
                }
            }
        }
        let foreign: Vec<_> = entity.fields.iter().flat_map(|f| f.keys.clone()).collect();
        for key in foreign {
            entity
                .fields
                .iter_mut()
                .find(|f| f.name == key)
                .ok_or_else(|| format!("FK desconocida: {}.{key}", entity.name))?
                .foreign = true;
        }
        entities.push(entity);
        if entities.len() > 200 || entities.iter().map(|e| e.fields.len()).sum::<usize>() > 4000 {
            return Err("El diagrama admite hasta 200 entidades y 4000 campos.".into());
        }
    }
    if entities.is_empty() {
        return Err("El esquema no contiene modelos, vistas, enums o tipos.".into());
    }
    entities.sort_by(|a, b| a.name.cmp(&b.name));
    let mut heights = [24f32; 3];
    let columns = entities.len().min(3);
    for (index, entity) in entities.iter_mut().enumerate() {
        let col = index % columns; // fixed deterministic columns
        entity.x = 24. + col as f32 * 330.;
        entity.y = heights[col];
        entity.height = 40. + entity.fields.len() as f32 * 22.;
        heights[col] += entity.height + 72.;
    }
    let lookup: HashMap<_, _> = entities
        .iter()
        .enumerate()
        .map(|(i, e)| (e.name.as_str(), i))
        .collect();
    let mut relations = Vec::new();
    let mut visited = HashSet::new();
    let mut warnings = Vec::new();
    for explicit in [true, false] {
        for (source_index, entity) in entities
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == "model" || e.kind == "view")
        {
            for (field_index, field) in entity.fields.iter().enumerate() {
                if (!field.keys.is_empty()) != explicit
                    || visited.contains(&(source_index, field_index))
                {
                    continue;
                }
                let Some(&target_index) = lookup.get(field.kind.as_str()) else {
                    if !matches!(
                        field.kind.as_str(),
                        "String"
                            | "Int"
                            | "BigInt"
                            | "Float"
                            | "Decimal"
                            | "Boolean"
                            | "DateTime"
                            | "Json"
                            | "Bytes"
                    ) && !field.kind.starts_with("Unsupported(")
                    {
                        return Err(format!(
                            "Tipo o modelo no encontrado: {}. Cargá la carpeta si el esquema está dividido en archivos.",
                            field.kind
                        ));
                    }
                    continue;
                };
                let target = &entities[target_index];
                if target.kind != "model" && target.kind != "view" {
                    continue;
                }
                if field.keys.len() != field.references.len() {
                    return Err(format!(
                        "{}.{}: fields y references deben tener la misma cantidad de campos.",
                        entity.name, field.name
                    ));
                }
                for name in &field.references {
                    if !target.fields.iter().any(|f| &f.name == name) {
                        return Err(format!("Referencia desconocida: {}.{name}", target.name));
                    }
                }
                let inverses: Vec<_> = target
                    .fields
                    .iter()
                    .enumerate()
                    .filter(|(index, f)| {
                        f.kind == entity.name
                            && f.relation == field.relation
                            && !(target_index == source_index && *index == field_index)
                    })
                    .collect();
                if inverses.len() > 1 {
                    warnings.push(format!("Relación ambigua: {}.{}", entity.name, field.name));
                }
                let inverse = if inverses.len() == 1 {
                    Some(inverses[0])
                } else {
                    None
                };
                let cardinal = |f: &Field| {
                    if f.list {
                        "0..N"
                    } else if f.optional {
                        "0..1"
                    } else {
                        "1"
                    }
                };
                let cardinality = format!(
                    "{} → {}",
                    inverse.map(|(_, f)| cardinal(f)).unwrap_or("?"),
                    cardinal(field)
                );
                visited.insert((source_index, field_index));
                if let Some((index, _)) = inverse {
                    visited.insert((target_index, index));
                }
                let same = entity.x == target.x;
                let from_right = same || entity.x < target.x;
                let x1 = entity.x + if from_right { 260. } else { 0. };
                let x2 = target.x + if same || !from_right { 260. } else { 0. };
                let y1 = entity.y + 40. + field_index as f32 * 22. + 11.;
                let y2 = target.y + 40. + inverse.map(|(i, _)| i as f32 * 22. + 11.).unwrap_or(11.);
                let mid = if same {
                    x1 + 20. + (relations.len() % 4) as f32 * 8.
                } else {
                    (x1 + x2) / 2.
                };
                if relations.len() >= 1000 {
                    return Err("El esquema supera las 1000 relaciones de esta vista.".into());
                }
                relations.push(Relation {
                    source: entity.name.clone(),
                    target: target.name.clone(),
                    field: field.name.clone(),
                    cardinality,
                    keys: field.keys.clone(),
                    references: field.references.clone(),
                    implicit: field.keys.is_empty(),
                    x1,
                    y1,
                    mid,
                    x2,
                    y2,
                });
            }
        }
    }
    Ok(Diagram {
        width: entities.len().min(3) as f32 * 330. + 24.,
        height: heights.into_iter().fold(0f32, f32::max) + 24.,
        entities,
        relations,
        warnings,
    })
}

fn read(root: &Path, path: &Path, limit: usize) -> Result<String, String> {
    let canonical = path.canonicalize().map_err(|e| e.to_string())?;
    if !canonical.starts_with(root) {
        return Err("El esquema apunta fuera del proyecto.".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(canonical)
        .map_err(|e| e.to_string())?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("El esquema supera el límite de 1 MB.".into());
    }
    String::from_utf8(bytes)
        .map(|s| s.trim_start_matches('\u{feff}').to_string())
        .map_err(|_| "El esquema debe usar UTF-8.".into())
}
fn walk(
    root: &Path,
    directory: &Path,
    depth: usize,
    budget: &mut usize,
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if depth > 8 {
        return Err(
            "La búsqueda supera 8 niveles. Indicá una carpeta de esquema más específica.".into(),
        );
    }
    let entries = fs::read_dir(directory).map_err(|e| e.to_string())?;
    for entry in entries {
        *budget += 1;
        if *budget > 8000 {
            return Err("La búsqueda supera 8000 entradas. Indicá la ruta del esquema.".into());
        }
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(
            name.as_ref(),
            "node_modules"
                | ".git"
                | "target"
                | "dist"
                | "build"
                | ".next"
                | ".cache"
                | "migrations"
        ) {
            continue;
        }
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            walk(root, &path, depth + 1, budget, files)?;
        } else if path.extension().is_some_and(|e| e == "prisma") {
            files.push(
                path.strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_path_buf(),
            );
        }
    }
    Ok(())
}
pub fn discover(root: &Path) -> Result<Vec<String>, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut choices = Vec::new();
    for config in [
        "prisma.config.ts",
        "prisma.config.js",
        "prisma.config.mjs",
        "prisma.config.mts",
        ".config/prisma.ts",
    ] {
        let path = root.join(config);
        if path.is_file() {
            if let Ok(tokens) = lex(&read(&root, &path, 128 * 1024)?) {
                if let Some(value) = tokens.windows(3).find(|w| {
                    w[0].value == "schema"
                        && w[1].value == ":"
                        && w[2].quoted
                        && !w[2].value.contains("${")
                }) {
                    choices.push(value[2].value.clone());
                }
            }
        }
    }
    let package = root.join("package.json");
    if package.is_file() {
        if let Ok(value) =
            serde_json::from_str::<serde_json::Value>(&read(&root, &package, 128 * 1024)?)
        {
            if let Some(path) = value["prisma"]["schema"].as_str() {
                choices.push(path.into());
            }
        }
    }
    let mut files = Vec::new();
    walk(&root, &root, 0, &mut 0, &mut files)?;
    files.sort_by_key(|p| {
        (
            p != Path::new("prisma/schema.prisma"),
            p.file_name().is_none_or(|n| n != "schema.prisma"),
            p.clone(),
        )
    });
    for file in files {
        choices.push(file.to_string_lossy().replace('\\', "/"));
    }
    let mut seen = HashSet::new();
    choices.retain(|p| seen.insert(p.clone()));
    Ok(choices)
}
pub fn load(root: &Path, schema: &str) -> Result<Diagram, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let path = root.join(schema);
    let canonical = path.canonicalize().map_err(|e| e.to_string())?;
    if !canonical.starts_with(&root) {
        return Err("Seleccioná un esquema dentro del proyecto.".into());
    }
    let mut sources = Vec::new();
    if canonical.is_dir() {
        walk(&root, &canonical, 0, &mut 0, &mut sources)?;
        sources.sort();
    } else {
        sources.push(
            canonical
                .strip_prefix(&root)
                .map_err(|e| e.to_string())?
                .to_path_buf(),
        );
    }
    if sources.is_empty() {
        return Err("No hay archivos .prisma en la carpeta seleccionada.".into());
    }
    let mut text = String::new();
    for file in sources {
        let source = read(&root, &root.join(file), 1024 * 1024)?;
        if text.len() + source.len() > 1024 * 1024 {
            return Err("El conjunto de esquemas supera 1 MB.".into());
        }
        text.push_str(&source);
        text.push('\n');
    }
    parse(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relations_keys_enums_maps_and_multiline_attributes_are_structural() {
        let schema = r#"// model Fake { id Int }
        datasource db { provider = "postgresql" url = env("DATABASE_URL") }
        model User { id Int @id email String @unique posts Post[] roles Role[] @relation("Membership") @@map("users") }
        model Role { id Int @id users User[] @relation("Membership") }
        model Post {
            id Int @id
            ownerId Int
            owner User @relation(
                fields: [ownerId], references: [id], onDelete: Cascade)
            status Status @default(ACTIVE)
            title String @default("A } // B")
        }
        enum Status { ACTIVE INACTIVE }
        model Pair { a Int b Int @@id([a, b]) }
        model Tree { id Int @id parentId Int? parent Tree? @relation("Tree", fields: [parentId], references: [id]) children Tree[] @relation("Tree") }
        "#;
        let diagram = parse(schema).unwrap();
        assert_eq!(diagram.entities.len(), 6);
        assert_eq!(diagram.relations.len(), 3);
        let post = diagram.entities.iter().find(|e| e.name == "Post").unwrap();
        assert!(
            post.fields
                .iter()
                .find(|f| f.name == "ownerId")
                .unwrap()
                .foreign
        );
        let edge = diagram
            .relations
            .iter()
            .find(|e| e.source == "Post")
            .unwrap();
        assert_eq!(edge.cardinality, "0..N → 1");
        assert_eq!(edge.keys, ["ownerId"]);
        assert!(
            diagram
                .relations
                .iter()
                .any(|e| e.implicit && e.source != e.target)
        );
        assert!(
            diagram
                .entities
                .iter()
                .find(|e| e.name == "Pair")
                .unwrap()
                .fields
                .iter()
                .all(|f| f.primary)
        );
        assert_eq!(
            diagram
                .entities
                .iter()
                .find(|e| e.name == "User")
                .unwrap()
                .mapped,
            "users"
        );
        assert!(parse("model A { id Int @id").is_err());
        assert!(parse("model A { id Int @id } model A { id Int }").is_err());
        assert!(
            parse("model A { id Int @id b B @relation(fields: [missing], references: [id]) }")
                .is_err()
        );
    }
    #[test]
    fn compound_keys_ignore_sort_options_and_nesting_is_bounded() {
        let d = parse("model A { a Int b Int @@unique([a(sort: Desc), b]) }").unwrap();
        assert!(
            d.entities[0]
                .fields
                .iter()
                .all(|f| f.unique_composite && !f.unique)
        );
        let nested = format!(
            "model A {{ a Int @default({}0{}) }}",
            "(".repeat(140),
            ")".repeat(140)
        );
        assert!(parse(&nested).is_err());
    }
    #[test]
    fn discovery_and_multifile_loading_are_local_and_do_not_execute_config() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("prisma")).unwrap();
        fs::write(
            root.join("prisma.config.ts"),
            "throw new Error('must never execute'); export default {schema: './prisma'};",
        )
        .unwrap();
        fs::write(
            root.join("prisma/user.prisma"),
            "model User {id Int @id posts Post[]}",
        )
        .unwrap();
        fs::write(root.join("prisma/post.prisma"), "model Post {id Int @id userId Int user User @relation(fields: [userId], references: [id])}").unwrap();
        fs::create_dir(root.join("node_modules")).unwrap();
        fs::write(root.join("node_modules/ignored.prisma"), "bad schema").unwrap();
        let choices = discover(root).unwrap();
        assert_eq!(choices[0], "./prisma");
        assert!(choices.iter().all(|p| !p.contains("node_modules")));
        let diagram = load(root, &choices[0]).unwrap();
        assert_eq!(diagram.relations.len(), 1);
        assert!(load(root, "../outside.prisma").is_err());
        fs::write(root.join("prisma/bad.prisma"), "x".repeat(1024 * 1024 + 1)).unwrap();
        assert!(load(root, &choices[0]).is_err());
    }
}
