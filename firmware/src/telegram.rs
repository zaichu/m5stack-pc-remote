// Telegram Bot APIクライアント。外向きHTTPS long pollingで、受信portを開けずに
// スマホ操作を受け取る。
//
// 守るべき挙動:
//   - `from.id` が許可ユーザーIDと一致するupdateだけ処理する
//   - /reboot と /shutdown と /sleep と /update は即実行せず、単回使用の確認nonceを発行する
//   - 確認は成功・失敗・期限切れのいずれでも消費し、再利用させない
//   - 起動直後の最初のgetUpdates結果はoffset更新だけにし、起動前に届いた古い命令を実行しない
//   - bot tokenとメッセージ内容をログへ出さない

use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::app_config::AppConfig;
use crate::board::Battery;
use crate::telegram_root_ca::TELEGRAM_ROOT_CA_PEM;

pub(crate) mod api;
pub(crate) mod bot;
pub(crate) mod report;

pub(crate) use api::Api;
pub use bot::Client;
pub use report::{Notifier, start_notifier};

/// ロック中でも受け付けるコマンド。これ以外は拒否する(default-deny)。
/// 追加するときは「ロック中の利用者に許してよいか」を必ず考えること。
pub(crate) const ALLOWED_WHILE_LOCKED: [&str; 4] = ["/status", "/settings", "/lock", "/unlock"];

const PLACEHOLDER_TOKEN: &str = "replace-with-your-telegram-bot-token";
const PLACEHOLDER_USER_ID: &str = "replace-with-your-telegram-user-id";

/// OTAで配るイメージの版と同じ値になるため、利用者が更新の成否を確認できる。
pub(crate) const FIRMWARE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) const BACKOFF_MIN: Duration = Duration::from_secs(5);
pub(crate) const BACKOFF_MAX: Duration = Duration::from_secs(60);
pub(crate) const RESPONSE_BUFFER: usize = 4096;

/// UIスレッドへ共有するTelegram状態。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Disabled,
    Polling,
    Error,
}

/// タッチUIとTelegramスレッドからの電源操作を直列化するロック。
pub type PowerLock = Arc<Mutex<()>>;

/// Telegram HTTPS接続の直列化ロック(Issue #127)。
///
/// ESP32のmbedTLSヒープでは実質同時に1本のTLS接続しか張れない。long poll保持中に
/// 2本目を開くと `ESP_ERR_HTTP_CONNECT` で失敗し、**通知が黙って消える**
/// (ログにも残らない)。pollingスレッドと通知スレッドで共有して直列化する。
///
/// トレードオフ: long poll中(最大 `long_poll_timeout+10` 秒)は通知がそのぶん遅れる。
/// 「遅れて届く」は許容し「黙って消える」は許さない、という選択。
///
/// **leafとして扱う。** 握ったまま他のロック(power/state)を取らず、他のロックの
/// 内側でも取らない。std Mutexは再入不可のため、握ったまま `send_message` 系を
/// 呼ぶと自己デッドロックする。送信箇所を増やすときもこの順序を守ること。
pub type HttpsLock = Arc<Mutex<()>>;

/// Telegram HTTPSの排他を取る。poisonしていても排他は維持する。
pub fn lock_https(https_lock: &HttpsLock) -> std::sync::MutexGuard<'_, ()> {
    https_lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 電源操作の排他を取る。poisonしていても排他は維持する。
///
/// `unwrap()` だと無関係なスレッドのpanicでUIループごと落ちる。守っているのは
/// 値型だけで壊れた状態を引き継ぐ心配がないため `into_inner()` で回復してよい
/// (このファイルの `lock_*` はすべて同じ扱い)。
pub fn lock_power(power_lock: &PowerLock) -> std::sync::MutexGuard<'_, ()> {
    power_lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// UIスレッドへ共有するTelegram状態の排他を取る。poisonしていても排他は維持する。
