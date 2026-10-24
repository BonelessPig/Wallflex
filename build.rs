//! Embeds `wallflex.manifest` (per-monitor DPI awareness and the supported
//! Windows versions) and the icon, `wallflex.ico`, into the exe when
//! building with the MSVC linker.
//!
//! The icon goes in through a `.res` file written here (the linker takes
//! those directly), so no resource compiler or extra crate is needed.

use std::error::Error;
use std::path::Path;

/// `RT_ICON`: one image of an icon.
const RT_ICON: u16 = 3;
/// `RT_GROUP_ICON`: the list of an icon's images.
const RT_GROUP_ICON: u16 = 14;
/// The icon group's id; `src/win32/tray.rs` loads the same one.
const ICON_ID: u16 = 1;
/// `MOVEABLE | DISCARDABLE`, as resource compilers mark icon images.
const IMAGE_FLAGS: u16 = 0x1010;
/// `MOVEABLE | PURE | DISCARDABLE`, as they mark icon groups.
const GROUP_FLAGS: u16 = 0x1030;
/// `LANG_NEUTRAL`.
const LANGUAGE: u16 = 0;
/// Bytes in a resource header with numeric type and name.
const HEADER_BYTES: u32 = 32;
/// Bytes in the icon file's `ICONDIR` header.
const DIR_BYTES: usize = 6;
/// Bytes in each `ICONDIRENTRY`.
const ENTRY_BYTES: usize = 16;
/// Resource data is padded to a multiple of this.
const ALIGN: usize = 4;

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=wallflex.manifest");
    println!("cargo:rerun-if-changed=wallflex.ico");

    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env != "msvc" {
        return Ok(());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = root.join("wallflex.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );

    let icon = std::fs::read(root.join("wallflex.ico"))?;
    let res = Path::new(&std::env::var("OUT_DIR")?).join("wallflex.res");
    std::fs::write(&res, icon_resources(&icon).ok_or("bad wallflex.ico")?)?;
    println!("cargo:rustc-link-arg-bins={}", res.display());
    Ok(())
}

/// A `.res` file holding the icon file's images and their group.
/// `None` if the icon file is malformed.
fn icon_resources(icon: &[u8]) -> Option<Vec<u8>> {
    let count = u16::from_le_bytes(icon.get(4..DIR_BYTES)?.try_into().ok()?);
    let entries_end = usize::from(count)
        .checked_mul(ENTRY_BYTES)?
        .checked_add(DIR_BYTES)?;
    let entries = icon.get(DIR_BYTES..entries_end)?;

    // A .res file starts with an empty resource.
    let mut out = Vec::new();
    push_resource(&mut out, 0, 0, 0, &[])?;
    // The group: the icon's directory, with resource ids for offsets.
    let mut group = icon.get(..DIR_BYTES)?.to_vec();
    for (entry, id) in entries.as_chunks::<ENTRY_BYTES>().0.iter().zip(1_u16..)
    {
        let length = u32::from_le_bytes(entry.get(8..12)?.try_into().ok()?);
        let offset = u32::from_le_bytes(entry.get(12..16)?.try_into().ok()?);
        let start = usize::try_from(offset).ok()?;
        let end = start.checked_add(usize::try_from(length).ok()?)?;
        let image = icon.get(start..end)?;
        push_resource(&mut out, RT_ICON, id, IMAGE_FLAGS, image)?;
        // Size, colours, planes, bit count and length stay; the offset
        // becomes the image's id.
        group.extend_from_slice(entry.get(..12)?);
        group.extend_from_slice(&id.to_le_bytes());
    }
    push_resource(&mut out, RT_GROUP_ICON, ICON_ID, GROUP_FLAGS, &group)?;
    Some(out)
}

/// Appends one resource with a numeric type and name, padded to 4 bytes.
/// `None` if the data is too long for a resource.
fn push_resource(
    out: &mut Vec<u8>,
    kind: u16,
    name: u16,
    flags: u16,
    data: &[u8],
) -> Option<()> {
    /// Marks the next `u16` as a numeric id rather than a string.
    const NUMERIC: u16 = 0xFFFF;
    let length = u32::try_from(data.len()).ok()?;
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(&HEADER_BYTES.to_le_bytes());
    for half in [NUMERIC, kind, NUMERIC, name] {
        out.extend_from_slice(&half.to_le_bytes());
    }
    out.extend_from_slice(&0_u32.to_le_bytes()); // data version
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&LANGUAGE.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes()); // version
    out.extend_from_slice(&0_u32.to_le_bytes()); // characteristics
    out.extend_from_slice(data);
    while !out.len().is_multiple_of(ALIGN) {
        out.push(0);
    }
    Some(())
}
