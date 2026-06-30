use async_trait::async_trait;
use std::io::Write;

#[async_trait]
pub trait Approver: Send + Sync {
    async fn approve(&self, call_id: &str, tool_name: &str, args: &str) -> bool;
}

pub struct ConsoleApprover;

#[async_trait]
impl Approver for ConsoleApprover {
    async fn approve(&self, _call_id: &str, tool_name: &str, args: &str) -> bool {
        let prompt = format!(
            "Allow tool `{}` with arguments `{}`? [y/N] ",
            tool_name, args
        );
        tokio::task::spawn_blocking(move || {
            let mut stdout = std::io::stdout();
            let _ = write!(stdout, "{}", prompt);
            let _ = stdout.flush();
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            let line = line.trim().to_lowercase();
            matches!(line.as_str(), "y" | "yes" | "ok" | "sure")
        })
        .await
        .unwrap_or(false)
    }
}

pub struct AutoApprover(pub bool);

#[async_trait]
impl Approver for AutoApprover {
    async fn approve(&self, _call_id: &str, _tool_name: &str, _args: &str) -> bool {
        self.0
    }
}
