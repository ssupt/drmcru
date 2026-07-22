use crate::edid::{encode_detailed_timing, encode_standard_timing, parse_edid};
use crate::models::{
    ConnectorStatus, HyprlandMonitor, Monitor, StandardTiming, StandardTimingAspect,
    TimingDescriptor,
};
use anyhow::Result;

const BLOCK_LEN: usize = 128;
const DTD_LEN: usize = 18;

pub fn monitors() -> Result<Vec<Monitor>> {
    Ok(vec![Monitor {
        connector: "DP-1".to_string(),
        drm_path: None,
        status: ConnectorStatus::Connected,
        hyprland: Some(HyprlandMonitor {
            id: Some(1),
            name: "DP-1".to_string(),
            description: "Example Display".to_string(),
            make: Some("Demo".to_string()),
            model: Some("Reference Panel".to_string()),
            serial: None,
            active_width: Some(2560),
            active_height: Some(1440),
            refresh_hz: Some(144.0),
            x: Some(0),
            y: Some(0),
            scale: Some(1.0),
            available_modes: vec![
                "2560x1440@144.00Hz".to_string(),
                "2560x1440@120.00Hz".to_string(),
                "1920x1080@120.00Hz".to_string(),
                "1920x1080@60.00Hz".to_string(),
            ],
            focused: true,
        }),
        edid: Some(parse_edid(sample_edid()?)?),
    }])
}

fn sample_edid() -> Result<Vec<u8>> {
    let mut raw = vec![0u8; BLOCK_LEN * 2];
    let base = &mut raw[..BLOCK_LEN];
    base[..8].copy_from_slice(&[0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00]);
    base[8..10].copy_from_slice(&manufacturer_code("DEM").to_be_bytes());
    base[10..12].copy_from_slice(&1u16.to_le_bytes());
    base[16] = 1;
    base[17] = 36;
    base[18] = 1;
    base[19] = 4;
    base[20] = 0xa5;
    base[21] = 60;
    base[22] = 34;
    base[23] = 120;
    base[24] = 0x0a;
    base[35] = 1 << 5;
    base[36] = 1 << 3;
    base[38..54].fill(0x01);

    for timing in [
        StandardTiming {
            slot: 0,
            width: 1920,
            height: 1080,
            refresh_hz: 60,
            aspect: StandardTimingAspect::SixteenNine,
        },
        StandardTiming {
            slot: 1,
            width: 1280,
            height: 720,
            refresh_hz: 60,
            aspect: StandardTimingAspect::SixteenNine,
        },
    ] {
        let encoded = encode_standard_timing(&timing)?;
        let offset = 38 + timing.slot * 2;
        base[offset..offset + 2].copy_from_slice(&encoded);
    }

    for (slot, timing) in [
        timing_1440p(144.0),
        timing_1440p(120.0),
        timing_1080p(120.0),
    ]
    .iter()
    .enumerate()
    {
        let offset = 54 + slot * DTD_LEN;
        base[offset..offset + DTD_LEN].copy_from_slice(&encode_detailed_timing(timing)?);
    }
    write_name_descriptor(&mut base[108..126], "Example Display");
    base[126] = 1;
    repair_checksum(base);

    let cta = &mut raw[BLOCK_LEN..];
    cta[0] = 0x02;
    cta[1] = 3;
    cta[2] = 9;
    cta[4] = (2 << 5) | 4;
    cta[5..9].copy_from_slice(&[0x80 | 16, 4, 64, 97]);
    cta[9..9 + DTD_LEN].copy_from_slice(&encode_detailed_timing(&timing_1080p(60.0))?);
    repair_checksum(cta);

    Ok(raw)
}

fn timing_1440p(refresh_hz: f64) -> TimingDescriptor {
    timing(2560, 1440, 48, 32, 80, 3, 5, 33, refresh_hz)
}

fn timing_1080p(refresh_hz: f64) -> TimingDescriptor {
    timing(1920, 1080, 48, 32, 80, 3, 5, 23, refresh_hz)
}

#[allow(clippy::too_many_arguments)]
fn timing(
    width: u16,
    height: u16,
    h_front: u16,
    h_sync: u16,
    h_back: u16,
    v_front: u16,
    v_sync: u16,
    v_back: u16,
    refresh_hz: f64,
) -> TimingDescriptor {
    let h_blanking = h_front + h_sync + h_back;
    let v_blanking = v_front + v_sync + v_back;
    let pixel_clock_khz =
        (f64::from(width + h_blanking) * f64::from(height + v_blanking) * refresh_hz / 1000.0)
            .round() as u32;
    TimingDescriptor {
        pixel_clock_khz,
        h_active: width,
        h_blanking,
        h_front_porch: h_front,
        h_sync_width: h_sync,
        h_back_porch: h_back,
        v_active: height,
        v_blanking,
        v_front_porch: v_front,
        v_sync_width: v_sync,
        v_back_porch: v_back,
        h_sync_positive: true,
        v_sync_positive: false,
        interlaced: false,
    }
}

fn manufacturer_code(value: &str) -> u16 {
    value.bytes().fold(0u16, |code, byte| {
        code << 5 | u16::from(byte.saturating_sub(b'A') + 1)
    })
}

fn write_name_descriptor(descriptor: &mut [u8], name: &str) {
    descriptor.fill(0);
    descriptor[3] = 0xfc;
    descriptor[4] = 0;
    let bytes = name.as_bytes();
    let len = bytes.len().min(13);
    descriptor[5..5 + len].copy_from_slice(&bytes[..len]);
    if len < 13 {
        descriptor[5 + len] = b'\n';
    }
}

fn repair_checksum(block: &mut [u8]) {
    block[127] = 0;
    let sum = block[..127]
        .iter()
        .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
    block[127] = 0u8.wrapping_sub(sum);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_monitor_is_synthetic_and_checksum_valid() {
        let monitors = monitors().unwrap();
        let monitor = &monitors[0];
        let edid = monitor.edid.as_ref().unwrap();

        assert_eq!(monitor.label(), "DP-1 - Example Display");
        assert!(monitor.hyprland.as_ref().unwrap().serial.is_none());
        assert!(edid.checksum_valid);
        assert!(edid.cta_blocks[0].checksum_valid);
        assert_eq!(edid.monitor_name.as_deref(), Some("Example Displ"));
    }
}
