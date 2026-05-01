use crate::buttons::Buttons;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReportError {
    #[error("report too short: got {got} bytes, need at least {need}")]
    TooShort { got: usize, need: usize },
    #[error("unknown or unsupported report id: 0x{0:02x}")]
    UnknownId(u8),
}

#[derive(Debug, Clone, Copy)]
pub struct StatusFlags {
    pub battery_low: bool,
    pub extension_connected: bool,
    pub speaker_enabled: bool,
    pub ir_enabled: bool,
    pub leds: u8,
}

/// 10-bit raw accelerometer reading per axis, range 0..=1023.
/// At rest with the Wiimote flat (buttons up), z ≈ 612, x ≈ y ≈ 512.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Accelerometer {
    pub x: u16,
    pub y: u16,
    pub z: u16,
}

/// One IR camera dot. The camera emits 4 of these per report.
/// X spans 0..=1023, Y spans 0..=767. When no dot is detected the
/// camera returns X=Y=0x3FF; we surface that as `visible = false`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IrDot {
    pub x: u16,
    pub y: u16,
    pub size: u8,
    pub visible: bool,
}

pub type IrDots = [IrDot; 4];

#[derive(Debug, Clone)]
pub enum InputReport {
    /// 0x20: status report — battery, LEDs, extension presence.
    Status {
        buttons: Buttons,
        battery: u8,
        flags: StatusFlags,
    },
    /// 0x21: read response from EEPROM/registers.
    ReadResponse {
        buttons: Buttons,
        /// Low nibble of byte 3: 0 on success, non-zero error code.
        error: u8,
        /// (high nibble of byte 3) + 1: number of valid bytes in `data`.
        size: usize,
        /// Low 16 bits of the original read address.
        address: u16,
        /// Always 16 bytes; only the first `size` are meaningful.
        data: [u8; 16],
    },
    /// 0x22: ack for an output report. `report_id` is the report we
    /// wrote (e.g. 0x16 for register write); `error == 0` means success.
    Ack {
        buttons: Buttons,
        report_id: u8,
        error: u8,
    },
    /// 0x30: core buttons only.
    Buttons { buttons: Buttons },
    /// 0x31: buttons + 10-bit accelerometer.
    ButtonsAccel {
        buttons: Buttons,
        accel: Accelerometer,
    },
    /// 0x33: buttons + accel + 12-byte extended IR (4 dots).
    ButtonsAccelIr {
        buttons: Buttons,
        accel: Accelerometer,
        ir: IrDots,
    },
}

pub fn parse_input(buf: &[u8]) -> Result<InputReport, ReportError> {
    if buf.is_empty() {
        return Err(ReportError::TooShort { got: 0, need: 1 });
    }
    let id = buf[0];
    match id {
        0x20 => {
            need(buf, 7)?;
            let buttons = Buttons::parse(buf[1], buf[2]);
            let f = buf[3];
            Ok(InputReport::Status {
                buttons,
                battery: buf[6],
                flags: StatusFlags {
                    battery_low: f & 0x01 != 0,
                    extension_connected: f & 0x02 != 0,
                    speaker_enabled: f & 0x04 != 0,
                    ir_enabled: f & 0x08 != 0,
                    leds: (f >> 4) & 0x0F,
                },
            })
        }
        0x21 => {
            need(buf, 22)?;
            let buttons = Buttons::parse(buf[1], buf[2]);
            let size_err = buf[3];
            let size = ((size_err >> 4) as usize) + 1;
            let error = size_err & 0x0F;
            let address = u16::from_be_bytes([buf[4], buf[5]]);
            let mut data = [0u8; 16];
            data.copy_from_slice(&buf[6..22]);
            Ok(InputReport::ReadResponse {
                buttons,
                error,
                size,
                address,
                data,
            })
        }
        0x22 => {
            need(buf, 5)?;
            Ok(InputReport::Ack {
                buttons: Buttons::parse(buf[1], buf[2]),
                report_id: buf[3],
                error: buf[4],
            })
        }
        0x30 => {
            need(buf, 3)?;
            Ok(InputReport::Buttons {
                buttons: Buttons::parse(buf[1], buf[2]),
            })
        }
        0x31 => {
            need(buf, 6)?;
            let buttons = Buttons::parse(buf[1], buf[2]);
            let accel = parse_accel(buf[1], buf[2], buf[3], buf[4], buf[5]);
            Ok(InputReport::ButtonsAccel { buttons, accel })
        }
        0x33 => {
            need(buf, 18)?;
            let buttons = Buttons::parse(buf[1], buf[2]);
            let accel = parse_accel(buf[1], buf[2], buf[3], buf[4], buf[5]);
            let ir = parse_ir_extended(&buf[6..18]);
            Ok(InputReport::ButtonsAccelIr {
                buttons,
                accel,
                ir,
            })
        }
        other => Err(ReportError::UnknownId(other)),
    }
}

