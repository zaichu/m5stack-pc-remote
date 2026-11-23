/// PCの死活状態の日本語表記。用語は `docs/glossary.md` が正本
/// (状態は「オン/オフ」と書き、「オンライン/オフライン」は使わない)。
pub const fn pc_online_label_ja(online: bool) -> &'static str {
    if online { "オン" } else { "オフ" }
}

/// PCの状態変化の通知文。出来事(起動/停止)で書く(状態表示は `pc_online_label_ja`)。
pub const fn pc_state_notification_ja(online: bool) -> &'static str {
    if online {
        "PCが起動しました。"
    } else {
        "PCが停止しました。"
    }
}

/// 画面表示用のASCII表記(`mono_font::ascii` は非ASCIIを `?` へ落とすため)。
pub const fn pc_online_label_ascii(online: bool) -> &'static str {
    if online { "ONLINE" } else { "OFFLINE" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_and_offline_map_to_different_non_empty_labels() {
        assert_ne!(pc_online_label_ja(true), pc_online_label_ja(false));
        assert!(!pc_online_label_ja(true).is_empty());
        assert!(!pc_online_label_ja(false).is_empty());
    }

    #[test]
    fn boot_and_stop_notifications_differ() {
        assert_ne!(
            pc_state_notification_ja(true),
            pc_state_notification_ja(false)
        );
    }

    #[test]
    fn ascii_labels_are_ascii_and_differ() {
        assert!(pc_online_label_ascii(true).is_ascii());
        assert!(pc_online_label_ascii(false).is_ascii());
        assert_ne!(pc_online_label_ascii(true), pc_online_label_ascii(false));
    }
}
