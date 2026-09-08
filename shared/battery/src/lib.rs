//! M5Stack Core2のバッテリー残量推定と給電状態表示の純粋ロジック。
//!
//! `firmware` はxtensa-esp32-espidf専用のbinary crateで、host上でビルド・
//! テストできない(`[[bin]]` のみで `[lib]` を持たず、`esp_idf_hal` 等を
//! ソース側で無条件importしているため)。残量計算と表示文言だけをここへ分離
//! することで、実機なしでhost側のテストを回せるようにする
//! (`config-validation` と同じ方針。Issue #153)。
//!
//! 扱うのは電圧→残量%の換算、満充電判定、給電状態の分類と表示文言、
//! 画面の再描画要否の判定、AXP192の電源系割り込み(IRQ)のマスク定義と
//! 状態ラッチの判定のみで、AXP192のI2C読み出しは
//! `firmware/src/board.rs` の `read_battery` が担当する。

/// 給電あり・充電なしのときに満充電とみなす電池電圧の閾値(V)。
///
/// 実測(firmware 0.7.0-diag2、M5Stack Core2実機): USB給電中の満充電停止時は
/// 4.139V(USB挿2回目)〜4.173V(USB挿1回目)で、充電電流0mA・REG01充電bit 0だった。
/// 最低値から約40mVのマージンを取った4.10Vを閾値とする。4.00V(75%)より十分
/// 高いため、中盤の電圧を誤って100%にすることはない。
pub const FULL_CHARGE_VOLTS: f32 = 4.10;

/// 外部給電の有無と充電の有無を組み合わせた3状態。
///
/// USBを挿したまま満充電になると充電だけが止まるため、従来の `charging`
/// 1bitでは「給電されているが充電していない」状態を表現できず、「挿したのに
/// CHGが出ない」という違和感になっていた(Issue #153)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerState {
    /// 実際に充電中(充電電流が流れている)。
    Charging,
    /// 給電されているが充電していない(充電器が満充電と判断して停止中)。
    Powered,
    /// 給電なし(電池駆動)。
    OnBattery,
}

/// 充電中かどうかと給電の有無から3状態へ分類する。
///
/// 両方がtrueの矛盾した組み合わせでは充電を優先する。充電電流が流れている
/// 事実のほうが確度が高く、充電表示を落とす側へは倒さない。
pub fn classify(charging: bool, powered: bool) -> PowerState {
    if charging {
        PowerState::Charging
    } else if powered {
        PowerState::Powered
    } else {
        PowerState::OnBattery
    }
}

/// 電池電圧から残量%を推定する。
///
/// Li-Poの放電カーブは中央が平坦なので、電圧を単純に線形換算すると
/// 中盤で大きくずれる。代表点を結ぶ折れ線で概算する。
/// 精度は数%程度で、残量の目安を出す用途に限る。
///
/// 上端だけ実測に合わせてある: 満充電で充電終了した直後の電池電圧は
/// 4.139〜4.173V(USB給電中、軽負荷時)だったため、100%対応を旧4.20Vから
/// 4.15V(観測範囲の中央付近)へ調整した。4.20Vは充電終了後の実測に現れない
/// 値で、常用状態では100%に到達しなかった。下端(3.3V/3.6V付近)は今回
/// 計測していないため変更しない。
///
/// 5%刻みへ丸める。電圧のわずかな揺れで表示が1%ずつ動くと、変化検知で
/// 画面を描き直してしまいちらつく。精度的にも1%単位に意味はない。
pub fn percent_from_volts(volts: f32) -> u8 {
    const CURVE: [(f32, f32); 6] = [
        (3.30, 0.0),
        (3.60, 10.0),
        (3.70, 25.0),
        (3.85, 50.0),
        (4.00, 75.0),
        (4.15, 100.0),
    ];

    let round5 = |p: f32| ((p / 5.0).round() * 5.0) as u8;

    if volts <= CURVE[0].0 {
        return 0;
    }
    for pair in CURVE.windows(2) {
        let (v_low, p_low) = pair[0];
        let (v_high, p_high) = pair[1];
        if volts < v_high {
            let ratio = (volts - v_low) / (v_high - v_low);
            return round5(p_low + ratio * (p_high - p_low));
        }
    }
    100
}

/// 給電・充電状態を加味した残量%を返す。
///
/// 給電あり(`powered`)かつ充電していない(`!charging`)かつ電池電圧が
/// `FULL_CHARGE_VOLTS` 以上の場合は、充電器が満充電と判断して充電を終了した
/// 状態なので100%とみなす。電圧からの推定より充電器の判定のほうが確度が高い
/// (Issue #153の実測: REG33=0xC3で充電設定は正常、REG01充電bit 0・充電電流
/// 0mAで満充電終了。再充電ヒステリシスを下回るまで再開しない正常動作)。
pub fn battery_percent(volts: f32, powered: bool, charging: bool) -> u8 {
    if powered && !charging && volts >= FULL_CHARGE_VOLTS {
        return 100;
    }
    percent_from_volts(volts)
}

