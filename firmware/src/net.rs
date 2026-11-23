// Wi-Fi、Wake-on-LAN、STATUS相当の疎通確認。
//
// 共有の定数・型の置き場にし、外部向けの参照名(`net::Wifi` 等)は
// 再エクスポートで維持する。振る舞いは変えず、関数の中身は移すだけ。

pub(crate) mod labels;
pub(crate) mod reachability;
pub(crate) mod time;
pub(crate) mod wifi;
pub(crate) mod wol;

pub use labels::{pc_online_label_ascii, pc_online_label_ja, pc_state_notification_ja};
pub use reachability::{
    STATUS_PROBE_TIMEOUT, check_pc_online, check_pc_online_cached, resolve_status_addr_once,
};
pub use time::{NTP_SYNC_WAIT, is_ntp_synced, start_sntp, wait_for_time_sync};
pub use wifi::Wifi;
pub use wol::send_wake_on_lan;
