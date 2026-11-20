//! Shared helpers for the `reality` regression tests (#136, #156).

use std::{fs, num::NonZeroUsize, path::Path};

use synapse_core::{ForegroundContext, HudReadings, Rect, SensorStatus};
use synapse_perception::ObservationInput;
use synapse_reflex::DEFAULT_MAX_SUBSCRIPTIONS_NONZERO;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::{m1::M1State, m2::M2ServiceConfig, m3::M3ServiceConfig, m4::M4ServiceConfig};

pub(super) fn service_with_profile_dir(
    path: &Path,
    profile_dir: &Path,
) -> anyhow::Result<SynapseService> {
    SynapseService::try_with_m2_shutdown_reason_and_m3_config(
        CancellationToken::new(),
        "test",
        CancellationToken::new(),
        &M2ServiceConfig::default(),
        M3ServiceConfig::from_cli_parts(
            Some(path.join("db")),
            Some(profile_dir.to_path_buf()),
            false,
            "127.0.0.1:0".to_owned(),
            NonZeroUsize::new(DEFAULT_MAX_SUBSCRIPTIONS_NONZERO.get())
                .ok_or_else(|| anyhow::anyhow!("max subscriptions must be nonzero"))?,
            false,
            true,
            None,
            false,
            None,
        ),
        M4ServiceConfig::default(),
    )
}

pub(super) fn write_profile(path: &Path, id: &str, use_scope: &str) -> anyhow::Result<()> {
    fs::write(
        path,
        format!(
            r#"
id = "{id}"
label = "{id}"
schema_version = 2
use_scope = "{use_scope}"
mouse_velocity_profile_default = "natural"
keyboard_dynamics_default = "natural"

[[matches]]
exe = "{id}.exe"

[detection]
classes_of_interest = ["window"]
confidence_threshold = 0.50
max_detections = 8
"#
        ),
    )?;
    Ok(())
}

pub(super) fn install_synthetic_input(
    service: &SynapseService,
    input: ObservationInput,
) -> anyhow::Result<()> {
    let mut state = service
        .m1_state
        .lock()
        .map_err(|_| anyhow::anyhow!("M1 state lock poisoned"))?;
    *state = M1State {
        synthetic: Some(input),
        ..M1State::default()
    };
    Ok(())
}

pub(super) fn synthetic_input(window_title: &str) -> ObservationInput {
    let mut input = ObservationInput::new(ForegroundContext {
        hwnd: 0x1234,
        pid: 1234,
        process_name: "synthetic.exe".to_owned(),
        process_path: "C:\\Synthetic\\synthetic.exe".to_owned(),
        window_title: window_title.to_owned(),
        window_bounds: Rect {
            x: 1,
            y: 2,
            w: 800,
            h: 600,
        },
        monitor_index: 0,
        dpi_scale: 1.0,
        profile_id: None,
        steam_appid: None,
        is_fullscreen: false,
        is_dwm_composed: true,
    });
    input.hud = HudReadings::default();
    input.a11y_status = SensorStatus::Healthy;
    input.capture_status = SensorStatus::Healthy;
    input.detection_status = SensorStatus::Disabled;
    input.audio_status = SensorStatus::Disabled;
    input
}
