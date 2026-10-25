//! A monitor's physical size from its EDID (the description every
//! monitor gives the PC), so a spanned picture can be drawn the same
//! physical size on monitors with different pixel sizes.

/// Every EDID starts with these bytes.
const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
/// Where the first detailed timing descriptor starts.
const TIMING: usize = 54;
/// Offsets in a timing descriptor: pixel clock (2 bytes, 0 means it's
/// not a timing), width and height in mm (low 8 bits each), then their
/// high 4 bits.
const CLOCK: usize = 0;
/// See [`CLOCK`].
const WIDTH_LOW: usize = 12;
/// See [`CLOCK`].
const HEIGHT_LOW: usize = 13;
/// See [`CLOCK`].
const HIGH_BITS: usize = 14;
/// Where the basic size in centimetres is (width, then height).
const BASIC_SIZE: usize = 21;
/// Millimetres per centimetre.
const MM_PER_CM: u32 = 10;
/// Where Windows keeps what it knows about each device.
const ENUM_KEY: &str = r"SYSTEM\CurrentControlSet\Enum";
/// The subkey of a monitor's key holding its `EDID` value.
const PARAMETERS: &str = "Device Parameters";

/// The screen's width and height in millimetres, from the first
/// detailed timing (to the millimetre), else the basic size (to the
/// centimetre). `None` if the data isn't an EDID or gives no size.
pub fn size_mm(edid: &[u8]) -> Option<(u32, u32)> {
    if edid.get(..HEADER.len())? != HEADER {
        return None;
    }
    timing_size(edid).or_else(|| basic_size(edid))
}

/// The registry key (under `HKEY_LOCAL_MACHINE`) holding the `EDID`
/// of the monitor whose device interface name is `interface`, such as
/// `\\?\DISPLAY#GSM774F#5&23aa1edc&0&UID45314#{e6f07b5f-...}`.
pub fn registry_key(interface: &str) -> Option<String> {
    let path = interface.strip_prefix(r"\\?\")?;
    let mut parts = path.split('#');
    let (class, model, instance) =
        (parts.next()?, parts.next()?, parts.next()?);
    if [class, model, instance].iter().any(|p| p.is_empty()) {
        return None;
    }
    Some(format!(
        r"{ENUM_KEY}\{class}\{model}\{instance}\{PARAMETERS}"
    ))
}

/// The size in the first detailed timing descriptor, if it has one.
fn timing_size(edid: &[u8]) -> Option<(u32, u32)> {
    let at = |offset: usize| {
        TIMING
            .checked_add(offset)
            .and_then(|i| edid.get(i))
            .copied()
            .map(u32::from)
    };
    let clock = at(CLOCK)? | at(CLOCK.checked_add(1)?)?;
    if clock == 0 {
        return None;
    }
    let high = at(HIGH_BITS)?;
    let width = at(WIDTH_LOW)? | ((high >> 4) << 8);
    let height = at(HEIGHT_LOW)? | ((high & 0x0F) << 8);
    (width > 0 && height > 0).then_some((width, height))
}

/// The basic size, in whole centimetres, as millimetres.
fn basic_size(edid: &[u8]) -> Option<(u32, u32)> {
    let width = u32::from(*edid.get(BASIC_SIZE)?);
    let height = u32::from(*edid.get(BASIC_SIZE.checked_add(1)?)?);
    (width > 0 && height > 0).then(|| {
        (
            width.saturating_mul(MM_PER_CM),
            height.saturating_mul(MM_PER_CM),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{HEADER, size_mm};

    /// An EDID with the basic size `cm` and, if given, a first timing
    /// with the size `mm`.
    fn edid(cm: (u8, u8), mm: Option<(u16, u16)>) -> Vec<u8> {
        let mut data = vec![0_u8; 128];
        data[..8].copy_from_slice(&HEADER);
        data[21] = cm.0;
        data[22] = cm.1;
        if let Some((w, h)) = mm {
            let byte = |v: u16| u8::try_from(v).unwrap();
            data[54] = 0x01;
            data[66] = byte(w & 0xFF);
            data[67] = byte(h & 0xFF);
            data[68] = (byte(w >> 8) << 4) | byte(h >> 8);
        }
        data
    }

    #[test]
    fn reads_the_timing_size_to_the_millimetre() {
        assert_eq!(
            size_mm(&edid((70, 40), Some((698, 393)))),
            Some((698, 393))
        );
    }

    #[test]
    fn falls_back_to_centimetres() {
        assert_eq!(size_mm(&edid((80, 33), None)), Some((800, 330)));
    }

    #[test]
    fn finds_the_registry_key() {
        let name = r"\\?\DISPLAY#GSM774F#5&23aa1edc&0&UID45314#{e6f07b5f-ee97}";
        assert_eq!(
            super::registry_key(name).as_deref(),
            Some(concat!(
                r"SYSTEM\CurrentControlSet\Enum\DISPLAY\GSM774F",
                r"\5&23aa1edc&0&UID45314\Device Parameters"
            ))
        );
        assert_eq!(super::registry_key("DISPLAY#A#B"), None);
        assert_eq!(super::registry_key(r"\\?\DISPLAY#A"), None);
    }

    #[test]
    fn rejects_junk_and_unknown_sizes() {
        assert_eq!(size_mm(&[0; 128]), None);
        assert_eq!(size_mm(&edid((0, 0), None)), None);
        assert_eq!(size_mm(&HEADER), None);
    }
}
