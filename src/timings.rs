//! Detailed timing presets, ported from CRU's `DetailedResolutionClass`.
//!
//! The arithmetic deliberately mirrors CRU's integer math (pixel clock in
//! 10 kHz EDID DTD units, refresh in millihertz, vertical geometry in
//! full-frame lines) so a preset produces the same timing CRU would.

mod tables;

use crate::models::TimingDescriptor;
use tables::{AUTOMATIC_CRT, AUTOMATIC_HDTV, AUTOMATIC_PC, NATIVE, TableTiming};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CvtRequest {
    pub width: u16,
    /// Frame height; for interlaced modes this is both fields together.
    pub height: u16,
    /// Field rate for interlaced modes, frame rate otherwise.
    pub refresh_hz: f64,
    pub interlaced: bool,
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
    ExactCrt,
    CvtStandard,
    CvtRbStandard,
    CvtRb2Standard,
    GtfStandard,
    VerticalTotal,
}

impl TimingPreset {
    pub const ALL: [Self; 14] = [
        Self::Manual,
        Self::AutomaticPc,
        Self::AutomaticHdtv,
        Self::AutomaticCrt,
        Self::NativePc,
        Self::NativeHdtv,
        Self::Exact,
        Self::ExactReduced,
        Self::ExactCrt,
        Self::CvtStandard,
        Self::CvtRbStandard,
        Self::CvtRb2Standard,
        Self::GtfStandard,
        Self::VerticalTotal,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => "Manual",
            Self::AutomaticPc => "Automatic PC",
            Self::AutomaticHdtv => "Automatic HDTV",
            Self::AutomaticCrt => "Automatic CRT",
            Self::NativePc => "Native PC",
            Self::NativeHdtv => "Native HDTV",
            Self::Exact => "Exact",
            Self::ExactReduced => "Exact reduced",
            Self::ExactCrt => "Exact CRT",
            Self::CvtStandard => "CVT standard",
            Self::CvtRbStandard => "CVT-RB standard",
            Self::CvtRb2Standard => "CVT-RB2 standard",
            Self::GtfStandard => "GTF standard",
            Self::VerticalTotal => "Vertical total calculator (QFT)",
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

/// Computes the timing a preset yields for the requested size and refresh.
///
/// `current` supplies what CRU keeps from the existing timing: everything for
/// Manual, the sync polarity for Exact/Exact reduced, and the horizontal
/// timing, vertical front porch/sync and pixel clock for the vertical total
/// calculator.
pub fn timing_for_preset(
    preset: TimingPreset,
    request: CvtRequest,
    current: Option<&TimingDescriptor>,
) -> Result<TimingDescriptor, &'static str> {
    if preset == TimingPreset::Manual {
        return current
            .cloned()
            .ok_or("manual timing needs an existing timing");
    }

    let mut calc = Calc::new(request, current)?;
    let ok = match preset {
        TimingPreset::Manual => unreachable!(),
        TimingPreset::AutomaticPc => calc.automatic_pc(),
        TimingPreset::AutomaticHdtv => calc.automatic_hdtv(),
        TimingPreset::AutomaticCrt => calc.automatic_crt(),
        TimingPreset::NativePc => calc.native(Calc::automatic_pc, (true, false)),
        TimingPreset::NativeHdtv => calc.native(Calc::automatic_hdtv, (true, true)),
        TimingPreset::Exact => calc.exact(),
        TimingPreset::ExactReduced => calc.exact_reduced(),
        TimingPreset::ExactCrt => calc.exact_crt(),
        TimingPreset::CvtStandard => calc.standard(Calc::cvt, (false, true)),
        TimingPreset::CvtRbStandard => calc.standard(Calc::cvt_rb, (true, false)),
        TimingPreset::CvtRb2Standard => calc.standard(Calc::cvt_rb2, (true, false)),
        TimingPreset::GtfStandard => calc.standard(Calc::gtf, (false, true)),
        TimingPreset::VerticalTotal => {
            let current = current.ok_or("vertical total calculator needs an existing timing")?;
            calc.vertical_total(current)
        }
    };
    if !ok {
        return Err("preset cannot produce a timing for this size and refresh");
    }
    calc.descriptor()
}

/// CVT reduced blanking (v1) as CRU's "CVT-RB standard" preset computes it.
pub fn cvt_reduced_blanking(request: CvtRequest) -> Result<TimingDescriptor, &'static str> {
    timing_for_preset(TimingPreset::CvtRbStandard, request, None)
}

// CRU sentinels for "no value".
const BLANK: i64 = i32::MIN as i64;
const INVALID: i64 = BLANK + 1;
const DECIMAL_BLANK: i64 = i64::MIN;

