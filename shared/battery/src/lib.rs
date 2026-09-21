//! バッテリー残量推定と給電状態表示の純粋ロジック。
//!
//! `firmware` はESP32専用でhostビルドできないため、判定と文言だけをここへ分離して
//! hostでテストする(`shared/*` 共通の方針)。I2C読み出しは `board.rs` が担当する。

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

/// 電池電圧から残量%を推定する。精度は数%程度の目安。
///
/// Li-Poの放電カーブは中央が平坦なので、線形換算では中盤が大きくずれる。折れ線で概算する。
/// 上端の100%を4.15Vにしたのは実測が根拠(満充電で充電終了した直後が4.139〜4.173V。
/// 旧4.20Vは実測に現れず、常用状態で100%に到達しなかった)。下端は未計測なので変えない。
/// 5%刻みに丸めるのは、1%ずつ動くと変化検知で画面を描き直してちらつくため。
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

/// 給電状態の変化をTelegramへ通知するまでに必要な、同じ観測の連続回数。
///
/// 給電の確認周期は1秒のため、5回連続=約5秒続いた変化だけを通知する。
/// PC状態通知(10秒周期×2回=20秒)より短くした根拠:
/// - ケーブルの接触不良などのチャタリングは通常1〜2秒程度で収まるため、
///   5秒あれば一瞬の抜き差しを抑えられる。
/// - 停電・コンセント抜けは早く知りたい(放置するとバッテリーが尽きて
///   通知も送れなくなる)ため、10秒以上は待たせない。PCの再起動(数十秒)に
///   比べ、給電の揺れが5秒続くことは稀なので誤通知のリスクは低い。
pub const POWER_NOTIFY_STABLE_POLLS: u8 = 5;

/// 給電変化の通知状態。PC状態通知の `notified_online` + `notify_streak` と
/// 同じ考え方(起動直後の最初の観測は通知せず基準値として取り込む、N回連続で
/// 同じ結果のときだけ通知する)を純粋な値として切り出したもの。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerNotify {
    /// 直前に通知(または基準値として取り込み)した給電状態。
    /// 起動直後は `None` で、最初の観測は通知せず取り込むだけにする。
    /// そうしないとM5Stackを再起動するたびに通知が飛ぶ。
    pub notified: Option<bool>,
    /// `notified` と異なる観測が連続した回数。同じ観測に戻れば0へ戻る。
    pub streak: u8,
}

impl PowerNotify {
    /// 初期状態(未観測)。最初の `poll` は通知せず基準値を取り込む。
    pub fn new() -> Self {
        Self::default()
    }

    /// 今回の給電観測(`powered`)を与え、次の内部状態と通知要否を返す。
    ///
    /// 純粋関数のため `main.rs` のループ側にはI/O(通知送信)とこの呼び出し
    /// だけが残る。通知は1回の変化につき1回だけ(`notified` を更新するため、
    /// 同じ状態が続いても重複送信しない)。バッテリー駆動中は電力が有限なため、
    /// 繰り返し送らないことが重要になる。
    pub fn poll(self, observed: bool) -> (Self, bool) {
        match self.notified {
            // 起動直後の最初の観測は通知せず基準値として取り込むだけ。
            None => (
                Self {
                    notified: Some(observed),
                    streak: 0,
                },
                false,
            ),
            // 変化なし: 連続カウントを捨てる(接触不良の一瞬はここで消える)。
            Some(prev) if prev == observed => (
                Self {
                    notified: Some(prev),
                    streak: 0,
                },
                false,
            ),
            // 変化あり: 連続回数を数え、確定回数に達したら通知する。
            Some(prev) => {
                let streak = self.streak.saturating_add(1);
                if streak >= POWER_NOTIFY_STABLE_POLLS {
                    (
                        Self {
                            notified: Some(observed),
                            streak: 0,
                        },
                        true,
                    )
                } else {
                    (
                        Self {
                            notified: Some(prev),
                            streak,
                        },
                        false,
                    )
                }
            }
        }
    }
}

