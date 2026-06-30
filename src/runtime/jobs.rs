use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JobStatus {
    Running,
    Completed { exit_code: Option<i32> },
    Failed { reason: String },
    Orphaned { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub command: String,
    pub cwd: PathBuf,
    pub status: JobStatus,
    #[serde(default)]
    pub stdout_preview: String,
    #[serde(default)]
    pub stderr_preview: String,
}

pub struct JobManager {
    state_file: PathBuf,
    jobs: HashMap<String, Job>,
    handles: HashMap<String, Child>,
}

impl JobManager {
    pub fn new(state_file: PathBuf) -> Result<Self> {
        let jobs: HashMap<String, Job> = if state_file.exists() {
            let text = std::fs::read_to_string(&state_file)?;
            serde_json::from_str(&text).unwrap_or_default()
        } else {
            HashMap::new()
        };
        let mut manager = Self {
            state_file,
            jobs,
            handles: HashMap::new(),
        };
        manager.mark_orphaned();
        manager.save()?;
        Ok(manager)
    }

    fn mark_orphaned(&mut self) {
        for job in self.jobs.values_mut() {
            if matches!(job.status, JobStatus::Running) {
                job.status = JobStatus::Orphaned {
                    reason: "session restarted".to_string(),
                };
            }
        }
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.state_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(&self.jobs)?;
        std::fs::write(&self.state_file, text)?;
        Ok(())
    }

    pub fn list(&self) -> Vec<&Job> {
        let mut jobs: Vec<&Job> = self.jobs.values().collect();
        jobs.sort_by(|a, b| a.id.cmp(&b.id));
        jobs
    }

    pub async fn spawn(&mut self, command: String, cwd: PathBuf) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&command);
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg(&command);
            c
        };
        cmd.current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let job = Job {
            id: id.clone(),
            command: command.clone(),
            cwd: cwd.clone(),
            status: JobStatus::Running,
            stdout_preview: String::new(),
            stderr_preview: String::new(),
        };
        self.jobs.insert(id.clone(), job);
        self.save()?;
        self.handles.insert(id.clone(), child);

        // Wait task.
        let state_file = self.state_file.clone();
        let job_id = id.clone();
        tokio::spawn(async move {
            let mut out = Vec::new();
            let mut err = Vec::new();
            if let Some(stdout) = stdout {
                let mut reader = BufReader::new(stdout);
                let _ = reader.read_to_end(&mut out).await;
            }
            if let Some(stderr) = stderr {
                let mut reader = BufReader::new(stderr);
                let _ = reader.read_to_end(&mut err).await;
            }
            // Child is in handles map; we wait on it there when reaped.
            let out_preview = String::from_utf8_lossy(&out[..out.len().min(4096)]).to_string();
            let err_preview = String::from_utf8_lossy(&err[..err.len().min(4096)]).to_string();
            let _ = (state_file, job_id, out_preview, err_preview);
        });

        Ok(id)
    }

    pub async fn reap(&mut self, id: &str) -> Result<Option<Job>> {
        if let Some(mut child) = self.handles.remove(id) {
            let status = child.wait().await?;
            if let Some(job) = self.jobs.get_mut(id) {
                job.status = JobStatus::Completed {
                    exit_code: status.code(),
                };
            }
            self.save()?;
        }
        Ok(self.jobs.get(id).cloned())
    }

    pub async fn cancel(&mut self, id: &str) -> Result<()> {
        if let Some(mut child) = self.handles.remove(id) {
            let _ = child.kill().await;
            if let Some(job) = self.jobs.get_mut(id) {
                job.status = JobStatus::Failed {
                    reason: "cancelled by user".to_string(),
                };
            }
            self.save()?;
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&Job> {
        self.jobs.get(id)
    }
}
