//! Target-scoped raw-CDP browser emulation helpers (#1173/#1174/#1175/#1176/#1177/#1178).

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use serde::{Deserialize, Serialize};

use crate::{A11yError, A11yResult};

pub const CDP_DEVICE_METRICS_MAX_DIMENSION: u32 = 10_000_000;
pub const CDP_DEVICE_SCALE_FACTOR_MAX: f64 = 1000.0;
pub const CDP_DEVICE_MAX_TOUCH_POINTS: u32 = 16;
pub const CDP_DEVICE_MAX_USER_AGENT_CHARS: usize = 4096;
pub const CDP_GEOLOCATION_MAX_ACCURACY_METERS: f64 = 1_000_000_000.0;
pub const CDP_LOCALE_MAX_CHARS: usize = 128;
pub const CDP_TIMEZONE_MAX_CHARS: usize = 128;
pub const CDP_NETWORK_MAX_LATENCY_MS: f64 = 3_600_000.0;
pub const CDP_NETWORK_MAX_THROUGHPUT_BYTES_PER_SEC: f64 = 10_000_000_000.0;

fn finite_f64_eq(left: f64, right: f64) -> bool {
    left.partial_cmp(&right)
        .is_some_and(std::cmp::Ordering::is_eq)
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CdpViewportOverride {
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f64,
    pub mobile: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CdpViewportReadback {
    pub inner_width: i64,
    pub inner_height: i64,
    pub device_pixel_ratio: f64,
    pub screen_width: i64,
    pub screen_height: i64,
    pub outer_width: i64,
    pub outer_height: i64,
    pub visual_viewport_width: Option<f64>,
    pub visual_viewport_height: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CdpViewportResult {
    pub endpoint: String,
    pub cdp_target_id: String,
    pub operation: String,
    pub requested: Option<CdpViewportOverride>,
    pub page_url: String,
    pub page_title: String,
    pub ready_state: String,
    pub readback: CdpViewportReadback,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CdpDeviceDescriptor {
    pub user_agent: String,
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f64,
    pub is_mobile: bool,
    pub has_touch: bool,
    pub max_touch_points: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CdpDeviceReadback {
    pub viewport: CdpViewportReadback,
    pub user_agent: String,
    pub max_touch_points: i64,
    pub ontouchstart_available: bool,
    pub pointer_coarse: bool,
    pub any_pointer_coarse: bool,
    pub hover_none: bool,
    pub any_hover_none: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CdpDeviceResult {
    pub endpoint: String,
    pub cdp_target_id: String,
    pub operation: String,
    pub descriptor: Option<CdpDeviceDescriptor>,
    pub restored_user_agent: Option<String>,
    pub page_url: String,
    pub page_title: String,
    pub ready_state: String,
    pub readback: CdpDeviceReadback,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CdpGeolocationOverride {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy: f64,
    pub altitude: Option<f64>,
    pub altitude_accuracy: Option<f64>,
    pub heading: Option<f64>,
    pub speed: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CdpGeolocationCoordinatesReadback {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy: f64,
    pub altitude: Option<f64>,
    pub altitude_accuracy: Option<f64>,
    pub heading: Option<f64>,
    pub speed: Option<f64>,
    pub timestamp: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CdpGeolocationErrorReadback {
    pub code: i64,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CdpGeolocationReadback {
    pub permission_state: String,
    pub position: Option<CdpGeolocationCoordinatesReadback>,
    pub error: Option<CdpGeolocationErrorReadback>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CdpGeolocationResult {
    pub endpoint: String,
    pub cdp_target_id: String,
    pub operation: String,
    pub origin: String,
    pub requested: Option<CdpGeolocationOverride>,
    pub permission_setting: String,
    pub page_url: String,
    pub page_title: String,
    pub ready_state: String,
    pub readback: CdpGeolocationReadback,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CdpLocaleTimezoneOverride {
    pub locale: Option<String>,
    pub timezone_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CdpLocaleTimezoneReadback {
    pub locale: String,
    pub calendar: String,
    pub numbering_system: String,
    pub time_zone: String,
    pub sample_number: String,
    pub sample_date: String,
    pub date_string: String,
    pub timezone_offset_minutes: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CdpLocaleTimezoneResult {
    pub endpoint: String,
    pub cdp_target_id: String,
    pub operation: String,
    pub requested: Option<CdpLocaleTimezoneOverride>,
    pub page_url: String,
    pub page_title: String,
    pub ready_state: String,
    pub readback: CdpLocaleTimezoneReadback,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CdpMediaOverride {
    pub media: Option<String>,
    pub color_scheme: Option<String>,
    pub reduced_motion: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CdpMediaReadback {
    pub media_screen: bool,
    pub media_print: bool,
    pub color_scheme_dark: bool,
    pub color_scheme_light: bool,
    pub color_scheme_no_preference: bool,
    pub reduced_motion_reduce: bool,
    pub reduced_motion_no_preference: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CdpMediaResult {
    pub endpoint: String,
    pub cdp_target_id: String,
    pub operation: String,
    pub requested: Option<CdpMediaOverride>,
    pub page_url: String,
    pub page_title: String,
    pub ready_state: String,
    pub readback: CdpMediaReadback,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CdpNetworkConditionsOverride {
    pub offline: bool,
    pub latency_ms: f64,
    pub download_throughput_bytes_per_sec: f64,
    pub upload_throughput_bytes_per_sec: f64,
    pub connection_type: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CdpNetworkConditionsReadback {
    pub online: bool,
    pub connection_type: Option<String>,
    pub effective_type: Option<String>,
    pub downlink_mbps: Option<f64>,
    pub rtt_ms: Option<f64>,
    pub save_data: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CdpNetworkConditionsResult {
    pub endpoint: String,
    pub cdp_target_id: String,
    pub operation: String,
    pub requested: Option<CdpNetworkConditionsOverride>,
    pub page_url: String,
    pub page_title: String,
    pub ready_state: String,
    pub readback: CdpNetworkConditionsReadback,
}

enum DeviceMetricsCommand {
    Set(CdpViewportOverride),
    Reset,
}

enum DeviceDescriptorCommand {
    Set(CdpDeviceDescriptor),
    Reset { original_user_agent: Option<String> },
}

enum GeolocationCommand {
    Set {
        geolocation: CdpGeolocationOverride,
        permission_setting: GeolocationPermissionSetting,
        origin: String,
    },
    Reset {
        origin: String,
    },
}

enum LocaleTimezoneCommand {
    Set(CdpLocaleTimezoneOverride),
    Reset,
}

enum MediaCommand {
    Set(CdpMediaOverride),
    Reset,
}

enum NetworkConditionsCommand {
    Set(CdpNetworkConditionsOverride),
    Reset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GeolocationPermissionSetting {
    Granted,
    Denied,
    Prompt,
}

impl GeolocationPermissionSetting {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::Denied => "denied",
            Self::Prompt => "prompt",
        }
    }
}

fn original_user_agents() -> &'static Mutex<HashMap<String, String>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn device_registry_key(endpoint: &str, target_id: &str) -> String {
    format!("{endpoint}\0{target_id}")
}

/// Applies `Emulation.setDeviceMetricsOverride` to one CDP page target, then
/// reads back page-visible viewport metrics from the same target.
pub async fn cdp_set_viewport_size(
    endpoint: &str,
    target_id: &str,
    width: u32,
    height: u32,
    device_scale_factor: f64,
) -> A11yResult<CdpViewportResult> {
    cdp_set_viewport_size_with_mobile(
        endpoint,
        target_id,
        width,
        height,
        device_scale_factor,
        false,
    )
    .await
}

/// Applies `Emulation.setDeviceMetricsOverride` to one CDP page target with an
/// explicit mobile-emulation flag, then reads back page-visible viewport
/// metrics from the same target.
pub async fn cdp_set_viewport_size_with_mobile(
    endpoint: &str,
    target_id: &str,
    width: u32,
    height: u32,
    device_scale_factor: f64,
    mobile: bool,
) -> A11yResult<CdpViewportResult> {
    validate_viewport_override(width, height, device_scale_factor)?;
    let requested = CdpViewportOverride {
        width,
        height,
        device_scale_factor,
        mobile,
    };
    run_device_metrics_command(
        endpoint,
        target_id,
        DeviceMetricsCommand::Set(requested.clone()),
    )
    .await?;
    let readback = viewport_readback(endpoint, target_id).await?;
    if let Some(detail) = viewport_override_mismatch(&requested, &readback.metrics) {
        let _ = run_device_metrics_command(endpoint, target_id, DeviceMetricsCommand::Reset).await;
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "emulate set viewport did not take effect: {detail}; the override was cleared"
            ),
        });
    }
    Ok(CdpViewportResult {
        endpoint: endpoint.to_owned(),
        cdp_target_id: readback.target_id,
        operation: "set".to_owned(),
        requested: Some(requested),
        page_url: readback.url,
        page_title: readback.title,
        ready_state: readback.ready_state,
        readback: readback.metrics,
    })
}

/// Clears `Emulation.setDeviceMetricsOverride` for one CDP page target, then
/// reads back the real page-visible viewport metrics from that target.
pub async fn cdp_reset_viewport_size(
    endpoint: &str,
    target_id: &str,
) -> A11yResult<CdpViewportResult> {
    run_device_metrics_command(endpoint, target_id, DeviceMetricsCommand::Reset).await?;
    let readback = viewport_readback(endpoint, target_id).await?;
    Ok(CdpViewportResult {
        endpoint: endpoint.to_owned(),
        cdp_target_id: readback.target_id,
        operation: "reset".to_owned(),
        requested: None,
        page_url: readback.url,
        page_title: readback.title,
        ready_state: readback.ready_state,
        readback: readback.metrics,
    })
}

/// Applies a Playwright-style device descriptor to one CDP page target: user
/// agent, viewport/DPR/mobile metrics, and touch capability in one operation.
pub async fn cdp_apply_device_descriptor(
    endpoint: &str,
    target_id: &str,
    descriptor: CdpDeviceDescriptor,
) -> A11yResult<CdpDeviceResult> {
    validate_device_descriptor(&descriptor)?;
    let key = device_registry_key(endpoint, target_id);
    let original_readback = device_readback(endpoint, target_id).await?;
    if let Ok(mut originals) = original_user_agents().lock() {
        originals
            .entry(key)
            .or_insert_with(|| original_readback.metrics.user_agent.clone());
    }
    run_device_descriptor_command(
        endpoint,
        target_id,
        DeviceDescriptorCommand::Set(descriptor.clone()),
    )
    .await?;
    let readback = device_readback(endpoint, target_id).await?;
    if let Some(detail) = device_descriptor_mismatch(&descriptor, &readback.metrics) {
        let original_user_agent = original_user_agents()
            .lock()
            .ok()
            .and_then(|mut originals| originals.remove(&device_registry_key(endpoint, target_id)));
        let _ = run_device_descriptor_command(
            endpoint,
            target_id,
            DeviceDescriptorCommand::Reset {
                original_user_agent,
            },
        )
        .await;
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "emulate set device did not take effect: {detail}; the override was cleared"
            ),
        });
    }
    Ok(CdpDeviceResult {
        endpoint: endpoint.to_owned(),
        cdp_target_id: readback.target_id,
        operation: "set".to_owned(),
        descriptor: Some(descriptor),
        restored_user_agent: None,
        page_url: readback.url,
        page_title: readback.title,
        ready_state: readback.ready_state,
        readback: readback.metrics,
    })
}

/// Clears the active device descriptor for one CDP page target. Device metrics
/// and touch emulation are cleared through CDP. The user agent is restored to
/// the value observed before the first descriptor set in this process.
pub async fn cdp_reset_device_descriptor(
    endpoint: &str,
    target_id: &str,
) -> A11yResult<CdpDeviceResult> {
    let key = device_registry_key(endpoint, target_id);
    let original_user_agent = original_user_agents()
        .lock()
        .ok()
        .and_then(|mut originals| originals.remove(&key));
    run_device_descriptor_command(
        endpoint,
        target_id,
        DeviceDescriptorCommand::Reset {
            original_user_agent: original_user_agent.clone(),
        },
    )
    .await?;
    let readback = device_readback(endpoint, target_id).await?;
    Ok(CdpDeviceResult {
        endpoint: endpoint.to_owned(),
        cdp_target_id: readback.target_id,
        operation: "reset".to_owned(),
        descriptor: None,
        restored_user_agent: original_user_agent,
        page_url: readback.url,
        page_title: readback.title,
        ready_state: readback.ready_state,
        readback: readback.metrics,
    })
}

/// Applies `Emulation.setGeolocationOverride` to one CDP page target and sets
/// the current page origin's geolocation permission to granted or denied.
pub async fn cdp_set_geolocation_override(
    endpoint: &str,
    target_id: &str,
    geolocation: CdpGeolocationOverride,
    grant_permission: bool,
) -> A11yResult<CdpGeolocationResult> {
    validate_geolocation_override(&geolocation)?;
    let origin = geolocation_origin(endpoint, target_id).await?;
    let permission_setting = if grant_permission {
        GeolocationPermissionSetting::Granted
    } else {
        GeolocationPermissionSetting::Denied
    };
    run_geolocation_command(
        endpoint,
        target_id,
        GeolocationCommand::Set {
            geolocation: geolocation.clone(),
            permission_setting,
            origin: origin.clone(),
        },
    )
    .await?;
    let readback = geolocation_readback(endpoint, target_id).await?;
    Ok(CdpGeolocationResult {
        endpoint: endpoint.to_owned(),
        cdp_target_id: readback.target_id,
        operation: "set".to_owned(),
        origin,
        requested: Some(geolocation),
        permission_setting: permission_setting.as_str().to_owned(),
        page_url: readback.url,
        page_title: readback.title,
        ready_state: readback.ready_state,
        readback: readback.metrics,
    })
}

/// Clears the active geolocation override for one CDP page target and restores
/// the current page origin's geolocation permission to the default prompt state.
pub async fn cdp_reset_geolocation_override(
    endpoint: &str,
    target_id: &str,
) -> A11yResult<CdpGeolocationResult> {
    let origin = geolocation_origin(endpoint, target_id).await?;
    run_geolocation_command(
        endpoint,
        target_id,
        GeolocationCommand::Reset {
            origin: origin.clone(),
        },
    )
    .await?;
    let readback = geolocation_readback(endpoint, target_id).await?;
    Ok(CdpGeolocationResult {
        endpoint: endpoint.to_owned(),
        cdp_target_id: readback.target_id,
        operation: "reset".to_owned(),
        origin,
        requested: None,
        permission_setting: GeolocationPermissionSetting::Prompt.as_str().to_owned(),
        page_url: readback.url,
        page_title: readback.title,
        ready_state: readback.ready_state,
        readback: readback.metrics,
    })
}

/// Applies locale and/or timezone overrides to one CDP page target and reads
/// back page-visible Intl/Date state from the same target.
pub async fn cdp_set_locale_timezone_override(
    endpoint: &str,
    target_id: &str,
    requested: CdpLocaleTimezoneOverride,
) -> A11yResult<CdpLocaleTimezoneResult> {
    validate_locale_timezone_override(&requested)?;
    run_locale_timezone_command(
        endpoint,
        target_id,
        LocaleTimezoneCommand::Set(requested.clone()),
    )
    .await?;
    let readback = locale_timezone_readback(endpoint, target_id).await?;
    if let Some(detail) = locale_timezone_override_mismatch(&requested, &readback.metrics) {
        let _ =
            run_locale_timezone_command(endpoint, target_id, LocaleTimezoneCommand::Reset).await;
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "emulate set locale/timezone did not take effect: {detail}; the override was cleared"
            ),
        });
    }
    Ok(CdpLocaleTimezoneResult {
        endpoint: endpoint.to_owned(),
        cdp_target_id: readback.target_id,
        operation: "set".to_owned(),
        requested: Some(requested),
        page_url: readback.url,
        page_title: readback.title,
        ready_state: readback.ready_state,
        readback: readback.metrics,
    })
}

/// Clears locale and timezone overrides for one CDP page target and reads back
/// host-default Intl/Date state from that target.
pub async fn cdp_reset_locale_timezone_override(
    endpoint: &str,
    target_id: &str,
) -> A11yResult<CdpLocaleTimezoneResult> {
    run_locale_timezone_command(endpoint, target_id, LocaleTimezoneCommand::Reset).await?;
    let readback = locale_timezone_readback(endpoint, target_id).await?;
    Ok(CdpLocaleTimezoneResult {
        endpoint: endpoint.to_owned(),
        cdp_target_id: readback.target_id,
        operation: "reset".to_owned(),
        requested: None,
        page_url: readback.url,
        page_title: readback.title,
        ready_state: readback.ready_state,
        readback: readback.metrics,
    })
}

/// Applies media type and/or media feature overrides to one CDP page target and
/// reads back page-visible matchMedia state from the same target.
pub async fn cdp_set_media_override(
    endpoint: &str,
    target_id: &str,
    requested: CdpMediaOverride,
) -> A11yResult<CdpMediaResult> {
    validate_media_override(&requested)?;
    run_media_command(endpoint, target_id, MediaCommand::Set(requested.clone())).await?;
    let readback = media_readback(endpoint, target_id).await?;
    if let Some(detail) = media_override_mismatch(&requested, &readback.metrics) {
        let _ = run_media_command(endpoint, target_id, MediaCommand::Reset).await;
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "emulate set media did not take effect: {detail}; the override was cleared"
            ),
        });
    }
    Ok(CdpMediaResult {
        endpoint: endpoint.to_owned(),
        cdp_target_id: readback.target_id,
        operation: "set".to_owned(),
        requested: Some(requested),
        page_url: readback.url,
        page_title: readback.title,
        ready_state: readback.ready_state,
        readback: readback.metrics,
    })
}

/// Clears media type and media feature overrides for one CDP page target.
pub async fn cdp_reset_media_override(
    endpoint: &str,
    target_id: &str,
) -> A11yResult<CdpMediaResult> {
    run_media_command(endpoint, target_id, MediaCommand::Reset).await?;
    let readback = media_readback(endpoint, target_id).await?;
    Ok(CdpMediaResult {
        endpoint: endpoint.to_owned(),
        cdp_target_id: readback.target_id,
        operation: "reset".to_owned(),
        requested: None,
        page_url: readback.url,
        page_title: readback.title,
        ready_state: readback.ready_state,
        readback: readback.metrics,
    })
}

/// Applies offline/throttling network conditions to one CDP page target, then
/// reads back page-visible navigator network state from the same target.
pub async fn cdp_set_network_conditions(
    endpoint: &str,
    target_id: &str,
    requested: CdpNetworkConditionsOverride,
) -> A11yResult<CdpNetworkConditionsResult> {
    validate_network_conditions_override(&requested)?;
    run_network_conditions_command(
        endpoint,
        target_id,
        NetworkConditionsCommand::Set(requested.clone()),
    )
    .await?;
    let readback = network_conditions_readback(endpoint, target_id).await?;
    Ok(CdpNetworkConditionsResult {
        endpoint: endpoint.to_owned(),
        cdp_target_id: readback.target_id,
        operation: "set".to_owned(),
        requested: Some(requested),
        page_url: readback.url,
        page_title: readback.title,
        ready_state: readback.ready_state,
        readback: readback.metrics,
    })
}

/// Clears network-condition emulation for one CDP page target by restoring
/// Chromium's online/full-speed defaults.
pub async fn cdp_reset_network_conditions(
    endpoint: &str,
    target_id: &str,
) -> A11yResult<CdpNetworkConditionsResult> {
    run_network_conditions_command(endpoint, target_id, NetworkConditionsCommand::Reset).await?;
    let readback = network_conditions_readback(endpoint, target_id).await?;
    Ok(CdpNetworkConditionsResult {
        endpoint: endpoint.to_owned(),
        cdp_target_id: readback.target_id,
        operation: "reset".to_owned(),
        requested: None,
        page_url: readback.url,
        page_title: readback.title,
        ready_state: readback.ready_state,
        readback: readback.metrics,
    })
}

fn validate_viewport_override(width: u32, height: u32, device_scale_factor: f64) -> A11yResult<()> {
    if width == 0 || width > CDP_DEVICE_METRICS_MAX_DIMENSION {
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "viewport width must be 1..={CDP_DEVICE_METRICS_MAX_DIMENSION}, got {width}"
            ),
        });
    }
    if height == 0 || height > CDP_DEVICE_METRICS_MAX_DIMENSION {
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "viewport height must be 1..={CDP_DEVICE_METRICS_MAX_DIMENSION}, got {height}"
            ),
        });
    }
    if !device_scale_factor.is_finite()
        || device_scale_factor <= 0.0
        || device_scale_factor > CDP_DEVICE_SCALE_FACTOR_MAX
    {
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "device_scale_factor must be finite and in 0..={CDP_DEVICE_SCALE_FACTOR_MAX}, got {device_scale_factor}"
            ),
        });
    }
    Ok(())
}

