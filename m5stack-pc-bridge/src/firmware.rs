//! M5Stack向けfirmware配信(`GET /firmware`, `GET /firmware/manifest`)の本体。
//! bridgeは「配布場所」であって「信頼の根」ではないため、manifestへ署名を付ける
//! (Issue #41)。署名の正本は `pc-remote-signing`。**ここでcanonical文字列の順序を組み立てない。**

use std::path::PathBuf;

use pc_remote_signing::OtaManifest;
use time::format_description::well_known::Rfc3339;

/// `firmware.version` が無いときにmanifestの `version` へ入れるフォールバック値
/// (`firmware.bin` だけの配置でも配信を壊さないため)。表示用。
pub const UNKNOWN_VERSION: &str = "unknown";

/// 配信ファイルの配置。既定は実行ファイルと同じディレクトリ。
/// `version` はバイナリ内に版を持たないため、運用者が置く `firmware.version`
/// (1行テキスト)を別に読む。無い・空なら [`UNKNOWN_VERSION`]。sha256は実バイナリから計算する。
#[derive(Clone, Debug)]
pub struct FirmwarePaths {
    pub bin: PathBuf,
    pub version: PathBuf,
}

impl FirmwarePaths {
    pub fn from_exe_dir() -> Self {
        Self {
            bin: crate::exe_dir_file("firmware.bin"),
            version: crate::exe_dir_file("firmware.version"),
        }
    }
}

/// `GET /firmware` 用にバイナリだけを読む(同期I/Oなので呼び出し側でblockingスレッドへ逃がす)。
/// sha256・version・mtimeはmanifestのための値であり、バイト列の配信には要らないため取らない。
/// `firmware.bin` が無いときは `ErrorKind::NotFound` を返す(呼び出し側は404へ写像)。
/// 応答本文・エラーメッセージにファイルパスは含めない。
pub fn read_bin(paths: &FirmwarePaths) -> std::io::Result<Vec<u8>> {
    std::fs::read(&paths.bin)
}

/// manifestの組み立てに失敗した原因。
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// 配信ファイルの読み込みに失敗した。`firmware.bin` が無いときは
    /// `ErrorKind::NotFound`(呼び出し側は404へ写像)。
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// `created_at` のRFC3339整形に失敗した。
    #[error(transparent)]
    Format(#[from] time::error::Format),
}

/// `GET /firmware/manifest` 用にmanifestを組み立ててHMAC-SHA256署名を付ける
/// (同期I/Oなので呼び出し側でblockingスレッドへ逃がす)。`created_at` は
/// `firmware.bin` のmtime(UTC)のRFC3339文字列。
/// canonical文字列の組み立ては `pc-remote-signing::manifest_canonical_string` に任せ、
/// ここで自前の順序を発明しない(検証側とずれるため)。
/// 応答本文・エラーメッセージにファイルパスは含めない。
pub fn build_manifest(paths: &FirmwarePaths, secret: &[u8]) -> Result<OtaManifest, ManifestError> {
    let bytes = read_bin(paths)?;
    let size = bytes.len() as u64;
    let sha256 = pc_remote_signing::body_sha256_hex(&bytes);
    let version = read_version(&paths.version);
    let modified = std::fs::metadata(&paths.bin)?.modified()?;
    let created_at = time::OffsetDateTime::from(modified).format(&Rfc3339)?;
    let signature = pc_remote_signing::sign_manifest(secret, &version, size, &sha256, &created_at);
    Ok(OtaManifest {
        version,
        size,
        sha256,
        created_at,
        signature,
    })
}

/// `firmware.version` の1行目を使う。無い・空・読めない場合は [`UNKNOWN_VERSION`]
/// (配置ミスで配信全体を500にしない。同一性はsha256で担保)。
fn read_version(path: &std::path::Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.lines().next().map(|line| line.trim().to_owned()))
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| UNKNOWN_VERSION.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{build_manifest, read_bin, FirmwarePaths, ManifestError, UNKNOWN_VERSION};
    use std::io::Write;

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

    fn write_bin(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) {
        let mut file = std::fs::File::create(dir.path().join(name)).unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
    }

    fn paths(dir: &tempfile::TempDir) -> FirmwarePaths {
        FirmwarePaths {
            bin: dir.path().join("firmware.bin"),
            version: dir.path().join("firmware.version"),
        }
    }

    #[test]
    fn read_bin_returns_only_bytes() {
        let dir = tempfile::tempdir().unwrap();
        write_bin(&dir, "firmware.bin", b"fake-firmware-image");
        write_bin(&dir, "firmware.version", b"  0.2.0\n");
        assert_eq!(read_bin(&paths(&dir)).unwrap(), b"fake-firmware-image");
    }

    #[test]
    fn manifest_has_size_hash_and_version() {
        let dir = tempfile::tempdir().unwrap();
        write_bin(&dir, "firmware.bin", b"fake-firmware-image");
        write_bin(&dir, "firmware.version", b"  0.2.0\n");
        let manifest = build_manifest(&paths(&dir), SECRET).unwrap();
        assert_eq!(manifest.version, "0.2.0");
        assert_eq!(manifest.size, 19);
        assert_eq!(
            manifest.sha256,
            pc_remote_signing::body_sha256_hex(b"fake-firmware-image")
        );
        assert!(manifest.created_at.contains('T'));
    }

    #[test]
    fn falls_back_to_unknown_version_without_version_file() {
        let dir = tempfile::tempdir().unwrap();
        write_bin(&dir, "firmware.bin", b"fake-firmware-image");
        let manifest = build_manifest(&paths(&dir), SECRET).unwrap();
        assert_eq!(manifest.version, UNKNOWN_VERSION);
    }

    #[test]
    fn missing_bin_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_bin(&paths(&dir)).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        let err = build_manifest(&paths(&dir), SECRET).unwrap_err();
        assert!(matches!(err, ManifestError::Io(e) if e.kind() == std::io::ErrorKind::NotFound));
    }

    #[test]
    fn manifest_signature_covers_all_fields() {
        let dir = tempfile::tempdir().unwrap();
        write_bin(&dir, "firmware.bin", b"fake-firmware-image");
        write_bin(&dir, "firmware.version", b"0.2.0");
        let manifest = build_manifest(&paths(&dir), SECRET).unwrap();
        assert!(pc_remote_signing::verify_manifest_signature(
            SECRET,
            &manifest.version,
            manifest.size,
            &manifest.sha256,
            &manifest.created_at,
            &manifest.signature,
        ));
    }
}
