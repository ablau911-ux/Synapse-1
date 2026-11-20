//! Regression tests for #100 (re-review): the `task` facade must enforce the
//! advertised schema bounds (`list.max` in 1..=1000, `concurrency_cap >= 1`)
//! instead of returning an empty-looking success.

use std::num::NonZeroUsize;

use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::{m2::M2ServiceConfig, m3::M3ServiceConfig, m4::M4ServiceConfig};

fn service(dir: &std::path::Path) -> anyhow::Result<SynapseService> {
    SynapseService::try_with_m2_shutdown_reason_and_m3_config(
        CancellationToken::new(),
        "test",
        CancellationToken::new(),
        &M2ServiceConfig::default(),
        M3ServiceConfig::from_cli_parts(
            Some(dir.join("db")),
            Some(dir.to_path_buf()),
            false,
            "127.0.0.1:0".to_owned(),
            NonZeroUsize::new(4).ok_or_else(|| anyhow::anyhow!("nonzero"))?,
            false,
            true,
            None,
            false,
            None,
        ),
        M4ServiceConfig::default(),
    )
}

fn code_of(error: &ErrorData) -> String {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(|code| code.as_str())
        .unwrap_or_default()
        .to_owned()
}

fn list(service: &SynapseService, max: usize) -> Result<TaskListResponse, ErrorData> {
    service.task_list_impl(TaskListParams { state: None, max })
}

#[test]
fn must_fail_task_list_rejects_max_outside_schema_range() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service(temp.path())?;
    for bad in [0_usize, MAX_LIST_TASKS + 1, usize::MAX] {
        let error = list(&service, bad).expect_err("out-of-range max must be refused");
        assert_eq!(
            code_of(&error),
            "TOOL_PARAMS_INVALID",
            "max={bad}: {error:?}"
        );
    }
    Ok(())
}

#[test]
fn must_pass_task_list_accepts_both_range_boundaries() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service(temp.path())?;
    for ok in [1_usize, MAX_LIST_TASKS] {
        let response = list(&service, ok).map_err(|error| anyhow::anyhow!("{error:?}"))?;
        assert!(response.ok, "max={ok}");
        assert_eq!(response.count, 0, "empty queue, max={ok}");
    }
    Ok(())
}

#[test]
fn must_fail_task_next_rejects_zero_concurrency_cap() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service(temp.path())?;
    let error = service
        .task_next_impl(TaskNextParams { concurrency_cap: 0 })
        .expect_err("cap 0 must be refused");
    assert_eq!(code_of(&error), "TOOL_PARAMS_INVALID", "{error:?}");
    let ok = service
        .task_next_impl(TaskNextParams { concurrency_cap: 1 })
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(ok.decision, "empty");
    Ok(())
}

#[tokio::test]
async fn must_fail_dashboard_dispatch_once_rejects_zero_concurrency_cap() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service(temp.path())?;
    let params: TaskDispatchOnceParams = serde_json::from_value(serde_json::json!({
        "concurrency_cap": 0
    }))?;
    let error = service
        .dashboard_task_dispatch_once(params)
        .await
        .expect_err("cap 0 must be refused");
    assert_eq!(code_of(&error), "TOOL_PARAMS_INVALID", "{error:?}");
    Ok(())
}
