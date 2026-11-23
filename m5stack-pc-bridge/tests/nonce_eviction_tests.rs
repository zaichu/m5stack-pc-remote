use m5stack_pc_bridge::auth::NonceStore;
use std::sync::{Arc, Barrier};
use time::OffsetDateTime;

fn fill_until_full(
    store: &NonceStore,
    base: OffsetDateTime,
    ttl_seconds: i64,
    prefix: &str,
) -> usize {
    let mut count = 0;
    let mut reached_full = false;
    for i in 0..1_000_000 {
        let nonce = format!("{prefix}-{i:06}");
        if !store.insert_once(&nonce, base.unix_timestamp(), base, ttl_seconds) {
            reached_full = true;
            break;
        }
        count += 1;
    }
    assert!(reached_full, "store should reach capacity within bound");
    count
}

#[test]
fn recovers_from_full_store_when_expired_entries_exist() {
    let store = NonceStore::default();
    let ttl_seconds: i64 = 60;
    let base = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();

    let filled = fill_until_full(&store, base, ttl_seconds, "n");
    assert!(filled > 0, "filling should reach capacity");

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

    let ok2 = store.insert_once(
        "another-nonce",
        future.unix_timestamp(),
        future,
        ttl_seconds,
    );
    assert!(ok2, "store should have shrunk and accept further nonces");
}

#[test]
fn stays_full_when_no_expired_entries() {
    let store = NonceStore::default();
    let ttl_seconds: i64 = 60;
    let base = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();

    let filled = fill_until_full(&store, base, ttl_seconds, "m");
    assert!(filled > 0, "filling should reach capacity");

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

#[test]
fn mixed_capacity_recovery_keeps_unexpired() {
    let store = NonceStore::default();
    let ttl_seconds: i64 = 60;
    let base = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let mid = base + time::Duration::seconds(30);

    let mut kept_mid_nonce = String::new();
    let mut reached_full = false;
    for i in 0..1_000_000 {
        let at = if i % 2 == 0 { base } else { mid };
        let nonce = format!("mix-{i:06}");
        if !store.insert_once(&nonce, at.unix_timestamp(), at, ttl_seconds) {
            reached_full = true;
            break;
        }
        if kept_mid_nonce.is_empty() && i % 2 == 1 {
            kept_mid_nonce = nonce;
        }
    }
    assert!(reached_full, "mixed fill should reach capacity");
    assert!(
        !kept_mid_nonce.is_empty(),
        "mixed fill should include unexpired entries"
    );
    assert!(
        !kept_mid_nonce.is_empty(),
        "mixed fill should reach capacity"
    );

    let later = base + time::Duration::seconds(61);
    assert!(
        store.insert_once("recovered-1", later.unix_timestamp(), later, ttl_seconds),
        "should accept new nonce after evicting expired entries at capacity"
    );

    assert!(
        !store.insert_once(&kept_mid_nonce, mid.unix_timestamp(), later, ttl_seconds),
        "unexpired nonce must still be rejected as replay"
    );
}

#[test]
fn overfull_store_with_expired_entries_recovers() {
    let store = Arc::new(NonceStore::default());
    let ttl_seconds: i64 = 60;
    let base = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();

    let capacity = {
        let probe = NonceStore::default();
        fill_until_full(&probe, base, ttl_seconds, "cap-probe")
    };
    for i in 0..capacity.saturating_sub(1) {
        let nonce = format!("p-{i:06}");
        assert!(
            store.insert_once(&nonce, base.unix_timestamp(), base, ttl_seconds),
            "prefill {i} should succeed"
        );
    }

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

    assert!(
        !store.insert_once("probe-full", base.unix_timestamp(), base, ttl_seconds),
        "store should be at or over capacity"
    );

    let later = base + time::Duration::seconds(1_000);
    assert!(
        store.insert_once("after-expiry", later.unix_timestamp(), later, ttl_seconds),
        "should recover after entries expire even when over capacity"
    );
}
