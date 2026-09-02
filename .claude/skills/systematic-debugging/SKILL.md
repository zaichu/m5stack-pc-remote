---
name: systematic-debugging
description: Use when firmware, m5stack-pc-bridge, CI, or local tests fail unexpectedly.
---

# Systematic Debugging

## Rule

原因を特定する前に場当たり的にpatchしない。

## Workflow

1. 失敗コマンドまたは実機症状を再現する。
2. エラー全文、時刻、対象コンポーネントを読む。
3. 境界を特定する。
   - firmware Wi-Fi
   - firmware WOL
   - firmware STATUS
   - m5stack-pc-bridge auth
   - m5stack-pc-bridge power command
   - future external relay
4. 期待設定と実設定を比較する。
5. 仮説を1つ立て、根拠を書く。
6. 最小修正を入れる。
7. 同じ失敗を検出できるテストまたはコマンドで検証する。

実PCのshutdown/reboot、実LAN設定変更、外部公開変更が必要な場合は、事前にユーザーへ確認する。