fn validate_device_descriptor(descriptor: &CdpDeviceDescriptor) -> A11yResult<()> {
    validate_user_agent(&descriptor.user_agent)?;
    validate_viewport_override(
        descriptor.width,
        descriptor.height,
        descriptor.device_scale_factor,
    )?;
    if descriptor.has_touch {
        if descriptor.max_touch_points == 0
            || descriptor.max_touch_points > CDP_DEVICE_MAX_TOUCH_POINTS
        {
            return Err(A11yError::CdpAxtreeFailed {
                detail: format!(
                    "max_touch_points must be 1..={CDP_DEVICE_MAX_TOUCH_POINTS} when has_touch=true, got {}",
                    descriptor.max_touch_points
                ),
            });
        }
    } else if descriptor.max_touch_points != 0 {
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "max_touch_points must be 0 when has_touch=false, got {}",
                descriptor.max_touch_points
            ),
        });
    }
    Ok(())
}

fn validate_user_agent(value: &str) -> A11yResult<()> {
    if value.trim() != value || value.is_empty() {
        return Err(A11yError::CdpAxtreeFailed {
            detail: "device descriptor user_agent must be non-empty without surrounding whitespace"
                .to_owned(),
        });
    }
    if value.contains(['\r', '\n', '\0']) {
        return Err(A11yError::CdpAxtreeFailed {
            detail: "device descriptor user_agent must not contain line breaks or NUL".to_owned(),
        });
    }
    if value.chars().count() > CDP_DEVICE_MAX_USER_AGENT_CHARS {
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "device descriptor user_agent must be at most {CDP_DEVICE_MAX_USER_AGENT_CHARS} Unicode scalar values"
            ),
        });
    }
    Ok(())
}

