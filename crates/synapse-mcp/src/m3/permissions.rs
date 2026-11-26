use std::{
    collections::BTreeSet,
    fmt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use rmcp::{ErrorData, model::ErrorCode};
use serde_json::json;
use synapse_core::{Action, Backend, ComboInput, ComboStep, error_codes};

pub type RequiredPermissions = BTreeSet<Permission>;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Permission {
    ReadEvents,
    WriteReflex,
    ReadReflex,
    ReadProfile,
    WriteProfileActive,
    WriteReplay,
    ReadAudio,
    ReadStorage,
    WriteStorage,
    InputKeyboard,
    InputMouse,
    InputPad,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionGrants {
    granted: RequiredPermissions,
}

impl PermissionGrants {
    pub fn from_config(raw: Option<&str>, audio_enabled: bool) -> Result<Self> {
        let granted = match raw {
            Some(raw) => parse_grants(raw)?,
            None => default_grants(audio_enabled),
        };
        if granted.contains(&Permission::ReadAudio) && !audio_enabled {
            bail!("READ_AUDIO requires --enable-audio or SYNAPSE_ENABLE_AUDIO=true");
        }
        Ok(Self { granted })
    }

    #[must_use]
    pub fn first_missing(&self, required: &RequiredPermissions) -> Option<Permission> {
        required
            .iter()
            .find(|permission| !self.granted.contains(permission))
            .copied()
    }

    #[must_use]
    pub fn names(&self) -> Vec<&'static str> {
        self.granted
            .iter()
            .map(|permission| permission.as_str())
            .collect()
    }
}

impl Permission {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadEvents => "READ_EVENTS",
            Self::WriteReflex => "WRITE_REFLEX",
            Self::ReadReflex => "READ_REFLEX",
            Self::ReadProfile => "READ_PROFILE",
            Self::WriteProfileActive => "WRITE_PROFILE_ACTIVE",
            Self::WriteReplay => "WRITE_REPLAY",
            Self::ReadAudio => "READ_AUDIO",
            Self::ReadStorage => "READ_STORAGE",
            Self::WriteStorage => "WRITE_STORAGE",
            Self::InputKeyboard => "INPUT_KEYBOARD",
            Self::InputMouse => "INPUT_MOUSE",
            Self::InputPad => "INPUT_PAD",
        }
    }

    fn parse(raw: &str) -> Result<Self> {
        let normalized = raw.trim().replace(['-', ' '], "_").to_ascii_uppercase();
        match normalized.as_str() {
            "READ_EVENTS" => Ok(Self::ReadEvents),
            "WRITE_REFLEX" => Ok(Self::WriteReflex),
            "READ_REFLEX" => Ok(Self::ReadReflex),
            "READ_PROFILE" => Ok(Self::ReadProfile),
            "WRITE_PROFILE_ACTIVE" => Ok(Self::WriteProfileActive),
            "WRITE_REPLAY" => Ok(Self::WriteReplay),
            "READ_AUDIO" => Ok(Self::ReadAudio),
            "READ_STORAGE" => Ok(Self::ReadStorage),
            "WRITE_STORAGE" => Ok(Self::WriteStorage),
            "INPUT_KEYBOARD" | "KEYBOARD" => Ok(Self::InputKeyboard),
            "INPUT_MOUSE" | "MOUSE" => Ok(Self::InputMouse),
            "INPUT_PAD" | "PAD" => Ok(Self::InputPad),
            other => bail!("unknown M3 permission {other:?}"),
        }
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[must_use]
pub fn required<const N: usize>(permissions: [Permission; N]) -> RequiredPermissions {
    permissions.into_iter().collect()
}

pub fn authorization_error(tool: &str, missing: Permission) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        format!("tool {tool} requires permission {missing}"),
        Some(json!({
            "code": error_codes::SAFETY_PERMISSION_DENIED,
            "tool": tool,
            "missing_permission": missing.as_str(),
            // #276: say how to obtain the grant. M3 grants come only from
            // startup config here (the runtime overlay is reality-only).
            "remediation": format!(
                "set SYNAPSE_MCP_ALLOWED_PERMISSIONS (env) or --allowed-permissions (CLI) to a list that includes {missing} plus every grant already in use, and restart the daemon; an explicit list replaces the read-only default grants instead of adding to them"
            ),
        })),
    )
}

pub fn profile_scope_error(profile_id: &str) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "profile {profile_id} has use_scope=\"unknown\"; start with --allow-unknown-profile to activate it"
        ),
        Some(json!({
            "code": error_codes::SAFETY_PROFILE_ACTION_DENIED,
            "profile_id": profile_id,
            "use_scope": "unknown",
        })),
    )
}

