//! Audio hardware for the i.MX RT1020 (MIMXRT1020-EVK): audio PLL, SAI1 as
//! I2S master, eDMA ring buffers, LPI2C1 for the codec.
//!
//! Port of `firmware/app/audio_hw_imxrt.c`. Register values were checked
//! against a register dump of the running C firmware (M3).
//!
//! Clocking: audio PLL (PLL4) 24 MHz * (32 + 768/1000) = 786.432 MHz ->
//! SAI1 clock root /4 /N = 256 fs, output on the SAI1_MCLK pad (codec
//! SYSCLK). SAI1 TX is the I2S master (BCLK = 64 fs, FS = fs), RX is
//! synchronous to TX. The audio clock comes from the 24 MHz crystal and is
//! independent of the USB host clock.
//!
//! Data: eDMA channel 0 loops over the TX ring into TDR0, channel 1 loops
//! from RDR0 into the RX ring, both forever (SLAST / DLAST rewind the
//! address at major loop end). A major-loop interrupt counts ring wraps;
//! the position within the ring comes from CITER. Rings are in DTCM (not
//! cached, DMA accessible).

use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m::peripheral::NVIC;
use imxrt_hal as hal;
use imxrt_ral as ral;

use hal::ccm;
use hal::iomuxc::{self, Config, DriveStrength, OpenDrain, PullKeeper, Speed};
use ral::Interrupt as interrupt;

pub const CHANNELS: usize = 2;
pub const RING_FRAMES: usize = 2048;
const RING_WORDS: usize = RING_FRAMES * CHANNELS;

const TX_CH: usize = 0;
const RX_CH: usize = 1;
/// DMAMUX request sources (RT1020 reference manual, DMA_CH_MUX table).
const DMAMUX_SAI1_RX: u32 = 19;
const DMAMUX_SAI1_TX: u32 = 20;

/// Audio PLL: 24 MHz * (32 + 768/1000) = 786.432 MHz.
const PLL4_DIV_SELECT: u32 = 32;
const PLL4_NUM: u32 = 768;
const PLL4_DENOM: u32 = 1000;
/// SAI1 root = 786.432 MHz / 4 / div = 256 * rate; div = 768000 / rate.
const SAI_PREDIVIDER: u32 = 4;
const SAI_ROOT_TIMES_RATE: u32 = 768_000;

/// LPI2C1 functional clock: PLL3 / 8 (60 MHz) / 6 = 10 MHz.
const LPI2C_DIVIDER: u32 = 6;
pub const LPI2C_CLK_HZ: u32 = 60_000_000 / LPI2C_DIVIDER;

// SAI register values (as programmed by the C firmware via the MCUXpresso
// SAI driver: classic I2S, 32-bit words, stereo, channel 0, master).
/// TCR1/RCR1: FIFO watermark 16 words.
const XCR1: u32 = 0x10;
/// TCR2: async, MCLK1, BCLK polarity active low, BCLK output, DIV 1
/// (BCLK = MCLK / 4 = 64 fs).
const TCR2: u32 = 0x0700_0001;
/// RCR2: as TCR2, synchronous to the transmitter.
const RCR2: u32 = 0x4700_0001;
/// TCR3/RCR3: data channel 0 enabled.
const XCR3: u32 = 0x0001_0000;
/// TCR4: FIFO continue on error, 2 words per frame, sync width 32 bits,
/// MSB first, frame sync one bit early, active low, generated internally,
/// CHMOD output mode.
const TCR4: u32 = 0x1001_1F3B;
/// RCR4: as TCR4 without CHMOD.
const RCR4: u32 = 0x1001_1F1B;
/// TCR5/RCR5: word N width 32, word 0 width 32, first bit shifted 31.
const XCR5: u32 = 0x1F1F_1F00;

const CSR_TE: u32 = 1 << 31; // TE / RE
const CSR_BCE: u32 = 1 << 28;
const CSR_FR: u32 = 1 << 25;
const CSR_SR: u32 = 1 << 24;
const CSR_WSF: u32 = 1 << 20;
const CSR_SEF: u32 = 1 << 19;
const CSR_FEF: u32 = 1 << 18;
const CSR_FRDE: u32 = 1 << 0;
/// Write-1-to-clear flags that must not be written back.
const CSR_W1C: u32 = CSR_WSF | CSR_SEF | CSR_FEF;

