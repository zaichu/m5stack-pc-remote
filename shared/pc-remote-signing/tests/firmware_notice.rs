use pc_remote_signing::{firmware_available_text, FirmwareCheckSchedule, FirmwareNotice};

#[test]
fn first_check_transition_table() {
    for (offered, notify) in [
        (Some("0.11.0"), true),
        (Some("0.8.0"), false),
        (Some("0.7.0"), false),
        (Some("unknown"), false),
        (Some("0.8.0-diag"), false),
        (None, false),
    ] {
        let state = FirmwareNotice::default();
        let (next, actual) = state.clone().observe("0.8.0", offered);
        assert_eq!(actual, notify, "{offered:?}");
        if !notify {
            assert_eq!(next, state, "{offered:?}");
        }
    }
}

#[test]
fn notified_version_transition_table() {
    let (state, notify) = FirmwareNotice::default().observe("0.8.0", Some("0.11.0"));
    assert!(notify);
    for (offered, expected) in [
        (Some("0.11.0"), false),
        (Some("0.10.0"), false),
        (Some("0.7.0"), false),
        (Some("0.12.0"), true),
        (Some("unknown"), false),
        (Some("0.11.0-diag"), false),
        (None, false),
    ] {
        let (next, notify) = state.clone().observe("0.8.0", offered);
        assert_eq!(notify, expected, "{offered:?}");
        if expected {
            assert!(!next.observe("0.8.0", offered).1);
        } else {
            assert_eq!(next, state, "{offered:?}");
        }
    }
}

#[test]
fn unknown_current_never_notifies() {
    assert!(
        !FirmwareNotice::default()
            .observe("unknown", Some("0.11.0"))
            .1
    );
}

#[test]
fn notification_wording() {
    assert_eq!(
        firmware_available_text("0.8.0", "0.11.0"),
        "新しいfirmware 0.11.0 が利用可能です(現在 0.8.0)。/update で更新できます"
    );
}

#[test]
fn schedule_waits_then_checks_every_six_hours() {
    let state = FirmwareCheckSchedule::default();
    for now in [0, 60, 179] {
        assert_eq!(state.poll(now), (state, false));
    }
    let (state, due) = state.poll(180);
    assert!(due);
    for now in [180, 240, 21779] {
        assert_eq!(state.poll(now), (state, false));
    }
    let (state, due) = state.poll(21780);
    assert!(due);
    assert!(!state.poll(21780).1);
    let (late, due) = state.poll(50000);
    assert!(due);
    assert!(!late.poll(50000).1);
    assert!(late.poll(71600).1);
}
