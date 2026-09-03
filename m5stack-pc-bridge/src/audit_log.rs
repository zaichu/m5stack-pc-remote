use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// audit.logがこのサイズ以上に育っていたら、書き込み前に1世代だけローテーションする。
const AUDIT_LOG_MAX_BYTES: u64 = 1_000_000;
static AUDIT_LOG_LOCK: Mutex<()> = Mutex::new(());

/// 監査ログの既定パス。実行ファイルと同じディレクトリの`audit.log`。
///
/// Windows ServiceのCWDは`%SystemRoot%\System32`になるため、CWD相対ではなく実行
/// ファイルの場所を基準にする。config.tomlでのpath上書きは提供しない。
pub fn path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("audit.log")))
        .unwrap_or_else(|| PathBuf::from("audit.log"))
}

/// 認証成功かつ`confirm=true`のREBOOT/SHUTDOWNだけを1行追記する。
///
/// 書く項目は timestamp / action / dry_run / result のみ。shared_secret、signature、
/// nonce、request body、Telegram tokenは書かない。
pub fn append(action: &str, dry_run: bool, result: &str) -> std::io::Result<()> {
    let _guard = AUDIT_LOG_LOCK
        .lock()
        .map_err(|_| std::io::Error::other("audit log lock poisoned"))?;
    let path = path();
    rotate_if_too_large(&path)?;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;

    let timestamp = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".to_string());

    writeln!(
        file,
        "{timestamp} action={action} dry_run={dry_run} result={result}"
    )?;
    file.sync_all()
}

fn rotate_if_too_large(path: &Path) -> std::io::Result<()> {
    let len = match std::fs::metadata(path) {
        Ok(meta) => meta.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if len < AUDIT_LOG_MAX_BYTES {
        return Ok(());
    }

    let rotated = path.with_extension("log.1");
    let _ = std::fs::remove_file(&rotated);
    std::fs::rename(path, rotated)
}
