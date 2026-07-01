//! StepFun开放平台登录状态管理。
//!
//! 启动 step-cli 前要求用户先登录 https://platform.stepfun.com，
//! 登录信息（cookie / token）保存在 `~/.step/state/auth.json`。

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

const PLATFORM_URL: &str = "https://platform.stepfun.com";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlatformAuth {
    pub username: Option<String>,
    pub session_cookie: Option<String>,
    pub token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

fn auth_file_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("cannot determine home directory")?;
    Ok(home.join(".step").join("state").join("auth.json"))
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

impl PlatformAuth {
    pub fn load() -> Result<Self> {
        let path = auth_file_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read auth file {:?}", path))?;
        let auth: PlatformAuth = serde_json::from_str(&text)
            .with_context(|| format!("failed to parse auth file {:?}", path))?;
        Ok(auth)
    }

    pub fn save(&self) -> Result<()> {
        let path = auth_file_path()?;
        ensure_parent(&path)?;
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn logout() -> Result<()> {
        let path = auth_file_path()?;
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    pub fn is_authenticated(&self) -> bool {
        if self.session_cookie.is_none() && self.token.is_none() {
            return false;
        }
        if let Some(exp) = self.expires_at {
            return exp > Utc::now();
        }
        true
    }

    pub fn auth_file_display() -> Option<String> {
        auth_file_path()
            .ok()
            .map(|p| p.to_string_lossy().to_string())
    }

    /// 交互式登录流程。
    pub async fn login_interactive() -> Result<Self> {
        println!("请先登录 StepFun 开放平台: {}\n", PLATFORM_URL);
        println!("登录方式：");
        println!("  1. 浏览器登录（推荐）");
        println!("  2. 用户名/密码登录（尝试）");
        let choice = loop {
            print!("请选择 (1-2): ");
            io::stdout().flush()?;
            let mut line = String::new();
            io::stdin().read_line(&mut line)?;
            match line.trim() {
                "1" => break 1,
                "2" => break 2,
                _ => println!("无效输入，请重新选择。"),
            }
        };

        if choice == 1 {
            Self::browser_login().await
        } else {
            Self::password_login().await
        }
    }

    async fn browser_login() -> Result<Self> {
        println!("\n正在启动浏览器，请在打开的窗口中完成登录...");
        match browser_login_auto().await {
            Ok(auth) => {
                auth.save()?;
                println!("\n登录信息已自动获取并保存。");
                Ok(auth)
            }
            Err(e) => {
                eprintln!("\n自动获取登录信息失败: {}", e);
                Self::browser_login_manual().await
            }
        }
    }

    async fn browser_login_manual() -> Result<Self> {
        println!("\n请在浏览器中完成登录，然后返回这里。");
        let _ = open::that(PLATFORM_URL);

        println!("\n登录完成后，请粘贴浏览器中的 session cookie 或 platform token:");
        let cookie = prompt_secret("Cookie / Token (输入隐藏): ")?;
        if cookie.is_empty() {
            anyhow::bail!("登录信息不能为空");
        }

        let username = prompt("StepFun 用户名/邮箱 (可选，用于标识): ")?;
        let username = if username.is_empty() {
            None
        } else {
            Some(username)
        };

        let auth = Self {
            username,
            session_cookie: Some(cookie),
            token: None,
            // 默认 7 天过期，平台实际过期时间可能不同。
            expires_at: Some(Utc::now() + chrono::Duration::days(7)),
        };
        auth.save()?;
        println!("\n登录信息已保存。");
        Ok(auth)
    }

    async fn password_login() -> Result<Self> {
        let username = prompt("用户名/邮箱: ")?;
        let password = prompt_secret("密码: ")?;
        if username.is_empty() || password.is_empty() {
            anyhow::bail!("用户名和密码不能为空");
        }

        println!("\n正在尝试登录 {} ...", PLATFORM_URL);
        match try_platform_login(&username, &password).await {
            Ok(auth) => {
                auth.save()?;
                println!("\n登录成功，登录信息已保存。");
                Ok(auth)
            }
            Err(e) => {
                eprintln!("\n自动登录失败: {}", e);
                println!("请使用浏览器登录方式，手动粘贴 cookie/token。");
                Self::browser_login().await
            }
        }
    }
}

async fn try_platform_login(username: &str, password: &str) -> Result<PlatformAuth> {
    // StepFun 开放平台的具体登录接口未公开文档，这里使用一个通用尝试：
    // 1. 获取登录页以收集必要 cookie。
    // 2. 向常见登录端点提交用户名密码。
    // 如果平台实际接口不同，会失败并回退到浏览器手动登录。
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    // 先访问首页，建立 cookie jar。
    let _ = client.get(PLATFORM_URL).send().await;

    // 尝试常见的 JSON 登录端点。
    let login_body = serde_json::json!({
        "username": username,
        "password": password,
        "email": username,
    });

    let endpoints = [
        format!("{}/api/auth/login", PLATFORM_URL),
        format!("{}/api/login", PLATFORM_URL),
        format!("{}/auth/login", PLATFORM_URL),
    ];

    for url in &endpoints {
        let resp = client.post(url).json(&login_body).send().await;
        if let Ok(resp) = resp {
            let status = resp.status();
            if status.is_success() {
                // 尝试从响应 JSON 中读取 token，否则从 cookie jar 中读取 session。
                let cookies: Vec<String> = resp
                    .cookies()
                    .map(|c| format!("{}={}", c.name(), c.value()))
                    .collect();
                let token: Option<String> =
                    resp.json::<serde_json::Value>().await.ok().and_then(|v| {
                        v.get("token")
                            .or_else(|| v.get("accessToken"))
                            .and_then(|t| t.as_str().map(|s| s.to_string()))
                    });

                return Ok(PlatformAuth {
                    username: Some(username.to_string()),
                    session_cookie: if cookies.is_empty() {
                        None
                    } else {
                        Some(cookies.join("; "))
                    },
                    token,
                    expires_at: Some(Utc::now() + chrono::Duration::days(7)),
                });
            }
        }
    }

    anyhow::bail!("未能识别 StepFun 开放平台的登录接口")
}

fn prompt(message: &str) -> Result<String> {
    print!("{}", message);
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn prompt_secret(message: &str) -> Result<String> {
    print!("{}", message);
    io::stdout().flush()?;
    let line = if io::stdin().is_terminal() {
        rpassword::read_password()?
    } else {
        let mut buf = String::new();
        io::stdin().read_line(&mut buf)?;
        buf
    };
    Ok(line.trim().to_string())
}

async fn browser_login_auto() -> Result<PlatformAuth> {
    use chromiumoxide::browser::{Browser, BrowserConfig};
    use futures_util::StreamExt;
    use std::time::Duration;

    let (mut browser, mut handler) = Browser::launch(
        BrowserConfig::builder()
            .with_head()
            .window_size(1280, 900)
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build browser config: {e}"))?,
    )
    .await
    .context("failed to launch Chrome/Chromium")?;

    // Drive the browser event loop in the background.
    tokio::spawn(async move { while let Some(_event) = handler.next().await {} });

    let page = browser
        .new_page(PLATFORM_URL)
        .await
        .context("failed to open platform page")?;

    println!("浏览器已打开，请在窗口中登录 StepFun 开放平台。");
    println!("登录成功后程序会自动读取 session cookie（120 秒超时）。");

    let start = tokio::time::Instant::now();
    let timeout = Duration::from_secs(120);
    let mut last_cookie_count = 0;

    while start.elapsed() < timeout {
        tokio::time::sleep(Duration::from_secs(2)).await;

        let current_url = page.url().await.unwrap_or_default().unwrap_or_default();
        let on_dashboard = !current_url.is_empty()
            && (current_url.contains("/console")
                || current_url.contains("/dashboard")
                || current_url.contains("/workspace")
                || current_url.contains("/projects")
                || !current_url.contains("/login"));

        let cookies = page
            .get_cookies()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|c| !c.value.is_empty())
            .collect::<Vec<_>>();

        if cookies.is_empty() {
            continue;
        }

        // Detect login by navigating away from the login page or by session-like cookies.
        let has_session = cookies.iter().any(|c| {
            let name = c.name.to_lowercase();
            name.contains("session")
                || name.contains("token")
                || name.contains("auth")
                || name.contains("step")
        });

        if on_dashboard || has_session || cookies.len() > last_cookie_count {
            last_cookie_count = cookies.len();
            // Wait a bit more for cookies to settle after redirect.
            tokio::time::sleep(Duration::from_secs(2)).await;
            let cookies = page.get_cookies().await.unwrap_or_default();
            let cookie_str = cookies
                .into_iter()
                .filter(|c| !c.value.is_empty())
                .map(|c| format!("{}={}", c.name, c.value))
                .collect::<Vec<_>>()
                .join("; ");

            if !cookie_str.is_empty() {
                let _ = browser.close().await;
                return Ok(PlatformAuth {
                    username: None,
                    session_cookie: Some(cookie_str),
                    token: None,
                    expires_at: Some(Utc::now() + chrono::Duration::days(7)),
                });
            }
        }
    }

    let _ = browser.close().await;
    anyhow::bail!("等待登录超时")
}