/// FIFO pointers: 5-bit index plus wrap bit.
const FIFO_PTR_MASK: u32 = 0x3F;

const DMA_ATTR_32BIT: u16 = (2 << 8) | 2;
const DMA_CSR_INTMAJOR: u16 = 1 << 1;
const DMAMUX_ENBL: u32 = 1 << 31;

#[repr(C, align(32))]
struct Ring([u32; RING_WORDS]);

static mut TX_RING: Ring = Ring([0; RING_WORDS]);
static mut RX_RING: Ring = Ring([0; RING_WORDS]);

static TX_WRAPS: AtomicU32 = AtomicU32::new(0);
static RX_WRAPS: AtomicU32 = AtomicU32::new(0);

fn sai() -> ral::sai::SAI1 {
    unsafe { ral::sai::SAI1::instance() }
}

fn dma() -> ral::dma::DMA {
    unsafe { ral::dma::DMA::instance() }
}

// --- interrupts: count major loop completions (ring wraps) ---------------

#[imxrt_rt::interrupt]
fn DMA0_DMA16() {
    let dma = dma();
    if ral::read_reg!(ral::dma, dma, INT) & (1 << TX_CH) != 0 {
        // Increment before clearing the flag, see dma_words().
        TX_WRAPS.fetch_add(1, Ordering::SeqCst);
        ral::write_reg!(ral::dma, dma, CINT, TX_CH as u8);
    }
    cortex_m::asm::dsb();
}

#[imxrt_rt::interrupt]
fn DMA1_DMA17() {
    let dma = dma();
    if ral::read_reg!(ral::dma, dma, INT) & (1 << RX_CH) != 0 {
        RX_WRAPS.fetch_add(1, Ordering::SeqCst);
        ral::write_reg!(ral::dma, dma, CINT, RX_CH as u8);
    }
    cortex_m::asm::dsb();
}

/// Total words moved by a ring channel. Race-free against the wrap
/// interrupt (which preempts every caller): retry if the wrap count
/// changed; if the major loop completed but its interrupt is still pending,
/// account for it here.
fn dma_words(ch: usize, wraps: &AtomicU32) -> u32 {
    let dma = dma();
    let (w, pos, pending) = loop {
        let w = wraps.load(Ordering::SeqCst);
        let pos = RING_WORDS as u32 - dma.TCD[ch].TCD_CITER_ELINKNO.read() as u32;
        let pending = ral::read_reg!(ral::dma, dma, INT) & (1 << ch) != 0;
        if w == wraps.load(Ordering::SeqCst) {
            break (w, pos, pending);
        }
    };
    let w = if pending && pos < RING_WORDS as u32 / 2 { w + 1 } else { w };
    w.wrapping_mul(RING_WORDS as u32).wrapping_add(pos)
}

/// Pads used by the audio interface.
pub struct Pins {
    pub mclk: iomuxc::pads::gpio_ad_b1::GPIO_AD_B1_00,
    pub tx_bclk: iomuxc::pads::gpio_ad_b1::GPIO_AD_B1_01,
    pub tx_sync: iomuxc::pads::gpio_ad_b1::GPIO_AD_B1_02,
    pub tx_data: iomuxc::pads::gpio_ad_b1::GPIO_AD_B1_03,
    pub rx_data: iomuxc::pads::gpio_ad_b1::GPIO_AD_B1_05,
    pub scl: iomuxc::pads::gpio_ad_b1::GPIO_AD_B1_14,
    pub sda: iomuxc::pads::gpio_ad_b1::GPIO_AD_B1_15,
}

/// SAI pads: keeper, R0/6, 100 MHz (0x10B0, NXP EVK reference).
const PAD_SAI: Config = Config::zero()
    .set_pull_keeper(Some(PullKeeper::Keeper))
    .set_speed(Speed::Fast)
    .set_drive_strength(DriveStrength::R0_6);
/// I2C pads: open drain, keeper, R0/6, 100 MHz (0xD8B0 minus the unused
/// pull-up select; the EVK has external pull-ups).
const PAD_I2C: Config = Config::zero()
    .set_open_drain(OpenDrain::Enabled)
    .set_pull_keeper(Some(PullKeeper::Keeper))
    .set_speed(Speed::Fast)
    .set_drive_strength(DriveStrength::R0_6);

pub type I2c = hal::lpi2c::Lpi2c;

