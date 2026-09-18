// タッチUI。STATUS画面、WAKE / REBOOT / SHUTDOWNボタン、危険操作の確認画面を描画する。
//
// REBOOTとSHUTDOWNはPCがONLINEのときだけ表示し、m5stack-pc-bridgeへ送る前に確認画面を挟む。
//
// 画面文言は全てASCIIにする。描画に使う`mono_font::ascii`のフォントはASCII範囲外を
// 全て'?'グリフへ置き換えるため、日本語を書くと文字化けする。

use std::error::Error;

use embedded_graphics::mono_font::ascii::{FONT_10X20, FONT_6X10, FONT_8X13_BOLD};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{
    Circle, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle, RoundedRectangle,
};
use embedded_graphics::text::{Alignment, Text};

use crate::board::{Battery, Core2Display, DISPLAY_HEIGHT, DISPLAY_WIDTH};
use crate::bridge_client::PowerAction;

/// 配色。1箇所にまとめて画面全体のトーンを揃える。
mod palette {
    use embedded_graphics::pixelcolor::Rgb565;
    use embedded_graphics::prelude::RgbColor;

    /// 背景。真っ黒よりわずかに浮かせて、カードの輪郭が沈まないようにする。
    pub const BG: Rgb565 = Rgb565::new(2, 4, 6);
    /// ヘッダー帯。
    pub const HEADER: Rgb565 = Rgb565::new(4, 9, 14);
    /// カード面。背景より一段明るくして層を作る。
    pub const SURFACE: Rgb565 = Rgb565::new(4, 8, 11);
    pub const TEXT: Rgb565 = Rgb565::WHITE;
    pub const TEXT_DIM: Rgb565 = Rgb565::new(17, 34, 17);
    pub const OK: Rgb565 = Rgb565::new(6, 50, 14);
    pub const NG: Rgb565 = Rgb565::new(28, 8, 8);
    pub const WARN: Rgb565 = Rgb565::new(31, 40, 0);
    pub const ACCENT: Rgb565 = Rgb565::new(8, 32, 28);
    pub const DANGER: Rgb565 = Rgb565::new(24, 8, 8);
    pub const NEUTRAL: Rgb565 = Rgb565::new(8, 16, 20);
}

const HEADER_HEIGHT: u32 = 26;
const BANNER_HEIGHT: u32 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Button {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Button {
    /// 当たり判定。右端・下端は含めない。
    ///
    /// `draw` が使う embedded-graphics の `Rectangle::new(point, size)` は
    /// 上限が排他(x..x+w)なので、判定だけ `<=` にすると描画より1px広くなり、
    /// ボタン間の隙間の設計とずれる。
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w as i32 && y >= self.y && y < self.y + self.h as i32
    }

    /// 面 + 明るい縁取りで立体感を出す。`enabled`がfalseなら沈んだ配色にする。
    fn draw(
        &self,
        display: &mut Core2Display<'_>,
        label: &str,
        fill: Rgb565,
        enabled: bool,
    ) -> Result<(), Box<dyn Error>> {
        let (fill, border, text_color) = if enabled {
            (fill, lighten(fill), palette::TEXT)
        } else {
            (palette::NEUTRAL, palette::NEUTRAL, palette::TEXT_DIM)
        };

        RoundedRectangle::with_equal_corners(
            Rectangle::new(Point::new(self.x, self.y), Size::new(self.w, self.h)),
            Size::new(8, 8),
        )
        .into_styled(
            PrimitiveStyleBuilder::new()
                .fill_color(fill)
                .stroke_color(border)
                .stroke_width(2)
                .build(),
        )
        .draw(display)
        .map_err(|e| format!("button fill failed: {e:?}"))?;

        Text::with_alignment(
            label,
            Point::new(self.x + self.w as i32 / 2, self.y + self.h as i32 / 2 + 4),
            MonoTextStyle::new(&FONT_6X10, text_color),
            Alignment::Center,
        )
        .draw(display)
        .map_err(|e| format!("button label failed: {e:?}"))?;
        Ok(())
    }
}

