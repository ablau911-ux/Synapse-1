//! #244 r2 D1 regression evidence, not manual daemon FSV.
//! Real headless Chromium AX -> production cdp_dom mapping. No canned AX output.
//! Private profile/port, never attaches to a human browser.
use std::{
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

use base64::Engine;
use serde_json::Value;
use synapse_core::AccessibleNode;

const HTML: &str = r#"<!doctype html><title>244 AX regression</title>
<input aria-label="Ordinary" value="ordinary-value-244">
<input aria-label="Short credential" type="password" value="a">
<input aria-label="Long credential" type="password" value="long-credential-244">
<input aria-label="Empty credential" type="password" value="">
<input id="runtime" aria-label="Runtime credential" value="runtime-credential-244">
<div id="shadow"></div>
<script>
document.getElementById('runtime').type = 'password';
document.getElementById('shadow').attachShadow({mode:'open'}).innerHTML =
  '<input aria-label="Shadow credential" type="password" value="shadow-credential-244">';
document.body.dataset.ready = 'yes';
</script>"#;

struct OwnedBrowser {
    child: Child,
    // Keep private profile alive until after the owned process tree exits.
    _profile: tempfile::TempDir,
}

impl Drop for OwnedBrowser {
    fn drop(&mut self) {
        // This PID came only from our spawn with a unique profile, never discovery.
        if self.child.try_wait().is_ok_and(|status| status.is_none()) {
            let _ = Command::new("taskkill.exe")
                .args(["/PID", &self.child.id().to_string(), "/T", "/F"])
                .creation_flags(0x0800_0000)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.wait();
    }
}

async fn ax_snapshot() -> anyhow::Result<Vec<AccessibleNode>> {
    let executable = [
        "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
        "C:/Program Files/Microsoft/Edge/Application/msedge.exe",
        "C:/Program Files/Google/Chrome/Application/chrome.exe",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
    .ok_or_else(|| anyhow::anyhow!("headless AX regression requires installed Chromium"))?;
    let profile = tempfile::tempdir()?;
    let page = format!(
        "data:text/html;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(HTML)
    );
    let child = Command::new(executable)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-mode",
            "--remote-debugging-address=127.0.0.1",
            "--remote-debugging-port=0",
        ])
        .arg(format!("--user-data-dir={}", profile.path().display()))
        .arg(page)
        .creation_flags(0x0800_0000)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let port_file = profile.path().join("DevToolsActivePort");
    let mut browser = OwnedBrowser {
        child,
        _profile: profile,
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let port = loop {
        if let Ok(contents) = std::fs::read_to_string(&port_file)
            && let Some(port) = contents
                .lines()
                .next()
                .and_then(|line| line.parse::<u16>().ok())
        {
            break port;
        }
        anyhow::ensure!(
            browser.child.try_wait()?.is_none(),
            "owned Chromium exited before CDP readiness"
        );
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "owned Chromium CDP readiness timed out"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let endpoint = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    let target = loop {
        let targets: Vec<Value> = client
            .get(format!("{endpoint}/json/list"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if let Some(id) = targets
            .iter()
            .find(|target| {
                target["type"] == "page"
                    && target["url"]
                        .as_str()
                        .is_some_and(|url| url.starts_with("data:text/html;base64,"))
            })
            .and_then(|target| target["id"].as_str())
        {
            break id.to_owned();
        }
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "owned fixture target discovery timed out"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    loop {
        let state = synapse_a11y::cdp_evaluate_expression(
            &endpoint,
            &target,
            "document.body && document.body.dataset.ready === 'yes'",
            false,
            true,
        )
        .await?;
        if state.value == true {
            break;
        }
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "owned fixture DOM readiness timed out"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let snapshot = tokio::time::timeout(
        Duration::from_secs(10),
        synapse_a11y::fetch_dom_snapshot(
            &endpoint,
            0x244_244,
            "244 AX regression",
            None,
            Some(&target),
            100,
        ),
    )
    .await??;
    Ok(snapshot.nodes)
}

#[tokio::test]
async fn must_pass_real_ax_ordinary_textbox_retains_value_244_r2_d1() -> anyhow::Result<()> {
    let nodes = ax_snapshot().await?;
    let field = nodes
        .iter()
        .find(|node| node.name == "Ordinary")
        .ok_or_else(|| anyhow::anyhow!("ordinary field absent from real AX tree"))?;
    assert_eq!(field.role, "textbox");
    assert_eq!(field.value.as_deref(), Some("ordinary-value-244"));
    Ok(())
}

#[tokio::test]
async fn must_fail_real_ax_password_textboxes_never_expose_bullet_length_244_r2_d1()
-> anyhow::Result<()> {
    let nodes = ax_snapshot().await?;
    for name in [
        "Short credential",
        "Long credential",
        "Empty credential",
        "Runtime credential",
        "Shadow credential",
    ] {
        let field = nodes
            .iter()
            .find(|node| node.name == name)
            .ok_or_else(|| anyhow::anyhow!("{name} absent from real AX tree"))?;
        assert_eq!(
            field.value.as_deref(),
            Some("[REDACTED]"),
            "Chrome password role is textbox; DOM type/AX protected must replace even masked or empty value: {name}"
        );
        // Accept either explicit metadata or a canonical password role that the
        // production ARIA renderer already maps to is_password:true.
        let wire = serde_json::to_value(field)?;
        assert!(
            field.role.eq_ignore_ascii_case("password")
                || wire["is_password"] == true
                || field
                    .control_state
                    .as_deref()
                    .is_some_and(|state| state.contains("protected=true")
                        || state.contains("is_password=true")),
            "password identity must survive cdp_dom mapping for build_aria_snapshot: {wire}"
        );
    }
    Ok(())
}
