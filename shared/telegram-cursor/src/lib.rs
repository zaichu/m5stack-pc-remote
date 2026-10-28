//! Telegram `getUpdates` の offset 進行の純粋ロジック(Issue #250)。
//! `firmware` はESP32専用でhostビルドできないため、取りこぼしが起きないこと
//! (offset の進め方)だけを分離してhostでテストする(`shared/*` 共通の方針)。

/// 1回の `getUpdates` で取る件数。
/// 1件ずつ取り切ることで応答全体を小さく保ち、ESP32のヒープピークを下げる。
/// 取りこぼしは起きない(offsetは処理した分だけ進み、次回pollで続きを取る)。
pub const GET_UPDATES_LIMIT: u32 = 1;

/// `getUpdates` 応答の受信上限(バイト)。
/// `limit=1` でも1件のupdateは長い本文(最大4096文字。日本語はJSON escapeで
/// 膨らむ)で数十KBになり得るため、下げるとoffsetが進まず同じupdateで毎回
/// エラーになりpollingが詰まる。32KBのまま残す。
pub const GET_UPDATES_MAX_BYTES: usize = 32 * 1024;

/// `getUpdates` の offset 進行状態。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UpdateCursor {
    next_offset: i64,
    synced: bool,
}

impl UpdateCursor {
    pub fn new() -> Self {
        Self::default()
    }

    /// 次回 `getUpdates` へ渡す `offset`。
    pub fn offset(&self) -> i64 {
        self.next_offset
    }

    /// 1回分の `result` の update_id 列を受けて offset を進める。
    /// 戻り値はその回分を処理してよいか。起動直後の未同期中は古い命令を
    /// 実行せず、空応答(=滞留なし)が返るまで同期中とする。
    pub fn advance<I>(&mut self, update_ids: I) -> bool
    where
        I: IntoIterator<Item = i64>,
    {
        let mut empty = true;
        for id in update_ids {
            empty = false;
            if id >= self.next_offset {
                self.next_offset = id + 1;
            }
        }
        let dispatch = self.synced;
        if empty {
            self.synced = true;
        }
        dispatch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_is_one() {
        // ヒープピーク削減の要。変えたら実機のヒープ再確認が要る。
        assert_eq!(GET_UPDATES_LIMIT, 1);
    }

    #[test]
    fn max_bytes_covers_worst_single_update() {
        // 1件のupdateは長い本文で数十KBになり得る。下げる変更は
        // polling詰まりの回帰リスクがあるため値を固定して検知する。
        assert_eq!(GET_UPDATES_MAX_BYTES, 32 * 1024);
    }

    #[test]
    fn first_batch_only_advances_offset() {
        // 起動前に届いた古い命令を実行しない。
        let mut cursor = UpdateCursor::new();
        assert!(!cursor.advance([10]));
        assert_eq!(cursor.offset(), 11);
    }

    #[test]
    fn backlog_drains_before_dispatch() {
        // limit=1 で滞留を1件ずつ吐き出し、空応答で同期完了。
        // 完了前の分は実行しない。
        let mut cursor = UpdateCursor::new();
        assert!(!cursor.advance([10]));
        assert!(!cursor.advance([11]));
        assert!(!cursor.advance([]));
        assert_eq!(cursor.offset(), 12);
        // 同期後は新しい分を処理する。
        assert!(cursor.advance([12]));
        assert_eq!(cursor.offset(), 13);
    }

    #[test]
    fn offset_never_goes_back() {
        // 重複・古い id でoffsetを戻さない(取りこぼし防止)。
        let mut cursor = UpdateCursor::new();
        assert!(!cursor.advance([10]));
        assert!(!cursor.advance([10]));
        assert!(!cursor.advance([9]));
        assert_eq!(cursor.offset(), 11);
    }

    #[test]
    fn empty_first_poll_syncs_immediately() {
        // 滞留が無ければ最初の空応答で同期完了し、次回から処理する。
        let mut cursor = UpdateCursor::new();
        assert!(!cursor.advance([]));
        assert!(cursor.advance([5]));
        assert_eq!(cursor.offset(), 6);
    }
}
