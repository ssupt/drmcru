use crate::models::TimingDescriptor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingWarningSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingWarning {
    pub severity: TimingWarningSeverity,
    pub message: String,
}

impl TimingWarning {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            severity: TimingWarningSeverity::Error,
            message: message.into(),
        }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: TimingWarningSeverity::Warning,
            message: message.into(),
        }
    }

    pub fn label(&self) -> &'static str {
        match self.severity {
            TimingWarningSeverity::Error => "Error",
            TimingWarningSeverity::Warning => "Warning",
        }
    }
}

pub fn validate_timing(timing: &TimingDescriptor) -> Vec<TimingWarning> {
    let mut warnings = Vec::new();

    if timing.h_active == 0 || timing.v_active == 0 {
        warnings.push(TimingWarning::error(
            "active horizontal and vertical pixels must be greater than zero",
        ));
    }
    if timing.pixel_clock_khz == 0 {
        warnings.push(TimingWarning::error(
            "pixel clock must be greater than zero",
        ));
    } else if timing.pixel_clock_khz < 10 {
        warnings.push(TimingWarning::error(
            "pixel clock must be at least 10 kHz for an EDID DTD",
        ));
    }
    if timing.pixel_clock_khz > 655_350 {
        warnings.push(TimingWarning::error(format!(
            "pixel clock {} kHz exceeds EDID DTD encoding limit 655350 kHz",
            timing.pixel_clock_khz
        )));
    } else if timing.pixel_clock_khz > 600_000 {
        warnings.push(TimingWarning::warning(format!(
            "pixel clock is high: {} kHz",
            timing.pixel_clock_khz
        )));
    }

    validate_12_bit(&mut warnings, "horizontal active", timing.h_active);
    validate_12_bit(&mut warnings, "horizontal blanking", timing.h_blanking);
    validate_12_bit(&mut warnings, "vertical active", timing.v_active);
    validate_12_bit(&mut warnings, "vertical blanking", timing.v_blanking);
    validate_10_bit(
        &mut warnings,
        "horizontal front porch",
        timing.h_front_porch,
    );
    validate_10_bit(&mut warnings, "horizontal sync width", timing.h_sync_width);
    validate_6_bit(&mut warnings, "vertical front porch", timing.v_front_porch);
    validate_6_bit(&mut warnings, "vertical sync width", timing.v_sync_width);

    let h_parts = u32::from(timing.h_front_porch)
        + u32::from(timing.h_sync_width)
        + u32::from(timing.h_back_porch);
    let v_parts = u32::from(timing.v_front_porch)
        + u32::from(timing.v_sync_width)
        + u32::from(timing.v_back_porch);

    if h_parts != u32::from(timing.h_blanking) {
        warnings.push(TimingWarning::error(format!(
            "horizontal porch/sync sum {h_parts} does not match blanking {}",
            timing.h_blanking
        )));
    }
    if v_parts != u32::from(timing.v_blanking) {
        warnings.push(TimingWarning::error(format!(
            "vertical porch/sync sum {v_parts} does not match blanking {}",
            timing.v_blanking
        )));
    }
    if timing.h_front_porch + timing.h_sync_width > timing.h_blanking {
        warnings.push(TimingWarning::error(
            "horizontal front porch plus sync width exceeds horizontal blanking",
        ));
    }
    if timing.v_front_porch + timing.v_sync_width > timing.v_blanking {
        warnings.push(TimingWarning::error(
            "vertical front porch plus sync width exceeds vertical blanking",
        ));
    }

    if timing.h_blanking == 0 || timing.v_blanking == 0 {
        warnings.push(TimingWarning::error(
            "horizontal and vertical blanking must be greater than zero",
        ));
    } else {
        if timing.h_blanking < 16 {
            warnings.push(TimingWarning::warning(format!(
                "horizontal blanking is very small: {} pixels",
                timing.h_blanking
            )));
        }
        if timing.v_blanking < 3 {
            warnings.push(TimingWarning::warning(format!(
                "vertical blanking is very small: {} lines",
                timing.v_blanking
            )));
        }
    }

    match timing.refresh_hz() {
        Some(refresh) if !(23.0..=360.0).contains(&refresh) => warnings.push(
            TimingWarning::warning(format!("refresh rate is unusual: {refresh:.3} Hz")),
        ),
        None => warnings.push(TimingWarning::error("refresh rate cannot be calculated")),
        _ => {}
    }

    if timing.interlaced {
        warnings.push(TimingWarning::warning(
            "interlaced DTDs are uncommon for modern Wayland/DRM workflows",
        ));
    }

    warnings
}

pub fn internal_panel_scaling_warning(
    connector: &str,
    native: &TimingDescriptor,
    timings: &[TimingDescriptor],
) -> Option<TimingWarning> {
    if !connector.starts_with("eDP-") {
        return None;
    }

    let mut sizes = timings
        .iter()
        .filter(|timing| timing.h_active != native.h_active || timing.v_active != native.v_active)
        .map(|timing| format!("{}x{}", timing.h_active, timing.v_active))
        .collect::<Vec<_>>();
    sizes.sort();
    sizes.dedup();

    (!sizes.is_empty()).then(|| {
        TimingWarning::warning(format!(
            "internal panel modes {} differ from native {}x{}; many eDP panels cannot scale smaller scanouts and may tile or corrupt the image. Keep the native active size and change only the refresh timing",
            sizes.join(", "),
            native.h_active,
            native.v_active
        ))
    })
}

fn validate_12_bit(warnings: &mut Vec<TimingWarning>, label: &str, value: u16) {
    validate_limit(warnings, label, u32::from(value), 0x0fff);
}

fn validate_10_bit(warnings: &mut Vec<TimingWarning>, label: &str, value: u16) {
    validate_limit(warnings, label, u32::from(value), 0x03ff);
}

fn validate_6_bit(warnings: &mut Vec<TimingWarning>, label: &str, value: u16) {
    validate_limit(warnings, label, u32::from(value), 0x003f);
}

fn validate_limit(warnings: &mut Vec<TimingWarning>, label: &str, value: u32, limit: u32) {
    if value > limit {
        warnings.push(TimingWarning::error(format!(
            "{label} value {value} exceeds EDID DTD field limit {limit}"
        )));
    } else if value > limit.saturating_mul(95) / 100 {
        warnings.push(TimingWarning::warning(format!(
            "{label} value {value} is close to EDID DTD field limit {limit}"
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_timing() -> TimingDescriptor {
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
    fn valid_timing_has_no_warnings() {
        assert!(validate_timing(&valid_timing()).is_empty());
    }

    #[test]
    fn warns_about_non_native_internal_panel_modes() {
        let native = valid_timing();
        let mut smaller = native.clone();
        smaller.h_active = 1280;
        smaller.v_active = 720;

        assert!(
            internal_panel_scaling_warning("eDP-1", &native, std::slice::from_ref(&native))
                .is_none()
        );
        assert!(internal_panel_scaling_warning("DP-1", &native, &[smaller.clone()]).is_none());
        assert!(internal_panel_scaling_warning("eDP-1", &native, &[smaller]).is_some());
    }

    #[test]
    fn detects_bad_blanking_sum_and_clock_limit() {
        let mut timing = valid_timing();
        timing.h_back_porch = 1;
        timing.pixel_clock_khz = 700_000;

        let warnings = validate_timing(&timing);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.severity == TimingWarningSeverity::Error)
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.message.contains("horizontal porch/sync sum"))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.message.contains("exceeds EDID DTD encoding limit"))
        );
    }
}
