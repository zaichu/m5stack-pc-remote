use m5stack_pc_bridge::auth::AuthConfig;

// Issue #136: `AuthConfig` は secret を含むため `Debug` は手書きで
// secret を `[REDACTED]` に置き換える。secret 実値を出力する変異を
// 入れても既存テストは全て緑のままだったため、ここで固定する。
// テスト用の secret は既存テストと同じダミー値を使う。
#[test]
fn auth_config_debug_does_not_leak_secret() {
    let cfg = AuthConfig {
        secret: b"0123456789abcdef0123456789abcdef".to_vec(),
        allowed_skew_seconds: 60,
    };

    let rendered = format!("{cfg:?}");

    // secret の実値 (ASCII 文字列としてもバイト列としても) を含まないこと。
    assert!(
        !rendered.contains("0123456789abcdef"),
        "rendered={rendered}"
    );
    // redact マーカーで置き換えられていること (Debug 実装の変更自体を検出する)。
    assert!(rendered.contains("[REDACTED]"), "rendered={rendered}");
    // 非 secret の field は通常どおり出力されること。
    assert!(rendered.contains("AuthConfig"), "rendered={rendered}");
    assert!(rendered.contains("60"), "rendered={rendered}");
}
