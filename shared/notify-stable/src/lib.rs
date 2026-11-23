//! PC状態の安定通知判定の純粋ロジック。
//! `firmware` はESP32専用でhostビルドできないため、判定だけを分離してhostで
//! テストする(`shared/*` 共通の方針)。
//!
//! 画面表示は即座に切り替えるが、Telegram通知だけは同じ結果を `STABLE_POLLS`
//! 回連続で観測してから送る。瞬断やPCの再起動中の短い揺れで連投されないように
//! するため。判定に必要なのは前回状態・連続回数・今回の観測と抑止フラグだけ
//! (STATUS取得・描画・通知送信は呼び出し側に残す)。

mod firmware_notice;
pub use firmware_notice::{FirmwareCheckSchedule, FirmwareNotice, firmware_available_text};

/// 安定通知を出すまでに必要な、同じ結果の連続観測回数。
/// STATUS_INTERVAL(10秒)×2回なので、20秒続いた変化だけを通知する。
pub const STABLE_POLLS: u8 = 2;

/// PC状態の安定通知の状態。最初の観測は通知せず基準値として取り込む
/// (そうしないと再起動のたびに通知が飛ぶ)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StableNotify {
    /// 直前に通知(または基準値として取り込み)したPC状態。
    pub notified_online: Option<bool>,
    /// `notified_online` と異なる観測が連続した回数。同じ観測に戻れば0へ戻る。
    pub notify_streak: u8,
}

impl StableNotify {
    /// 初期状態(未観測)。最初の `poll` は通知せず基準値を取り込む。
    pub fn new() -> Self {
        Self::default()
    }

