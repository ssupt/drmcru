use crate::models::TimingDescriptor;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CvtRequest {
    pub width: u16,
    pub height: u16,
    pub refresh_hz: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingPreset {
    Manual,
    AutomaticPc,
    AutomaticHdtv,
    AutomaticCrt,
    NativePc,
    NativeHdtv,
    Exact,
    ExactReduced,
}

impl TimingPreset {
    pub const ALL: [Self; 8] = [
        Self::Manual,
        Self::AutomaticPc,
        Self::AutomaticHdtv,
        Self::AutomaticCrt,
        Self::NativePc,
        Self::NativeHdtv,
        Self::Exact,
        Self::ExactReduced,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => "Manual",
            Self::AutomaticPc => "Automatic (PC)",
            Self::AutomaticHdtv => "Automatic (HDTV)",
            Self::AutomaticCrt => "Automatic (CRT)",
            Self::NativePc => "Native (PC)",
            Self::NativeHdtv => "Native (HDTV)",
            Self::Exact => "Exact",
            Self::ExactReduced => "Exact reduced",
        }
    }

    pub fn cycle(self, delta: isize) -> Self {
        let index = Self::ALL
            .iter()
            .position(|preset| *preset == self)
            .unwrap_or_default();
        let len = Self::ALL.len() as isize;
        Self::ALL[(index as isize + delta).rem_euclid(len) as usize]
    }
}

pub fn timing_for_preset(
    preset: TimingPreset,
    request: CvtRequest,
    current: Option<&TimingDescriptor>,
) -> TimingDescriptor {
    match preset {
        TimingPreset::Manual => current
            .cloned()
            .unwrap_or_else(|| cvt_reduced_blanking(request)),
        TimingPreset::AutomaticPc | TimingPreset::NativePc => cvt_reduced_blanking(request),
        TimingPreset::AutomaticHdtv | TimingPreset::NativeHdtv => hdtv_timing(request),
        TimingPreset::AutomaticCrt => crt_timing(request),
        TimingPreset::Exact => exact_timing(request, current),
        TimingPreset::ExactReduced => {
            let mut timing = cvt_reduced_blanking(request);
            timing.pixel_clock_khz = exact_pixel_clock_khz(
                u32::from(timing.h_total()),
                u32::from(timing.v_total()),
                request.refresh_hz,
            );
            timing
        }
    }
}

/// Produces a practical CVT reduced-blanking style timing.
///
/// The constants match the common CVT-RB v1 porch shape: 160 horizontal
/// blanking pixels, 48 pixel front porch, 32 pixel sync, 3 line vertical front
/// porch, and a minimum 460 us vertical blanking interval.
pub fn cvt_reduced_blanking(request: CvtRequest) -> TimingDescriptor {
    const CELL_GRANULARITY: u16 = 8;
    const H_BLANK: u16 = 160;
    const H_FRONT_PORCH: u16 = 48;
    const H_SYNC_WIDTH: u16 = 32;
    const V_FRONT_PORCH: u16 = 3;
    const MIN_V_BACK_PORCH: u16 = 6;
    const RB_MIN_VBLANK_US: f64 = 460.0;
    const CLOCK_STEP_KHZ: u32 = 250;

    let h_active = round_down_to(request.width, CELL_GRANULARITY);
    let v_active = request.height;
    let refresh_hz = request.refresh_hz.max(1.0);
    let v_sync_width = vertical_sync_width(h_active, v_active);
    let h_front_porch: u16 = H_FRONT_PORCH;
    let h_sync_width: u16 = H_SYNC_WIDTH;
    let h_blanking: u16 = H_BLANK;
    let v_front_porch: u16 = V_FRONT_PORCH;
    let h_total = u32::from(h_active) + u32::from(h_blanking);
    let estimated_h_period_us =
        ((1_000_000.0 / refresh_hz) - RB_MIN_VBLANK_US) / f64::from(v_active);
    let min_vbi_lines = V_FRONT_PORCH + v_sync_width + MIN_V_BACK_PORCH;
    let rb_vbi_lines = (RB_MIN_VBLANK_US / estimated_h_period_us).floor() as u16 + 1;
    let v_blanking = rb_vbi_lines.max(min_vbi_lines);
    let v_total = u32::from(v_active) + u32::from(v_blanking);
    let pixel_clock_khz = round_down_to_step(
        f64::from(h_total * v_total) * refresh_hz / 1000.0,
        CLOCK_STEP_KHZ,
    );

    TimingDescriptor {
        pixel_clock_khz,
        h_active,
        h_blanking,
        h_front_porch,
        h_sync_width,
        h_back_porch: h_blanking - h_front_porch - h_sync_width,
        v_active,
        v_blanking,
        v_front_porch,
        v_sync_width,
        v_back_porch: v_blanking - v_front_porch - v_sync_width,
        h_sync_positive: true,
        v_sync_positive: false,
        interlaced: false,
    }
}

fn hdtv_timing(request: CvtRequest) -> TimingDescriptor {
    let h_active = round_down_to(request.width, 8);
    let v_active = request.height;
    let (h_front_porch, h_sync_width, h_back_porch, v_front_porch, v_sync_width, v_back_porch) =
        if h_active >= 1920 || v_active >= 1080 {
            (88, 44, 148, 4, 5, 36)
        } else if h_active >= 1280 || v_active >= 720 {
            (110, 40, 220, 5, 5, 20)
        } else {
            (16, 62, 60, 9, 6, 30)
        };
    detailed_from_parts(
        h_active,
        v_active,
        h_front_porch,
        h_sync_width,
        h_back_porch,
        v_front_porch,
        v_sync_width,
        v_back_porch,
        request.refresh_hz,
        true,
        true,
        false,
    )
}

