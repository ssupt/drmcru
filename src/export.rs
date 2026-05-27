use crate::edid::{DtdLocation, EdidError, insert_detailed_timing};
use crate::models::{ExportPlan, Monitor, TimingDescriptor};
use crate::workspace::EdidWorkspace;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub fn build_export_plan_with_file(
    connector: impl Into<String>,
    edid_file_name: impl Into<String>,
    timing: TimingDescriptor,
) -> ExportPlan {
    ExportPlan {
        connector: connector.into(),
        edid_file_name: edid_file_name.into(),
        timing,
        position: "auto".to_string(),
        scale: "1".to_string(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportResult {
    pub path: PathBuf,
    pub instructions_path: PathBuf,
    pub plan: ExportPlan,
    pub insert_location: Option<DtdLocation>,
}

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("selected monitor has no readable EDID")]
    MissingEdid,
    #[error("failed to patch EDID: {0}")]
    Edid(#[from] EdidError),
    #[error("failed to write {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
}

pub fn export_patched_edid(
    monitor: &Monitor,
    timing: &TimingDescriptor,
    output_dir: &Path,
) -> Result<ExportResult, ExportError> {
    let edid = monitor.edid.as_ref().ok_or(ExportError::MissingEdid)?;
    let (patched, insert_location) = insert_detailed_timing(&edid.raw, timing)?;
    let file_name = custom_edid_file_name(&monitor.connector);
    let path = output_dir.join(&file_name);
    let instructions_path = output_dir.join(instructions_file_name(&monitor.connector));
    let plan = build_export_plan_for_monitor(monitor, file_name, timing.clone());

    fs::write(&path, patched).map_err(|source| ExportError::Write {
        path: path.clone(),
        source,
    })?;
    write_instructions(&instructions_path, &path, &plan)?;

    Ok(ExportResult {
        path,
        instructions_path,
        plan,
        insert_location: Some(insert_location),
    })
}

pub fn export_workspace_edid(
    monitor: &Monitor,
    workspace: &EdidWorkspace,
    timing: &TimingDescriptor,
    output_dir: &Path,
) -> Result<ExportResult, ExportError> {
    let connector = &monitor.connector;
    let file_name = custom_edid_file_name(connector);
    let path = output_dir.join(&file_name);
    let instructions_path = output_dir.join(instructions_file_name(connector));
    let plan = build_export_plan_for_monitor(monitor, file_name, timing.clone());

    fs::write(&path, workspace.export_bytes()).map_err(|source| ExportError::Write {
        path: path.clone(),
        source,
    })?;
    write_instructions(&instructions_path, &path, &plan)?;

    Ok(ExportResult {
        path,
        instructions_path,
        plan,
        insert_location: None,
    })
}

pub fn export_instructions(edid_path: &Path, plan: &ExportPlan) -> String {
    let firmware_target = format!("/lib/firmware/edid/{}", plan.edid_file_name);
    [
        "drmcru export instructions".to_string(),
        String::new(),
        format!("Generated EDID: {}", edid_path.display()),
        format!("Install target: {firmware_target}"),
        String::new(),
        "1. Install the EDID override file:".to_string(),
        format!(
            "   sudo install -D -m 0644 {} {firmware_target}",
            edid_path.display()
        ),
        String::new(),
        "2. Add this kernel parameter to your bootloader:".to_string(),
        format!("   {}", plan.drm_kernel_parameter()),
        String::new(),
        "3. Add or update this Hyprland monitor rule:".to_string(),
        format!("   {}", plan.hyprland_monitor_rule()),
        String::new(),
        "4. Rebuild your bootloader/initramfs as required by your distribution, then reboot."
            .to_string(),
    ]
    .join("\n")
}

pub fn custom_edid_file_name(connector: &str) -> String {
    format!("drmcru_custom_{}.bin", file_safe_connector(connector))
}

fn build_export_plan_for_monitor(
    monitor: &Monitor,
    edid_file_name: impl Into<String>,
    timing: TimingDescriptor,
) -> ExportPlan {
    let mut plan = build_export_plan_with_file(&monitor.connector, edid_file_name, timing);
    plan.position = monitor_position(monitor);
    plan.scale = monitor_scale(monitor);
    plan
}

fn monitor_position(monitor: &Monitor) -> String {
    monitor
        .hyprland
        .as_ref()
        .and_then(|hyprland| hyprland.x.zip(hyprland.y))
        .map(|(x, y)| format!("{x}x{y}"))
        .unwrap_or_else(|| "auto".to_string())
}

fn monitor_scale(monitor: &Monitor) -> String {
    monitor
        .hyprland
        .as_ref()
        .and_then(|hyprland| hyprland.scale)
        .map(format_hyprland_float)
        .unwrap_or_else(|| "1".to_string())
}

fn format_hyprland_float(value: f64) -> String {
    let mut text = format!("{value:.3}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

fn file_safe_connector(connector: &str) -> String {
    connector
        .chars()
        .map(|value| {
            if value.is_ascii_alphanumeric() || value == '-' || value == '_' {
                value
            } else {
                '_'
            }
        })
        .collect()
}

fn instructions_file_name(connector: &str) -> String {
    format!(
        "drmcru_custom_{}_instructions.txt",
        file_safe_connector(connector)
    )
}

fn write_instructions(
    instructions_path: &Path,
    edid_path: &Path,
    plan: &ExportPlan,
) -> Result<(), ExportError> {
    let instructions = export_instructions(edid_path, plan);
    fs::write(instructions_path, instructions).map_err(|source| ExportError::Write {
        path: instructions_path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ConnectorStatus, HyprlandMonitor};

    fn sample_timing() -> TimingDescriptor {
        TimingDescriptor {
            pixel_clock_khz: 138_500,
            h_active: 1920,
            h_blanking: 160,
            h_front_porch: 48,
            h_sync_width: 32,
            h_back_porch: 80,
            v_active: 1080,
            v_blanking: 31,
            v_front_porch: 3,
            v_sync_width: 5,
            v_back_porch: 23,
            h_sync_positive: true,
            v_sync_positive: false,
            interlaced: false,
        }
    }

    #[test]
    fn instructions_include_kernel_and_hyprland_rules() {
        let plan = build_export_plan_with_file("DP-1", "drmcru_custom_DP-1.bin", sample_timing());
        let instructions = export_instructions(Path::new("/tmp/drmcru_custom_DP-1.bin"), &plan);

        assert!(instructions.contains("sudo install -D -m 0644"));
        assert!(instructions.contains("drm.edid_firmware=DP-1:edid/drmcru_custom_DP-1.bin"));
        assert!(instructions.contains("monitor=DP-1,1920x1080@"));
        assert!(instructions.contains(",auto,1"));
    }

    #[test]
    fn export_plan_can_use_current_hyprland_position_and_scale() {
        let monitor = Monitor {
            connector: "DP-1".to_string(),
            drm_path: None,
            status: ConnectorStatus::Connected,
            hyprland: Some(HyprlandMonitor {
                id: Some(1),
                name: "DP-1".to_string(),
                description: String::new(),
                make: None,
                model: None,
                serial: None,
                active_width: Some(1920),
                active_height: Some(1080),
                refresh_hz: Some(240.0),
                x: Some(2560),
                y: Some(0),
                scale: Some(1.25),
                available_modes: Vec::new(),
                focused: false,
            }),
            edid: None,
        };

        let plan =
            build_export_plan_for_monitor(&monitor, "drmcru_custom_DP-1.bin", sample_timing());

        let rule = plan.hyprland_monitor_rule();
        assert!(rule.starts_with("monitor=DP-1,1920x1080@"));
        assert!(rule.ends_with(",2560x0,1.25"));
    }
}
