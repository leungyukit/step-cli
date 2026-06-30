use crate::chat::session::Session;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: String,
    pub name: Option<String>,
    pub created_at: i64,
    pub session: Session,
    pub files: HashMap<PathBuf, String>,
}

pub struct CheckpointManager {
    dir: PathBuf,
}

impl CheckpointManager {
    pub fn new(dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    pub fn create(
        &self,
        session: &Session,
        workspace: &Path,
        name: Option<String>,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let mut files = HashMap::new();
        Self::snapshot_dir(workspace, workspace, &mut files)?;
        let checkpoint = Checkpoint {
            id: id.clone(),
            name,
            created_at: chrono::Utc::now().timestamp(),
            session: session.clone(),
            files,
        };
        let path = self.dir.join(format!("{}.json", id));
        std::fs::write(path, serde_json::to_string_pretty(&checkpoint)?)?;
        Ok(id)
    }

    fn snapshot_dir(base: &Path, dir: &Path, files: &mut HashMap<PathBuf, String>) -> Result<()> {
        for entry in std::fs::read_dir(dir)?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default();
                if name.starts_with('.') || name == "target" {
                    continue;
                }
                Self::snapshot_dir(base, &path, files)?;
            } else {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if text.len() <= 256 * 1024 {
                        files.insert(path.strip_prefix(base).unwrap_or(&path).to_path_buf(), text);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn restore(&self, id: &str, workspace: &Path) -> Result<Session> {
        let path = self.dir.join(format!("{}.json", id));
        let text = std::fs::read_to_string(&path)?;
        let checkpoint: Checkpoint = serde_json::from_str(&text)?;
        for (rel, content) in &checkpoint.files {
            let target = workspace.join(rel);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&target, content)
                .with_context(|| format!("failed to restore {:?}", target))?;
        }
        Ok(checkpoint.session)
    }

    pub fn list(&self) -> Result<Vec<Checkpoint>> {
        let mut out: Vec<Checkpoint> = Vec::new();
        for entry in std::fs::read_dir(&self.dir)?.flatten() {
            if let Ok(text) = std::fs::read_to_string(entry.path()) {
                if let Ok(cp) = serde_json::from_str(&text) {
                    out.push(cp);
                }
            }
        }
        out.sort_by_key(|b| std::cmp::Reverse(b.created_at));
        Ok(out)
    }
}