/// 給電状態の変化の通知文(日本語)。
///
/// 用語は `docs/glossary.md` が正本。M5Stackが対象であることを明記し、
/// 出来事の言葉(切れました/戻りました)で書く。PC側の
/// `net::pc_state_notification_ja`(「PCが起動しました。」/「PCが停止しました。」)
/// と同じく文末に「。」を付ける。
/// 給電断の通知にはその時点の残量%を添える(バッテリー駆動があとどれくらい
/// 持つかの目安になる)。給電復帰の通知に「切れていた時間」は添えない
/// (時計がNTP未同期のときに壊れないよう、時刻計算を持ち込まないため)。
pub fn power_state_notification_ja(powered: bool, percent: u8) -> String {
    if powered {
        "M5Stackの給電が戻りました。".to_string()
    } else {
        format!("M5Stackの給電が切れました(バッテリー駆動に切り替わりました。残量 {percent}%)。")
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
/// レジスタ番号とビット位置は X-Powers AXP192 Datasheet v1.13 による。
/// IRQ enable: REG 0x40-0x43,0x4A / status: REG 0x44-0x47,0x4D(write-1-to-clear)。
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

    // --- PowerNotify (給電変化の通知判定) ---

    /// 観測列を与えて最後まで回し、通知が出た回数を数える。
    fn notify_count(observed: &[bool]) -> usize {
        let mut state = PowerNotify::new();
        let mut count = 0;
        for &o in observed {
            let (next, notify) = state.poll(o);
            state = next;
            if notify {
                count += 1;
            }
        }
        count
    }

    #[test]
    fn power_notify_stable_polls_is_five_seconds() {
        // 確定回数の変更は通知遅延の変更になるため、値を固定して検知する。
        // 1秒周期×5回=約5秒。根拠は `POWER_NOTIFY_STABLE_POLLS` のコメント参照。
        assert_eq!(POWER_NOTIFY_STABLE_POLLS, 5);
    }

    #[test]
    fn first_observation_is_baseline_not_notification() {
        // 起動直後の最初の観測は給電あり・なしのどちらでも通知しない。
        // 再起動のたびに通知しないための抑止(PC通知の `None` と同じ扱い)。
        let (_, notify) = PowerNotify::new().poll(true);
        assert!(!notify);
        let (_, notify) = PowerNotify::new().poll(false);
        assert!(!notify);
    }

    #[test]
    fn notifies_when_power_lost_stably() {
        // 給電ありで起動→なしが5回連続で初めて通知(4回までは通知なし)。
        let mut state = PowerNotify::new();
        let (next, notify) = state.poll(true);
        state = next;
        assert!(!notify);
        for i in 1..=POWER_NOTIFY_STABLE_POLLS {
            let (next, notify) = state.poll(false);
            state = next;
            if i < POWER_NOTIFY_STABLE_POLLS {
                assert!(!notify, "まだ確定前({i}回目)のため通知しない");
            } else {
                assert!(notify, "5回連続で確定したため通知する");
            }
        }
        assert_eq!(state.notified, Some(false));
        assert_eq!(state.streak, 0);
    }

    #[test]
    fn notifies_when_power_restored_stably() {
        // 給電なしで起動→ありが5回連続で初めて通知する。
        assert_eq!(notify_count(&[false, true, true, true, true]), 0);
        assert_eq!(notify_count(&[false, true, true, true, true, true]), 1);
    }

    #[test]
    fn momentary_disconnect_does_not_notify() {
        // 接触不良の一瞬(確定前に元へ戻る)は通知しない。
        // 3回連続で切れても4回目で戻れば、その後の5連続カウントも最初から。
        assert_eq!(notify_count(&[true, false, false, false, true]), 0);
        assert_eq!(
            notify_count(&[true, false, false, false, true, false, false]),
            0
        );
        // 1回だけの瞬断も通知しない。
        assert_eq!(notify_count(&[true, false, true]), 0);
    }

    #[test]
    fn flapping_never_notifies_and_never_duplicates() {
        // 切れる/戻るを交互に繰り返しても確定しない(連続カウントが育たない)。
        assert_eq!(
            notify_count(&[true, false, true, false, true, false, true]),
            0
        );
        // 一度通知した後は同じ状態が続いても重複通知しない
        // (バッテリー駆動中は電力が有限のため繰り返し送らない)。
        assert_eq!(
            notify_count(&[true, false, false, false, false, false, false, false]),
            1
        );
        // 切れる→戻るの往復で通知は各1回ずつ。
        assert_eq!(
            notify_count(&[
                true, false, false, false, false, false, //
                true, true, true, true, true, true,
            ]),
            2
        );
    }

    #[test]
    fn power_notification_text_mentions_m5stack_and_percent() {
        // 実際の文面を固定する。用語は `docs/glossary.md` が正本
        // (M5Stackは対象を明記、出来事の言葉で書く)。
        assert_eq!(
            power_state_notification_ja(false, 62),
            "M5Stackの給電が切れました(バッテリー駆動に切り替わりました。残量 62%)。"
        );
        assert_eq!(
            power_state_notification_ja(true, 62),
            "M5Stackの給電が戻りました。"
        );
    }
}
