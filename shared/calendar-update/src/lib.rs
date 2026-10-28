#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CalendarDay {
    pub year: i64,
    pub month: u32,
    pub day: u32,
}

pub fn needs_redraw(displayed_day: Option<CalendarDay>, current_day: Option<CalendarDay>) -> bool {
    current_day != displayed_day
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: CalendarDay = CalendarDay {
        year: 2026,
        month: 10,
        day: 29,
    };

    #[test]
    fn stays_unchanged_while_unsynced() {
        assert!(!needs_redraw(None, None));
    }

    #[test]
    fn redraws_when_time_becomes_synced() {
        assert!(needs_redraw(None, Some(TODAY)));
    }

    #[test]
    fn redraws_when_time_becomes_unsynced() {
        assert!(needs_redraw(Some(TODAY), None));
    }

    #[test]
    fn stays_unchanged_on_the_same_date() {
        assert!(!needs_redraw(Some(TODAY), Some(TODAY)));
    }

    #[test]
    fn redraws_when_any_date_component_changes_in_either_direction() {
        for changed_day in [
            CalendarDay { day: 30, ..TODAY },
            CalendarDay { day: 28, ..TODAY },
            CalendarDay { month: 11, ..TODAY },
            CalendarDay { month: 9, ..TODAY },
            CalendarDay {
                year: 2027,
                ..TODAY
            },
            CalendarDay {
                year: 2025,
                ..TODAY
            },
        ] {
            assert!(needs_redraw(Some(TODAY), Some(changed_day)));
        }
    }
}
