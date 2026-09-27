// M5Stack Core2(初代、AXP192)のハードウェア初期化。
// ピン配置と電源投入手順の根拠はM5GFXのCore2実装とaxp192 crateのexample。
//   LCD (ILI9342C, 320x240): MOSI=23, MISO=38, SCLK=18, DC=15, CS=5
//   LCD reset:     AXP192 GPIO4
//   LCD power:     AXP192 LDO2  @ 3300mV固定(LCD+タッチ共用のため変更しない)
//   LCD backlight: AXP192 DCDC3(0〜100%を電圧へ変換。100%で2800mV)
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
use esp_idf_hal::spi::{Dma, SpiDeviceDriver, SpiDriver, SPI2};
use ft6x36::Ft6x36;
use mipidsi::interface::SpiInterface;
use mipidsi::models::ILI9342CRgb565;
use mipidsi::options::{ColorInversion, Orientation, Rotation};
use mipidsi::Builder;

/// mipidsiのSPI転送バッファ。小さいと全画面描画が数千トランザクションに分割される
/// (Issue #160)。4096Bなら38回程度で、これ以上は転送時間が支配的で効果が薄い。
const SPI_BUFFER_SIZE: usize = 4096;

/// `SPI_BUFFER_SIZE` と同じにし、mipidsiの1回の書き込みを分割させない。
/// `Dma::max_transfer_size` の契約上4の倍数必須。
const SPI_DMA_MAX_TRANSFER_SIZE: usize = 4096;

// DMA契約違反を実行時panicや実機でのDMA失敗ではなくビルド時に検出する。
const _: () = assert!(
    SPI_BUFFER_SIZE % 4 == 0,
    "SPI_BUFFER_SIZE must be a multiple of 4 for DMA"
);
const _: () = assert!(
    SPI_DMA_MAX_TRANSFER_SIZE % 4 == 0,
    "SPI_DMA_MAX_TRANSFER_SIZE must be a multiple of 4 (Dma::max_transfer_size)"
);
const _: () = assert!(
    SPI_BUFFER_SIZE <= SPI_DMA_MAX_TRANSFER_SIZE,
    "SPI_BUFFER_SIZE must fit in one DMA transaction"
);

pub const DISPLAY_WIDTH: u16 = 320;
pub const DISPLAY_HEIGHT: u16 = 240;

/// Core2のタッチ範囲は画面より縦に広い。y=240..279は画面下の物理ボタン帯。
pub const TOUCH_WIDTH: u16 = 320;
pub const TOUCH_HEIGHT: u16 = 280;

// IRQの生レジスタアクセスで使うスレーブアドレス(axp192/ft6x36と同じ内部I2Cバス)。
const AXP192_ADDRESS: u8 = 0x34;

pub type SharedI2c<'d> = RefCell<I2cDriver<'d>>;

/// LCDとタッチを使う前に必要なAXP192電源投入手順。
///
/// `brightness_percent` は保存済み設定(0〜100)を渡す。DCDC3(バックライト)をここで
/// 設定済み電圧にし、起動直後から正しい明るさにする。起動後の変更反映は
/// `apply_brightness` が担当(UIループが設定変化を検出して呼ぶ)。
pub fn init_power<I2C, E>(axp: &mut Axp192<I2C>, brightness_percent: u8) -> Result<(), E>
where
    I2C: embedded_hal::i2c::I2c<Error = E>,
{
    axp.set_dcdc1_voltage(3350)?; // ESP32 VDD
    axp.set_ldo2_voltage(3300)?; // LCD + touch power
    axp.set_ldo2_on(true)?;
    axp.set_ldo3_voltage(2000)?; // vibration motor
    axp.set_ldo3_on(false)?;
    apply_brightness(axp, brightness_percent)?;

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

    axp.set_gpio4_output(false)?;
    FreeRtos::delay_ms(100);
    axp.set_gpio4_output(true)?;
    FreeRtos::delay_ms(100);

    Ok(())
}

/// バックライト(DCDC3)の電圧を明るさ設定へ合わせる(Issue #167)。
///
/// LDO2(LCD+タッチ電源、3300mV固定)は絶対に触らない。下げると表示とタッチが両方壊れる。
/// 変換式と上下限の根拠は `config_validation::brightness_percent_to_dcdc3_mv` を参照。
pub fn apply_brightness<I2C, E>(axp: &mut Axp192<I2C>, percent: u8) -> Result<(), E>
where
    I2C: embedded_hal::i2c::I2c<Error = E>,
{
    axp.set_dcdc3_voltage(config_validation::brightness_percent_to_dcdc3_mv(percent))?;
    axp.set_dcdc3_on(true)?;
    Ok(())
}

/// DCDC3のon/offだけを切り替える(Issue #168のスリープ機能用)。
/// 電圧(=明るさ設定)は保持されるため `apply_brightness` で直前の明るさへ復帰する。
/// LDO2は触らない(消灯中もタッチ復帰検出を生かすため)。
pub fn set_backlight_on<I2C, E>(axp: &mut Axp192<I2C>, on: bool) -> Result<(), E>
where
    I2C: embedded_hal::i2c::I2c<Error = E>,
{
    axp.set_dcdc3_on(on)
}

