//! M5Stack向けfirmware配信(`GET /firmware`, `GET /firmware/manifest`)の本体。
//!
//! Issue #41の設計判断: bridgeは「配布場所」であって「信頼の根」ではない。
//! manifestへHMAC-SHA256署名を付け、M5Stack側(Phase 3のOTAクライアント)が
//! 公開値(version/size/sha256)だけを信じずに検証できるようにする。
//! 署名のcanonical文字列・鍵・方式の正本は `pc-remote-signing` の
//! `manifest_canonical_string` / `sign_manifest` に置き、ここでは順序を
//! 組み立てない(両側で同一でなければ壊れるものは共有crateへ置く方針)。

use std::path::PathBuf;

use serde::Serialize;
use time::format_description::well_known::Rfc3339;

/// `firmware.version` が無いときにmanifestの `version` へ入れる値。
///
/// `firmware.bin` だけの配置でも配信を壊さないためのフォールバック。
/// 運用では `firmware.version` を一緒に置いて人間可読な版を指定する。
/// Phase 3で版比較の意味が決まるまでは、この値は表示用に留める。
pub const UNKNOWN_VERSION: &str = "unknown";

/// 配信ファイルの配置。既定は実行ファイルと同じディレクトリ。
///
/// `version` は `firmware.bin` だけでは決まらない(バイナリ内に版を持たない
/// ため)ので、運用者が配置する `firmware.version`(1行のテキスト)を別に読む。
/// 無い・空の場合は [`UNKNOWN_VERSION`] になる。sha256は常に実バイナリから
/// 計算するため、内容の同一性はversionの有無に依存しない。
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

/// ディスクから読んだ配信対象。`created_at` は `firmware.bin` のmtime(UTC)。
#[derive(Clone, Debug)]
pub struct FirmwareImage {
    pub bytes: Vec<u8>,
    pub version: String,
    pub sha256_hex: String,
    pub created_at: time::OffsetDateTime,
}

impl FirmwareImage {
    pub fn size(&self) -> u64 {
        self.bytes.len() as u64
    }
}

/// `GET /firmware/manifest` の応答。本文はフィールド宣言順で固定される。
#[derive(Debug, Serialize)]
pub struct FirmwareManifest {
    pub version: String,
    pub size: u64,
    pub sha256: String,
    pub created_at: String,
    pub signature: String,
}

/// 配信ファイルを読み込む。同期I/Oなので呼び出し側でblockingスレッドへ逃がす。
///
/// `firmware.bin` が無いときは `ErrorKind::NotFound` の `io::Error` を返す。
/// 呼び出し側はこれを404に写像し、他の読み込み失敗は500に写像する。
/// 応答本文・エラーメッセージにファイルパスは含めない。
pub fn load(paths: &FirmwarePaths) -> std::io::Result<FirmwareImage> {
    let bytes = std::fs::read(&paths.bin)?;
    let modified = std::fs::metadata(&paths.bin)?.modified()?;
    Ok(FirmwareImage {
        sha256_hex: pc_remote_signing::body_sha256_hex(&bytes),
        bytes,
        version: read_version(&paths.version),
        created_at: modified.into(),
    })
}

/// manifestを組み立て、HMAC-SHA256署名を付ける。
///
/// 署名対象は「version・size・sha256・created_at を含む、順序が一意に決まる
/// 文字列」で、組み立ては `pc-remote-signing::manifest_canonical_string` が
/// 行う(`"FIRMWARE-MANIFEST-v1\n{version}\n{size}\n{sha256}\n{created_at}"`)。
/// ここで自前の結合順序を発明しないこと(Phase 3の検証側とずれるため)。
pub fn build_manifest(
    image: &FirmwareImage,
    secret: &[u8],
) -> Result<FirmwareManifest, time::error::Format> {
    let created_at = image.created_at.format(&Rfc3339)?;
    let signature = pc_remote_signing::sign_manifest(
        secret,
        &image.version,
        image.size(),
        &image.sha256_hex,
        &created_at,
    );
    Ok(FirmwareManifest {
        version: image.version.clone(),
        size: image.size(),
        sha256: image.sha256_hex.clone(),
        created_at,
        signature,
    })
}

/// `firmware.version` の1行目を使う。無い・空・読めない場合は
/// [`UNKNOWN_VERSION`]。運用者の配置ミスで配信全体を500にしないための
/// 判断(内容の同一性はsha256で担保される)。
fn read_version(path: &std::path::Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.lines().next().map(|line| line.trim().to_owned()))
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| UNKNOWN_VERSION.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{build_manifest, load, FirmwarePaths, UNKNOWN_VERSION};
    use std::io::Write;

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
    fn loads_size_hash_and_version() {
        let dir = tempfile::tempdir().unwrap();
        write_bin(&dir, "firmware.bin", b"fake-firmware-image");
        write_bin(&dir, "firmware.version", b"  0.2.0\n");
        let image = load(&paths(&dir)).unwrap();
        assert_eq!(image.bytes, b"fake-firmware-image");
        assert_eq!(image.size(), 19);
        assert_eq!(
            image.sha256_hex,
            pc_remote_signing::body_sha256_hex(b"fake-firmware-image")
        );
        assert_eq!(image.version, "0.2.0");
    }

    #[test]
    fn falls_back_to_unknown_version_without_version_file() {
        let dir = tempfile::tempdir().unwrap();
        write_bin(&dir, "firmware.bin", b"fake-firmware-image");
        let image = load(&paths(&dir)).unwrap();
        assert_eq!(image.version, UNKNOWN_VERSION);
    }

    #[test]
    fn missing_bin_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = load(&paths(&dir)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn manifest_signature_covers_all_fields() {
        let dir = tempfile::tempdir().unwrap();
        write_bin(&dir, "firmware.bin", b"fake-firmware-image");
        write_bin(&dir, "firmware.version", b"0.2.0");
        let image = load(&paths(&dir)).unwrap();
        let secret = b"0123456789abcdef0123456789abcdef";
        let manifest = build_manifest(&image, secret).unwrap();
        assert_eq!(manifest.version, "0.2.0");
        assert_eq!(manifest.size, image.size());
        assert_eq!(manifest.sha256, image.sha256_hex);
        assert!(manifest.created_at.contains('T'));
        assert!(pc_remote_signing::verify_manifest_signature(
            secret,
            &manifest.version,
            manifest.size,
            &manifest.sha256,
            &manifest.created_at,
            &manifest.signature,
        ));
    }
}