/// Clocks, pins, DMA interrupts. Returns the codec I2C bus.
pub fn init(mut pins: Pins) -> I2c {
    let mut ccm = unsafe { ral::ccm::CCM::instance() };
    let mut ccm_analog = unsafe { ral::ccm_analog::CCM_ANALOG::instance() };

    ccm::analog::pll4::reconfigure(
        &mut ccm_analog,
        PLL4_DIV_SELECT,
        PLL4_NUM,
        PLL4_DENOM,
        ccm::analog::pll4::PostDivider::U1,
    );

    // Clock roots must change with their gates off.
    ccm::clock_gate::sai::<1>().set(&mut ccm, ccm::clock_gate::OFF);
    ccm::clock_gate::lpi2c::<1>().set(&mut ccm, ccm::clock_gate::OFF);
    ccm::sai_clk::set_selection::<1>(&mut ccm, ccm::sai_clk::Selection::Pll4);
    ccm::sai_clk::set_predivider::<1>(&mut ccm, SAI_PREDIVIDER);
    ccm::sai_clk::set_divider::<1>(&mut ccm, SAI_ROOT_TIMES_RATE / 48_000);
    ccm::lpi2c_clk::set_selection(&mut ccm, ccm::lpi2c_clk::Selection::Pll3Div8);
    ccm::lpi2c_clk::set_divider(&mut ccm, LPI2C_DIVIDER);
    for gate in [
        ccm::clock_gate::sai::<1>(),
        ccm::clock_gate::lpi2c::<1>(),
        ccm::clock_gate::dma(),
    ] {
        gate.set(&mut ccm, ccm::clock_gate::ON);
    }

    iomuxc::sai::prepare(&mut pins.mclk);
    iomuxc::sai::prepare(&mut pins.tx_bclk);
    iomuxc::sai::prepare(&mut pins.tx_sync);
    iomuxc::sai::prepare(&mut pins.tx_data);
    iomuxc::sai::prepare(&mut pins.rx_data);
    iomuxc::configure(&mut pins.mclk, PAD_SAI);
    iomuxc::configure(&mut pins.tx_bclk, PAD_SAI);
    iomuxc::configure(&mut pins.tx_sync, PAD_SAI);
    iomuxc::configure(&mut pins.tx_data, PAD_SAI);
    iomuxc::configure(&mut pins.rx_data, PAD_SAI);

    // SAI1_MCLK pad is an output driven by SAI1 MCLK1 = SAI1 clock root.
    let gpr = unsafe { ral::iomuxc_gpr::IOMUXC_GPR::instance() };
    ral::modify_reg!(ral::iomuxc_gpr, gpr, GPR1, SAI1_MCLK1_SEL: 0, SAI1_MCLK_DIR: 1);

    let lpi2c = unsafe { ral::lpi2c::LPI2C1::instance() };
    let i2c = hal::lpi2c::Lpi2c::with_pins(
        lpi2c,
        hal::lpi2c::Pins { scl: pins.scl, sda: pins.sda },
        &hal::lpi2c::Timing::ideal(LPI2C_CLK_HZ, hal::lpi2c::ClockSpeed::KHz100),
    );
    // with_pins() muxes the pads; apply the open-drain pad config after.
    // SAFETY: the pads were consumed by with_pins and are only reconfigured.
    unsafe {
        iomuxc::configure(&mut iomuxc::pads::gpio_ad_b1::GPIO_AD_B1_14::new(), PAD_I2C);
        iomuxc::configure(&mut iomuxc::pads::gpio_ad_b1::GPIO_AD_B1_15::new(), PAD_I2C);
    }

    unsafe {
        let mut cp = cortex_m::Peripherals::steal();
        cp.NVIC.set_priority(ral::Interrupt::DMA0_DMA16, 0);
        cp.NVIC.set_priority(ral::Interrupt::DMA1_DMA17, 0);
        NVIC::unmask(ral::Interrupt::DMA0_DMA16);
        NVIC::unmask(ral::Interrupt::DMA1_DMA17);
    }
    i2c
}

