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
    pub const fn from_parts(
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

    pub const fn is_online(&self) -> bool {
        matches!(self, Self::Online(_))
    }

    pub const fn services(&self) -> Services {
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

/// bridgeの `GET /status` 応答の解釈(Issue #183)。`agent_online` の形は
/// wire protocolのためここが正本。
/// `GET /status` は無認証で返すのは固定値だけ。**稼働時間などの環境情報を載せるなら
/// 認証付きの別エンドポイントが要る。**
pub const BRIDGE_STATUS_PATH: &str = "/status";

/// `agent_online` がJSONの `true` のときだけtrueを返す。不正JSON・キー欠落・
/// 想定外の型・空本文はすべてfalse(解釈不能を「応答あり」へ倒さない)。
pub fn bridge_status_online(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return false;
    };
    value
        .get("agent_online")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

#[derive(Clone, Copy, Debug)]
pub struct PcStatusSnapshot {
    pub pc_online: bool,
    pub bridge_online: bool,
    pub locked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcControlStatus {
    Locked,
    Off,
    Ready,
    BridgeUnavailable,
}

impl From<PcStatusSnapshot> for PcControlStatus {
    fn from(snapshot: PcStatusSnapshot) -> Self {
        match snapshot {
            PcStatusSnapshot { locked: true, .. } => Self::Locked,
            PcStatusSnapshot {
                pc_online: false, ..
            } => Self::Off,
            PcStatusSnapshot {
                bridge_online: true,
                ..
            } => Self::Ready,
            _ => Self::BridgeUnavailable,
        }
    }
}

/// `/status` のボタン。`data` は `parse_callback_data` が解釈する固定文字列。
pub struct StatusButton {
    pub label: &'static str,
    pub data: &'static str,
}

pub const STATUS_CB_REFRESH: &str = "status:refresh";
pub const STATUS_CB_WAKE: &str = "status:wake";
pub const STATUS_CB_REBOOT: &str = "status:reboot";
pub const STATUS_CB_SHUTDOWN: &str = "status:shutdown";
/// 設定メニューのロック解除と同じcallback。ロック中でも通る唯一のボタン。
pub const STATUS_CB_UNLOCK: &str = "lock:off";

const REFRESH_ROW: &[StatusButton] = &[StatusButton {
    label: "↻ 最新の状態に更新",
    data: STATUS_CB_REFRESH,
}];

/// 押された時点で意味のあるボタンだけを出す。ただし**古いメッセージのボタンは
/// 後から押せる**ので、呼び出し側はここに出ていないことを根拠にしない。
pub fn status_buttons(status: PcControlStatus) -> Vec<&'static [StatusButton]> {
    let first: &[StatusButton] = match status {
        PcControlStatus::Ready => &[
            StatusButton {
                label: "🔄 PCを再起動",
                data: STATUS_CB_REBOOT,
            },
            StatusButton {
                label: "⏻ シャットダウン",
                data: STATUS_CB_SHUTDOWN,
            },
        ],
        PcControlStatus::Off => &[StatusButton {
            label: "⏻ PCを起動",
            data: STATUS_CB_WAKE,
        }],
        PcControlStatus::Locked => &[StatusButton {
            label: "🔓 ロックを解除",
            data: STATUS_CB_UNLOCK,
        }],
        PcControlStatus::BridgeUnavailable => &[],
    };
    if first.is_empty() {
        vec![REFRESH_ROW]
    } else {
        vec![first, REFRESH_ROW]
    }
}

/// `parse_mode=HTML` で送るため、値に含まれる `&` `<` `>` を実体参照へ置き換える。
pub fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

pub fn status_message_html(
    status: PcControlStatus,
    pc_label: &str,
    battery_text: &str,
    version: &str,
) -> String {
    let actions = match status {
        PcControlStatus::Locked => {
            "🔒 操作をロック中\n解除するまで起動・再起動・シャットダウンはできません"
        }
        PcControlStatus::Off => "起動できます",
        PcControlStatus::Ready => "再起動・シャットダウンができます",
        PcControlStatus::BridgeUnavailable => {
            "⚠️ 再起動・シャットダウンはできません\nPC側の操作サービスが応答していません"
        }
    };
    format!(
        "🖥️ <b>PC ・ {}</b>\n{actions}\n\n📱 <b>M5Stack</b>\n🔋 {}\n🏷️ v{}",
        escape_html(pc_label),
        escape_html(battery_text),
        escape_html(version)
    )
}

#[cfg(test)]
mod bridge_status_tests {
    use super::{BRIDGE_STATUS_PATH, bridge_status_online};

    #[test]
    fn status_path_is_slash_status() {
        // bridge側(`server.rs`)のrouteと一致させること。署名対象ではないため
        // 変えても署名の互換性には影響しないが、確認先がずれる。
        assert_eq!(BRIDGE_STATUS_PATH, "/status");
    }

    #[test]
    fn accepts_agent_online_true_with_extra_keys() {
        // bridgeの実際の応答(`agent`・`status`付き)を受け入れること。
        assert!(bridge_status_online(
            br#"{"agent_online":true,"agent":"m5stack-pc-bridge","status":"ok"}"#
        ));
        // 余分な空白・改行があっても受け入れること。
        assert!(bridge_status_online(b"{\n  \"agent_online\" : true\n}"));
    }

    #[test]
    fn rejects_agent_online_false() {
        assert!(!bridge_status_online(
            br#"{"agent_online":false,"agent":"m5stack-pc-bridge","status":"ok"}"#
        ));
    }

    #[test]
    fn rejects_missing_key() {
        assert!(!bridge_status_online(br#"{"agent":"m5stack-pc-bridge"}"#));
        assert!(!bridge_status_online(b"{}"));
    }

    #[test]
    fn rejects_invalid_json_and_empty_body() {
        assert!(!bridge_status_online(b"{not json"));
        assert!(!bridge_status_online(b""));
        // JSONとしては有効だがオブジェクトではない。
        assert!(!bridge_status_online(b"[true]"));
        assert!(!bridge_status_online(b"true"));
    }

    #[test]
    fn rejects_unexpected_types_without_treating_as_online() {
        // 想定外の型はすべて「応答なし」に倒す。truthyな値でもtrueにしない。
        for body in [
            r#"{"agent_online":"true"}"#,
            r#"{"agent_online":1}"#,
            r#"{"agent_online":null}"#,
            r#"{"agent_online":{}}"#,
            r#"{"agent_online":[]}"#,
        ] {
            assert!(!bridge_status_online(body.as_bytes()), "{body}");
        }
    }
}

#[cfg(test)]
mod status_text_tests {
    use super::{
        PcControlStatus, PcStatusSnapshot, STATUS_CB_REBOOT, STATUS_CB_REFRESH, STATUS_CB_SHUTDOWN,
        STATUS_CB_UNLOCK, STATUS_CB_WAKE, status_buttons, status_message_html,
    };

    fn status(pc_online: bool, bridge_online: bool, locked: bool) -> PcControlStatus {
        PcStatusSnapshot {
            pc_online,
            bridge_online,
            locked,
        }
        .into()
    }

    fn render(
        pc_online: bool,
        bridge_online: bool,
        locked: bool,
        label: &str,
        battery: &str,
    ) -> String {
        status_message_html(
            status(pc_online, bridge_online, locked),
            label,
            battery,
            "0.13.0",
        )
    }

    fn data(status: PcControlStatus) -> Vec<Vec<&'static str>> {
        status_buttons(status)
            .into_iter()
            .map(|row| row.iter().map(|b| b.data).collect())
            .collect()
    }

    #[test]
    fn a_pc_on_ready() {
        assert_eq!(
            render(
                true,
                true,
                false,
                "オン",
                &battery::status_ja(100, battery::PowerState::Powered)
            ),
            "🖥️ <b>PC ・ オン</b>\n再起動・シャットダウンができます\n\n📱 <b>M5Stack</b>\n🔋 100% 満充電・給電中\n🏷️ v0.13.0"
        );
        assert_eq!(
            data(status(true, true, false)),
            vec![
                vec![STATUS_CB_REBOOT, STATUS_CB_SHUTDOWN],
                vec![STATUS_CB_REFRESH]
            ]
        );
    }

    #[test]
    fn b_pc_off() {
        for bridge_online in [false, true] {
            assert_eq!(
                render(
                    false,
                    bridge_online,
                    false,
                    "オフ",
                    &battery::status_ja(78, battery::PowerState::OnBattery)
                ),
                "🖥️ <b>PC ・ オフ</b>\n起動できます\n\n📱 <b>M5Stack</b>\n🔋 78% 電池駆動\n🏷️ v0.13.0"
            );
            assert_eq!(
                data(status(false, bridge_online, false)),
                vec![vec![STATUS_CB_WAKE], vec![STATUS_CB_REFRESH]]
            );
        }
    }

    #[test]
    fn c_bridge_unavailable() {
        assert_eq!(
            render(
                true,
                false,
                false,
                "オン",
                &battery::status_ja(100, battery::PowerState::Powered)
            ),
            "🖥️ <b>PC ・ オン</b>\n⚠️ 再起動・シャットダウンはできません\nPC側の操作サービスが応答していません\n\n📱 <b>M5Stack</b>\n🔋 100% 満充電・給電中\n🏷️ v0.13.0"
        );
        // 電源操作が通らない状態なので、押せるのは更新だけ。
        assert_eq!(
            data(status(true, false, false)),
            vec![vec![STATUS_CB_REFRESH]]
        );
    }

    #[test]
    fn d_locked() {
        assert_eq!(
            render(
                true,
                true,
                true,
                "オン",
                &battery::status_ja(100, battery::PowerState::Powered)
            ),
            "🖥️ <b>PC ・ オン</b>\n🔒 操作をロック中\n解除するまで起動・再起動・シャットダウンはできません\n\n📱 <b>M5Stack</b>\n🔋 100% 満充電・給電中\n🏷️ v0.13.0"
        );
        assert_eq!(
            data(status(true, true, true)),
            vec![vec![STATUS_CB_UNLOCK], vec![STATUS_CB_REFRESH]]
        );
    }

    #[test]
    fn unknown_battery() {
        assert_eq!(
            render(false, false, false, "オフ", "不明"),
            "🖥️ <b>PC ・ オフ</b>\n起動できます\n\n📱 <b>M5Stack</b>\n🔋 不明\n🏷️ v0.13.0"
        );
    }

    #[test]
    fn lock_takes_priority_over_bridge_unavailable() {
        assert_eq!(status(true, false, true), PcControlStatus::Locked);
        assert_eq!(
            data(status(true, false, true)),
            vec![vec![STATUS_CB_UNLOCK], vec![STATUS_CB_REFRESH]]
        );
    }

    #[test]
    fn lock_takes_priority_over_pc_off() {
        assert_eq!(status(false, false, true), PcControlStatus::Locked);
    }

    #[test]
    fn refresh_is_always_available() {
        for s in [
            PcControlStatus::Ready,
            PcControlStatus::Off,
            PcControlStatus::Locked,
            PcControlStatus::BridgeUnavailable,
        ] {
            let rows = data(s);
            assert_eq!(rows.last().unwrap(), &vec![STATUS_CB_REFRESH], "{s:?}");
        }
    }

    #[test]
    fn escapes_html_in_interpolated_values() {
        // 値にHTMLが混ざっても書式を壊さない。
        let text = super::status_message_html(PcControlStatus::Off, "オフ", "<b>&", "0.13.0<");
        assert!(text.contains("🔋 &lt;b&gt;&amp;"), "{text}");
        assert!(text.contains("🏷️ v0.13.0&lt;"), "{text}");
    }
}
