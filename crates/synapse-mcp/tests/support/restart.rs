//! Restart the same isolated test daemon; retain DB, endpoint, and client id.
use super::*;

impl HttpDaemon {
    pub async fn crash_and_restart(&mut self, envs: &[(&str, &str)]) -> anyhow::Result<()> {
        self.child.start_kill().context("crash test daemon")?;
        tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await??;
        let mut command = Command::new(env!("CARGO_BIN_EXE_synapse-mcp"));
        for (key, _) in std::env::vars_os() {
            if key
                .to_str()
                .is_some_and(|k| k.to_ascii_uppercase().starts_with("SYNAPSE_"))
            {
                command.env_remove(key);
            }
        }
        command
            .args(["--mode", "http", "--bind", &self.bind, "--db"])
            .arg(&self.db_path)
            .env("APPDATA", self.home.path())
            .env("LOCALAPPDATA", self.home.path())
            .env("SYNAPSE_LOG_DIR", self.home.path().join("logs"))
            .env("SYNAPSE_BEARER_TOKEN", BEARER)
            .env("SYNAPSE_MCP_DISABLE_OPERATOR_HOTKEY", "1")
            .env("SYNAPSE_USER_ACTIVE_QUIET_MS", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for (key, value) in envs {
            command.env(key, value);
        }
        self.child = command.spawn().context("restart isolated test daemon")?;
        self.wait_for_health().await
    }
}
