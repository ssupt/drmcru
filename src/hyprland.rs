use crate::hyprland_config;
use serde::Deserialize;
use std::io;
use std::process::Command;
use std::thread;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeModeActual {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: f64,
}

impl RuntimeModeActual {
    pub fn label(&self) -> String {
        format!("{}x{}@{:.2}", self.width, self.height, self.refresh_hz)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModeRequest {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: f64,
}

impl ModeRequest {
    pub fn new(width: u32, height: u32, refresh_hz: f64) -> Self {
        Self {
            width,
            height,
            refresh_hz,
        }
    }

    pub fn label(&self) -> String {
        format!(
            "{}x{}@{}",
            self.width,
            self.height,
            format_float(self.refresh_hz)
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModeSwitchReport {
    pub requested: ModeRequest,
    pub actual: Option<RuntimeModeActual>,
    pub output: String,
    pub monitor_rule: String,
    pub matched: bool,
    pub already_active: bool,
    pub restored_previous_mode: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModeInspection {
    pub requested: ModeRequest,
    pub active: Option<RuntimeModeActual>,
    pub available_mode: Option<String>,
    pub monitor_rule: Option<String>,
}

impl ModeInspection {
    pub fn is_available(&self) -> bool {
        self.available_mode.is_some()
    }

    pub fn active_matches(&self) -> bool {
        self.active.as_ref().is_some_and(|active| {
            mode_matches_request(
                active.width,
                active.height,
                active.refresh_hz,
                &self.requested,
            )
        })
    }
}

#[derive(Debug, Error)]
pub enum HyprlandError {
    #[error("failed to launch hyprctl: {0}")]
    Launch(io::Error),
    #[error("failed to parse hyprctl monitor JSON: {0}")]
    MonitorJson(serde_json::Error),
    #[error("Hyprland does not currently report connector {0}")]
    MonitorNotFound(String),
    #[error("mode {mode} is not exposed by DRM for {connector}")]
    ModeUnavailable { connector: String, mode: String },
    #[error("hyprctl failed (exit {code}): {output}")]
    CommandFailed { code: i32, output: String },
}

pub fn switch_to_available_mode(
    connector: &str,
    requested: &ModeRequest,
) -> Result<ModeSwitchReport, HyprlandError> {
    let before = live_monitor(connector)?
        .ok_or_else(|| HyprlandError::MonitorNotFound(connector.to_string()))?;
    let before_actual = before.actual_mode();
    let Some(mode_label) = before.mode_label_for_request(requested) else {
        return Err(HyprlandError::ModeUnavailable {
            connector: connector.to_string(),
            mode: requested.label(),
        });
    };

    if mode_matches_request(
        before_actual.width,
        before_actual.height,
        before_actual.refresh_hz,
        requested,
    ) {
        return Ok(ModeSwitchReport {
            requested: requested.clone(),
            actual: Some(before_actual),
            output: String::new(),
            monitor_rule: before.monitor_rule(connector, &mode_label),
            matched: true,
            already_active: true,
            restored_previous_mode: false,
        });
    }

    let argument = before.monitor_argument(connector, &mode_label);
    let monitor_rule = before.monitor_rule(connector, &mode_label);
    let output = apply_monitor_argument(&argument)?;

    let actual = match wait_for_requested_mode(connector, requested) {
        Ok(actual) => actual,
        Err(error) => {
            let _ = apply_monitor_argument(&before.restore_argument(connector));
            return Err(error);
        }
    };
    let matched = actual
        .as_ref()
        .map(|actual| {
            mode_matches_request(actual.width, actual.height, actual.refresh_hz, requested)
        })
        .unwrap_or(false);
    let restored_previous_mode = if matched {
        false
    } else {
        apply_monitor_argument(&before.restore_argument(connector)).is_ok()
    };

    Ok(ModeSwitchReport {
        requested: requested.clone(),
        actual,
        output,
        monitor_rule,
        matched,
        already_active: false,
        restored_previous_mode,
    })
}

pub fn inspect_mode(
    connector: &str,
    requested: &ModeRequest,
) -> Result<ModeInspection, HyprlandError> {
    let monitor = live_monitor(connector)?
        .ok_or_else(|| HyprlandError::MonitorNotFound(connector.to_string()))?;
    let available_mode = monitor.mode_label_for_request(requested);
    let monitor_rule = available_mode
        .as_ref()
        .map(|mode_label| monitor.monitor_rule(connector, mode_label));

    Ok(ModeInspection {
        requested: requested.clone(),
        active: Some(monitor.actual_mode()),
        available_mode,
        monitor_rule,
    })
}

fn apply_monitor_argument(argument: &str) -> Result<String, HyprlandError> {
    let output = Command::new("hyprctl")
        .args(["keyword", "monitor", argument])
        .output()
        .map_err(HyprlandError::Launch)?;
    let combined = command_output_text(&output.stdout, &output.stderr);

    if output.status.success() {
        Ok(combined)
    } else {
        apply_monitor_argument_lua(argument).map_err(|fallback| HyprlandError::CommandFailed {
            code: fallback.0,
            output: command_output_text(
                combined.as_bytes(),
                format!("Lua fallback: {}", fallback.1).as_bytes(),
            ),
        })
    }
}

fn apply_monitor_argument_lua(argument: &str) -> Result<String, (i32, String)> {
    let mut parts = argument.splitn(4, ',');
    let (Some(connector), Some(mode), Some(position), Some(scale)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err((-1, "invalid monitor argument".to_string()));
    };
    let expression = format!(
        "hl.monitor({{ output = {}, mode = {}, position = {}, scale = {} }})",
        lua_string(connector),
        lua_string(mode),
        lua_string(position),
        scale
    );
    let output = Command::new("hyprctl")
        .args(["eval", &expression])
        .output()
        .map_err(|error| (-1, error.to_string()))?;
    let combined = command_output_text(&output.stdout, &output.stderr);
    if output.status.success() {
        Ok(combined)
    } else {
        Err((output.status.code().unwrap_or(-1), combined))
    }
}

fn lua_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn command_output_text(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    match (stdout.is_empty(), stderr.is_empty()) {
        (false, false) => format!("{stdout}\n{stderr}"),
        (false, true) => stdout,
        (true, false) => stderr,
        (true, true) => String::new(),
    }
}

fn active_mode_for_connector(connector: &str) -> Result<Option<RuntimeModeActual>, HyprlandError> {
    Ok(live_monitor(connector)?.map(|monitor| monitor.actual_mode()))
}

fn wait_for_requested_mode(
    connector: &str,
    requested: &ModeRequest,
) -> Result<Option<RuntimeModeActual>, HyprlandError> {
    poll_requested_mode(requested, 20, Duration::from_millis(100), || {
        active_mode_for_connector(connector)
    })
}

fn poll_requested_mode<F>(
    requested: &ModeRequest,
    attempts: usize,
    delay: Duration,
    mut read_mode: F,
) -> Result<Option<RuntimeModeActual>, HyprlandError>
where
    F: FnMut() -> Result<Option<RuntimeModeActual>, HyprlandError>,
{
    let attempts = attempts.max(1);
    let mut latest = None;
    let mut successful_read = false;
    let mut last_error = None;

    for attempt in 0..attempts {
        match read_mode() {
            Ok(actual) => {
                successful_read = true;
                latest = actual;
                if latest.as_ref().is_some_and(|actual| {
                    mode_matches_request(actual.width, actual.height, actual.refresh_hz, requested)
                }) {
                    return Ok(latest);
                }
            }
            Err(error) => last_error = Some(error),
        }

        if attempt + 1 < attempts {
            thread::sleep(delay);
        }
    }

    if successful_read {
        Ok(latest)
    } else {
        Err(last_error.unwrap_or_else(|| {
            HyprlandError::MonitorNotFound("mode verification returned no result".to_string())
        }))
    }
}

fn live_monitor(connector: &str) -> Result<Option<HyprlandMonitorJson>, HyprlandError> {
    let output = Command::new("hyprctl")
        .args(["monitors", "-j"])
        .output()
        .map_err(HyprlandError::Launch)?;

    if !output.status.success() || output.stdout.is_empty() {
        return Ok(None);
    }

    let monitors = serde_json::from_slice::<Vec<HyprlandMonitorJson>>(&output.stdout)
        .map_err(HyprlandError::MonitorJson)?;
    Ok(monitors.into_iter().find(|monitor| {
        monitor.name == connector
            && monitor.width.is_some()
            && monitor.height.is_some()
            && monitor.refresh_rate.is_some()
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyprlandMonitorJson {
    name: String,
    width: Option<u32>,
    height: Option<u32>,
    refresh_rate: Option<f64>,
    x: Option<i32>,
    y: Option<i32>,
    scale: Option<f64>,
    available_modes: Option<Vec<String>>,
}

impl HyprlandMonitorJson {
    fn actual_mode(&self) -> RuntimeModeActual {
        RuntimeModeActual {
            width: self.width.unwrap_or_default(),
            height: self.height.unwrap_or_default(),
            refresh_hz: self.refresh_rate.unwrap_or_default(),
        }
    }

    fn position_string(&self) -> String {
        match (self.x, self.y) {
            (Some(x), Some(y)) => format!("{x}x{y}"),
            _ => "auto".to_string(),
        }
    }

    fn scale_string(&self) -> String {
        self.scale
            .map(format_float)
            .unwrap_or_else(|| "1".to_string())
    }

    fn restore_argument(&self, connector: &str) -> String {
        format!(
            "{connector},{}x{}@{},{},{}",
            self.width.unwrap_or_default(),
            self.height.unwrap_or_default(),
            format_float(self.refresh_rate.unwrap_or_default()),
            self.position_string(),
            self.scale_string()
        )
    }

    fn monitor_argument(&self, connector: &str, mode_label: &str) -> String {
        format!(
            "{connector},{},{},{}",
            mode_label,
            self.position_string(),
            self.scale_string()
        )
    }

    fn monitor_rule(&self, connector: &str, mode_label: &str) -> String {
        hyprland_config::format_monitor_rule(
            connector,
            mode_label,
            &self.position_string(),
            &self.scale_string(),
        )
    }

    fn mode_label_for_request(&self, requested: &ModeRequest) -> Option<String> {
        if mode_matches_request(
            self.width.unwrap_or_default(),
            self.height.unwrap_or_default(),
            self.refresh_rate.unwrap_or_default(),
            requested,
        ) {
            return Some(format!(
                "{}x{}@{}",
                self.width.unwrap_or_default(),
                self.height.unwrap_or_default(),
                format_float(self.refresh_rate.unwrap_or_default())
            ));
        }

        self.available_modes
            .as_ref()
            .into_iter()
            .flatten()
            .filter_map(|mode| parse_mode(mode))
            .filter(|mode| mode.matches_request(requested))
            .min_by(|left, right| {
                (left.refresh_hz - requested.refresh_hz)
                    .abs()
                    .total_cmp(&(right.refresh_hz - requested.refresh_hz).abs())
            })
            .map(|mode| mode.label())
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ParsedMode {
    width: u32,
    height: u32,
    refresh_hz: f64,
}

impl ParsedMode {
    fn matches_request(&self, requested: &ModeRequest) -> bool {
        mode_matches_request(self.width, self.height, self.refresh_hz, requested)
    }

    fn label(&self) -> String {
        format!(
            "{}x{}@{}",
            self.width,
            self.height,
            format_float(self.refresh_hz)
        )
    }
}

fn parse_mode(mode: &str) -> Option<ParsedMode> {
    let mut parts = mode.trim().split('@');
    let size = parts.next()?;
    let refresh = parts.next()?;
    if parts.next().is_some() {
        return None;
    }

    let mut dimensions = size.split('x');
    let width = dimensions.next()?.parse::<u32>().ok()?;
    let height = dimensions.next()?.parse::<u32>().ok()?;
    if dimensions.next().is_some() {
        return None;
    }

    let refresh = refresh
        .trim_end_matches(['H', 'h', 'Z', 'z'])
        .parse::<f64>()
        .ok()?;

    Some(ParsedMode {
        width,
        height,
        refresh_hz: refresh,
    })
}

fn mode_matches_request(width: u32, height: u32, refresh_hz: f64, requested: &ModeRequest) -> bool {
    width == requested.width
        && height == requested.height
        && (refresh_hz - requested.refresh_hz).abs() <= 0.5
}

fn format_float(value: f64) -> String {
    let mut text = format!("{value:.3}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_live_monitor() -> HyprlandMonitorJson {
        HyprlandMonitorJson {
            name: "DP-1".to_string(),
            width: Some(1920),
            height: Some(1080),
            refresh_rate: Some(144.0),
            x: Some(2560),
            y: Some(0),
            scale: Some(1.25),
            available_modes: Some(vec![
                "1920x1080@144.00Hz".to_string(),
                "1280x1080@239.76Hz".to_string(),
            ]),
        }
    }

    #[test]
    fn mode_request_label_trims_fractional_zeroes() {
        assert_eq!(ModeRequest::new(1280, 1080, 240.0).label(), "1280x1080@240");
        assert_eq!(
            ModeRequest::new(1280, 1080, 239.76).label(),
            "1280x1080@239.76"
        );
    }

    #[test]
    fn parses_hyprland_available_mode_strings() {
        assert_eq!(
            parse_mode("1280x1080@239.76Hz"),
            Some(ParsedMode {
                width: 1280,
                height: 1080,
                refresh_hz: 239.76,
            })
        );
        assert_eq!(
            parse_mode("1920x1080@60"),
            Some(ParsedMode {
                width: 1920,
                height: 1080,
                refresh_hz: 60.0,
            })
        );
    }

    #[test]
    fn mode_availability_accepts_current_mode() {
        assert!(
            sample_live_monitor()
                .mode_label_for_request(&ModeRequest::new(1920, 1080, 144.0))
                .is_some()
        );
    }

    #[test]
    fn mode_availability_accepts_reported_available_mode() {
        assert!(
            sample_live_monitor()
                .mode_label_for_request(&ModeRequest::new(1280, 1080, 240.0))
                .is_some()
        );
    }

    #[test]
    fn mode_availability_rejects_unreported_mode() {
        assert!(
            sample_live_monitor()
                .mode_label_for_request(&ModeRequest::new(1080, 1080, 240.0))
                .is_none()
        );
    }

    #[test]
    fn restore_argument_uses_previous_active_mode() {
        assert_eq!(
            sample_live_monitor().restore_argument("DP-1"),
            "DP-1,1920x1080@144,2560x0,1.25"
        );
    }

    #[test]
    fn monitor_rule_uses_full_hyprland_syntax() {
        assert_eq!(
            sample_live_monitor().monitor_rule("DP-1", "1280x1080@239.76"),
            hyprland_config::format_monitor_rule("DP-1", "1280x1080@239.76", "2560x0", "1.25")
        );
    }

    #[test]
    fn mode_polling_waits_through_transient_old_and_missing_states() {
        let requested = ModeRequest::new(1280, 1080, 239.76);
        let mut reads = vec![
            Some(RuntimeModeActual {
                width: 1920,
                height: 1080,
                refresh_hz: 144.0,
            }),
            None,
            Some(RuntimeModeActual {
                width: 1280,
                height: 1080,
                refresh_hz: 239.761,
            }),
        ]
        .into_iter();
        let mut calls = 0;

        let actual = poll_requested_mode(&requested, 5, Duration::ZERO, || {
            calls += 1;
            Ok(reads.next().flatten())
        })
        .unwrap();

        assert_eq!(calls, 3);
        assert!(actual.is_some_and(|actual| actual.width == 1280));
    }

    #[test]
    fn mode_polling_returns_latest_mismatch_after_timeout() {
        let requested = ModeRequest::new(1280, 1080, 239.76);
        let actual = poll_requested_mode(&requested, 3, Duration::ZERO, || {
            Ok(Some(RuntimeModeActual {
                width: 1920,
                height: 1080,
                refresh_hz: 144.0,
            }))
        })
        .unwrap();

        assert_eq!(actual.unwrap().width, 1920);
    }

    #[test]
    fn available_mode_label_preserves_exact_hyprland_refresh() {
        assert_eq!(
            sample_live_monitor().mode_label_for_request(&ModeRequest::new(1280, 1080, 240.0)),
            Some("1280x1080@239.76".to_string())
        );
    }

    #[test]
    fn available_mode_label_chooses_closest_refresh() {
        let mut monitor = sample_live_monitor();
        monitor.available_modes = Some(vec![
            "1280x1080@239.60Hz".to_string(),
            "1280x1080@239.94Hz".to_string(),
        ]);

        assert_eq!(
            monitor.mode_label_for_request(&ModeRequest::new(1280, 1080, 240.0)),
            Some("1280x1080@239.94".to_string())
        );
    }

    #[test]
    fn mode_inspection_reports_availability_and_active_match() {
        let requested = ModeRequest::new(1920, 1080, 144.0);
        let monitor = sample_live_monitor();
        let available_mode = monitor.mode_label_for_request(&requested);
        let inspection = ModeInspection {
            requested,
            active: Some(monitor.actual_mode()),
            monitor_rule: available_mode
                .as_ref()
                .map(|mode| monitor.monitor_rule("DP-1", mode)),
            available_mode,
        };

        assert!(inspection.is_available());
        assert!(inspection.active_matches());
        assert_eq!(
            inspection.monitor_rule,
            Some(hyprland_config::format_monitor_rule(
                "DP-1",
                "1920x1080@144",
                "2560x0",
                "1.25"
            ))
        );
    }

    #[test]
    fn actual_mode_match_allows_small_refresh_drift() {
        assert!(mode_matches_request(
            1920,
            1080,
            143.8,
            &ModeRequest::new(1920, 1080, 144.0)
        ));
    }

    #[test]
    fn actual_mode_match_rejects_a_whole_hertz_difference() {
        assert!(!mode_matches_request(
            1920,
            1080,
            59.0,
            &ModeRequest::new(1920, 1080, 60.0)
        ));
    }

    #[test]
    fn actual_mode_mismatch_detects_fallback_size() {
        assert!(!mode_matches_request(
            1024,
            768,
            60.0,
            &ModeRequest::new(1080, 1080, 240.0)
        ));
    }
}
