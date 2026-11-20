//! Path policy for tools that write a file the caller names (#184).
//!
//! These tools are reachable from page-influenced agent prompts, so the path
//! must not let a prompt-injected write land on a script, device, network share
//! or alternate data stream. Synapse stays open to local use: no auth is added,
//! the path is only constrained to a plain local file path.

use crate::m1::mcp_error;
use crate::server::ErrorData;
use std::path::PathBuf;
use synapse_core::error_codes;

/// Longest accepted path, in Unicode scalar values.
pub(crate) const MAX_OUTPUT_PATH_CHARS: usize = 4096;
/// Windows device names that resolve to devices regardless of extension.
const RESERVED_DEVICE_NAMES: [&str; 6] = ["con", "prn", "aux", "nul", "conin$", "conout$"];
/// Windows device name prefixes followed by a single digit 1-9 (incl. superscripts).
const RESERVED_NUMBERED_DEVICES: [&str; 2] = ["com", "lpt"];
const FORBIDDEN_PATH_CHARS: [char; 6] = ['<', '>', '"', '|', '?', '*'];
/// Extensions Windows executes or interprets on open; a written file must never
/// get one (platform fact, like the device names above). Covers every default
/// `PATHEXT` entry (.COM .EXE .BAT .CMD .VBS .VBE .JS .JSE .WSF .WSH .MSC .CPL,
/// listed first) plus the other shell-executed types: binaries and libraries,
/// Java and help files, installers and packaged apps, `ClickOnce`, shortcuts,
/// `PowerShell`, scripts and scriptlets, registry and setup files.
pub(crate) const EXECUTABLE_EXTENSIONS: [&str; 52] = [
    "com",
    "exe",
    "bat",
    "cmd",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "msc",
    "cpl",
    "scr",
    "pif",
    "dll",
    "ocx",
    "sys",
    "jar",
    "hta",
    "chm",
    "gadget",
    "msi",
    "msp",
    "mst",
    "appx",
    "appxbundle",
    "msix",
    "msixbundle",
    "application",
    "appref-ms",
    "xbap",
    "diagcab",
    "lnk",
    "url",
    "scf",
    "settingcontent-ms",
    "ps1",
    "psm1",
    "psd1",
    "ps1xml",
    "ps2",
    "ps2xml",
    "psc1",
    "psc2",
    "vb",
    "vbscript",
    "ws",
    "wsc",
    "sct",
    "msh",
    "reg",
    "inf",
];

/// `capture_screenshot` (and the screenshot facade) image writes.
pub(crate) const SCREENSHOT_PATH_POLICY: OutputPathPolicy = OutputPathPolicy {
    tool: "capture_screenshot",
    extensions: ExtensionRule::NoneOf(&EXECUTABLE_EXTENSIONS),
    require_absolute: true,
};
/// `browser_pdf` writes.
pub(crate) const BROWSER_PDF_PATH_POLICY: OutputPathPolicy = OutputPathPolicy {
    tool: "browser_pdf",
    extensions: ExtensionRule::OneOf(&["pdf"]),
    require_absolute: true,
};
/// `browser_downloads` save targets.
pub(crate) const BROWSER_DOWNLOAD_PATH_POLICY: OutputPathPolicy = OutputPathPolicy {
    tool: "browser_downloads",
    extensions: ExtensionRule::NoneOf(&EXECUTABLE_EXTENSIONS),
    require_absolute: true,
};
/// `capture_gif` (and `screenshot operation=gif`) animated GIF writes.
pub(crate) const CAPTURE_GIF_PATH_POLICY: OutputPathPolicy = OutputPathPolicy {
    tool: "capture_gif",
    extensions: ExtensionRule::OneOf(&["gif"]),
    require_absolute: true,
};

/// Which file extensions the written file may have.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ExtensionRule {
    /// Exactly one of these (lowercase, without the dot).
    OneOf(&'static [&'static str]),
    /// Any extension (or none) except these.
    NoneOf(&'static [&'static str]),
}

/// The constraints one tool puts on its output path.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OutputPathPolicy {
    pub(crate) tool: &'static str,
    pub(crate) extensions: ExtensionRule,
    /// Require a drive-absolute `X:\...` path.
    pub(crate) require_absolute: bool,
}