pub fn replay_path_error(path: &Path, root: &Path) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "replay path {} must stay under {}",
            path.display(),
            root.display()
        ),
        Some(json!({
            "code": error_codes::SAFETY_PERMISSION_DENIED,
            "permission": Permission::WriteReplay.as_str(),
            "reason": "path_outside_allow_root",
            "path": path.display().to_string(),
            "allow_root": root.display().to_string(),
        })),
    )
}

pub fn add_action_permissions(action: &Action, required: &mut RequiredPermissions) {
    match action {
        Action::KeyPress { backend, .. }
        | Action::KeyDown { backend, .. }
        | Action::KeyUp { backend, .. }
        | Action::KeyChord { backend, .. }
        | Action::TypeText { backend, .. } => {
            let _ = backend;
            required.insert(Permission::InputKeyboard);
        }
        Action::MouseMove { backend, .. }
        | Action::MouseMoveRelative { backend, .. }
        | Action::MouseButton { backend, .. }
        | Action::MouseDrag { backend, .. }
        | Action::MouseStroke { backend, .. }
        | Action::MouseScroll { backend, .. }
        | Action::AimAt { backend, .. } => {
            let _ = backend;
            required.insert(Permission::InputMouse);
        }
        Action::PadButton { .. }
        | Action::PadStick { .. }
        | Action::PadTrigger { .. }
        | Action::PadReport { .. } => {
            required.insert(Permission::InputPad);
        }
        Action::Combo { steps, backend } => {
            let _ = backend;
            add_combo_step_permissions(steps, *backend, required);
        }
        Action::ReleaseAll => {
            required.insert(Permission::InputKeyboard);
            required.insert(Permission::InputMouse);
            required.insert(Permission::InputPad);
        }
    }
}

pub fn normalize_replay_path(
    root: &Path,
    path: Option<&str>,
) -> std::result::Result<PathBuf, ErrorData> {
    let raw_path = path.map(str::trim);
    if raw_path.is_some_and(str::is_empty) {
        return Err(crate::m1::mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "replay path must not be empty when provided",
        ));
    }

    let requested = raw_path.map_or_else(|| default_replay_path(root), PathBuf::from);
    normalize_path_under_root(root, &requested).map_err(|_| replay_path_error(&requested, root))
}

/// Validate before any open. Reject links rather than following reparse points
/// that could resolve to a remote share or escape the artifact root.
pub(crate) fn normalize_path_under_root(
    root: &Path,
    requested: &Path,
) -> std::result::Result<PathBuf, ErrorData> {
    let denied = || {
        crate::m1::mcp_error(
            error_codes::SAFETY_PERMISSION_DENIED,
            format!(
                "path must stay under {} without network/device paths or links",
                root.display()
            ),
        )
    };
    let local_path = |path: &Path| {
        let raw = path.to_string_lossy().replace('\\', "/");
        let (drive, rest) = split_windows_drive_prefix(&raw);
        !raw.starts_with("//")
            && (drive.is_empty() || rest.starts_with('/'))
            && !rest.contains(':')
            && !raw.chars().any(char::is_control)
            && !rest.split('/').any(|part| {
                let name = part
                    .split('.')
                    .next()
                    .unwrap_or("")
                    .trim_end()
                    .to_ascii_uppercase();
                let device_number = name
                    .strip_prefix("COM")
                    .or_else(|| name.strip_prefix("LPT"));
                !matches!(part, "" | "." | "..")
                    && (part.ends_with(['.', ' '])
                        || matches!(
                            name.as_str(),
                            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
                        )
                        || device_number.is_some_and(|number| {
                            matches!(
                                number,
                                "1" | "2"
                                    | "3"
                                    | "4"
                                    | "5"
                                    | "6"
                                    | "7"
                                    | "8"
                                    | "9"
                                    | "¹"
                                    | "²"
                                    | "³"
                            )
                        }))
            })
    };
    if !root.is_absolute() || !local_path(root) || !local_path(requested) {
        return Err(denied());
    }
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    let root = lexical_normalize(root);
    let candidate = lexical_normalize(&candidate);
    let (resolved_root, root_existing) = resolve_path_without_links(&root).map_err(|_| denied())?;
    let (resolved_candidate, candidate_existing) =
        resolve_path_without_links(&candidate).map_err(|_| denied())?;
    if !resolved_path_is_under_root(
        &resolved_candidate,
        &resolved_root,
        root_existing.max(candidate_existing),
    ) {
        return Err(denied());
    }
    Ok(candidate)
}

