//! #59 re-review regression: when a descendant keeps a stdout/stderr pipe open past the
//! drain cap, the capped drain must return the bytes it already collected (marked
//! truncated) instead of aborting the reader and returning an empty buffer while the log
//! claims "Returning partial output".

use std::time::Duration;

use tokio::io::AsyncWriteExt;

use super::*;

const SHORT_CAP: Duration = Duration::from_millis(150);

async fn let_reader_consume() {
    tokio::time::sleep(Duration::from_millis(50)).await;
}

async fn joined(task: CappedStreamTask, name: &'static str) -> CappedOutput {
    match join_capped_stream_with_cap(task, name, SHORT_CAP).await {
        Ok(output) => output,
        Err(error) => panic!("join {name}: {error:?}"),
    }
}

#[tokio::test]
async fn must_fail_already_buffered_bytes_survive_a_drain_cap_abort_59() {
    let (mut writer, reader) = tokio::io::duplex(64 * 1024);
    if let Err(error) = writer.write_all(b"partial output").await {
        panic!("write: {error}");
    }
    let task = spawn_capped_stream(reader);
    let_reader_consume().await;

    // `writer` stays open: an escaped descendant holds the pipe, so EOF never arrives.
    let output = joined(task, "stdout").await;

    assert_eq!(output.bytes, b"partial output");
    assert!(
        output.truncated,
        "an unfinished drain must be reported truncated"
    );
    drop(writer);
}

#[tokio::test]
async fn must_pass_complete_output_is_returned_untruncated_59() {
    let (mut writer, reader) = tokio::io::duplex(64 * 1024);
    if let Err(error) = writer.write_all(b"all of it").await {
        panic!("write: {error}");
    }
    drop(writer);
    let output = joined(spawn_capped_stream(reader), "stdout").await;

    assert_eq!(output.bytes, b"all of it");
    assert!(!output.truncated);
}

#[tokio::test]
async fn must_fail_empty_open_pipe_yields_empty_truncated_output_59() {
    let (writer, reader) = tokio::io::duplex(1024);
    let output = joined(spawn_capped_stream(reader), "stderr").await;

    assert!(output.bytes.is_empty());
    assert!(output.truncated);
    drop(writer);
}

#[tokio::test]
async fn must_fail_output_over_the_byte_cap_keeps_the_cap_and_reports_truncation_59() {
    let (mut writer, reader) = tokio::io::duplex(64 * 1024);
    let feeder = tokio::spawn(async move {
        let chunk = [b'x'; 8192];
        let mut sent = 0usize;
        while sent < SHELL_OUTPUT_CAP_BYTES + 4096 {
            if writer.write_all(&chunk).await.is_err() {
                break;
            }
            sent += chunk.len();
        }
    });
    // Generous cap: the writer closes at EOF, so this never waits it out on a healthy host.
    let output = match join_capped_stream_with_cap(
        spawn_capped_stream(reader),
        "stdout",
        Duration::from_secs(30),
    )
    .await
    {
        Ok(output) => output,
        Err(error) => panic!("join stdout: {error:?}"),
    };
    let _ = feeder.await;

    assert_eq!(output.bytes.len(), SHELL_OUTPUT_CAP_BYTES);
    assert!(output.truncated);
}

#[tokio::test]
async fn must_pass_stdout_and_stderr_partials_are_preserved_independently_59() {
    let (mut out_writer, out_reader) = tokio::io::duplex(1024);
    let (mut err_writer, err_reader) = tokio::io::duplex(1024);
    if let Err(error) = out_writer.write_all(b"stdout so far").await {
        panic!("write stdout: {error}");
    }
    if let Err(error) = err_writer.write_all(b"stderr so far").await {
        panic!("write stderr: {error}");
    }
    let out_task = spawn_capped_stream(out_reader);
    let err_task = spawn_capped_stream(err_reader);
    let_reader_consume().await;

    let stdout = joined(out_task, "stdout").await;
    let stderr = joined(err_task, "stderr").await;

    assert_eq!(stdout.bytes, b"stdout so far");
    assert_eq!(stderr.bytes, b"stderr so far");
    assert!(stdout.truncated && stderr.truncated);
    drop((out_writer, err_writer));
}
