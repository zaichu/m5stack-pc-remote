use std::error::Error;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use m5unified::{colors, M5Unified};

#[cfg(target_os = "espidf")]
fn link_platform_patches() {
    esp_idf_sys::link_patches();
}

#[cfg(not(target_os = "espidf"))]
fn link_platform_patches() {}

fn draw_boot_text(m5: &mut M5Unified) -> Result<(), Box<dyn Error>> {
    m5.display.fill_screen(colors::BLACK);
    m5.display.set_text_color(colors::GREEN, colors::BLACK);
    m5.display.set_text_size(2);
    m5.display.set_cursor(8, 16);
    m5.display.println("hello from rust")?;

    let line_gap = (m5.display.font_height() / 2).max(4);
    let second_line_y = m5.display.cursor_y() + line_gap;
    m5.display.set_cursor(8, second_line_y);
    m5.display.println("m5remote-rust-poc")?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    link_platform_patches();

    println!("m5remote-rust-poc boot: before M5Unified::begin");
    let _ = io::stdout().flush();

    let mut m5 = M5Unified::begin()?;
    println!(
        "M5Unified started: display={}x{}, rotation={}",
        m5.display.width(),
        m5.display.height(),
        m5.display.rotation()
    );
    let _ = io::stdout().flush();

    draw_boot_text(&mut m5)?;

    let mut heartbeat_at = Instant::now();
    let mut heartbeat = 0u32;

    loop {
        m5.update();
        if heartbeat_at.elapsed() >= Duration::from_secs(1) {
            heartbeat = heartbeat.wrapping_add(1);
            println!("heartbeat {heartbeat}");
            let _ = io::stdout().flush();
            heartbeat_at = Instant::now();
        }
        m5.delay_ms(16);
    }
}