fn resolved_path_is_under_root(path: &Path, root: &Path, existing_components: usize) -> bool {
    let mut parts = path.components();
    root.components().enumerate().all(|(index, expected)| {
        parts.next().is_some_and(|actual| {
            // Existing names come from filesystem handles. Compare them exactly
            // so Windows directories with case sensitivity stay distinct. Only
            // missing suffix names need the Windows case-insensitive comparison.
            if index < existing_components {
                return actual == expected;
            }
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStrExt;
                use windows::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
                let actual: Vec<u16> = actual.as_os_str().encode_wide().collect();
                let expected: Vec<u16> = expected.as_os_str().encode_wide().collect();
                // SAFETY: both UTF-16 slices live through the comparison.
                unsafe { CompareStringOrdinal(&actual, &expected, true) == CSTR_EQUAL }
            }
            #[cfg(not(windows))]
            {
                actual == expected
            }
        })
    })
}

/// The returned directory handles must remain alive until all file I/O ends.
/// On Windows they deny write/delete sharing, pinning ancestors against rename,
/// reparse mutation and replacement. Existing files are opened without truncation,
/// validated by handle, and only then truncated through that same handle.
pub(crate) fn open_confined_artifact(
    root: &Path,
    requested: &Path,
    write: bool,
) -> std::io::Result<(std::fs::File, Vec<std::fs::File>)> {
    let denied = || {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "artifact handle is outside its root or names a link",
        )
    };
    let path = normalize_path_under_root(root, requested).map_err(|_| denied())?;
    let (mut directories, resolved_root) = pin_artifact_directories(root, write)?;
    let parent = path.parent().ok_or_else(denied)?;
    let (mut parents, resolved_parent) = pin_artifact_directories(parent, write)?;
    directories.append(&mut parents);
    if !resolved_path_is_under_root(
        &resolved_parent,
        &resolved_root,
        resolved_root.components().count(),
    ) {
        return Err(denied());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(write).create(write);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .share_mode(FILE_SHARE_READ.0);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(&path)?;
    reject_artifact_link(&file)?;
    if !file.metadata()?.is_file()
        || !resolved_path_is_under_root(
            &artifact_handle_path(&file, &path)?,
            &resolved_root,
            resolved_root.components().count(),
        )
    {
        return Err(denied());
    }
    if write {
        file.set_len(0)?;
    }
    Ok((file, directories))
}

fn pin_artifact_directories(
    path: &Path,
    create: bool,
) -> std::io::Result<(Vec<std::fs::File>, PathBuf)> {
    let mut guards = Vec::new();
    let mut resolved = PathBuf::new();
    for ancestor in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows::Win32::Storage::FileSystem::{
                FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            };
            options
                .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
                .share_mode(FILE_SHARE_READ.0);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY);
        }
        let directory = match options.open(ancestor) {
            Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::create_dir(ancestor) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
                options.open(ancestor)?
            }
            result => result?,
        };
        reject_artifact_link(&directory)?;
        if !directory.metadata()?.is_dir() {
            return Err(std::io::Error::other(
                "artifact ancestor is not a directory",
            ));
        }
        resolved = artifact_handle_path(&directory, ancestor)?;
        guards.push(directory);
    }
    Ok((guards, resolved))
}

