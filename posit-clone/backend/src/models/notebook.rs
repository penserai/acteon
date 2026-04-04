use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CellType {
    Code,
    Markdown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Python,
    R,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OutputType {
    Stdout,
    Stderr,
    Error,
    DisplayData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellOutput {
    pub output_type: OutputType,
    pub content: String,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cell {
    pub id: Uuid,
    pub cell_type: CellType,
    pub source: String,
    pub language: Language,
    pub outputs: Vec<CellOutput>,
    pub execution_count: Option<u32>,
}

impl Cell {
    pub fn new(cell_type: CellType, language: Language) -> Self {
        Self {
            id: Uuid::new_v4(),
            cell_type,
            source: String::new(),
            language,
            outputs: Vec::new(),
            execution_count: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notebook {
    pub id: Uuid,
    pub name: String,
    pub path: PathBuf,
    pub cells: Vec<Cell>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Notebook {
    pub fn new(name: String, language: Language, notebooks_dir: &std::path::Path) -> Self {
        let id = Uuid::new_v4();
        let filename = format!("{id}.json");
        let path = notebooks_dir.join(&filename);
        let now = Utc::now();
        Self {
            id,
            name,
            path,
            cells: vec![Cell::new(CellType::Code, language)],
            created_at: now,
            updated_at: now,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn notebook_new_generates_unique_ids() {
        let dir = Path::new("/tmp");
        let nb1 = Notebook::new("A".into(), Language::Python, dir);
        let nb2 = Notebook::new("B".into(), Language::Python, dir);
        assert_ne!(nb1.id, nb2.id);
    }

    #[test]
    fn notebook_new_sets_timestamps() {
        let dir = Path::new("/tmp");
        let nb = Notebook::new("Test".into(), Language::R, dir);
        assert_eq!(nb.created_at, nb.updated_at);
    }

    #[test]
    fn notebook_new_has_one_initial_cell() {
        let dir = Path::new("/tmp");
        let nb = Notebook::new("Test".into(), Language::Python, dir);
        assert_eq!(nb.cells.len(), 1);
        assert_eq!(nb.cells[0].cell_type, CellType::Code);
        assert_eq!(nb.cells[0].language, Language::Python);
    }

    #[test]
    fn cell_output_roundtrip() {
        let out = CellOutput {
            output_type: OutputType::Stdout,
            content: "hello".into(),
            mime_type: Some("text/plain".into()),
        };
        let json = serde_json::to_string(&out).unwrap();
        let decoded: CellOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.content, "hello");
    }
}
