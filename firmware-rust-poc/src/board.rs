// M5Stack Core2 (1st gen, AXP192) hardware bring-up in pure Rust.
//
// Pin assignments and the AXP192 power sequence are taken from M5GFX's own
// Core2 autodetect path (M5GFX.cpp) and the axp192 crate's m5stack-core2
// example:
//   LCD (ILI9342C, 320x240): MOSI=23, MISO=38, SCLK=18, DC=15, CS=5
//   LCD reset:     AXP192 GPIO4
//   LCD power:     AXP192 LDO2  @ 3300mV
//   LCD backlight: AXP192 DCDC3 @ 2800mV
//   Touch (FT6336U, FT5x06-compatible): I2C 0x38, INT=39
//   AXP192: I2C 0x34; shared bus SDA=21, SCL=22 @ 400kHz

use std::cell::RefCell;

use axp192::Axp192;
use embedded_hal_bus::i2c::RefCellDevice;
use esp_idf_hal::delay::{Delay, FreeRtos};
use esp_idf_hal::gpio::{AnyIOPin, Gpio15, Gpio18, Gpio23, Gpio5, Output, PinDriver};
use esp_idf_hal::i2c::{I2cConfig, I2cDriver, I2C0};
use esp_idf_hal::prelude::*;
use esp_idf_hal::spi::config::{Config as SpiConfig, Duplex, DriverConfig};
use esp_idf_hal::spi::{SpiDeviceDriver, SpiDriver, SPI2};
use ft6x36::Ft6x36;
use mipidsi::interface::SpiInterface;
use mipidsi::models::ILI9342CRgb565;
use mipidsi::options::{ColorInversion, Orientation, Rotation};
use mipidsi::Builder;

/// Pixel batching buffer for mipidsi's SPI interface. 320px * 2 bytes covers
/// a full display row per SPI transaction.
const SPI_BUFFER_SIZE: usize = DISPLAY_WIDTH as usize * 2;

pub const DISPLAY_WIDTH: u16 = 320;
pub const DISPLAY_HEIGHT: u16 = 240;

const AXP192_I2C_ADDR: u8 = 0x34;
const TOUCH_I2C_ADDR: u8 = 0x38;

pub type SharedI2c<'d> = RefCell<I2cDriver<'d>>;

/// Runs the AXP192 power-up sequence required before the LCD and touch
/// controller respond. Mirrors the sequence in the axp192 crate's
/// m5stack-core2 example.
pub fn init_power<I2C, E>(axp: &mut Axp192<I2C>) -> Result<(), E>
where
    I2C: embedded_hal::i2c::I2c<Error = E>,
{
    axp.set_dcdc1_voltage(3350)?; // ESP32 VDD
    axp.set_ldo2_voltage(3300)?; // LCD + touch power
    axp.set_ldo2_on(true)?;
    axp.set_ldo3_voltage(2000)?; // vibration motor
    axp.set_ldo3_on(false)?;
    axp.set_dcdc3_voltage(2800)?; // LCD backlight
    axp.set_dcdc3_on(true)?;

    axp.set_gpio1_mode(axp192::GpioMode12::NmosOpenDrainOutput)?; // power LED
    axp.set_gpio1_output(false)?;
    axp.set_gpio2_mode(axp192::GpioMode12::NmosOpenDrainOutput)?; // speaker
    axp.set_gpio2_output(true)?;

    axp.set_key_mode(
        axp192::ShutdownDuration::Sd4s,
        axp192::PowerOkDelay::Delay64ms,
        true,
        axp192::LongPress::Lp1000ms,
        axp192::BootTime::Boot512ms,
    )?;

    axp.set_gpio4_mode(axp192::GpioMode34::NmosOpenDrainOutput)?; // LCD reset

    axp.set_battery_voltage_adc_enable(true)?;
    axp.set_battery_current_adc_enable(true)?;
    axp.set_acin_current_adc_enable(true)?;
    axp.set_acin_voltage_adc_enable(true)?;

    // Pulse the LCD reset line.
    axp.set_gpio4_output(false)?;
    FreeRtos::delay_ms(100);
    axp.set_gpio4_output(true)?;
    FreeRtos::delay_ms(100);

    Ok(())
}

