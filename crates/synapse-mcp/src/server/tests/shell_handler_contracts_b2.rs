//! criteria_b2.md:5,7. Public MCP shell handlers; real children + disk readbacks.
#![cfg(windows)]
use crate::m4::{self, ActRunShellParams, M4ServiceConfig};
use crate::server::{SynapseService, tool_profiles::ToolProfileKind};
use crate::test_support::{request_context_for_session, service_in};
use rmcp::handler::server::wrapper::Parameters;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use synapse_storage::cf;

const SESSION: &str = "batch2-shell-handler";
const BENIGN: &str = "normal diagnostic token_count=42";
const SECRET: &str = "batch2-real-secret-value";

fn service(path: &std::path::Path, params: &ActRunShellParams) -> anyhow::Result<SynapseService> {
    let mut service = service_in(path)?;
    let command_line = m4::shell_command_line_from_parts(&params.command, &params.args);
    service.m4_config = M4ServiceConfig::from_cli_parts(
        vec![format!("^{}$", regex::escape(&command_line))],
        Vec::new(),
        60_000,
    )?;
    service.m1_state()?.synthetic = Some(crate::m1::synthetic_notepad_input());
    service.set_tool_profile_for_test(SESSION, ToolProfileKind::FullCapability)?;
    Ok(service)
}

fn params(mode: &str, split: bool) -> anyhow::Result<ActRunShellParams> {
    // Split BOTH the sensitive assignment and benign identifier across OS pipe
    // reads. The child constructs secret values; request args contain none.
    let script = if split {
        "[Console]::Out.Write('normal diagnostic token_'); [Console]::Out.Flush(); \
         Start-Sleep -Milliseconds 150; [Console]::Out.WriteLine('count=42'); \
         [Console]::Out.Write('API_TO'); [Console]::Out.Flush(); \
         Start-Sleep -Milliseconds 150; [Console]::Out.WriteLine('KEN='+'batch2-real-'+'secret-value'); \
         [Console]::Error.WriteLine('SECRET_KEY='+'batch2-real-'+'secret-value')"
    } else {
        "Write-Output 'normal diagnostic token_count=42'; \
         Write-Output ('API_TOKEN='+'batch2-real-'+'secret-value'); \
         [Console]::Error.WriteLine('SECRET_KEY='+'batch2-real-'+'secret-value')"
    };
    assert!(!script.contains(SECRET));
    Ok(serde_json::from_value(json!({"command":"powershell.exe",
        "args":["-NoProfile","-NonInteractive","-Command",script],
        "timeout_ms":20_000,"execution_mode":mode}))?)
}

