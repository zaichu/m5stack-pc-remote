use std::io;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// PC死活確認のTCP接続タイムアウト。長くすると画面更新とTelegram応答が遅くなる。
pub const STATUS_PROBE_TIMEOUT: Duration = Duration::from_millis(800);

/// STATUS相当の疎通確認。接続成功または即時refusedならオン、timeoutならオフ/到達不能。
/// UIループでは使わないこと。ホスト名の名前解決が毎回走り画面もタッチも止まる
/// ため、UI側は起動時に1回だけ解決した `SocketAddr` を保持し、
/// `check_pc_online_addr` を使う(Issue #248)。Telegramスレッド側の互換経路として残す。
pub fn check_pc_online(addr_text: &str, timeout: Duration) -> bool {
    // IPリテラルならDNSを引かない。ホスト名の名前解決はブロックするため、
    // UIループからは呼ばないこと(関数の説明を参照。Issue #130-3)。
    // `to_socket_addrs()` は制限前にNVSへ書かれたホスト名が残っている場合の互換。
    if let Ok(addr) = addr_text.parse::<SocketAddr>() {
        return probe(addr, timeout);
    }

    let Ok(addrs) = addr_text.to_socket_addrs() else {
        return false;
    };
    // A/AAAA両方を持つホスト名では先頭1件だけだと取りこぼして誤判定するため順に試す。
    for addr in addrs {
        if probe(addr, timeout) {
            return true;
        }
    }
    false
}

/// 起動時に1回だけ呼ぶSTATUS確認先の解決。IPリテラルはDNSを引かない。
/// 解決結果の保持と再解決の判断は呼び出し側が行い、UIループ内ではDNSを引かない。
/// 複数アドレスは順序どおり返し、先頭1件への切り捨てはしない。
pub fn resolve_status_addr_once(addr_text: &str) -> Vec<SocketAddr> {
    config_validation::resolve_status_addr_with(addr_text, |trimmed| {
        trimmed
            .to_socket_addrs()
            .map_or_default(std::iter::Iterator::collect)
    })
}

/// 解決済みの確認先への疎通確認。DNSを引かないためUIループから呼べる。
/// 複数アドレスは順に試し、いずれかが成功すればオンにする(従来の
/// `check_pc_online` と同じフォールバック)。
pub fn check_pc_online_cached(addrs: &[SocketAddr], timeout: Duration) -> bool {
    addrs.iter().any(|addr| probe(*addr, timeout))
}

/// `ConnectionRefused` は「相手は居るがそのportで待っていない」ためオンとみなす。
fn probe(addr: SocketAddr, timeout: Duration) -> bool {
    match TcpStream::connect_timeout(&addr, timeout) {
        Ok(_) => true,
        Err(e) => e.kind() == io::ErrorKind::ConnectionRefused,
    }
}
