use m5stack_pc_bridge::auth::NonceStore;
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