impl OutputPathPolicy {
    fn invalid(self, message: &str) -> ErrorData {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{} path {message}", self.tool),
        )
    }

    /// Validates `path` and returns it unchanged as a `PathBuf`.
    pub(crate) fn validate(self, path: &str) -> Result<PathBuf, ErrorData> {
        if path.is_empty() || path.trim() != path {
            return Err(self.invalid("must be non-empty without leading or trailing whitespace"));
        }
        if path.chars().any(char::is_control) {
            return Err(self.invalid("must not contain control characters"));
        }
        if path.chars().count() > MAX_OUTPUT_PATH_CHARS {
            return Err(self.invalid(&format!(
                "must be at most {MAX_OUTPUT_PATH_CHARS} Unicode scalar values"
            )));
        }
        let mut chars = path.chars();
        if chars.next().is_some_and(is_separator) && chars.next().is_some_and(is_separator) {
            return Err(self.invalid("must not be a UNC, device or extended-length path"));
        }
        if path.contains(FORBIDDEN_PATH_CHARS) {
            return Err(self.invalid("must not contain wildcard or reserved characters"));
        }
        let rest = self.strip_drive_prefix(path)?;
        if rest.contains(':') {
            return Err(
                self.invalid("must not contain ':' (alternate data streams are not allowed)")
            );
        }
        self.validate_components(rest)?;
        Ok(PathBuf::from(path))
    }

    /// Strips a leading `X:` + separator drive prefix; any other `:` use is left
    /// in place for the caller to reject, and a drive-relative `X:name` is refused.
    fn strip_drive_prefix(self, path: &str) -> Result<&str, ErrorData> {
        let mut chars = path.chars();
        let (first, second) = (chars.next(), chars.next());
        if first.is_some_and(|c| c.is_ascii_alphabetic()) && second == Some(':') {
            return match chars.next() {
                Some(c) if is_separator(c) => Ok(&path[3..]),
                _ => {
                    Err(self
                        .invalid("with a drive letter must be absolute (e.g. C:\\dir\\file.ext)"))
                }
            };
        }
        if self.require_absolute {
            return Err(self.invalid("must be an absolute path (e.g. C:\\dir\\file.ext)"));
        }
        Ok(path)
    }

    fn validate_components(self, rest: &str) -> Result<(), ErrorData> {
        let file_name = rest.rsplit(is_separator).next().unwrap_or_default();
        self.validate_extension(file_name)?;
        for component in rest.split(is_separator) {
            if component.is_empty() || component == "." || component == ".." {
                continue;
            }
            if component.ends_with('.') || component.ends_with(' ') {
                return Err(self.invalid("components must not end with '.' or a space"));
            }
            if is_reserved_device_name(component) {
                return Err(
                    self.invalid("must not use reserved device names (CON, NUL, COM1, ...)")
                );
            }
        }
        Ok(())
    }

    fn validate_extension(self, file_name: &str) -> Result<(), ErrorData> {
        let extension = file_name
            .rsplit_once('.')
            .map(|(stem, extension)| (stem, extension.to_ascii_lowercase()))
            .filter(|(stem, extension)| !stem.is_empty() && !extension.is_empty())
            .map(|(_, extension)| extension);
        match self.extensions {
            ExtensionRule::OneOf(allowed) => {
                if extension
                    .as_deref()
                    .is_some_and(|extension| allowed.contains(&extension))
                {
                    Ok(())
                } else {
                    Err(self.invalid(&format!(
                        "must name a file with a .{} extension",
                        allowed.join(" / .")
                    )))
                }
            }
            ExtensionRule::NoneOf(refused) => match extension {
                Some(extension) if refused.contains(&extension.as_str()) => Err(self.invalid(
                    &format!("must not name an executable or script file (.{extension})"),
                )),
                _ => Ok(()),
            },
        }
    }
}

const fn is_separator(c: char) -> bool {
    c == '\\' || c == '/'
}

/// Windows resolves `NAME`, `NAME.ext` and `NAME .ext` to the device.
fn is_reserved_device_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_lowercase();
    if RESERVED_DEVICE_NAMES.contains(&stem.as_str()) {
        return true;
    }
    RESERVED_NUMBERED_DEVICES.iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|digit| {
            let mut digits = digit.chars();
            matches!(
                (digits.next(), digits.next()),
                (Some('1'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'), None)
            )
        })
    })
}

#[cfg(test)]
#[path = "tests/output_path_policy_184.rs"]
mod output_path_policy_184_tests;
