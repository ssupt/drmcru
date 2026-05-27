use crate::discovery;
use crate::export::custom_edid_file_name;
use crate::hyprland_config;
use crate::install::{self, InstalledOverrideStatus, SystemSupportReport, UninstallPlan};
use crate::models::Monitor;
use anyhow::Result;

pub fn run() -> Result<()> {
    let monitors = discovery::discover_monitors()?;
    let support = install::inspect_system_support();
    let overrides = monitors
        .iter()
        .map(|monitor| inspect_connector_override(&monitor.connector))
        .collect::<Vec<_>>();

    println!("{}", report_text(&monitors, &support, &overrides));
    Ok(())
}

fn report_text(
    monitors: &[Monitor],
    support: &SystemSupportReport,
    overrides: &[InstalledOverrideStatus],
) -> String {
    let mut lines = vec![
        format!("drmcru {}", env!("CARGO_PKG_VERSION")),
        "Doctor report".to_string(),
        String::new(),
        "System support".to_string(),
    ];
    lines.extend(support.summary_lines());
    if support.is_supported() {
        lines.push("Automatic Apply: supported".to_string());
    } else {
        lines.push("Automatic Apply: unsupported".to_string());
        for issue in support.blocking_issues() {
            lines.push(format!("- {issue}"));
        }
    }

    lines.push(String::new());
    lines.push(format!("Monitors: {}", monitors.len()));
    for (index, monitor) in monitors.iter().enumerate() {
        lines.push(String::new());
        lines.push(format!("{}. {}", index + 1, monitor.connector));
        lines.push(format!("   Status: {:?}", monitor.status));
        lines.push(format!(
            "   DRM path: {}",
            monitor
                .drm_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "(unknown)".to_string())
        ));

        if let Some(hyprland) = &monitor.hyprland {
            lines.push(format!(
                "   Hyprland: {}x{} @ {:.3} Hz, {} available mode(s)",
                hyprland.active_width.unwrap_or_default(),
                hyprland.active_height.unwrap_or_default(),
                hyprland.refresh_hz.unwrap_or_default(),
                hyprland.available_modes.len()
            ));
        } else {
            lines.push("   Hyprland: not reported".to_string());
        }

        if let Some(edid) = &monitor.edid {
            lines.push(format!(
                "   EDID: {} extension block(s), base checksum {}",
                edid.extension_blocks,
                ok_bad(edid.checksum_valid)
            ));
            lines.push(format!(
                "   EDID name: {}",
                edid.monitor_name.as_deref().unwrap_or("(none)")
            ));
        } else {
            lines.push("   EDID: unavailable".to_string());
        }

        if let Some(status) = overrides.get(index) {
            let firmware_comparison = install::compare_firmware_to_live_edid(
                &status.firmware_target,
                monitor.edid.as_ref().map(|edid| edid.raw.as_slice()),
            );
            lines.push(format!("   Override: {}", status.short_label()));
            lines.push(format!(
                "   Override files: firmware {}, mkinitcpio {}, boot config {} ({}/{} cmdline entries), entry-tool {}, active kernel {}",
                yes_no(status.firmware_present),
                yes_no(status.mkinitcpio_references_firmware),
                yes_no(status.bootloader_references_kernel_parameter),
                status.bootloader_cmdline_entries_with_kernel_parameter,
                status.bootloader_cmdline_entries,
                yes_no(status.limine_entry_tool_references_kernel_parameter),
                yes_no(status.active_kernel_references_kernel_parameter)
            ));
            lines.push(format!(
                "   Override EDID match: {} ({})",
                firmware_comparison.short_label(),
                firmware_comparison.detail()
            ));
            for warning in &status.read_warnings {
                lines.push(format!("   Override warning: {warning}"));
            }
        }

        let config = hyprland_config::inspect_connector_rules(&monitor.connector);
        if !config.connector_rules.is_empty() {
            lines.push(format!(
                "   Hyprland config rules: {} literal rule(s)",
                config.connector_rules.len()
            ));
            if let Some(last) = &config.last_connector_rule {
                lines.push(format!("   Hyprland config winner: {}", last.location()));
                lines.push(format!(
                    "   Hyprland config mode: {}",
                    last.normalized_rule()
                ));
            }
        }
        for warning in config.read_warnings.iter().take(3) {
            lines.push(format!("   Hyprland config warning: {warning}"));
        }
        if config.read_warnings.len() > 3 {
            lines.push(format!(
                "   Hyprland config warning: ... {} more",
                config.read_warnings.len() - 3
            ));
        }
    }

    lines.join("\n")
}

fn inspect_connector_override(connector: &str) -> InstalledOverrideStatus {
    let edid_file_name = custom_edid_file_name(connector);
    let plan = UninstallPlan {
        connector: connector.to_string(),
        kernel_parameter: format!("drm.edid_firmware={connector}:edid/{edid_file_name}"),
        edid_file_name,
    };
    install::inspect_installed_override(&plan)
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn ok_bad(value: bool) -> &'static str {
    if value { "ok" } else { "bad" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ConnectorStatus, EdidData};
    use std::path::PathBuf;

    fn sample_support() -> SystemSupportReport {
        SystemSupportReport {
            pkexec_available: true,
            mkinitcpio_available: true,
            mkinitcpio_conf_present: true,
            mkinitcpio_presets_present: true,
            limine_mkinitcpio_available: true,
            limine_conf_present: true,
            limine_has_cmdline: true,
            read_warnings: Vec::new(),
        }
    }

    fn sample_override() -> InstalledOverrideStatus {
        InstalledOverrideStatus {
            connector: "DP-1".to_string(),
            edid_file_name: "drmcru_custom_DP-1.bin".to_string(),
            kernel_parameter: "drm.edid_firmware=DP-1:edid/drmcru_custom_DP-1.bin".to_string(),
            firmware_target: PathBuf::from("/lib/firmware/edid/drmcru_custom_DP-1.bin"),
            firmware_present: true,
            mkinitcpio_references_firmware: true,
            bootloader_references_kernel_parameter: true,
            limine_entry_tool_references_kernel_parameter: true,
            bootloader_cmdline_entries: 1,
            bootloader_cmdline_entries_with_kernel_parameter: 1,
            active_kernel_references_kernel_parameter: true,
            read_warnings: Vec::new(),
        }
    }

    #[test]
    fn report_includes_system_and_monitor_summary() {
        let monitor = Monitor {
            connector: "DP-1".to_string(),
            drm_path: Some(PathBuf::from("/sys/class/drm/card0-DP-1")),
            status: ConnectorStatus::Connected,
            hyprland: None,
            edid: Some(EdidData {
                raw: vec![0; 128],
                manufacturer_id: Some("ABC".to_string()),
                product_code: Some(1),
                serial_number: Some(2),
                monitor_name: Some("Test Monitor".to_string()),
                descriptor_text: Vec::new(),
                established_timings: Vec::new(),
                standard_timings: Vec::new(),
                detailed_timings: Vec::new(),
                cta_blocks: Vec::new(),
                extension_blocks: 0,
                checksum_valid: true,
            }),
        };

        let report = report_text(&[monitor], &sample_support(), &[sample_override()]);

        assert!(report.contains("Automatic Apply: supported"));
        assert!(report.contains("1. DP-1"));
        assert!(report.contains("EDID name: Test Monitor"));
        assert!(report.contains("Override: active"));
        assert!(report.contains("Override EDID match:"));
    }
}
