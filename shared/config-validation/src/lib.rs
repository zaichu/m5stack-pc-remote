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

/// STATUS確認(オンライン判定のTCP probe)の接続先を組み立てる。
///
/// 接続先は常に `{pc_ip_address}:{status_port}` を**読み出し時に組み立てる**。
/// hostを別の設定値として持たない理由: 同じ情報(IP)を2箇所に持つと、PCのIPが
/// 変わったときに2項目の直しが必要になり、片方の直し忘れで電源操作は通るのに
/// STATUSが常にOFFLINEになる(またはその逆)。hostの正本は `pc_ip_address` の
/// 1箇所だけにし、ここでは連結だけを行う。
/// DNS解決はしない。IPv4リテラルを連結するだけで、`check_pc_online` が
/// IPリテラルなら `SocketAddr` として直接parseする高速経路に乗る
/// (Issue #130-3の方針を維持)。
pub fn compose_status_addr(pc_ip_address: &str, status_port: u16) -> String {
    format!("{}:{status_port}", pc_ip_address.trim())
}

/// STATUS確認先portの既定値。`firmware/build.rs` の
/// `Key::int("pc_status_port", ...).default(80)` と同じ値にすること。
pub const DEFAULT_STATUS_PORT: u16 = 80;

/// STATUS確認先portを正規化する。0なら既定値、それ以外はそのまま返す。
pub fn normalize_status_port(port: u16) -> u16 {
    if port == 0 {
        DEFAULT_STATUS_PORT
    } else {
        port
    }
}

/// `wol_port` として妥当なport番号か検証する。
pub fn validate_wol_port(input: &str) -> Result<u16, String> {
    validate_port(input.trim())
}

/// 画面の明るさとして妥当なパーセント(0〜100)か検証する(Issue #167)。
///
/// 0%を「消灯」の意味にはしない(消灯は別Issue #168のスリープ機能の役割)。
/// ここでは0〜100の範囲だけを見て、下限電圧への丸めは
/// `brightness_percent_to_dcdc3_mv` が担当する。
pub fn validate_brightness_percent(input: &str) -> Result<u8, String> {
    let trimmed = input.trim();
    let percent: u8 = trimmed
        .parse()
        .map_err(|_| format!("`{trimmed}` は明るさ(0〜100の数値)ではありません(例: 80)"))?;
    if percent > 100 {
        return Err(format!(
            "`{trimmed}` は範囲外です。明るさは0〜100で指定してください"
        ));
    }
    Ok(percent)
}

/// 明るさ100%に対応するDCDC3電圧(mV)。従来の固定値であり実機で動作確認済み。
pub const BRIGHTNESS_DCDC3_MAX_MV: u16 = 2800;
/// 明るさ0%に対応するDCDC3電圧(mV)。
///
/// 実機で未検証の暫定値であり、後日実機で見ながら追い込む前提(Issue #167)。
/// AXP192自体のDCDC3範囲は700〜3500mV/25mVステップだが、それはICの仕様上の
/// 範囲であり、Core2のバックライトLEDが実際に安全かつ点灯する範囲ではない
/// ため、ユーザーに公開する範囲は保守的に絞る。0%でも「暗いが点いている」
/// 状態に留め、消灯にはしない(消灯は別Issue #168のスリープ機能の役割)。
/// LDO2(LCD+タッチ電源、3300mV固定)は絶対に変更しないこと。
pub const BRIGHTNESS_DCDC3_MIN_MV: u16 = 2500;

