use core::time::Duration;

pub const TOAST_TTL: Duration = Duration::from_secs(3);

pub fn is_expired(has_toast: bool, elapsed: Duration) -> bool {
    has_toast && elapsed >= TOAST_TTL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_visible_while_toast_is_present_but_ttl_has_not_passed() {
        for elapsed in [
            Duration::ZERO,
            Duration::from_secs(1),
            Duration::from_millis(2_999),
        ] {
            assert!(!is_expired(true, elapsed));
        }
    }

    #[test]
    fn expires_when_toast_is_present_and_ttl_has_passed() {
        for elapsed in [
            TOAST_TTL,
            Duration::from_secs(4),
            Duration::from_secs(3_600),
            Duration::MAX,
        ] {
            assert!(is_expired(true, elapsed));
        }
    }

    #[test]
    fn never_expires_without_toast_regardless_of_elapsed() {
        for elapsed in [
            Duration::ZERO,
            Duration::from_millis(2_999),
            TOAST_TTL,
            Duration::from_secs(3_600),
            Duration::MAX,
        ] {
            assert!(!is_expired(false, elapsed));
        }
    }
}