/// Pixel clock units per MHz for EDID DTDs (10 kHz).
const PCLOCK_PRECISION: i64 = 100;
/// CVT/GTF pixel clock step: 0.25 MHz.
const PCLOCK_MULTIPLE: i64 = PCLOCK_PRECISION / 4;

// GTF/CVT blanking formula parameters.
const C_PRIME: i64 = (40 - 20) * 128 / 256 + 20;
const M_PRIME: i64 = 600 * 128 / 256;

// Minimum vertical blanking times in picoseconds.
const MIN_TIME_CVT: i64 = 550_000_000;
const MIN_TIME_CVT_RB: i64 = 460_000_000;
const MIN_TIME_CVT_RB2: i64 = 460_000_000;
const MIN_TIME_GTF: i64 = 550_000_000;

/// `{min, max, vsync}` against `v_active * 4000 / h_active` (aspect ±2%).
const ASPECT_V_SYNC: [(i64, i64, i64); 5] = [
    (2205, 2295, 5), // 16:9
    (2352, 2448, 7), // 15:9
    (2450, 2550, 6), // 16:10
    (2940, 3060, 4), // 4:3
    (3136, 3264, 7), // 5:4
];

/// Field limits. CRU validates presets against the EDID DTD limits and
/// guards arithmetic with the wider DisplayID limits ("supported").
struct Limits {
    max_h_active: i64,
    max_h_front: i64,
    max_h_sync: i64,
    max_h_blank: i64,
    max_v_active: i64,
    max_v_front: i64,
    max_v_sync: i64,
    max_v_blank: i64,
    max_pclock: i64,
}

const DTD: Limits = Limits {
    max_h_active: 4095,
    max_h_front: 1023,
    max_h_sync: 1023,
    max_h_blank: 4095,
    max_v_active: 4095,
    max_v_front: 63,
    max_v_sync: 63,
    max_v_blank: 4095,
    max_pclock: 65535,
};

const SUPPORTED: Limits = Limits {
    max_h_active: 65536,
    max_h_front: 32768,
    max_h_sync: 65536,
    max_h_blank: 65536,
    max_v_active: 65536,
    max_v_front: 32768,
    max_v_sync: 65536,
    max_v_blank: 65536,
    max_pclock: 16_777_216,
};

/// Mirror of CRU's `DetailedResolutionClass` calculation state.
struct Calc {
    h_active: i64,
    h_front: i64,
    h_sync: i64,
    h_back: i64,
    h_blank: i64,
    h_total: i64,
    v_active: i64,
    v_front: i64,
    v_sync: i64,
    v_back: i64,
    v_blank: i64,
    v_total: i64,
    /// Requested refresh in mHz.
    v_rate: i64,
    pclock: i64,
    actual_pclock: i64,
    actual_v_rate: i64,
    interlaced: bool,
    h_polarity: bool,
    v_polarity: bool,
    reset_h_polarity: bool,
    reset_v_polarity: bool,
    vertical_total_mode: bool,
}

fn positive(value: i64) -> bool {
    value != BLANK && value != INVALID && value >= 0
}