fn validate_geolocation_override(geolocation: &CdpGeolocationOverride) -> A11yResult<()> {
    validate_geolocation_range("latitude", geolocation.latitude, -90.0, 90.0)?;
    validate_geolocation_range("longitude", geolocation.longitude, -180.0, 180.0)?;
    validate_geolocation_range(
        "accuracy",
        geolocation.accuracy,
        0.0,
        CDP_GEOLOCATION_MAX_ACCURACY_METERS,
    )?;
    validate_geolocation_optional_finite("altitude", geolocation.altitude)?;
    validate_geolocation_optional_range(
        "altitude_accuracy",
        geolocation.altitude_accuracy,
        0.0,
        CDP_GEOLOCATION_MAX_ACCURACY_METERS,
    )?;
    validate_geolocation_optional_range("heading", geolocation.heading, 0.0, 360.0)?;
    validate_geolocation_optional_range(
        "speed",
        geolocation.speed,
        0.0,
        CDP_GEOLOCATION_MAX_ACCURACY_METERS,
    )?;
    Ok(())
}

fn validate_geolocation_range(field: &str, value: f64, min: f64, max: f64) -> A11yResult<()> {
    if !value.is_finite() || value < min || value > max {
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!("geolocation {field} must be finite and in {min}..={max}, got {value}"),
        });
    }
    Ok(())
}

fn validate_geolocation_optional_range(
    field: &str,
    value: Option<f64>,
    min: f64,
    max: f64,
) -> A11yResult<()> {
    if let Some(value) = value {
        validate_geolocation_range(field, value, min, max)?;
    }
    Ok(())
}

fn validate_geolocation_optional_finite(field: &str, value: Option<f64>) -> A11yResult<()> {
    if let Some(value) = value
        && !value.is_finite()
    {
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!("geolocation {field} must be finite, got {value}"),
        });
    }
    Ok(())
}

fn validate_locale_timezone_override(requested: &CdpLocaleTimezoneOverride) -> A11yResult<()> {
    if requested.locale.is_none() && requested.timezone_id.is_none() {
        return Err(A11yError::CdpAxtreeFailed {
            detail: "locale/timezone override requires locale and/or timezone_id".to_owned(),
        });
    }
    if let Some(locale) = requested.locale.as_deref() {
        validate_locale_override(locale)?;
    }
    if let Some(timezone_id) = requested.timezone_id.as_deref() {
        validate_timezone_override(timezone_id)?;
    }
    Ok(())
}

fn validate_locale_override(value: &str) -> A11yResult<()> {
    if value.trim() != value || value.is_empty() {
        return Err(A11yError::CdpAxtreeFailed {
            detail: "locale override must be non-empty without surrounding whitespace".to_owned(),
        });
    }
    if value.chars().count() > CDP_LOCALE_MAX_CHARS {
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!("locale override must be at most {CDP_LOCALE_MAX_CHARS} characters"),
        });
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(A11yError::CdpAxtreeFailed {
            detail: "locale override must contain only ASCII letters, digits, '_' or '-'"
                .to_owned(),
        });
    }
    Ok(())
}

fn validate_timezone_override(value: &str) -> A11yResult<()> {
    if value.trim() != value || value.is_empty() {
        return Err(A11yError::CdpAxtreeFailed {
            detail: "timezone_id override must be non-empty without surrounding whitespace"
                .to_owned(),
        });
    }
    if value.chars().count() > CDP_TIMEZONE_MAX_CHARS {
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "timezone_id override must be at most {CDP_TIMEZONE_MAX_CHARS} characters"
            ),
        });
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '_' | '-' | '+'))
    {
        return Err(A11yError::CdpAxtreeFailed {
            detail:
                "timezone_id override must contain only ASCII letters, digits, '/', '_', '-' or '+'"
                    .to_owned(),
        });
    }
    Ok(())
}

fn validate_media_override(requested: &CdpMediaOverride) -> A11yResult<()> {
    if requested.media.is_none()
        && requested.color_scheme.is_none()
        && requested.reduced_motion.is_none()
    {
        return Err(A11yError::CdpAxtreeFailed {
            detail: "media override requires media, color_scheme and/or reduced_motion".to_owned(),
        });
    }
    if let Some(media) = requested.media.as_deref() {
        validate_media_type(media)?;
    }
    if let Some(color_scheme) = requested.color_scheme.as_deref() {
        validate_color_scheme(color_scheme)?;
    }
    if let Some(reduced_motion) = requested.reduced_motion.as_deref() {
        validate_reduced_motion(reduced_motion)?;
    }
    Ok(())
}

fn validate_media_type(value: &str) -> A11yResult<()> {
    if matches!(value, "screen" | "print") {
        Ok(())
    } else {
        Err(A11yError::CdpAxtreeFailed {
            detail: format!("media must be 'screen' or 'print', got {value:?}"),
        })
    }
}

fn validate_color_scheme(value: &str) -> A11yResult<()> {
    if matches!(value, "light" | "dark" | "no-preference") {
        Ok(())
    } else {
        Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "color_scheme must be 'light', 'dark' or 'no-preference', got {value:?}"
            ),
        })
    }
}

fn validate_reduced_motion(value: &str) -> A11yResult<()> {
    if matches!(value, "reduce" | "no-preference") {
        Ok(())
    } else {
        Err(A11yError::CdpAxtreeFailed {
            detail: format!("reduced_motion must be 'reduce' or 'no-preference', got {value:?}"),
        })
    }
}

