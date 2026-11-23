use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use crate::app_config::AppConfig;
use crate::net;
use crate::settings::RuntimeSettings;

use super::{Api, FIRMWARE_VERSION, HttpsLock, allowed_chat_id, ensure_root_ca, is_configured};

/// 定期レポートの時刻判定を行う間隔。長すぎると送信時刻がずれ、短すぎると
/// スレッドが無駄に起きる。1分あれば時刻単位の判定には十分。
const DAILY_REPORT_CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// 1日1回の定期レポートの送信判定。
///
/// SNTP同期後のwall clockから「ローカル時刻の何日目か」と「何時か」を整数演算で
/// 求める。tzデータベースを持たない環境なので、設定のUTCオフセットだけを使う。
struct DailyReport {
    /// 最後に送った日(UNIX epochからのローカル日数)。同じ日には二度送らない。
    last_sent_day: Option<i64>,
}

impl DailyReport {
    fn new(config: &AppConfig) -> Self {
        // 起動時点で既に送信時刻を過ぎているなら、その日の分は送信済みとして扱う。
        // M5Stackの再起動のたびに同じ日のレポートが届くのを防ぐため。
        //
        // まだ送信時刻より前なら未送信のままにする。ここで無条件に送信済みへ
        // すると、送信時刻の前にM5Stackを再起動しただけでその日の分が飛んでしまう
        // (例: 8時に再起動 → 9時のレポートが翌日まで来ない)。
        let last_sent_day = Self::local_now(config)
            .and_then(|(day, hour)| (hour >= config.daily_report_hour).then_some(day));
        Self { last_sent_day }
    }

    /// (ローカル日数, ローカル時)を返す。NTP未同期なら None。
    fn local_now(config: &AppConfig) -> Option<(i64, i64)> {
        let unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs() as i64;
        if !net::is_ntp_synced(unix) {
            return None;
        }
        let local = unix + config.timezone_offset_hours * 3600;
        Some((local.div_euclid(86_400), local.rem_euclid(86_400) / 3600))
    }

    /// 送信すべき時刻なら(対象日, レポート本文)を返す。
    ///
    /// ここでは「送信済み」にしない。送信に失敗した日を既済にしてしまうと、
    /// その日のレポートが黙って落ちる。成功後に`mark_sent()`を呼ぶこと。
    fn due_report(&self, config: &AppConfig, settings: &RuntimeSettings) -> Option<(i64, String)> {
        if !(0..=23).contains(&config.daily_report_hour) {
            return None;
        }
        let (day, hour) = Self::local_now(config)?;
        if hour != config.daily_report_hour || self.last_sent_day == Some(day) {
            return None;
        }

        let online = net::check_pc_online(&settings.pc_status_addr(), net::STATUS_PROBE_TIMEOUT);
        Some((
            day,
            format!("定期レポート\nPC: {}", net::pc_online_label_ja(online)),
        ))
    }

    /// 送信に成功した日を記録する。以降その日は送らない。
    const fn mark_sent(&mut self, day: i64) {
        self.last_sent_day = Some(day);
    }
}

/// UIスレッドからTelegramへ能動的に通知を送るためのハンドル。
///
/// 送信はHTTPSで数秒かかり得るため、UIループを止めないよう専用スレッドへ
/// channelで渡す。送信スレッドが落ちていても`notify`は失敗を無視する。
#[derive(Clone)]
pub struct Notifier {
    tx: Arc<mpsc::Sender<String>>,
}

impl Notifier {
    pub fn notify(&self, text: String) {
        let _ = self.tx.send(text);
    }
}

/// 通知送信スレッドを起動し、UIスレッド用のハンドルを返す。
/// Telegram未設定、または許可ユーザーIDがchat_idとして使えない場合はNone。
/// `https` はpollingスレッドと共有するTLS直列化ロック(Issue #127)。
pub fn start_notifier(
    config: Arc<AppConfig>,
    settings: Arc<RuntimeSettings>,
    https: HttpsLock,
) -> Option<Notifier> {
    if !is_configured(config.as_ref()) {
        return None;
    }
    let chat_id = allowed_chat_id(config.as_ref())?;

    let (tx, rx) = mpsc::channel::<String>();
    let tx = Arc::new(tx);
    // 通知スレッド自身が送信側を保持して、切断時の終了を妨げないようにする。
    let weak_tx = Arc::downgrade(&tx);
    let api = Api { config, https };
    let spawned = std::thread::Builder::new()
        .stack_size(12 * 1024)
        .spawn(move || {
            let mut schedule = DailyReport::new(&api.config);
            let started = Instant::now();
            let mut firmware_schedule = notify_stable::FirmwareCheckSchedule::default();
            let mut firmware_notice = notify_stable::FirmwareNotice::default();
            loop {
                // 定期レポートの時刻判定のため、通知が無くても定期的に起きる。
                let queued = match rx.recv_timeout(DAILY_REPORT_CHECK_INTERVAL) {
                    Ok(text) => Some(text),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    // 送信側(UIスレッド)が全て落ちた場合は終了する。
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                };

                let (next_schedule, due) = firmware_schedule.poll(started.elapsed().as_secs());
                firmware_schedule = next_schedule;
                if due {
                    let pc_ip_address = settings.pc_ip_address();
                    let manifest = crate::ota::fetch_verified_manifest(&api.config, &pc_ip_address);
                    if manifest.is_err() {
                        println!("ota: firmware availability check failed");
                    }
                    let offered = manifest.as_ref().ok().map(|value| value.version.as_str());
                    let (next_notice, notify) = firmware_notice.observe(FIRMWARE_VERSION, offered);
                    firmware_notice = next_notice;
                    if notify && let (Some(tx), Some(version)) = (weak_tx.upgrade(), offered) {
                        Notifier { tx }.notify(notify_stable::firmware_available_text(
                            FIRMWARE_VERSION,
                            version,
                        ));
                    }
                }

                // CA storeの確認は`due_report()`より前に行う。`due_report()`は
                // 返した時点で「その日は送信済み」と記録するため、後段で失敗すると
                // その日のレポートが黙って落ちる。
                if let Err(e) = ensure_root_ca() {
                    println!("telegram: root CA install failed: {e}");
                    continue;
                }

                if let Some(text) = queued {
                    // Issue #127: 以前は戻り値を捨てており、TLS衝突時の失敗が
                    // 黙って消えた。直列化した今も送信自体は失敗し得るため、
                    // 失敗の事実だけログへ残す(本文・tokenは出さない)。
                    if !api.send_message(chat_id, &text) {
                        println!("telegram: queued notify failed");
                    }
                }
                // 送信できた日だけ既済にする。失敗した場合は次のループで
                // 再送する(送信時刻のうちは何度でも試す)。
                if let Some((day, text)) = schedule.due_report(&api.config, &settings)
                    && api.send_message(chat_id, &text)
                {
                    schedule.mark_sent(day);
                }
            }
        });

    match spawned {
        Ok(_) => Some(Notifier { tx }),
        Err(e) => {
            println!("telegram: failed to start notifier thread: {e}");
            None
        }
    }
}