fn reject_artifact_link(file: &std::fs::File) -> std::io::Result<()> {
    let metadata = file.metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::{
            Foundation::HANDLE,
            Storage::FileSystem::{
                BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT,
                GetFileInformationByHandle,
            },
        };
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(std::io::Error::other(
                "artifact handle names a reparse point",
            ));
        }
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the file owns the live handle and info is writable for this call.
        unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
            .map_err(std::io::Error::other)?;
        if metadata.is_file() && info.nNumberOfLinks > 1 {
            return Err(std::io::Error::other(
                "artifact handle names a multiply linked file",
            ));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.is_file() && metadata.nlink() > 1 {
            return Err(std::io::Error::other(
                "artifact handle names a multiply linked file",
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn artifact_handle_path(file: &std::fs::File, _path: &Path) -> std::io::Result<PathBuf> {
    use std::os::windows::{ffi::OsStringExt, io::AsRawHandle};
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW},
    };
    let mut buffer = vec![0_u16; 512];
    loop {
        // SAFETY: file owns the live handle and buffer is a writable UTF-16 slice.
        let length = unsafe {
            GetFinalPathNameByHandleW(
                HANDLE(file.as_raw_handle()),
                &mut buffer,
                FILE_NAME_NORMALIZED,
            )
        } as usize;
        if length == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if length < buffer.len() {
            return Ok(PathBuf::from(std::ffi::OsString::from_wide(
                &buffer[..length],
            )));
        }
        buffer.resize(length + 1, 0);
    }
}

#[cfg(target_os = "linux")]
fn artifact_handle_path(file: &std::fs::File, _path: &Path) -> std::io::Result<PathBuf> {
    use std::os::fd::AsRawFd;
    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

#[cfg(all(not(windows), not(target_os = "linux")))]
fn artifact_handle_path(_file: &std::fs::File, path: &Path) -> std::io::Result<PathBuf> {
    // O_NOFOLLOW protects final components on Unix, but directory descriptors
    // do not prevent concurrent ancestor renames there as Windows sharing does.
    std::fs::canonicalize(path)
}

fn resolve_path_without_links(path: &Path) -> std::io::Result<(PathBuf, usize)> {
    // Pin from the volume down before looking up the next child. Do not let
    // canonicalization follow an ancestor swapped after its metadata check.
    let mut existing = path;
    let mut canonical = None;
    let mut guards = Vec::new();
    for ancestor in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows::Win32::Storage::FileSystem::{
                FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            };
            options
                .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
                .share_mode(FILE_SHARE_READ.0);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        match options.open(ancestor) {
            Ok(file) => {
                reject_artifact_link(&file)?;
                canonical = Some(artifact_handle_path(&file, ancestor)?);
                existing = ancestor;
                guards.push(file);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        }
    }
    let canonical =
        canonical.ok_or_else(|| std::io::Error::other("artifact path has no existing ancestor"))?;
    let suffix = path.strip_prefix(existing).map_err(std::io::Error::other)?;
    let existing_components = canonical.components().count();
    Ok((canonical.join(suffix), existing_components))
}

#[must_use]
pub fn replay_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("synapse")
        .join("replays")
}

fn parse_grants(raw: &str) -> Result<RequiredPermissions> {
    let trimmed = raw.trim();
    if matches!(
        trimmed.to_ascii_uppercase().as_str(),
        "" | "NONE" | "DENY_ALL"
    ) {
        return Ok(RequiredPermissions::new());
    }
    let mut granted = RequiredPermissions::new();
    for token in trimmed
        .split(|ch: char| ch == ',' || ch == ';' || ch.is_whitespace())
        .filter(|token| !token.trim().is_empty())
    {
        granted.insert(Permission::parse(token)?);
    }
    Ok(granted)
}

fn default_grants(audio_enabled: bool) -> RequiredPermissions {
    // Fail closed by default: a stock daemon may read state, but write and
    // synthetic-input capabilities require explicit operator opt-in.
    let mut granted = required([
        Permission::ReadEvents,
        Permission::ReadReflex,
        Permission::ReadProfile,
        Permission::ReadStorage,
    ]);
    if audio_enabled {
        granted.insert(Permission::ReadAudio);
    }
    granted
}

fn add_combo_step_permissions(
    steps: &[ComboStep],
    _backend: Backend,
    required: &mut RequiredPermissions,
) {
    for step in steps {
        match step.input {
            ComboInput::KeyDown { .. } | ComboInput::KeyUp { .. } | ComboInput::KeyPress { .. } => {
                required.insert(Permission::InputKeyboard);
            }
            ComboInput::MouseButton { .. } | ComboInput::MouseMoveRel { .. } => {
                required.insert(Permission::InputMouse);
            }
            ComboInput::PadButton { .. } | ComboInput::PadStick { .. } => {
                required.insert(Permission::InputPad);
            }
        }
    }
}

fn default_replay_path(root: &Path) -> PathBuf {
    root.join(format!("replay-{}.jsonl", synapse_core::new_session_id()))
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let raw = path.to_string_lossy().replace('\\', "/");
    let (prefix, rest) = split_windows_drive_prefix(&raw);
    let rooted = rest.starts_with('/');
    let mut parts = Vec::new();

    for part in rest.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|last| *last != "..") {
                    parts.pop();
                } else if !rooted && prefix.is_empty() {
                    parts.push(part);
                }
            }
            _ => parts.push(part),
        }
    }

    let separator = if prefix.is_empty() { "/" } else { "\\" };
    let mut normalized = String::new();
    if !prefix.is_empty() {
        normalized.push_str(prefix);
        if rooted {
            normalized.push('\\');
        }
    } else if rooted {
        normalized.push('/');
    }
    normalized.push_str(&parts.join(separator));
    PathBuf::from(normalized)
}

fn split_windows_drive_prefix(raw: &str) -> (&str, &str) {
    if raw.len() >= 2 && raw.as_bytes()[1] == b':' && raw.as_bytes()[0].is_ascii_alphabetic() {
        raw.split_at(2)
    } else {
        ("", raw)
    }
}

pub fn configured_grants_from_parts(
    raw: Option<&str>,
    audio_enabled: bool,
) -> Result<PermissionGrants> {
    PermissionGrants::from_config(raw, audio_enabled).context("parse M3 permission grants")
}
