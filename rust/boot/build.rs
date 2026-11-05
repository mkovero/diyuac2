use imxrt_rt::{Family, Memory, RuntimeBuilder};

/// The bootloader's share of the flash: FCB, IVT and image, up to the app
/// info sector. Keep in sync with `common/src/layout.rs` (BOOT_SIZE).
const BOOT_SIZE: usize = 0x3_F000;

fn main() {
    RuntimeBuilder::from_flexspi(Family::Imxrt1020, BOOT_SIZE)
        // USB queue heads, dTDs and the EP0 buffer are statics: DTCM, which
        // the USB DMA reaches and which is never cached.
        .data(Memory::Dtcm)
        .bss(Memory::Dtcm)
        .build()
        .unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
