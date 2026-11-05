use imxrt_rt::{Family, Memory, RuntimeBuilder};

/// Flash partition of the app: after the DFU bootloader (`boot/`) and the
/// app info sector. Keep in sync with `common/src/layout.rs`.
const APP_OFFSET: u32 = 0x4_0000;
const APP_MAX: usize = 0x10_0000;

fn main() {
    RuntimeBuilder::in_flash(Family::Imxrt1020, APP_MAX, APP_OFFSET)
        // USB queue heads, dTDs and buffers are statics: keep them in DTCM,
        // which the USB DMA reaches and which is never cached.
        .data(Memory::Dtcm)
        .bss(Memory::Dtcm)
        .build()
        .unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