fn validate_network_conditions_override(
    requested: &CdpNetworkConditionsOverride,
) -> A11yResult<()> {
    validate_network_latency(requested.latency_ms)?;
    validate_network_throughput(
        "download_throughput_bytes_per_sec",
        requested.download_throughput_bytes_per_sec,
    )?;
    validate_network_throughput(
        "upload_throughput_bytes_per_sec",
        requested.upload_throughput_bytes_per_sec,
    )?;
    if let Some(connection_type) = requested.connection_type.as_deref() {
        validate_network_connection_type(connection_type)?;
    }
    if !requested.offline
        && finite_f64_eq(requested.latency_ms, 0.0)
        && finite_f64_eq(requested.download_throughput_bytes_per_sec, -1.0)
        && finite_f64_eq(requested.upload_throughput_bytes_per_sec, -1.0)
        && requested.connection_type.is_none()
    {
        return Err(A11yError::CdpAxtreeFailed {
            detail: "network conditions set requires offline=true, latency, throughput and/or connection_type; use reset to restore defaults".to_owned(),
        });
    }
    Ok(())
}

fn validate_network_latency(value: f64) -> A11yResult<()> {
    if value.is_finite() && (0.0..=CDP_NETWORK_MAX_LATENCY_MS).contains(&value) {
        Ok(())
    } else {
        Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "latency_ms must be finite and in 0..={CDP_NETWORK_MAX_LATENCY_MS}, got {value}"
            ),
        })
    }
}

fn validate_network_throughput(field: &str, value: f64) -> A11yResult<()> {
    if value.is_finite()
        && (finite_f64_eq(value, -1.0)
            || (0.0..=CDP_NETWORK_MAX_THROUGHPUT_BYTES_PER_SEC).contains(&value))
    {
        Ok(())
    } else {
        Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "{field} must be -1 to disable throttling or finite in 0..={CDP_NETWORK_MAX_THROUGHPUT_BYTES_PER_SEC}, got {value}"
            ),
        })
    }
}

fn validate_network_connection_type(value: &str) -> A11yResult<()> {
    if matches!(
        value,
        "none"
            | "cellular2g"
            | "cellular3g"
            | "cellular4g"
            | "bluetooth"
            | "ethernet"
            | "wifi"
            | "wimax"
            | "other"
    ) {
        Ok(())
    } else {
        Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "connection_type must be one of none, cellular2g, cellular3g, cellular4g, bluetooth, ethernet, wifi, wimax or other, got {value:?}"
            ),
        })
    }
}

/// One emulation override domain held on a target's long-lived raw-CDP session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum EmulationDomain {
    Viewport,
    Device,
    Geolocation,
    LocaleTimezone,
    Media,
    Network,
}

/// One long-lived raw-CDP session that keeps a target's emulation overrides
/// alive. Chrome ends every `Emulation.*` / `Network.emulateNetworkConditions`
/// override the instant the CDP client that set it disconnects, so — exactly
/// like [`crate::cdp_action`]'s init-script slot — we keep one session open per
/// target for as long as any emulation domain holds an override on it. The
/// slot's domain set is the one source of truth for which domains are overridden.
struct EmulationSlot {
    /// Distinguishes this slot from a later one opened at the same key, so the
    /// reaper only ever evicts its own entry, never a replacement.
    slot_id: u64,
    page: chromiumoxide::Page,
    /// Kept open so a geolocation permission write (a Browser-level command) and
    /// every override outlive the call; dropping it closes the CDP session.
    browser: chromiumoxide::Browser,
    handler_task: tokio::task::JoinHandle<()>,
    /// A page-scoped event stream kept only as a liveness sentinel: Chrome ends
    /// it when the target is destroyed, so this task finishes and reaps the slot
    /// without waiting for the next set/reset on that target.
    listener_task: tokio::task::JoinHandle<()>,
    /// Which emulation domains currently hold an override on this session.
    domains: Mutex<HashSet<EmulationDomain>>,
}

impl Drop for EmulationSlot {
    fn drop(&mut self) {
        self.handler_task.abort();
        self.listener_task.abort();
    }
}

#[derive(Default)]
struct EmulationSlotRegistry {
    slots: Mutex<HashMap<String, Arc<EmulationSlot>>>,
}

fn emulation_slot_registry() -> &'static EmulationSlotRegistry {
    static REGISTRY: OnceLock<EmulationSlotRegistry> = OnceLock::new();
    REGISTRY.get_or_init(EmulationSlotRegistry::default)
}

fn emulation_slot_key(endpoint: &str, target_id: &str) -> String {
    format!("{endpoint}\n{target_id}")
}

fn next_emulation_slot_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// A slot is dead once either its browser connection (`handler_task`) or its
/// target (`listener_task`, the page-scoped sentinel) has gone.
fn emulation_slot_live(slot: &EmulationSlot) -> bool {
    !slot.handler_task.is_finished() && !slot.listener_task.is_finished()
}

/// Removes a slot whose target was destroyed, identified by `slot_id` so a
/// replacement slot opened at the same key after this one is never evicted. Run
/// by the sentinel task the moment the target's event stream ends, so a closed
/// tab releases its CDP session without waiting for the next call or the drain.
fn reap_dead_emulation_slot(endpoint: &str, target_id: &str, slot_id: u64) {
    let key = emulation_slot_key(endpoint, target_id);
    let Ok(mut slots) = emulation_slot_registry().slots.lock() else {
        return;
    };
    if slots.get(&key).is_some_and(|slot| slot.slot_id == slot_id) {
        slots.remove(&key);
    }
}

/// Returns the live slot for a target, dropping and forgetting one whose session
/// has already ended (target gone or connection lost) so a later reset honestly
/// takes the harmless fresh-connection path instead of reusing a dead session.
fn lookup_live_emulation_slot(endpoint: &str, target_id: &str) -> Option<Arc<EmulationSlot>> {
    let key = emulation_slot_key(endpoint, target_id);
    let mut slots = emulation_slot_registry().slots.lock().ok()?;
    match slots.get(&key) {
        Some(slot) if emulation_slot_live(slot) => Some(Arc::clone(slot)),
        Some(_) => {
            slots.remove(&key);
            None
        }
        None => None,
    }
}

/// Returns the target's existing emulation session or opens a new long-lived
/// one, mirroring the init-script slot: connect, spawn the handler task, attach
/// a target-destroyed sentinel and store the slot (including the lost-race case
/// where a concurrent caller inserted first — the surplus session is dropped and
/// its handler aborted).
async fn get_or_create_emulation_slot(
    endpoint: &str,
    target_id: &str,
) -> A11yResult<Arc<EmulationSlot>> {
    use chromiumoxide::Browser;
    use chromiumoxide::cdp::browser_protocol::page::{
        EnableParams as PageEnableParams, EventFrameNavigated,
    };
    use futures_util::StreamExt as _;

    let target_id = target_id.trim();
    if target_id.is_empty() {
        return Err(A11yError::CdpAttachFailed {
            detail: "CDP target id must not be empty".to_owned(),
        });
    }
    if let Some(slot) = lookup_live_emulation_slot(endpoint, target_id) {
        return Ok(slot);
    }

    let (browser, mut handler) =
        Browser::connect(endpoint)
            .await
            .map_err(|err| A11yError::CdpAttachFailed {
                detail: format!("emulation session connect {endpoint}: {err}"),
            })?;
    let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let page = match crate::cdp_action::get_target_page_with_discovery(&browser, target_id).await {
        Ok(page) => page,
        Err(err) => {
            handler_task.abort();
            return Err(err);
        }
    };

    // Subscribe to one page-scoped event purely as a liveness sentinel, exactly
    // as the init-script slot does. We never read the events; what matters is
    // that Chrome ends this stream when the target is destroyed (navigation just
    // fires frameNavigated, which is ignored, so an override survives a reload).
    // When the stream ends, the task below reaps the slot, so a closed tab does
    // not leak its CDP session. On any setup failure, abort the handler.
    let sentinel = async {
        page.execute(PageEnableParams::default())
            .await
            .map_err(|err| A11yError::CdpAxtreeFailed {
                detail: format!("Page.enable for emulation session sentinel: {err}"),
            })?;
        page.event_listener::<EventFrameNavigated>()
            .await
            .map_err(|err| A11yError::CdpAxtreeFailed {
                detail: format!("subscribe Page.frameNavigated for emulation sentinel: {err}"),
            })
    }
    .await;
    let mut target_gone = match sentinel {
        Ok(stream) => stream,
        Err(err) => {
            handler_task.abort();
            return Err(err);
        }
    };

    let slot_id = next_emulation_slot_id();
    let listener_task = {
        let listener_page = page.clone();
        let listener_endpoint = endpoint.to_owned();
        let listener_target = target_id.to_owned();
        tokio::spawn(async move {
            let _page = listener_page;
            while target_gone.next().await.is_some() {}
            reap_dead_emulation_slot(&listener_endpoint, &listener_target, slot_id);
        })
    };

    let slot = Arc::new(EmulationSlot {
        slot_id,
        page,
        browser,
        handler_task,
        listener_task,
        domains: Mutex::new(HashSet::new()),
    });

    let key = emulation_slot_key(endpoint, target_id);
    let mut slots = emulation_slot_registry().slots.lock().map_err(|_| {
        A11yError::internal("emulation slot registry lock poisoned while inserting slot")
    })?;
    match slots.get(&key) {
        Some(existing) if emulation_slot_live(existing) => Ok(Arc::clone(existing)),
        _ => {
            slots.insert(key, Arc::clone(&slot));
            Ok(slot)
        }
    }
}

