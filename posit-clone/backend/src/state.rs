use std::path::PathBuf;
use uuid::Uuid;

use crate::models::notebook::Notebook;

#[derive(Debug, Clone)]
pub struct AppState {
    pub workspace_dir: PathBuf,
}

impl AppState {
    pub fn new(workspace_dir: PathBuf) -> Self {
        Self { workspace_dir }
    }

    pub fn notebooks_dir(&self) -> PathBuf {
        self.workspace_dir.join("notebooks")
    }

    fn notebook_path(&self, id: Uuid) -> PathBuf {
        self.notebooks_dir().join(format!("{id}.json"))
    }

    pub fn load_notebook(&self, id: Uuid) -> std::io::Result<Option<Notebook>> {
        let path = self.notebook_path(id);
        match std::fs::read_to_string(&path) {
            Ok(contents) => {
                let nb = serde_json::from_str(&contents)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                Ok(Some(nb))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn save_notebook(&self, notebook: &Notebook) -> std::io::Result<()> {
        let dir = self.notebooks_dir();
        std::fs::create_dir_all(&dir)?;
        let path = self.notebook_path(notebook.id);
        let contents = serde_json::to_string_pretty(notebook)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, contents)
    }

    pub fn list_notebooks(&self) -> std::io::Result<Vec<Notebook>> {
        let dir = self.notebooks_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut notebooks = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                match std::fs::read_to_string(&path) {
                    Ok(contents) => {
                        if let Ok(nb) = serde_json::from_str::<Notebook>(&contents) {
                            notebooks.push(nb);
                        }
                    }
                    Err(_) => continue,
                }
            }
        }
        notebooks.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(notebooks)
    }

    pub fn delete_notebook(&self, id: Uuid) -> std::io::Result<bool> {
        let path = self.notebook_path(id);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::notebook::{Language, Notebook};
    use tempfile::TempDir;

    fn make_state() -> (AppState, TempDir) {
        let dir = TempDir::new().unwrap();
        let state = AppState::new(dir.path().to_path_buf());
        (state, dir)
    }

    #[test]
    fn save_and_load_roundtrip() {
        let (state, _dir) = make_state();
        let nb = Notebook::new("Test".into(), Language::Python, &state.notebooks_dir());
        state.save_notebook(&nb).unwrap();
        let loaded = state.load_notebook(nb.id).unwrap().unwrap();
        assert_eq!(loaded.id, nb.id);
        assert_eq!(loaded.name, nb.name);
    }

    #[test]
    fn load_missing_returns_none() {
        let (state, _dir) = make_state();
        let result = state.load_notebook(Uuid::new_v4()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn list_empty_dir_returns_empty() {
        let (state, _dir) = make_state();
        let list = state.list_notebooks().unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn list_returns_saved_notebooks() {
        let (state, _dir) = make_state();
        let nb1 = Notebook::new("A".into(), Language::Python, &state.notebooks_dir());
        let nb2 = Notebook::new("B".into(), Language::R, &state.notebooks_dir());
        state.save_notebook(&nb1).unwrap();
        state.save_notebook(&nb2).unwrap();
        let list = state.list_notebooks().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn delete_existing_returns_true() {
        let (state, _dir) = make_state();
        let nb = Notebook::new("Del".into(), Language::Python, &state.notebooks_dir());
        state.save_notebook(&nb).unwrap();
        assert!(state.delete_notebook(nb.id).unwrap());
        assert!(state.load_notebook(nb.id).unwrap().is_none());
    }

    #[test]
    fn delete_missing_returns_false() {
        let (state, _dir) = make_state();
        assert!(!state.delete_notebook(Uuid::new_v4()).unwrap());
    }
}
