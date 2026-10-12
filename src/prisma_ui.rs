use crate::{AppWindow, DatabaseEntity, DatabaseField, DatabaseRelation};
use ascurra_harness::{
    prisma::{self, Diagram},
    runtime::Supervisor,
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

pub fn display(ui: &AppWindow, diagram: Diagram) {
    let entities = diagram
        .entities
        .iter()
        .map(|entity| DatabaseEntity {
            name: entity.name.as_str().into(),
            detail: if entity.mapped.is_empty() {
                entity.kind.clone()
            } else {
                format!("{} · {}", entity.kind, entity.mapped)
            }
            .into(),
            x: entity.x,
            y: entity.y,
            height: entity.height,
            fields: ModelRc::new(VecModel::from(
                entity
                    .fields
                    .iter()
                    .map(|field| {
                        let tags = [
                            if field.primary { "PK" } else { "" },
                            if field.foreign { "FK" } else { "" },
                            if field.unique {
                                "UQ"
                            } else if field.unique_composite {
                                "UQ*"
                            } else {
                                ""
                            },
                        ]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ");
                        DatabaseField {
                            name: if field.mapped.is_empty() {
                                field.name.clone()
                            } else {
                                format!("{} → {}", field.name, field.mapped)
                            }
                            .into(),
                            kind: format!(
                                "{}{}{}",
                                field.kind,
                                if field.list { "[]" } else { "" },
                                if field.optional { "?" } else { "" }
                            )
                            .into(),
                            tags: tags.into(),
                            key: field.primary || field.foreign,
                        }
                    })
                    .collect::<Vec<_>>(),
            )),
        })
        .collect::<Vec<_>>();
    let relations = diagram
        .relations
        .iter()
        .map(|r| DatabaseRelation {
            x1: r.x1,
            y1: r.y1,
            mid: r.mid,
            x2: r.x2,
            y2: r.y2,
            label: format!(
                "{}.{} → {} · {}",
                r.source, r.field, r.target, r.cardinality
            )
            .into(),
            detail: if r.implicit {
                "Relación lógica · sin FK declarada en este lado".into()
            } else {
                format!(
                    "FK {}({}) → {}({})",
                    r.source,
                    r.keys.join(", "),
                    r.target,
                    r.references.join(", ")
                )
                .into()
            },
        })
        .collect::<Vec<_>>();
    ui.set_database_status(
        format!(
            "{} entidades · {} relaciones{}",
            entities.len(),
            relations.len(),
            if diagram.warnings.is_empty() {
                String::new()
            } else {
                format!(
                    "\n{}{}",
                    diagram
                        .warnings
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n"),
                    if diagram.warnings.len() > 3 {
                        format!("\n{} advertencias más", diagram.warnings.len() - 3)
                    } else {
                        String::new()
                    }
                )
            }
        )
        .into(),
    );
    ui.set_database_width(diagram.width);
    ui.set_database_height(diagram.height);
    ui.set_database_entities(ModelRc::new(VecModel::from(entities)));
    ui.set_database_relations(ModelRc::new(VecModel::from(relations)));
}
pub fn configure(ui: &AppWindow, supervisor: Arc<Supervisor>) {
    let generation = Arc::new(AtomicU64::new(0));
    let selected = Arc::new(Mutex::new(HashMap::<String, String>::new()));
    let weak = ui.as_weak();
    let service = supervisor.clone();
    ui.on_refresh_database(move |discover| {
        let Some(ui) = weak.upgrade() else { return; };
        if !ui.get_database_open() { return; }
        let project = ui.get_git_project().to_string();
        if project.is_empty() { return; }
        let current = ui.get_database_schema().to_string();
        let schema = if current.trim().is_empty() { selected.lock().unwrap().get(&project).cloned().unwrap_or_default() } else { current };
        let request = generation.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let check = generation.clone(); let cache = selected.clone(); let handle = ui.as_weak();
        ui.set_database_busy(true);
        ui.set_database_status("Leyendo esquema…".into());
        service.executor.spawn(async move {
            let path = project.clone();
            let requested = schema.clone();
            let result = tokio::task::spawn_blocking(move || -> Result<(Option<Vec<String>>, String, Result<Diagram, String>), String> {
                let choices = if discover { Some(prisma::discover(Path::new(&path))?) } else { None };
                let schema = if requested.is_empty() { choices.as_ref().and_then(|c| c.first()).cloned().ok_or("No se encontró un esquema Prisma. Indicá la ruta de un archivo o carpeta.")? } else { requested };
                let diagram = prisma::load(Path::new(&path), &schema);
                Ok((choices, schema, diagram))
            }).await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = handle.upgrade_in_event_loop(move |ui| {
                if check.load(Ordering::Relaxed) != request { return; }
                ui.set_database_busy(false);
                if ui.get_git_project().as_str() != project { ui.invoke_refresh_database(true); return; }
                match result {
                    Ok((choices, schema, diagram)) => {
                        if let Some(choices) = choices {
                            ui.set_database_schema_index(choices.iter().position(|p| p == &schema).unwrap_or(0) as i32);
                            ui.set_database_schemas(ModelRc::new(VecModel::from(choices.into_iter().map(Into::into).collect::<Vec<slint::SharedString>>())));
                        }
                        ui.set_database_schema(schema.clone().into());
                        match diagram {
                            Ok(diagram) => {
                                cache.lock().unwrap().insert(project, schema);
                                display(&ui, diagram);
                            }
                            Err(error) => {
                                ui.set_database_entities(ModelRc::default()); ui.set_database_relations(ModelRc::default());
                                ui.set_database_status(error.chars().take(1200).collect::<String>().into());
                            }
                        }
                    }
                    Err(error) => {
                        ui.set_database_entities(ModelRc::default()); ui.set_database_relations(ModelRc::default());
                        ui.set_database_status(error.chars().take(1200).collect::<String>().into());
                    }
                }
            });
        });
    });
    let weak = ui.as_weak();
    ui.on_browse_database_schema(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let project = ui.get_git_project().to_string();
        if let Some(path) = rfd::FileDialog::new()
            .set_directory(&project)
            .add_filter("Prisma", &["prisma"])
            .pick_file()
        {
            ui.set_database_schema(path.to_string_lossy().as_ref().into());
            ui.invoke_refresh_database(false);
        }
    });
}
