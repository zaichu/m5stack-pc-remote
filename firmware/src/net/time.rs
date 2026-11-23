use std::error::Error;
use std::time::Duration;

/// これより古いUNIX時刻(2023-11-14より前)はNTP未同期とみなす。
/// 直接比較せず `is_ntp_synced` を使うこと。
const MIN_VALID_UNIX_TIME: u64 = 1_700_000_000;

/// NTP同期済みとみなせる時刻か。判定式を1関数へ寄せる(分散すると閾値変更の
/// 更新漏れや `<`/`>=`・符号の取り違えが起きる)。
pub const fn is_ntp_synced(unix_seconds: i64) -> bool {
    unix_seconds >= MIN_VALID_UNIX_TIME as i64
}

/// Telegram polling開始前のNTP待ち上限。未同期でも電源操作は送信側が拒否する
/// ため、長く待たずpollingを開始する(Issue #248)。
pub const NTP_SYNC_WAIT: Duration = Duration::from_secs(2);

/// SNTPでシステム時刻を同期する。bridgeはtimestampを検証するため、署名付き
/// REBOOT/SHUTDOWNは時刻同期後だけ成功する。返したhandleは保持する。
pub fn start_sntp() -> Result<esp_idf_svc::sntp::EspSntp<'static>, Box<dyn Error>> {
    Ok(esp_idf_svc::sntp::EspSntp::new_default()?)
}

/// NTP同期済みに見えるまで待つ。timeoutした場合も呼び出し側は処理を続ける。
pub fn wait_for_time_sync(timeout: Duration) -> bool {
    use std::time::{SystemTime, UNIX_EPOCH};

    let start = std::time::Instant::now();
    loop {
        let synced = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .is_ok_and(|d| is_ntp_synced(d.as_secs() as i64));
        if synced {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_and_negative_are_not_synced() {
        assert!(!is_ntp_synced(0));
        assert!(!is_ntp_synced(-1));
    }

    #[test]
    fn current_time_is_synced() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before UNIX epoch")
            .as_secs() as i64;
        assert!(is_ntp_synced(now));
        assert!(is_ntp_synced(now + 60));
    }
}
