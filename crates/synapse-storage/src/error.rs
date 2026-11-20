use std::path::PathBuf;

use synapse_core::error_codes;
use thiserror::Error;

use crate::pressure::DiskPressureLevel;

pub type StorageResult<T> = Result<T, StorageError>;

/// Storage failures with stable Synapse error codes.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("storage open failed for {path:?}: {detail}")]
    OpenFailed { path: PathBuf, detail: String },
    #[error("storage write failed while encoding {type_name}: {source}")]
    EncodeJson {
        type_name: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("storage read failed while decoding {type_name}: {source}")]
    DecodeJson {
        type_name: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("storage write failed in {cf_name}: {detail}")]
    WriteFailed { cf_name: String, detail: String },
    #[error(
        "storage write shed in {cf_name} at {pressure_level:?}: {dropped_rows} rows were not persisted"
    )]
    WriteShed {
        cf_name: String,
        pressure_level: DiskPressureLevel,
        dropped_rows: usize,
    },
    #[error(
        "storage column family {cf_name} reached hard cap of {hard_cap} bytes (estimated {estimated_bytes} bytes)"
    )]
    HardCapReached {
        cf_name: String,
        hard_cap: u64,
        estimated_bytes: u64,
    },
    #[error(
        "storage namespace {namespace} in {cf_name} reached its quota of {quota_bytes} bytes (holds {used_bytes} bytes, write needs {incoming_bytes} more)"
    )]
    NamespaceQuotaReached {
        cf_name: String,
        namespace: String,
        quota_bytes: u64,
        used_bytes: u64,
        incoming_bytes: u64,
    },
    #[error("storage read failed in {cf_name}: {detail}")]
    ReadFailed { cf_name: String, detail: String },
    #[error("storage schema mismatch: expected {expected}, actual {actual}")]
    SchemaMismatch { expected: u32, actual: u32 },
}

impl StorageError {
    /// Returns the stable Synapse error code for this storage failure.
    #[tracing::instrument(skip_all, fields(storage_error = ?self))]
    pub fn code(&self) -> &'static str {
        match self {
            Self::OpenFailed { .. } => error_codes::STORAGE_OPEN_FAILED,
            Self::EncodeJson { .. } | Self::WriteFailed { .. } => error_codes::STORAGE_WRITE_FAILED,
            Self::WriteShed { .. } => error_codes::STORAGE_WRITE_SHED,
            Self::HardCapReached { .. } => error_codes::STORAGE_CF_HARD_CAP_REACHED,
            Self::NamespaceQuotaReached { .. } => error_codes::STORAGE_NAMESPACE_QUOTA_REACHED,
            Self::DecodeJson { .. } | Self::ReadFailed { .. } => error_codes::STORAGE_READ_FAILED,
            Self::SchemaMismatch { .. } => error_codes::STORAGE_SCHEMA_MISMATCH,
        }
    }
}