pub fn lock_state(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// UIループが読んだ最新のバッテリー状態。pollingスレッドはI2Cドライバを持たない
/// (AXP192はUIループ側の `axp` 経由でしか読めない)ため、読み取り結果の値だけを共有する。
pub type SharedBattery = Arc<Mutex<Option<Battery>>>;

/// 共有バッテリー状態の排他を取る。poisonしていても排他は維持する。
pub fn lock_battery(
    battery: &Mutex<Option<Battery>>,
) -> std::sync::MutexGuard<'_, Option<Battery>> {
    battery
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// PCの起動指示後の待機開始時刻。`None`=待機なし。画面ボタンはUIループ、`/wake` は
/// pollingスレッドで入るため共有する。leaf扱い(握ったまま送信系を呼ばない)。
pub type SharedWakeWatch = Arc<Mutex<Option<Instant>>>;

/// **ガードを呼び出した文の外へ持ち出さないこと。** `match *lock_wake_watch(..)` の
/// 腕の中で取り直すと、`match` が終わるまでガードが解放されず自己デッドロックする
/// (実機で再現済み、Issue #182)。呼び出し側は下の3つの公開関数を使う。
fn lock_wake_watch(
    wake_watch: &Mutex<Option<Instant>>,
) -> std::sync::MutexGuard<'_, Option<Instant>> {
    wake_watch
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 起動指示後の待機の有無と開始時刻を読む。ガードはこの関数内で手放すため、
/// 呼び出し側がガード保持中の再ロック(自己デッドロック)を起こせない。
pub fn wake_watch_started(wake_watch: &SharedWakeWatch) -> Option<Instant> {
    *lock_wake_watch(wake_watch)
}

/// 起動指示後の待機を終わらせる。ガードはこの関数内で手放す。
pub fn clear_wake_watch(wake_watch: &SharedWakeWatch) {
    *lock_wake_watch(wake_watch) = None;
}

/// 起動指示後の待機を開始・更新する。WOL送信に成功した呼び出し側だけが呼ぶ。
///
/// - すでにPCがオンなら待機しない(残っていた待機があれば終わらせる)。
/// - オフなら待機を開始する。待機中の再指示は開始時刻を今に置き換える
///   (=期限を最後の指示から数え直す)。
pub fn begin_wake_watch(wake_watch: &SharedWakeWatch, pc_online: bool) {
    // `begin()` の結果は `pc_online` だけで決まり直前の待機有無には依らないため、
    // 読み取りのためのロックは取らず書き込みの1回だけにする。
    let (next, _) = wake_check::WakeWatch::idle().begin(pc_online);
    *lock_wake_watch(wake_watch) = next.waiting.then(Instant::now);
}

/// スリープ指示後のオフ通知の抑止状態(Issue #217)。スリープを指示した直後、
/// PCはTCP probeに応答しなくなり「オフ」に見えるため、「PCが停止しました。」が
/// 誤って飛ぶ。`wake_watch` と同じくUIループとpollingスレッドで共有し、
/// UIループのSTATUS確認で読む。オフになるのを待ってから(`Armed`)、
/// オフ確認後はオンに戻るまで(`Suppressing`)抑止する。
/// leaf扱い(握ったまま送信系を呼ばない)。
pub type SharedSleepWatch = Arc<Mutex<wake_check::SleepWatch>>;

/// **ガードを呼び出した文の外へ持ち出さないこと。** `wake_watch` と同じ理由
/// (Issue #182)。呼び出し側は下の2つの公開関数を使う。
fn lock_sleep_watch(
    sleep_watch: &Mutex<wake_check::SleepWatch>,
) -> std::sync::MutexGuard<'_, wake_check::SleepWatch> {
    sleep_watch
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// スリープ指示を受け付けて抑止を始める。bridgeが受理した呼び出し側だけが呼ぶ。
pub fn begin_sleep_watch(sleep_watch: &SharedSleepWatch) {
    let mut guard = lock_sleep_watch(sleep_watch);
    *guard = (*guard).begin();
}

/// STATUS確認周期ごとの更新。オフ通知を抑止すべきならtrueを返す。
/// ガードはこの関数内で一度だけ取り、その中で `poll`→書き戻しまで行う。
/// 読み取りと書き戻しで二度取ると、UI側が読んだ直後にTelegram側が
/// `begin_sleep_watch` してもUI側の書き戻しで `begin` が消せる。
/// オフになるのを待ってから(`Armed`)、オフ確認後はオンに戻るまで
/// (`Suppressing`)抑止する。オンに戻ったら抑止を終わらせる。
pub fn poll_sleep_watch(sleep_watch: &SharedSleepWatch, pc_online: bool) -> bool {
    let mut guard = lock_sleep_watch(sleep_watch);
    let (next, suppress) = (*guard).poll(pc_online);
    *guard = next;
    suppress
}

/// 操作ロック。有効な間はWAKE / REBOOT / SHUTDOWN / SLEEPを一切実行しない。
/// Telegramの `/lock` `/unlock` で切り替え、本体パネル操作にも効く。
///
/// 状態はメモリ上だけで保持し、M5Stackを再起動すると解除される。再起動できる
/// 位置に居るなら本人が近くに居るとみなせるため、永続化はしない。
#[derive(Clone, Default)]
pub struct OperationLock(Arc<AtomicBool>);

impl OperationLock {
    pub fn is_locked(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    fn set(&self, locked: bool) {
        self.0.store(locked, Ordering::Relaxed);
    }
}

/// M5Stackの起動通知の文面。`firmware/Cargo.toml` の `version` が正本で、
/// `/status` のバージョン表示(`FIRMWARE_VERSION`)と同じ値を使う。
/// OTA後に新版が動いているかの確認と、M5Stackの予期しない再起動の検知が目的。
/// 用語は `docs/glossary.md` が正本(M5Stack自身の立ち上がりは「M5Stackが起動しました」)。
pub fn boot_notification_text() -> String {
    format!("M5Stackが起動しました ({FIRMWARE_VERSION})")
}

pub fn is_configured(config: &AppConfig) -> bool {
    // trimしないと ` token ` のような値がplaceholder判定をすり抜ける(Issue #130-1)。
    let token = config.telegram_bot_token.trim();
    let user_id = config_validation::normalize_telegram_user_id(&config.telegram_allowed_user_id);
    !token.is_empty()
        && token != PLACEHOLDER_TOKEN
        && !user_id.is_empty()
        && user_id != PLACEHOLDER_USER_ID
}

/// ピン留めしたルートCAをesp-tlsのglobal CA storeへ登録する。
/// HTTPSリクエスト前に1回だけ実行する。
fn install_root_ca() -> Result<(), Box<dyn Error>> {
    let pem = TELEGRAM_ROOT_CA_PEM.as_bytes();
    esp_idf_sys::esp!(unsafe { esp_idf_sys::esp_tls_init_global_ca_store() })?;
    esp_idf_sys::esp!(unsafe {
        esp_idf_sys::esp_tls_set_global_ca_store(pem.as_ptr(), pem.len() as u32)
    })?;
    Ok(())
}

/// `Once` は使わない。失敗しても「完了」扱いになり、heap不足等で1回失敗すると
/// 以降どのスレッドも再試行できずCA store未設定のままHTTPSを使ってしまう。
static ROOT_CA_INSTALLED: AtomicBool = AtomicBool::new(false);
static ROOT_CA_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn ensure_root_ca() -> Result<(), Box<dyn Error>> {
    let _guard = ROOT_CA_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if ROOT_CA_INSTALLED.load(Ordering::Acquire) {
        return Ok(());
    }
    install_root_ca()?;
    ROOT_CA_INSTALLED.store(true, Ordering::Release);
    Ok(())
}

/// private chatではchat_idがuser_idと一致するため、許可ユーザーIDをそのまま
/// 送信先として使う。能動送信(pollingの応答ではない通知)で必要になる。
pub(crate) fn allowed_chat_id(config: &AppConfig) -> Option<i64> {
    // Issue #130-1: 正規化を `config_validation` に一本化する(3箇所が別々にtrimするとずれる)。
    config_validation::parse_telegram_user_id(&config.telegram_allowed_user_id)
}
