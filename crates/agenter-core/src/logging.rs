use std::{
    env,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use tracing_subscriber::{filter::EnvFilter, fmt::writer::MakeWriterExt};

const DEFAULT_FILTER: &str = "agenter=debug,tower_http=debug,sqlx=warn";
const DEFAULT_LOG_DIR: &str = "tmp/agenter-logs";
const PAYLOAD_PREVIEW_LIMIT: usize = 512;

static LOG_GUARD: OnceLock<Option<tracing_appender::non_blocking::WorkerGuard>> = OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogFormat {
    Pretty,
    Json,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoggingConfig {
    pub format: LogFormat,
    pub filter: String,
    pub log_dir: Option<PathBuf>,
    pub payloads_enabled: bool,
}

impl LoggingConfig {
    pub fn from_env() -> Self {
        Self::from_env_with(|name| env::var(name))
    }

    pub fn from_env_with<F>(get_env: F) -> Self
    where
        F: Fn(&str) -> Result<String, env::VarError>,
    {
        let format = match get_env("AGENTER_LOG_FORMAT").ok().as_deref() {
            Some("json") | Some("JSON") => LogFormat::Json,
            _ => LogFormat::Pretty,
        };
        let filter = get_env("RUST_LOG").unwrap_or_else(|_| DEFAULT_FILTER.to_owned());
        let log_dir = match get_env("AGENTER_LOG_DIR").ok() {
            Some(value) if matches!(value.as_str(), "0" | "false" | "FALSE" | "False" | "") => None,
            Some(value) => Some(PathBuf::from(value)),
            None => Some(PathBuf::from(DEFAULT_LOG_DIR)),
        };
        let payloads_enabled = get_env("AGENTER_LOG_PAYLOADS")
            .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "True"));

        Self {
            format,
            filter,
            log_dir,
            payloads_enabled,
        }
    }
}

pub fn init_tracing(service_name: &'static str) {
    let config = LoggingConfig::from_env();
    init_tracing_with_config(service_name, config);
}

pub fn init_tracing_with_config(service_name: &'static str, config: LoggingConfig) {
    let env_filter =
        EnvFilter::try_new(&config.filter).unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    if let Some(dir) = &config.log_dir {
        if std::fs::create_dir_all(dir).is_ok() {
            let file_appender =
                tracing_appender::rolling::never(dir, format!("{service_name}.log"));
            let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
            let _ = LOG_GUARD.set(Some(guard));
            let writer = std::io::stdout.and(file_writer);
            match config.format {
                LogFormat::Json => {
                    let _ = tracing_subscriber::fmt()
                        .with_env_filter(env_filter)
                        .json()
                        .with_current_span(true)
                        .with_writer(writer)
                        .try_init();
                }
                LogFormat::Pretty => {
                    let _ = tracing_subscriber::fmt()
                        .with_env_filter(env_filter)
                        .with_writer(writer)
                        .try_init();
                }
            }
            return;
        }
    }

    match config.format {
        LogFormat::Json => {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .json()
                .with_current_span(true)
                .try_init();
        }
        LogFormat::Pretty => {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .try_init();
        }
    }
}

pub fn payload_logging_enabled() -> bool {
    LoggingConfig::from_env().payloads_enabled
}

pub fn payload_preview(payload: &serde_json::Value, enabled: bool) -> Option<String> {
    if !enabled {
        return None;
    }
    let mut encoded = payload.to_string();
    if encoded.len() > PAYLOAD_PREVIEW_LIMIT {
        encoded.truncate(PAYLOAD_PREVIEW_LIMIT);
        encoded.push_str("...");
    }
    Some(encoded)
}

pub fn path_for_log_label(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logging_config_uses_safe_defaults() {
        let config = LoggingConfig::from_env_with(|_| Err(std::env::VarError::NotPresent));

        assert_eq!(config.format, LogFormat::Pretty);
        assert_eq!(config.filter, "agenter=debug,tower_http=debug,sqlx=warn");
        assert_eq!(
            config.log_dir.as_deref(),
            Some(std::path::Path::new("tmp/agenter-logs"))
        );
        assert!(!config.payloads_enabled);
    }

    #[test]
    fn logging_config_parses_json_format_payload_flag_and_log_dir() {
        let config = LoggingConfig::from_env_with(|key| match key {
            "AGENTER_LOG_FORMAT" => Ok("json".to_owned()),
            "AGENTER_LOG_DIR" => Ok("/tmp/agenter-test-logs".to_owned()),
            "AGENTER_LOG_PAYLOADS" => Ok("1".to_owned()),
            "RUST_LOG" => Ok("agenter_runner=trace".to_owned()),
            _ => Err(std::env::VarError::NotPresent),
        });

        assert_eq!(config.format, LogFormat::Json);
        assert_eq!(config.filter, "agenter_runner=trace");
        assert_eq!(
            config.log_dir.as_deref(),
            Some(std::path::Path::new("/tmp/agenter-test-logs"))
        );
        assert!(config.payloads_enabled);
    }

    #[test]
    fn payload_preview_is_metadata_only_unless_enabled() {
        let payload = serde_json::json!({"prompt": "secret prompt", "method": "turn/start"});

        assert_eq!(payload_preview(&payload, false), None);
        assert_eq!(
            payload_preview(&payload, true),
            Some("{\"method\":\"turn/start\",\"prompt\":\"secret prompt\"}".to_owned())
        );
    }
}
