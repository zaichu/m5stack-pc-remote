.PHONY: install install-hooks fmt fmt-check clippy test agent-check \
	firmware-build firmware-nvs-image \
	secret-path-check secret-scan diff-check check git-pre-commit git-pre-push

install: install-hooks

install-hooks:
	bash ./scripts/install-git-hooks.sh

fmt:
	cargo fmt --manifest-path windows-agent/Cargo.toml

fmt-check:
	cargo fmt --manifest-path windows-agent/Cargo.toml --check

clippy:
	cargo clippy --manifest-path windows-agent/Cargo.toml --all-targets -- -D warnings

test:
	cargo test --manifest-path windows-agent/Cargo.toml

agent-check: fmt-check clippy test

# Rust firmware。espupのXtensa Rust toolchainとローカルconfig.tomlが必要。
# どちらかが無い環境では警告だけ出してskipする。
firmware-build:
	bash ./scripts/build-firmware.sh

firmware-nvs-image:
	python3 ./scripts/provision-firmware-rust-nvs.py

secret-path-check:
	bash ./scripts/check-staged-secret-paths.sh

secret-scan:
	bash ./scripts/scan-secrets.sh

diff-check:
	git diff --check

check: diff-check secret-path-check secret-scan agent-check firmware-build

git-pre-commit: fmt-check secret-path-check diff-check

git-pre-push: check