/// 明るさパーセント(0〜100)をAXP192 DCDC3電圧(mV)へ線形に変換する純粋関数。
///
/// `BRIGHTNESS_DCDC3_MIN_MV`〜`BRIGHTNESS_DCDC3_MAX_MV` へ線形に割り付け、
/// AXP192の25mVステップへ切り捨てる(`axp192` crateの `set_dcdc3_voltage` も
/// 同じ切り捨てを行うため、ここで丸めて実電圧と一致させる)。
/// 101以上が来ても `validate_brightness_percent` を素通りした不正値として
/// 上限へ丸める(消灯側へは倒さない)。
pub fn brightness_percent_to_dcdc3_mv(percent: u8) -> u16 {
    let clamped = percent.min(100);
    let range = BRIGHTNESS_DCDC3_MAX_MV - BRIGHTNESS_DCDC3_MIN_MV;
    let mv = BRIGHTNESS_DCDC3_MIN_MV + range * clamped as u16 / 100;
    mv - (mv % 25)
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
    fn composes_status_addr() {
        assert_eq!(compose_status_addr("192.168.1.50", 80), "192.168.1.50:80");
        assert_eq!(compose_status_addr("192.168.1.50", 8080), "192.168.1.50:8080");
        assert_eq!(compose_status_addr("10.0.0.1", 65535), "10.0.0.1:65535");
        // 前後の空白はtrimする(Telegramのコピペ経由の値を想定)。
        assert_eq!(compose_status_addr("  192.168.1.50 \n", 80), "192.168.1.50:80");
    }

    #[test]
    fn composed_status_addr_parses_as_socket_addr() {
        // Issue #130-3の方針: IPリテラル連結のため、`check_pc_online` の
        // `SocketAddr` 高速経路(DNSを引かない)に乗ること。
        use std::net::SocketAddr;
        for (ip, port) in [("192.168.1.50", 80), ("10.0.0.1", 8080), ("172.16.0.2", 65535)] {
            let addr = compose_status_addr(ip, port);
            assert!(
                addr.parse::<SocketAddr>().is_ok(),
                "{addr} は SocketAddr としてparseできること"
            );
        }
    }

    #[test]
    fn normalizes_status_port() {
        assert_eq!(DEFAULT_STATUS_PORT, 80);
        assert_eq!(normalize_status_port(0), 80);
        assert_eq!(normalize_status_port(80), 80);
        assert_eq!(normalize_status_port(8080), 8080);
        assert_eq!(normalize_status_port(65535), 65535);
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
    fn accepts_valid_brightness_percent() {
        assert_eq!(validate_brightness_percent("0").unwrap(), 0);
        assert_eq!(validate_brightness_percent("80").unwrap(), 80);
        assert_eq!(validate_brightness_percent("100").unwrap(), 100);
        // 前後の空白は許容してtrimする(Telegramのコピペ経由の値を想定)。
        assert_eq!(validate_brightness_percent("  80 \n").unwrap(), 80);
    }

    #[test]
    fn rejects_invalid_brightness_percent() {
        // Issue #167: 境界値は 0/100 が有効、101 が無効。
        assert!(validate_brightness_percent("101").is_err(), "101は範囲外");
        assert!(validate_brightness_percent("255").is_err(), "u8範囲内だが範囲外");
        assert!(validate_brightness_percent("256").is_err(), "u8範囲外");
        assert!(validate_brightness_percent("-1").is_err(), "負数");
        assert!(validate_brightness_percent("abc").is_err(), "数値でない");
        assert!(validate_brightness_percent("").is_err(), "空文字");
        assert!(validate_brightness_percent("80%").is_err(), "単位付きは拒否");
        assert!(validate_brightness_percent("8.5").is_err(), "小数は拒否");
    }

    #[test]
    fn maps_brightness_percent_endpoints_to_dcdc3_mv() {
        // Issue #167: 0%は下限(消灯ではない)、100%は現状の2800mV。
        assert_eq!(brightness_percent_to_dcdc3_mv(0), BRIGHTNESS_DCDC3_MIN_MV);
        assert_eq!(brightness_percent_to_dcdc3_mv(100), BRIGHTNESS_DCDC3_MAX_MV);
        assert_eq!(BRIGHTNESS_DCDC3_MAX_MV, 2800);
    }

    #[test]
    fn maps_brightness_percent_monotonically_in_25mv_steps() {
        // 全域で単調非減少・範囲内・25mVステップであること。
        // 境界の切り捨て(`>` と `>=` の取り違え等)を殺すため全点を検証する。
        let mut prev = brightness_percent_to_dcdc3_mv(0);
        for percent in 0..=100u8 {
            let mv = brightness_percent_to_dcdc3_mv(percent);
            assert!(
                (BRIGHTNESS_DCDC3_MIN_MV..=BRIGHTNESS_DCDC3_MAX_MV).contains(&mv),
                "{percent}% -> {mv}mV は範囲外"
            );
            assert_eq!(mv % 25, 0, "{percent}% -> {mv}mV は25mVステップでない");
            assert!(mv >= prev, "{percent}% で減少した");
            prev = mv;
        }
        // 中点の代表値。線形補間の向き(上限・下限の取り違え)を殺す。
        assert_eq!(brightness_percent_to_dcdc3_mv(50), 2650);
    }

    #[test]
    fn clamps_out_of_range_brightness_to_max() {
        // `validate_brightness_percent` を素通りした不正値は上限へ丸める。
        // 下限(消灯側)へ倒さないこと。
        assert_eq!(brightness_percent_to_dcdc3_mv(101), BRIGHTNESS_DCDC3_MAX_MV);
        assert_eq!(brightness_percent_to_dcdc3_mv(u8::MAX), BRIGHTNESS_DCDC3_MAX_MV);
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
