//! Wi-Fi接続とオンラインサービス(SNTP・通知・Telegram polling)の起動状態の遷移。
//! `firmware` はESP32専用でhostビルドできないため、副作用のない判定だけを分離して
//! hostでテストする(`shared/*` 共通の方針)。
//!
//! 呼び出し側(`firmware/src/main.rs`)は `Option`/`bool` の実ハンドルを持ち続け、
//! 判定が必要なときに `OnlineState::from_parts` で状態を組み立てて `transition`
//! へ渡す。Wi-Fiの再接続・SNTPの開始・Telegramスレッドの起動といった副作用は
//! すべて呼び出し側に残し、このcrateは「次に何をすべきか」だけを返す。
//!
//! NTP未同期でも開始要求自体は止めない。電源操作の送信側が未同期の時計を拒否する
//! 挙動(`bridge_client` の時刻検査)はこのcrateの外にあり、変えていない。

/// オンラインサービスの起動済み flags。`OnlineState` の両バリアントが持つ。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Services {
    pub sntp_started: bool,
    pub notifier_started: bool,
    pub telegram_started: bool,
}

/// Wi-Fi・SNTP・通知・Telegramの起動状態を1つにまとめた型。
/// 切断しても起動済みサービスは止めないため、両バリアントが `Services` を持つ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnlineState {
    Offline(Services),
    Online(Services),
}

/// 状態への入力。Wi-Fiリンクの有無だけを見る(NTP同期状態は入力にしない)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnlineEvent {
    WifiConnected,
    WifiDisconnected,
}

/// 呼び出し側が行うべき副作用の一覧。すべてhostテスト可能な純粋な判定結果。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OnlineActions {
    pub start_sntp: bool,
    pub start_notifier: bool,
    pub start_telegram: bool,
}

impl OnlineActions {
    pub fn none() -> Self {
        Self::default()
    }
}

impl OnlineState {
    pub fn initial() -> Self {
        Self::Offline(Services::default())
    }

    /// 呼び出し側の実ハンドル(`Option` の有無と起動済み旗)から組み立てる。
    pub fn from_parts(
        wifi_connected: bool,
        sntp_started: bool,
        notifier_started: bool,
        telegram_started: bool,
    ) -> Self {
        let services = Services {
            sntp_started,
            notifier_started,
            telegram_started,
        };
        if wifi_connected {
            Self::Online(services)
        } else {
            Self::Offline(services)
        }
    }

    pub fn is_online(&self) -> bool {
        matches!(self, Self::Online(_))
    }

    pub fn services(&self) -> Services {
        match *self {
            Self::Online(services) | Self::Offline(services) => services,
        }
    }

    /// 状態と入力イベントから次の状態と行うべき動作を返す。副作用は行わない。
    /// - 接続中は何度呼んでもよい。起動済みの再起動は要求せず、開始に失敗した分
    ///   だけ再試行する(冪等)。
    /// - 未設定のTelegramは開始を要求しない(二重起動の防止と対になる条件)。
    /// - 切断では何も要求せず、起動済み flags は保持する(再接続で再起動しない)。
    pub fn transition(
        self,
        event: OnlineEvent,
        telegram_configured: bool,
    ) -> (Self, OnlineActions) {
        match event {
            OnlineEvent::WifiConnected => {
                let services = self.services();
                let actions = OnlineActions {
                    start_sntp: !services.sntp_started,
                    start_notifier: !services.notifier_started,
                    start_telegram: !services.telegram_started && telegram_configured,
                };
                (Self::Online(services), actions)
            }
            OnlineEvent::WifiDisconnected => {
                (Self::Offline(self.services()), OnlineActions::none())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_services() -> Services {
        Services {
            sntp_started: true,
            notifier_started: true,
            telegram_started: true,
        }
    }

    #[test]
    fn offline_to_online_requests_all_starts() {
        // 未接続→接続の初回は3つとも開始要求する。
        let (next, actions) = OnlineState::initial().transition(OnlineEvent::WifiConnected, true);
        assert_eq!(next, OnlineState::Online(Services::default()));
        assert_eq!(
            actions,
            OnlineActions {
                start_sntp: true,
                start_notifier: true,
                start_telegram: true,
            }
        );
        assert!(next.is_online());
    }

    #[test]
    fn offline_to_online_without_telegram_config_skips_telegram() {
        // Telegram未設定では他だけ開始し、pollingスレッドを作らない。
        let (next, actions) = OnlineState::initial().transition(OnlineEvent::WifiConnected, false);
        assert!(next.is_online());
        assert!(actions.start_sntp);
        assert!(actions.start_notifier);
        assert!(!actions.start_telegram);
    }

    #[test]
    fn connected_poll_retries_only_failed_starts() {
        // 接続中の再判定は冪等。SNTP開始に失敗していた分だけ再試行する。
        let state = OnlineState::from_parts(true, false, true, true);
        let (next, actions) = state.transition(OnlineEvent::WifiConnected, true);
        assert_eq!(next, state);
        assert!(actions.start_sntp);
        assert!(!actions.start_notifier);
        assert!(!actions.start_telegram);
    }

    #[test]
    fn telegram_never_starts_twice() {
        // 起動済みのTelegramは設定の有無に関わらず再起動を要求しない。
        for configured in [false, true] {
            let state = OnlineState::Online(full_services());
            let (next, actions) = state.transition(OnlineEvent::WifiConnected, configured);
            assert_eq!(next, state);
            assert_eq!(actions, OnlineActions::none());
        }
    }

    #[test]
    fn disconnect_keeps_services_without_actions() {
        // 接続→切断では何も要求せず、起動済み flags を保持する。
        let (next, actions) =
            OnlineState::Online(full_services()).transition(OnlineEvent::WifiDisconnected, true);
        assert_eq!(next, OnlineState::Offline(full_services()));
        assert_eq!(actions, OnlineActions::none());
        assert!(!next.is_online());
    }

    #[test]
    fn reconnect_does_not_restart_services() {
        // 切断→再接続では再起動を要求しない(起動済み flags が残っているため)。
        let (next, actions) =
            OnlineState::Offline(full_services()).transition(OnlineEvent::WifiConnected, true);
        assert_eq!(next, OnlineState::Online(full_services()));
        assert_eq!(actions, OnlineActions::none());
    }

    #[test]
    fn repeated_disconnect_is_stable() {
        // 未接続のままの切断通知では状態も動作も変わらない。
        let (next, actions) =
            OnlineState::initial().transition(OnlineEvent::WifiDisconnected, true);
        assert_eq!(next, OnlineState::initial());
        assert_eq!(actions, OnlineActions::none());
    }

    #[test]
    fn sntp_start_is_requested_without_waiting_for_sync() {
        // NTP同期状態は遷移の入力にない。未同期でもSNTP開始だけは要求し、
        // 同期待ちでUIループを止めない。電源操作の拒否は送信側(`bridge_client`
        // の時刻検査)が担い、このcrateは触らない。
        let (_, actions) = OnlineState::initial().transition(OnlineEvent::WifiConnected, true);
        assert!(actions.start_sntp);
    }

    #[test]
    fn from_parts_round_trips_flags() {
        // 呼び出し側の実ハンドルとの対応がずれていないこと。
        let state = OnlineState::from_parts(false, true, false, true);
        assert!(!state.is_online());
        assert_eq!(
            state.services(),
            Services {
                sntp_started: true,
                notifier_started: false,
                telegram_started: true,
            }
        );
        let state = OnlineState::from_parts(true, false, false, false);
        assert!(state.is_online());
        assert_eq!(state.services(), Services::default());
    }
}
