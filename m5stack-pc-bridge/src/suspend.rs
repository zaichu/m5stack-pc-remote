//! PCのスリープ実行(Issue #217)。`SetSuspendState(false,false,false)` で
//! S3スリープへ入る。休止が有効な環境でも同APIでスリープになることは
//! 実機確認済み(2026-10-07、`suspend_test` 診断バイナリで検証)。
//!
//! テストから本物の `SetSuspendState` を呼ばないこと。`power.rs` は
//! executor注入でテストし、このモジュールの実呼び出しは差し替える。

/// スリープへ移行する。成功時はPCがスリープするため戻らないことがある。
#[cfg(windows)]
pub fn suspend() -> anyhow::Result<()> {
    enable_shutdown_privilege()?;
    // hibernate=false, force=false, disable_wake_event=false。
    // 休止ではなくスリープになることは実機確認済み。
    let ok = unsafe { windows_sys::Win32::System::Power::SetSuspendState(false, false, false) };
    if !ok {
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        anyhow::bail!("SetSuspendState failed: GetLastError={err}");
    }
    Ok(())
}

/// 開発ホスト(Linux/CI)では `shutdown.exe` が無いのと同じく失敗扱いにする。
/// 呼び出し側(`power.rs`)の既存のexecutor失敗の仕組みに乗せる。
#[cfg(not(windows))]
pub fn suspend() -> anyhow::Result<()> {
    anyhow::bail!("sleep is only supported on Windows")
}

/// `SetSuspendState` の呼び出し前に必要な `SeShutdownPrivilege` を有効化する。
#[cfg(windows)]
fn enable_shutdown_privilege() -> anyhow::Result<()> {
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
        anyhow::bail!("OpenProcessToken failed: GetLastError={open_err}");
    }

    let result = grant_enable(token);
    unsafe { CloseHandle(token) };
    result
}

#[cfg(windows)]
fn grant_enable(token: windows_sys::Win32::Foundation::HANDLE) -> anyhow::Result<()> {
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
        anyhow::bail!("LookupPrivilegeValueW(SeShutdownPrivilege) failed: GetLastError={luid_err}");
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
        anyhow::bail!("AdjustTokenPrivileges failed: GetLastError={adjust_err}");
    }
    if adjust_err != 0 {
        anyhow::bail!(
            "AdjustTokenPrivileges: GetLastError={adjust_err} (権限が有効化されていない可能性)"
        );
    }
    Ok(())
}
