.PHONY: check release release-native release-wasm demo bench package

check:
	./scripts/check.sh

release:
	cargo build --release --all-features

release-native:
	cargo build --release --manifest-path adapters/cpal/Cargo.toml

release-wasm:
	rustup target add wasm32-unknown-unknown
	cargo build --release --target wasm32-unknown-unknown --manifest-path adapters/wasm/Cargo.toml

demo:
	./scripts/demo.sh

bench:
	cargo bench --bench realtime

package:
	./scripts/package.sh
