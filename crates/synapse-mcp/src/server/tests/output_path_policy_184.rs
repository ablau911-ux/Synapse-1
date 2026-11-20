//! #184: tools that write a caller-named file accept a plain local path and
//! refuse UNC, device, alternate-data-stream and executable/script targets.
//!
//! These tests exercise the real shared policy consts the tools use (and the
//! real `capture_gif` path validator), never local copies. Every refusal is
//! asserted by error code AND by the per-rule reason text, so a path refused by
//! the wrong rule cannot mask a missing rule.

use super::{
    BROWSER_DOWNLOAD_PATH_POLICY, BROWSER_PDF_PATH_POLICY, CAPTURE_GIF_PATH_POLICY,
    EXECUTABLE_EXTENSIONS, ErrorData, OutputPathPolicy, SCREENSHOT_PATH_POLICY,
};
use crate::server::capture_gif::capture_gif_output_path;
use synapse_core::error_codes;

/// Default Windows `PATHEXT` entries; every one runs when opened by name.
const DEFAULT_PATHEXT: [&str; 12] = [
    "com", "exe", "bat", "cmd", "vbs", "vbe", "js", "jse", "wsf", "wsh", "msc", "cpl",
];

const WHITESPACE: &str = "must be non-empty without leading or trailing whitespace";
const UNC: &str = "must not be a UNC, device or extended-length path";
const WILDCARD: &str = "must not contain wildcard or reserved characters";
const DRIVE_RELATIVE: &str = "with a drive letter must be absolute";
const NOT_ABSOLUTE: &str = "must be an absolute path";
const STREAM: &str = "must not contain ':'";
const TRAILING_DOT: &str = "components must not end with '.' or a space";
const DEVICE_NAME: &str = "must not use reserved device names";
const EXECUTABLE: &str = "must not name an executable or script file";
const GIF_EXTENSION: &str = "must name a file with a .gif extension";
const PDF_EXTENSION: &str = "must name a file with a .pdf extension";

fn error_code(error: &ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str)
}

/// Asserts the refusal is `TOOL_PARAMS_INVALID` and its message contains at
/// least one of the accepted rule reasons (more than one only where the policy
/// legitimately has two refusing rules for the same input).
fn assert_refused(result: Result<std::path::PathBuf, ErrorData>, path: &str, reasons: &[&str]) {
    let error = match result {
        Ok(accepted) => panic!("{path:?} must be refused but was accepted as {accepted:?}"),
        Err(error) => error,
    };
    assert_eq!(
        error_code(&error),
        Some(error_codes::TOOL_PARAMS_INVALID),
        "{path:?}: {error:?}"
    );
    assert!(
        reasons.iter().any(|reason| error.message.contains(reason)),
        "{path:?} refused by the wrong rule, wanted one of {reasons:?}: {}",
        error.message
    );
}

fn assert_policy_refuses(policy: OutputPathPolicy, path: &str, reasons: &[&str]) {
    assert_refused(policy.validate(path), path, reasons);
}

#[test]
fn must_pass_plain_local_files_are_accepted() {
    for path in [
        r"C:\Users\Adam\Downloads\report.csv",
        r"C:\Users\Adam\Downloads\archive.tar.gz",
        r"D:\out\no_extension",
        "C:/Users/Adam/Downloads/photo.png",
    ] {
        assert!(
            BROWSER_DOWNLOAD_PATH_POLICY.validate(path).is_ok(),
            "{path}"
        );
    }
    assert!(BROWSER_PDF_PATH_POLICY.validate(r"C:\out\page.PDF").is_ok());
    assert!(SCREENSHOT_PATH_POLICY.validate(r"C:\out\shot.png").is_ok());
}

#[test]
fn must_pass_capture_gif_accepts_a_plain_local_gif() {
    for path in [
        r"C:\out\demo.gif",
        r"D:\captures\run 1\Demo.GIF",
        "C:/out/a.gif",
    ] {
        assert!(capture_gif_output_path(path).is_ok(), "{path}");
        assert!(CAPTURE_GIF_PATH_POLICY.validate(path).is_ok(), "{path}");
    }
}