fn need(buf: &[u8], n: usize) -> Result<(), ReportError> {
    if buf.len() < n {
        Err(ReportError::TooShort {
            got: buf.len(),
            need: n,
        })
    } else {
        Ok(())
    }
}

/// Reassemble the 10-bit accel value from the high 8 bits in the
/// dedicated byte plus 2 LSBs stored in the (otherwise unused) bits
/// of the buttons bytes:
/// * X gets bits 5-6 of buttons[0]
/// * Y gets bit 5 of buttons[1] (only one LSB; the very lowest bit is dropped)
/// * Z gets bit 6 of buttons[1]
fn parse_accel(bb0: u8, bb1: u8, xh: u8, yh: u8, zh: u8) -> Accelerometer {
    let x = ((xh as u16) << 2) | (((bb0 >> 5) & 0x03) as u16);
    let y = ((yh as u16) << 2) | (((bb1 & 0x20) >> 4) as u16);
    let z = ((zh as u16) << 2) | (((bb1 & 0x40) >> 5) as u16);
    Accelerometer { x, y, z }
}

/// Decode 12 bytes of extended-mode IR data into 4 dots.
/// Per dot (3 bytes): `XX YY SY SX YY YY XX XX SS SS SS SS`
/// (low 8 X, low 8 Y, then 2-bit Y high, 2-bit X high, 4-bit size).
fn parse_ir_extended(buf: &[u8]) -> IrDots {
    debug_assert!(buf.len() >= 12);
    let mut dots = [IrDot::default(); 4];
    for (i, dot) in dots.iter_mut().enumerate() {
        let b0 = buf[i * 3] as u16;
        let b1 = buf[i * 3 + 1] as u16;
        let b2 = buf[i * 3 + 2] as u16;
        let x = ((b2 & 0x30) << 4) | b0;
        let y = ((b2 & 0xC0) << 2) | b1;
        let size = (b2 & 0x0F) as u8;
        let visible = !(x == 0x3FF && y == 0x3FF);
        *dot = IrDot {
            x,
            y,
            size,
            visible,
        };
    }
    dots
}

#[derive(Debug, Clone)]
pub enum OutputReport {
    /// 0x11: set player LEDs. `leds` is the low nibble (bit 0 = LED 1, …).
    SetLeds { leds: u8 },
    /// 0x12: set reporting mode. `mode` is the input report ID we want.
    SetReportingMode { continuous: bool, mode: u8 },
    /// 0x15: request a status report (will arrive as 0x20).
    RequestStatus,
    /// 0x16: write up to 16 bytes to a control register. The address is
    /// 24-bit; we always use address space 0x04 (control registers).
    WriteRegister { address: u32, data: Vec<u8> },
    /// 0x17: request a register read; the response arrives as 0x21.
    /// `count` is the number of bytes to read (max 16 per response).
    ReadRegister { address: u32, count: u16 },
}

