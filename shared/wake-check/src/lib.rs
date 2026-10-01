//! PCの起動指示後の待機状態と期限判定の純粋ロジック。
//! `firmware` はESP32専用でhostビルドできないため、判定だけを分離してhostで
//! テストする(`shared/*` 共通の方針)。時刻は扱わず単調時計の経過秒だけを受け取る
//! (NTP未同期でも壊れない)。

/// 起動指示後にPCがオンになるのを待つ期限(秒)。
/// Windows起動1〜2分 + STATUS安定判定20秒 + bridge起動のばらつきを見込んだ3分。
/// **実機未測定。** ずれる場合はこの定数だけを変える(判定式は変えない)。
pub const WAKE_WATCH_TIMEOUT_SECS: u64 = 180;

/// 起動指示後の待機状態。`firmware` 側は開始時刻を `Option<Instant>`
/// (`None`=待機なし)として持ち、`waiting=is_some()` と対応する。
/// 時刻自体は持たず、経過秒だけを `poll` へ渡す。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WakeWatch {
    pub waiting: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeNotice {
    /// 待機中にPCがオンになった。既存の「PCが起動しました。」に所要時間を添えて送る。
    Succeeded,
    /// 期限までにPCがオンにならなかった。1回だけ送る。
    TimedOut,
}

impl WakeWatch {
    pub fn idle() -> Self {
        Self { waiting: false }
    }

    /// 起動指示時の更新。WOL送信に成功した呼び出し側だけが呼ぶ。
    /// オンなら待機しない(残っていた待機は終わらせる)。オフなら待機開始。
    /// 再指示時の期限更新(開始時刻の置き換え)は呼び出し側が行うため、ここでは待機継続を返す。
    /// 戻り値は(次の状態, 待機を開始したか)。
    pub fn begin(self, pc_online: bool) -> (Self, bool) {
        if pc_online {
            (Self { waiting: false }, false)
        } else {
            (Self { waiting: true }, true)
        }
    }

    /// STATUS確認周期ごとの更新。`elapsed_secs` は指示からの経過秒(単調時計由来)。
    /// 戻り値は(次の状態, 通知の要否と種別)。
    /// 待機なしでは何も送らない(既存の状態変化通知と二重になるため)。
    /// 成功・期限到達のどちらでも待機を終わらせる(期限通知は1回だけ)。
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

/// 起動指示を受け付けたときの応答文(日本語)。用語は `docs/glossary.md` が正本。
/// WOLは内部手段のため文言に使わない。
pub fn wake_request_text() -> &'static str {
    "PCの起動を指示しました。"
}

/// 起動指示自体に失敗したときの応答文(日本語)。成功文と対になる書き出しにする。
pub fn wake_request_failed_text() -> &'static str {
    "PCの起動の指示に失敗しました。"
}

/// 待機中にPCがオンになったときの通知文(日本語)。
/// 「PCが起動しました。」へ所要時間を添えたもの。
pub fn wake_succeeded_text(elapsed_secs: u64) -> String {
    format!(
        "PCが起動しました(所要時間 {})。",
        format_elapsed_ja(elapsed_secs)
    )
}

/// 期限到達時の通知文(日本語)。Issue #182指定の「指示から N 分経過」を含める。
/// 分未満は切り捨てる(180秒→「3分」)。
pub fn wake_timed_out_text(elapsed_secs: u64) -> String {
    format!(
        "PCが起動しませんでした(指示から {}分経過)。",
        elapsed_secs / 60
    )
}

/// 経過秒の日本語表記。分が0なら秒だけ、秒が0なら分だけ出す(「3分0秒」を避ける)。
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
        for (online, elapsed) in [
            (false, 0),
            (true, 0),
            (false, 179),
            (false, 180),
            (true, 10_000),
        ] {
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
        assert_eq!(wake_request_failed_text(), "PCの起動の指示に失敗しました。");
        assert!(!wake_request_text().contains("WOL"));
        assert!(!wake_request_failed_text().contains("WOL"));
    }

    #[test]
    fn succeeded_text_appends_elapsed() {
        // 既存の「PCが起動しました」に所要時間を添える。
        assert_eq!(wake_succeeded_text(45), "PCが起動しました(所要時間 45秒)。");
        assert_eq!(
            wake_succeeded_text(65),
            "PCが起動しました(所要時間 1分5秒)。"
        );
        assert_eq!(wake_succeeded_text(180), "PCが起動しました(所要時間 3分)。");
        assert_eq!(wake_succeeded_text(0), "PCが起動しました(所要時間 0秒)。");
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