pub struct DisplayPins {
    pub sclk: Gpio18,
    pub mosi: Gpio23,
    pub dc: Gpio15,
    pub cs: Gpio5,
}

pub type Core2Display<'d> = mipidsi::Display<
    SpiInterface<'static, SpiDeviceDriver<'d, SpiDriver<'d>>, PinDriver<'d, Gpio15, Output>>,
    ILI9342CRgb565,
    mipidsi::NoResetPin,
>;

/// Initializes the ILI9342C over SPI. The panel's reset line hangs off the
/// AXP192 (GPIO4), so `init_power` must have run first and mipidsi is built
/// without a reset pin.
pub fn init_display<'d>(
    spi: SPI2,
    pins: DisplayPins,
) -> Result<Core2Display<'d>, Box<dyn std::error::Error>> {
    // MISO is deliberately left unconfigured: the panel is write-only here, and
    // configuring it puts the SPI peripheral in full-duplex mode, which caps the
    // usable clock at 26.7MHz ("device cannot read correct data" from spi_hal).
    let spi_driver = SpiDriver::new(
        spi,
        pins.sclk,
        pins.mosi,
        None::<AnyIOPin>,
        &DriverConfig::new(),
    )?;

    // Half-duplex/write-only: the panel is never read here, and full-duplex
    // caps the usable clock at 26.7MHz. M5GFX drives this panel the same way
    // (spi_3wire = true, 40MHz write clock).
    let spi_config = SpiConfig::new()
        .baudrate(40.MHz().into())
        .write_only(true)
        .duplex(Duplex::Half3Wire);
    let spi_device = SpiDeviceDriver::new(spi_driver, Some(pins.cs), &spi_config)?;

    let dc = PinDriver::output(pins.dc)?;
    // Leaked so the interface can hold a 'static buffer; the display lives for
    // the whole program anyway.
    let buffer: &'static mut [u8] = Box::leak(Box::new([0u8; SPI_BUFFER_SIZE]));
    let di = SpiInterface::new(spi_device, dc, buffer);

    let mut delay = Delay::new_default();
    let display = Builder::new(ILI9342CRgb565, di)
        .display_size(DISPLAY_WIDTH, DISPLAY_HEIGHT)
        .orientation(Orientation::new().rotate(Rotation::Deg0))
        .invert_colors(ColorInversion::Inverted)
        .init(&mut delay)
        .map_err(|e| format!("display init failed: {e:?}"))?;

    Ok(display)
}

pub fn new_i2c<'d>(
    i2c: I2C0,
    sda: AnyIOPin,
    scl: AnyIOPin,
) -> Result<I2cDriver<'d>, esp_idf_sys::EspError> {
    let config = I2cConfig::new().baudrate(400.kHz().into());
    I2cDriver::new(i2c, sda, scl, &config)
}

pub fn new_axp<'a, 'd>(bus: &'a SharedI2c<'d>) -> Axp192<RefCellDevice<'a, I2cDriver<'d>>> {
    Axp192::new(RefCellDevice::new(bus))
}

pub fn new_touch<'a, 'd>(bus: &'a SharedI2c<'d>) -> Ft6x36<RefCellDevice<'a, I2cDriver<'d>>> {
    let _ = AXP192_I2C_ADDR;
    Ft6x36::new(RefCellDevice::new(bus), ft6x36::Dimension(DISPLAY_WIDTH, DISPLAY_HEIGHT))
}

pub const fn touch_addr() -> u8 {
    TOUCH_I2C_ADDR
}

/// Raw dump of the FT6x36 report header (DEV_MODE, GEST_ID, TD_STATUS and the
/// first touch point). Used to tell "the controller reports no touch" apart
/// from "we are not reading the controller correctly".
pub fn read_touch_raw(bus: &SharedI2c<'_>) -> Result<[u8; 7], esp_idf_sys::EspError> {
    let mut buf = [0u8; 7];
    bus.borrow_mut()
        .write_read(TOUCH_I2C_ADDR, &[0x00], &mut buf, 1000)?;
    Ok(buf)
}