async fn check_output(mode: &str, split: bool) -> anyhow::Result<()> {
    let _jobs = m4::ShellJobRootGuard::new();
    let dir = tempfile::tempdir()?;
    let params = params(mode, split)?;
    let service = service(dir.path(), &params)?;
    let response = service
        .act_run_shell(
            Parameters(params),
            request_context_for_session(SESSION).await,
        )
        .await?
        .0;
    let (stdout, stderr) = if mode == "inline" {
        assert_eq!(response.exit_code, Some(0), "{response:?}");
        (response.stdout, response.stderr)
    } else {
        let job_id = response
            .job_id
            .ok_or_else(|| anyhow::anyhow!("durable handle"))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            let status = service
                .act_run_shell_status(
                    Parameters(serde_json::from_value(json!({
                        "job_id":job_id,"tail_bytes":4096
                    }))?),
                    request_context_for_session(SESSION).await,
                )
                .await?
                .0;
            // completed_at can already be populated while log finalization runs.
            if !matches!(
                status.job.status.as_str(),
                "running" | "cancel_requested" | "finalizing"
            ) {
                break status;
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "child did not reach terminal status: {status:?}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        };
        assert_eq!(status.job.exit_code, Some(0), "{status:?}");
        // Independent physical readback; don't trust sanitized status alone.
        let stdout = std::fs::read_to_string(&status.job.stdout_path)?;
        let stderr = std::fs::read_to_string(&status.job.stderr_path)?;
        assert!(
            !stdout.contains(SECRET) && !stderr.contains(SECRET),
            "secret in durable logs"
        );
        assert!(
            stdout.contains(BENIGN),
            "benign diagnostic lost from disk: {stdout}"
        );
        (status.stdout_tail, status.stderr_tail)
    };
    assert!(
        !stdout.contains(SECRET) && !stderr.contains(SECRET),
        "secret in public result"
    );
    assert!(
        stdout.contains("[redacted-shell-output]") && stderr.contains("[redacted-shell-output]"),
        "sensitive assignments must be masked"
    );
    assert!(
        stdout.contains(BENIGN),
        "benign token_count=42 redacted: {stdout}"
    );
    Ok(())
}

#[tokio::test]
async fn must_pass_inline_handler_keeps_token_count_and_masks_secrets_39() -> anyhow::Result<()> {
    check_output("inline", false).await
}

#[tokio::test]
async fn must_pass_durable_handler_and_logs_keep_token_count_39() -> anyhow::Result<()> {
    check_output("durable", false).await
}

#[tokio::test]
async fn must_pass_split_output_handler_and_logs_keep_token_count_39() -> anyhow::Result<()> {
    check_output("durable", true).await
}

#[tokio::test]
async fn must_fail_crashed_reservation_handler_explicitly_forbids_retry_134() -> anyhow::Result<()>
{
    let _jobs = m4::ShellJobRootGuard::new();
    let dir = tempfile::tempdir()?;
    let work = tempfile::tempdir()?;
    let marker = work.path().join("effect.txt");
    let raw: ActRunShellParams = serde_json::from_value(json!({
        "command":"cmd.exe","args":["/d","/c",format!("echo duplicate>>\"{}\"", marker.display())],
        "timeout_ms":10_000,"idempotency_key":"batch2-crashed","execution_mode":"inline"
    }))?;
    let service_before = service(dir.path(), &raw)?;
    let context = m4::shell_execution_context_for_session(SESSION)?;
    let prepared = m4::prepare_run_shell_params_for_context(raw.clone(), &context)?;
    let auth = m4::authorize_run_shell(&service_before.m4_config, &prepared)?;
    let key = m4::run_shell_idempotency_row_key(&prepared, Some(SESSION))?
        .ok_or_else(|| anyhow::anyhow!("idempotency key"))?;
    let row = m4::run_shell_idempotency_reservation_row(
        &prepared,
        &auth,
        Some(SESSION),
        "dead-generation",
    )?;
    {
        let runtime = service_before.reflex_runtime()?;
        let runtime = runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("runtime lock"))?;
        runtime.storage_put_kv_rows(vec![(key.clone(), row)])?;
        runtime.storage_db().flush()?;
    }
    drop(service_before);
    let service = service(dir.path(), &raw)?;
    let db = {
        let runtime = service.reflex_runtime()?;
        let guard = runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("runtime lock"))?;
        guard.storage_db()
    };
    let before: Value = serde_json::from_slice(
        &db.get_cf_exact(cf::CF_KV, &key)?
            .ok_or_else(|| anyhow::anyhow!("orphan survived restart"))?,
    )?;
    assert_eq!(before["status"], "in_progress");
    assert!(!marker.exists());
    let mut errors = Vec::new();
    for _ in 0..3 {
        let result = service
            .act_run_shell(
                Parameters(raw.clone()),
                request_context_for_session(SESSION).await,
            )
            .await;
        errors.push(
            result
                .err()
                .ok_or_else(|| anyhow::anyhow!("orphan executed"))?,
        );
        let row: Value = serde_json::from_slice(
            &db.get_cf_exact(cf::CF_KV, &key)?
                .ok_or_else(|| anyhow::anyhow!("terminal reservation"))?,
        )?;
        assert_eq!(row["status"], "indeterminate", "{row}");
        assert!(!marker.exists(), "retry ran external side effect");
    }
    for error in errors {
        let data = error
            .data
            .ok_or_else(|| anyhow::anyhow!("typed indeterminate"))?;
        assert_eq!(data["reason"], "idempotency_outcome_indeterminate");
        assert_eq!(data["automatic_retry_safe"], false);
        assert_eq!(
            data["retryable"], false,
            "explicit retry prohibition missing: {data}"
        );
    }
    Ok(())
}
