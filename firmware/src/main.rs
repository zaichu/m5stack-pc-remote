mod agent;
mod app_config;
mod board;
mod net;
mod telegram;
mod telegram_root_ca;
mod ui;

/// Git管理外の `config.toml` からビルド時に生成する設定。
/// secretを `src/` 配下のRustソースへ直接置かないことで、コンパイラ警告による
/// ビルドログ漏えいを防ぐ。
mod config {
    include!(concat!(env!("OUT_DIR"), "/generated_config.rs"));
}

use std::cell::RefCell;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use esp_idf_hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;

use agent::PowerAction;
use app_config::AppConfig;
use board::{DisplayPins, DISPLAY_HEIGHT, DISPLAY_WIDTH};
use ui::{Status, TelegramState};

const STATUS_INTERVAL: Duration = Duration::from_secs(10);
const STATUS_PROBE_TIMEOUT: Duration = Duration::from_millis(800);
const WIFI_RECONNECT_INTERVAL: Duration = Duration::from_secs(15);
const TOAST_TTL: Duration = Duration::from_secs(3);

/// タッチUIの現在画面。
enum Screen {
    Main,
    Confirm(PowerAction),
}

/// Wi-Fi接続後にだけ意味があるサービス(SNTPとTelegram poller)を開始する。
/// 既に開始済みなら何もしないため、再接続時に何度呼んでもよい。
///
/// NTP同期完了までは待たない。ここはUIループ上で動くため、STATUS更新やタッチ処理を
/// 止めないことを優先する。電源操作側で未同期時計は拒否する。
fn start_online_services(
    sntp: &mut Option<esp_idf_svc::sntp::EspSntp<'static>>,
    telegram_started: &mut bool,
    power_lock: &telegram::PowerLock,
    telegram_state: &Arc<Mutex<telegram::State>>,
    app_config: &Arc<AppConfig>,
) {
    if sntp.is_none() {
        // Windows Agentはtimestampを検証するため、電源操作前に時計同期が必要になる。
        // ここではSNTP開始だけ行い、同期待ちは別スレッド側に任せる。
        match net::start_sntp() {
            Ok(started) => {
                println!("SNTP started");
                *sntp = Some(started);
            }
            Err(e) => println!("SNTP start failed: {e}"),
        }
    }

    if !*telegram_started && telegram::is_configured(app_config.as_ref()) {
        let client = telegram::Client::new(Arc::clone(power_lock), Arc::clone(app_config));
        let state_handle = Arc::clone(telegram_state);
        // long pollingでUIやSTATUS更新を止めないよう、Telegramは専用スレッドで動かす。
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

    println!("m5remote-rust boot (pure Rust stack)");

    let peripherals = Peripherals::take()?;
    let nvs_partition = EspDefaultNvsPartition::take()?;
    let app_config = Arc::new(AppConfig::load(nvs_partition.clone()));

    // AXP192とタッチコントローラーは同じI2Cバスを共有する。
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

    // UI操作とTelegram操作からの電源操作を直列化する。
    let power_lock: telegram::PowerLock = Arc::new(Mutex::new(()));
    let telegram_state = Arc::new(Mutex::new(telegram::State::Disabled));
    let mut sntp: Option<esp_idf_svc::sntp::EspSntp<'static>> = None;
    let mut telegram_started = false;
    if !telegram::is_configured(app_config.as_ref()) {
        println!("telegram: disabled (token or user id is a placeholder)");
    }

    // Wifiハンドルは接続維持に必要なのでプログラム終了まで保持する。
    // 初回接続に失敗しても、下の再接続処理で定期的に復旧を試す。
    let mut wifi = match net::Wifi::connect(
        peripherals.modem,
        nvs_partition.clone(),
        &app_config.wifi_ssid,
        &app_config.wifi_password,
    ) {
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
            &app_config,
        );
    }

    ui::draw_main(&mut display, &status)?;

    let mut screen = Screen::Main;
    let mut status_at = Instant::now();
    let mut wifi_check_at = Instant::now();
    let mut toast_at = Instant::now();
    let mut touch_was_down = false;

    loop {
        // Wi-Fi切断時は一定間隔で再接続を試す。
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
                    // 初回接続失敗時はModemも破棄されるため、再取得して接続を試す。
                    println!("Wi-Fi never connected; retrying");
                    match net::Wifi::connect_retry(
                        nvs_partition.clone(),
                        &app_config.wifi_ssid,
                        &app_config.wifi_password,
                    ) {
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
                    &app_config,
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
            let now_online = net::check_pc_online(&app_config.pc_status_addr, STATUS_PROBE_TIMEOUT);
            if now_online != status.pc_online {
                status.pc_online = now_online;
                println!("PC status changed: online={}", status.pc_online);
            }
            if matches!(screen, Screen::Main) {
                ui::draw_main(&mut display, &with_toast(&status, &toast_text))?;
            }
        }

        // タッチの立ち上がりだけを見ることで、1回のタップで1回だけ実行する。
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
                            toast_text = Some(
                                match net::send_wake_on_lan(
                                    &app_config.pc_mac_address,
                                    app_config.wol_port,
                                ) {
                                    Ok(()) => {
                                        println!("WOL sent");
                                        "Magic Packet送信".to_string()
                                    }
                                    Err(e) => {
                                        println!("WOL failed: {e}");
                                        "WOL失敗".to_string()
                                    }
                                },
                            );
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
                            toast_text =
                                Some(match agent::send_command(action, app_config.as_ref()) {
                                    Ok(code) if agent::is_accepted(code) => {
                                        "操作を受け付けました".into()
                                    }
                                    Ok(code) => format!("操作が拒否されました ({code})"),
                                    Err(e) => {
                                        println!("agent command failed: {e}");
                                        "操作に失敗しました".to_string()
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