fn dma_ring_setup(ch: usize, src: u32, soff: u16, slast: u32, dst: u32, doff: u16, dlast: u32, source: u32) {
    let dma = dma();
    let mux = unsafe { ral::dmamux::DMAMUX::instance() };
    ral::write_reg!(ral::dma, dma, CERQ, ch as u8);
    mux.CHCFG[ch].write(0);

    let tcd = &dma.TCD[ch];
    tcd.TCD_CSR.write(0);
    tcd.TCD_SADDR.write(src);
    tcd.TCD_SOFF.write(soff);
    tcd.TCD_ATTR.write(DMA_ATTR_32BIT);
    tcd.TCD_NBYTES_MLNO.write(4);
    tcd.TCD_SLAST.write(slast);
    tcd.TCD_DADDR.write(dst);
    tcd.TCD_DOFF.write(doff);
    tcd.TCD_CITER_ELINKNO.write(RING_WORDS as u16);
    tcd.TCD_BITER_ELINKNO.write(RING_WORDS as u16);
    tcd.TCD_DLASTSGA.write(dlast);
    tcd.TCD_CSR.write(DMA_CSR_INTMAJOR);
    ral::write_reg!(ral::dma, dma, CINT, ch as u8);
    ral::write_reg!(ral::dma, dma, CERR, ch as u8);

    mux.CHCFG[ch].write(source | DMAMUX_ENBL);
}

fn tx_fifo_fill(tfr: u32) -> u32 {
    ((tfr >> 16).wrapping_sub(tfr)) & FIFO_PTR_MASK
}

/// SAI1 root divider for a rate, if the rate is reachable at 256 fs.
fn rate_divider(rate: u32) -> Option<u32> {
    let d = SAI_ROOT_TIMES_RATE.checked_div(rate)?;
    (d >= 1 && d <= 64 && d * rate == SAI_ROOT_TIMES_RATE).then_some(d)
}

pub fn stop() {
    let sai = sai();
    // RX is synchronous to TX: stop RX first, then TX. Enables clear at the
    // end of the current frame.
    ral::modify_reg!(ral::sai, sai, RCSR, |v| v & !CSR_W1C & !(CSR_TE | CSR_FRDE));
    ral::modify_reg!(ral::sai, sai, TCSR, |v| v & !CSR_W1C & !(CSR_TE | CSR_FRDE));
    for _ in 0..1_000_000 {
        if (ral::read_reg!(ral::sai, sai, RCSR) | ral::read_reg!(ral::sai, sai, TCSR)) & CSR_TE == 0 {
            break;
        }
    }
    ral::modify_reg!(ral::sai, sai, TCSR, |v| v & !CSR_W1C & !CSR_BCE);
    ral::modify_reg!(ral::sai, sai, RCSR, |v| v & !CSR_W1C & !CSR_BCE);

    let dma = dma();
    ral::write_reg!(ral::dma, dma, CERQ, TX_CH as u8);
    ral::write_reg!(ral::dma, dma, CERQ, RX_CH as u8);
}

