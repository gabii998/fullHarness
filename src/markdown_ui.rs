use crate::{MarkdownBlock, MarkdownCell, MarkdownRow, render_message};
use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
use slint::{ModelRc, VecModel};

pub fn blocks(role: &str, text: &str) -> ModelRc<MarkdownBlock> {
    let paragraph = |text: &str| MarkdownBlock {
        rendered: render_message(role, text),
        rows: ModelRc::default(),
    };
    if role != "Codex" && role != "Claude" {
        return ModelRc::new(VecModel::from(vec![paragraph(text)]));
    }
    let mut blocks = Vec::new();
    let mut cursor = 0;
    let mut alignments = Vec::new();
    let mut rows = Vec::new();
    let mut cells = Vec::new();
    let mut header = false;
    for (event, range) in Parser::new_ext(text, Options::ENABLE_TABLES).into_offset_iter() {
        match event {
            Event::Start(Tag::Table(columns)) => {
                if !text[cursor..range.start].trim().is_empty() {
                    blocks.push(paragraph(&text[cursor..range.start]));
                }
                alignments = columns;
                rows.clear();
            }
            Event::Start(Tag::TableHead) => {
                header = true;
                cells.clear();
            }
            Event::Start(Tag::TableRow) => {
                header = false;
                cells.clear();
            }
            Event::Start(Tag::TableCell) => {
                let cell = text[range].trim();
                let formatted = if header {
                    format!("**{cell}**")
                } else {
                    cell.into()
                };
                cells.push(MarkdownCell {
                    rendered: render_message(role, &formatted),
                    alignment: match alignments.get(cells.len()) {
                        Some(Alignment::Center) => 1,
                        Some(Alignment::Right) => 2,
                        _ => 0,
                    },
                });
            }
            Event::End(TagEnd::TableHead | TagEnd::TableRow) => {
                rows.push(MarkdownRow {
                    header,
                    cells: ModelRc::new(VecModel::from(std::mem::take(&mut cells))),
                });
            }
            Event::End(TagEnd::Table) => {
                blocks.push(MarkdownBlock {
                    rendered: slint::StyledText::default(),
                    rows: ModelRc::new(VecModel::from(std::mem::take(&mut rows))),
                });
                cursor = range.end;
            }
            _ => {}
        }
    }
    if blocks.is_empty() || !text[cursor..].trim().is_empty() {
        blocks.push(paragraph(&text[cursor..]));
    }
    ModelRc::new(VecModel::from(blocks))
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;

    #[test]
    fn tables_keep_surrounding_text_inline_styles_alignment_and_escaped_pipes() {
        let source = "Antes\n\n| Área | Tecnología |\n| :--- | ---: |\n| Gestión | **Rust** y `Cargo.toml` |\n| Vacía | |\n| Escape | a\\|b |\n\nDespués";
        let output = blocks("Codex", source);
        assert_eq!(output.row_count(), 3);
        let table = output.row_data(1).unwrap();
        assert_eq!(table.rows.row_count(), 4);
        let head = table.rows.row_data(0).unwrap();
        assert!(head.header);
        assert_eq!(head.cells.row_count(), 2);
        assert_eq!(head.cells.row_data(1).unwrap().alignment, 2);
        assert_eq!(
            table
                .rows
                .row_data(1)
                .unwrap()
                .cells
                .row_data(1)
                .unwrap()
                .rendered,
            render_message("Codex", "**Rust** y `Cargo.toml`")
        );
        assert_eq!(
            table
                .rows
                .row_data(3)
                .unwrap()
                .cells
                .row_data(1)
                .unwrap()
                .rendered,
            render_message("Codex", "a\\|b")
        );
        assert_eq!(
            output.row_data(2).unwrap().rendered,
            render_message("Codex", "\nDespués")
        );
        assert_eq!(blocks("Vos", source).row_count(), 1);
    }

    #[test]
    fn fences_and_incomplete_streamed_tables_remain_text_until_valid() {
        for source in [
            "```text\n| A | B |\n| --- | --- |\n| x | y |\n```",
            "| A | B |\n| --",
        ] {
            let output = blocks("Claude", source);
            assert_eq!(output.row_count(), 1);
            assert_eq!(output.row_data(0).unwrap().rows.row_count(), 0);
        }
        let output = blocks("Claude", "A|B\n---|---\nx|y\n\nC|D\n---|---\nz|w");
        assert_eq!(output.row_count(), 2);
        assert_eq!(output.row_data(0).unwrap().rows.row_count(), 2);
        assert_eq!(output.row_data(1).unwrap().rows.row_count(), 2);
    }
}
