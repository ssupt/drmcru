use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    pub connector: String,
    pub drm_path: Option<PathBuf>,
    pub status: ConnectorStatus,
    pub hyprland: Option<HyprlandMonitor>,
    pub edid: Option<EdidData>,
}

impl Monitor {
    pub fn label(&self) -> String {
        match &self.hyprland {
            Some(hypr) if !hypr.description.is_empty() => {
                format!("{} - {}", self.connector, hypr.description)
            }
            _ => self.connector.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorStatus {
    Connected,
    Disconnected,
    Unknown,
}

impl ConnectorStatus {
    pub fn from_sysfs(value: &str) -> Self {
        match value.trim() {
            "connected" => Self::Connected,
            "disconnected" => Self::Disconnected,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HyprlandMonitor {
    pub id: Option<i64>,
    pub name: String,
    pub description: String,
    pub make: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub active_width: Option<u32>,
    pub active_height: Option<u32>,
    pub refresh_hz: Option<f64>,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub scale: Option<f64>,
    pub available_modes: Vec<String>,
    pub focused: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EdidData {
    pub raw: Vec<u8>,
    pub manufacturer_id: Option<String>,
    pub product_code: Option<u16>,
    pub serial_number: Option<u32>,
    pub monitor_name: Option<String>,
    pub descriptor_text: Vec<String>,
    pub established_timings: Vec<EstablishedTiming>,
    pub standard_timings: Vec<StandardTiming>,
    pub detailed_timings: Vec<TimingDescriptor>,
    pub cta_blocks: Vec<Cta861Block>,
    pub displayid_blocks: Vec<DisplayIdBlock>,
    pub extension_blocks: u8,
    pub checksum_valid: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cta861Block {
    pub extension_index: u8,
    pub revision: u8,
    pub dtd_offset: u8,
    pub checksum_valid: bool,
    pub data_blocks: Vec<CtaDataBlock>,
    pub detailed_timings: Vec<TimingDescriptor>,
    pub available_dtd_slots: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayIdBlock {
    pub extension_index: u8,
    pub version_major: u8,
    pub version_minor: u8,
    pub product_type: u8,
    pub extension_count: u8,
    pub checksum_valid: bool,
    pub data_blocks: Vec<DisplayIdDataBlock>,
    pub detailed_timings: Vec<DisplayIdDetailedTiming>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayIdDataBlock {
    pub tag: u8,
    pub revision: u8,
    pub payload_len: usize,
}

impl DisplayIdDataBlock {
    pub fn label(&self) -> String {
        match self.tag {
            0x03 => "Type I timings".to_string(),
            0x04 => "Type II timings".to_string(),
            0x05 => "Type III timings".to_string(),
            0x12 => "Tiled display".to_string(),
            0x20 => "Product ID".to_string(),
            0x21 => "Display parameters".to_string(),
            0x22 => "Color characteristics".to_string(),
            tag => format!("DisplayID tag 0x{tag:02x}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayIdDetailedTiming {
    pub extension_index: u8,
    pub data_block_index: usize,
    pub descriptor_index: usize,
    pub raw_flags: u8,
    pub preferred: bool,
    pub timing: TimingDescriptor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CtaDataBlock {
    pub tag_code: u8,
    pub extended_tag: Option<u8>,
    pub payload_len: usize,
    pub video_modes: Vec<CtaVideoDescriptor>,
}

impl CtaDataBlock {
    pub fn label(&self) -> String {
        match (self.tag_code, self.extended_tag) {
            (1, _) => "Audio".to_string(),
            (2, _) => "Video".to_string(),
            (3, _) => "Vendor".to_string(),
            (4, _) => "Speaker".to_string(),
            (5, _) => "VESA DTC".to_string(),
            (7, Some(0)) => "Video Capability".to_string(),
            (7, Some(1)) => "Vendor Video".to_string(),
            (7, Some(2)) => "VESA Display".to_string(),
            (7, Some(3)) => "VESA Timing".to_string(),
            (7, Some(4)) => "HDMI Video".to_string(),
            (7, Some(5)) => "Colorimetry".to_string(),
            (7, Some(6)) => "HDR Static".to_string(),
            (7, Some(7)) => "HDR Dynamic".to_string(),
            (7, Some(13)) => "Video Preference".to_string(),
            (7, Some(14)) => "YCbCr 4:2:0 Video".to_string(),
            (7, Some(15)) => "YCbCr 4:2:0 Map".to_string(),
            (7, Some(tag)) => format!("Extended {tag}"),
            (tag, _) => format!("CTA tag {tag}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CtaVideoDescriptor {
    Known(CtaVideoMode),
    Unknown { vic: u16, native: bool },
}

impl CtaVideoDescriptor {
    pub fn label(&self) -> String {
        match self {
            Self::Known(mode) => mode.label(),
            Self::Unknown { vic, native } => {
                let native = if *native { " native" } else { "" };
                format!("VIC {vic}  [unmapped]{native}")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CtaVideoMode {
    pub vic: u16,
    pub native: bool,
    pub width: u16,
    pub height: u16,
    pub refresh_millihz: u32,
    pub interlaced: bool,
}

impl CtaVideoMode {
    pub fn refresh_hz(&self) -> f64 {
        f64::from(self.refresh_millihz) / 1000.0
    }

    pub fn label(&self) -> String {
        let refresh = self.refresh_hz();
        let scan = if self.interlaced { "i" } else { "p" };
        let native = if self.native { " native" } else { "" };
        format!(
            "VIC {}  {}x{}{}@{refresh:.2}{native}",
            self.vic, self.width, self.height, scan
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EstablishedTiming {
    pub width: u16,
    pub height: u16,
    pub refresh_hz: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardTiming {
    pub slot: usize,
    pub width: u16,
    pub height: u16,
    pub refresh_hz: u16,
    pub aspect: StandardTimingAspect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardTimingAspect {
    SixteenTen,
    FourThree,
    FiveFour,
    SixteenNine,
}

impl StandardTimingAspect {
    pub fn label(self) -> &'static str {
        match self {
            Self::SixteenTen => "16:10",
            Self::FourThree => "4:3",
            Self::FiveFour => "5:4",
            Self::SixteenNine => "16:9",
        }
    }

    pub fn from_dimensions(width: u16, height: u16) -> Option<Self> {
        [
            (Self::SixteenTen, 10, 16),
            (Self::FourThree, 3, 4),
            (Self::FiveFour, 4, 5),
            (Self::SixteenNine, 9, 16),
        ]
        .into_iter()
        .find_map(|(aspect, height_ratio, width_ratio)| {
            (u32::from(width) * height_ratio == u32::from(height) * width_ratio).then_some(aspect)
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimingDescriptor {
    pub pixel_clock_khz: u32,
    pub h_active: u16,
    pub h_blanking: u16,
    pub h_front_porch: u16,
    pub h_sync_width: u16,
    pub h_back_porch: u16,
    pub v_active: u16,
    pub v_blanking: u16,
    pub v_front_porch: u16,
    pub v_sync_width: u16,
    pub v_back_porch: u16,
    pub h_sync_positive: bool,
    pub v_sync_positive: bool,
    pub interlaced: bool,
}

impl TimingDescriptor {
    pub fn h_total(&self) -> u16 {
        self.h_active.saturating_add(self.h_blanking)
    }

    pub fn v_total(&self) -> u16 {
        self.v_active.saturating_add(self.v_blanking)
    }

    pub fn horizontal_rate_khz(&self) -> Option<f64> {
        let h_total = self.h_total();
        if h_total == 0 {
            return None;
        }

        Some(f64::from(self.pixel_clock_khz) / f64::from(h_total))
    }

    pub fn refresh_hz(&self) -> Option<f64> {
        let h_total = u32::from(self.h_total());
        let v_total = u32::from(self.v_total());
        if h_total == 0 || v_total == 0 {
            return None;
        }

        Some((f64::from(self.pixel_clock_khz) * 1000.0) / f64::from(h_total * v_total))
    }

    pub fn hyprland_mode(&self) -> String {
        let refresh = self.refresh_hz().unwrap_or_default();
        format!("{}x{}@{refresh:.2}", self.h_active, self.v_active)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportPlan {
    pub connector: String,
    pub edid_file_name: String,
    pub hyprland_mode: String,
    pub hyprland_rule: String,
    pub position: String,
    pub scale: String,
}

impl ExportPlan {
    pub fn drm_kernel_parameter(&self) -> String {
        format!(
            "drm.edid_firmware={}:edid/{}",
            self.connector, self.edid_file_name
        )
    }

    pub fn hyprland_monitor_rule(&self) -> String {
        self.hyprland_rule.clone()
    }
}