impl Calc {
    fn new(request: CvtRequest, current: Option<&TimingDescriptor>) -> Result<Self, &'static str> {
        let refresh_millihz = request.refresh_hz * 1000.0;
        if !refresh_millihz.is_finite() || refresh_millihz < 0.0 || refresh_millihz > 1e15 {
            return Err("invalid refresh");
        }
        let (reset_h_polarity, reset_v_polarity) = current
            .map(|timing| (timing.h_sync_positive, timing.v_sync_positive))
            .unwrap_or((true, false));
        Ok(Self {
            h_active: i64::from(request.width),
            h_front: BLANK,
            h_sync: BLANK,
            h_back: BLANK,
            h_blank: BLANK,
            h_total: BLANK,
            v_active: i64::from(request.height),
            v_front: BLANK,
            v_sync: BLANK,
            v_back: BLANK,
            v_blank: BLANK,
            v_total: BLANK,
            v_rate: refresh_millihz.round() as i64,
            pclock: DECIMAL_BLANK,
            actual_pclock: DECIMAL_BLANK,
            actual_v_rate: DECIMAL_BLANK,
            interlaced: request.interlaced,
            h_polarity: reset_h_polarity,
            v_polarity: reset_v_polarity,
            reset_h_polarity,
            reset_v_polarity,
            vertical_total_mode: false,
        })
    }

    fn descriptor(&self) -> Result<TimingDescriptor, &'static str> {
        let field = |value: i64| u16::try_from(value).map_err(|_| "timing exceeds EDID limits");
        let pixel_clock_khz = self
            .actual_pclock
            .checked_mul(1000 / PCLOCK_PRECISION)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or("pixel clock exceeds EDID limits")?;
        Ok(TimingDescriptor {
            pixel_clock_khz,
            h_active: field(self.h_active)?,
            h_blanking: field(self.h_blank)?,
            h_front_porch: field(self.h_front)?,
            h_sync_width: field(self.h_sync)?,
            h_back_porch: field(self.h_back)?,
            v_active: field(self.v_active)?,
            v_blanking: field(self.v_blank)?,
            v_front_porch: field(self.v_front)?,
            v_sync_width: field(self.v_sync)?,
            v_back_porch: field(self.v_back)?,
            h_sync_positive: self.h_polarity,
            v_sync_positive: self.v_polarity,
            interlaced: self.interlaced,
        })
    }

    fn fields(&self) -> i64 {
        i64::from(self.interlaced) + 1
    }

    fn il(&self) -> i64 {
        i64::from(self.interlaced)
    }

    /// Vertical values as stored in a DTD (per field when interlaced).
    fn value(&self, value: i64) -> i64 {
        if self.interlaced && positive(value) {
            value / 2
        } else {
            value
        }
    }

    // --- Range checks -------------------------------------------------------

    fn h_active_in(&self, l: &Limits) -> bool {
        (1..=l.max_h_active).contains(&self.h_active)
    }
    fn h_front_in(&self, l: &Limits) -> bool {
        (1..=l.max_h_front).contains(&self.h_front)
    }
    fn h_sync_in(&self, l: &Limits) -> bool {
        (1..=l.max_h_sync).contains(&self.h_sync)
    }
    // CRU clamps against the DTD validity of the neighbouring fields even
    // when checking the wider limits; the minimum is 1 for both.
    fn clamp_h_front(&self) -> i64 {
        if self.h_front_in(&DTD) {
            self.h_front
        } else {
            1
        }
    }
    fn clamp_h_sync(&self) -> i64 {
        if self.h_sync_in(&DTD) { self.h_sync } else { 1 }
    }
    fn h_back_in(&self, l: &Limits) -> bool {
        self.h_back >= 0
            && self.h_back <= l.max_h_blank - self.clamp_h_front() - self.clamp_h_sync()
    }
    fn h_blank_in(&self, l: &Limits) -> bool {
        self.h_blank >= self.clamp_h_front() + self.clamp_h_sync() && self.h_blank <= l.max_h_blank
    }
    fn h_total_in(&self, l: &Limits) -> bool {
        let valid_active = self.h_active_in(&DTD);
        let min_active = if valid_active { self.h_active } else { 1 };
        let max_active = if valid_active {
            self.h_active
        } else {
            l.max_h_active
        };
        self.h_total >= min_active + self.clamp_h_front() + self.clamp_h_sync()
            && self.h_total <= max_active + l.max_h_blank
    }
    fn v_active_in(&self, l: &Limits) -> bool {
        (1..=l.max_v_active).contains(&self.value(self.v_active))
    }
    fn v_front_in(&self, l: &Limits) -> bool {
        (1..=l.max_v_front).contains(&self.value(self.v_front))
    }
    fn v_sync_in(&self, l: &Limits) -> bool {
        (1..=l.max_v_sync).contains(&self.value(self.v_sync))
    }
    fn clamp_v_front(&self) -> i64 {
        if self.v_front_in(&DTD) {
            self.value(self.v_front)
        } else {
            1
        }
    }
    fn clamp_v_sync(&self) -> i64 {
        if self.v_sync_in(&DTD) {
            self.value(self.v_sync)
        } else {
            1
        }
    }
    fn v_back_in(&self, l: &Limits) -> bool {
        let value = self.value(self.v_back);
        value >= 0 && value <= l.max_v_blank - self.clamp_v_front() - self.clamp_v_sync()
    }
    fn v_blank_in(&self, l: &Limits) -> bool {
        let value = self.value(self.v_blank);
        value >= self.clamp_v_front() + self.clamp_v_sync() && value <= l.max_v_blank
    }
    fn v_total_bounds(&self, l: &Limits) -> (i64, i64) {
        let valid_active = self.v_active_in(&DTD);
        let active = self.value(self.v_active);
        let min_active = if valid_active { active } else { 1 };
        let max_active = if valid_active { active } else { l.max_v_active };
        (
            min_active + self.clamp_v_front() + self.clamp_v_sync(),
            max_active + l.max_v_blank,
        )
    }
    fn v_total_in(&self, l: &Limits) -> bool {
        let (min, max) = self.v_total_bounds(l);
        (min..=max).contains(&self.value(self.v_total))
    }
    fn pclock_in(value: i64, l: &Limits) -> bool {
        (1..=l.max_pclock).contains(&value)
    }

    fn all_in(&self, l: &Limits) -> bool {
        self.h_active_in(l)
            && self.h_front_in(l)
            && self.h_sync_in(l)
            && self.h_back_in(l)
            && self.h_blank_in(l)
            && self.h_total_in(l)
            && self.v_active_in(l)
            && self.v_front_in(l)
            && self.v_sync_in(l)
            && self.v_back_in(l)
            && self.v_blank_in(l)
            && self.v_total_in(l)
            && Self::pclock_in(self.actual_pclock, l)
    }

    fn is_valid(&self) -> bool {
        self.all_in(&DTD)
    }

    fn is_supported(&self) -> bool {
        self.all_in(&SUPPORTED)
    }

    fn v_rate_supported(&self) -> bool {
        if self.vertical_total_mode {
            self.v_rate > 0
        } else {
            self.v_rate >= 0
        }
    }

    fn inputs_supported(&self) -> bool {
        self.h_active_in(&SUPPORTED) && self.v_active_in(&SUPPORTED) && self.v_rate_supported()
    }

    // --- Derived geometry ---------------------------------------------------

    fn calculate_h_blank(&mut self) {
        self.h_blank = if self.h_front_in(&SUPPORTED)
            && self.h_sync_in(&SUPPORTED)
            && self.h_back_in(&SUPPORTED)
        {
            self.h_front + self.h_sync + self.h_back
        } else {
            BLANK
        };
        if !self.h_blank_in(&SUPPORTED) {
            self.h_blank = BLANK;
        }
    }

    fn calculate_h_total(&mut self) {
        self.h_total = if self.h_active_in(&SUPPORTED)
            && self.h_front_in(&SUPPORTED)
            && self.h_sync_in(&SUPPORTED)
            && self.h_back_in(&SUPPORTED)
        {
            self.h_active + self.h_front + self.h_sync + self.h_back
        } else {
            BLANK
        };
        if !self.h_total_in(&SUPPORTED) {
            self.h_total = BLANK;
        }
    }

    fn calculate_h_back_from_h_total(&mut self) {
        self.h_back = if self.h_total_in(&SUPPORTED)
            && self.h_active_in(&SUPPORTED)
            && self.h_front_in(&SUPPORTED)
            && self.h_sync_in(&SUPPORTED)
        {
            self.h_total - self.h_active - self.h_front - self.h_sync
        } else {
            BLANK
        };
        if !self.h_back_in(&SUPPORTED) {
            self.h_back = BLANK;
        }
    }

    fn calculate_v_blank(&mut self) {
        self.v_blank = if self.v_front_in(&SUPPORTED)
            && self.v_sync_in(&SUPPORTED)
            && self.v_back_in(&SUPPORTED)
        {
            self.v_front + self.v_sync + self.v_back
        } else {
            BLANK
        };
        if !self.v_blank_in(&SUPPORTED) {
            self.v_blank = BLANK;
        }
    }

    fn calculate_v_total(&mut self) {
        self.v_total = if self.v_active_in(&SUPPORTED)
            && self.v_front_in(&SUPPORTED)
            && self.v_sync_in(&SUPPORTED)
            && self.v_back_in(&SUPPORTED)
        {
            self.v_active + self.v_front + self.v_sync + self.v_back
        } else {
            BLANK
        };
        if !self.v_total_in(&SUPPORTED) {
            self.v_total = BLANK;
        }
    }

    fn calculate_v_back_from_v_total(&mut self) {
        self.v_back = if self.v_total_in(&SUPPORTED)
            && self.v_active_in(&SUPPORTED)
            && self.v_front_in(&SUPPORTED)
            && self.v_sync_in(&SUPPORTED)
        {
            self.v_total - self.v_active - self.v_front - self.v_sync
        } else {
            BLANK
        };
        if !self.v_back_in(&SUPPORTED) {
            self.v_back = BLANK;
        }
    }

    fn calculate_totals_from_porches(&mut self) {
        self.calculate_h_blank();
        self.calculate_h_total();
        self.calculate_v_blank();
        self.calculate_v_total();
    }

    fn calculate_porches_from_totals(&mut self) {
        self.calculate_h_back_from_h_total();
        self.calculate_h_blank();
        self.calculate_v_back_from_v_total();
        self.calculate_v_blank();
    }

    // --- Rates and clocks ---------------------------------------------------

    fn v_rate_divisor(&self) -> i64 {
        1_000_000_000 * self.fields() / PCLOCK_PRECISION
    }

    fn v_rate_limit(&self, subtract: i64) -> i64 {
        (i64::MAX - subtract) / self.h_total / self.v_total
    }

    fn clock_inputs_supported(&self) -> bool {
        self.v_rate_supported() && self.h_total_in(&SUPPORTED) && self.v_total_in(&SUPPORTED)
    }

    fn finish_pclock(&mut self, value: i64) -> bool {
        self.actual_pclock = if Self::pclock_in(value, &SUPPORTED) {
            value
        } else {
            DECIMAL_BLANK
        };
        self.actual_pclock != DECIMAL_BLANK
    }

    /// Pixel clock rounded up so the refresh is at least the requested one.
    fn calculate_pclock_from_v_rate(&mut self) -> bool {
        if !self.clock_inputs_supported() {
            self.actual_pclock = DECIMAL_BLANK;
            return false;
        }
        if self.v_rate == 0 {
            self.actual_pclock = 1;
            return true;
        }
        let divisor = self.v_rate_divisor();
        if self.v_rate > self.v_rate_limit(divisor - 1) {
            self.actual_pclock = DECIMAL_BLANK;
            return false;
        }
        self.finish_pclock((self.v_rate * self.h_total * self.v_total + (divisor - 1)) / divisor)
    }

    fn calculate_actual_v_rate(&mut self) {
        self.actual_v_rate = if Self::pclock_in(self.actual_pclock, &SUPPORTED)
            && self.h_total_in(&SUPPORTED)
            && self.v_total_in(&SUPPORTED)
        {
            self.actual_pclock * self.v_rate_divisor() / self.h_total / self.v_total
        } else {
            DECIMAL_BLANK
        };
    }

    fn calculate_pclock_for_cvt(&mut self) -> bool {
        if !self.clock_inputs_supported() {
            self.actual_pclock = DECIMAL_BLANK;
            return false;
        }
        let h_period = self.h_period(MIN_TIME_CVT, self.v_front_cvt());
        if h_period <= 0 {
            self.actual_pclock = DECIMAL_BLANK;
            return false;
        }
        let multiplier = 1_000_000 * PCLOCK_PRECISION;
        self.finish_pclock(self.h_total * multiplier / h_period / PCLOCK_MULTIPLE * PCLOCK_MULTIPLE)
    }

    fn calculate_pclock_for_rb(&mut self, multiple: i64) -> bool {
        if !self.clock_inputs_supported() || self.v_rate > self.v_rate_limit(0) {
            self.actual_pclock = DECIMAL_BLANK;
            return false;
        }
        let value = self.v_rate * self.h_total * self.v_total / self.v_rate_divisor();
        self.finish_pclock(value / multiple * multiple)
    }

    fn calculate_pclock_for_gtf(&mut self) -> bool {
        if !self.clock_inputs_supported() {
            self.actual_pclock = DECIMAL_BLANK;
            return false;
        }
        let divisor = self.v_rate_divisor();
        if self.v_rate > self.v_rate_limit(divisor / 2) {
            self.actual_pclock = DECIMAL_BLANK;
            return false;
        }
        self.finish_pclock((self.v_rate * self.h_total * self.v_total + divisor / 2) / divisor)
    }

    fn finish_rates(&mut self) {
        self.calculate_actual_v_rate();
        self.pclock = self.actual_pclock;
    }

    // --- CVT / GTF formula pieces --------------------------------------------

    /// Estimated line period in picoseconds (scaled by 1000 for precision).
    fn h_period_with(&self, min_time: i64, lines: i64) -> i64 {
        if !self.inputs_supported() {
            return 0;
        }
        if self.v_rate == 0 {
            return min_time;
        }
        let fields = self.fields();
        (1_000_000_000_000_000 * fields / self.v_rate - min_time * fields) / lines
    }

    fn h_period(&self, min_time: i64, v_front: i64) -> i64 {
        self.h_period_with(min_time, self.v_active + v_front + self.il())
    }

    fn h_period_rb(&self, min_time: i64) -> i64 {
        self.h_period_with(min_time, self.v_active)
    }

    fn duty_cycle_blank(&self, h_period: i64, clamp: bool, round: i64) -> i64 {
        let mut duty = C_PRIME * 1_000_000_000 - M_PRIME * h_period;
        if clamp && duty < 20_000_000_000 {
            duty = 20_000_000_000;
        }
        // i128: GTF's unclamped duty cycle can be hugely negative.
        let blank =
            i128::from(self.h_active) * i128::from(duty) / (100_000_000_000 - i128::from(duty));
        let blank = (blank + i128::from(round)) / 16 * 16;
        i64::try_from(blank).unwrap_or(BLANK)
    }

    fn h_blank_cvt(&self) -> i64 {
        if !self.inputs_supported() {
            return BLANK;
        }
        let h_period = self.h_period(MIN_TIME_CVT, self.v_front_cvt());
        if h_period <= 0 {
            return BLANK;
        }
        self.duty_cycle_blank(h_period, true, 0)
    }

    fn h_blank_gtf(&self) -> i64 {
        if !self.inputs_supported() {
            return BLANK;
        }
        let h_period = self.h_period(MIN_TIME_GTF, self.v_front_gtf());
        if h_period <= 0 {
            return BLANK;
        }
        self.duty_cycle_blank(h_period, false, 8)
    }

    fn v_front_cvt(&self) -> i64 {
        3 * self.fields()
    }

    fn v_front_gtf(&self) -> i64 {
        self.fields()
    }

    fn v_sync_cvt(&self) -> i64 {
        let aspect = self.v_active * 4000 / self.h_active;
        let lines = ASPECT_V_SYNC
            .iter()
            .find(|(min, max, _)| (*min..=*max).contains(&aspect))
            .map_or(10, |(_, _, lines)| *lines);
        lines * self.fields()
    }

    fn v_back_cvt(&self) -> i64 {
        let h_period = self.h_period(MIN_TIME_CVT, self.v_front_cvt());
        if h_period <= 0 {
            return BLANK;
        }
        let sync_and_back = MIN_TIME_CVT / h_period + 1;
        let v_back = sync_and_back * self.fields() + self.il() - self.v_sync_cvt();
        v_back.max(6 * self.fields() + self.il())
    }

    fn v_back_cvt_rb(&self) -> i64 {
        let h_period = self.h_period_rb(MIN_TIME_CVT_RB);
        if h_period <= 0 {
            return BLANK;
        }
        let v_blank = MIN_TIME_CVT_RB / h_period + 1;
        let v_back = v_blank * self.fields() + self.il() - self.v_front_cvt() - self.v_sync_cvt();
        v_back.max(6 * self.fields() + self.il())
    }

    fn v_back_cvt_rb2(&self) -> i64 {
        6 * self.fields() + self.il()
    }

    fn v_front_cvt_rb2(&self) -> i64 {
        let h_period = self.h_period_rb(MIN_TIME_CVT_RB2);
        if h_period <= 0 {
            return BLANK;
        }
        let v_blank = MIN_TIME_CVT_RB2 / h_period + 1;
        let v_front =
            v_blank * self.fields() + self.il() - 8 * self.fields() - self.v_back_cvt_rb2();
        v_front.max(self.fields())
    }

    fn v_back_gtf(&self) -> i64 {
        let h_period = self.h_period(MIN_TIME_GTF, self.v_front_gtf());
        if h_period <= 0 {
            return BLANK;
        }
        let sync_and_back = (MIN_TIME_GTF * 2 / h_period + 1) / 2;
        sync_and_back * self.fields() + self.il() - 3 * self.fields()
    }

    // --- Formula timings ----------------------------------------------------

    fn cvt(&mut self) -> bool {
        (self.h_polarity, self.v_polarity) = (false, true);
        if !self.inputs_supported() {
            return false;
        }
        let h_blank = self.h_blank_cvt();
        let h_sync = (self.h_active + h_blank) / 100 * 8;
        let h_back = h_blank / 2;
        self.h_front = h_back - h_sync;
        self.h_sync = h_sync;
        self.h_back = h_back;
        self.v_front = self.v_front_cvt();
        self.v_sync = self.v_sync_cvt();
        self.v_back = self.v_back_cvt();
        self.calculate_totals_from_porches();
        self.calculate_pclock_for_cvt();
        self.finish_rates();
        self.is_supported()
    }

    fn cvt_rb(&mut self) -> bool {
        (self.h_polarity, self.v_polarity) = (true, false);
        if !self.inputs_supported() {
            return false;
        }
        (self.h_front, self.h_sync, self.h_back) = (48, 32, 80);
        self.v_front = self.v_front_cvt();
        self.v_sync = self.v_sync_cvt();
        self.v_back = self.v_back_cvt_rb();
        self.calculate_totals_from_porches();
        self.calculate_pclock_for_rb(PCLOCK_MULTIPLE);
        self.finish_rates();
        self.is_supported()
    }

    fn cvt_rb2(&mut self) -> bool {
        (self.h_polarity, self.v_polarity) = (true, false);
        if !self.inputs_supported() {
            return false;
        }
        (self.h_front, self.h_sync, self.h_back) = (8, 32, 40);
        self.v_front = self.v_front_cvt_rb2();
        self.v_sync = 8 * self.fields();
        self.v_back = self.v_back_cvt_rb2();
        self.calculate_totals_from_porches();
        self.calculate_pclock_for_rb(1);
        self.finish_rates();
        self.is_supported()
    }

    fn gtf(&mut self) -> bool {
        (self.h_polarity, self.v_polarity) = (false, true);
        if !self.inputs_supported() {
            return false;
        }
        let h_blank = self.h_blank_gtf();
        let h_sync = (self.h_active + h_blank + 50) / 100 * 8;
        let h_back = h_blank / 2;
        self.h_front = h_back - h_sync;
        self.h_sync = h_sync;
        self.h_back = h_back;
        self.v_front = self.v_front_gtf();
        self.v_sync = 3 * self.fields();
        self.v_back = self.v_back_gtf();
        self.calculate_totals_from_porches();
        self.calculate_pclock_for_gtf();
        self.finish_rates();
        self.is_supported()
    }

    /// CRU's "* standard" presets: run the formula, then rerun it at the
    /// refresh the first pass actually achieved and derive the clock from it.
    fn standard(&mut self, formula: fn(&mut Self) -> bool, polarity: (bool, bool)) -> bool {
        (self.h_polarity, self.v_polarity) = polarity;
        if !self.inputs_supported() {
            return false;
        }
        let old_v_rate = self.v_rate;
        formula(self);
        self.calculate_pclock_from_v_rate();
        self.calculate_actual_v_rate();
        self.v_rate = self.actual_v_rate;
        formula(self);
        self.calculate_pclock_from_v_rate();
        self.calculate_actual_v_rate();
        self.v_rate = old_v_rate;
        self.pclock = self.actual_pclock;
        self.is_supported()
    }

    // --- Table-driven presets -----------------------------------------------

    fn automatic(
        &mut self,
        table: &[TableTiming],
        polarity: (bool, bool),
        fallback: fn(&mut Self) -> bool,
    ) -> bool {
        (self.h_polarity, self.v_polarity) = polarity;
        if !self.inputs_supported() {
            return false;
        }
        let entry = table.iter().find(|entry| {
            entry.h_active == self.h_active
                && entry.v_active == self.v_active
                && entry.interlaced == self.interlaced
                && (entry.min_v_rate..=entry.max_v_rate).contains(&self.v_rate)
        });
        if let Some(entry) = entry {
            [self.h_front, self.h_sync, self.h_back] = entry.h;
            [self.v_front, self.v_sync, self.v_back] = entry.v;
            (self.h_polarity, self.v_polarity) = (entry.h_polarity, entry.v_polarity);
            self.calculate_totals_from_porches();
            self.calculate_pclock_from_v_rate();
            self.finish_rates();
            // Keep the table timing unless only its porches break DTD limits.
            if self.is_valid()
                || !self.h_active_in(&DTD)
                || !self.v_active_in(&DTD)
                || !Self::pclock_in(self.actual_pclock, &DTD)
            {
                return self.is_supported();
            }
        }
        fallback(self)
    }

    fn automatic_pc(&mut self) -> bool {
        self.automatic(AUTOMATIC_PC, (true, false), |calc| {
            calc.standard(Self::cvt_rb, (true, false))
        })
    }

    fn automatic_hdtv(&mut self) -> bool {
        self.automatic(AUTOMATIC_HDTV, (true, true), |calc| {
            calc.standard(Self::cvt_rb, (true, false))
        })
    }

    fn automatic_crt(&mut self) -> bool {
        self.automatic(AUTOMATIC_CRT, (false, true), |calc| {
            calc.standard(Self::cvt, (false, true))
        })
    }

    /// Porches from the automatic preset at the panel's native refresh,
    /// with the clock rescaled to the requested refresh.
    fn native(&mut self, automatic: fn(&mut Self) -> bool, polarity: (bool, bool)) -> bool {
        (self.h_polarity, self.v_polarity) = polarity;
        if !self.inputs_supported() {
            return false;
        }
        let native_rate = NATIVE
            .iter()
            .find(|(h, v, interlaced, _)| {
                *h == self.h_active && *v == self.v_active && *interlaced == self.interlaced
            })
            .map_or(60_000, |(_, _, _, rate)| *rate);
        let old_v_rate = self.v_rate;
        self.v_rate = native_rate;
        automatic(self);
        self.v_rate = old_v_rate;
        self.calculate_pclock_from_v_rate();
        self.finish_rates();
        self.is_supported()
    }

    // --- Exact presets ------------------------------------------------------

    /// Totals rounded so the requested refresh is hit exactly.
    fn round_v_total(&self, step: i64) -> i64 {
        let lines = self.v_active + self.v_front + self.v_sync;
        if self.interlaced {
            (lines + step) / (step * 2) * (step * 2) + step
        } else {
            (lines + step) / step * step
        }
    }

    fn exact_h_total(&self) -> i64 {
        let step = if self.interlaced { 160 } else { 80 };
        (self.h_active + self.h_front + self.h_sync + step) / step * step
    }

    fn exact(&mut self) -> bool {
        (self.h_polarity, self.v_polarity) = (self.reset_h_polarity, self.reset_v_polarity);
        if !self.inputs_supported() {
            return false;
        }
        (self.h_front, self.h_sync) = (48, 32);
        self.h_total = self.exact_h_total();
        self.v_front = self.v_front_cvt();
        self.v_sync = self.v_sync_cvt();
        self.v_total = self.round_v_total(125);
        self.calculate_porches_from_totals();
        self.calculate_pclock_from_v_rate();
        self.finish_rates();
        self.is_supported()
    }

    fn exact_reduced_v_total(&self) -> i64 {
        if self.h_total % 400 == 0 || self.v_rate % 5000 == 0 {
            self.round_v_total(25)
        } else {
            self.round_v_total(125)
        }
    }

    fn exact_reduced(&mut self) -> bool {
        (self.h_polarity, self.v_polarity) = (self.reset_h_polarity, self.reset_v_polarity);
        if !self.inputs_supported() {
            return false;
        }
        (self.h_front, self.h_sync) = (48, 32);
        self.h_total = self.exact_h_total();
        self.v_front = self.v_front_cvt();
        self.v_sync = self.v_sync_cvt();
        self.v_total = self.exact_reduced_v_total();
        self.calculate_porches_from_totals();
        self.calculate_pclock_from_v_rate();

        // Narrower front porch for single-link DVI sized modes above 165 MHz.
        if self.h_active * self.v_active <= 2048 * 1200
            && self.actual_pclock > 165 * PCLOCK_PRECISION
            && !self.interlaced
        {
            (self.h_front, self.h_sync) = (24, 32);
            self.h_total = self.exact_h_total();
            self.v_front = self.v_front_cvt();
            self.v_sync = self.v_sync_cvt();
            self.v_total = self.exact_reduced_v_total();
            self.calculate_porches_from_totals();
            self.calculate_pclock_from_v_rate();
        }

        self.finish_rates();
        self.is_supported()
    }

    fn exact_crt(&mut self) -> bool {
        (self.h_polarity, self.v_polarity) = (self.reset_h_polarity, self.reset_v_polarity);
        if !self.inputs_supported() {
            return false;
        }
        self.standard(Self::cvt, (false, true));
        if self.interlaced {
            self.h_total = (self.h_total + 159) / 160 * 160;
            self.v_total = (self.v_total + 124) / 250 * 250 + 125;
        } else {
            self.h_total = (self.h_total + 79) / 80 * 80;
            self.v_total = (self.v_total + 124) / 125 * 125;
        }
        self.calculate_porches_from_totals();
        self.calculate_pclock_from_v_rate();
        self.finish_rates();
        self.is_supported()
    }

    // --- Vertical total calculator ------------------------------------------

    /// Keeps the horizontal timing and pixel clock and stretches the vertical
    /// back porch until the requested refresh is reached (quick frame
    /// transport).
    fn vertical_total(&mut self, current: &TimingDescriptor) -> bool {
        self.vertical_total_mode = true;
        self.h_front = i64::from(current.h_front_porch);
        self.h_sync = i64::from(current.h_sync_width);
        self.h_back = i64::from(current.h_back_porch);
        self.v_front = i64::from(current.v_front_porch);
        self.v_sync = i64::from(current.v_sync_width);
        self.pclock = i64::from(current.pixel_clock_khz) * PCLOCK_PRECISION / 1000;
        self.calculate_h_blank();
        self.calculate_h_total();

        self.v_total = BLANK;
        if Self::pclock_in(self.pclock, &SUPPORTED)
            && self.h_total_in(&SUPPORTED)
            && self.v_rate_supported()
        {
            let mut value = self.pclock * self.v_rate_divisor() / self.h_total / self.v_rate;
            if self.interlaced && value % 2 == 0 {
                value -= 1;
            }
            let (min, max) = self.v_total_bounds(&SUPPORTED);
            if (min..=max).contains(&self.value(value)) {
                self.v_total = value;
            }
        }
        self.calculate_v_back_from_v_total();
        self.calculate_v_blank();
        self.calculate_pclock_from_v_rate();
        self.finish_rates();
        self.is_supported()
    }
}

#[cfg(test)]
mod tests;
