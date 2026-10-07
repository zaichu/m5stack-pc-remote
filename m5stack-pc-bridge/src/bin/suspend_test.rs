//! #217 事前確認用の診断バイナリ。Windows Service相当の環境(session 0 / SYSTEM)から
//! スリープ実行(`m5stack_pc_bridge::suspend::suspend`)を呼んだときの振る舞いを
//! 実機で確かめるためのもの。実行ロジックの正本はライブラリ側(`suspend.rs`)にあり、
//! ここは呼び出しと結果の記録だけを行う(重複コードを残さない)。

#[cfg(not(windows))]
fn main() {
    eprintln!("suspend_test は Windows 専用の診断バイナリです。");
}

#[cfg(windows)]
fn main() {
    let log_path = std::env::temp_dir().join("suspend-test.log");
    let log = |msg: &str| log_line(&log_path, msg);

    log(&format!("log file: {}", log_path.display()));
    // SESSIONNAME は session 0(service)と session 1(対話的)の判別に使う。
    log(&format!(
        "context: USERNAME={:?} SESSIONNAME={:?}",
        std::env::var("USERNAME").unwrap_or_else(|_| "<unset>".to_string()),
        std::env::var("SESSIONNAME").unwrap_or_else(|_| "<unset>".to_string())
    ));

    // session 0 ではコンソールが無いため、呼ぶ前に必ずstdoutとログの両方へ書いてflushする。
    log(
        "calling suspend (SetSuspendState(hibernate=false, force=false, disable_wake_event=false))",
    );
    match m5stack_pc_bridge::suspend::suspend() {
        Ok(()) => {
            // 成功時はスリープへ入るので、この行は復帰後にしか出ない。
            log("suspend returned Ok (resumed after sleep)");
        }
        Err(e) => {
            log(&format!("suspend failed: {e}"));
        }
    }
}

/// session 0ではコンソールが無いため、stdoutへ出すと同じ内容を
/// `temp_dir()/suspend-test.log` へ追記する。起動直後にログパスをstdoutへ出している。
#[cfg(windows)]
fn log_line(log_path: &std::path::Path, msg: &str) {
    use std::io::Write;

    println!("{msg}");
    let _ = std::io::stdout().flush();

    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(file, "[{now}] {msg}");
    }
}
