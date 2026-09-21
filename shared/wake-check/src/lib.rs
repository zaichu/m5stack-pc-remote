//! PCの起動指示後の待機状態と期限判定の純粋ロジック。
//!
//! `firmware` はESP32専用でhostビルドできないため、判定だけをここへ分離して
//! hostでテストする(`shared/*` 共通の方針)。
//!
//! 時刻は扱わず、単調時計で測った経過秒だけを受け取る(NTP未同期でも壊れない)。

/// 起動指示後にPCがオンになるのを待つ期限(秒)。
///
/// 3分の根拠: Windowsの起動1〜2分 + STATUS確認の安定判定20秒 + bridge起動のばらつき。
/// **実機では測っていない。** ずれる場合はこの定数だけを変える(判定式は変えない)。
pub const WAKE_WATCH_TIMEOUT_SECS: u64 = 180;

/// 起動指示後の待機状態。
///
/// `firmware` 側は開始時刻(`Instant`)を `SharedWakeWatch`(`Option<Instant>`、
/// `None`=待機なし)として持ち、この値とは `waiting=is_some()` で対応する。
/// 時刻自体はここに持たず、経過秒だけを `poll` へ渡す。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WakeWatch {
    /// 待機中かどうか。
    pub waiting: bool,
}

/// 待機の更新で送る通知の種別。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeNotice {
    /// 待機中にPCがオンになった。既存の「PCが起動しました。」に所要時間を添えて送る。
    Succeeded,
    /// 期限までにPCがオンにならなかった。1回だけ送る。
    TimedOut,
}

impl WakeWatch {
    /// 待機なしの初期状態。
    pub fn idle() -> Self {
        Self { waiting: false }
    }

    /// 起動を指示したときの更新。WOL送信に成功した呼び出し側だけが呼ぶ。
    ///
    /// - すでにPCがオンなら待機を始めない(意味が無い)。残っていた待機があれば
    ///   終わらせる。
    /// - オフなら待機を開始する。待機中の再指示はここへもう一度来るため、
    ///   開始時刻の置き換え(=期限の更新)は呼び出し側で行い、ここでは待機継続を返す。
    /// 戻り値は(次の状態, 待機を開始したか)。
    pub fn begin(self, pc_online: bool) -> (Self, bool) {
        if pc_online {
            (Self { waiting: false }, false)
        } else {
            (Self { waiting: true }, true)
        }
    }

    /// STATUS確認周期ごとの更新。`elapsed_secs` は指示からの経過秒(単調時計由来)。
    ///
    /// 戻り値は(次の状態, 通知の要否と種別)。
    /// - 待機なしなら何も送らない。既存の状態変化通知とは別経路のため、
    ///   ここで送ると二重通知になる。
    /// - 待機中のオンで成功を送り、待機を終わらせる。
    /// - 待機中の期限到達で失敗を送り、待機を終わらせる(1回だけ。以降は
    ///   待機なしのため重複送信しない)。
    pub fn poll(self, pc_online: bool, elapsed_secs: u64) -> (Self, Option<WakeNotice>) {
        if !self.waiting {
            return (self, None);
        }
        if pc_online {
            return (Self { waiting: false }, Some(WakeNotice::Succeeded));
        }
        if elapsed_secs >= WAKE_WATCH_TIMEOUT_SECS {
            return (Self { waiting: false }, Some(WakeNotice::TimedOut));
        }
        (self, None)
    }
}

/// 起動指示を受け付けたときの応答文(日本語)。
///
/// 用語は `docs/glossary.md` が正本。利用者は「PCの起動を指示した」と考えており、
/// WOLは内部の手段のため文言に使わない(旧「WOLを送信しました。」)。
pub fn wake_request_text() -> &'static str {
    "PCの起動を指示しました。"
}

/// 起動指示自体に失敗したときの応答文(日本語)。
///
/// 成功文と対になるよう「PCの起動の指示」で始める(旧「WOL送信に失敗しました。」)。
pub fn wake_request_failed_text() -> &'static str {
    "PCの起動の指示に失敗しました。"
}

/// 待機中にPCがオンになったときの通知文(日本語)。
///
/// 既存の `net::pc_state_notification_ja(true)`(「PCが起動しました。」)に
/// 指示からの所要時間を添えたもの。`elapsed_secs` は単調時計由来の経過秒。
pub fn wake_succeeded_text(elapsed_secs: u64) -> String {
    format!("PCが起動しました(所要時間 {})。", format_elapsed_ja(elapsed_secs))
}

/// 期限までにPCがオンにならなかったときの通知文(日本語)。
///
/// Issue #182の指定どおり「指示から N 分経過」を含める。分未満は切り捨てる
/// (期限が分単位のため。180秒→「3分」、185秒→「3分」)。
pub fn wake_timed_out_text(elapsed_secs: u64) -> String {
    format!(
        "PCが起動しませんでした(指示から {}分経過)。",
        elapsed_secs / 60
    )
}

