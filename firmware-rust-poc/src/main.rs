mod agent;
mod board;
mod net;
mod telegram;
mod telegram_root_ca;
mod ui;

/// Generated at build time from the git-ignored `config.toml` (see
/// `config.example.toml`), never written into `src/` as Rust source. See
/// `build.rs` and Issue #21: a literal secret in `src/` could leak into
/// build logs via a compiler warning printing the offending source line.
mod config {
    include!(concat!(env!("OUT_DIR"), "/generated_config.rs"));
}

use std::cell::RefCell;
use std::error::Error;
use std::sync::{Arc, Mutex};
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

/// Starts the services that only make sense once Wi-Fi is up (SNTP and the
/// Telegram poller). Idempotent: safe to call every time the link comes up,
/// including after a drop-and-reconnect, because `sntp`/`telegram_started`
/// remember what has already been started. Called both right after the
/// initial connection attempt and from the reconnect loop, so that Telegram
/// still starts even when Wi-Fi was down at boot and only recovers later.
///
/// Does not wait for the NTP sync to complete: this runs on the touch UI
/// loop's thread, which must keep polling touch/STATUS every ~20ms on this
/// 24/7 device. `agent::send_command` already refuses to run on an unsynced
/// clock, and the Telegram thread (`telegram::Client::run`) does its own
/// bounded wait on its own thread before polling, so neither needs the UI
/// loop to block here.
fn start_online_services(
    sntp: &mut Option<esp_idf_svc::sntp::EspSntp<'static>>,
    telegram_started: &mut bool,
    power_lock: &telegram::PowerLock,
    telegram_state: &Arc<Mutex<telegram::State>>,
) {
    if sntp.is_none() {
        // The Windows Agent rejects requests whose timestamp is outside its
        // clock skew window, so the clock has to be real before
        // REBOOT/SHUTDOWN work. Starting it here is enough; see the doc
        // comment above for why we don't also wait for sync here.
        match net::start_sntp() {
            Ok(started) => {
                println!("SNTP started");
                *sntp = Some(started);
            }
            Err(e) => println!("SNTP start failed: {e}"),
        }
    }

    if !*telegram_started && telegram::is_configured() {
        let client = telegram::Client::new(Arc::clone(power_lock));
        let state_handle = Arc::clone(telegram_state);
        // Own thread: a long poll blocks for TELEGRAM_LONG_POLL_TIMEOUT_SECONDS,
        // which must never stall the touch UI or the STATUS refresh.
        match std::thread::Builder::new()
            .stack_size(12 * 1024)
            .spawn(move || client.run(state_handle))
        {
            Ok(_) => {
                *telegram_started = true;
                println!("telegram: polling task started");
            }
            Err(e) => println!("telegram: failed to start polling thread: {e}"),
        }
    }
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

    // Serializes power actions between this thread and the Telegram thread,
    // the role the FreeRTOS mutex plays in the C++ PowerController.
    let power_lock: telegram::PowerLock = Arc::new(Mutex::new(()));
    let telegram_state = Arc::new(Mutex::new(telegram::State::Disabled));
    let mut sntp: Option<esp_idf_svc::sntp::EspSntp<'static>> = None;
    let mut telegram_started = false;
    if !telegram::is_configured() {
        println!("telegram: disabled (token or user id is a placeholder)");
    }

    // Held for the whole program: dropping it tears down the connection. If
    // this fails, `wifi_check_at` below retries every WIFI_RECONNECT_INTERVAL
    // instead of leaving the device offline for good.
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
    if status.wifi_connected {
        start_online_services(
            &mut sntp,
            &mut telegram_started,
            &power_lock,
            &telegram_state,
        );
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
            match wifi.as_mut() {
                Some(w) => {
                    if !w.is_up() {
                        println!("Wi-Fi down; reconnecting");
                        match w.reconnect() {
                            Ok(()) => println!("Wi-Fi reconnected"),
                            Err(e) => println!("Wi-Fi reconnect failed: {e}"),
                        }
                    }
                }
                None => {
                    // The initial connect (or a previous retry) failed and
                    // dropped its Modem; re-acquire one and try again so a
                    // failed boot connection is not permanent.
                    println!("Wi-Fi never connected; retrying");
                    match net::Wifi::connect_retry(WIFI_SSID, WIFI_PASSWORD) {
                        Ok(w) => {
                            println!("Wi-Fi connected");
                            wifi = Some(w);
                        }
                        Err(e) => println!("Wi-Fi connect retry failed: {e}"),
                    }
                }
            }

            let now_connected = wifi.as_ref().is_some_and(net::Wifi::is_up);
            if now_connected != status.wifi_connected {
                status.wifi_connected = now_connected;
                if !status.wifi_connected {
                    status.pc_online = false;
                }
                if matches!(screen, Screen::Main) {
                    ui::draw_main(&mut display, &with_toast(&status, &toast_text))?;
                }
            }
            if now_connected {
                start_online_services(
                    &mut sntp,
                    &mut telegram_started,
                    &power_lock,
                    &telegram_state,
                );
            }
        }

        let now_telegram = match *telegram_state.lock().unwrap() {
            telegram::State::Disabled => TelegramState::Disabled,
            telegram::State::Polling => TelegramState::Polling,
            telegram::State::Error => TelegramState::Error,
        };
        if now_telegram != status.telegram {
            status.telegram = now_telegram;
            if matches!(screen, Screen::Main) {
                ui::draw_main(&mut display, &with_toast(&status, &toast_text))?;
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
                            let _guard = power_lock.lock().unwrap();
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
                            let _guard = power_lock.lock().unwrap();
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
