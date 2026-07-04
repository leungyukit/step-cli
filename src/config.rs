use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    #[serde(default)]
    pub allow_shell: bool,
    #[serde(default)]
    pub yolo: bool,
    #[serde(default)]
    pub trust: bool,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default = "default_max_rounds")]
    pub max_rounds: usize,
    #[serde(default = "default_context_threshold")]
    pub context_threshold: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: default_base_url(),
            model: default_model(),
            workspace: None,
            allow_shell: false,
            yolo: false,
            trust: false,
            max_tokens: None,
            temperature: None,
            max_rounds: default_max_rounds(),
            context_threshold: default_context_threshold(),
        }
    }
}

fn default_base_url() -> String {
    "https://api.stepfun.com/v1".to_string()
}

fn default_model() -> String {
    "step-2-16k".to_string()
}

fn default_max_rounds() -> usize {
    30
}

fn default_context_threshold() -> f32 {
    0.8
}

fn config_path() -> Result<PathBuf> {
    if let Some(p) = env::var_os("STEP_CONFIG_PATH") {
        return Ok(PathBuf::from(p));
    }
    let home = dirs::home_dir().context("cannot determine home directory")?;
    Ok(home.join(".step").join("config.toml"))
}

fn load_file(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config {:?}", path))?;
    let cfg: Config =
        toml::from_str(&text).with_context(|| format!("failed to parse config {:?}", path))?;
    Ok(cfg)
}

fn env_bool(key: &str) -> Option<bool> {
    env::var(key)
        .ok()
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

fn env_usize(key: &str) -> Option<usize> {
    env::var(key).ok().and_then(|v| v.parse().ok())
}

fn env_path(key: &str) -> Option<PathBuf> {
    env::var_os(key).map(PathBuf::from)
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path()?;
        let mut cfg = load_file(&path)?;

        if let Ok(v) = env::var("STEPFUN_API_KEY") {
            cfg.api_key = v;
        }
        if let Some(v) = env::var("STEP_BASE_URL").ok().filter(|s| !s.is_empty()) {
            cfg.base_url = v;
        }
        if let Some(v) = env::var("STEP_MODEL").ok().filter(|s| !s.is_empty()) {
            cfg.model = v;
        }
        if let Some(v) = env_path("STEP_WORKSPACE") {
            cfg.workspace = Some(v);
        }
        if let Some(v) = env_bool("STEP_ALLOW_SHELL") {
            cfg.allow_shell = v;
        }
        if let Some(v) = env_bool("STEP_YOLO") {
            cfg.yolo = v;
        }
        if let Some(v) = env_bool("STEP_TRUST") {
            cfg.trust = v;
        }
        if let Some(v) = env_usize("STEP_MAX_ROUNDS") {
            cfg.max_rounds = v;
        }
        if let Some(v) = env::var("STEP_CONTEXT_THRESHOLD")
            .ok()
            .and_then(|s| s.parse::<f32>().ok())
        {
            cfg.context_threshold = v.clamp(0.1, 1.0);
        }

        if cfg.workspace.is_none() {
            cfg.workspace = Some(env::current_dir()?);
        }

        // Trim trailing slash from base_url.
        cfg.base_url = cfg.base_url.trim_end_matches('/').to_string();

        Ok(cfg)
    }

    pub fn apply_cli(&mut self, cli: &crate::cli::Cli) {
        if let Some(url) = cli.base_url.as_deref().filter(|s| !s.is_empty()) {
            self.base_url = url.trim_end_matches('/').to_string();
        }
        if let Some(model) = cli.model.as_deref().filter(|s| !s.is_empty()) {
            self.model = model.to_string();
        }
        if let Some(ws) = cli.workspace.clone() {
            self.workspace = Some(ws);
        }
        if cli.yolo {
            self.yolo = true;
        }
        if cli.trust {
            self.trust = true;
        }
        if cli.max_rounds != 30 {
            self.max_rounds = cli.max_rounds;
        }
        if let Some(threshold) = cli.context_threshold {
            self.context_threshold = threshold.clamp(0.1, 1.0);
        }
    }

    pub fn sessions_dir(&self) -> Result<PathBuf> {
        let home = dirs::home_dir().context("cannot determine home directory")?;
        let dir = home.join(".step").join("sessions");
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    pub fn state_dir(&self) -> Result<PathBuf> {
        let home = dirs::home_dir().context("cannot determine home directory")?;
        let dir = home.join(".step").join("state");
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(&path, text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn config_file_exists() -> bool {
        config_path().map(|p| p.exists()).unwrap_or(false)
    }

    pub fn path_display() -> Option<String> {
        config_path().ok().map(|p| p.to_string_lossy().to_string())
    }
}