/// 画面ヘッダー用の短い表示文言(ASCIIのみ)。
///
/// 描画フォント(`mono_font::ascii`)はASCII範囲外を'?'へ置き換えるため、
/// 日本語や電源プラグ等の記号は使えない。どの状態でも残量%を常に出す。
/// 以前は充電中に%を隠して `CHG` だけ出していたため、充電中の残量を
/// 利用者が確認できなかった(Issue #153)。
pub fn lamp_label(percent: u8, state: PowerState) -> String {
    match state {
        PowerState::Charging => format!("CHG {percent}%"),
        PowerState::Powered => format!("PWR {percent}%"),
        PowerState::OnBattery => format!("{percent}%"),
    }
}

/// Telegram `/status` 用の表示文言(日本語)。
pub fn status_ja(percent: u8, state: PowerState) -> String {
    match state {
        PowerState::Charging => format!("バッテリー: {percent}% (充電中)"),
        PowerState::Powered => format!("バッテリー: {percent}% (満充電・給電中)"),
        PowerState::OnBattery => format!("バッテリー: {percent}%"),
    }
}

/// 画面表示に使う値だけを抜き出したもの。
///
/// `firmware` 側の `board::Battery` と1対1に対応する。残量%は5%刻みへ
/// 丸め済みのため、電圧のわずかな揺れでは変わらず、ちらつき防止になる。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayState {
    pub percent: u8,
    pub powered: bool,
    pub charging: bool,
}

/// 前回表示と今回の読み取り値を比べて、描き直しが必要かを返す。
///
/// `draw_main` は全画面消去から始まるため、無条件で呼ぶとその周期で
/// 画面がちらつく。表示に使う値(percentと給電/充電状態)が変わったときだけ
/// trueにする。読み取り失敗(None)の復帰・発生も「変化」として扱い、
/// 固まった表示へはしない。
pub fn needs_redraw(prev: Option<DisplayState>, next: Option<DisplayState>) -> bool {
    prev != next
}

/// AXP192の電源系割り込み(IRQ)に関する純粋定義。
///
/// レジスタ番号とビット位置は X-Powers AXP192 Datasheet v1.13(一次情報)による。
/// IRQ enable: REG 0x40-0x43,0x4A / IRQ status: REG 0x44-0x47,0x4D。
/// statusは該当ビットへ1を書くとクリアされる(write-1-to-clear)。
/// IRQ出力自体はアクティブLow(外部51kプルアップ)。
/// `axp192` crate 0.2.0に割り込み設定APIは無いため、`firmware` 側は
/// これらの定義を使った生レジスタ書き込みになる。
pub mod power_irq {
    /// 有効化する割り込みの `(enableレジスタ, ORするビット)`。
    ///
    /// ACIN挿入/抜去・VBUS挿入/抜去・充電開始/充電完了だけに絞る。
    /// - REG 0x40 bit6=ACIN挿入(IRQ2)、bit5=ACIN抜去(IRQ3)、
    ///   bit3=VBUS挿入(IRQ5)、bit2=VBUS抜去(IRQ6) → 0x6C
    /// - REG 0x41 bit3=充電開始(IRQ12)、bit2=充電完了(IRQ13) → 0x0C
    ///
    /// 電池温度・ボタン(PEK)・過負荷などのビットは触らない。既存設定へ
    /// ORするため、他の用途の有効ビットを殺さない。
    pub const ENABLE_UPDATES: [(u8, u8); 2] = [(0x40, 0x6C), (0x41, 0x0C)];

    /// クリアする割り込み状態の `(statusレジスタ, 1を書くビット)`。
    ///
    /// 有効化したものと対になる。REG 0x44が0x40に、0x45が0x41に対応する。
    /// 関係ないビットへ1を書くと他用途のラッチを消してしまうため、
    /// 有効化したビットだけを書く。
    pub const STATUS_CLEAR: [(u8, u8); 2] = [(0x44, 0x6C), (0x45, 0x0C)];

    /// 判定に使う `(statusレジスタ, マスクビット)`。
    ///
    /// `STATUS_CLEAR` と同じ値だが意味が逆(読むときのマスク)なので別名にする。
    /// 別名にせず使い回すと、有効化・クリア・判定のどれかが変わったときに
    /// 残りへ波及して事故になる。
    pub const STATUS_MASK: [(u8, u8); 2] = [(0x44, 0x6C), (0x45, 0x0C)];