/// (Re)start the audio interface at `rate`. MCLK runs from here on.
pub fn start(rate: u32) -> bool {
    let Some(div) = rate_divider(rate) else {
        return false;
    };
    stop();

    let mut ccm = unsafe { ral::ccm::CCM::instance() };
    ccm::clock_gate::sai::<1>().set(&mut ccm, ccm::clock_gate::OFF);
    ccm::sai_clk::set_divider::<1>(&mut ccm, div);
    ccm::clock_gate::sai::<1>().set(&mut ccm, ccm::clock_gate::ON);

    let sai = sai();
    // Software reset, then FIFO reset.
    ral::write_reg!(ral::sai, sai, TCSR, CSR_SR);
    ral::write_reg!(ral::sai, sai, RCSR, CSR_SR);
    ral::write_reg!(ral::sai, sai, TCSR, 0);
    ral::write_reg!(ral::sai, sai, RCSR, 0);
    ral::write_reg!(ral::sai, sai, TCSR, CSR_FR);
    ral::write_reg!(ral::sai, sai, RCSR, CSR_FR);

    ral::write_reg!(ral::sai, sai, TCR1, XCR1);
    ral::write_reg!(ral::sai, sai, TCR2, TCR2);
    ral::write_reg!(ral::sai, sai, TCR3, XCR3);
    ral::write_reg!(ral::sai, sai, TCR4, TCR4);
    ral::write_reg!(ral::sai, sai, TCR5, XCR5);
    ral::write_reg!(ral::sai, sai, TMR, 0);
    ral::write_reg!(ral::sai, sai, RCR1, XCR1);
    ral::write_reg!(ral::sai, sai, RCR2, RCR2);
    ral::write_reg!(ral::sai, sai, RCR3, XCR3);
    ral::write_reg!(ral::sai, sai, RCR4, RCR4);
    ral::write_reg!(ral::sai, sai, RCR5, XCR5);
    ral::write_reg!(ral::sai, sai, RMR, 0);

    // SAFETY: the DMA is stopped; nothing else reads the rings now.
    let (tx, rx) = unsafe { (&mut *addr_of_mut!(TX_RING), &mut *addr_of_mut!(RX_RING)) };
    tx.0.fill(0);
    rx.0.fill(0);
    TX_WRAPS.store(0, Ordering::SeqCst);
    RX_WRAPS.store(0, Ordering::SeqCst);

    let ring_bytes = (RING_WORDS * 4) as u32;
    let tdr0 = &sai.TDR[0] as *const _ as u32;
    let rdr0 = &sai.RDR[0] as *const _ as u32;
    dma_ring_setup(TX_CH, tx.0.as_ptr() as u32, 4, ring_bytes.wrapping_neg(), tdr0, 0, 0, DMAMUX_SAI1_TX);
    dma_ring_setup(RX_CH, rdr0, 0, 0, rx.0.as_ptr() as u32, 4, ring_bytes.wrapping_neg(), DMAMUX_SAI1_RX);

    let dma = dma();
    ral::write_reg!(ral::dma, dma, SERQ, TX_CH as u8);
    ral::write_reg!(ral::dma, dma, SERQ, RX_CH as u8);
    ral::write_reg!(ral::sai, sai, TCSR, CSR_FRDE);
    ral::write_reg!(ral::sai, sai, RCSR, CSR_FRDE);

    // Let the DMA prefill the TX FIFO before the transmitter starts.
    for _ in 0..100_000 {
        if tx_fifo_fill(ral::read_reg!(ral::sai, sai, TFR[0])) > 16 {
            break;
        }
    }
    // The synchronous receiver starts with the transmitter's first frame.
    ral::write_reg!(ral::sai, sai, RCSR, CSR_FRDE | CSR_TE | CSR_BCE);
    ral::write_reg!(ral::sai, sai, TCSR, CSR_FRDE | CSR_TE | CSR_BCE);
    true
}

/// TX ring, frames interleaved L R (32-bit slots).
pub fn tx_ring() -> &'static mut [u32; RING_WORDS] {
    // SAFETY: the USB ISR is the only writer; the DMA only reads.
    unsafe { &mut (*addr_of_mut!(TX_RING)).0 }
}

pub fn rx_ring() -> &'static [u32; RING_WORDS] {
    unsafe { &(*addr_of_mut!(RX_RING)).0 }
}

/// Frames the TX DMA has fetched from the ring (free-running).
pub fn tx_frames_fetched() -> u32 {
    dma_words(TX_CH, &TX_WRAPS) / CHANNELS as u32
}

/// Frames the RX DMA has written into the ring (free-running).
pub fn rx_frames_written() -> u32 {
    dma_words(RX_CH, &RX_WRAPS) / CHANNELS as u32
}

/// Words the SAI transmitter has shifted out: DMA words minus TX FIFO fill.
pub fn tx_words_played() -> u32 {
    let sai = sai();
    loop {
        let f1 = ral::read_reg!(ral::sai, sai, TFR[0]);
        let d = dma_words(TX_CH, &TX_WRAPS);
        let f2 = ral::read_reg!(ral::sai, sai, TFR[0]);
        // A DMA write between the reads changes the FIFO write pointer: retry.
        if (f1 ^ f2) & (FIFO_PTR_MASK << 16) == 0 {
            return d.wrapping_sub(tx_fifo_fill(f2));
        }
    }
}

/// Count and clear SAI FIFO errors (TX underrun, RX overrun).
pub fn poll_errors(tx_errors: &mut u32, rx_errors: &mut u32) {
    let sai = sai();
    if ral::read_reg!(ral::sai, sai, TCSR) & CSR_FEF != 0 {
        ral::modify_reg!(ral::sai, sai, TCSR, |v| (v & !CSR_W1C) | CSR_FEF);
        *tx_errors = tx_errors.wrapping_add(1);
    }
    if ral::read_reg!(ral::sai, sai, RCSR) & CSR_FEF != 0 {
        ral::modify_reg!(ral::sai, sai, RCSR, |v| (v & !CSR_W1C) | CSR_FEF);
        *rx_errors = rx_errors.wrapping_add(1);
    }
}