/// Drops a target's emulation session once its last override has been reset,
/// re-checking emptiness under the registry lock so a domain set between the
/// reset and this call keeps its session open.
fn close_empty_emulation_slot(endpoint: &str, target_id: &str) {
    let key = emulation_slot_key(endpoint, target_id);
    let Ok(mut slots) = emulation_slot_registry().slots.lock() else {
        return;
    };
    let still_empty = slots
        .get(&key)
        .is_some_and(|slot| slot.domains.lock().is_ok_and(|domains| domains.is_empty()));
    if still_empty {
        slots.remove(&key);
    }
}

/// Where one emulation command runs. A set, and a reset of a target that still
/// holds overrides, run on the target's long-lived override session; a reset of
/// a target with no live session runs on a throwaway connection (the clear is
/// harmless there and answers exactly as before this fix).
enum EmulationContext {
    Slot(Arc<EmulationSlot>),
    Fresh {
        /// Boxed: a `Browser` is far larger than the slot variant's `Arc`.
        browser: Box<chromiumoxide::Browser>,
        page: chromiumoxide::Page,
        handler_task: tokio::task::JoinHandle<()>,
    },
}

impl EmulationContext {
    /// Acquires the session a command runs on: a set always takes (and opens, if
    /// needed) the long-lived slot; a reset reuses a live slot or, when none
    /// exists, a throwaway connection.
    async fn acquire(endpoint: &str, target_id: &str, is_set: bool) -> A11yResult<Self> {
        use chromiumoxide::Browser;
        use futures_util::StreamExt as _;

        if is_set {
            return Ok(Self::Slot(
                get_or_create_emulation_slot(endpoint, target_id).await?,
            ));
        }
        if let Some(slot) = lookup_live_emulation_slot(endpoint, target_id) {
            return Ok(Self::Slot(slot));
        }

        let (browser, mut handler) =
            Browser::connect(endpoint)
                .await
                .map_err(|err| A11yError::CdpAttachFailed {
                    detail: format!("connect {endpoint}: {err}"),
                })?;
        let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });
        let page =
            match crate::cdp_action::get_target_page_with_discovery(&browser, target_id).await {
                Ok(page) => page,
                Err(err) => {
                    handler_task.abort();
                    return Err(err);
                }
            };
        Ok(Self::Fresh {
            browser: Box::new(browser),
            page,
            handler_task,
        })
    }

    fn page(&self) -> &chromiumoxide::Page {
        match self {
            Self::Slot(slot) => &slot.page,
            Self::Fresh { page, .. } => page,
        }
    }

    fn browser(&self) -> &chromiumoxide::Browser {
        match self {
            Self::Slot(slot) => &slot.browser,
            Self::Fresh { browser, .. } => browser.as_ref(),
        }
    }

    /// Finishes a command: a throwaway session's handler task is aborted; a
    /// slot's domain set is updated on success (a set records its domain, a
    /// reset forgets it) and an emptied slot is closed so its session is freed.
    fn finish(
        self,
        endpoint: &str,
        target_id: &str,
        domain: EmulationDomain,
        is_set: bool,
        succeeded: bool,
    ) -> A11yResult<()> {
        match self {
            Self::Slot(slot) => {
                {
                    let mut domains = slot.domains.lock().map_err(|_| {
                        A11yError::internal(
                            "emulation slot domain set poisoned while updating a domain",
                        )
                    })?;
                    if succeeded {
                        if is_set {
                            domains.insert(domain);
                        } else {
                            domains.remove(&domain);
                        }
                    }
                }
                close_empty_emulation_slot(endpoint, target_id);
            }
            Self::Fresh { handler_task, .. } => handler_task.abort(),
        }
        Ok(())
    }
}

/// Count of emulation override sessions torn down at daemon drain.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct CdpEmulationOverrideDrainReadback {
    pub found: usize,
    pub cleared: usize,
    pub failures: Vec<String>,
    pub active_after: usize,
}

/// Tears down every live emulation override session at daemon drain. Dropping a
/// slot aborts its handler task and closes the CDP session, and Chrome clears
/// every emulation override that session installed — that is the physical
/// teardown, so no per-domain clear is reopened here. Mirrors the init-script
/// drain and is called from the browser-mutation drain while it holds the
/// operation lock, so it takes only the registry lock and never re-acquires it.
#[must_use]
pub fn durable_emulation_overrides_disable_and_drain_all() -> CdpEmulationOverrideDrainReadback {
    let slots = match emulation_slot_registry().slots.lock() {
        Ok(mut slots) => std::mem::take(&mut *slots),
        Err(_) => {
            return CdpEmulationOverrideDrainReadback {
                failures: vec!["durable emulation override registry lock is poisoned".to_owned()],
                active_after: usize::MAX,
                ..Default::default()
            };
        }
    };
    let found = slots.len();
    drop(slots);
    let active_after = emulation_slot_registry()
        .slots
        .lock()
        .map_or(usize::MAX, |slots| slots.len());
    CdpEmulationOverrideDrainReadback {
        found,
        cleared: found,
        failures: Vec::new(),
        active_after,
    }
}

// Rule 2: a set whose page readback contradicts the request did not take
// effect, so it is an error rather than a false success. Each helper compares
// only values a page reports back unambiguously for its domain and returns a
// human-readable mismatch, or `None` when the readback agrees or carries
// nothing comparable. On `Some`, the caller clears the override and forgets the
// domain. Geolocation (an async, permission-gated position) and network
// conditions (only weakly observable through `navigator`) carry nothing exact,
// so they have no helper and are never compared.

/// Viewport — the live-reproduced failure. The emulated layout viewport is
/// reported by `innerWidth`/`innerHeight`; the device scale factor is not
/// compared because `devicePixelRatio` can legitimately round.
fn viewport_override_mismatch(
    requested: &CdpViewportOverride,
    readback: &CdpViewportReadback,
) -> Option<String> {
    let want_width = i64::from(requested.width);
    let want_height = i64::from(requested.height);
    if readback.inner_width == want_width && readback.inner_height == want_height {
        return None;
    }
    Some(format!(
        "requested inner size {want_width}x{want_height} but the page read {}x{}",
        readback.inner_width, readback.inner_height
    ))
}

/// Device descriptor — the user agent round-trips exactly through
/// `navigator.userAgent`, and the emulated viewport is compared as above.
fn device_descriptor_mismatch(
    requested: &CdpDeviceDescriptor,
    readback: &CdpDeviceReadback,
) -> Option<String> {
    let mut problems = Vec::new();
    if readback.user_agent != requested.user_agent {
        problems.push(format!(
            "requested user_agent {:?} but the page read {:?}",
            requested.user_agent, readback.user_agent
        ));
    }
    let want_width = i64::from(requested.width);
    let want_height = i64::from(requested.height);
    if readback.viewport.inner_width != want_width || readback.viewport.inner_height != want_height
    {
        problems.push(format!(
            "requested inner size {want_width}x{want_height} but the page read {}x{}",
            readback.viewport.inner_width, readback.viewport.inner_height
        ));
    }
    if problems.is_empty() {
        None
    } else {
        Some(problems.join("; "))
    }
}

/// Media — each requested feature maps to a `matchMedia` boolean the readback
/// carries directly, so every set feature is comparable.
fn media_override_mismatch(
    requested: &CdpMediaOverride,
    readback: &CdpMediaReadback,
) -> Option<String> {
    let mut problems = Vec::new();
    if let Some(scheme) = requested.color_scheme.as_deref() {
        let honored = match scheme {
            "light" => readback.color_scheme_light,
            "dark" => readback.color_scheme_dark,
            "no-preference" => readback.color_scheme_no_preference,
            _ => true,
        };
        if !honored {
            problems.push(format!(
                "color_scheme={scheme} not reflected (light={}, dark={}, no-preference={})",
                readback.color_scheme_light,
                readback.color_scheme_dark,
                readback.color_scheme_no_preference
            ));
        }
    }
    if let Some(motion) = requested.reduced_motion.as_deref() {
        let honored = match motion {
            "reduce" => readback.reduced_motion_reduce,
            "no-preference" => readback.reduced_motion_no_preference,
            _ => true,
        };
        if !honored {
            problems.push(format!(
                "reduced_motion={motion} not reflected (reduce={}, no-preference={})",
                readback.reduced_motion_reduce, readback.reduced_motion_no_preference
            ));
        }
    }
    if let Some(media) = requested.media.as_deref() {
        let honored = match media {
            "screen" => readback.media_screen,
            "print" => readback.media_print,
            _ => true,
        };
        if !honored {
            problems.push(format!(
                "media={media} not reflected (screen={}, print={})",
                readback.media_screen, readback.media_print
            ));
        }
    }
    if problems.is_empty() {
        None
    } else {
        Some(problems.join("; "))
    }
}

/// Locale/timezone — only the timezone is compared: an IANA id round-trips
/// exactly through `Intl.DateTimeFormat().resolvedOptions().timeZone`, whereas a
/// requested BCP-47 locale can be legitimately normalised (case, subtag order)
/// by the engine, so comparing it would reject correct sets.
fn locale_timezone_override_mismatch(
    requested: &CdpLocaleTimezoneOverride,
    readback: &CdpLocaleTimezoneReadback,
) -> Option<String> {
    let timezone_id = requested.timezone_id.as_deref()?;
    if timezone_id.is_empty() || readback.time_zone == timezone_id {
        return None;
    }
    Some(format!(
        "requested timezone_id {timezone_id:?} but the page read time_zone {:?}",
        readback.time_zone
    ))
}