    /// IRQ状態ラッチ(REG 0x44,0x45の読み値)に電源系イベントが残っているか。
    ///
    /// ラッチはレベルではなくエッジの記憶なので、短い抜き挿しでも
    /// 次の確認まで残る。`firmware` 側はこれがtrueのときだけ
    /// `read_battery` まで進み、falseならI2Cを触らずに終える。
    pub fn is_pending(status44: u8, status45: u8) -> bool {
        (status44 & STATUS_MASK[0].1) != 0 || (status45 & STATUS_MASK[1].1) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_zero_at_or_below_lower_bound() {
        assert_eq!(percent_from_volts(3.30), 0);
        assert_eq!(percent_from_volts(3.00), 0);
    }

    #[test]
    fn keeps_lower_curve_unchanged() {
        // 下端は今回未計測のため変更しない。変更前の対応を固定する。
        assert_eq!(percent_from_volts(3.60), 10);
        assert_eq!(percent_from_volts(3.70), 25);
        assert_eq!(percent_from_volts(3.85), 50);
        assert_eq!(percent_from_volts(4.00), 75);
    }

    #[test]
    fn maps_measured_full_voltages_near_top() {
        // 実測(満充電停止時): USB挿1回目 4.173V・2回目 4.139V → 100%。
        assert_eq!(percent_from_volts(4.173), 100);
        assert_eq!(percent_from_volts(4.139), 100);
        // USB抜 4.114V → 電圧推定だけでは100%に届かず95%。
        assert_eq!(percent_from_volts(4.114), 95);
    }

    #[test]
    fn caps_above_curve_top_at_100() {
        assert_eq!(percent_from_volts(4.15), 100);
        assert_eq!(percent_from_volts(4.25), 100);
    }

    #[test]
    fn rounds_to_nearest_five_percent() {
        // 4.10V → 75 + (0.10/0.15)*25 = 91.7% → 90%。
        // 丸め境界ちょうどの値はf32誤差でずれるため、境界から離れた値で見る。
        assert_eq!(percent_from_volts(4.10), 90);
    }

    #[test]
    fn treats_powered_but_not_charging_high_voltage_as_full() {
        // 実測の満充電停止状態(USB挿・充電電流0mA・REG01充電bit 0)。
        assert_eq!(battery_percent(4.173, true, false), 100);
        assert_eq!(battery_percent(4.139, true, false), 100);
        // 閾値ちょうどでも満充電とみなす(境界は >=)。
        assert_eq!(battery_percent(FULL_CHARGE_VOLTS, true, false), 100);
        // 4.10Vは電圧推定だけでは90%にしかならない。満充電判定が無ければ
        // 100%にならない電圧で見ることで、閾値の存在自体を担保する。
        assert_eq!(percent_from_volts(4.10), 90);
        assert_eq!(battery_percent(4.10, true, false), 100);
    }

    #[test]
    fn does_not_force_full_below_threshold() {
        // 閾値未満では電圧推定のまま(4.09V → 90%)。中盤の電圧を100%にしない。
        assert_eq!(battery_percent(4.09, true, false), 90);
    }

    #[test]
    fn does_not_force_full_while_charging() {
        // 充電中は充電器が満充電判定していないため電圧推定のまま。
        // 4.12V → 75 + (0.12/0.15)*25 = 95%。
        assert_eq!(battery_percent(4.12, true, true), 95);
    }

    #[test]
    fn does_not_force_full_on_battery() {
        // 給電なしでは満充電扱いしない(USB抜の実測 4.114V → 95%)。
        assert_eq!(battery_percent(4.114, false, false), 95);
    }

    #[test]
    fn classifies_three_power_states() {
        assert_eq!(classify(true, true), PowerState::Charging);
        assert_eq!(classify(false, true), PowerState::Powered);
        assert_eq!(classify(false, false), PowerState::OnBattery);
    }

    #[test]
    fn prefers_charging_when_both_flags_set() {
        // 充電電流が流れている事実を優先する(矛盾した組み合わせでも
        // 充電表示を落とさない)。
        assert_eq!(classify(true, false), PowerState::Charging);
    }

    #[test]
    fn lamp_label_always_shows_percent() {
        // どの状態でも残量%を出す。充電中に%が隠れて残量を確認できなかった
        // のがIssue #153の違和感の一つ。
        assert_eq!(lamp_label(95, PowerState::Charging), "CHG 95%");
        assert_eq!(lamp_label(100, PowerState::Powered), "PWR 100%");
        assert_eq!(lamp_label(95, PowerState::OnBattery), "95%");
    }

    #[test]
    fn status_ja_distinguishes_three_states() {
        assert_eq!(
            status_ja(95, PowerState::Charging),
            "バッテリー: 95% (充電中)"
        );
        assert_eq!(
            status_ja(100, PowerState::Powered),
            "バッテリー: 100% (満充電・給電中)"
        );
        assert_eq!(status_ja(95, PowerState::OnBattery), "バッテリー: 95%");
    }

    #[test]
    fn measured_cases_end_to_end() {
        // USB挿(満充電停止): 4.173V・給電あり・充電なし → 満充電の給電中表示。
        let percent = battery_percent(4.173, true, false);
        let state = classify(false, true);
        assert_eq!(percent, 100);
        assert_eq!(lamp_label(percent, state), "PWR 100%");
        assert_eq!(status_ja(percent, state), "バッテリー: 100% (満充電・給電中)");

        // USB抜: 4.114V・給電なし・充電なし → 残量のみ。
        let percent = battery_percent(4.114, false, false);
        let state = classify(false, false);
        assert_eq!(percent, 95);
        assert_eq!(lamp_label(percent, state), "95%");
        assert_eq!(status_ja(percent, state), "バッテリー: 95%");

        // 充電中: 3.90V・給電あり・充電あり → 充電中表示。
        // 3.90V → 50 + (0.05/0.15)*25 = 58.3% → 60%。
        let percent = battery_percent(3.90, true, true);
        let state = classify(true, true);
        assert_eq!(percent, 60);
        assert_eq!(lamp_label(percent, state), "CHG 60%");
        assert_eq!(status_ja(percent, state), "バッテリー: 60% (充電中)");
    }

    // --- needs_redraw ---

    fn display(percent: u8, powered: bool, charging: bool) -> Option<DisplayState> {
        Some(DisplayState {
            percent,
            powered,
            charging,
        })
    }

    #[test]
    fn redraws_only_when_display_values_change() {
        // 全く同じ値なら描き直さない(ちらつき防止)。
        assert!(!needs_redraw(display(95, false, false), display(95, false, false)));
        // percent・給電・充電のいずれかが変われば描き直す。
        assert!(needs_redraw(display(95, false, false), display(90, false, false)));
        assert!(needs_redraw(
            display(95, false, false),
            display(95, true, false)
        ));
        assert!(needs_redraw(display(95, true, false), display(95, true, true)));
    }

    #[test]
    fn redraws_on_read_failure_transitions() {
        // 読み取り失敗(None)の発生・復帰も変化として描き直す。
        // 固まったままの表示と区別がつかなくなるのを防ぐ。
        assert!(needs_redraw(display(95, false, false), None));
        assert!(needs_redraw(None, display(95, false, false)));
        // 失敗が続く間は描き直さない。
        assert!(!needs_redraw(None, None));
    }

    // --- power_irq ---

    #[test]
    fn irq_masks_cover_only_power_events() {
        use power_irq::{ENABLE_UPDATES, STATUS_CLEAR, STATUS_MASK};
        // ACIN挿入bit6/抜去bit5・VBUS挿入bit3/抜去bit2だけ。
        assert_eq!(ENABLE_UPDATES, [(0x40, 0x6C), (0x41, 0x0C)]);
        // クリアと判定は有効化と対になる。
        assert_eq!(STATUS_CLEAR, [(0x44, 0x6C), (0x45, 0x0C)]);
        assert_eq!(STATUS_MASK, [(0x44, 0x6C), (0x45, 0x0C)]);
        // 過電圧bit7やVBUS弱bit1(0x40側)を有効化していないこと。
        assert_eq!(ENABLE_UPDATES[0].1 & 0x80, 0);
        assert_eq!(ENABLE_UPDATES[0].1 & 0x02, 0);
        // 電池温度bit1/bit0(0x41側)を有効化していないこと。
        assert_eq!(ENABLE_UPDATES[1].1 & 0x03, 0);
    }

    #[test]
    fn irq_pending_detects_each_power_event_bit() {
        use power_irq::is_pending;
        // 対象の6ビットは1つでも立てばtrue。
        for bit in [6, 5, 3, 2] {
            assert!(is_pending(1 << bit, 0), "status44 bit{bit}");
        }
        for bit in [3, 2] {
            assert!(is_pending(0, 1 << bit), "status45 bit{bit}");
        }
        // 何も立っていなければfalse。
        assert!(!is_pending(0x00, 0x00));
    }

    #[test]
    fn irq_pending_ignores_unrelated_bits() {
        use power_irq::is_pending;
        // 対象外(例: ACIN過電圧bit7、VBUS弱bit1、電池温度bit1/0)が
        // 立っているだけでは給電変化として扱わない。
        assert!(!is_pending(0x80 | 0x02, 0x00));
        assert!(!is_pending(0x00, 0x03));
    }
}
