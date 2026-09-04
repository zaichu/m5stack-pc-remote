mod app_config;
mod board;
mod bridge_client;
mod net;
mod ota;
mod settings;
mod telegram;
mod telegram_root_ca;
mod ui;

/// Git管理外の `config.toml` からビルド時に生成する設定。
/// secretを `src/` 配下のRustソースへ直接置かないことで、コンパイラ警告による
/// ビルドログ漏えいを防ぐ。`app_config` module(実行時設定、NVS優先)とは別物。
/// このcrateでは `src/config.rs` というファイル名は使わない(旧方式でsecretを
/// 直接書いていたファイル名と同じにすると、`scripts/check-local-firmware-secrets.sh`
/// の再発防止チェックと衝突するため)。
mod build_config {
    include!(concat!(env!("OUT_DIR"), "/generated_config.rs"));
}

use std::cell::RefCell;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use esp_idf_hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;

use app_config::AppConfig;
use board::{DisplayPins, DISPLAY_HEIGHT, DISPLAY_WIDTH};
use bridge_client::{PowerAction, PowerActionLabel};
use settings::RuntimeSettings;
use ui::{Status, TelegramState};

const STATUS_INTERVAL: Duration = Duration::from_secs(10);
const WIFI_RECONNECT_INTERVAL: Duration = Duration::from_secs(15);
const TOAST_TTL: Duration = Duration::from_secs(3);
/// PC状態のTelegram通知を出すまでに必要な、同じ結果の連続観測回数。
/// STATUS_INTERVAL(10秒)×2回なので、20秒続いた変化だけを通知する。
/// タッチのポーリング間隔。取りこぼさない程度に短く、CPUを回しすぎない程度に長く。
const TOUCH_POLL_INTERVAL: Duration = Duration::from_millis(20);