#[test]
fn must_fail_network_device_and_stream_paths_are_refused() {
    let cases: [(&str, &[&str]); 12] = [
        (r"\\attacker\share\report.csv", &[UNC]),
        (r"\\?\C:\out\report.csv", &[UNC]),
        (r"\\.\PhysicalDrive0", &[UNC]),
        (r"C:\out\report.csv:hidden", &[STREAM]),
        (r"C:\out\NUL.csv", &[DEVICE_NAME]),
        (r"C:\out\COM1", &[DEVICE_NAME]),
        (r"C:\out\report.csv.", &[TRAILING_DOT]),
        (r"C:out\report.csv", &[DRIVE_RELATIVE]),
        (r"out\report.csv", &[NOT_ABSOLUTE]),
        (r"C:\out\re*port.csv", &[WILDCARD]),
        (" C:\\out\\report.csv", &[WHITESPACE]),
        ("", &[WHITESPACE]),
    ];
    for (path, reasons) in cases {
        assert_policy_refuses(BROWSER_DOWNLOAD_PATH_POLICY, path, reasons);
        assert_policy_refuses(SCREENSHOT_PATH_POLICY, path, reasons);
    }
}

#[test]
fn must_fail_capture_gif_refuses_unc_stream_device_and_relative_paths() {
    // `cap.gif.` has no extension once the trailing dot is read as part of the
    // name, so either the extension rule or the trailing-dot rule may refuse it.
    let cases: [(&str, &[&str]); 13] = [
        (r"\\attacker\share\cap.gif", &[UNC]),
        ("//attacker/share/cap.gif", &[UNC]),
        (r"\\?\C:\out\cap.gif", &[UNC]),
        (r"\\.\C:\out\cap.gif", &[UNC]),
        (r"C:\out\x.gif:ads", &[STREAM]),
        (r"C:\out\x.txt:hidden.gif", &[STREAM]),
        (r"C:\out\NUL.gif", &[DEVICE_NAME]),
        (r"C:\out\cap.gif.", &[GIF_EXTENSION, TRAILING_DOT]),
        (r"C:out\cap.gif", &[DRIVE_RELATIVE]),
        (r"out\cap.gif", &[NOT_ABSOLUTE]),
        (r"C:\out\cap.png", &[GIF_EXTENSION]),
        (r"C:\out\cap", &[GIF_EXTENSION]),
        ("", &[WHITESPACE]),
    ];
    for (path, reasons) in cases {
        assert_refused(capture_gif_output_path(path), path, reasons);
        assert_policy_refuses(CAPTURE_GIF_PATH_POLICY, path, reasons);
    }
}

#[test]
fn must_fail_executable_and_script_targets_are_refused() {
    let cases: [(&str, &str); 10] = [
        (
            r"C:\Users\Adam\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup\run.bat",
            "bat",
        ),
        (r"C:\out\payload.EXE", "exe"),
        (r"C:\out\script.ps1", "ps1"),
        (r"C:\out\link.lnk", "lnk"),
        (r"C:\out\setup.msi", "msi"),
        (r"C:\out\console.msc", "msc"),
        (r"C:\out\tool.jar", "jar"),
        (r"C:\out\help.chm", "chm"),
        (r"C:\out\app.appref-ms", "appref-ms"),
        (r"C:\out\App.APPREF-MS", "appref-ms"),
    ];
    for (path, extension) in cases {
        let reason = format!("{EXECUTABLE} (.{extension})");
        assert_policy_refuses(BROWSER_DOWNLOAD_PATH_POLICY, path, &[&reason]);
        assert_policy_refuses(SCREENSHOT_PATH_POLICY, path, &[&reason]);
    }
}

#[test]
fn must_fail_every_default_pathext_entry_is_refused() {
    for extension in DEFAULT_PATHEXT {
        assert!(
            EXECUTABLE_EXTENSIONS.contains(&extension),
            ".{extension} missing from EXECUTABLE_EXTENSIONS"
        );
        let path = format!(r"C:\Users\Adam\Downloads\file.{}", extension.to_uppercase());
        let reason = format!("{EXECUTABLE} (.{extension})");
        assert_policy_refuses(BROWSER_DOWNLOAD_PATH_POLICY, &path, &[&reason]);
        assert_policy_refuses(SCREENSHOT_PATH_POLICY, &path, &[&reason]);
    }
}

#[test]
fn must_pass_executable_extensions_are_lowercase_and_unique() {
    for (index, extension) in EXECUTABLE_EXTENSIONS.iter().enumerate() {
        assert_eq!(*extension, extension.to_ascii_lowercase(), "{extension}");
        assert!(!extension.starts_with('.'), "{extension}");
        assert!(
            !EXECUTABLE_EXTENSIONS[index + 1..].contains(extension),
            ".{extension} listed twice"
        );
    }
}

#[test]
fn must_fail_pdf_requires_the_pdf_extension() {
    for path in [r"C:\out\page.html", r"C:\out\page", r"C:\out\.pdf"] {
        assert_policy_refuses(BROWSER_PDF_PATH_POLICY, path, &[PDF_EXTENSION]);
    }
}
