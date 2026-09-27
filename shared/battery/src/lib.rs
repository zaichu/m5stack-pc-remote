//! バッテリー残量推定と給電状態表示の純粋ロジック。
//! `firmware` はESP32専用でhostビルドできないため、判定と文言だけを分離しhostで
//! テストする(`shared/*` 共通の方針)。I2C読み出しは `board.rs` が担当。

/// 給電あり・充電なしのときに満充電とみなす電池電圧の閾値(V)。
///
/// 実機実測で満充電停止時は4.139〜4.173Vだったため、40mVのマージンを取って4.10V。
/// 4.00V(75%)より十分高く、中盤の電圧を100%へ誤認しない。
pub const FULL_CHARGE_VOLTS: f32 = 4.10;

/// 外部給電と充電の有無を組み合わせた3状態。
/// `charging` 1bitでは「給電中だが満充電で充電停止」を表せなかった(Issue #153)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerState {
    /// 充電電流が流れている。
    Charging,
    /// 給電されているが充電していない(充電器が満充電と判断して停止中)。
    Powered,
    /// 給電なし(電池駆動)。
    OnBattery,
}

/// 充電・給電の有無から3状態へ分類する。両方trueの矛盾した入力では充電を優先する
/// (流れている事実のほうが確度が高く、充電表示を落とす側へは倒さない)。
pub fn classify(charging: bool, powered: bool) -> PowerState {
    if charging {
        PowerState::Charging
    } else if powered {
        PowerState::Powered
    } else {
        PowerState::OnBattery
    }
}

/// 電池電圧から残量%を推定する(数%程度の目安)。
///
/// Li-Poの放電カーブは中央が平坦なので折れ線で概算する。上端4.15Vは実測根拠
/// (満充電停止直後が4.139〜4.173V。旧4.20Vは実測に現れなかった)。下端は未計測のまま。
/// 5%刻みに丸めるのは、1%単位の揺れで画面を描き直してちらつくため。
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
/// 給電あり・充電なし・`FULL_CHARGE_VOLTS` 以上は充電器が満充電終了した状態なので
/// 100%とみなす(充電器の判定のほうが電圧推定より確度が高い。Issue #153の実測)。
pub fn battery_percent(volts: f32, powered: bool, charging: bool) -> u8 {
    if powered && !charging && volts >= FULL_CHARGE_VOLTS {
        return 100;
    }
    percent_from_volts(volts)
}

/// 画面ヘッダー用の短い表示文言(ASCIIのみ)。
///
/// `mono_font::ascii` は非ASCIIを'?'へ落とすため記号・日本語は使えない。
/// どの状態でも残量%を出す(充電中に%を隠して確認できなかった Issue #153)。
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
        PowerState::Charging => format!("{percent}% 充電中"),
        PowerState::Powered => format!("{percent}% 満充電・給電中"),
        PowerState::OnBattery => format!("{percent}% 電池駆動"),
    }
}

/// 給電変化を通知するまでに必要な同じ観測の連続回数(1秒周期×5回=約5秒)。
///
/// 接触不良のチャタリング(1〜2秒)は抑えつつ、停電・コンセント抜けはバッテリーが
/// 尽きる前に早く知りたいため、PC状態通知(20秒)より短くした。
pub const POWER_NOTIFY_STABLE_POLLS: u8 = 5;

