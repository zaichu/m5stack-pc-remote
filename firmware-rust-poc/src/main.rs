mod board;
mod config;
mod net;

use std::cell::RefCell;
use std::error::Error;
use std::time::{Duration, Instant};

use embedded_graphics::mono_font::ascii::{FONT_10X20, FONT_6X10, FONT_9X18_BOLD};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::Text;
use esp_idf_hal::peripherals::Peripherals;

use board::{Core2Display, DisplayPins, DISPLAY_HEIGHT, DISPLAY_WIDTH};
use config::{PC_MAC_ADDRESS, PC_STATUS_ADDR, WIFI_PASSWORD, WIFI_SSID, WOL_PORT};

const STATUS_INTERVAL: Duration = Duration::from_secs(10);
const STATUS_PROBE_TIMEOUT: Duration = Duration::from_millis(800);

/// Full-width band at the bottom of the screen that acts as the WAKE button.
const WAKE_BUTTON_TOP: i32 = 180;

fn draw_screen(
    display: &mut Core2Display<'_>,
    wifi_connected: bool,
    pc_online: bool,
    toast: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    display
        .clear(Rgb565::BLACK)
        .map_err(|e| format!("clear failed: {e:?}"))?;

    let title = MonoTextStyle::new(&FONT_6X10, Rgb565::CSS_LIGHT_GRAY);
    Text::new("m5remote-rust-poc", Point::new(8, 14), title)
        .draw(display)
        .map_err(|e| format!("draw failed: {e:?}"))?;

    let wifi_style = MonoTextStyle::new(
        &FONT_6X10,
        if wifi_connected {
            Rgb565::GREEN
        } else {
            Rgb565::RED
        },
    );
    Text::new(
        if wifi_connected {
            "Wi-Fi: connected"
        } else {
            "Wi-Fi: disconnected"
        },
        Point::new(8, 32),
        wifi_style,
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;

    let status_style = MonoTextStyle::new(
        &FONT_10X20,
        if pc_online {
            Rgb565::GREEN
        } else {
            Rgb565::RED
        },
    );
    Text::new(
        if pc_online { "ONLINE" } else { "OFFLINE" },
        Point::new(110, 100),
        status_style,
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;

    // WAKE button
    Rectangle::new(
        Point::new(8, WAKE_BUTTON_TOP),
        Size::new(DISPLAY_WIDTH as u32 - 16, 48),
    )
    .into_styled(PrimitiveStyle::with_fill(Rgb565::CSS_DARK_GREEN))
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;
    Text::new(
        "WAKE (touch here)",
        Point::new(80, WAKE_BUTTON_TOP + 30),
        MonoTextStyle::new(&FONT_9X18_BOLD, Rgb565::WHITE),
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;

    if let Some(text) = toast {
        Text::new(
            text,
            Point::new(8, DISPLAY_HEIGHT as i32 - 8),
            MonoTextStyle::new(&FONT_6X10, Rgb565::YELLOW),
        )
        .draw(display)
        .map_err(|e| format!("draw failed: {e:?}"))?;
    }

    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    esp_idf_sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    println!("m5remote-rust-poc boot (pure Rust stack)");

    let peripherals = Peripherals::take()?;

    // AXP192 and the touch controller share one I2C bus.
    let i2c = board::new_i2c(
        peripherals.i2c0,
        peripherals.pins.gpio21.into(),
        peripherals.pins.gpio22.into(),
    )?;
    let i2c_bus = RefCell::new(i2c);

    let mut axp = board::new_axp(&i2c_bus);
    board::init_power(&mut axp).map_err(|e| format!("AXP192 init failed: {e:?}"))?;
    println!("AXP192 initialized");

    let mut display = board::init_display(
        peripherals.spi2,
        DisplayPins {
            sclk: peripherals.pins.gpio18,
            mosi: peripherals.pins.gpio23,
            dc: peripherals.pins.gpio15,
            cs: peripherals.pins.gpio5,
        },
    )?;
    println!("display initialized: {DISPLAY_WIDTH}x{DISPLAY_HEIGHT}");

    let mut touch = board::new_touch(&i2c_bus);
    if let Err(e) = touch.init() {
        println!("touch init failed: {e:?}");
    }

    draw_screen(&mut display, false, false, Some("connecting Wi-Fi..."))?;

    // Held for the whole program: dropping it tears down the connection.
    let _wifi = match net::connect_wifi(peripherals.modem, WIFI_SSID, WIFI_PASSWORD) {
        Ok(wifi) => {
            println!("Wi-Fi connected");
            Some(wifi)
        }
        Err(e) => {
            println!("Wi-Fi connect failed: {e}");
            None
        }
    };
    let wifi_connected = _wifi.is_some();

    let mut pc_online = false;
    let mut toast: Option<String> = None;
    draw_screen(&mut display, wifi_connected, pc_online, None)?;

    let mut status_at = Instant::now();
    let mut redraw_at = Instant::now();
    let mut touch_was_down = false;

    loop {
        if wifi_connected && status_at.elapsed() >= STATUS_INTERVAL {
            status_at = Instant::now();
            let now_online = net::check_pc_online(PC_STATUS_ADDR, STATUS_PROBE_TIMEOUT);
            if now_online != pc_online {
                pc_online = now_online;
                println!("PC status changed: online={pc_online}");
            }
            draw_screen(&mut display, wifi_connected, pc_online, toast.as_deref())?;
        }

        // Rising-edge detection so one tap sends exactly one magic packet.
        let touch_down = match touch.get_touch_event() {
            Ok(event) => match event.p1 {
                Some(p) => {
                    if !touch_was_down && (p.y as i32) >= WAKE_BUTTON_TOP {
                        println!("WAKE tapped at x={} y={}", p.x, p.y);
                        toast = Some(match net::send_wake_on_lan(PC_MAC_ADDRESS, WOL_PORT) {
                            Ok(()) => {
                                println!("WOL sent");
                                "Magic Packet sent".to_string()
                            }
                            Err(e) => {
                                println!("WOL failed: {e}");
                                "WOL failed".to_string()
                            }
                        });
                        draw_screen(&mut display, wifi_connected, pc_online, toast.as_deref())?;
                        redraw_at = Instant::now();
                    }
                    true
                }
                None => false,
            },
            Err(_) => false,
        };
        touch_was_down = touch_down;

        // Clear the toast a few seconds after it was shown.
        if toast.is_some() && redraw_at.elapsed() >= Duration::from_secs(3) {
            toast = None;
            draw_screen(&mut display, wifi_connected, pc_online, None)?;
        }

        std::thread::sleep(Duration::from_millis(20));
    }
}