async fn run_device_metrics_command(
    endpoint: &str,
    target_id: &str,
    command: DeviceMetricsCommand,
) -> A11yResult<()> {
    use chromiumoxide::cdp::browser_protocol::emulation::{
        ClearDeviceMetricsOverrideParams, SetDeviceMetricsOverrideParams,
    };

    let _operation_guard = crate::cdp_network::durable_browser_mutation_operation_guard().await;
    let is_set = matches!(command, DeviceMetricsCommand::Set(_));
    let ctx = EmulationContext::acquire(endpoint, target_id, is_set).await?;

    let result = async {
        let page = ctx.page();
        match command {
            DeviceMetricsCommand::Set(override_metrics) => {
                let params = SetDeviceMetricsOverrideParams::builder()
                    .width(i64::from(override_metrics.width))
                    .height(i64::from(override_metrics.height))
                    .device_scale_factor(override_metrics.device_scale_factor)
                    .mobile(override_metrics.mobile)
                    .screen_width(i64::from(override_metrics.width))
                    .screen_height(i64::from(override_metrics.height))
                    .build()
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setDeviceMetricsOverride params: {err}"),
                    })?;
                page.execute(params)
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setDeviceMetricsOverride: {err}"),
                    })?;
            }
            DeviceMetricsCommand::Reset => {
                page.execute(ClearDeviceMetricsOverrideParams::default())
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.clearDeviceMetricsOverride: {err}"),
                    })?;
                let params = SetDeviceMetricsOverrideParams::builder()
                    .width(0)
                    .height(0)
                    .device_scale_factor(0.0)
                    .mobile(false)
                    .screen_width(0)
                    .screen_height(0)
                    .build()
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setDeviceMetricsOverride reset params: {err}"),
                    })?;
                page.execute(params)
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setDeviceMetricsOverride reset: {err}"),
                    })?;
            }
        }
        Ok::<(), A11yError>(())
    }
    .await;

    ctx.finish(
        endpoint,
        target_id,
        EmulationDomain::Viewport,
        is_set,
        result.is_ok(),
    )?;
    result
}

async fn run_device_descriptor_command(
    endpoint: &str,
    target_id: &str,
    command: DeviceDescriptorCommand,
) -> A11yResult<()> {
    use chromiumoxide::cdp::browser_protocol::emulation::{
        ClearDeviceMetricsOverrideParams, SetDeviceMetricsOverrideParams,
        SetEmitTouchEventsForMouseConfiguration, SetEmitTouchEventsForMouseParams,
        SetTouchEmulationEnabledParams, SetUserAgentOverrideParams,
    };

    let _operation_guard = crate::cdp_network::durable_browser_mutation_operation_guard().await;
    let is_set = matches!(command, DeviceDescriptorCommand::Set(_));
    let ctx = EmulationContext::acquire(endpoint, target_id, is_set).await?;

    let result = async {
        let page = ctx.page();
        match command {
            DeviceDescriptorCommand::Set(descriptor) => {
                let user_agent = SetUserAgentOverrideParams::builder()
                    .user_agent(descriptor.user_agent.clone())
                    .build()
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setUserAgentOverride params: {err}"),
                    })?;
                page.execute(user_agent)
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setUserAgentOverride: {err}"),
                    })?;

                let metrics = SetDeviceMetricsOverrideParams::builder()
                    .width(i64::from(descriptor.width))
                    .height(i64::from(descriptor.height))
                    .device_scale_factor(descriptor.device_scale_factor)
                    .mobile(descriptor.is_mobile)
                    .screen_width(i64::from(descriptor.width))
                    .screen_height(i64::from(descriptor.height))
                    .build()
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setDeviceMetricsOverride params: {err}"),
                    })?;
                page.execute(metrics)
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setDeviceMetricsOverride: {err}"),
                    })?;

                let mut touch =
                    SetTouchEmulationEnabledParams::builder().enabled(descriptor.has_touch);
                if descriptor.has_touch {
                    touch = touch.max_touch_points(i64::from(descriptor.max_touch_points));
                }
                page.execute(touch.build().map_err(|err| A11yError::CdpAxtreeFailed {
                    detail: format!("Emulation.setTouchEmulationEnabled params: {err}"),
                })?)
                .await
                .map_err(|err| A11yError::CdpAxtreeFailed {
                    detail: format!("Emulation.setTouchEmulationEnabled: {err}"),
                })?;

                let touch_config = if descriptor.is_mobile {
                    SetEmitTouchEventsForMouseConfiguration::Mobile
                } else {
                    SetEmitTouchEventsForMouseConfiguration::Desktop
                };
                let emit = SetEmitTouchEventsForMouseParams::builder()
                    .enabled(descriptor.has_touch)
                    .configuration(touch_config)
                    .build()
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setEmitTouchEventsForMouse params: {err}"),
                    })?;
                page.execute(emit)
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setEmitTouchEventsForMouse: {err}"),
                    })?;
            }
            DeviceDescriptorCommand::Reset {
                original_user_agent,
            } => {
                page.execute(ClearDeviceMetricsOverrideParams::default())
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.clearDeviceMetricsOverride: {err}"),
                    })?;
                page.execute(SetTouchEmulationEnabledParams::new(false))
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setTouchEmulationEnabled(false): {err}"),
                    })?;
                let emit = SetEmitTouchEventsForMouseParams::builder()
                    .enabled(false)
                    .configuration(SetEmitTouchEventsForMouseConfiguration::Desktop)
                    .build()
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setEmitTouchEventsForMouse params: {err}"),
                    })?;
                page.execute(emit)
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setEmitTouchEventsForMouse(false): {err}"),
                    })?;
                if let Some(original_user_agent) = original_user_agent {
                    let user_agent = SetUserAgentOverrideParams::builder()
                        .user_agent(original_user_agent)
                        .build()
                        .map_err(|err| A11yError::CdpAxtreeFailed {
                            detail: format!("Emulation.setUserAgentOverride restore params: {err}"),
                        })?;
                    page.execute(user_agent)
                        .await
                        .map_err(|err| A11yError::CdpAxtreeFailed {
                            detail: format!("Emulation.setUserAgentOverride restore: {err}"),
                        })?;
                }
            }
        }
        Ok::<(), A11yError>(())
    }
    .await;

    ctx.finish(
        endpoint,
        target_id,
        EmulationDomain::Device,
        is_set,
        result.is_ok(),
    )?;
    result
}

async fn run_geolocation_command(
    endpoint: &str,
    target_id: &str,
    command: GeolocationCommand,
) -> A11yResult<()> {
    use chromiumoxide::cdp::browser_protocol::emulation::{
        ClearGeolocationOverrideParams, SetGeolocationOverrideParams,
    };

    let _operation_guard = crate::cdp_network::durable_browser_mutation_operation_guard().await;
    let is_set = matches!(command, GeolocationCommand::Set { .. });
    let ctx = EmulationContext::acquire(endpoint, target_id, is_set).await?;

    let result = async {
        let page = ctx.page();
        match command {
            GeolocationCommand::Set {
                geolocation,
                permission_setting,
                origin,
            } => {
                let permission = geolocation_permission_params(&origin, permission_setting)?;
                ctx.browser().execute(permission).await.map_err(|err| {
                    A11yError::CdpAxtreeFailed {
                        detail: format!(
                            "Browser.setPermission geolocation={}: {err}",
                            permission_setting.as_str()
                        ),
                    }
                })?;

                let mut params = SetGeolocationOverrideParams::builder()
                    .latitude(geolocation.latitude)
                    .longitude(geolocation.longitude)
                    .accuracy(geolocation.accuracy);
                if let Some(value) = geolocation.altitude {
                    params = params.altitude(value);
                }
                if let Some(value) = geolocation.altitude_accuracy {
                    params = params.altitude_accuracy(value);
                }
                if let Some(value) = geolocation.heading {
                    params = params.heading(value);
                }
                if let Some(value) = geolocation.speed {
                    params = params.speed(value);
                }
                page.execute(params.build())
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setGeolocationOverride: {err}"),
                    })?;
            }
            GeolocationCommand::Reset { origin } => {
                page.execute(ClearGeolocationOverrideParams::default())
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.clearGeolocationOverride: {err}"),
                    })?;
                let permission =
                    geolocation_permission_params(&origin, GeolocationPermissionSetting::Prompt)?;
                ctx.browser().execute(permission).await.map_err(|err| {
                    A11yError::CdpAxtreeFailed {
                        detail: format!("Browser.setPermission geolocation=prompt: {err}"),
                    }
                })?;
            }
        }
        Ok::<(), A11yError>(())
    }
    .await;

    ctx.finish(
        endpoint,
        target_id,
        EmulationDomain::Geolocation,
        is_set,
        result.is_ok(),
    )?;
    result
}

fn geolocation_permission_params(
    origin: &str,
    permission_setting: GeolocationPermissionSetting,
) -> A11yResult<chromiumoxide::cdp::browser_protocol::browser::SetPermissionParams> {
    use chromiumoxide::cdp::browser_protocol::browser::{
        PermissionDescriptor, PermissionSetting, SetPermissionParams,
    };

    let setting = match permission_setting {
        GeolocationPermissionSetting::Granted => PermissionSetting::Granted,
        GeolocationPermissionSetting::Denied => PermissionSetting::Denied,
        GeolocationPermissionSetting::Prompt => PermissionSetting::Prompt,
    };
    SetPermissionParams::builder()
        .permission(PermissionDescriptor::new("geolocation"))
        .setting(setting)
        .origin(origin.to_owned())
        .build()
        .map_err(|err| A11yError::CdpAxtreeFailed {
            detail: format!("Browser.setPermission geolocation params: {err}"),
        })
}