    /// 今回のSTATUS観測(`now_online`)を与え、次の内部状態と通知要否を返す。
    /// - `wake_succeeded` が真のときは何もしない(起動成功通知側で既済のため、
    ///   10秒後に同じオンを「PCが起動しました。」として二重に送らない)。
    /// - `suppress_off` が真のときは通知せず、オフとして既済に進める
    ///   (スリープ直後のオフは意図した結果であり、溜まったstreakで抑止明けに
    ///   飛ばないようにする)。
    ///
    /// 通知は1回の変化につき1回だけ(連続観測で確定したとき)。
    pub const fn poll(
        self,
        now_online: bool,
        wake_succeeded: bool,
        suppress_off: bool,
    ) -> (Self, bool) {
        if wake_succeeded {
            return (self, false);
        }
        if suppress_off {
            return (
                Self {
                    notified_online: Some(false),
                    notify_streak: 0,
                },
                false,
            );
        }
        match self.notified_online {
            // 最初の観測は通知せず基準値として取り込む。
            None => (
                Self {
                    notified_online: Some(now_online),
                    notify_streak: self.notify_streak,
                },
                false,
            ),
            // 変化なし: 連続カウントを捨てる(瞬断や短い揺れはここで消える)。
            Some(prev) if prev == now_online => (
                Self {
                    notified_online: Some(prev),
                    notify_streak: 0,
                },
                false,
            ),
            Some(prev) => {
                let notify_streak = self.notify_streak.saturating_add(1);
                if notify_streak >= STABLE_POLLS {
                    (
                        Self {
                            notified_online: Some(now_online),
                            notify_streak: 0,
                        },
                        true,
                    )
                } else {
                    (
                        Self {
                            notified_online: Some(prev),
                            notify_streak,
                        },
                        false,
                    )
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_polls_is_two() {
        // 10秒周期×2回=約20秒。短くすると瞬断で誤通知、長くすると気づきが遅れる。
        assert_eq!(STABLE_POLLS, 2);
    }

    #[test]
    fn first_observation_becomes_baseline_without_notify() {
        // 起動直後の最初の観測は通知せず取り込む(再起動のたびに飛ばない)。
        for now_online in [false, true] {
            let (next, notify) = StableNotify::new().poll(now_online, false, false);
            assert_eq!(
                next,
                StableNotify {
                    notified_online: Some(now_online),
                    notify_streak: 0,
                },
                "now_online={now_online}"
            );
            assert!(!notify, "now_online={now_online}");
        }
    }

    #[test]
    fn same_observation_resets_streak() {
        // 同じ観測に戻れば連続カウントを捨てる(瞬断はここで消える)。
        let mut state = StableNotify::new().poll(false, false, false).0;
        let (next, notify) = state.poll(true, false, false);
        assert!(!notify);
        assert_eq!(next.notify_streak, 1);
        state = next;
        let (next, notify) = state.poll(false, false, false);
        assert!(!notify);
        assert_eq!(next.notified_online, Some(false));
        assert_eq!(next.notify_streak, 0);
    }

    #[test]
    fn two_consecutive_changes_notify_once() {
        // 2回連続で同じ変化を見たときだけ1回通知する。
        let mut state = StableNotify::new().poll(false, false, false).0;
        let (next, notify) = state.poll(true, false, false);
        assert!(!notify);
        state = next;
        let (next, notify) = state.poll(true, false, false);
        assert!(notify);
        assert_eq!(next.notified_online, Some(true));
        assert_eq!(next.notify_streak, 0);
        // 確定後は同じ状態が続いても通知しない。
        let (next, notify) = next.poll(true, false, false);
        assert!(!notify);
        assert_eq!(next.notify_streak, 0);
    }

    #[test]
    fn offline_change_notifies_as_well() {
        // オン→オフの変化も同じ回数で通知する(方向で変えない)。
        let mut state = StableNotify::new().poll(true, false, false).0;
        let (next, notify) = state.poll(false, false, false);
        assert!(!notify);
        state = next;
        let (next, notify) = state.poll(false, false, false);
        assert!(notify);
        assert_eq!(next.notified_online, Some(false));
    }

    #[test]
    fn wake_success_skips_without_changing_state() {
        // 起動成功通知で既済のため、安定通知側は何もしない(変化を二重に送らない)。
        let mut state = StableNotify::new().poll(false, false, false).0;
        let (next, _) = state.poll(true, false, false);
        state = next;
        assert_eq!(state.notify_streak, 1);
        let (next, notify) = state.poll(true, true, false);
        assert!(!notify);
        assert_eq!(next, state);
    }

    #[test]
    fn suppressed_off_advances_baseline_without_notify() {
        // スリープ指示後のオフは通知せず、オフとして既済に進める。
        let state = StableNotify::new().poll(true, false, false).0;
        let (next, notify) = state.poll(false, false, true);
        assert!(!notify);
        assert_eq!(
            next,
            StableNotify {
                notified_online: Some(false),
                notify_streak: 0,
            }
        );
        // 抑止中のオン復帰は通常の変化として扱う(「PCが起動しました。」は抑止しない)。
        let (next, notify) = next.poll(true, false, false);
        assert!(!notify);
        assert_eq!(next.notify_streak, 1);
    }

    #[test]
    fn streak_before_suppression_does_not_leak_afterwards() {
        // 抑止前に溜まった連続カウントは捨てる(抑止明けの誤通知を防ぐ)。
        let mut state = StableNotify::new().poll(true, false, false).0;
        let (next, _) = state.poll(false, false, false);
        state = next;
        assert_eq!(state.notify_streak, 1);
        let (next, notify) = state.poll(false, false, true);
        assert!(!notify);
        assert_eq!(next.notify_streak, 0);
        // 抑止明けのオフは基準値と一致するため通知せず、カウントも捨てる。
        let (next, notify) = next.poll(false, false, false);
        assert!(!notify);
        assert_eq!(next.notified_online, Some(false));
        assert_eq!(next.notify_streak, 0);
    }
}

/// 認証・認可の失敗が続いたときに通知するか決める抑制ロジック
/// (firmwareの未許可ユーザーとbridgeのHTTP認証失敗で同じポリシー)。
/// 閾値回たまったら発火し、以後は一定時間鳴らさない(単発の失敗で通知しないため)。
pub struct AlertThrottle {
    threshold: u32,
    interval: std::time::Duration,
    failures: u32,
    last_fired: Option<std::time::Instant>,
}

impl AlertThrottle {
    pub const DEFAULT_THRESHOLD: u32 = 3;
    /// 発火後、次に鳴らせるまでの時間(スキャンや連投で通知が埋まらないようにする)。
    pub const DEFAULT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3600);

    pub const fn new(threshold: u32, interval: std::time::Duration) -> Self {
        Self {
            threshold,
            interval,
            failures: 0,
            last_fired: None,
        }
    }

    /// 失敗を1件記録する。通知すべきならたまっていた件数を返し、カウンタを戻す。
    pub fn record(&mut self, now: std::time::Instant) -> Option<u32> {
        self.failures += 1;
        if self.failures < self.threshold {
            return None;
        }
        if let Some(fired) = self.last_fired
            && now.duration_since(fired) < self.interval
        {
            return None;
        }

        let count = self.failures;
        self.failures = 0;
        self.last_fired = Some(now);
        Some(count)
    }
}

impl Default for AlertThrottle {
    fn default() -> Self {
        Self::new(Self::DEFAULT_THRESHOLD, Self::DEFAULT_INTERVAL)
    }
}

/// firmwareの版の新旧比較の結果(`/update` 確認画面の出し分け用)。
/// 呼び出し側は網羅的に `match` すること。`Unknown` を「新しい」にも「同じ/古い」にも
/// 倒さない(根拠なく更新を促さず、確認なしに適用もさせない)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionOrder {
    /// 提示版が新しい。通常の更新として提示する。
    Newer,
    /// 提示版と同じ。確認ボタンを出さない。
    Same,
    /// 提示版が古い。巻き戻しのため明示して確認する(誤操作防止。意図的な巻き戻しは許す)。
    Older,
    /// 新旧を判定できない。両方の版をそのまま出し比較不可を明示する。
    Unknown,
}

impl VersionOrder {
    /// 確認ボタン付きの確認画面を出すか。「同じ」のときだけ出さない。
    /// `false` のとき呼び出し側はnonceを発行しないこと
    /// (使われないnonceを残さない)。
    pub const fn shows_confirm_button(self) -> bool {
        !matches!(self, Self::Same)
    }
}

/// 版文字列を数値の三つ組と接尾辞に分ける(最初の `-`/`+` 以降が接尾辞)。
/// 数値部はちょうど3要素のASCII数字。正規化はしない(別物の版の取り違えになる)。
/// `u64` に収まらない桁数は解釈不能(None)。panicしない。
fn split_version(version: &str) -> Option<([u64; 3], &str)> {
    let numeric_len = version.find(['-', '+']).unwrap_or(version.len());
    let (numeric, suffix) = version.split_at(numeric_len);
    let mut parts = [0u64; 3];
    let mut iter = numeric.split('.');
    for slot in parts.iter_mut() {
        let part = iter.next()?;
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse::<u64>().ok()?;
    }
    if iter.next().is_some() {
        return None;
    }
    Some((parts, suffix))
}

/// 現在の版(`FIRMWARE_VERSION`)と提示版(manifestの `version`)を数値で比較する。
/// **文字列比較にしない**: `"0.8.0" > "0.11.0"` となり古い版を「更新」と提示する
/// 事故(Issue #180)になる。
///
/// - 数値部が異なればその大小で決める。
/// - 数値部が同じでも接尾辞まで完全一致のときだけ `Same`。`0.7.0` と `0.7.0-diag2`
///   は別物扱い(接尾辞の新旧順序は定義できないため `Unknown`)。
/// - どちらかが解釈不能(`unknown`・要素不足・非数値など)でも `Unknown`。
///   bridgeは版が無いとき `"unknown"` を入れる。panicしない。
pub fn compare_versions(current: &str, offered: &str) -> VersionOrder {
    match (split_version(current), split_version(offered)) {
        (Some((cur, cur_suffix)), Some((off, off_suffix))) => match cur.cmp(&off) {
            std::cmp::Ordering::Less => VersionOrder::Newer,
            std::cmp::Ordering::Greater => VersionOrder::Older,
            std::cmp::Ordering::Equal if cur_suffix == off_suffix => VersionOrder::Same,
            std::cmp::Ordering::Equal => VersionOrder::Unknown,
        },
        _ => VersionOrder::Unknown,
    }
}

/// `/update` 実行前にmanifestのversionとsizeを提示する確認文。
/// sha256や署名は載せない(送信文に不要な情報を増やさない)。
/// 新旧の判定は [`compare_versions`] が正本。`Same` のとき呼び出し側は確認ボタンを
/// 出さずnonceも発行しないこと。
pub fn ota_confirm_text(current: &str, offered: &str, size: u64) -> String {
    match compare_versions(current, offered) {
        VersionOrder::Newer => format!(
            "firmware更新があります。\n現在: {current}\nversion: {offered}\nsize: {size} bytes\n\
             更新しますか？\nボタンを押すと開始します。完了すると自動でM5Stackを再起動します。"
        ),
        VersionOrder::Same => format!("すでに最新です(現在 {current})"),
        VersionOrder::Older => format!(
            "古い版への変更です。\n現在: {current} → 提示: {offered}\nsize: {size} bytes\n\
             意図的に巻き戻す場合のみボタンを押してください。完了すると自動でM5Stackを再起動します。"
        ),
        VersionOrder::Unknown => format!(
            "firmwareの版の新旧を判定できませんでした。\n現在: {current}\n提示: {offered}\n\
             size: {size} bytes\n版を確認してからボタンを押してください。\
             完了すると自動でM5Stackを再起動します。"
        ),
    }
}

/// OTAの進捗率(0-100)。表示専用でpanicしない。`total` が0なら0を返す
/// (manifestのsizeは検証前の値であり得るため落とさない)。
pub fn ota_progress_percent(received: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    // u64乗算はオーバーフローし得るためu128へ広げる(`saturating_mul` だと
    // received == total でも100にならない)。
    let received = received.min(total);
    u8::try_from((u128::from(received) * 100) / u128::from(total)).unwrap_or(100)
}

/// 進捗を通知する刻み(パーセント)。バイト数固定だとサイズにより更新回数が
/// 変わるため割合で刻む。細かくしすぎない(通知ごとにTLSハンドシェイクが入り
/// ダウンロードが止まる)。
pub const OTA_PROGRESS_STEP_PERCENT: u8 = 5;

/// Telegramへ出す進捗テキスト。`editMessageText` で書き換え続ける前提で、
/// Telegramは同一内容への編集を400にするため内容が変わるときだけ送ること。
pub fn ota_progress_text(version: &str, received: u64, total: u64) -> String {
    // セル数は刻みから計算する(刻みだけ変えてバーを古いままにする事故を防ぐ)。
    const CELLS: usize = (100 / OTA_PROGRESS_STEP_PERCENT) as usize;
    let percent = ota_progress_percent(received, total);
    let filled = (usize::from(percent) * CELLS).div_ceil(100).min(CELLS);
    let bar: String = "█".repeat(filled) + &"░".repeat(CELLS - filled);
    format!(
        "firmware更新中 ({version})\n[{bar}] {percent}%  {}KB / {}KB",
        received / 1024,
        total / 1024
    )
}

/// 検証・書き込み完了後に `editMessageText` で出す文言。
/// 100%バーとは別の文字列にすること(同一内容の編集は400になるため、同じ文言だと
/// 再起動前の最後の通知が届かない)。
pub fn ota_applying_text(version: &str) -> String {
    format!("firmware更新を適用します。M5Stackを再起動します ({version})")
}

#[cfg(test)]
mod alert_throttle_tests {
    use super::AlertThrottle;
    use std::time::{Duration, Instant};

    #[test]
    fn fires_at_threshold_and_resets_counter() {
        let mut throttle = AlertThrottle::default();
        let now = Instant::now();

        assert_eq!(throttle.record(now), None);
        assert_eq!(throttle.record(now), None);
        assert_eq!(throttle.record(now), Some(AlertThrottle::DEFAULT_THRESHOLD));
    }

    #[test]
    fn stays_quiet_until_the_interval_has_passed() {
        let interval = Duration::from_secs(3600);
        let mut throttle = AlertThrottle::new(3, interval);
        let start = Instant::now();

        for _ in 0..2 {
            assert_eq!(throttle.record(start), None);
        }
        assert_eq!(throttle.record(start), Some(3));

        // 間隔内は閾値へ再到達しても鳴らさない。
        for _ in 0..6 {
            assert_eq!(
                throttle.record(start + interval - Duration::from_secs(1)),
                None
            );
        }
        // 間隔を過ぎれば、たまっていた件数をまとめて返す。
        assert_eq!(throttle.record(start + interval), Some(7));
    }
}

#[cfg(test)]
mod version_order_tests {
    use super::{VersionOrder, compare_versions};

    #[test]
    fn accident_case_0_8_0_is_older_than_0_11_0() {
        // Issue #180そのもの: 文字列比較だと "0.8.0" > "0.11.0" になり逆転する。
        // 数値で比較するため、桁が違っても正しく判定できること。
        assert_eq!(compare_versions("0.11.0", "0.8.0"), VersionOrder::Older);
        assert_eq!(compare_versions("0.8.0", "0.11.0"), VersionOrder::Newer);
    }

    #[test]
    fn same_version_is_same() {
        assert_eq!(compare_versions("0.11.0", "0.11.0"), VersionOrder::Same);
        assert_eq!(compare_versions("0.2.0", "0.2.0"), VersionOrder::Same);
        assert_eq!(
            compare_versions("0.7.0-diag2", "0.7.0-diag2"),
            VersionOrder::Same
        );
    }

    #[test]
    fn numeric_parts_decide_regardless_of_digit_count() {
        assert_eq!(compare_versions("0.10.0", "0.9.0"), VersionOrder::Older);
        assert_eq!(compare_versions("0.9.0", "0.10.0"), VersionOrder::Newer);
        assert_eq!(compare_versions("0.99.99", "1.0.0"), VersionOrder::Newer);
        assert_eq!(compare_versions("1.0.0", "0.99.99"), VersionOrder::Older);
    }

    #[test]
    fn numeric_parts_decide_regardless_of_suffix() {
        // 数値部が異なれば接尾辞の有無は結果を変えない。
        assert_eq!(
            compare_versions("0.7.0-diag2", "0.7.1"),
            VersionOrder::Newer
        );
        assert_eq!(
            compare_versions("0.8.0", "0.7.0-diag2"),
            VersionOrder::Older
        );
    }

    #[test]
    fn same_numeric_parts_with_different_suffix_is_not_same() {
        // 接尾辞の順序は定義できないため、新しいとも古いとも断定しない。
        // 同じ扱いにすると確認ボタンが出ず、別物の版を見逃す。
        assert_eq!(
            compare_versions("0.7.0", "0.7.0-diag2"),
            VersionOrder::Unknown
        );
        assert_eq!(
            compare_versions("0.7.0-diag2", "0.7.0"),
            VersionOrder::Unknown
        );
        assert_eq!(
            compare_versions("0.7.0-diag2", "0.7.0-diag3"),
            VersionOrder::Unknown
        );
    }

    #[test]
    fn unparsable_versions_are_unknown() {
        // bridgeは版が無いときに "unknown" を入れる仕様。
        // 解釈できないときは新しい・同じ・古いのいずれにも倒さない。
        for bad in [
            "",
            "unknown",
            "1.2",
            "1",
            "1.2.3.4",
            "a.b.c",
            "1.x.3",
            "v1.2.3",
            " 0.8.0",
            "0.8.0 ",
            "1..3",
            ".1.2.3",
            "-diag2",
            "99999999999999999999999.0.0",
        ] {
            assert_eq!(
                compare_versions("0.11.0", bad),
                VersionOrder::Unknown,
                "{bad}"
            );
            assert_eq!(
                compare_versions(bad, "0.11.0"),
                VersionOrder::Unknown,
                "{bad}"
            );
        }
    }
}

#[cfg(test)]
mod update_confirm_tests {
    use super::{VersionOrder, compare_versions, ota_confirm_text};

    const VERSION: &str = "0.2.0";
    const SIZE: u64 = 19;

    #[test]
    fn ota_confirm_text_newer_shows_both_versions() {
        let text = ota_confirm_text("0.1.0", VERSION, SIZE);
        assert!(text.contains("firmware更新があります"), "{text}");
        assert!(text.contains("0.1.0"), "{text}");
        assert!(text.contains(VERSION), "{text}");
        assert!(text.contains(&SIZE.to_string()), "{text}");
        // 用語集: 再起動の対象(M5Stack)を必ず明記する。
        assert!(text.contains("M5Stackを再起動します"), "{text}");
    }

    #[test]
    fn ota_confirm_text_same_says_already_latest() {
        let text = ota_confirm_text(VERSION, VERSION, SIZE);
        assert!(text.contains("すでに最新です"), "{text}");
        assert!(text.contains(VERSION), "{text}");
    }

    #[test]
    fn ota_confirm_text_older_warns_downgrade() {
        let text = ota_confirm_text("0.11.0", "0.8.0", SIZE);
        assert!(text.contains("古い版への変更です"), "{text}");
        assert!(text.contains("0.11.0"), "{text}");
        assert!(text.contains("0.8.0"), "{text}");
        // 用語集: 再起動の対象(M5Stack)を必ず明記する。
        assert!(text.contains("M5Stackを再起動します"), "{text}");
    }

    #[test]
    fn ota_confirm_text_unknown_shows_both_versions_without_claiming() {
        let text = ota_confirm_text("0.11.0", "unknown", SIZE);
        assert!(text.contains("判定できませんでした"), "{text}");
        assert!(text.contains("0.11.0"), "{text}");
        assert!(text.contains("unknown"), "{text}");
        assert!(!text.contains("すでに最新です"), "{text}");
        assert!(!text.contains("firmware更新があります"), "{text}");
        assert!(!text.contains("古い版への変更です"), "{text}");
        // 用語集: 再起動の対象(M5Stack)を必ず明記する。
        assert!(text.contains("M5Stackを再起動します"), "{text}");
    }

    #[test]
    fn only_same_version_hides_confirm_button() {
        use super::VersionOrder;
        assert!(!VersionOrder::Same.shows_confirm_button());
        for order in [
            VersionOrder::Newer,
            VersionOrder::Older,
            VersionOrder::Unknown,
        ] {
            assert!(order.shows_confirm_button(), "{order:?}");
        }
    }

    #[test]
    fn notice_confirm_and_button_agree_on_version_order() {
        // 版の新旧に対する3つの出し分け(通知・確認文・確認ボタン)がずれないこと。
        // どれかだけ判定を変えると「通知は来るのに確認が出ない」等の食い違いになる。
        use super::FirmwareNotice;
        for (current, offered, order, notice, button, marker) in [
            (
                "0.1.0",
                "0.2.0",
                VersionOrder::Newer,
                true,
                true,
                "firmware更新があります",
            ),
            (
                "0.2.0",
                "0.2.0",
                VersionOrder::Same,
                false,
                false,
                "すでに最新です",
            ),
            (
                "0.11.0",
                "0.8.0",
                VersionOrder::Older,
                false,
                true,
                "古い版への変更です",
            ),
            (
                "0.11.0",
                "unknown",
                VersionOrder::Unknown,
                false,
                true,
                "判定できませんでした",
            ),
        ] {
            assert_eq!(compare_versions(current, offered), order);
            let (_, notified) = FirmwareNotice::default().observe(current, Some(offered));
            assert_eq!(notified, notice, "{current} {offered}");
            let text = ota_confirm_text(current, offered, SIZE);
            assert!(text.contains(marker), "{text}");
            assert_eq!(order.shows_confirm_button(), button);
        }
    }
}

#[cfg(test)]
mod ota_progress_tests {
    use super::{
        OTA_PROGRESS_STEP_PERCENT, ota_applying_text, ota_progress_percent, ota_progress_text,
    };

    #[test]
    fn percent_handles_boundaries_without_panicking() {
        // total=0 は0除算になる。進捗不明として0を返す。
        assert_eq!(ota_progress_percent(0, 0), 0);
        assert_eq!(ota_progress_percent(100, 0), 0);
        assert_eq!(ota_progress_percent(0, 1000), 0);
        assert_eq!(ota_progress_percent(500, 1000), 50);
        assert_eq!(ota_progress_percent(1000, 1000), 100);
        // 受信が想定を超えても100で頭打ちにする(表示が101%にならない)。
        assert_eq!(ota_progress_percent(2000, 1000), 100);
        // u64の上限付近でも saturating_mul でオーバーフローしない。
        assert_eq!(ota_progress_percent(u64::MAX, u64::MAX), 100);
    }

    #[test]
    fn text_shows_bar_percent_and_size() {
        let text = ota_progress_text("0.3.0", 512 * 1024, 1024 * 1024);
        assert!(text.contains("0.3.0"), "{text}");
        assert!(text.contains("50%"), "{text}");
        assert!(text.contains("512KB / 1024KB"), "{text}");
        assert!(text.contains('█') && text.contains('░'), "{text}");
    }

    #[test]
    fn bar_is_empty_at_zero_and_full_at_hundred() {
        // 20セル = 100 / OTA_PROGRESS_STEP_PERCENT(5)。
        let zero = ota_progress_text("v", 0, 1000);
        assert!(zero.contains("[░░░░░░░░░░░░░░░░░░░░]"), "{zero}");
        let full = ota_progress_text("v", 1000, 1000);
        assert!(full.contains("[████████████████████]"), "{full}");
    }

    #[test]
    fn text_changes_as_download_advances() {
        // 同一内容への editMessageText はTelegramが400にする。刻みごとに
        // 文字列が変わることを固定しておく。
        let a = ota_progress_text("v", 100 * 1024, 1000 * 1024);
        let b = ota_progress_text("v", 200 * 1024, 1000 * 1024);
        assert_ne!(a, b);
    }

    #[test]
    fn bar_cell_count_matches_step_percent() {
        // 描画セル数を `100 / OTA_PROGRESS_STEP_PERCENT` に固定する。
        // 定数だけ変えてバーを古いままにすると、通知回数と見た目がずれる。
        let expected = (100 / OTA_PROGRESS_STEP_PERCENT) as usize;
        for (received, total) in [(0, 1000), (500, 1000), (1000, 1000)] {
            let text = ota_progress_text("v", received, total);
            let bar = text.split('[').nth(1).unwrap().split(']').next().unwrap();
            assert_eq!(bar.chars().count(), expected, "{text}");
        }
    }

    #[test]
    fn text_changes_at_every_step() {
        // STEP刻みで進めたとき、毎回テキストが変わること。同一内容だと
        // Telegramが400を返すため、1回でも変わらない刻みがあってはならない。
        let total = 1000u64;
        let mut previous = ota_progress_text("v", 0, total);
        let mut percent = 0u8;
        let mut steps = 0u32;
        while percent < 100 {
            percent = percent.saturating_add(OTA_PROGRESS_STEP_PERCENT);
            let received = u64::from(percent) * total / 100;
            // 刻み通りに進んでいることの sanity (受信量の計算ミス検出用)。
            assert_eq!(ota_progress_percent(received, total), percent.min(100));
            let text = ota_progress_text("v", received, total);
            assert_ne!(text, previous, "percent={percent}");
            previous = text;
            steps += 1;
        }
        assert_eq!(steps, u32::from(100 / OTA_PROGRESS_STEP_PERCENT));
    }

    #[test]
    fn final_notification_reaches_hundred_despite_remainder() {
        // Issue #143追加分: 1,388,544B の実機サイズでは刻みの端数が残り、
        // 最終通知なしでは94%で止まった。割合ベースでも端数は必ず出るため、
        // 受信量=サイズでの最終通知が100%になることを固定する
        // (`ota.rs` はループ後に刻みと無関係に必ず1回呼ぶ)。
        let total = 1_388_544u64;
        assert_eq!(ota_progress_percent(total, total), 100);
        let text = ota_progress_text("v", total, total);
        assert!(text.contains("100%"), "{text}");
        let expected = (100 / OTA_PROGRESS_STEP_PERCENT) as usize;
        let bar = text.split('[').nth(1).unwrap().split(']').next().unwrap();
        assert_eq!(bar.chars().count(), expected, "{text}");
        assert!(bar.chars().all(|c| c == '█'), "{text}");
    }

    #[test]
    fn applying_text_differs_from_full_bar() {
        // M5Stack再起動前の最後の通知が直前の100%バーと同じ内容だとTelegramが400に
        // するため、文言が変わることを固定する。
        let full = ota_progress_text("v", 1000, 1000);
        let applying = ota_applying_text("v");
        assert_ne!(full, applying, "{full} vs {applying}");
        // 用語集: 再起動の対象(M5Stack)を必ず明記する。
        assert!(applying.contains("M5Stackを再起動します"), "{applying}");
    }
}
