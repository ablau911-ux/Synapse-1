use std::{env, path::PathBuf, process::Command};

fn main() {
    let manifest_dir = match env::var_os("CARGO_MANIFEST_DIR") {
        Some(value) => PathBuf::from(value),
        None => return,
    };
    let git_dir = manifest_dir.join("..").join("..").join(".git");
    println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
    println!("cargo:rerun-if-changed={}", git_dir.join("index").display());

    if env::var("PROFILE").as_deref() != Ok("release") {
        return;
    }

    let Some(mut build_id) = git_output(&manifest_dir, ["rev-parse", "--short=12", "HEAD"]) else {
        return;
    };
    if !git_status_clean(&manifest_dir) {
        build_id.push_str("-dirty");
    }
    println!("cargo:rustc-env=VERGEN_GIT_SHA={build_id}");
}

fn git_output<const N: usize>(cwd: &PathBuf, args: [&str; N]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let value = text.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn git_status_clean(cwd: &PathBuf) -> bool {
    Command::new("git")
        .args(["diff", "--quiet", "HEAD", "--"])
        .current_dir(cwd)
        .status()
        .is_ok_and(|status| status.success())
}