const NOTIFY_STABLE_POLLS: u8 = 2;

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
///
/// 呼び出し元(main内)がこの数だけの状態を個別に持っているため、素直に引数へ
/// 並べている。呼び出し箇所は2つだけで、構造体へまとめても本体の複雑さは
/// 変わらない。
#[allow(clippy::too_many_arguments)]
fn start_online_services(
    sntp: &mut Option<esp_idf_svc::sntp::EspSntp<'static>>,
    telegram_started: &mut bool,
    power_lock: &telegram::PowerLock,
    operation_lock: &telegram::OperationLock,
    telegram_state: &Arc<Mutex<telegram::State>>,
    notifier: &mut Option<telegram::Notifier>,
    app_config: &Arc<AppConfig>,
    settings: &Arc<RuntimeSettings>,
    // Telegramのpollingスレッドと通知スレッドで共有するTLS直列化ロック
    // (Issue #127。mbedTLSは実質同時1本のため、両スレッドで同じものを渡す)。
    https_lock: &telegram::HttpsLock,
) {
    if sntp.is_none() {
        // m5stack-pc-bridgeはtimestampを検証するため、電源操作前に時計同期が必要になる。
        // ここではSNTP開始だけ行い、同期待ちは別スレッド側に任せる。
        match net::start_sntp() {
            Ok(started) => {
                println!("SNTP started");
                *sntp = Some(started);
            }
            Err(e) => println!("SNTP start failed: {e}"),
        }
    }

    if notifier.is_none() {
        // PC状態変化などをUIループから送るための送信専用スレッド。
        *notifier = telegram::start_notifier(
            Arc::clone(app_config),
            Arc::clone(settings),
            https_lock.clone(),
        );
    }

    if !*telegram_started && telegram::is_configured(app_config.as_ref()) {
        let client = telegram::Client::new(
            Arc::clone(power_lock),
            operation_lock.clone(),
            Arc::clone(app_config),
            Arc::clone(settings),
            https_lock.clone(),
        );
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
    // Telegramから実行時に変更できる設定値(pc_ip_address/pc_status_addr/wol_port)。
    // AppConfigは起動時の読み取り専用スナップショットのまま残す。
    let settings = Arc::new(RuntimeSettings::new(&app_config, nvs_partition.clone())?);

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
        locked: false,
        battery: None,
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
    // Telegramの /lock で操作を止められるようにする。パネル操作にも効かせるため
    // UIループとTelegramスレッドで共有する。
    let operation_lock = telegram::OperationLock::default();
    let telegram_state = Arc::new(Mutex::new(telegram::State::Disabled));
    let mut sntp: Option<esp_idf_svc::sntp::EspSntp<'static>> = None;
    let mut telegram_started = false;
    let mut notifier: Option<telegram::Notifier> = None;
    // Telegramのpollingスレッドと通知スレッドで共有するTLS直列化ロック。
    // mbedTLSは実質同時1本のため、両スレッドへ同じものを渡す(Issue #127)。
    let https_lock: telegram::HttpsLock = Arc::new(Mutex::new(()));
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
            &operation_lock,
            &telegram_state,
            &mut notifier,
            &app_config,
            &settings,
            &https_lock,
        );
    }

    ui::draw_main(&mut display, &status)?;

    // 起動自己診断の材料。display初期化と描画がここまで `?` で通ったことが
    // 「画面が動いた」の証拠になる。失敗していたらmain自体がErrで終わり、
    // 下のvalidマークへは届かない(壊れたfirmwareはvalidにならない)。
    let display_ok = true;
    // OTA後の新slotをvalidとマークしたか。Wi-Fi接続中にSTATUS周期(10秒)で
    // 再試行する。一時的な接続失敗だけで正常firmwareを戻さないため。
    // `mark_app_valid_after_self_test` は未pendingの通常起動ではno-opなので、
    // 毎起動呼んでも無害(詳細は `ota.rs` のコメントを参照)。
    let mut ota_validated = false;
    // 自己診断が通らなかったことを一度だけログへ出すためのフラグ。
    let mut self_test_reported = false;

    let mut screen = Screen::Main;
    let mut status_at = Instant::now();
    // バッテリー読みはPC状態確認とは別周期で回す(Wi-Fi断でも止めないため)。
    let mut battery_at = Instant::now();
    let mut wifi_check_at = Instant::now();
    let mut toast_at = Instant::now();
    let mut touch_was_down = false;
    // Telegramへ通知済みのPC状態。画面表示(status.pc_online)とは別に持ち、
    // 瞬断による通知の連投を防ぐ。
    //
    // 起動直後の最初の観測は通知せず基準値として取り込むだけにする(Noneの間)。
    // そうしないとM5Stackを再起動するたびに「オンラインになりました」を送って
    // しまう。Telegram pollerが最初のgetUpdatesを実行しないのと同じ考え方。
    let mut notified_online: Option<bool> = None;
    let mut notify_streak: u8 = 0;

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
                    &operation_lock,
                    &telegram_state,
                    &mut notifier,
                    &app_config,
                    &settings,
                    &https_lock,
                );
            }
        }

        let now_telegram = match *telegram::lock_state(&telegram_state) {
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

        // バッテリーはI2Cで読むだけなのでWi-Fiに依存しない。PC状態の確認と
        // 同じ条件に入れていると、Wi-Fi断の間ずっと電池表示が固まる。
        if battery_at.elapsed() >= STATUS_INTERVAL {
            battery_at = Instant::now();
            // I2Cはタッチと共有だが、同一スレッドから順に触るので競合しない。
            let now_battery = board::read_battery(&mut axp);
            if now_battery != status.battery {
                status.battery = now_battery;
                if matches!(screen, Screen::Main) {
                    ui::draw_main(&mut display, &with_toast(&status, &toast_text))?;
                }
            }
        }

        if status.wifi_connected && status_at.elapsed() >= STATUS_INTERVAL {
            status_at = Instant::now();
            let previous_online = status.pc_online;
            let previous_battery = status.battery;
            let now_online =
                net::check_pc_online(&settings.pc_status_addr(), net::STATUS_PROBE_TIMEOUT);
            if now_online != status.pc_online {
                status.pc_online = now_online;
                println!("PC status changed: online={}", status.pc_online);
            }

            // 画面表示は即座に切り替えるが、Telegram通知だけは同じ結果を
            // NOTIFY_STABLE_POLLS回連続で観測してから送る。瞬断やPC再起動中の
            // 短い揺れで通知が連投されるのを防ぐ。
            match notified_online {
                None => notified_online = Some(now_online),
                Some(prev) if prev == now_online => notify_streak = 0,
                Some(_) => {
                    notify_streak += 1;
                    if notify_streak >= NOTIFY_STABLE_POLLS {
                        notified_online = Some(now_online);
                        notify_streak = 0;
                        if let Some(notifier) = notifier.as_ref() {
                            notifier.notify(format!(
                                "PCが{}になりました。",
                                net::pc_online_label_ja(now_online)
                            ));
                        }
                    }
                }
            }
            // 表示内容が変わったときだけ描き直す。`draw_main`は全画面消去から
            // 始まるため、10秒ごとに無条件で呼ぶとその周期で画面がちらつく。
            if matches!(screen, Screen::Main)
                && (status.pc_online != previous_online || status.battery != previous_battery)
            {
                ui::draw_main(&mut display, &with_toast(&status, &toast_text))?;
            }

            // 起動自己診断: 通ったときだけOTA後の新slotをvalidとマークする。
            // 通らなければ何もしない(次回起動で旧slotへ戻る)。判定式の根拠は
            // `pc_remote_signing::boot_self_test_passed` のコメントを参照。
            // このブロックはWi-Fi接続中にしか走らないため、一時的な接続失敗は
            // 次の周期で再試行される。失敗してもpanicしない(戻る方向が安全側)。
            if !ota_validated && display_ok {
                let checks = pc_remote_signing::BootChecks {
                    display_ok,
                    wifi_connected: status.wifi_connected,
                };
                match crate::ota::mark_app_valid_after_self_test(&checks) {
                    Ok(true) => {
                        ota_validated = true;
                        println!("ota: boot self-test passed, marked app valid");
                    }
                    Ok(false) => {
                        // 通らなかったことを一度だけ出す。これが無いと、
                        // 「次回起動で旧slotへ戻る」状態がログから読み取れず、
                        // OTA後の実機確認で正常との区別がつかない。10秒周期で
                        // 出すと通常運用のログを埋めるため初回だけにする。
                        if !self_test_reported {
                            self_test_reported = true;
                            println!(
                                "ota: boot self-test not passed yet (wifi_connected={}); \
                                 pending slot stays unvalidated and will roll back on reboot",
                                status.wifi_connected
                            );
                        }
                    }
                    Err(e) => println!("ota: mark app valid failed (will retry): {e}"),
                }
            }
        }

        // ロック状態はTelegramスレッド側から変わるので、画面表示へ反映する。
        let now_locked = operation_lock.is_locked();
        if now_locked != status.locked {
            status.locked = now_locked;
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
                // ロック中はどのボタンも実行しない。REBOOT/SHUTDOWNは確認画面を
                // 開くだけだが、OKまで進んでから弾くより先に理由を返す。
                // `status.locked`はループ先頭で読んだ値なので共有状態を直接見る。
                //
                // 判定対象は今表示している画面のボタンだけにする。全画面分を
                // まとめて見ると、Main画面でOK_BUTTON(170,150,130,60)の領域まで
                // 拾ってしまい、そこはMainでは何も無い場所なので、空白をタップ
                // しただけでロックのトーストが出る。CANCELは電源操作ではないため
                // ロック中でも通す(でないと確認画面から戻れない)。
                let power_button_tapped = match screen {
                    Screen::Main => {
                        ui::WAKE_BUTTON.contains(x, y)
                            || ui::REBOOT_BUTTON.contains(x, y)
                            || ui::SHUTDOWN_BUTTON.contains(x, y)
                    }
                    Screen::Confirm(_) => ui::OK_BUTTON.contains(x, y),
                };
                if power_button_tapped && operation_lock.is_locked() {
                    reject_locked(
                        &mut display,
                        &status,
                        &mut toast_text,
                        &mut toast_at,
                        &mut screen,
                    )?;
                    touch_was_down = touch_down;
                    std::thread::sleep(TOUCH_POLL_INTERVAL);
                    continue;
                }

                match screen {
                    Screen::Main => {
                        if ui::WAKE_BUTTON.contains(x, y) {
                            println!("WAKE tapped at x={x} y={y}");
                            let _guard = telegram::lock_power(&power_lock);
                            // status.lockedはループ先頭で読んだ値なので、判定から
                            // ここまでの間にTelegramの/lockが通っている可能性がある。
                            // 実行直前に共有状態を直接見る。
                            if operation_lock.is_locked() {
                                reject_locked(
                                    &mut display,
                                    &status,
                                    &mut toast_text,
                                    &mut toast_at,
                                    &mut screen,
                                )?;
                                touch_was_down = touch_down;
                                std::thread::sleep(TOUCH_POLL_INTERVAL);
                                continue;
                            }
                            let wol_result = net::send_wake_on_lan(
                                &app_config.pc_mac_address,
                                settings.wol_port(),
                            );
                            let (toast, report) = match wol_result {
                                Ok(()) => {
                                    println!("WOL sent");
                                    ("Magic packet sent", "WOLを送信しました。")
                                }
                                Err(e) => {
                                    println!("WOL failed: {e}");
                                    ("WOL failed", "WOL送信に失敗しました。")
                                }
                            };
                            notify_panel_action(notifier.as_ref(), report);
                            toast_text = Some(toast.to_string());
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
                            let _guard = telegram::lock_power(&power_lock);
                            // WAKEと同じ理由で、実行直前にロックを再確認する。
                            if operation_lock.is_locked() {
                                reject_locked(
                                    &mut display,
                                    &status,
                                    &mut toast_text,
                                    &mut toast_at,
                                    &mut screen,
                                )?;
                                touch_was_down = touch_down;
                                std::thread::sleep(TOUCH_POLL_INTERVAL);
                                continue;
                            }
                            let pc_ip_address = settings.pc_ip_address();
                            let (toast, report) =
                                match bridge_client::send_command(action, app_config.as_ref(), &pc_ip_address) {
                                    Ok(code) if bridge_client::is_accepted(code) => (
                                        "Command accepted".to_string(),
                                        format!("{}を受け付けました。", action.label_ja()),
                                    ),
                                    Ok(code) => (
                                        format!("Command rejected ({code})"),
                                        format!("{}が拒否されました。({code})", action.label_ja()),
                                    ),
                                    Err(e) => {
                                        println!("bridge command failed: {e}");
                                        (
                                            "Command failed".to_string(),
                                            format!("{}に失敗しました。", action.label_ja()),
                                        )
                                    }
                                };
                            notify_panel_action(notifier.as_ref(), &report);
                            toast_text = Some(toast);
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

        std::thread::sleep(TOUCH_POLL_INTERVAL);
    }
}

/// 本体パネルからの操作結果をTelegramへ通知する。
///
/// Telegram経由の操作はその場でチャットへ返信されるが、パネル操作は画面の
/// トーストで完結してしまう。外出中に「誰かが本体を触った」ことへ気づける
/// ように、パネル操作であることが分かる文言で送る。
fn notify_panel_action(notifier: Option<&telegram::Notifier>, text: &str) {
    if let Some(notifier) = notifier {
        notifier.notify(format!("[本体パネル操作] {text}"));
    }
}

/// ロック中に電源操作を弾いたときのトースト。無反応だと故障と区別がつかない
/// ため、解除方法まで出す。画面はASCIIフォントなので英語。
const LOCKED_TOAST: &str = "Locked (/unlock in Telegram)";

/// ロック中の操作を弾き、理由をトーストで表示してメイン画面へ戻す。
/// 呼び出し側はこの後ループを`continue`する。
fn reject_locked(
    display: &mut board::Core2Display<'_>,
    status: &Status<'_>,
    toast_text: &mut Option<String>,
    toast_at: &mut Instant,
    screen: &mut Screen,
) -> Result<(), Box<dyn std::error::Error>> {
    *toast_text = Some(LOCKED_TOAST.to_string());
    *toast_at = Instant::now();
    *screen = Screen::Main;
    ui::draw_main(display, &with_toast(status, toast_text))?;
    Ok(())
}

fn with_toast<'a>(status: &Status<'a>, toast: &'a Option<String>) -> Status<'a> {
    Status {
        toast: toast.as_deref(),
        ..*status
    }
}
