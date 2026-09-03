# AGENTS.md

このリポジトリを将来変更するエージェント向けのルールです。

- 目的は M5Stack Core2 for AWS を常時稼働させ、自宅LAN上の Windows 11 Pro デスクトップPCの電源管理専用端末にすること。
- 実装はフェーズごとに動作確認できる状態を維持する。最初の正本は `Wi-Fi接続 -> Wake-on-LAN -> STATUS`。
- M5Stack firmware は `firmware/` のRust実装のみとする。旧C++/Arduino/M5Unified版は実運用検証を経て削除済み(#24)。
- Windows側は `m5stack-pc-bridge/` に閉じる。Rustで単一バイナリ配布できる構成を維持する。
- 外部スマートフォン操作は直接 m5stack-pc-bridge へ到達させない。ルーターVPNを前提にできない環境では `Smartphone -> Telegram Bot API -> M5Stack -> Windows PC` を基本経路にする。
- 外部操作経路はコストゼロを絶対条件にする（詳細は `docs/cost.md`）。
- m5stack-pc-bridge の管理ポートをインターネットへ直接公開しない。
- `REBOOT` / `SHUTDOWN` は HMAC-SHA256、timestamp、nonceで認証する。認証を弱めたり、LAN内だからという理由で無認証APIを追加しない。
- 秘密鍵、Wi-Fiパスワード、PCの実MACアドレス、実LAN構成はGitへ入れない。
- `m5stack-pc-bridge/config.toml` はローカル専用で、テンプレートだけをGit管理する。
- `firmware/config.toml` はローカル専用にし、テンプレート(`config.example.toml`)だけをGit管理する。secretをRustソース(`src/`配下)へ直接 `pub const` として書かない。コンパイラの unused警告がソース行を出力してビルドログへ秘密情報が漏れる事故があったため、`build.rs` がビルド時に `config.toml` を読み込む方式にしている。Rust firmwareは起動時にESP-IDF NVSの `m5remote` namespaceを先に読み、未設定ならビルド時configへfallbackする。
- 設定を追加したら、対応する `*.example.*`、README、関連docsを同じ変更で更新する。
- テストで実ネットワーク、実PCのshutdown/reboot、実Wi-Fi認証情報を使わない。Agentの認証や設定処理は純粋関数またはループバックでテストする。
- firmwareの実機動作確認結果は、対応するGitHub IssueまたはPRに日時と確認範囲を明記して残す。`HANDOFF.md`は変わりにくい恒久構成のみを書き、日時付きの実装履歴は書かない。
- ソースコードの識別子は英語、コメントとドキュメントは日本語または英語のどちらでもよいが、ユーザー向け説明は日本語を優先する。
- `.githooks/` はローカルで早期に事故を止める入口であり、品質ゲートの正本は `Makefile` と `.claude/skills/verify/SKILL.md` に置く。
- `git add .` / `git add -A` は使わない。stageは明示パス指定または `git add -p` にする。
- mainへ直接pushしない。短期作業branchとPRを使う。
- Codexは設計・レビュー・リリース判断を担当し、Claudeは実装・テスト・PR作成を担当する運用を標準とする。
- Codexがコード、テスト、スクリプト、ドキュメントを直接書いた場合は、PR作成前または作成後にClaudeへ逆レビューを依頼する。できない場合は理由と残リスクをPR本文に書く。
