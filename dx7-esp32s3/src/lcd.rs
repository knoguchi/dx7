/// GC9A01 240x240 round LCD display driver (SPI, BasicMode — no framebuffer).

use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::spi::master::{Spi, Config as SpiConfig};
use esp_hal::time::Rate;
use esp_hal::Blocking;

use embedded_hal_bus::spi::ExclusiveDevice;
use display_interface_spi::SPIInterface;
use gc9a01::{prelude::*, Gc9a01, mode::BasicMode};
use embedded_graphics::{
    mono_font::{ascii::FONT_10X20, MonoTextStyleBuilder},
    pixelcolor::Rgb565,
    prelude::*,
    text::{Alignment, Text},
};

type SpiDev = ExclusiveDevice<Spi<'static, Blocking>, Output<'static>, BusyDelay>;
type Display = Gc9a01<
    SPIInterface<SpiDev, Output<'static>>,
    DisplayResolution240x240,
    BasicMode,
>;

static mut DISPLAY: Option<Display> = None;

/// Initialize the GC9A01 display. Call once at startup.
pub fn init() {
    let spi = Spi::new(
        unsafe { esp_hal::peripherals::SPI2::steal() },
        SpiConfig::default().with_frequency(Rate::from_mhz(40)),
    ).unwrap()
    .with_sck(unsafe { esp_hal::peripherals::GPIO4::steal() })
    .with_mosi(unsafe { esp_hal::peripherals::GPIO2::steal() });

    let cs = Output::new(
        unsafe { esp_hal::peripherals::GPIO5::steal() },
        Level::High, OutputConfig::default(),
    );
    let dc = Output::new(
        unsafe { esp_hal::peripherals::GPIO47::steal() },
        Level::Low, OutputConfig::default(),
    );
    let mut rst = Output::new(
        unsafe { esp_hal::peripherals::GPIO38::steal() },
        Level::High, OutputConfig::default(),
    );
    // Backlight on (active low)
    let _bl = Output::new(
        unsafe { esp_hal::peripherals::GPIO42::steal() },
        Level::Low, OutputConfig::default(),
    );
    core::mem::forget(_bl);

    let spi_dev = ExclusiveDevice::new(spi, cs, BusyDelay).unwrap();
    let spi_iface = SPIInterface::new(spi_dev, dc);
    let mut display = Gc9a01::new(
        spi_iface,
        DisplayResolution240x240,
        DisplayRotation::Rotate0,
    );

    // Hardware reset
    rst.set_low();
    busy_delay_ms(10);
    rst.set_high();
    busy_delay_ms(120);

    display.init_with_addr_mode(&mut BusyDelay).unwrap();

    use embedded_graphics::prelude::DrawTarget as _;
    DrawTarget::clear(&mut display, Rgb565::BLACK).unwrap();

    core::mem::forget(rst);
    unsafe { DISPLAY = Some(display); }
}

/// Draw patch name centered on display.
pub fn draw_patch(program: u8, name: &str) {
    let display = unsafe { DISPLAY.as_mut().unwrap() };

    use embedded_graphics::prelude::DrawTarget as _;
    DrawTarget::clear(display, Rgb565::BLACK).unwrap();

    // Format "N: NAME"
    let mut buf = [0u8; 16];
    let len = format_patch_line(program + 1, name.trim(), &mut buf);
    let line = core::str::from_utf8(&buf[..len]).unwrap();

    let style = MonoTextStyleBuilder::new()
        .font(&FONT_10X20)
        .text_color(Rgb565::WHITE)
        .background_color(Rgb565::BLACK)
        .build();

    Text::with_alignment(line, Point::new(120, 120), style, Alignment::Center)
        .draw(display)
        .unwrap();
}

/// Format "N: NAME" into buf, return length.
fn format_patch_line(num: u8, name: &str, buf: &mut [u8; 16]) -> usize {
    let mut pos = 0;
    // Write number
    if num >= 100 { buf[pos] = b'0' + num / 100; pos += 1; }
    if num >= 10 { buf[pos] = b'0' + (num / 10) % 10; pos += 1; }
    buf[pos] = b'0' + num % 10; pos += 1;
    buf[pos] = b':'; pos += 1;
    // Write name (up to remaining space)
    for &b in name.as_bytes() {
        if pos >= buf.len() { break; }
        buf[pos] = b; pos += 1;
    }
    pos
}

fn format_u8(n: u8, buf: &mut [u8; 4]) -> &str {
    let mut i = buf.len();
    let mut val = n;
    if val == 0 {
        i -= 1;
        buf[i] = b'0';
    } else {
        while val > 0 {
            i -= 1;
            buf[i] = b'0' + (val % 10);
            val /= 10;
        }
    }
    core::str::from_utf8(&buf[i..]).unwrap()
}

fn busy_delay_ms(ms: u32) {
    for _ in 0..ms {
        for _ in 0..20_000u32 {
            core::hint::spin_loop();
        }
    }
}

#[derive(Clone)]
struct BusyDelay;

impl embedded_hal::delay::DelayNs for BusyDelay {
    fn delay_ns(&mut self, ns: u32) {
        let ms = (ns + 999_999) / 1_000_000;
        if ms > 0 { busy_delay_ms(ms); }
    }
}
