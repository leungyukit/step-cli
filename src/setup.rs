use crate::config::Config;
use anyhow::{Context, Result};
use std::io::{self, IsTerminal, Write};

struct Plan {
    name: &'static str,
    description: &'static str,
    base_url: &'static str,
    default_model: &'static str,
    models: &'static [&'static str],
}

const PLANS: &[Plan] = &[
    Plan {
        name: "Step API（按量计费）",
        description: "标准 OpenAI 兼容接口，适合个人/按量使用",
        base_url: "https://api.stepfun.com/v1",
        default_model: "step-3.7-flash",
        models: &[
            "step-3.7-flash",
            "step-2-16k",
            "step-1-8k",
            "step-1-32k",
            "step-1-128k",
        ],
    },
    Plan {
        name: "Step Plan（套餐）",
        description: "Step Plan 接口，使用套餐额度",
        base_url: "https://api.stepfun.com/step_plan/v1",
        default_model: "step-3.7-flash",
        models: &["step-3.7-flash", "step-3.5-flash", "step-3.5-flash-2603"],
    },
    Plan {
        name: "自定义",
        description: "手动输入 base_url 和模型 ID",
        base_url: "",
        default_model: "",
        models: &[],
    },
];

pub async fn run_setup() -> Result<Config> {
    println!("欢迎使用 step-cli！首次使用需要配置 StepFun API。\n");

    println!("请选择使用的 API 类型：");
    for (i, plan) in PLANS.iter().enumerate() {
        println!("  {}. {} — {}", i + 1, plan.name, plan.description);
    }
    let choice = loop {
        let line = prompt("输入数字 (1-3): ").await?;
        match line.trim().parse::<usize>() {
            Ok(n) if n >= 1 && n <= PLANS.len() => break n - 1,
            _ => println!("无效输入，请重新选择。"),
        }
    };
    let plan = &PLANS[choice];

    let base_url = if plan.base_url.is_empty() {
        prompt("请输入 base_url: ").await?
    } else {
        let line = prompt_with_default("base_url", plan.base_url).await?;
        if line.is_empty() {
            plan.base_url.to_string()
        } else {
            line
        }
    };

    let api_key = loop {
        let line = prompt_secret("请输入 API Key (输入隐藏): ").await?;
        if !line.is_empty() {
            break line;
        }
        println!("API Key 不能为空，请重新输入。");
    };

    let model = if plan.models.is_empty() {
        prompt("请输入模型 ID: ").await?
    } else {
        println!("\n可选模型：");
        for (i, m) in plan.models.iter().enumerate() {
            println!("  {}. {}", i + 1, m);
        }
        let line =
            prompt_with_default("模型 ID（可直接输入模型名或数字）", plan.default_model).await?;
        if let Ok(idx) = line.trim().parse::<usize>() {
            plan.models
                .get(idx - 1)
                .map(|s| s.to_string())
                .unwrap_or_else(|| line)
        } else if line.trim().is_empty() {
            plan.default_model.to_string()
        } else {
            line
        }
    };

    let allow_shell = prompt_bool("是否默认允许执行 Shell 命令？ (y/N): ", false).await?;

    let config = Config {
        api_key,
        base_url: base_url.trim_end_matches('/').to_string(),
        model,
        allow_shell,
        ..Default::default()
    };
    config.save().context("failed to save config")?;

    let config_path =
        crate::config::Config::path_display().unwrap_or_else(|| "~/.step/config.toml".to_string());
    println!("\n配置已保存到 {}", config_path);
    println!("  base_url: {}", config.base_url);
    println!("  model: {}", config.model);
    Ok(config)
}

async fn prompt(message: &str) -> Result<String> {
    let message = message.to_string();
    tokio::task::spawn_blocking(move || {
        let mut stdout = io::stdout();
        write!(stdout, "{}", message)?;
        stdout.flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        Ok::<_, anyhow::Error>(line.trim().to_string())
    })
    .await
    .context("prompt task failed")?
}

async fn prompt_with_default(message: &str, default: &str) -> Result<String> {
    let msg = format!("{} [{}]: ", message, default);
    tokio::task::spawn_blocking(move || {
        let mut stdout = io::stdout();
        write!(stdout, "{}", msg)?;
        stdout.flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        let trimmed = line.trim();
        Ok::<_, anyhow::Error>(trimmed.to_string())
    })
    .await
    .context("prompt task failed")?
}

async fn prompt_secret(message: &str) -> Result<String> {
    let message = message.to_string();
    tokio::task::spawn_blocking(move || {
        let mut stdout = io::stdout();
        write!(stdout, "{}", message)?;
        stdout.flush()?;
        let line = if io::stdin().is_terminal() {
            rpassword::read_password()?
        } else {
            let mut buf = String::new();
            io::stdin().read_line(&mut buf)?;
            buf
        };
        Ok::<_, anyhow::Error>(line.trim().to_string())
    })
    .await
    .context("prompt task failed")?
}

async fn prompt_bool(message: &str, default: bool) -> Result<bool> {
    let message = message.to_string();
    tokio::task::spawn_blocking(move || {
        let mut stdout = io::stdout();
        write!(stdout, "{}", message)?;
        stdout.flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        let trimmed = line.trim().to_lowercase();
        Ok::<_, anyhow::Error>(match trimmed.as_str() {
            "y" | "yes" | "true" | "1" => true,
            "n" | "no" | "false" | "0" => false,
            _ => default,
        })
    })
    .await
    .context("prompt task failed")?
}