fn crt_timing(request: CvtRequest) -> TimingDescriptor {
    let h_active = round_down_to(request.width, 8);
    let v_active = request.height;
    let h_blanking = round_down_to((h_active / 4).max(160), 8);
    let h_sync_width = round_down_to((h_blanking / 3).max(32), 8);
    let h_front_porch = round_down_to((h_blanking / 6).max(16), 8);
    let h_back_porch = h_blanking.saturating_sub(h_front_porch + h_sync_width);
    let v_front_porch = 3;
    let v_sync_width = vertical_sync_width(h_active, v_active);
    let v_back_porch = 30;

    detailed_from_parts(
        h_active,
        v_active,
        h_front_porch,
        h_sync_width,
        h_back_porch,
        v_front_porch,
        v_sync_width,
        v_back_porch,
        request.refresh_hz,
        false,
        false,
        false,
    )
}

fn exact_timing(request: CvtRequest, current: Option<&TimingDescriptor>) -> TimingDescriptor {
    let base = current
        .cloned()
        .unwrap_or_else(|| cvt_reduced_blanking(request));
    let mut timing = TimingDescriptor {
        h_active: request.width,
        v_active: request.height,
        ..base
    };
    timing.h_blanking = timing.h_front_porch + timing.h_sync_width + timing.h_back_porch;
    timing.v_blanking = timing.v_front_porch + timing.v_sync_width + timing.v_back_porch;
    timing.pixel_clock_khz = exact_pixel_clock_khz(
        u32::from(timing.h_total()),
        u32::from(timing.v_total()),
        request.refresh_hz,
    );
    timing
}

#[allow(clippy::too_many_arguments)]
fn detailed_from_parts(
    h_active: u16,
    v_active: u16,
    h_front_porch: u16,
    h_sync_width: u16,
    h_back_porch: u16,
    v_front_porch: u16,
    v_sync_width: u16,
    v_back_porch: u16,
    refresh_hz: f64,
    h_sync_positive: bool,
    v_sync_positive: bool,
    interlaced: bool,
) -> TimingDescriptor {
    let h_blanking = h_front_porch + h_sync_width + h_back_porch;
    let v_blanking = v_front_porch + v_sync_width + v_back_porch;
    let pixel_clock_khz = exact_pixel_clock_khz(
        u32::from(h_active + h_blanking),
        u32::from(v_active + v_blanking),
        refresh_hz,
    );

    TimingDescriptor {
        pixel_clock_khz,
        h_active,
        h_blanking,
        h_front_porch,
        h_sync_width,
        h_back_porch,
        v_active,
        v_blanking,
        v_front_porch,
        v_sync_width,
        v_back_porch,
        h_sync_positive,
        v_sync_positive,
        interlaced,
    }
}

fn exact_pixel_clock_khz(h_total: u32, v_total: u32, refresh_hz: f64) -> u32 {
    (f64::from(h_total * v_total) * refresh_hz / 1000.0).round() as u32
}

fn round_down_to(value: u16, granularity: u16) -> u16 {
    value / granularity * granularity
}

fn round_down_to_step(value: f64, step: u32) -> u32 {
    ((value / f64::from(step)).floor() as u32) * step
}

fn vertical_sync_width(width: u16, height: u16) -> u16 {
    if has_aspect(width, height, 4, 3) {
        4
    } else if has_aspect(width, height, 16, 9) {
        5
    } else if has_aspect(width, height, 16, 10) {
        6
    } else if has_aspect(width, height, 5, 4) || has_aspect(width, height, 15, 9) {
        7
    } else {
        10
    }
}

fn has_aspect(width: u16, height: u16, aspect_width: u16, aspect_height: u16) -> bool {
    u32::from(width) * u32::from(aspect_height) == u32::from(height) * u32::from(aspect_width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rb_timing_has_expected_totals() {
        let timing = cvt_reduced_blanking(CvtRequest {
            width: 1920,
            height: 1080,
            refresh_hz: 60.0,
        });

        assert_eq!(timing.h_active, 1920);
        assert_eq!(timing.h_blanking, 160);
        assert_eq!(timing.v_blanking, 31);
        assert_eq!(timing.v_sync_width, 5);
        assert_eq!(timing.pixel_clock_khz, 138_500);
    }

    #[test]
    fn exact_preset_preserves_current_blanking_and_updates_clock() {
        let current = cvt_reduced_blanking(CvtRequest {
            width: 1280,
            height: 1080,
            refresh_hz: 60.0,
        });
        let timing = timing_for_preset(
            TimingPreset::Exact,
            CvtRequest {
                width: 1280,
                height: 1080,
                refresh_hz: 100.0,
            },
            Some(&current),
        );

        assert_eq!(timing.h_front_porch, current.h_front_porch);
        assert_eq!(timing.h_back_porch, current.h_back_porch);
        assert!((timing.refresh_hz().unwrap() - 100.0).abs() < 0.001);
    }

    #[test]
    fn exact_reduced_uses_reduced_blanking_shape() {
        let timing = timing_for_preset(
            TimingPreset::ExactReduced,
            CvtRequest {
                width: 1920,
                height: 1080,
                refresh_hz: 120.0,
            },
            None,
        );

        assert_eq!(timing.h_blanking, 160);
        assert_eq!(timing.h_front_porch, 48);
        assert!((timing.refresh_hz().unwrap() - 120.0).abs() < 0.001);
    }
}
