use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub path: PathBuf,
    pub content: String,
}

pub struct SkillRegistry {
    skills: HashMap<String, Skill>,
}

impl SkillRegistry {
    pub fn load(global_dir: &Path, workspace: Option<&Path>) -> Result<Self> {
        let mut skills = HashMap::new();
        let mut scan = |dir: &Path| {
            for entry in WalkDir::new(dir).max_depth(3).into_iter().flatten() {
                if entry.file_name() == "SKILL.md" {
                    let parent = entry.path().parent().unwrap_or(dir);
                    let name = parent
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown");
                    if let Ok(content) = std::fs::read_to_string(entry.path()) {
                        skills.insert(
                            name.to_string(),
                            Skill {
                                name: name.to_string(),
                                path: entry.path().to_path_buf(),
                                content,
                            },
                        );
                    }
                }
            }
        };
        if global_dir.exists() {
            scan(global_dir);
        }
        if let Some(ws) = workspace {
            let agents = ws.join(".agents").join("skills");
            if agents.exists() {
                scan(&agents);
            }
        }
        Ok(Self { skills })
    }

    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    pub fn list(&self) -> Vec<&Skill> {
        self.skills.values().collect()
    }

    pub fn system_prompt_extras(&self) -> String {
        if self.skills.is_empty() {
            return String::new();
        }
        let mut text = String::from("\n\nAvailable skills:\n");
        for skill in self.skills.values() {
            text.push_str(&format!("- {}: {}\n", skill.name, skill.path.display()));
        }
        text.push_str("Use /skill <name> to view a skill's content when relevant.\n");
        text
    }
}