impl OutputReport {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::SetLeds { leds } => vec![0x11, (leds & 0x0F) << 4],
            Self::SetReportingMode { continuous, mode } => {
                vec![0x12, if *continuous { 0x04 } else { 0x00 }, *mode]
            }
            Self::RequestStatus => vec![0x15, 0x00],
            Self::WriteRegister { address, data } => {
                let mut buf = vec![0u8; 22];
                buf[0] = 0x16;
                buf[1] = 0x04; // address space: control registers
                buf[2] = ((address >> 16) & 0xFF) as u8;
                buf[3] = ((address >> 8) & 0xFF) as u8;
                buf[4] = (address & 0xFF) as u8;
                let n = data.len().min(16);
                buf[5] = n as u8;
                buf[6..6 + n].copy_from_slice(&data[..n]);
                buf
            }
            Self::ReadRegister { address, count } => {
                vec![
                    0x17,
                    0x04,
                    ((address >> 16) & 0xFF) as u8,
                    ((address >> 8) & 0xFF) as u8,
                    (address & 0xFF) as u8,
                    ((count >> 8) & 0xFF) as u8,
                    (count & 0xFF) as u8,
                ]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_buttons_only_report() {
        let r = parse_input(&[0x30, 0x08, 0x00]).unwrap();
        match r {
            InputReport::Buttons { buttons } => assert!(buttons.contains(Buttons::UP)),
            _ => panic!("expected Buttons"),
        }
    }

    #[test]
    fn parses_buttons_accel_report() {
        // BB0=0x00 (no dpad/plus, no X LSBs), BB1=0x00 (no buttons, no Y/Z LSBs)
        // Accel X high = 0x80 (=128), Y high = 0x7F, Z high = 0x99.
        // Expected: x = 128 << 2 = 512; y = 127 << 2 = 508; z = 153 << 2 = 612.
        let r = parse_input(&[0x31, 0x00, 0x00, 0x80, 0x7F, 0x99]).unwrap();
        match r {
            InputReport::ButtonsAccel { accel, .. } => {
                assert_eq!(accel.x, 512);
                assert_eq!(accel.y, 508);
                assert_eq!(accel.z, 612);
            }
            _ => panic!("expected ButtonsAccel"),
        }
    }

    #[test]
    fn accel_lsbs_picked_from_buttons_bytes() {
        // BB0 bits 5-6 set = 0b0110_0000 = 0x60 ⇒ X LSBs = 0b11 = 3
        // BB1 bit 5 set    = 0b0010_0000 = 0x20 ⇒ Y bit 1 = 1
        // BB1 bit 6 set    = 0b0100_0000 = 0x40 ⇒ Z bit 1 = 1
        let r = parse_input(&[0x31, 0x60, 0x60, 0x00, 0x00, 0x00]).unwrap();
        match r {
            InputReport::ButtonsAccel { accel, .. } => {
                assert_eq!(accel.x, 0b11); // 3
                assert_eq!(accel.y, 0b10); // 2 (only the LSB+1, real LSB is dropped)
                assert_eq!(accel.z, 0b10);
            }
            _ => panic!("expected ButtonsAccel"),
        }
    }

    #[test]
    fn parses_ir_dot_visible_and_invisible() {
        let mut buf = vec![0x33, 0x00, 0x00, 0x00, 0x00, 0x00];
        // Dot 0: X=200, Y=300, size=4
        // X = 200, low 8 = 0xC8, high 2 = 0
        // Y = 300, low 8 = 0x2C, high 2 = 0x01 → b2 bit 6
        // size = 4
        let dot0 = [0xC8u8, 0x2C, (1 << 6) | 4];
        // Dot 1: invisible (0x3FF, 0x3FF)
        let dot1 = [0xFFu8, 0xFF, 0xFF];
        // Dots 2 & 3: zero
        buf.extend_from_slice(&dot0);
        buf.extend_from_slice(&dot1);
        buf.extend_from_slice(&[0; 6]);
        let r = parse_input(&buf).unwrap();
        match r {
            InputReport::ButtonsAccelIr { ir, .. } => {
                assert!(ir[0].visible);
                assert_eq!(ir[0].x, 200);
                assert_eq!(ir[0].y, 300);
                assert_eq!(ir[0].size, 4);
                assert!(!ir[1].visible);
            }
            _ => panic!("expected ButtonsAccelIr"),
        }
    }

    #[test]
    fn encodes_leds() {
        let r = OutputReport::SetLeds { leds: 0b0001 };
        assert_eq!(r.encode(), vec![0x11, 0b0001_0000]);
    }

    #[test]
    fn encodes_write_register_padded_to_22_bytes() {
        let r = OutputReport::WriteRegister {
            address: 0x00a4_00f0,
            data: vec![0x55],
        };
        let buf = r.encode();
        assert_eq!(buf.len(), 22);
        assert_eq!(buf[0], 0x16);
        assert_eq!(buf[1], 0x04);
        assert_eq!(&buf[2..5], &[0xa4, 0x00, 0xf0]);
        assert_eq!(buf[5], 1);
        assert_eq!(buf[6], 0x55);
        assert!(buf[7..].iter().all(|&b| b == 0));
    }

    #[test]
    fn encodes_read_register() {
        let r = OutputReport::ReadRegister {
            address: 0x00a4_00fa,
            count: 6,
        };
        assert_eq!(r.encode(), vec![0x17, 0x04, 0xa4, 0x00, 0xfa, 0x00, 0x06]);
    }

    #[test]
    fn parses_ack_for_write_register() {
        // 0x22 BB BB report_id error
        let r = parse_input(&[0x22, 0x00, 0x00, 0x16, 0x00]).unwrap();
        match r {
            InputReport::Ack {
                report_id, error, ..
            } => {
                assert_eq!(report_id, 0x16);
                assert_eq!(error, 0);
            }
            _ => panic!("expected Ack"),
        }
    }

    #[test]
    fn parses_read_response_with_extension_id() {
        let mut buf = vec![0x21, 0x00, 0x00];
        // size_err: 6 bytes (size-1 = 5 in high nibble) + error 0
        buf.push(5 << 4);
        // address (low 16 bits) = 0x00fa
        buf.push(0x00);
        buf.push(0xfa);
        // 6 bytes of data (Guitar ID) + 10 bytes padding
        buf.extend_from_slice(&[0x00, 0x00, 0xa4, 0x20, 0x01, 0x03]);
        buf.extend_from_slice(&[0; 10]);
        let r = parse_input(&buf).unwrap();
        match r {
            InputReport::ReadResponse {
                error,
                size,
                address,
                data,
                ..
            } => {
                assert_eq!(error, 0);
                assert_eq!(size, 6);
                assert_eq!(address, 0x00fa);
                assert_eq!(&data[..6], &[0x00, 0x00, 0xa4, 0x20, 0x01, 0x03]);
            }
            _ => panic!("expected ReadResponse"),
        }
    }
}