async fn run_locale_timezone_command(
    endpoint: &str,
    target_id: &str,
    command: LocaleTimezoneCommand,
) -> A11yResult<()> {
    use chromiumoxide::cdp::browser_protocol::emulation::{
        SetLocaleOverrideParams, SetTimezoneOverrideParams,
    };

    let _operation_guard = crate::cdp_network::durable_browser_mutation_operation_guard().await;
    let is_set = matches!(command, LocaleTimezoneCommand::Set(_));
    let ctx = EmulationContext::acquire(endpoint, target_id, is_set).await?;

    let result = async {
        let page = ctx.page();
        match command {
            LocaleTimezoneCommand::Set(requested) => {
                if let Some(locale) = requested.locale {
                    page.execute(SetLocaleOverrideParams::builder().locale(locale).build())
                        .await
                        .map_err(|err| A11yError::CdpAxtreeFailed {
                            detail: format!("Emulation.setLocaleOverride: {err}"),
                        })?;
                }
                if let Some(timezone_id) = requested.timezone_id {
                    let params = SetTimezoneOverrideParams::builder()
                        .timezone_id(timezone_id)
                        .build()
                        .map_err(|err| A11yError::CdpAxtreeFailed {
                            detail: format!("Emulation.setTimezoneOverride params: {err}"),
                        })?;
                    page.execute(params)
                        .await
                        .map_err(|err| A11yError::CdpAxtreeFailed {
                            detail: format!("Emulation.setTimezoneOverride: {err}"),
                        })?;
                }
            }
            LocaleTimezoneCommand::Reset => {
                page.execute(SetLocaleOverrideParams::default())
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setLocaleOverride reset: {err}"),
                    })?;
                page.execute(SetTimezoneOverrideParams::new(""))
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setTimezoneOverride reset: {err}"),
                    })?;
            }
        }
        Ok::<(), A11yError>(())
    }
    .await;

    ctx.finish(
        endpoint,
        target_id,
        EmulationDomain::LocaleTimezone,
        is_set,
        result.is_ok(),
    )?;
    result
}

async fn run_media_command(
    endpoint: &str,
    target_id: &str,
    command: MediaCommand,
) -> A11yResult<()> {
    use chromiumoxide::cdp::browser_protocol::emulation::{MediaFeature, SetEmulatedMediaParams};

    let _operation_guard = crate::cdp_network::durable_browser_mutation_operation_guard().await;
    let is_set = matches!(command, MediaCommand::Set(_));
    let ctx = EmulationContext::acquire(endpoint, target_id, is_set).await?;

    let result = async {
        let page = ctx.page();
        let params = match command {
            MediaCommand::Set(requested) => {
                let mut features = Vec::new();
                if let Some(color_scheme) = requested.color_scheme {
                    features.push(MediaFeature::new("prefers-color-scheme", color_scheme));
                }
                if let Some(reduced_motion) = requested.reduced_motion {
                    features.push(MediaFeature::new("prefers-reduced-motion", reduced_motion));
                }
                SetEmulatedMediaParams::builder()
                    .media(requested.media.unwrap_or_default())
                    .features(features)
                    .build()
            }
            MediaCommand::Reset => SetEmulatedMediaParams::builder()
                .media("")
                .features(Vec::<MediaFeature>::new())
                .build(),
        };
        page.execute(params)
            .await
            .map_err(|err| A11yError::CdpAxtreeFailed {
                detail: format!("Emulation.setEmulatedMedia: {err}"),
            })?;
        Ok::<(), A11yError>(())
    }
    .await;

    ctx.finish(
        endpoint,
        target_id,
        EmulationDomain::Media,
        is_set,
        result.is_ok(),
    )?;
    result
}

#[allow(deprecated)]
async fn run_network_conditions_command(
    endpoint: &str,
    target_id: &str,
    command: NetworkConditionsCommand,
) -> A11yResult<()> {
    use chromiumoxide::cdp::browser_protocol::network::{
        ConnectionType, EmulateNetworkConditionsParams,
    };

    let _operation_guard = crate::cdp_network::durable_browser_mutation_operation_guard().await;
    let is_set = matches!(command, NetworkConditionsCommand::Set(_));
    let ctx = EmulationContext::acquire(endpoint, target_id, is_set).await?;

    let result = async {
        let page = ctx.page();
        let params = match command {
            NetworkConditionsCommand::Set(requested) => {
                let mut builder = EmulateNetworkConditionsParams::builder()
                    .offline(requested.offline)
                    .latency(requested.latency_ms)
                    .download_throughput(requested.download_throughput_bytes_per_sec)
                    .upload_throughput(requested.upload_throughput_bytes_per_sec);
                if let Some(connection_type) = requested.connection_type.as_deref() {
                    let parsed = connection_type.parse::<ConnectionType>().map_err(|err| {
                        A11yError::CdpAxtreeFailed {
                            detail: format!("connection_type {connection_type:?}: {err}"),
                        }
                    })?;
                    builder = builder.connection_type(parsed);
                }
                builder.build().map_err(|err| A11yError::CdpAxtreeFailed {
                    detail: format!("Network.emulateNetworkConditions params: {err}"),
                })?
            }
            NetworkConditionsCommand::Reset => EmulateNetworkConditionsParams::builder()
                .offline(false)
                .latency(0.0)
                .download_throughput(-1.0)
                .upload_throughput(-1.0)
                .build()
                .map_err(|err| A11yError::CdpAxtreeFailed {
                    detail: format!("Network.emulateNetworkConditions reset params: {err}"),
                })?,
        };
        page.execute(params)
            .await
            .map_err(|err| A11yError::CdpAxtreeFailed {
                detail: format!("Network.emulateNetworkConditions: {err}"),
            })?;
        Ok::<(), A11yError>(())
    }
    .await;

    ctx.finish(
        endpoint,
        target_id,
        EmulationDomain::Network,
        is_set,
        result.is_ok(),
    )?;
    result
}

struct ViewportReadback {
    target_id: String,
    url: String,
    title: String,
    ready_state: String,
    metrics: CdpViewportReadback,
}

struct DeviceReadback {
    target_id: String,
    url: String,
    title: String,
    ready_state: String,
    metrics: CdpDeviceReadback,
}

struct GeolocationReadback {
    target_id: String,
    url: String,
    title: String,
    ready_state: String,
    metrics: CdpGeolocationReadback,
}

struct LocaleTimezoneReadback {
    target_id: String,
    url: String,
    title: String,
    ready_state: String,
    metrics: CdpLocaleTimezoneReadback,
}

struct MediaReadback {
    target_id: String,
    url: String,
    title: String,
    ready_state: String,
    metrics: CdpMediaReadback,
}

struct NetworkConditionsReadback {
    target_id: String,
    url: String,
    title: String,
    ready_state: String,
    metrics: CdpNetworkConditionsReadback,
}

async fn viewport_readback(endpoint: &str, target_id: &str) -> A11yResult<ViewportReadback> {
    let evaluated = crate::cdp_action::cdp_evaluate_expression(
        endpoint,
        target_id,
        VIEWPORT_READBACK_JS,
        false,
        true,
    )
    .await?;
    let metrics =
        serde_json::from_value::<CdpViewportReadback>(evaluated.value).map_err(|error| {
            A11yError::CdpAxtreeFailed {
                detail: format!("viewport metrics readback decode: {error}"),
            }
        })?;
    Ok(ViewportReadback {
        target_id: evaluated.target_id,
        url: evaluated.url,
        title: evaluated.title,
        ready_state: evaluated.ready_state,
        metrics,
    })
}

async fn device_readback(endpoint: &str, target_id: &str) -> A11yResult<DeviceReadback> {
    let evaluated = crate::cdp_action::cdp_evaluate_expression(
        endpoint,
        target_id,
        DEVICE_READBACK_JS,
        false,
        true,
    )
    .await?;
    let metrics =
        serde_json::from_value::<CdpDeviceReadback>(evaluated.value).map_err(|error| {
            A11yError::CdpAxtreeFailed {
                detail: format!("device descriptor readback decode: {error}"),
            }
        })?;
    Ok(DeviceReadback {
        target_id: evaluated.target_id,
        url: evaluated.url,
        title: evaluated.title,
        ready_state: evaluated.ready_state,
        metrics,
    })
}

async fn geolocation_origin(endpoint: &str, target_id: &str) -> A11yResult<String> {
    let evaluated = crate::cdp_action::cdp_evaluate_expression(
        endpoint,
        target_id,
        GEOLOCATION_ORIGIN_JS,
        false,
        true,
    )
    .await?;
    let origin = serde_json::from_value::<String>(evaluated.value).map_err(|error| {
        A11yError::CdpAxtreeFailed {
            detail: format!("geolocation origin readback decode: {error}"),
        }
    })?;
    if origin.is_empty() {
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "geolocation permission requires a page with a non-opaque origin, current URL: {}",
                evaluated.url
            ),
        });
    }
    Ok(origin)
}

async fn geolocation_readback(endpoint: &str, target_id: &str) -> A11yResult<GeolocationReadback> {
    let evaluated = crate::cdp_action::cdp_evaluate_expression(
        endpoint,
        target_id,
        GEOLOCATION_READBACK_JS,
        true,
        true,
    )
    .await?;
    let metrics =
        serde_json::from_value::<CdpGeolocationReadback>(evaluated.value).map_err(|error| {
            A11yError::CdpAxtreeFailed {
                detail: format!("geolocation readback decode: {error}"),
            }
        })?;
    Ok(GeolocationReadback {
        target_id: evaluated.target_id,
        url: evaluated.url,
        title: evaluated.title,
        ready_state: evaluated.ready_state,
        metrics,
    })
}

