use imxrt_rt::{Family, FlexRamBanks, Memory, RuntimeBuilder};

/// Flash partition of the app: after the DFU bootloader (`boot/`) and the
/// app info sector. Keep in sync with `common/src/layout.rs`.
const APP_OFFSET: u32 = 0x4_0000;
const APP_MAX: usize = 0x10_0000;

fn main() {
    RuntimeBuilder::in_flash(Family::Imxrt1020, APP_MAX, APP_OFFSET)
        // 8 x 32 KiB FlexRAM banks: code in 32 KiB ITCM, everything else
        // in 192 KiB DTCM (the two 64 KiB audio rings, USB buffers, stack);
        // OCRAM keeps read-only data only.
        .flexram_banks(FlexRamBanks { ocram: 1, itcm: 1, dtcm: 6 })
        // USB queue heads, dTDs and buffers are statics: keep them in DTCM,
        // which the USB DMA reaches and which is never cached.
        .data(Memory::Dtcm)
        .bss(Memory::Dtcm)
        .build()
        .unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
