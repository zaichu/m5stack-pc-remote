//! PC状態の安定通知判定の純粋ロジック。
//! `firmware` はESP32専用でhostビルドできないため、判定だけを分離してhostで
//! テストする(`shared/*` 共通の方針)。
//!
//! 画面表示は即座に切り替えるが、Telegram通知だけは同じ結果を `STABLE_POLLS`
//! 回連続で観測してから送る。瞬断やPCの再起動中の短い揺れで連投されないように
//! するため。判定に必要なのは前回状態・連続回数・今回の観測と抑止フラグだけ
//! (STATUS取得・描画・通知送信は呼び出し側に残す)。

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
    pub fn poll(self, now_online: bool, wake_succeeded: bool, suppress_off: bool) -> (Self, bool) {
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