async fn locale_timezone_readback(
    endpoint: &str,
    target_id: &str,
) -> A11yResult<LocaleTimezoneReadback> {
    let evaluated = crate::cdp_action::cdp_evaluate_expression(
        endpoint,
        target_id,
        LOCALE_TIMEZONE_READBACK_JS,
        false,
        true,
    )
    .await?;
    let metrics =
        serde_json::from_value::<CdpLocaleTimezoneReadback>(evaluated.value).map_err(|error| {
            A11yError::CdpAxtreeFailed {
                detail: format!("locale/timezone readback decode: {error}"),
            }
        })?;
    Ok(LocaleTimezoneReadback {
        target_id: evaluated.target_id,
        url: evaluated.url,
        title: evaluated.title,
        ready_state: evaluated.ready_state,
        metrics,
    })
}

async fn media_readback(endpoint: &str, target_id: &str) -> A11yResult<MediaReadback> {
    let evaluated = crate::cdp_action::cdp_evaluate_expression(
        endpoint,
        target_id,
        MEDIA_READBACK_JS,
        false,
        true,
    )
    .await?;
    let metrics = serde_json::from_value::<CdpMediaReadback>(evaluated.value).map_err(|error| {
        A11yError::CdpAxtreeFailed {
            detail: format!("media readback decode: {error}"),
        }
    })?;
    Ok(MediaReadback {
        target_id: evaluated.target_id,
        url: evaluated.url,
        title: evaluated.title,
        ready_state: evaluated.ready_state,
        metrics,
    })
}

async fn network_conditions_readback(
    endpoint: &str,
    target_id: &str,
) -> A11yResult<NetworkConditionsReadback> {
    let evaluated = crate::cdp_action::cdp_evaluate_expression(
        endpoint,
        target_id,
        NETWORK_CONDITIONS_READBACK_JS,
        false,
        true,
    )
    .await?;
    let metrics = serde_json::from_value::<CdpNetworkConditionsReadback>(evaluated.value).map_err(
        |error| A11yError::CdpAxtreeFailed {
            detail: format!("network conditions readback decode: {error}"),
        },
    )?;
    Ok(NetworkConditionsReadback {
        target_id: evaluated.target_id,
        url: evaluated.url,
        title: evaluated.title,
        ready_state: evaluated.ready_state,
        metrics,
    })
}

const VIEWPORT_READBACK_JS: &str = r"(() => {
  const viewport = globalThis.visualViewport || null;
  return {
    inner_width: Math.round(globalThis.innerWidth || 0),
    inner_height: Math.round(globalThis.innerHeight || 0),
    device_pixel_ratio: Number(globalThis.devicePixelRatio || 0),
    screen_width: Math.round(globalThis.screen ? globalThis.screen.width || 0 : 0),
    screen_height: Math.round(globalThis.screen ? globalThis.screen.height || 0 : 0),
    outer_width: Math.round(globalThis.outerWidth || 0),
    outer_height: Math.round(globalThis.outerHeight || 0),
    visual_viewport_width: viewport ? Number(viewport.width) : null,
    visual_viewport_height: viewport ? Number(viewport.height) : null
  };
})()";

const DEVICE_READBACK_JS: &str = r#"(() => {
  const viewport = globalThis.visualViewport || null;
  const media = query => {
    try { return Boolean(globalThis.matchMedia && globalThis.matchMedia(query).matches); }
    catch (_error) { return false; }
  };
  return {
    viewport: {
      inner_width: Math.round(globalThis.innerWidth || 0),
      inner_height: Math.round(globalThis.innerHeight || 0),
      device_pixel_ratio: Number(globalThis.devicePixelRatio || 0),
      screen_width: Math.round(globalThis.screen ? globalThis.screen.width || 0 : 0),
      screen_height: Math.round(globalThis.screen ? globalThis.screen.height || 0 : 0),
      outer_width: Math.round(globalThis.outerWidth || 0),
      outer_height: Math.round(globalThis.outerHeight || 0),
      visual_viewport_width: viewport ? Number(viewport.width) : null,
      visual_viewport_height: viewport ? Number(viewport.height) : null
    },
    user_agent: String(globalThis.navigator ? globalThis.navigator.userAgent || "" : ""),
    max_touch_points: Number(globalThis.navigator ? globalThis.navigator.maxTouchPoints || 0 : 0),
    ontouchstart_available: Boolean("ontouchstart" in globalThis),
    pointer_coarse: media("(pointer: coarse)"),
    any_pointer_coarse: media("(any-pointer: coarse)"),
    hover_none: media("(hover: none)"),
    any_hover_none: media("(any-hover: none)")
  };
})()"#;

const GEOLOCATION_ORIGIN_JS: &str = r#"(() => {
  const location = globalThis.location || null;
  const origin = location ? String(location.origin || "") : "";
  return origin && origin !== "null" ? origin : "";
})()"#;

const GEOLOCATION_READBACK_JS: &str = r#"(async () => {
  const permissionState = await (async () => {
    try {
      if (!globalThis.navigator || !navigator.permissions || !navigator.permissions.query) {
        return "unsupported";
      }
      const status = await navigator.permissions.query({ name: "geolocation" });
      return String(status.state || "unknown");
    } catch (error) {
      const name = error && error.name ? error.name : error;
      return `error:${String(name)}`;
    }
  })();

  const result = await new Promise(resolve => {
    if (!globalThis.navigator || !navigator.geolocation) {
      resolve({
        position: null,
        error: { code: -1, message: "navigator.geolocation unavailable" }
      });
      return;
    }
    let settled = false;
    const finish = value => {
      if (!settled) {
        settled = true;
        resolve(value);
      }
    };
    const timer = globalThis.setTimeout(() => finish({
      position: null,
      error: { code: 3, message: "timeout waiting for geolocation callback" }
    }), 1500);
    navigator.geolocation.getCurrentPosition(
      position => {
        globalThis.clearTimeout(timer);
        const coords = position.coords || {};
        finish({
          position: {
            latitude: Number(coords.latitude),
            longitude: Number(coords.longitude),
            accuracy: Number(coords.accuracy),
            altitude: coords.altitude == null ? null : Number(coords.altitude),
            altitude_accuracy: coords.altitudeAccuracy == null ? null : Number(coords.altitudeAccuracy),
            heading: coords.heading == null || Number.isNaN(Number(coords.heading)) ? null : Number(coords.heading),
            speed: coords.speed == null || Number.isNaN(Number(coords.speed)) ? null : Number(coords.speed),
            timestamp: Number(position.timestamp || 0)
          },
          error: null
        });
      },
      error => {
        globalThis.clearTimeout(timer);
        finish({
          position: null,
          error: {
            code: Number(error && error.code || 0),
            message: String(error && error.message || "")
          }
        });
      },
      { enableHighAccuracy: false, maximumAge: 0, timeout: 1000 }
    );
  });

  return {
    permission_state: permissionState,
    position: result.position,
    error: result.error
  };
})()"#;

const LOCALE_TIMEZONE_READBACK_JS: &str = r#"(() => {
  const sampleDate = new Date(Date.UTC(2020, 0, 2, 3, 4, 5));
  const options = Intl.DateTimeFormat().resolvedOptions();
  const dateFormatter = new Intl.DateTimeFormat(undefined, {
    dateStyle: "full",
    timeStyle: "long"
  });
  return {
    locale: String(options.locale || ""),
    calendar: String(options.calendar || ""),
    numbering_system: String(options.numberingSystem || ""),
    time_zone: String(options.timeZone || ""),
    sample_number: new Intl.NumberFormat().format(1234567.89),
    sample_date: dateFormatter.format(sampleDate),
    date_string: sampleDate.toString(),
    timezone_offset_minutes: Number(sampleDate.getTimezoneOffset())
  };
})()"#;

const MEDIA_READBACK_JS: &str = r#"(() => {
  const media = query => {
    try { return Boolean(globalThis.matchMedia && globalThis.matchMedia(query).matches); }
    catch (_error) { return false; }
  };
  return {
    media_screen: media("screen"),
    media_print: media("print"),
    color_scheme_dark: media("(prefers-color-scheme: dark)"),
    color_scheme_light: media("(prefers-color-scheme: light)"),
    color_scheme_no_preference: media("(prefers-color-scheme: no-preference)"),
    reduced_motion_reduce: media("(prefers-reduced-motion: reduce)"),
    reduced_motion_no_preference: media("(prefers-reduced-motion: no-preference)")
  };
})()"#;

const NETWORK_CONDITIONS_READBACK_JS: &str = r#"(() => {
  const nav = globalThis.navigator || null;
  const connection = nav && (nav.connection || nav.mozConnection || nav.webkitConnection) || null;
  const finiteNumber = value => {
    return typeof value === "number" && Number.isFinite(value) ? value : null;
  };
  return {
    online: Boolean(nav ? nav.onLine : false),
    connection_type: connection && typeof connection.type === "string" ? connection.type : null,
    effective_type: connection && typeof connection.effectiveType === "string" ? connection.effectiveType : null,
    downlink_mbps: connection ? finiteNumber(connection.downlink) : null,
    rtt_ms: connection ? finiteNumber(connection.rtt) : null,
    save_data: connection && typeof connection.saveData === "boolean" ? connection.saveData : null
  };
})()"#;
