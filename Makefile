.PHONY: install install-hooks fmt fmt-check clippy test agent-check firmware-build \
	firmware-rust-build firmware-rust-poc-build secret-path-check secret-scan diff-check check \
	git-pre-commit git-pre-push

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

firmware-build:
	@if command -v pio >/dev/null 2>&1; then \
		pio run -d firmware; \
	else \
		echo "WARNING: PlatformIO CLI 'pio' not found; skipping firmware build."; \
	fi

# Rust firmware (Issue #17). Needs the Xtensa Rust toolchain from espup and
# a local config.toml, so it skips with a warning when either is missing
# (same policy as firmware-build without PlatformIO).
firmware-rust-build:
	bash ./scripts/build-firmware-rust-poc.sh

firmware-rust-poc-build: firmware-rust-build

secret-path-check:
	bash ./scripts/check-staged-secret-paths.sh

secret-scan:
	bash ./scripts/scan-secrets.sh

diff-check:
	git diff --check

check: diff-check secret-path-check secret-scan agent-check firmware-build \
	firmware-rust-build

git-pre-commit: fmt-check secret-path-check diff-check

git-pre-push: check
