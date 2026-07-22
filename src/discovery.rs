use crate::edid::{EdidError, parse_edid};
use crate::models::{ConnectorStatus, HyprlandMonitor, Monitor};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("failed to read sysfs: {0}")]
    Io(#[from] io::Error),
    #[error("failed to parse Hyprland monitor JSON: {0}")]
    HyprlandJson(#[from] serde_json::Error),
    #[error("failed to parse Hyprland version JSON: {0}")]
    HyprlandVersionJson(serde_json::Error),
}

#[derive(Debug, Error)]
pub enum EdidReadError {
    #[error("failed to read EDID from {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("failed to parse EDID: {0}")]
    Parse(#[from] EdidError),
}

pub fn discover_monitors() -> Result<Vec<Monitor>, DiscoveryError> {
    discover_monitors_from(Path::new("/sys/class/drm"))
}

pub fn discover_monitors_from(sysfs_drm_root: &Path) -> Result<Vec<Monitor>, DiscoveryError> {
    let hypr_monitors = hyprland_monitors().unwrap_or_default();
    let hypr_by_name = hypr_monitors
        .into_iter()
        .map(|monitor| (monitor.name.clone(), monitor))
        .collect::<BTreeMap<_, _>>();

    let mut connectors = BTreeMap::new();
    if sysfs_drm_root.exists() {
        for entry in fs::read_dir(sysfs_drm_root)? {
            let entry = entry?;
            let path = entry.path();
            let Some(connector) = connector_name_from_sysfs_path(&path) else {
                continue;
            };

            let status = read_connector_status(&path).unwrap_or(ConnectorStatus::Unknown);
            let edid = read_edid(&path).ok();
            let candidate = Monitor {
                connector: connector.clone(),
                drm_path: Some(path),
                status,
                hyprland: None,
                edid,
            };
            match connectors.entry(connector) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(candidate);
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    if monitor_sort_rank(&candidate) < monitor_sort_rank(entry.get()) {
                        entry.insert(candidate);
                    }
                }
            }
        }
    }

    let all_names = connectors
        .keys()
        .chain(hypr_by_name.keys())
        .cloned()
        .collect::<BTreeSet<_>>();

    let mut monitors = all_names
        .into_iter()
        .map(|name| {
            let mut monitor = connectors.remove(&name).unwrap_or(Monitor {
                connector: name.clone(),
                drm_path: None,
                status: ConnectorStatus::Unknown,
                hyprland: None,
                edid: None,
            });
            monitor.hyprland = hypr_by_name.get(&name).cloned();
            monitor
        })
        .collect::<Vec<_>>();
    monitors.sort_by(|left, right| {
        monitor_sort_rank(left)
            .cmp(&monitor_sort_rank(right))
            .then_with(|| left.connector.cmp(&right.connector))
    });

    Ok(monitors)
}

fn monitor_sort_rank(monitor: &Monitor) -> u8 {
    if monitor
        .hyprland
        .as_ref()
        .is_some_and(|hyprland| hyprland.focused)
    {
        0
    } else {
        match (
            monitor.status,
            monitor.edid.is_some(),
            monitor.hyprland.is_some(),
        ) {
            (ConnectorStatus::Connected, true, _) => 1,
            (ConnectorStatus::Connected, false, _) => 2,
            (_, true, _) => 3,
            (_, _, true) => 4,
            (ConnectorStatus::Unknown, false, false) => 5,
            (ConnectorStatus::Disconnected, false, false) => 6,
        }
    }
}

pub fn read_raw_edid(connector_path: &Path) -> io::Result<Vec<u8>> {
    fs::read(connector_path.join("edid"))
}

pub fn read_edid(connector_path: &Path) -> Result<crate::models::EdidData, EdidReadError> {
    let edid_path = connector_path.join("edid");
    let raw = read_raw_edid(connector_path).map_err(|source| EdidReadError::Io {
        path: edid_path,
        source,
    })?;
    parse_edid(raw).map_err(EdidReadError::Parse)
}

