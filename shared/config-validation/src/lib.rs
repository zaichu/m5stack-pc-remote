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
pub fn validate_status_addr(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    let Some((host, port)) = trimmed.rsplit_once(':') else {
        return Err(format!(
            "`{trimmed}` は host:port 形式で指定してください(例: 192.168.1.50:80)"
        ));
    };
    if host.is_empty() || host.chars().any(char::is_whitespace) {
        return Err(format!("`{trimmed}` のhost部分が不正です"));
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
        // ホスト名も許容する(check_pc_onlineがto_socket_addrsでDNS解決するため)。
        assert_eq!(
            validate_status_addr("my-pc.local:8080").unwrap(),
            "my-pc.local:8080"
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
}
