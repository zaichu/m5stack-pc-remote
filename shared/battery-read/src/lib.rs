//! バッテリー読み出し要否の純粋判定。
//! `firmware` はESP32専用でhostビルドできないため、判定だけを分離してhostで
//! テストする(`shared/*` 共通の方針)。
//!
//! 1秒ごとのIRQラッチ確認でラッチが立っていればすぐ読み、立っていなければ
//! 10秒の保険で読む。I2Cの読み出し・ラッチのクリア・描画・通知は呼び出し側
//! (`firmware/src/main.rs`)に残し、ループの順序と周期は変えない。

use core::time::Duration;

/// ラッチ無しでもフル読み出しを行う間隔。取りこぼしの保険。
/// `firmware/src/main.rs` の `STATUS_INTERVAL`(10秒)と等価。
pub const FULL_READ_INTERVAL: Duration = Duration::from_secs(10);

/// フル読み出しが必要かを返す。
/// - `pending` はIRQラッチの有無(読み失敗時は呼び出し側が安全側のtrueへ倒す)。
/// - `elapsed_since_read` は前回のフル読み出しからの経過時間(単調時計由来)。
pub fn should_read(pending: bool, elapsed_since_read: Duration) -> bool {
    pending || elapsed_since_read >= FULL_READ_INTERVAL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_is_ten_seconds() {
        // 保険間隔の変更は読み頻度・I2C負荷に直結するため値を固定する。
        // `main.rs` の `STATUS_INTERVAL` と等価。
        assert_eq!(FULL_READ_INTERVAL, Duration::from_secs(10));
    }

    #[test]
    fn pending_reads_without_waiting_for_interval() {
        // ラッチ有りは保険期限前でもすぐ読む(抜き挿しを約1秒で反映するため)。
        for elapsed in [
            Duration::ZERO,
            Duration::from_secs(1),
            Duration::from_secs(9),
            FULL_READ_INTERVAL,
            Duration::from_secs(3_600),
        ] {
            assert!(should_read(true, elapsed), "elapsed={elapsed:?}");
        }
    }

    #[test]
    fn no_pending_waits_for_interval() {
        // ラッチ無しは保険期限まで読まない(I2C負荷を抑えるため)。
        for elapsed in [
            Duration::ZERO,
            Duration::from_secs(1),
            Duration::from_millis(9_999),
        ] {
            assert!(!should_read(false, elapsed), "elapsed={elapsed:?}");
        }
    }

    #[test]
    fn interval_boundary_is_inclusive() {
        // 期限ちょうどで読む(`>=`。`>` だと1周期遅れる)。
        assert!(!should_read(
            false,
            FULL_READ_INTERVAL
                .checked_sub(Duration::from_nanos(1))
                .unwrap()
        ));
        assert!(should_read(false, FULL_READ_INTERVAL));
    }

    #[test]
    fn no_pending_reads_after_interval() {
        // ラッチ無しでも期限超過後は読む(取りこぼしの保険)。
        for elapsed in [
            Duration::from_secs(11),
            Duration::from_secs(3_600),
            Duration::MAX,
        ] {
            assert!(should_read(false, elapsed), "elapsed={elapsed:?}");
        }
    }
}
