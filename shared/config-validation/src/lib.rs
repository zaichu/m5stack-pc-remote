//! firmwareの実行時設定値(Telegramから変更可能なもの)のvalidation。
//!
//! `firmware` はxtensa-esp32-espidf専用のbinary crateで、host上でビルド・
//! テストできない(`[[bin]]` のみで `[lib]` を持たず、`esp_idf_hal` 等を
//! ソース側で無条件importしているため)。入力検証ロジックだけをここへ分離
//! することで、実ネットワーク・実機なしでhost側のテストを回せるようにする
//! (Issue #78、AGENTS.mdの方針に沿う)。
//!
//! 扱うのは文字列の形式チェックのみで、DNS解決やネットワーク接続はしない。
//! 呼び出し側(firmware)が確認nonce発行前にこれを通し、確認フローと
//! NVSへの永続化は担当しない。

use std::net::Ipv4Addr;
use std::str::FromStr;

/// `pc_ip_address` として妥当なIPv4アドレスか検証する。
///
/// ホスト名は受け付けない。m5stack-pc-bridgeへのHTTP接続先を直接組み立てる
/// ため、ここでDNS解決の失敗経路を増やしたくない。
pub fn validate_ipv4(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    Ipv4Addr::from_str(trimmed)
        .map(|_| trimmed.to_string())
        .map_err(|_| format!("`{trimmed}` はIPv4アドレスとして解釈できません(例: 192.168.1.50)"))
}

/// `pc_status_addr` として妥当な `host:port` 形式か検証する。
///
/// DNS解決はしない。確認nonce発行のたびに名前解決すると、遅い・失敗する
/// DNSでUI・Telegram応答が止まるため。
///
/// 方針(Issue #130-3): host部分はIPv4リテラル限定とし、ホスト名は受け付けない。
/// 「IPリテラル限定」と「解決の別スレッド化」の二択で前者を選んだ。理由:
/// - STATUS確認はUIループ(10秒毎)とTelegram応答生成の直列経路で呼ばれ、
///   別スレッド化は結果待ちの同期・スレッド生存管理・NVS書換との競合を増やす。
///   家庭LAN内のPC相手にそこまでの複雑さは要らない。
/// - `pc_ip_address` 側は既にIPv4限定で実績があり、扱いをそろえられる。
/// - このcrateの契約自体が「DNS解決はしない」であり、ホスト名を許すのは
///   契約違反だった(以前はテストでホスト名許容を明示していた)。
/// ホスト名が本当に必要になったら別スレッド化を検討する(要設計レビュー)。
pub fn validate_status_addr(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    let Some((host, port)) = trimmed.rsplit_once(':') else {
        return Err(format!(
            "`{trimmed}` は host:port 形式で指定してください(例: 192.168.1.50:80)"
        ));
    };
    if host.parse::<Ipv4Addr>().is_err() {
        return Err(format!(
            "`{trimmed}` のhostはIPv4アドレスで指定してください(例: 192.168.1.50:80)"
        ));
    }
    if validate_port(port).is_err() {
        return Err(format!("`{trimmed}` のport部分が不正です(1-65535)"));
    }
    Ok(trimmed.to_string())
}

/// `wol_port` として妥当なport番号か検証する。
pub fn validate_wol_port(input: &str) -> Result<u16, String> {
    validate_port(input.trim())
}

/// Telegram許可ユーザーIDの前後空白を取り除く。
///
/// firmware側の `is_configured` は `trim()` して判定するのに、chat_id化と
/// ID照合がtrimしていなかったため、値の前後空白混入で正規ユーザーが
/// 「権限がありません」で全拒否される事故があった(Issue #130-1)。
/// 3箇所が別々に `trim` すると将来またずれるため、正規化はこの関数に
/// 一本化し、呼び出し側は生文字列を直接触らないこと。
pub fn normalize_telegram_user_id(input: &str) -> &str {
    input.trim()
}

/// 正規化してから `from.id` と一致するかを判定する。
///
/// 既存の照合が文字列比較だったため、数値化せず文字列比較にそろえる
/// (挙動を変えないため。`+123` のような表記ゆれは設定ミスとして拒否側に倒す)。
pub fn telegram_user_id_matches(config_value: &str, from_id: i64) -> bool {
    from_id.to_string() == normalize_telegram_user_id(config_value)
}

/// private chatの送信先(chat_id)として使える数値IDを取り出す。
/// 前後空白は許容するが、空文字や非数値は `None` になる。
pub fn parse_telegram_user_id(config_value: &str) -> Option<i64> {
    normalize_telegram_user_id(config_value).parse::<i64>().ok()
}

/// OTAバイナリの受信サイズがmanifestの申告サイズを超えたかの判定。
///
/// 超えた時点でこれ以上読んでも正規imageにならないため、呼び出し側は
/// flashへ書かず即座に打ち切ること(Issue #130-2)。終端まで書いてから
/// 突き合わせで落とすと、slot上限まで無駄な消去・書き込みが続く。
/// 早期return時はOTAハンドルのDropがabortするため、boot切替は起きない。
///
/// firmware crateはESP-IDF専用でhostテストできないため、hostで検証できる
/// ようこのcrateへ置く(境界値のテストは下の `mod tests` を参照)。
pub fn ota_received_too_large(received: u64, manifest_size: u64) -> bool {
    received > manifest_size
}