pub fn hyprland_version() -> Result<Option<String>, DiscoveryError> {
    let Ok(output) = Command::new("hyprctl").args(["version", "-j"]).output() else {
        return Ok(None);
    };

    if !output.status.success() || output.stdout.is_empty() {
        return Ok(None);
    }

    let version = serde_json::from_slice::<HyprlandVersionJson>(&output.stdout)
        .map_err(DiscoveryError::HyprlandVersionJson)?;
    Ok((!version.version.is_empty()).then_some(version.version))
}

fn read_connector_status(connector_path: &Path) -> io::Result<ConnectorStatus> {
    fs::read_to_string(connector_path.join("status"))
        .map(|status| ConnectorStatus::from_sysfs(&status))
}

fn connector_name_from_sysfs_path(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    if !name.starts_with("card") {
        return None;
    }

    let status_path = path.join("status");
    if !status_path.exists() {
        return None;
    }

    let (_, connector) = name.split_once('-')?;
    Some(connector.to_string())
}

fn hyprland_monitors() -> Result<Vec<HyprlandMonitor>, DiscoveryError> {
    let output = Command::new("hyprctl").args(["monitors", "-j"]).output();

    let Ok(output) = output else {
        return Ok(Vec::new());
    };

    if !output.status.success() || output.stdout.is_empty() {
        return Ok(Vec::new());
    }

    let raw = serde_json::from_slice::<Vec<HyprlandMonitorJson>>(&output.stdout)?;
    Ok(raw.into_iter().map(HyprlandMonitor::from).collect())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyprlandMonitorJson {
    id: Option<i64>,
    name: String,
    description: Option<String>,
    make: Option<String>,
    model: Option<String>,
    serial: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    refresh_rate: Option<f64>,
    x: Option<i32>,
    y: Option<i32>,
    scale: Option<f64>,
    available_modes: Option<Vec<String>>,
    focused: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct HyprlandVersionJson {
    version: String,
}

impl From<HyprlandMonitorJson> for HyprlandMonitor {
    fn from(value: HyprlandMonitorJson) -> Self {
        Self {
            id: value.id,
            name: value.name,
            description: value.description.unwrap_or_default(),
            make: empty_to_none(value.make),
            model: empty_to_none(value.model),
            serial: empty_to_none(value.serial),
            active_width: value.width,
            active_height: value.height,
            refresh_hz: value.refresh_rate,
            x: value.x,
            y: value.y,
            scale: value.scale,
            available_modes: value.available_modes.unwrap_or_default(),
            focused: value.focused.unwrap_or(false),
        }
    }
}

fn empty_to_none(value: Option<String>) -> Option<String> {
    value.and_then(|value| (!value.is_empty()).then_some(value))
}

#[allow(dead_code)]
fn is_connector_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .and_then(|name| name.split_once('-'))
        .is_some()
}

#[allow(dead_code)]
fn connector_sysfs_path(root: &Path, card: &str, connector: &str) -> PathBuf {
    root.join(format!("{card}-{connector}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(connector: &str, status: ConnectorStatus, focused: bool) -> Monitor {
        Monitor {
            connector: connector.to_string(),
            drm_path: None,
            status,
            hyprland: focused.then(|| HyprlandMonitor {
                id: Some(1),
                name: connector.to_string(),
                description: String::new(),
                make: None,
                model: None,
                serial: None,
                active_width: Some(1920),
                active_height: Some(1080),
                refresh_hz: Some(60.0),
                x: Some(0),
                y: Some(0),
                scale: Some(1.0),
                available_modes: Vec::new(),
                focused: true,
            }),
            edid: None,
        }
    }

    #[test]
    fn focused_and_connected_monitors_sort_before_disconnected_ports() {
        let focused = monitor("HDMI-A-1", ConnectorStatus::Connected, true);
        let connected = monitor("eDP-1", ConnectorStatus::Connected, false);
        let disconnected = monitor("DP-1", ConnectorStatus::Disconnected, false);

        assert!(monitor_sort_rank(&focused) < monitor_sort_rank(&connected));
        assert!(monitor_sort_rank(&connected) < monitor_sort_rank(&disconnected));
    }

    #[test]
    fn connected_duplicate_connector_beats_disconnected_one() {
        let connected = monitor("DP-1", ConnectorStatus::Connected, false);
        let disconnected = monitor("DP-1", ConnectorStatus::Disconnected, false);

        assert!(monitor_sort_rank(&connected) < monitor_sort_rank(&disconnected));
    }
}
