# Build static binaries to copy onto the servers under test.
#
#   make test   unit tests (release mode so the heavy kernels are quick)
#   make dist   dist/cpubench-x86_64, cpubench-x86_64-v3 (AVX2/FMA), cpubench-aarch64
#
# All three are static musl binaries linked with rust-lld: no libc or toolchain
# is needed on the target (old glibc on Kylin/openEuler/CentOS is not a problem)
# and both architectures run identical libc code, which keeps A/B results fair.

X86  := x86_64-unknown-linux-musl
ARM  := aarch64-unknown-linux-musl
DIST := dist

.PHONY: all test dist src-dist clean

all: test dist

test:
	cargo test --release -- --test-threads=2

dist:
	cargo build --release --target $(X86)
	cargo build --release --target $(ARM)
	RUSTFLAGS="-C target-cpu=x86-64-v3" CARGO_TARGET_DIR=target/v3 cargo build --release --target $(X86)
	mkdir -p $(DIST)
	cp target/$(X86)/release/cpubench $(DIST)/cpubench-x86_64
	cp target/v3/$(X86)/release/cpubench $(DIST)/cpubench-x86_64-v3
	cp target/$(ARM)/release/cpubench $(DIST)/cpubench-aarch64
	cd $(DIST) && sha256sum cpubench-* > SHA256SUMS
	@ls -l $(DIST)

# Source package for building on the target OS itself (glibc or musl, your
# choice). Dependencies are vendored, so the target needs only a Rust toolchain
# (>= 1.85) and no network access.
src-dist:
	rm -rf target/srcpkg && mkdir -p target/srcpkg/cpubench $(DIST)
	cargo vendor --locked target/srcpkg/cpubench/vendor > /dev/null
	cp -r Cargo.toml Cargo.lock build.rs src Makefile README.md .cargo target/srcpkg/cpubench/
	printf '\n[source.crates-io]\nreplace-with = "vendored-sources"\n\n[source.vendored-sources]\ndirectory = "vendor"\n' >> target/srcpkg/cpubench/.cargo/config.toml
	tar -C target/srcpkg -czf $(DIST)/cpubench-src.tar.gz cpubench
	cd $(DIST) && sha256sum cpubench-* > SHA256SUMS
	@ls -l $(DIST)/cpubench-src.tar.gz

clean:
	cargo clean
	rm -rf $(DIST)