fn validate_port(input: &str) -> Result<u16, String> {
    let port: u16 = input
        .parse()
        .map_err(|_| format!("`{input}` はport番号(1-65535)ではありません"))?;
    if port == 0 {
        return Err("portは1以上を指定してください".to_string());
    }
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_ipv4() {
        assert_eq!(validate_ipv4("192.168.1.50").unwrap(), "192.168.1.50");
        // 前後の空白は許容してtrimする(Telegramのコピペ経由の値を想定)。
        assert_eq!(validate_ipv4("  192.168.1.50 \n").unwrap(), "192.168.1.50");
    }

    #[test]
    fn rejects_non_ipv4() {
        assert!(validate_ipv4("not-an-ip").is_err());
        assert!(validate_ipv4("192.168.1.256").is_err());
        assert!(validate_ipv4("").is_err());
        // ホスト名は対象外(このcrateの責務としてDNSを引かない)。
        assert!(validate_ipv4("my-pc.local").is_err());
        // IPv6も対象外(pc_ip_addressはIPv4専用として組み立てる)。
        assert!(validate_ipv4("::1").is_err());
    }

    #[test]
    fn accepts_valid_status_addr() {
        assert_eq!(
            validate_status_addr("192.168.1.50:80").unwrap(),
            "192.168.1.50:80"
        );
        // 前後の空白は許容してtrimする(Telegramのコピペ経由の値を想定)。
        assert_eq!(
            validate_status_addr("  192.168.1.50:8080 \n").unwrap(),
            "192.168.1.50:8080"
        );
    }

    #[test]
    fn rejects_hostname_status_addr() {
        // Issue #130-3: IPv4リテラル限定。ホスト名はUIループ上のDNS解決に
        // 繋がるため受け付けない(以前は許容していたが、方針変更で拒否へ)。
        assert!(
            validate_status_addr("my-pc.local:8080").is_err(),
            "ホスト名は拒否する"
        );
    }

    #[test]
    fn rejects_malformed_status_addr() {
        assert!(validate_status_addr("192.168.1.50").is_err(), "portが無い");
        assert!(validate_status_addr(":80").is_err(), "hostが無い");
        assert!(validate_status_addr("192.168.1.50:0").is_err(), "port 0");
        assert!(
            validate_status_addr("192.168.1.50:70000").is_err(),
            "port範囲外"
        );
        assert!(
            validate_status_addr("192.168.1.50:abc").is_err(),
            "portが数値でない"
        );
        assert!(
            validate_status_addr("bad host:80").is_err(),
            "hostに空白を含む"
        );
    }

    #[test]
    fn accepts_valid_wol_port() {
        assert_eq!(validate_wol_port("9").unwrap(), 9);
        assert_eq!(validate_wol_port(" 65535 ").unwrap(), 65535);
    }

    #[test]
    fn rejects_invalid_wol_port() {
        assert!(validate_wol_port("0").is_err(), "0はport指定として無効");
        assert!(validate_wol_port("65536").is_err(), "u16範囲外");
        assert!(validate_wol_port("-1").is_err(), "負数");
        assert!(validate_wol_port("nine").is_err(), "数値でない");
        assert!(validate_wol_port("").is_err(), "空文字");
    }

    #[test]
    fn normalizes_telegram_user_id() {
        // Issue #130-1: 前後空白を除いた値で判定・照合・パースをそろえる。
        assert_eq!(normalize_telegram_user_id("  12345 \n"), "12345");
        assert_eq!(normalize_telegram_user_id("12345"), "12345");
        assert_eq!(normalize_telegram_user_id(""), "");
    }

    #[test]
    fn matches_telegram_user_id_with_surrounding_whitespace() {
        // 設定値に空白が混じっても正規ユーザーを拒否しない。
        // 旧実装(`from_id.to_string() != config値` の直接比較)はここで不一致になった。
        assert!(telegram_user_id_matches("12345", 12345));
        assert!(telegram_user_id_matches("  12345 \n", 12345));
        assert!(!telegram_user_id_matches("  12345 \n", 54321));
        assert!(!telegram_user_id_matches("", 12345));
    }

    #[test]
    fn parses_telegram_user_id_with_surrounding_whitespace() {
        // 旧実装(`config値.parse::<i64>()` の直接パース)は空白混じりで
        // 失敗し、notifierが起動しなかった。
        assert_eq!(parse_telegram_user_id("12345"), Some(12345));
        assert_eq!(parse_telegram_user_id("  12345 \n"), Some(12345));
        assert_eq!(parse_telegram_user_id(""), None);
        assert_eq!(parse_telegram_user_id("not-a-number"), None);
    }

    #[test]
    fn detects_ota_oversize() {
        // Issue #130-2: 超過した瞬間に打ち切るため、境界は `>` である。
        assert!(!ota_received_too_large(0, 0));
        assert!(!ota_received_too_large(100, 100), "一致は打ち切らない");
        assert!(!ota_received_too_large(99, 100));
        assert!(ota_received_too_large(101, 100), "1バイト超過で打ち切る");
    }
}
