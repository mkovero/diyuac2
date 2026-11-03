use imxrt_rt::{Family, Memory, RuntimeBuilder};

/// MIMXRT1020-EVK: IS25LP064A, 8 MiB QSPI NOR.
const FLASH_SIZE: usize = 8 * 1024 * 1024;

fn main() {
    RuntimeBuilder::from_flexspi(Family::Imxrt1020, FLASH_SIZE)
        // USB queue heads, dTDs and buffers are statics: keep them in DTCM,
        // which the USB DMA reaches and which is never cached.
        .data(Memory::Dtcm)
        .bss(Memory::Dtcm)
        .build()
        .unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
