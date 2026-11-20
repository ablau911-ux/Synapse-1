//! NEEDS-SEAM: scoped cache root and decoder IO override, used by resolve_video.
use super::*;
use crate::server::g7b_support::{code, request_context_for_session};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn must_pass_video_url_cache_uses_repeatable_install_keyed_identity_174() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let media = dir.path().join("download.mp4");
    std::fs::write(&media, b"g7b media")?;
    assert_eq!(std::fs::read(&media)?, b"g7b media");
    let args = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
    let observed = args.clone();
    let output_path = media.clone();
    let decoder = Arc::new(move |_program: &str, supplied: &[String]| {
        observed.lock().expect("args").push(supplied.to_vec());
        Ok(Output {
            status: success_status_for_test(),
            stdout: format!("{}\n", output_path.display()).into_bytes(),
            stderr: Vec::new(),
        })
    });
    let url = "https://example.test/video?code=g7b-secret";
    let context = request_context_for_session("g7b").await;
    assert_eq!(args.lock().expect("before").len(), 0);
    for _ in 0..2 {
        let resolved = with_video_resolver_test_io(
            dir.path().into(),
            decoder.clone(),
            resolve_video(url, &context),
        )
        .await?;
        assert_eq!(resolved, media.to_string_lossy());
    }
    let calls = args.lock().expect("after");
    assert_eq!(calls.len(), 2);
    let expected = dir.path().join(format!(
        "{}.%(ext)s",
        crate::content_digest::keyed_text_digest_hex(url)
    ));
    for call in calls.iter() {
        let index = call
            .iter()
            .position(|arg| arg == "-o")
            .expect("output option");
        assert_eq!(call[index + 1], expected.to_string_lossy());
        assert_eq!(call.last().map(String::as_str), Some(url));
    }
    Ok(())
}

#[tokio::test]
async fn must_fail_decoder_empty_result_refuses_without_creating_cache_file_174()
-> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);
    let decoder = Arc::new(|_program: &str, _args: &[String]| {
        Ok(Output {
            status: success_status_for_test(),
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    });
    let context = request_context_for_session("g7b").await;
    let result = with_video_resolver_test_io(
        dir.path().into(),
        decoder,
        resolve_video("https://example.test/video?code=g7b-secret", &context),
    )
    .await;
    let error = result.expect_err("empty decoder output");
    assert_eq!(code(&error), "TOOL_PARAMS_INVALID");
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);
    Ok(())
}

#[cfg(windows)]
fn success_status_for_test() -> std::process::ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(0)
}

#[cfg(unix)]
fn success_status_for_test() -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(0)
}
