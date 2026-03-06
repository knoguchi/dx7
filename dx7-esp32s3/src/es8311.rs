/// ES8311 audio codec driver (I2C register configuration).
/// Based on Espressif's esp_codec_dev ES8311 driver (Apache-2.0).

use esp_hal::i2c::master::I2c;
use esp_hal::Blocking;

const ADDR: u8 = 0x18;

// Registers
const REG00_RESET: u8 = 0x00;
const REG01_CLK_MGR: u8 = 0x01;
const REG02_CLK_MGR: u8 = 0x02;
const REG03_CLK_MGR: u8 = 0x03;
const REG04_CLK_MGR: u8 = 0x04;
const REG05_CLK_MGR: u8 = 0x05;
const REG06_CLK_MGR: u8 = 0x06;
const REG07_CLK_MGR: u8 = 0x07;
const REG08_CLK_MGR: u8 = 0x08;
const REG09_SDP_IN: u8 = 0x09;  // DAC serial port
const REG0A_SDP_OUT: u8 = 0x0A; // ADC serial port
const REG0B_SYSTEM: u8 = 0x0B;
const REG0C_SYSTEM: u8 = 0x0C;
const REG0D_SYSTEM: u8 = 0x0D;
const REG0E_SYSTEM: u8 = 0x0E;
const REG10_SYSTEM: u8 = 0x10;
const REG11_SYSTEM: u8 = 0x11;
const REG12_SYSTEM: u8 = 0x12;
const REG13_SYSTEM: u8 = 0x13;
const REG14_SYSTEM: u8 = 0x14;
const REG15_ADC: u8 = 0x15;
const REG16_ADC: u8 = 0x16;
const REG17_ADC: u8 = 0x17;
const REG1B_ADC: u8 = 0x1B;
const REG1C_ADC: u8 = 0x1C;
const REG32_DAC_VOL: u8 = 0x32;
const REG37_DAC: u8 = 0x37;
const REG44_GPIO: u8 = 0x44;
const REG45_GP: u8 = 0x45;

// Clock coefficient table: mclk, rate, pre_div, pre_multi, adc_div, dac_div,
//                           fs_mode, lrck_h, lrck_l, bclk_div, adc_osr, dac_osr
struct CoeffDiv {
    mclk: u32,
    rate: u32,
    pre_div: u8,
    pre_multi: u8,
    adc_div: u8,
    dac_div: u8,
    fs_mode: u8,
    lrck_h: u8,
    lrck_l: u8,
    bclk_div: u8,
    adc_osr: u8,
    dac_osr: u8,
}

const COEFFS: &[CoeffDiv] = &[
    // 48k: MCLK=12.288MHz (256*fs)
    CoeffDiv { mclk: 12288000, rate: 48000, pre_div: 1, pre_multi: 1, adc_div: 1, dac_div: 1, fs_mode: 0, lrck_h: 0x00, lrck_l: 0xff, bclk_div: 4, adc_osr: 0x10, dac_osr: 0x10 },
    CoeffDiv { mclk: 6144000,  rate: 48000, pre_div: 1, pre_multi: 2, adc_div: 1, dac_div: 1, fs_mode: 0, lrck_h: 0x00, lrck_l: 0xff, bclk_div: 4, adc_osr: 0x10, dac_osr: 0x10 },
    // 44.1k: MCLK=11.2896MHz (256*fs)
    CoeffDiv { mclk: 11289600, rate: 44100, pre_div: 1, pre_multi: 1, adc_div: 1, dac_div: 1, fs_mode: 0, lrck_h: 0x00, lrck_l: 0xff, bclk_div: 4, adc_osr: 0x10, dac_osr: 0x10 },
];

fn find_coeff(mclk: u32, rate: u32) -> Option<&'static CoeffDiv> {
    COEFFS.iter().find(|c| c.mclk == mclk && c.rate == rate)
}

fn wreg(i2c: &mut I2c<'_, Blocking>, reg: u8, val: u8) {
    i2c.write(ADDR, &[reg, val]).ok();
}

fn rreg(i2c: &mut I2c<'_, Blocking>, reg: u8) -> u8 {
    let mut buf = [0u8; 1];
    i2c.write_read(ADDR, &[reg], &mut buf).ok();
    buf[0]
}

