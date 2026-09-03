// M5Stack Core2(初代、AXP192)のハードウェア初期化。
//
// ピン配置とAXP192の電源投入手順は、M5GFXのCore2 autodetect実装と
// axp192 crateのm5stack-core2 exampleを基準にしている。
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
use esp_idf_hal::spi::config::{Config as SpiConfig, DriverConfig, Duplex};
use esp_idf_hal::spi::{SpiDeviceDriver, SpiDriver, SPI2};
use esp_idf_hal::units::FromValueType;
use ft6x36::Ft6x36;
use mipidsi::interface::SpiInterface;
use mipidsi::models::ILI9342CRgb565;
use mipidsi::options::{ColorInversion, Orientation, Rotation};
use mipidsi::Builder;

/// mipidsiのSPI転送バッファ。320px * 2bytesで1行分をまとめて送れる。
const SPI_BUFFER_SIZE: usize = DISPLAY_WIDTH as usize * 2;

pub const DISPLAY_WIDTH: u16 = 320;
pub const DISPLAY_HEIGHT: u16 = 240;

/// Core2のタッチ範囲は画面より縦に広い。y=0..239が画面、y=240..279が
/// 画面下の物理ボタン帯に対応する。
pub const TOUCH_WIDTH: u16 = 320;
pub const TOUCH_HEIGHT: u16 = 280;

// axp192とft6x36は各ドライバ側でI2Cアドレスを持つ。同じ内部I2Cバスを共有する。

pub type SharedI2c<'d> = RefCell<I2cDriver<'d>>;

/// LCDとタッチコントローラーを使う前に必要なAXP192電源投入手順。
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

    // LCDリセット線をパルスする。
    axp.set_gpio4_output(false)?;
    FreeRtos::delay_ms(100);
    axp.set_gpio4_output(true)?;
    FreeRtos::delay_ms(100);

    Ok(())
}

pub struct DisplayPins {
    pub sclk: Gpio18<'static>,
    pub mosi: Gpio23<'static>,
    pub dc: Gpio15<'static>,
    pub cs: Gpio5<'static>,
}

pub type Core2Display<'d> = mipidsi::Display<
    SpiInterface<'static, SpiDeviceDriver<'d, SpiDriver<'d>>, PinDriver<'d, Output>>,
    ILI9342CRgb565,
    mipidsi::NoResetPin,
>;

/// SPI経由でILI9342Cを初期化する。LCDリセットはAXP192 GPIO4側で行うため、
/// 先に `init_power` を実行しておく。
pub fn init_display<'d>(
    spi: SPI2<'d>,
    pins: DisplayPins,
) -> Result<Core2Display<'d>, Box<dyn std::error::Error>> {
    // MISOは使わない。設定するとfull-duplex扱いになり、利用可能なSPI clockが
    // 26.7MHzに制限される。
    let spi_driver = SpiDriver::new(
        spi,
        pins.sclk,
        pins.mosi,
        None::<AnyIOPin>,
        &DriverConfig::new(),
    )?;

    // 画面からの読み取りはしないためhalf-duplex/write-onlyで駆動する。
    // M5GFXも同じ方針で40MHz書き込みを使う。
    let spi_config = SpiConfig::new()
        .baudrate(40_u32.MHz().into())
        .write_only(true)
        .duplex(Duplex::Half3Wire);
    let spi_device = SpiDeviceDriver::new(spi_driver, Some(pins.cs), &spi_config)?;

    let dc = PinDriver::output(pins.dc)?;
    // displayはプログラム全体で生存するため、SPIバッファもstaticとして保持する。
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
    i2c: I2C0<'d>,
    sda: AnyIOPin<'d>,
    scl: AnyIOPin<'d>,
) -> Result<I2cDriver<'d>, esp_idf_sys::EspError> {
    let config = I2cConfig::new().baudrate(400_u32.kHz().into());
    I2cDriver::new(i2c, sda, scl, &config)
}

pub fn new_axp<'a, 'd>(bus: &'a SharedI2c<'d>) -> Axp192<RefCellDevice<'a, I2cDriver<'d>>> {
    Axp192::new(RefCellDevice::new(bus))
}

pub fn new_touch<'a, 'd>(bus: &'a SharedI2c<'d>) -> Ft6x36<RefCellDevice<'a, I2cDriver<'d>>> {
    // Core2のタッチ座標は画面座標と一致するため、デフォルト向きのまま使う。
    Ft6x36::new(
        RefCellDevice::new(bus),
        ft6x36::Dimension(TOUCH_WIDTH, TOUCH_HEIGHT),
    )
}
