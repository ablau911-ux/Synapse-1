//! Path policy for `browser_network_har` (#184): a plain `.har` file path, by
//! the shared output-path rules.

use super::HAR_TOOL;
use crate::server::ErrorData;
use crate::server::output_path_policy::{ExtensionRule, OutputPathPolicy};
use std::path::PathBuf;

const HAR_PATH_POLICY: OutputPathPolicy = OutputPathPolicy {
    tool: HAR_TOOL,
    extensions: ExtensionRule::OneOf(&["har"]),
    require_absolute: false,
};

pub(super) fn validate_har_path(path: &str) -> Result<(PathBuf, String), ErrorData> {
    let validated = HAR_PATH_POLICY.validate(path)?;
    Ok((validated, path.to_owned()))
}