/// 縁取り用に少しだけ明るい色を作る。RGB565の各チャネル上限で飽和させる。
fn lighten(color: Rgb565) -> Rgb565 {
    Rgb565::new(
        (color.r() + 6).min(31),
        (color.g() + 12).min(63),
        (color.b() + 6).min(31),
    )
}

/// メイン画面のボタン。Core2は画面下の物理ボタン帯もタッチ座標として報告する。
pub const WAKE_BUTTON: Button = Button {
    x: 10,
    y: 180,
    w: 95,
    h: 48,
};
pub const REBOOT_BUTTON: Button = Button {
    x: 112,
    y: 180,
    w: 95,
    h: 48,
};
pub const SHUTDOWN_BUTTON: Button = Button {
    x: 214,
    y: 180,
    w: 95,
    h: 48,
};

/// Main画面の時計帯タップでCalendar画面へ遷移する領域(Issue #173)。
/// 時計帯(y=134..180)の内側へ絞る。電源ボタン行(y=180..)との間に
/// 10px以上の不感帯(y=170..180)を確保する。
pub const CLOCK_TAP_ZONE: Button = Button {
    x: 24,
    y: 138,
    w: 272,
    h: 32,
};

/// Calendar画面のBACKボタン。電源ボタン行(y=180..228)と重ならないよう、
/// CANCEL_BUTTON(Confirm専用、y=150..210)とは別に小さく取る。
/// グリッド下端(148)とも重ならない。
pub const CALENDAR_BACK_BUTTON: Button = Button {
    x: 20,
    y: 150,
    w: 130,
    h: 26,
};

/// 確認画面のボタン。
pub const CANCEL_BUTTON: Button = Button {
    x: 20,
    y: 150,
    w: 130,
    h: 60,
};
pub const OK_BUTTON: Button = Button {
    x: 170,
    y: 150,
    w: 130,
    h: 60,
};

pub struct Status<'a> {
    pub wifi_connected: bool,
    pub pc_online: bool,
    pub telegram: TelegramState,
    /// Telegramの /lock で操作が禁止されている状態。
    pub locked: bool,
    /// バッテリー状態。読み取れていないときはNone。
    pub battery: Option<Battery>,
    pub toast: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TelegramState {
    Disabled,
    Polling,
    Error,
}

impl TelegramState {
    fn color(self) -> Rgb565 {
        match self {
            TelegramState::Disabled => palette::TEXT_DIM,
            TelegramState::Polling => palette::OK,
            TelegramState::Error => palette::NG,
        }
    }
}

