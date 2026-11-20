use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};

pub(crate) struct OwnedClaimWindow {
    process: Child,
    hwnd: i64,
}

impl OwnedClaimWindow {
    pub(crate) const fn hwnd(&self) -> i64 {
        self.hwnd
    }
}

impl Drop for OwnedClaimWindow {
    fn drop(&mut self) {
        let _result = self.process.kill();
        let _result = self.process.wait();
    }
}

fn read_claim_hwnd(reader: impl BufRead) -> anyhow::Result<i64> {
    for line in reader.lines() {
        let line = line?;
        // PowerShell can emit startup/compilation output before our ready frame.
        let Some(value) = line
            .trim()
            .trim_start_matches('\u{feff}')
            .strip_prefix("SYNAPSE_CLAIM_HWND=")
        else {
            continue;
        };
        let hwnd = value.parse()?;
        anyhow::ensure!(hwnd != 0, "window helper returned zero HWND");
        return Ok(hwnd);
    }
    anyhow::bail!("window helper exited before its HWND ready frame")
}

const PROGRAM: &str = r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing -TypeDefinition @'
using System.Windows.Forms;
public sealed class ClaimForm : Form {
    protected override bool ShowWithoutActivation { get { return true; } }
    protected override CreateParams CreateParams { get {
        var p = base.CreateParams; p.ExStyle |= 0x08000000; return p;
    } }
}
'@
$form = New-Object ClaimForm
$form.ShowInTaskbar = $false
$form.StartPosition = 'Manual'
$form.Location = New-Object System.Drawing.Point(-32000, -32000)
$form.add_Shown({ [Console]::WriteLine('SYNAPSE_CLAIM_HWND=' + $form.Handle.ToInt64()); [Console]::Out.Flush() })
[System.Windows.Forms.Application]::Run($form)
"#;

/// An isolated off-screen WinForms window, never activated or shown in taskbar.
pub(crate) fn owned_claim_window() -> anyhow::Result<OwnedClaimWindow> {
    let process = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-STA", "-Command", PROGRAM])
        .creation_flags(0x0800_0000)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut window = OwnedClaimWindow { process, hwnd: 0 };
    let stdout = window
        .process
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("window stdout missing"))?;
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        let result = read_claim_hwnd(BufReader::new(stdout));
        let _sent = send.send(result);
    });
    let result = receive.recv_timeout(std::time::Duration::from_secs(10));
    if result.is_err() {
        let _killed = window.process.kill();
    }
    let _joined = reader.join();
    window.hwnd = result??;
    Ok(window)
}
