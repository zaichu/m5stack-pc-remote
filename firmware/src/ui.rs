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
        // 充電中は残量より「給電されている」ことを優先して示す。
        let label = if battery.charging {
            "CHG".to_string()
        } else {
            format!("{}%", battery.percent)
        };
        let color = if battery.charging {
            palette::ACCENT
        } else if battery.percent >= 40 {
            palette::OK
        } else if battery.percent >= 15 {
            palette::WARN
        } else {
            palette::NG
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

pub fn draw_main(
    display: &mut Core2Display<'_>,
    status: &Status<'_>,
) -> Result<(), Box<dyn Error>> {
    display
        .clear(palette::BG)
        .map_err(|e| format!("clear failed: {e:?}"))?;

    draw_header(display, status)?;
    draw_status_card(display, status)?;

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