/// ヘッダー右側の状態ランプ。色付きの点 + 短いラベルで、行を消費せずに状態を出す。
/// 次のランプを置ける左端のx座標を返す。
fn draw_lamp(
    display: &mut Core2Display<'_>,
    right_edge: i32,
    label: &str,
    color: Rgb565,
) -> Result<i32, Box<dyn Error>> {
    let text_x = right_edge - label.len() as i32 * 6;
    let dot_x = text_x - 12;

    Circle::new(Point::new(dot_x, 9), 8)
        .into_styled(PrimitiveStyle::with_fill(color))
        .draw(display)
        .map_err(|e| format!("lamp failed: {e:?}"))?;

    Text::new(
        label,
        Point::new(text_x, 17),
        MonoTextStyle::new(&FONT_6X10, palette::TEXT),
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;

    Ok(dot_x - 10)
}

fn draw_header(display: &mut Core2Display<'_>, status: &Status<'_>) -> Result<(), Box<dyn Error>> {
    Rectangle::new(
        Point::zero(),
        Size::new(DISPLAY_WIDTH as u32, HEADER_HEIGHT),
    )
    .into_styled(PrimitiveStyle::with_fill(palette::HEADER))
    .draw(display)
    .map_err(|e| format!("header failed: {e:?}"))?;

    Text::new(
        "M5 PC REMOTE",
        Point::new(10, 18),
        MonoTextStyle::new(&FONT_8X13_BOLD, palette::TEXT),
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;

    // 右端から左へ順に積む。
    let next = draw_lamp(
        display,
        DISPLAY_WIDTH as i32 - 10,
        "TG",
        status.telegram.color(),
    )?;
    let next = draw_lamp(
        display,
        next,
        "WIFI",
        if status.wifi_connected {
            palette::OK
        } else {
            palette::NG
        },
    )?;

    if let Some(battery) = status.battery {
        // 充電中・給電中(満充電で充電停止)・電池駆動の3状態を区別し、
        // どの状態でも残量%を出す。以前は充電中に%を隠して「CHG」だけ
        // 出していたため充電中の残量を確認できず、満充電で充電停止すると
        // %表示に戻って「挿したのにCHGが出ない」と見えた(Issue #153)。
        // ランプ文言の組み立ては `battery` crateに寄せ、hostテストで担保する。
        let state = battery::classify(battery.charging, battery.powered);
        let label = battery::lamp_label(battery.percent, state);
        let color = match state {
            battery::PowerState::Charging => palette::ACCENT,
            battery::PowerState::Powered => palette::OK,
            battery::PowerState::OnBattery => {
                if battery.percent >= 40 {
                    palette::OK
                } else if battery.percent >= 15 {
                    palette::WARN
                } else {
                    palette::NG
                }
            }
        };
        draw_lamp(display, next, &label, color)?;
    }
    Ok(())
}

/// PC状態を中央のカードで大きく見せる。枠線の色で状態が一目で分かるようにする。
fn draw_status_card(
    display: &mut Core2Display<'_>,
    status: &Status<'_>,
) -> Result<(), Box<dyn Error>> {
    let accent = if status.pc_online {
        palette::OK
    } else {
        palette::NG
    };

    RoundedRectangle::with_equal_corners(
        Rectangle::new(Point::new(24, 52), Size::new(272, 82)),
        Size::new(10, 10),
    )
    .into_styled(
        PrimitiveStyleBuilder::new()
            .fill_color(palette::SURFACE)
            .stroke_color(accent)
            .stroke_width(3)
            .build(),
    )
    .draw(display)
    .map_err(|e| format!("card failed: {e:?}"))?;

    Text::with_alignment(
        "TARGET PC",
        Point::new(DISPLAY_WIDTH as i32 / 2, 76),
        MonoTextStyle::new(&FONT_6X10, palette::TEXT_DIM),
        Alignment::Center,
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;

    Text::with_alignment(
        crate::net::pc_online_label_ascii(status.pc_online),
        Point::new(DISPLAY_WIDTH as i32 / 2, 110),
        MonoTextStyle::new(&FONT_10X20, accent),
        Alignment::Center,
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;

    Ok(())
}

/// 時計帯の表示文字列(Issue #172)。
/// 状態カード(y=134まで)とボタン(y=180から)の間にある空き帯へASCIIだけで描く。
pub struct ClockStrings {
    pub time: String,
    pub date: String,
}

/// 時計帯の上端。状態カードは y=52..134、ボタンは y=180 からなので、
/// y=134..180 の帯を使う。
const CLOCK_TOP: i32 = 134;
const CLOCK_HEIGHT: u32 = 46;
/// `CLOCK_TOP` から見た各行のベースライン。時刻は大きいフォント、
/// 日付は小さいフォントで中央寄せにする。
const CLOCK_TIME_BASELINE: i32 = CLOCK_TOP + 24;
const CLOCK_DATE_BASELINE: i32 = CLOCK_TOP + 40;

/// 日付行に出す短い曜日名。0が日曜日。
const WEEKDAY_NAMES: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

/// 時刻が信頼できない(SNTP未同期)間に出す表示。
/// 1970年などの不正な値を現在時刻として出さない。
pub const CLOCK_TIME_UNSYNCED: &str = "--:--";
pub const CLOCK_DATE_UNSYNCED: &str = "--/-- ---";

/// UNIX時刻とUTCオフセット(時間)から、時計の2行を作る。
///
/// `unix_secs` がNTP同期前に見える場合は未同期表示を返す。判定は電源操作経路と
/// 同じ `net::is_ntp_synced` を使う。実行中の時計には依存しない純粋な計算なので、
/// ハードウェア無しでも整形規則を追える。
pub fn clock_strings(unix_secs: i64, tz_offset_hours: i64) -> ClockStrings {
    if !crate::net::is_ntp_synced(unix_secs) {
        return ClockStrings {
            time: CLOCK_TIME_UNSYNCED.to_string(),
            date: CLOCK_DATE_UNSYNCED.to_string(),
        };
    }
    let local = unix_secs + tz_offset_hours * 3600;
    let days = local.div_euclid(86_400);
    let secs_of_day = local.rem_euclid(86_400);
    let hour = (secs_of_day / 3600) as u32;
    let minute = ((secs_of_day % 3600) / 60) as u32;
    let (_, month, day) = civil_from_days(days);
    let weekday = WEEKDAY_NAMES[((days + 4).rem_euclid(7)) as usize];
    ClockStrings {
        time: format!("{hour:02}:{minute:02}"),
        date: format!("{month:02}/{day:02} {weekday}"),
    }
}

/// UNIX epochからの日数を(year, month, day)へ変換する。
///
/// Howard Hinnant の `civil_from_days` を整数演算だけで使う。端末側にtz databaseは
/// 無いため、UTCオフセットは呼び出し側で適用済みとする。年も返すが、時計帯には
/// `MM/DD` だけを表示する。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 時計帯だけを描き直す。
/// 前の分の文字が残らないよう帯全体を塗ってから描く。全画面clearより軽く、
/// 10秒周期の全画面再描画で増えるちらつきも避けられる。
pub fn redraw_clock(
    display: &mut Core2Display<'_>,
    clock: &ClockStrings,
) -> Result<(), Box<dyn Error>> {
    Rectangle::new(
        Point::new(0, CLOCK_TOP),
        Size::new(DISPLAY_WIDTH as u32, CLOCK_HEIGHT),
    )
    .into_styled(PrimitiveStyle::with_fill(palette::BG))
    .draw(display)
    .map_err(|e| format!("clock band failed: {e:?}"))?;

    Text::with_alignment(
        clock.time.as_str(),
        Point::new(DISPLAY_WIDTH as i32 / 2, CLOCK_TIME_BASELINE),
        MonoTextStyle::new(&FONT_10X20, palette::TEXT),
        Alignment::Center,
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;

    Text::with_alignment(
        clock.date.as_str(),
        Point::new(DISPLAY_WIDTH as i32 / 2, CLOCK_DATE_BASELINE),
        MonoTextStyle::new(&FONT_6X10, palette::TEXT_DIM),
        Alignment::Center,
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;

    Ok(())
}

/// 月間カレンダーの1日分の日付。年月日すべて含めて持ち、日付変化の検出にも使う。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CalendarDay {
    pub year: i64,
    pub month: u32,
    pub day: u32,
}

/// UNIX時刻とUTCオフセット(時間)から当日の日付を求める。
/// SNTP未同期時はNoneを返し、1970年などの誤った日付を作らない。
/// 判定は時計帯と同じ `net::is_ntp_synced` を使う。
pub fn calendar_date(unix_secs: i64, tz_offset_hours: i64) -> Option<CalendarDay> {
    if !crate::net::is_ntp_synced(unix_secs) {
        return None;
    }
    let local = unix_secs + tz_offset_hours * 3600;
    let days = local.div_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    Some(CalendarDay { year, month, day })
}

/// うるう年判定。`civil_from_days` と同じグレゴリオ暦(先発)を前提にする。
fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// 月の日数。呼び出し元は `civil_from_days` 由来の1..=12だけを渡す。
fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        // うるう年2月だけ29日。
        2 if is_leap_year(year) => 29,
        2 => 28,
        // `civil_from_days` からは来ない経路の保険。panicより安全側の30日。
        _ => 30,
    }
}

/// Calendar画面に渡す当月分の表示データ。日曜始まり固定、前月/次月なし。
/// 計算は実行中の時計に依存しない純粋な組み立てにする。
pub struct CalendarView {
    /// ヘッダータイトル。`YYYY-MM`、未同期時は`----/--`。
    pub title: String,
    /// 同期済みかどうか。falseならグリッドは空で `NO CLOCK` を出す。
    pub synced: bool,
    /// 6行7列の日曜始まりグリッド。当月外はNone。
    pub weeks: [[Option<u32>; 7]; 6],
    /// 今日の日付。未同期時はNone。
    pub today: Option<u32>,
}

/// 当月の月間カレンダーを組み立てる。
/// 月初の曜日は、当日の通日からの差分で求める。`days_from_civil` のような
/// 逆変換を増やさず、既存 `civil_from_days` と曜日計算を整合させる。
pub fn calendar_view(unix_secs: i64, tz_offset_hours: i64) -> CalendarView {
    let Some(today) = calendar_date(unix_secs, tz_offset_hours) else {
        return CalendarView {
            title: "----/--".to_string(),
            synced: false,
            weeks: [[None; 7]; 6],
            today: None,
        };
    };
    let local = unix_secs + tz_offset_hours * 3600;
    let days = local.div_euclid(86_400);
    let first_days = days - (today.day as i64 - 1);
    // 0=日曜。`clock_strings` と同じ式で、1970-01-01(木曜)=4になる。
    let first_weekday = (first_days + 4).rem_euclid(7) as usize;
    let month_len = days_in_month(today.year, today.month);
    let mut weeks = [[None; 7]; 6];
    for day in 1..=month_len {
        let idx = first_weekday + (day as usize - 1);
        weeks[idx / 7][idx % 7] = Some(day);
    }
    CalendarView {
        title: format!("{:04}-{:02}", today.year, today.month),
        synced: true,
        weeks,
        today: Some(today.day),
    }
}

/// Calendar画面の配置。ヘッダー(0..26)と電源ボタン行(180..228)は維持し、
/// 中央にタイトル・曜日行・6行グリッドを置く。グリッド下端(148)は
/// BACKボタン(y=150..176)や電源ボタンと重ならないようにする。
const CAL_TITLE_BASELINE: i32 = 48;
const CAL_WEEKDAY_BASELINE: i32 = 64;
const CAL_GRID_TOP: i32 = 70;
const CAL_GRID_LEFT: i32 = 27;
const CAL_COL_W: i32 = 38;
const CAL_ROW_H: i32 = 13;
const CAL_NO_CLOCK_BASELINE: i32 = 112;
/// 曜日行。日曜始まり固定。
const CAL_WEEKDAY_NAMES: [&str; 7] = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"];

/// グリッド列の中央x座標。
fn calendar_cell_center_x(col: usize) -> i32 {
    CAL_GRID_LEFT + col as i32 * CAL_COL_W + CAL_COL_W / 2
}

/// Calendar画面の描画。ヘッダーと電源ボタン行はMainと同じ座標・条件で描き、
/// 中央領域にだけ月間カレンダーを出す。トーストとロック表示もMainと同じ扱い。
/// BACKボタンは `CALENDAR_BACK_BUTTON` を使う。`CANCEL_BUTTON` はConfirm専用で、
/// 電源ボタン行と重なるためCalendarでは使わない。
pub fn draw_calendar(
    display: &mut Core2Display<'_>,
    status: &Status<'_>,
    view: &CalendarView,
) -> Result<(), Box<dyn Error>> {
    display
        .clear(palette::BG)
        .map_err(|e| format!("clear failed: {e:?}"))?;

    draw_header(display, status)?;

    Text::with_alignment(
        view.title.as_str(),
        Point::new(DISPLAY_WIDTH as i32 / 2, CAL_TITLE_BASELINE),
        MonoTextStyle::new(&FONT_8X13_BOLD, palette::TEXT),
        Alignment::Center,
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;

    if !view.synced {
        // 未同期時はグリッドを出さず、誤った1970年カレンダーを見せない。
        Text::with_alignment(
            "NO CLOCK",
            Point::new(DISPLAY_WIDTH as i32 / 2, CAL_NO_CLOCK_BASELINE),
            MonoTextStyle::new(&FONT_10X20, palette::TEXT),
            Alignment::Center,
        )
        .draw(display)
        .map_err(|e| format!("draw failed: {e:?}"))?;
    } else {
        for (col, name) in CAL_WEEKDAY_NAMES.iter().enumerate() {
            Text::with_alignment(
                name,
                Point::new(calendar_cell_center_x(col), CAL_WEEKDAY_BASELINE),
                MonoTextStyle::new(&FONT_6X10, palette::TEXT_DIM),
                Alignment::Center,
            )
            .draw(display)
            .map_err(|e| format!("draw failed: {e:?}"))?;
        }
        for (row, week) in view.weeks.iter().enumerate() {
            for (col, cell) in week.iter().enumerate() {
                if let Some(day) = cell {
                    let center_x = calendar_cell_center_x(col);
                    let row_top = CAL_GRID_TOP + row as i32 * CAL_ROW_H;
                    let is_today = view.today == Some(*day);
                    if is_today {
                        // 今日のセルだけ背景を塗る。セル単位の矩形なので
                        // フォントのベースライン位置に依存しない。
                        Rectangle::new(
                            Point::new(center_x - 12, row_top),
                            Size::new(24, CAL_ROW_H as u32),
                        )
                        .into_styled(PrimitiveStyle::with_fill(palette::ACCENT))
                        .draw(display)
                        .map_err(|e| format!("calendar today failed: {e:?}"))?;
                    }
                    Text::with_alignment(
                        day.to_string().as_str(),
                        Point::new(center_x, row_top + 10),
                        MonoTextStyle::new(&FONT_6X10, palette::TEXT),
                        Alignment::Center,
                    )
                    .draw(display)
                    .map_err(|e| format!("draw failed: {e:?}"))?;
                }
            }
        }
    }

    // BACKボタン。電源ボタン行(y=180..)ともグリッド(下端148)とも重ならない。
    // `Button::draw` のラベルはボタン中央(y=167付近)に出て、隠れずに見える。
    CALENDAR_BACK_BUTTON.draw(display, "BACK", palette::NEUTRAL, true)?;

    // ロック中はMainと同じく沈めた配色にする。タップ自体はmain.rs側で弾く。
    let enabled = !status.locked;
    WAKE_BUTTON.draw(display, "WAKE", palette::ACCENT, enabled)?;
    // REBOOT / SHUTDOWNはMainと同じくPC起動中だけ表示する。
    if status.pc_online {
        REBOOT_BUTTON.draw(display, "REBOOT", palette::WARN, enabled)?;
        SHUTDOWN_BUTTON.draw(display, "SHUTDOWN", palette::DANGER, enabled)?;
    }

    // トーストは一時的な結果表示なので、常時表示のロックより優先する。
    // Main画面とあえて同じ文言・同じ優先順位にする。
    if let Some(text) = status.toast {
        draw_banner(display, text, palette::ACCENT)?;
    } else if status.locked {
        draw_banner(display, "LOCKED - send /unlock in Telegram", palette::WARN)?;
    }

    Ok(())
}

/// 画面下部のバナー。トーストとロック表示で共用する。
fn draw_banner(
    display: &mut Core2Display<'_>,
    text: &str,
    color: Rgb565,
) -> Result<(), Box<dyn Error>> {
    let top = DISPLAY_HEIGHT as i32 - BANNER_HEIGHT as i32;
    Rectangle::new(
        Point::new(0, top),
        Size::new(DISPLAY_WIDTH as u32, BANNER_HEIGHT),
    )
    .into_styled(PrimitiveStyle::with_fill(color))
    .draw(display)
    .map_err(|e| format!("banner failed: {e:?}"))?;

    Text::with_alignment(
        text,
        Point::new(DISPLAY_WIDTH as i32 / 2, top + 14),
        MonoTextStyle::new(&FONT_6X10, palette::TEXT),
        Alignment::Center,
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;
    Ok(())
}

/// ヘッダー帯(高さ26px)だけを描き直す。バッテリー残量の変化など、
/// ヘッダー内のランプだけが変わったときに使う。
/// `draw_header` は帯全体を先に塗りつぶすため、古いランプ文言の長さが
/// 変わってもゴーストは残らない。全画面clearはしないので、ちらつきと
/// 転送量(帯分16,640B、全画面の約1/9)が少ない。
pub fn redraw_header(
    display: &mut Core2Display<'_>,
    status: &Status<'_>,
) -> Result<(), Box<dyn Error>> {
    draw_header(display, status)
}

pub fn draw_main(
    display: &mut Core2Display<'_>,
    status: &Status<'_>,
    clock: &ClockStrings,
) -> Result<(), Box<dyn Error>> {
    display
        .clear(palette::BG)
        .map_err(|e| format!("clear failed: {e:?}"))?;

    draw_header(display, status)?;
    draw_status_card(display, status)?;
    redraw_clock(display, clock)?;

    // ロック中はボタンを沈めた配色にして、押しても動かないことを見た目でも示す。
    let enabled = !status.locked;
    WAKE_BUTTON.draw(display, "WAKE", palette::ACCENT, enabled)?;
    // REBOOT / SHUTDOWNはPC起動中だけ表示して、誤操作の入口を減らす。
    if status.pc_online {
        REBOOT_BUTTON.draw(display, "REBOOT", palette::WARN, enabled)?;
        SHUTDOWN_BUTTON.draw(display, "SHUTDOWN", palette::DANGER, enabled)?;
    }

    // トーストは一時的な結果表示なので、常時表示のロックより優先する。
    if let Some(text) = status.toast {
        draw_banner(display, text, palette::ACCENT)?;
    } else if status.locked {
        draw_banner(display, "LOCKED - send /unlock in Telegram", palette::WARN)?;
    }

    Ok(())
}

pub fn draw_confirm(
    display: &mut Core2Display<'_>,
    action: PowerAction,
) -> Result<(), Box<dyn Error>> {
    display
        .clear(palette::BG)
        .map_err(|e| format!("clear failed: {e:?}"))?;

    // 危険操作の確認画面。赤い帯で通常画面と明確に区別する。
    Rectangle::new(
        Point::zero(),
        Size::new(DISPLAY_WIDTH as u32, HEADER_HEIGHT),
    )
    .into_styled(PrimitiveStyle::with_fill(palette::DANGER))
    .draw(display)
    .map_err(|e| format!("header failed: {e:?}"))?;

    Text::with_alignment(
        "CONFIRM",
        Point::new(DISPLAY_WIDTH as i32 / 2, 18),
        MonoTextStyle::new(&FONT_8X13_BOLD, palette::TEXT),
        Alignment::Center,
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;

    let title = match action {
        PowerAction::Reboot => "REBOOT?",
        PowerAction::Shutdown => "SHUTDOWN?",
    };
    Text::with_alignment(
        title,
        Point::new(DISPLAY_WIDTH as i32 / 2, 82),
        MonoTextStyle::new(&FONT_10X20, palette::TEXT),
        Alignment::Center,
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;

    Text::with_alignment(
        "OK sends a signed command",
        Point::new(DISPLAY_WIDTH as i32 / 2, 108),
        MonoTextStyle::new(&FONT_6X10, palette::TEXT_DIM),
        Alignment::Center,
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;

    CANCEL_BUTTON.draw(display, "CANCEL", palette::NEUTRAL, true)?;
    OK_BUTTON.draw(display, "OK", palette::DANGER, true)?;

    Ok(())
}
