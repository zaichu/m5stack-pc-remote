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
use esp_idf_hal::prelude::*;
use esp_idf_hal::spi::config::{Config as SpiConfig, DriverConfig, Duplex};
use esp_idf_hal::spi::{SpiDeviceDriver, SpiDriver, SPI2};
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
// 下のIRQ生アクセスで使うAXP192のスレーブアドレスも同じ0x34。
const AXP192_ADDRESS: u8 = 0x34;

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

/// バッテリー状態。AXP192から読んだ値を `battery` crateの純粋ロジックで
/// 残量へ概算したもの。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    pub percent: u8,
    /// 実際に充電中(充電電流の向き)。USB給電中でも満充電で充電が止まれば
    /// falseになるため、画面の「CHG」表示は完了時に消える。
    pub charging: bool,
    /// 外部給電の有無(ACINまたはVBUS)。充電停止後の満充電状態でもtrueのまま
    /// なので、「給電されているが充電していない」を表せる。
    pub powered: bool,
}

/// バッテリー電圧と充電状態を読む。I2Cが応答しない場合はNone。
pub fn read_battery<I2C, E>(axp: &mut Axp192<I2C>) -> Option<Battery>
where
    I2C: embedded_hal::i2c::I2c<Error = E>,
{
    let volts = axp.get_battery_voltage().ok()?;
    // VBUS検出(get_vbus_present)ではなく充電電流の向き(get_charging)を見る。
    // USBを挿したまま満充電になると充電は止まるのにVBUSは残るため、
    // VBUS判定では満充電後も「CHG」が表示され続けた(Issue #153)。
    let charging = axp.get_charging().unwrap_or(false);
    // Core2のUSB給電はACINに来る(VBUSはM-Bus由来でUSB抜き挿しと無関係)。
    // 両方見るのはM-Bus給電にも対応するため。
    let powered =
        axp.get_acin_present().unwrap_or(false) || axp.get_vbus_present().unwrap_or(false);
    Some(Battery {
        percent: battery::battery_percent(volts, powered, charging),
        charging,
        powered,
    })
}

impl Battery {
    /// 画面の再描画判定に使う値だけを抜き出す。
    pub fn display_state(&self) -> battery::DisplayState {
        battery::DisplayState {
            percent: self.percent,
            powered: self.powered,
            charging: self.charging,
        }
    }
}

/// 給電・充電系の割り込みだけを有効化する。
///
/// `axp192` crate 0.2.0には割り込み設定APIが無い(lib.rs全522行を確認。
/// enable/statusレジスタへの言及自体が無い)ため、生レジスタ書き込みになる。
/// マスク定義は `battery::power_irq` に寄せてあり、ビットの根拠はそちらに書いた。
/// 有効化は既存値へのOR(read-modify-write)で行い、ボタン(PEK)など
/// 他用途の有効ビットを殺さない(M5UnifiedはCore2初期化で電源系を全無効にするが、
/// ここでは既存設定を温存する)。
///
/// GPIO割り込み(ISR)は設定しない。量産Core2のAXP192 IRQピンはESP32の
/// どのGPIOにも未接続で(M5Stack公式フォーラムtopic/2600でM5技術者が回答、
/// 公式回路図CORE2_V1.0_SCHでも別ネット、M-BusのG35はADC用途)、stock実機では
/// 立ち下がりが来ない。改造で配線した個体向けの土台としてenableだけ行い、
/// 検出はメインループのラッチ確認(`power_event_pending`)で行う。
/// ISRから共有I2Cバス(ft6x36と共用)を触ると壊れるため、I2C読み出しと
/// 描画は従来どおりメインループが担当する。
pub fn enable_power_irqs<I2C, E>(i2c: &mut I2C) -> Result<(), E>
where
    I2C: embedded_hal::i2c::I2c<Error = E>,
{
    for (reg, bits) in battery::power_irq::ENABLE_UPDATES {
        let mut current = [0u8];
        i2c.write_read(AXP192_ADDRESS, &[reg], &mut current)?;
        i2c.write(AXP192_ADDRESS, &[reg, current[0] | bits])?;
    }
    Ok(())
}

/// 給電・充電系のIRQ状態ラッチをクリアする。
///
/// AXP192は該当ビットへ1を書くとクリアされる(write-1-to-clear)。
/// 有効化したビットだけを書き、他用途のラッチは残す。
/// クリアしないとラッチが出っぱなしになり、次の抜き挿しを見分けられない。
/// `read_battery` の直後に呼ぶこと。
pub fn clear_power_irq_status<I2C, E>(i2c: &mut I2C) -> Result<(), E>
where
    I2C: embedded_hal::i2c::I2c<Error = E>,
{
    for (reg, bits) in battery::power_irq::STATUS_CLEAR {
        i2c.write(AXP192_ADDRESS, &[reg, bits])?;
    }
    Ok(())
}

/// 給電・充電系のIRQ状態ラッチに未処理のイベントがあるかを読む。
///
/// ラッチはエッジの記憶なので、短い抜き挿しでも次の確認まで残る。
/// I2Cが応答しない場合はErrにし、呼び出し側は安全側(読む側)に倒すこと。
pub fn power_event_pending<I2C, E>(i2c: &mut I2C) -> Result<bool, E>
where
    I2C: embedded_hal::i2c::I2c<Error = E>,
{
    let mut status44 = [0u8];
    let mut status45 = [0u8];
    i2c.write_read(AXP192_ADDRESS, &[0x44], &mut status44)?;
    i2c.write_read(AXP192_ADDRESS, &[0x45], &mut status45)?;
    // マスクと判定式はhostテスト済みの純粋ロジックを使う。
    debug_assert_eq!(
        battery::power_irq::STATUS_MASK,
        battery::power_irq::STATUS_CLEAR,
        "status regs must match clear regs"
    );
    Ok(battery::power_irq::is_pending(status44[0], status45[0]))
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

/// SPI経由でILI9342Cを初期化する。LCDリセットはAXP192 GPIO4側で行うため、
/// 先に `init_power` を実行しておく。
pub fn init_display<'d>(
    spi: SPI2,
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
        .baudrate(40.MHz().into())
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
    // Core2のタッチ座標は画面座標と一致するため、デフォルト向きのまま使う。
    Ft6x36::new(
        RefCellDevice::new(bus),
        ft6x36::Dimension(TOUCH_WIDTH, TOUCH_HEIGHT),
    )
}
