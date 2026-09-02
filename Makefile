.PHONY: install install-hooks fmt fmt-check clippy test agent-windows-build agent-check \
	firmware-build firmware-nvs-image \
	secret-path-check secret-scan diff-check check git-pre-commit git-pre-push

install: install-hooks

install-hooks:
	bash ./scripts/install-git-hooks.sh

fmt:
	cargo fmt --manifest-path m5stack-pc-bridge/Cargo.toml

fmt-check:
	cargo fmt --manifest-path m5stack-pc-bridge/Cargo.toml --check

clippy:
	cargo clippy --manifest-path m5stack-pc-bridge/Cargo.toml --all-targets -- -D warnings

test:
	cargo test --manifest-path m5stack-pc-bridge/Cargo.toml

agent-windows-build:
	@if rustup target list --installed 2>/dev/null | grep -q '^x86_64-pc-windows-gnu$$' && command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then \
		cargo build --manifest-path m5stack-pc-bridge/Cargo.toml --release --target x86_64-pc-windows-gnu; \
	else \
		echo "WARNING: x86_64-pc-windows-gnu target / mingw-w64 not found; skipping m5stack-pc-bridge Windows cross-build."; \
	fi

agent-check: fmt-check clippy test agent-windows-build

# Rust firmware。espupのXtensa Rust toolchainとローカルconfig.tomlが必要。
# どちらかが無い環境では警告だけ出してskipする。
firmware-build:
	bash ./scripts/build-firmware.sh

firmware-nvs-image:
	python3 ./scripts/provision-firmware-nvs.py

secret-path-check:
	bash ./scripts/check-staged-secret-paths.sh

secret-scan:
	bash ./scripts/scan-secrets.sh

diff-check:
	git diff --check

check: diff-check secret-path-check secret-scan agent-check firmware-build

git-pre-commit: fmt-check secret-path-check diff-check

git-pre-push: check
