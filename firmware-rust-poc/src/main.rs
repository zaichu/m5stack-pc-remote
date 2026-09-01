mod agent;
mod board;
mod config;
mod net;
mod ui;

use std::cell::RefCell;
use std::error::Error;
use std::time::{Duration, Instant};

use esp_idf_hal::peripherals::Peripherals;

use agent::PowerAction;
use board::{DisplayPins, DISPLAY_HEIGHT, DISPLAY_WIDTH};
use config::{PC_MAC_ADDRESS, PC_STATUS_ADDR, WIFI_PASSWORD, WIFI_SSID, WOL_PORT};
use ui::{Status, TelegramState};

const STATUS_INTERVAL: Duration = Duration::from_secs(10);
const STATUS_PROBE_TIMEOUT: Duration = Duration::from_millis(800);
const WIFI_RECONNECT_INTERVAL: Duration = Duration::from_secs(15);
const TOAST_TTL: Duration = Duration::from_secs(3);

/// Which screen the touch UI is currently showing.
enum Screen {
    Main,
    Confirm(PowerAction),
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
    match touch.init() {
        Ok(()) => println!("touch initialized: info={:?}", touch.get_info()),
        Err(e) => println!("touch init failed: {e:?}"),
    }

    let mut status = Status {
        wifi_connected: false,
        pc_online: false,
        telegram: TelegramState::Disabled,
        toast: None,
    };
    let mut toast_text: Option<String> = None;
    ui::draw_main(
        &mut display,
        &Status {
            toast: Some("connecting Wi-Fi..."),
            ..status
        },
    )?;

    // Held for the whole program: dropping it tears down the connection.
    let mut wifi = match net::Wifi::connect(peripherals.modem, WIFI_SSID, WIFI_PASSWORD) {
        Ok(wifi) => {
            println!("Wi-Fi connected");
            Some(wifi)
        }
        Err(e) => {
            println!("Wi-Fi connect failed: {e}");
            None
        }
    };
    status.wifi_connected = wifi.as_ref().is_some_and(net::Wifi::is_up);

    // The Windows Agent rejects requests whose timestamp is outside its clock
    // skew window, so the clock has to be real before REBOOT/SHUTDOWN work.
    let _sntp = if status.wifi_connected {
        match net::start_sntp() {
            Ok(sntp) => {
                println!("SNTP started");
                Some(sntp)
            }
            Err(e) => {
                println!("SNTP start failed: {e}");
                None
            }
        }
    } else {
        None
    };

    // REBOOT/SHUTDOWN carry a timestamp the agent checks against its own
    // clock, so wait for the first NTP sync before the UI offers them.
    if status.wifi_connected {
        let synced = net::wait_for_time_sync(Duration::from_secs(15));
        println!("SNTP synced: {synced}");
    }

    ui::draw_main(&mut display, &status)?;

    let mut screen = Screen::Main;
    let mut status_at = Instant::now();
    let mut wifi_check_at = Instant::now();
    let mut toast_at = Instant::now();
    let mut touch_was_down = false;

    loop {
        // Re-associate if the link dropped, rate-limited like the C++ firmware.
        if wifi_check_at.elapsed() >= WIFI_RECONNECT_INTERVAL {
            wifi_check_at = Instant::now();
            if let Some(w) = wifi.as_mut() {
                if !w.is_up() {
                    println!("Wi-Fi down; reconnecting");
                    match w.reconnect() {
                        Ok(()) => println!("Wi-Fi reconnected"),
                        Err(e) => println!("Wi-Fi reconnect failed: {e}"),
                    }
                }
                let now_connected = w.is_up();
                if now_connected != status.wifi_connected {
                    status.wifi_connected = now_connected;
                    if !status.wifi_connected {
                        status.pc_online = false;
                    }
                    if matches!(screen, Screen::Main) {
                        ui::draw_main(&mut display, &with_toast(&status, &toast_text))?;
                    }
                }
            }
        }

        if status.wifi_connected && status_at.elapsed() >= STATUS_INTERVAL {
            status_at = Instant::now();
            let now_online = net::check_pc_online(PC_STATUS_ADDR, STATUS_PROBE_TIMEOUT);
            if now_online != status.pc_online {
                status.pc_online = now_online;
                println!("PC status changed: online={}", status.pc_online);
            }
            if matches!(screen, Screen::Main) {
                ui::draw_main(&mut display, &with_toast(&status, &toast_text))?;
            }
        }

        // Rising-edge detection so one tap triggers exactly one action.
        let touch_point = match touch.get_touch_event() {
            Ok(event) => event.p1.map(|p| (p.x as i32, p.y as i32)),
            Err(_) => None,
        };
        let touch_down = touch_point.is_some();

        if let Some((x, y)) = touch_point {
            if !touch_was_down {
                match screen {
                    Screen::Main => {
                        if ui::WAKE_BUTTON.contains(x, y) {
                            println!("WAKE tapped at x={x} y={y}");
                            toast_text =
                                Some(match net::send_wake_on_lan(PC_MAC_ADDRESS, WOL_PORT) {
                                    Ok(()) => {
                                        println!("WOL sent");
                                        "Magic Packet sent".to_string()
                                    }
                                    Err(e) => {
                                        println!("WOL failed: {e}");
                                        "WOL failed".to_string()
                                    }
                                });
                            toast_at = Instant::now();
                            ui::draw_main(&mut display, &with_toast(&status, &toast_text))?;
                        } else if status.pc_online && ui::REBOOT_BUTTON.contains(x, y) {
                            screen = Screen::Confirm(PowerAction::Reboot);
                            ui::draw_confirm(&mut display, PowerAction::Reboot)?;
                        } else if status.pc_online && ui::SHUTDOWN_BUTTON.contains(x, y) {
                            screen = Screen::Confirm(PowerAction::Shutdown);
                            ui::draw_confirm(&mut display, PowerAction::Shutdown)?;
                        }
                    }
                    Screen::Confirm(action) => {
                        if ui::CANCEL_BUTTON.contains(x, y) {
                            println!("{} cancelled", action.slug());
                            screen = Screen::Main;
                            toast_text = None;
                            ui::draw_main(&mut display, &status)?;
                        } else if ui::OK_BUTTON.contains(x, y) {
                            println!("{} confirmed", action.slug());
                            toast_text = Some(match agent::send_command(action) {
                                Ok(code) if agent::is_accepted(code) => "Command accepted".into(),
                                Ok(code) => format!("Command rejected ({code})"),
                                Err(e) => {
                                    println!("agent command failed: {e}");
                                    "Command failed".to_string()
                                }
                            });
                            toast_at = Instant::now();
                            screen = Screen::Main;
                            ui::draw_main(&mut display, &with_toast(&status, &toast_text))?;
                        }
                    }
                }
            }
        }
        touch_was_down = touch_down;

        if toast_text.is_some() && toast_at.elapsed() >= TOAST_TTL {
            toast_text = None;
            if matches!(screen, Screen::Main) {
                ui::draw_main(&mut display, &status)?;
            }
        }

        std::thread::sleep(Duration::from_millis(20));
    }
}

fn with_toast<'a>(status: &Status<'a>, toast: &'a Option<String>) -> Status<'a> {
    Status {
        toast: toast.as_deref(),
        ..*status
    }
}
