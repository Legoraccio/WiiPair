//! Pure axis-mapping helpers shared between the Windows ViGEm and Linux
//! uinput backends. All math is in `i16` (XInput-native range); Linux
//! callers widen to `i32` at the call site since uinput axes are i32.

/// Wiimote accelerometer is centred near 512 on X/Y when held flat.
pub const ACCEL_CENTER: i32 = 512;
/// Within ±DEADZONE of the center we treat the axis as neutral. Keeps a
/// resting Wiimote from drifting the virtual stick.
pub const ACCEL_DEADZONE: i32 = 30;
/// Approximate accelerometer deflection at 45° of tilt — full stick at
/// that angle.
pub const ACCEL_RANGE: i32 = 220;

/// Nunchuk analog stick rests around 128 on each axis (8-bit raw).
pub const STICK_CENTER: i32 = 128;
/// Dead-zone applied to the Nunchuk stick — Nintendo's part has a
/// generous mechanical neutral that benefits from being silenced.
pub const STICK_DEADZONE: i32 = 8;
/// Effective stick travel either side of `STICK_CENTER`. The raw range
/// is [0..=255] but real units rarely sweep past ~30..=225, so the
/// scaled stick saturates a few px short of the mechanical extremes —
/// good enough for full deflection in every game we tested.
pub const STICK_RANGE: i32 = 95;

/// Map a Nunchuk stick byte (0..=255, 128 ≈ neutral) to a virtual
/// stick position in `i16`. Same shape as [`tilt_to_stick`]: dead-zone
/// near the centre, then linear scaling to ±`i16::MAX` (with the
/// asymmetric `i16::MIN+1` clamp on the negative side so the value
/// never sits below the uinput-declared minimum).
pub fn nunchuk_stick_to_axis(raw: u8) -> i16 {
    let delta = i32::from(raw) - STICK_CENTER;
    if delta.abs() < STICK_DEADZONE {
        return 0;
    }
    let signed = if delta > 0 {
        delta - STICK_DEADZONE
    } else {
        delta + STICK_DEADZONE
    };
    let span = (STICK_RANGE - STICK_DEADZONE).max(1);
    let scaled = (signed * i32::from(i16::MAX)) / span;
    scaled.clamp(i32::from(i16::MIN) + 1, i32::from(i16::MAX)) as i16
}

/// Map a 0..=31 whammy reading to a symmetric `i16` axis range
/// (`-32767`..=`32767`). Symmetry matters because the Linux uinput
/// backend declares the abs-axis range as `[-ABS_RANGE, +ABS_RANGE]`
/// (with `ABS_RANGE = 32767`); a `-32768` would sit one LSB below the
/// declared minimum. XInput on Windows accepts the same range without
/// loss — losing the very last LSB on the negative side is
/// imperceptible.
///
/// Inputs out of range are clamped — keeps the formula safe even if
/// the upstream parser ever lets a stray bit through.
pub fn whammy_to_axis(w: u8) -> i16 {
    let w = i32::from(w.min(31));
    // 0..=31 → -32767..=+32767 (multiplier 65534 = 2 * 32767).
    (w * 65534 / 31 - 32767) as i16
}

/// Map an accelerometer axis (raw 10-bit reading) to a virtual stick
/// position in `i16`. Applies a center offset, dead-zone, and linear
/// scaling up to ±[`ACCEL_RANGE`].
///
/// Returned value is in `i16::MIN+1..=i16::MAX` (the asymmetric clamp
/// keeps the absolute value bounded by `i16::MAX`, avoiding sign
/// surprises on platforms that round-trip through `abs`).
pub fn tilt_to_stick(raw_axis: i32) -> i16 {
    let delta = raw_axis - ACCEL_CENTER;
    if delta.abs() < ACCEL_DEADZONE {
        return 0;
    }
    let signed = if delta > 0 {
        delta - ACCEL_DEADZONE
    } else {
        delta + ACCEL_DEADZONE
    };
    let span = (ACCEL_RANGE - ACCEL_DEADZONE).max(1);
    let scaled = (signed * i32::from(i16::MAX)) / span;
    scaled.clamp(i32::from(i16::MIN) + 1, i32::from(i16::MAX)) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whammy_released_is_min() {
        // Symmetric range: `-i16::MAX` (one above `i16::MIN`) so the
        // value never sits below the uinput-declared abs range minimum.
        assert_eq!(whammy_to_axis(0), -32767);
    }

    #[test]
    fn whammy_fully_pressed_is_max() {
        assert_eq!(whammy_to_axis(31), 32767);
    }

    #[test]
    fn whammy_clamps_overflow() {
        // Out-of-range input must not produce out-of-range output.
        assert_eq!(whammy_to_axis(255), 32767);
    }

    #[test]
    fn whammy_range_is_symmetric() {
        // |min| == max — required by the symmetric uinput abs range.
        assert_eq!(whammy_to_axis(0), -whammy_to_axis(31));
    }

    #[test]
    fn whammy_midpoint_near_zero() {
        // 16/31 ≈ 0.516 → near zero but slightly positive.
        let v = whammy_to_axis(16);
        assert!((-1000..=2000).contains(&v), "got {v}");
    }

    #[test]
    fn tilt_resting_is_neutral() {
        assert_eq!(tilt_to_stick(ACCEL_CENTER), 0);
        // Within the dead-zone, still neutral.
        assert_eq!(tilt_to_stick(ACCEL_CENTER + ACCEL_DEADZONE - 1), 0);
        assert_eq!(tilt_to_stick(ACCEL_CENTER - ACCEL_DEADZONE + 1), 0);
    }

    #[test]
    fn tilt_full_deflection_saturates() {
        // Beyond ACCEL_RANGE on the positive side should hit i16::MAX.
        assert_eq!(tilt_to_stick(ACCEL_CENTER + ACCEL_RANGE * 2), i16::MAX);
        // Beyond on the negative side should hit i16::MIN+1 (asymmetric clamp).
        assert_eq!(tilt_to_stick(ACCEL_CENTER - ACCEL_RANGE * 2), i16::MIN + 1);
    }

    #[test]
    fn tilt_just_past_deadzone_is_small() {
        let v = tilt_to_stick(ACCEL_CENTER + ACCEL_DEADZONE + 1);
        assert!(v > 0 && v < 1000, "got {v}");
    }

    #[test]
    fn nunchuk_stick_neutral_inside_deadzone() {
        assert_eq!(nunchuk_stick_to_axis(128), 0);
        assert_eq!(nunchuk_stick_to_axis(132), 0);
        assert_eq!(nunchuk_stick_to_axis(124), 0);
    }

    #[test]
    fn nunchuk_stick_full_deflection_saturates() {
        // u8 saturates at 255 / 0; both should hit (a clamped) ±i16::MAX.
        assert_eq!(nunchuk_stick_to_axis(255), i16::MAX);
        assert_eq!(nunchuk_stick_to_axis(0), i16::MIN + 1);
    }

    #[test]
    fn nunchuk_stick_just_past_deadzone_is_small() {
        let v = nunchuk_stick_to_axis(128 + STICK_DEADZONE as u8 + 1);
        assert!(v > 0 && v < 5000, "got {v}");
    }
}