/// 給電変化の通知状態。PC状態通知と同じ考え方(最初の観測は通知せず基準値化、
/// N回連続で同じ結果のときだけ通知)を純粋な値として切り出したもの。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerNotify {
    /// 直前に通知(または基準値として取り込み)した給電状態。最初の観測は通知せず
    /// 取り込むだけにする(さもないと再起動のたびに通知が飛ぶ)。
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
    /// 通知は1回の変化につき1回だけ(バッテリー駆動中は電力が有限なため繰り返さない)。
    pub fn poll(self, observed: bool) -> (Self, bool) {
        match self.notified {
            // 最初の観測は通知せず基準値として取り込む。
            None => (
                Self {
                    notified: Some(observed),
                    streak: 0,
                },
                false,
            ),
            // 変化なし: 連続カウントを捨てる(一瞬の接触不良はここで消える)。
            Some(prev) if prev == observed => (
                Self {
                    notified: Some(prev),
                    streak: 0,
                },
                false,
            ),
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

/// 給電変化の通知文(日本語)。用語は `docs/glossary.md` が正本。
/// 給電断には残量%を添える(バッテリー駆動の持ち時間の目安)。復帰側に「切れていた
/// 時間」は添えない(NTP未同期でも壊れないよう時刻計算を持ち込まない)。
pub fn power_state_notification_ja(powered: bool, percent: u8) -> String {
    if powered {
        "M5Stackの給電が戻りました。".to_string()
    } else {
        format!("M5Stackの給電が切れました(バッテリー駆動に切り替わりました。残量 {percent}%)。")
    }
}

/// 画面表示に使う値だけを抜き出したもの(`board::Battery` と1対1)。
/// 残量%は5%刻みへ丸め済みのため、電圧のわずかな揺れでちらつかない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayState {
    pub percent: u8,
    pub powered: bool,
    pub charging: bool,
}

/// 表示に使う値が変わったときだけtrueを返す。
/// `draw_main` は全画面消去から始まるため無条件で呼ぶとちらつく。読み取り失敗
/// (None)の発生・復帰も変化として扱う(固まった表示にしない)。
pub fn needs_redraw(prev: Option<DisplayState>, next: Option<DisplayState>) -> bool {
    prev != next
}

/// AXP192の電源系割り込み(IRQ)に関する純粋定義。
///
/// レジスタ番号とビット位置は X-Powers AXP192 Datasheet v1.13 による。
/// IRQ enable: REG 0x40-0x43,0x4A / status: REG 0x44-0x47,0x4D(write-1-to-clear)。
pub mod power_irq {
    /// 有効化する割り込みの `(enableレジスタ, ORするビット)`。
    /// ACIN挿入/抜去・VBUS挿入/抜去・充電開始/完了だけに絞る
    /// (0x40: bit6=ACIN挿入,bit5=抜去,bit3=VBUS挿入,bit2=抜去。0x41: bit3=充電開始,bit2=完了)。
    /// 電池温度・PEK・過負荷などのビットは触らない。
    pub const ENABLE_UPDATES: [(u8, u8); 2] = [(0x40, 0x6C), (0x41, 0x0C)];

    /// クリアする `(statusレジスタ, 1を書くビット)`。有効化と対になる
    /// (0x44↔0x40、0x45↔0x41)。無関係なビットへ1を書くと他用途のラッチを消すため
    /// 有効化したビットだけを書く。
    pub const STATUS_CLEAR: [(u8, u8); 2] = [(0x44, 0x6C), (0x45, 0x0C)];

    /// 判定に使う `(statusレジスタ, マスクビット)`。`STATUS_CLEAR` と同じ値だが
    /// 意味が逆なので別名にする(片方の変更が他へ波及する事故を防ぐ)。
    pub const STATUS_MASK: [(u8, u8); 2] = [(0x44, 0x6C), (0x45, 0x0C)];

    /// IRQラッチ(REG 0x44,0x45)に電源系イベントが残っているか。
    /// ラッチはエッジの記憶なので短い抜き挿しも残る。falseならI2Cを触らずに終えられる。
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
            "95% 充電中"
        );
        assert_eq!(
            status_ja(100, PowerState::Powered),
            "100% 満充電・給電中"
        );
        assert_eq!(
            status_ja(95, PowerState::OnBattery),
            "95% 電池駆動"
        );
    }

    #[test]
    fn measured_cases_end_to_end() {
        // USB挿(満充電停止): 4.173V・給電あり・充電なし → 満充電の給電中表示。
        let percent = battery_percent(4.173, true, false);
        let state = classify(false, true);
        assert_eq!(percent, 100);
        assert_eq!(lamp_label(percent, state), "PWR 100%");
        assert_eq!(
            status_ja(percent, state),
            "100% 満充電・給電中"
        );

        // USB抜: 4.114V・給電なし・充電なし → 電池駆動の表示。
        let percent = battery_percent(4.114, false, false);
        let state = classify(false, false);
        assert_eq!(percent, 95);
        assert_eq!(lamp_label(percent, state), "95%");
        assert_eq!(status_ja(percent, state), "95% 電池駆動");

        // 充電中: 3.90V・給電あり・充電あり → 充電中表示。
        // 3.90V → 50 + (0.05/0.15)*25 = 58.3% → 60%。
        let percent = battery_percent(3.90, true, true);
        let state = classify(true, true);
        assert_eq!(percent, 60);
        assert_eq!(lamp_label(percent, state), "CHG 60%");
        assert_eq!(status_ja(percent, state), "60% 充電中");
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