/// Initialize ES8311 for I2S DAC playback in slave mode.
/// `mclk_hz`: MCLK frequency provided by ESP32 I2S (e.g. 12288000 for 48kHz * 256).
/// `sample_rate`: target sample rate (48000 or 44100).
pub fn init(i2c: &mut I2c<'_, Blocking>, mclk_hz: u32, sample_rate: u32) {
    // Phase 1: Soft defaults (from es8311_open)
    wreg(i2c, REG44_GPIO, 0x08); // I2C noise immunity
    wreg(i2c, REG44_GPIO, 0x08); // write twice per Espressif recommendation

    wreg(i2c, REG01_CLK_MGR, 0x30);
    wreg(i2c, REG02_CLK_MGR, 0x00);
    wreg(i2c, REG03_CLK_MGR, 0x10);
    wreg(i2c, REG16_ADC, 0x24);
    wreg(i2c, REG04_CLK_MGR, 0x10);
    wreg(i2c, REG05_CLK_MGR, 0x00);
    wreg(i2c, REG0B_SYSTEM, 0x00);
    wreg(i2c, REG0C_SYSTEM, 0x00);
    wreg(i2c, REG10_SYSTEM, 0x1F);
    wreg(i2c, REG11_SYSTEM, 0x7F);
    wreg(i2c, REG00_RESET, 0x80); // out of reset, slave mode

    // MCLK from pin, not inverted, all clocks on
    wreg(i2c, REG01_CLK_MGR, 0x3F);

    // SCLK not inverted
    let reg06 = rreg(i2c, REG06_CLK_MGR);
    wreg(i2c, REG06_CLK_MGR, reg06 & !0x20);

    wreg(i2c, REG13_SYSTEM, 0x10);
    wreg(i2c, REG1B_ADC, 0x0A);
    wreg(i2c, REG1C_ADC, 0x6A);
    wreg(i2c, REG44_GPIO, 0x08); // no DAC ref to ADC

    // Phase 2: Configure sample rate from coefficient table
    if let Some(c) = find_coeff(mclk_hz, sample_rate) {
        let mut reg02 = rreg(i2c, REG02_CLK_MGR) & 0x07;
        reg02 |= (c.pre_div - 1) << 5;
        let pre_multi_bits = match c.pre_multi {
            1 => 0, 2 => 1, 4 => 2, _ => 3,
        };
        reg02 |= pre_multi_bits << 3;
        wreg(i2c, REG02_CLK_MGR, reg02);

        wreg(i2c, REG05_CLK_MGR, ((c.adc_div - 1) << 4) | (c.dac_div - 1));

        let reg03 = (rreg(i2c, REG03_CLK_MGR) & 0x80) | (c.fs_mode << 6) | c.adc_osr;
        wreg(i2c, REG03_CLK_MGR, reg03);

        let reg04 = (rreg(i2c, REG04_CLK_MGR) & 0x80) | c.dac_osr;
        wreg(i2c, REG04_CLK_MGR, reg04);

        let reg07 = (rreg(i2c, REG07_CLK_MGR) & 0xC0) | c.lrck_h;
        wreg(i2c, REG07_CLK_MGR, reg07);
        wreg(i2c, REG08_CLK_MGR, c.lrck_l);

        let reg06v = (rreg(i2c, REG06_CLK_MGR) & 0xE0) | (c.bclk_div - 1);
        wreg(i2c, REG06_CLK_MGR, reg06v);
    }

    // Phase 3: Set format — I2S normal, 16-bit
    let dac_iface = (rreg(i2c, REG09_SDP_IN) & 0xFC) | 0x0C; // I2S + 16bit
    wreg(i2c, REG09_SDP_IN, dac_iface & 0xE3 | 0x0C);
    let adc_iface = (rreg(i2c, REG0A_SDP_OUT) & 0xFC) | 0x0C;
    wreg(i2c, REG0A_SDP_OUT, adc_iface & 0xE3 | 0x0C);

    // Phase 4: Start — enable DAC path (from es8311_start, DAC-only mode)
    wreg(i2c, REG00_RESET, 0x80); // slave mode
    wreg(i2c, REG01_CLK_MGR, 0x3F); // MCLK on

    // Unmute DAC SDP, mute ADC SDP
    let dac_sdp = rreg(i2c, REG09_SDP_IN) & !0x40;  // unmute DAC
    wreg(i2c, REG09_SDP_IN, dac_sdp);
    let adc_sdp = rreg(i2c, REG0A_SDP_OUT) | 0x40;  // mute ADC
    wreg(i2c, REG0A_SDP_OUT, adc_sdp);

    wreg(i2c, REG17_ADC, 0xBF);
    wreg(i2c, REG0E_SYSTEM, 0x02);
    wreg(i2c, REG12_SYSTEM, 0x00);
    wreg(i2c, REG14_SYSTEM, 0x1A);
    wreg(i2c, REG0D_SYSTEM, 0x01);
    wreg(i2c, REG15_ADC, 0x40);
    wreg(i2c, REG37_DAC, 0x08);
    wreg(i2c, REG45_GP, 0x00);

    // Set DAC volume to 0dB
    wreg(i2c, REG32_DAC_VOL, 0xBF);
}

/// Set DAC volume. 0x00 = -95.5dB, 0xBF = 0dB, 0xFF = +32dB.
pub fn set_volume(i2c: &mut I2c<'_, Blocking>, vol: u8) {
    wreg(i2c, REG32_DAC_VOL, vol);
}
