// タッチUI。STATUS画面、WAKE / REBOOT / SHUTDOWNボタン、危険操作の確認画面を描画する。
//
// REBOOTとSHUTDOWNはPCがONLINEのときだけ表示し、m5stack-pc-bridgeへ送る前に確認画面を挟む。

use std::error::Error;

use embedded_graphics::mono_font::ascii::{FONT_10X20, FONT_6X10};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle, RoundedRectangle};
use embedded_graphics::text::Text;

use crate::board::{Core2Display, DISPLAY_HEIGHT, DISPLAY_WIDTH};
use crate::bridge_client::PowerAction;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Button {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Button {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x <= self.x + self.w as i32 && y >= self.y && y <= self.y + self.h as i32
    }

    fn draw(
        &self,
        display: &mut Core2Display<'_>,
        label: &str,
        fill: Rgb565,
    ) -> Result<(), Box<dyn Error>> {
        RoundedRectangle::with_equal_corners(
            Rectangle::new(Point::new(self.x, self.y), Size::new(self.w, self.h)),
            Size::new(6, 6),
        )
        .into_styled(PrimitiveStyle::with_fill(fill))
        .draw(display)
        .map_err(|e| format!("button fill failed: {e:?}"))?;

        // FONT_6X10は1文字6pxなので、概算で中央寄せする。
        let text_x = self.x + (self.w as i32 - label.len() as i32 * 6) / 2;
        let text_y = self.y + self.h as i32 / 2 + 4;
        Text::new(
            label,
            Point::new(text_x.max(self.x + 2), text_y),
            MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE),
        )
        .draw(display)
        .map_err(|e| format!("button label failed: {e:?}"))?;
        Ok(())
    }
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
    pub toast: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TelegramState {
    Disabled,
    Polling,
    Error,
}

impl TelegramState {
    fn label(self) -> &'static str {
        match self {
            TelegramState::Disabled => "Telegram: disabled",
            TelegramState::Polling => "Telegram: polling",
            TelegramState::Error => "Telegram: error",
        }
    }

    fn color(self) -> Rgb565 {
        match self {
            TelegramState::Disabled => Rgb565::CSS_LIGHT_GRAY,
            TelegramState::Polling => Rgb565::GREEN,
            TelegramState::Error => Rgb565::RED,
        }
    }
}

pub fn draw_main(
    display: &mut Core2Display<'_>,
    status: &Status<'_>,
) -> Result<(), Box<dyn Error>> {
    display
        .clear(Rgb565::BLACK)
        .map_err(|e| format!("clear failed: {e:?}"))?;

    Text::new(
        "m5remote-rust",
        Point::new(12, 14),
        MonoTextStyle::new(&FONT_6X10, Rgb565::CSS_LIGHT_GRAY),
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;

    Text::new(
        if status.wifi_connected {
            "Wi-Fi: connected"
        } else {
            "Wi-Fi: disconnected"
        },
        Point::new(12, 32),
        MonoTextStyle::new(
            &FONT_6X10,
            if status.wifi_connected {
                Rgb565::GREEN
            } else {
                Rgb565::RED
            },
        ),
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;

    Text::new(
        status.telegram.label(),
        Point::new(12, 48),
        MonoTextStyle::new(&FONT_6X10, status.telegram.color()),
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;

    // ロック中は操作しても何も起きないため、理由が分かるよう明示する。
    if status.locked {
        Text::new(
            "LOCKED (/unlock to enable)",
            Point::new(12, 64),
            MonoTextStyle::new(&FONT_6X10, Rgb565::CSS_ORANGE),
        )
        .draw(display)
        .map_err(|e| format!("draw failed: {e:?}"))?;
    }

    Text::new(
        if status.pc_online {
            "ONLINE"
        } else {
            "OFFLINE"
        },
        Point::new(105, 110),
        MonoTextStyle::new(
            &FONT_10X20,
            if status.pc_online {
                Rgb565::GREEN
            } else {
                Rgb565::RED
            },
        ),
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;

    WAKE_BUTTON.draw(display, "WAKE", Rgb565::CSS_DARK_GREEN)?;
    // REBOOT / SHUTDOWNはPC起動中だけ表示して、誤操作の入口を減らす。
    if status.pc_online {
        REBOOT_BUTTON.draw(display, "REBOOT", Rgb565::CSS_DARK_ORANGE)?;
        SHUTDOWN_BUTTON.draw(display, "SHUTDOWN", Rgb565::CSS_DARK_RED)?;
    }

    if let Some(text) = status.toast {
        Text::new(
            text,
            Point::new(12, DISPLAY_HEIGHT as i32 - 6),
            MonoTextStyle::new(&FONT_6X10, Rgb565::YELLOW),
        )
        .draw(display)
        .map_err(|e| format!("draw failed: {e:?}"))?;
    }

    Ok(())
}

pub fn draw_confirm(
    display: &mut Core2Display<'_>,
    action: PowerAction,
) -> Result<(), Box<dyn Error>> {
    display
        .clear(Rgb565::BLACK)
        .map_err(|e| format!("clear failed: {e:?}"))?;

    let title = match action {
        PowerAction::Reboot => "REBOOT?",
        PowerAction::Shutdown => "SHUTDOWN?",
    };
    let title_x = (DISPLAY_WIDTH as i32 - title.len() as i32 * 10) / 2;
    Text::new(
        title,
        Point::new(title_x.max(8), 70),
        MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE),
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;

    Text::new(
        "OK sends a signed command",
        Point::new(12, 100),
        MonoTextStyle::new(&FONT_6X10, Rgb565::CSS_LIGHT_GRAY),
    )
    .draw(display)
    .map_err(|e| format!("draw failed: {e:?}"))?;

    CANCEL_BUTTON.draw(display, "CANCEL", Rgb565::CSS_DIM_GRAY)?;
    OK_BUTTON.draw(display, "OK", Rgb565::CSS_DARK_RED)?;

    Ok(())
}