/// AXP192の読み値を `battery` crateの純粋ロジックで残量へ概算したもの。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    pub percent: u8,
    /// 充電電流の向き。満充電で充電停止するとUSB接続中でもfalseになる。
    pub charging: bool,
    /// ACIN/VBUSの外部給電。「給電中だが充電していない」を区別するため別に持つ。
    pub powered: bool,
}

/// バッテリー電圧と充電状態を読む。I2Cが応答しない場合はNone。
pub fn read_battery<I2C, E>(axp: &mut Axp192<I2C>) -> Option<Battery>
where
    I2C: embedded_hal::i2c::I2c<Error = E>,
{
    let volts = axp.get_battery_voltage().ok()?;
    // VBUSではなく充電電流の向きを見る。満充電後もVBUSは残るため、
    // VBUS判定では「CHG」が消えなかった(Issue #153)。
    let charging = axp.get_charging().unwrap_or(false);
    // USB給電はACINに来る(VBUSはM-Bus由来)。M-Bus給電にも対応するため両方見る。
    let powered =
        axp.get_acin_present().unwrap_or(false) || axp.get_vbus_present().unwrap_or(false);
    Some(Battery {
        percent: battery::battery_percent(volts, powered, charging),
        charging,
        powered,
    })
}

impl Battery {
    pub fn display_state(&self) -> battery::DisplayState {
        battery::DisplayState {
            percent: self.percent,
            powered: self.powered,
            charging: self.charging,
        }
    }
}

/// 給電・充電系の割り込みだけを有効化する。`axp192` crate 0.2.0に割り込み設定APIが
/// 無いため生レジスタをread-modify-writeし、他用途の有効ビットは殺さない。
///
/// GPIO割り込み(ISR)は使えない: 量産Core2のAXP192 IRQピンはESP32のGPIOへ未接続
/// (M5フォーラムtopic/2600、回路図CORE2_V1.0_SCH)。検出はラッチのポーリングで行う。
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

/// IRQ状態ラッチをクリアする(write-1-to-clear)。有効化したビットだけを書き、
/// 他用途のラッチは残す。クリアしないと次の抜き挿しを見分けられない。
pub fn clear_power_irq_status<I2C, E>(i2c: &mut I2C) -> Result<(), E>
where
    I2C: embedded_hal::i2c::I2c<Error = E>,
{
    for (reg, bits) in battery::power_irq::STATUS_CLEAR {
        i2c.write(AXP192_ADDRESS, &[reg, bits])?;
    }
    Ok(())
}

/// IRQラッチに未処理イベントがあるか。ラッチはエッジを記憶するため短い抜き挿しも残る。
/// I2Cが応答しない場合はErr(呼び出し側は安全側へ倒す)。
pub fn power_event_pending<I2C, E>(i2c: &mut I2C) -> Result<bool, E>
where
    I2C: embedded_hal::i2c::I2c<Error = E>,
{
    let mut status44 = [0u8];
    let mut status45 = [0u8];
    i2c.write_read(AXP192_ADDRESS, &[0x44], &mut status44)?;
    i2c.write_read(AXP192_ADDRESS, &[0x45], &mut status45)?;
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

/// DMA転送用バッファを内部DRAMに確保する。`Box::new` ではPSRAMへ確保され得て
/// SPI DMAが使えないため、`MALLOC_CAP_DMA` で固定する(ESP-IDF公式)。
fn alloc_dma_buffer() -> &'static mut [u8] {
    let ptr = unsafe {
        esp_idf_sys::heap_caps_malloc(SPI_BUFFER_SIZE, esp_idf_sys::MALLOC_CAP_DMA) as *mut u8
    };
    assert!(!ptr.is_null(), "SPI DMA buffer allocation failed");
    // 端数送出時のゴミ転送を避けるためゼロ初期化(起動時1回だけのコスト)。
    unsafe {
        core::ptr::write_bytes(ptr, 0, SPI_BUFFER_SIZE);
        core::slice::from_raw_parts_mut(ptr, SPI_BUFFER_SIZE)
    }
}

/// LCDリセットはAXP192 GPIO4側で行うため、先に `init_power` が必要。
pub fn init_display<'d>(
    spi: SPI2,
    pins: DisplayPins,
) -> Result<Core2Display<'d>, Box<dyn std::error::Error>> {
    // MISOは使わない。設定するとfull-duplex扱いになりSPI clockが26.7MHzへ制限される。
    // DMA無効だと1トランザクション64B上限で全画面が約2,400回に分割される(Issue #160)。
    // チャネル選択は `Dma::Auto` でESP-IDFへ任せる。
    let spi_driver = SpiDriver::new(
        spi,
        pins.sclk,
        pins.mosi,
        None::<AnyIOPin>,
        &DriverConfig::new().dma(Dma::Auto(SPI_DMA_MAX_TRANSFER_SIZE)),
    )?;

    // 読み取りはしないためwrite-onlyで駆動(M5GFXも同方針で40MHz)。
    let spi_config = SpiConfig::new()
        .baudrate(40.MHz().into())
        .write_only(true)
        .duplex(Duplex::Half3Wire);
    let spi_device = SpiDeviceDriver::new(spi_driver, Some(pins.cs), &spi_config)?;

    let dc = PinDriver::output(pins.dc)?;
    // DMA転送には内部DRAM確保が必須なため `Box::leak` ではなくstaticバッファを使う。
    let buffer: &'static mut [u8] = alloc_dma_buffer();
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
