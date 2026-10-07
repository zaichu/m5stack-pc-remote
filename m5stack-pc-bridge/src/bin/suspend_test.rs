//! #217 事前確認用の診断バイナリ。Windows Service相当の環境(session 0 / SYSTEM)から
//! `SetSuspendState` を呼んだときに「S3スリープになるのか・休止になるのか・失敗するのか」を
//! 実機で確かめるためのもの。本実装のロジックは `power.rs` 側へ取り込む。

#[cfg(not(windows))]
fn main() {
    eprintln!("suspend_test は Windows 専用の診断バイナリです。");
}

#[cfg(windows)]
fn main() {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Power::SetSuspendState;

    let log_path = std::env::temp_dir().join("suspend-test.log");
    let log = |msg: &str| log_line(&log_path, msg);

    log(&format!("log file: {}", log_path.display()));
    // SESSIONNAME は session 0(service)と session 1(対話的)の判別に使う。
    log(&format!(
        "context: USERNAME={:?} SESSIONNAME={:?}",
        std::env::var("USERNAME").unwrap_or_else(|_| "<unset>".to_string()),
        std::env::var("SESSIONNAME").unwrap_or_else(|_| "<unset>".to_string())
    ));

    // 権限付与に失敗しても呼び出しは続けて、失敗の出方そのものを観測対象にする。
    match enable_shutdown_privilege() {
        Ok(()) => log("SeShutdownPrivilege: enabled"),
        Err(e) => log(&format!("SeShutdownPrivilege: FAILED ({e})")),
    }

    // session 0 ではコンソールが無いため、呼ぶ前に必ずstdoutとログの両方へ書いてflushする。
    log("calling SetSuspendState(hibernate=false, force=false, disable_wake_event=false)");
    let ok = unsafe { SetSuspendState(false, false, false) };
    let err = unsafe { GetLastError() };
    if ok {
        // TRUE ならスリープへ入るので、この行は復帰後にしか出ない。
        log(&format!(
            "SetSuspendState returned TRUE / GetLastError={err}"
        ));
    } else {
        log(&format!(
            "SetSuspendState returned FALSE / GetLastError={err}"
        ));
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

/// `SetSuspendState` の呼び出し前に必要な `SeShutdownPrivilege` を有効化する。
#[cfg(windows)]
fn enable_shutdown_privilege() -> Result<(), String> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
    use windows_sys::Win32::Security::{TOKEN_ADJUST_PRIVILEGES, TOKEN_QUERY};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token: HANDLE = std::ptr::null_mut();
    let opened = unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
    };
    // 失敗理由を正しく取るため、判定より前にGetLastErrorを退避する。
    let open_err = unsafe { GetLastError() };
    if opened == 0 {
        return Err(format!("OpenProcessToken failed: GetLastError={open_err}"));
    }

    let result = grant_enable(token);
    unsafe { CloseHandle(token) };
    result
}

#[cfg(windows)]
fn grant_enable(token: windows_sys::Win32::Foundation::HANDLE) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{GetLastError, LUID};
    use windows_sys::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
        SE_SHUTDOWN_NAME, TOKEN_PRIVILEGES,
    };

    let mut luid = LUID {
        LowPart: 0,
        HighPart: 0,
    };
    let found = unsafe { LookupPrivilegeValueW(std::ptr::null(), SE_SHUTDOWN_NAME, &mut luid) };
    let luid_err = unsafe { GetLastError() };
    if found == 0 {
        return Err(format!(
            "LookupPrivilegeValueW(SeShutdownPrivilege) failed: GetLastError={luid_err}"
        ));
    }

    let privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    let adjusted = unsafe {
        AdjustTokenPrivileges(
            token,
            0,
            &privileges,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    // AdjustTokenPrivileges は全権限を割り当てられなくてもTRUEを返し、
    // 最終エラーにERROR_NOT_ALL_ASSIGNED(1300)を残すため両方を見る。
    let adjust_err = unsafe { GetLastError() };
    if adjusted == 0 {
        return Err(format!(
            "AdjustTokenPrivileges failed: GetLastError={adjust_err}"
        ));
    }
    if adjust_err != 0 {
        return Err(format!(
            "AdjustTokenPrivileges: GetLastError={adjust_err} (権限が有効化されていない可能性)"
        ));
    }
    Ok(())
}
