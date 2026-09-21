use crate::{compare_versions, VersionOrder};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FirmwareNotice {
    last_notified: Option<String>,
}

impl FirmwareNotice {
    pub fn observe(self, current: &str, verified_version: Option<&str>) -> (Self, bool) {
        let Some(offered) = verified_version else {
            return (self, false);
        };
        if compare_versions(current, offered) != VersionOrder::Newer {
            return (self, false);
        }
        if let Some(previous) = &self.last_notified {
            if compare_versions(previous, offered) != VersionOrder::Newer {
                return (self, false);
            }
        }
        (
            Self {
                last_notified: Some(offered.to_owned()),
            },
            true,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirmwareCheckSchedule {
    next_check_secs: u64,
}

impl Default for FirmwareCheckSchedule {
    fn default() -> Self {
        Self {
            next_check_secs: 180,
        }
    }
}

impl FirmwareCheckSchedule {
    pub fn poll(self, elapsed_secs: u64) -> (Self, bool) {
        if elapsed_secs < self.next_check_secs {
            return (self, false);
        }
        (
            Self {
                next_check_secs: elapsed_secs.saturating_add(6 * 60 * 60),
            },
            true,
        )
    }
}

pub fn firmware_available_text(current: &str, offered: &str) -> String {
    format!("新しいfirmware {offered} が利用可能です(現在 {current})。/update で更新できます")
}
