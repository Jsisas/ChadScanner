//! Embeds `assets/icon-*.png` as the exe icon. Writes the `.res` file directly (the format is
//! small), so no resource compiler (rc.exe / llvm-rc) is needed; the linker takes `.res` as input.
use std::path::PathBuf;

const SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];
const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;

fn entry(out: &mut Vec<u8>, kind: u16, id: u16, flags: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes()); // header size
    for v in [0xFFFF, kind, 0xFFFF, id] {
        out.extend_from_slice(&u16::to_le_bytes(v));
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // data version
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&0x0409u16.to_le_bytes()); // en-US
    out.extend_from_slice(&0u32.to_le_bytes()); // version
    out.extend_from_slice(&0u32.to_le_bytes()); // characteristics
    out.extend_from_slice(data);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

fn main() {
    for n in SIZES {
        println!("cargo:rerun-if-changed=assets/icon-{n}.png");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = Vec::new();
    // A .res file starts with an empty entry.
    entry(&mut res, 0, 0, 0, &[]);
    let mut group = vec![0, 0, 1, 0];
    group.extend_from_slice(&(SIZES.len() as u16).to_le_bytes());
    for (i, n) in SIZES.into_iter().enumerate() {
        // PNG-compressed icon images are valid in icon resources (Windows Vista and later).
        let png = std::fs::read(format!("assets/icon-{n}.png")).expect("icon png");
        let id = i as u16 + 1;
        entry(&mut res, RT_ICON, id, 0x1010, &png);
        let side = if n >= 256 { 0 } else { n as u8 };
        group.extend_from_slice(&[side, side, 0, 0]);
        group.extend_from_slice(&1u16.to_le_bytes()); // planes
        group.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        group.extend_from_slice(&(png.len() as u32).to_le_bytes());
        group.extend_from_slice(&id.to_le_bytes());
    }
    entry(&mut res, RT_GROUP_ICON, 1, 0x1030, &group);
    let path = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("icon.res");
    std::fs::write(&path, res).expect("write icon.res");
    println!("cargo:rustc-link-arg-bins={}", path.display());
}
