use m5stack_pc_bridge::auth::NonceStore;
use std::sync::{Arc, Barrier};
use time::OffsetDateTime;

/// MAX_ENTRIES (=10_000) に達した状態でも、期限切れエントリがあれば
/// insert_once 内の evict_expired で間引かれて新規 nonce を受け付けられることを確認する。
/// 時刻は引数で渡す純粋な形でテストし、sleepや実ネットワークは使わない。
#[test]
fn recovers_from_full_store_when_expired_entries_exist() {
    let store = NonceStore::default();
    let ttl_seconds: i64 = 60;
    let base = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();

    // base 時点で 10_000 件を埋める（全て ttl 内なので間引かれない）
    for i in 0..10_000 {
        let nonce = format!("n-{i:05}");
        let ok = store.insert_once(&nonce, base.unix_timestamp(), base, ttl_seconds);
        assert!(ok, "filling {i} should succeed");
    }

    // 期限切れになる未来時刻で新規 nonce を挿入できること
    // 全ての既存エントリは base の timestamp なので、now が ttl を大きく超えれば期限切れ
    let future = base + time::Duration::seconds(1_000);
    let ok = store.insert_once(
        "new-nonce-after-expiry",
        future.unix_timestamp(),
        future,
        ttl_seconds,
    );
    assert!(
        ok,
        "should accept new nonce after evicting expired entries at capacity"
    );

    // さらに別 nonce も受け付けられる（store が縮んだことの追確認）
    let ok2 = store.insert_once(
        "another-nonce",
        future.unix_timestamp(),
        future,
        ttl_seconds,
    );
    assert!(ok2, "store should have shrunk and accept further nonces");
}

/// 期限切れが無い場合は上限で拒否し続けることを確認（上限チェック自体は生きている）
#[test]
fn stays_full_when_no_expired_entries() {
    let store = NonceStore::default();
    let ttl_seconds: i64 = 60;
    let base = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();

    for i in 0..10_000 {
        let nonce = format!("m-{i:05}");
        let ok = store.insert_once(&nonce, base.unix_timestamp(), base, ttl_seconds);
        assert!(ok, "filling {i} should succeed");
    }

    // 未来でもない（期限切れなし）状態では新規 nonce は拒否される
    let ok = store.insert_once(
        "should-be-rejected",
        base.unix_timestamp(),
        base,
        ttl_seconds,
    );
    assert!(
        !ok,
        "should reject when store is full with no expired entries"
    );
}

// ===== Issue #136: nonce 入力検証の固定 =====
//
// `insert_once` の実装は MAX_NONCE_LEN(=64) 超過と使用文字種
// (英数字と `-_.` のみ) を検査する。これらの検査を削除する変異を
// 入れても既存テストは全て緑のままだったため、境界を固定する。

/// MAX_NONCE_LEN(=64) ちょうどは受け付け、65文字は拒否すること。
#[test]
fn rejects_nonce_longer_than_max_len() {
    let store = NonceStore::default();
    let base = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();

    assert!(
        store.insert_once(&"a".repeat(64), base.unix_timestamp(), base, 60),
        "64-char nonce should be accepted"
    );
    assert!(
        !store.insert_once(&"a".repeat(65), base.unix_timestamp(), base, 60),
        "65-char nonce should be rejected"
    );
}

/// 許可された文字種 (英数字と `-_.`) は受け付けること。
#[test]
fn accepts_nonce_with_allowed_characters() {
    let store = NonceStore::default();
    let base = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();

    for (i, nonce) in ["abcXYZ012", "with-dash_and.dot", "---___...", "a"]
        .iter()
        .enumerate()
    {
        assert!(
            store.insert_once(nonce, base.unix_timestamp(), base, 60),
            "allowed nonce {i} ({nonce}) should be accepted"
        );
    }
}

/// 空白や記号を含む nonce は拒否すること。
#[test]
fn rejects_nonce_with_disallowed_characters() {
    let store = NonceStore::default();
    let base = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();

    for nonce in [
        "has space",
        "tab\there",
        "semi;colon",
        "slash/x",
        "bang!",
        "colon:x",
        "at@sign",
        "hash#tag",
        "plus+plus",
        "uni\u{00e7}ode",
    ] {
        assert!(
            !store.insert_once(nonce, base.unix_timestamp(), base, 60),
            "nonce {nonce:?} should be rejected"
        );
    }
}

/// 満杯(上限ちょうど)で期限切れと期限内が混在する場合、新規 nonce を受け付け、
/// 期限内の nonce の再送は引き続き拒否すること (期限内は必ず残す)。
#[test]
fn mixed_capacity_recovery_keeps_unexpired() {
    let store = NonceStore::default();
    let ttl_seconds: i64 = 60;
    let base = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();

    for i in 0..5_000 {
        let nonce = format!("old-{i:05}");
        assert!(
            store.insert_once(&nonce, base.unix_timestamp(), base, ttl_seconds),
            "filling old {i} should succeed"
        );
    }
    let mid = base + time::Duration::seconds(30);
    for i in 0..5_000 {
        let nonce = format!("new-{i:05}");
        assert!(
            store.insert_once(&nonce, mid.unix_timestamp(), mid, ttl_seconds),
            "filling new {i} should succeed"
        );
    }

    // old 側だけが期限切れになる時刻へ進める。
    let later = base + time::Duration::seconds(61);
    assert!(
        store.insert_once("recovered-1", later.unix_timestamp(), later, ttl_seconds),
        "should accept new nonce after evicting expired entries at capacity"
    );

    // 期限内の new 側は掃除されず残るため、再送は拒否される。
    assert!(
        !store.insert_once("new-00000", mid.unix_timestamp(), later, ttl_seconds),
        "unexpired nonce must still be rejected as replay"
    );
}

/// 並行挿入で上限を超えた非倍数サイズに留まっても、全期限切れ後は回復すること。
///
/// 上限検査と挿入の間には競合窓があり、並行時は上限を超えて 100 の倍数でない
/// 件数に留まり得る。その状態では 100 件ごとの掃除が二度と走らず、期限切れが
/// 残っても新規 nonce を拒否し続ける(上限到達時は無条件に掃除して回復する)。
#[test]
fn overfull_store_with_expired_entries_recovers() {
    let store = Arc::new(NonceStore::default());
    let ttl_seconds: i64 = 60;
    let base = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();

    for i in 0..9_999 {
        let nonce = format!("p-{i:05}");
        assert!(
            store.insert_once(&nonce, base.unix_timestamp(), base, ttl_seconds),
            "filling {i} should succeed"
        );
    }

    // 上限直前から複数スレッドで distinct nonce を入れ、上限超過を狙う。
    let threads: usize = 8;
    let attempts: usize = 20;
    let barrier = Arc::new(Barrier::new(threads));
    let handles: Vec<_> = (0..threads)
        .map(|t| {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                for a in 0..attempts {
                    let nonce = format!("r-{t}-{a}");
                    let _ = store.insert_once(&nonce, base.unix_timestamp(), base, ttl_seconds);
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    // 期限切れが無い状態では上限到達 (超過含む) で拒否される。
    assert!(
        !store.insert_once("probe-full", base.unix_timestamp(), base, ttl_seconds),
        "store should be at or over capacity"
    );

    // 全件が期限切れになれば、上限超過でも新規 nonce を受け付ける。
    let later = base + time::Duration::seconds(1_000);
    assert!(
        store.insert_once("after-expiry", later.unix_timestamp(), later, ttl_seconds),
        "should recover after entries expire even when over capacity"
    );
}
