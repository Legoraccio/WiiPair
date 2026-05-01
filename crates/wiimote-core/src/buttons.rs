use bitflags::bitflags;

bitflags! {
    /// Core Wiimote buttons. The byte layout is little-endian relative to the
    /// 2-byte field present in nearly every input report.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
    pub struct Buttons: u16 {
        const LEFT  = 0x0001;
        const RIGHT = 0x0002;
        const DOWN  = 0x0004;
        const UP    = 0x0008;
        const PLUS  = 0x0010;
        const TWO   = 0x0100;
        const ONE   = 0x0200;
        const B     = 0x0400;
        const A     = 0x0800;
        const MINUS = 0x1000;
        const HOME  = 0x8000;
    }
}

impl Buttons {
    /// Parse the 2-byte button field as it appears in input reports
    /// (bytes immediately after the report ID).
    pub fn parse(byte0: u8, byte1: u8) -> Self {
        Self::from_bits_truncate(u16::from_le_bytes([byte0, byte1]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_and_home() {
        // A (byte1 bit 3) + HOME (byte1 bit 7) = 0x88 in the second byte.
        let b = Buttons::parse(0x00, 0x88);
        assert!(b.contains(Buttons::A));
        assert!(b.contains(Buttons::HOME));
        assert!(!b.contains(Buttons::B));
    }

    #[test]
    fn dpad() {
        assert!(Buttons::parse(0x08, 0x00).contains(Buttons::UP));
        assert!(Buttons::parse(0x04, 0x00).contains(Buttons::DOWN));
        assert!(Buttons::parse(0x01, 0x00).contains(Buttons::LEFT));
        assert!(Buttons::parse(0x02, 0x00).contains(Buttons::RIGHT));
    }
}
