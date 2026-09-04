use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// audit.logがこのサイズ以上に育っていたら、書き込み前に1世代だけローテーションする。
const AUDIT_LOG_MAX_BYTES: u64 = 1_000_000;
static AUDIT_LOG_LOCK: Mutex<()> = Mutex::new(());

/// 監査ログの既定パス。実行ファイルと同じディレクトリの`audit.log`。
/// config.tomlでのpath上書きは提供しない。
pub fn path() -> PathBuf {
    crate::exe_dir_file("audit.log")
}

/// 認証成功かつ`confirm=true`のREBOOT/SHUTDOWNだけを1行追記する。
///
/// 書く項目は timestamp / action / dry_run / result のみ。shared_secret、signature、
/// nonce、request body、Telegram tokenは書かない。
pub fn append(action: &str, dry_run: bool, result: &str) -> std::io::Result<()> {
    // poisonしても排他は維持したまま処理を続ける。この排他が守るのはファイルへの
    // 追記1回分だけで、panicが残せる最悪の状態は行の途中までの書き込みにとどまる。
    // 一方でここでエラーを返すと fail-closed により電源操作が以後ずっと通らなくなる。
    let _guard = AUDIT_LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_uses_audit_log_file_name() {
        assert_eq!(
            path().file_name().and_then(|name| name.to_str()),
            Some("audit.log")
        );
    }

    // 追記内容(1行・action/dry_run/resultの項目)を固定する。項目を削ると
    // このテストが落ちる。並行するserver_testsのfail-closedテストが一瞬だけ
    // audit.logの場所を塞ぐことがあるため、失敗時は少し待って再試行する。
    #[test]
    fn append_writes_single_line_with_expected_fields() {
        let marker = format!("selftest-{}", std::process::id());
        let audit_path = path();
        let before = std::fs::read_to_string(&audit_path).unwrap_or_default();

        let mut last_err = None;
        for _ in 0..50 {
            match append("selftest", true, &marker) {
                Ok(()) => {
                    last_err = None;
                    break;
                }
                Err(err) => {
                    last_err = Some(err.to_string());
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
        if let Some(err) = last_err {
            panic!("append kept failing: {err}");
        }

        let after = std::fs::read_to_string(&audit_path).unwrap();
        let appended = after.get(before.len()..).unwrap_or(&after);
        let expected = format!("action=selftest dry_run=true result={marker}");
        assert!(
            appended.lines().any(|line| line.contains(&expected)),
            "audit log gained no selftest line: {appended:?}"
        );
    }

    fn temp_case_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "m5stack-bridge-audit-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_sized_file(path: &Path, len: usize) {
        std::fs::write(path, vec![b'x'; len]).unwrap();
    }

    #[test]
    fn does_not_rotate_small_log() {
        let dir = temp_case_dir("small");
        let target = dir.join("audit.log");
        write_sized_file(&target, 100);

        rotate_if_too_large(&target).unwrap();

        assert!(target.is_file());
        assert!(!target.with_extension("log.1").exists());
        assert_eq!(std::fs::metadata(&target).unwrap().len(), 100);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 上限ちょうどでローテーションする。`len < MAX` を `len <= MAX` に
    // 緩めると上限ちょうどのファイルが残ってしまい、このテストが落ちる。
    #[test]
    fn rotates_log_at_max_size() {
        let dir = temp_case_dir("at-max");
        let target = dir.join("audit.log");
        write_sized_file(&target, AUDIT_LOG_MAX_BYTES as usize);

        rotate_if_too_large(&target).unwrap();

        assert!(!target.exists());
        let rotated = target.with_extension("log.1");
        assert_eq!(
            std::fs::metadata(&rotated).unwrap().len(),
            AUDIT_LOG_MAX_BYTES
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_replaces_stale_backup() {
        let dir = temp_case_dir("replace");
        let target = dir.join("audit.log");
        write_sized_file(&target, AUDIT_LOG_MAX_BYTES as usize + 1);
        let rotated = target.with_extension("log.1");
        std::fs::write(&rotated, b"stale").unwrap();

        rotate_if_too_large(&target).unwrap();

        assert_eq!(
            std::fs::metadata(&rotated).unwrap().len(),
            AUDIT_LOG_MAX_BYTES + 1
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ローテーションの失敗は握り潰さず返す。退避先をディレクトリで塞ぐと
    // renameが失敗する。`?` を `let _ =` に変えるとこのテストが落ちる。
    #[test]
    fn rotation_failure_is_returned_not_swallowed() {
        let dir = temp_case_dir("failure");
        let target = dir.join("audit.log");
        write_sized_file(&target, AUDIT_LOG_MAX_BYTES as usize + 1);
        std::fs::create_dir(target.with_extension("log.1")).unwrap();

        let err = rotate_if_too_large(&target).unwrap_err();

        assert!(!err.to_string().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