/// 経過秒の日本語表記。「45秒」「1分5秒」「3分」のように、分が0なら秒だけ、
/// 秒が0なら分だけ出す(「3分0秒」のような読みにくい表記を避ける)。
fn format_elapsed_ja(elapsed_secs: u64) -> String {
    let minutes = elapsed_secs / 60;
    let seconds = elapsed_secs % 60;
    match (minutes, seconds) {
        (0, s) => format!("{s}秒"),
        (m, 0) => format!("{m}分"),
        (m, s) => format!("{m}分{s}秒"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_is_three_minutes() {
        // 期限の変更は誤警告・気づきの遅れに直結するため、値を固定して検知する。
        // 根拠は `WAKE_WATCH_TIMEOUT_SECS` のコメント参照(実機未測定)。
        assert_eq!(WAKE_WATCH_TIMEOUT_SECS, 180);
    }

    #[test]
    fn begin_starts_wait_when_off() {
        // 待機なし→指示(オフ)→待機。戻り値のtrueは「待機を開始した」の意味。
        let (next, started) = WakeWatch::idle().begin(false);
        assert_eq!(next, WakeWatch { waiting: true });
        assert!(started);
    }

    #[test]
    fn begin_does_not_wait_when_already_on() {
        // すでにオンなら待機しない。残っていた待機があっても終わらせる。
        let (next, started) = WakeWatch::idle().begin(true);
        assert_eq!(next, WakeWatch::idle());
        assert!(!started);
        let (next, started) = WakeWatch { waiting: true }.begin(true);
        assert_eq!(next, WakeWatch::idle());
        assert!(!started);
    }

    #[test]
    fn poll_without_wait_never_notifies() {
        // 待機がないときのPC状態変化では何も送らない(既存の通知と二重にしない)。
        // オン・オフ・期限超過のいずれの入力でも通知なし・状態不変。
        for (online, elapsed) in
            [(false, 0), (true, 0), (false, 179), (false, 180), (true, 10_000)]
        {
            let (next, notice) = WakeWatch::idle().poll(online, elapsed);
            assert_eq!(next, WakeWatch::idle(), "online={online} elapsed={elapsed}");
            assert_eq!(notice, None, "online={online} elapsed={elapsed}");
        }
    }

    #[test]
    fn poll_waiting_off_before_timeout_stays_waiting() {
        // 待機中のオフ・期限前は通知なし・待機継続。
        for elapsed in [0, 1, 179] {
            let (next, notice) = WakeWatch { waiting: true }.poll(false, elapsed);
            assert_eq!(next, WakeWatch { waiting: true }, "elapsed={elapsed}");
            assert_eq!(notice, None, "elapsed={elapsed}");
        }
    }

    #[test]
    fn poll_timeout_boundary_is_inclusive() {
        // 期限ちょうどで発火する(`>=`。`>` だと1秒遅れる)。
        let (_, before) = WakeWatch { waiting: true }.poll(false, WAKE_WATCH_TIMEOUT_SECS - 1);
        assert_eq!(before, None);
        let (next, notice) = WakeWatch { waiting: true }.poll(false, WAKE_WATCH_TIMEOUT_SECS);
        assert_eq!(next, WakeWatch::idle());
        assert_eq!(notice, Some(WakeNotice::TimedOut));
    }

    #[test]
    fn poll_timeout_notifies_only_once() {
        // 期限後は待機なしになるため、同じ入力を繰り返しても重複送信しない。
        let (next, notice) = WakeWatch { waiting: true }.poll(false, WAKE_WATCH_TIMEOUT_SECS);
        assert_eq!(notice, Some(WakeNotice::TimedOut));
        let (again, notice) = next.poll(false, WAKE_WATCH_TIMEOUT_SECS + 60);
        assert_eq!(again, WakeWatch::idle());
        assert_eq!(notice, None);
    }

    #[test]
    fn poll_waiting_online_succeeds() {
        // 待機中のオンで成功・待機終了。経過時間によらず成功する。
        for elapsed in [0, 30, 179, 180, 10_000] {
            let (next, notice) = WakeWatch { waiting: true }.poll(true, elapsed);
            assert_eq!(next, WakeWatch::idle(), "elapsed={elapsed}");
            assert_eq!(notice, Some(WakeNotice::Succeeded), "elapsed={elapsed}");
        }
    }

    #[test]
    fn re_instruction_keeps_waiting() {
        // 待機中の再指示(オフ)は待機継続。期限の更新(開始時刻の置き換え)は
        // 呼び出し側が行い、ここでは待機が終わらないことを担保する。
        let (next, started) = WakeWatch { waiting: true }.begin(false);
        assert_eq!(next, WakeWatch { waiting: true });
        assert!(started);
    }

    #[test]
    fn request_texts_follow_glossary() {
        // 実際の文面を固定する。WOLは内部手段のためユーザー向け文言に使わない。
        assert_eq!(wake_request_text(), "PCの起動を指示しました。");
        assert_eq!(
            wake_request_failed_text(),
            "PCの起動の指示に失敗しました。"
        );
        assert!(!wake_request_text().contains("WOL"));
        assert!(!wake_request_failed_text().contains("WOL"));
    }

    #[test]
    fn succeeded_text_appends_elapsed() {
        // 既存の「PCが起動しました」に所要時間を添える。
        assert_eq!(
            wake_succeeded_text(45),
            "PCが起動しました(所要時間 45秒)。"
        );
        assert_eq!(
            wake_succeeded_text(65),
            "PCが起動しました(所要時間 1分5秒)。"
        );
        assert_eq!(
            wake_succeeded_text(180),
            "PCが起動しました(所要時間 3分)。"
        );
        assert_eq!(
            wake_succeeded_text(0),
            "PCが起動しました(所要時間 0秒)。"
        );
    }

    #[test]
    fn timed_out_text_reports_minutes() {
        // Issue #182指定の「指示から N 分経過」。分未満は切り捨てる。
        assert_eq!(
            wake_timed_out_text(180),
            "PCが起動しませんでした(指示から 3分経過)。"
        );
        assert_eq!(
            wake_timed_out_text(185),
            "PCが起動しませんでした(指示から 3分経過)。"
        );
        assert_eq!(
            wake_timed_out_text(240),
            "PCが起動しませんでした(指示から 4分経過)。"
        );
    }
}
