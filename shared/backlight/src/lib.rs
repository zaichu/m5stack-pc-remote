use core::time::Duration;

pub const SLEEP_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    ResetIdle,
    TurnOff,
    Wake,
}

pub fn decide(backlight_on: bool, touch_rising_edge: bool, idle_elapsed: Duration) -> Action {
    if backlight_on {
        if touch_rising_edge {
            Action::ResetIdle
        } else if idle_elapsed >= SLEEP_TIMEOUT {
            Action::TurnOff
        } else {
            Action::None
        }
    } else if touch_rising_edge {
        Action::Wake
    } else {
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_on_before_timeout() {
        for elapsed in [Duration::ZERO, Duration::from_millis(59_999)] {
            assert_eq!(decide(true, false, elapsed), Action::None);
        }
    }

    #[test]
    fn turns_off_at_or_after_timeout() {
        for elapsed in [
            Duration::from_secs(60),
            Duration::from_secs(61),
            Duration::MAX,
        ] {
            assert_eq!(decide(true, false, elapsed), Action::TurnOff);
        }
    }

    #[test]
    fn touch_resets_idle_even_at_or_after_timeout() {
        for elapsed in [Duration::ZERO, Duration::from_secs(60), Duration::MAX] {
            assert_eq!(decide(true, true, elapsed), Action::ResetIdle);
        }
    }

    #[test]
    fn stays_off_without_new_touch() {
        for elapsed in [Duration::ZERO, Duration::from_secs(60), Duration::MAX] {
            assert_eq!(decide(false, false, elapsed), Action::None);
        }
    }

    #[test]
    fn new_touch_wakes_regardless_of_idle_duration() {
        for elapsed in [Duration::ZERO, Duration::from_secs(60), Duration::MAX] {
            assert_eq!(decide(false, true, elapsed), Action::Wake);
        }
    }
}
