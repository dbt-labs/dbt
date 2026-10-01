DOC_FEATURES=loader,json,urlencode,custom_syntax,fuel
TEST_FEATURES=unstable_machinery,builtins,loader,json,urlencode,debug,internal_debug,macros,multi_template,adjacent_loop_items,custom_syntax,deserialization,serde,loop_controls

.PHONY: all
all: test

.PHONY: build
build:
	@cargo build --all

.PHONY: doc
doc:
	@cd minijinja; RUSTC_BOOTSTRAP=1 RUSTDOCFLAGS="--cfg=docsrs --html-in-header doc-header.html" cargo doc -p minijinja -p minijinja-autoreload -p minijinja-contrib --no-deps --features=$(DOC_FEATURES)

.PHONY: test-msrv
test-msrv:
	@$(MAKE) run-tests FEATURES=$(TEST_FEATURES)
	@$(MAKE) run-tests FEATURES=$(TEST_FEATURES),preserve_order,key_interning,unicode
	@echo "CARGO TEST ALL FEATURES"
	@cd minijinja; cargo test --all-features

.PHONY: test
test: test-msrv test-cli
	@echo "CARGO TEST MINIJINJA-CONTRIB ALL FEATURES"
	@cd minijinja-contrib; cargo test --all-features

.PHONY: wasi-test
wasi-test:
	@cd minijinja; cargo test --all-features --target=wasm32-wasi -- --nocapture

.PHONY: python-test
python-test:
	@make -C minijinja-py

.PHONY: python-type-check
python-type-check:
	@make -C minijinja-py type-check

.PHONY: snapshot-tests
snapshot-tests:
	@cd minijinja; cargo insta test --all-features --review

.PHONY: run-tests
run-tests:
	@rustup component add rustfmt 2> /dev/null
	@echo "CARGO TESTS"
	@cd minijinja; cargo test --features=json,urlencode,internal_debug,loop_controls
	@echo "CARGO TEST SPEEDUPS"
	@cd minijinja; cargo test --no-default-features --features=speedups,$(TEST_FEATURES)
	@echo "CARGO CHECK NO_DEFAULT_FEATURES"
	@cd minijinja; cargo check --no-default-features --features=debug
	@cd minijinja-autoreload; cargo test
	@cd minijinja-contrib; cargo test

.PHONY: test-cli
test-cli:
	@cd minijinja-cli; cargo test

.PHONY: check
check:
	@echo "check no default features:"
	@cd minijinja; cargo check --no-default-features
	@echo "check all features:"
	@cd minijinja; cargo check --all-features
	@echo "check custom-delimiters:"
	@cd minijinja; cargo check --features=custom_syntax
	@echo "check custom-delimiters+loader:"
	@cd minijinja; cargo check --features=custom_syntax,loader
	@echo "check loader:"
	@cd minijinja; cargo check --features=loader
	@echo "check macro only:"
	@cd minijinja; cargo check --no-default-features --features macros
	@echo "check multi_template only:"
	@cd minijinja; cargo check --no-default-features --features multi_template
	@echo "check minijinja-autoreload:"
	@cd minijinja-autoreload; cargo check
	@cd minijinja-autoreload; cargo check --no-default-features
	@echo "check minijinja-contrib:"
	@cd minijinja-contrib; cargo check
	@cd minijinja-contrib; cargo check --all-features
	@cd minijinja-contrib; cargo check --no-default-features

.PHONY:
check-cli:
	@cd minijinja-cli; cargo check --no-default-features
	@cd minijinja-cli; cargo check

.PHONY: format
format:
	@rustup component add rustfmt 2> /dev/null
	@cargo fmt --all

.PHONY: format-check
format-check:
	@rustup component add rustfmt 2> /dev/null
	@cargo fmt --all -- --check

.PHONY: lint
lint:
	@rustup component add clippy 2> /dev/null
	@cargo clippy --all -- -F clippy::dbg-macro -D warnings
