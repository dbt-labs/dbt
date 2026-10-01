
.PHONY: fuzz-add-template
fuzz-add-template:
	@mkdir -p corpus/add_template
	@RUSTC_BOOTSTRAP=1 cargo fuzz run --release --jobs 8 add_template corpus/add_template seeds/add_template -- -dict=dict

.PHONY: fuzz-render
fuzz-render:
	@mkdir -p corpus/render
	@RUSTC_BOOTSTRAP=1 cargo fuzz run --release --jobs 8 render corpus/render seeds/render -- -dict=dict

.PHONY: clean-corpus
clean-corpus:
	@rm corpus/*/*

.PHONY: repro
repro:
	@if [ "x$(ARTIFACT)" == x ]; then echo 'no artifact specified, run `make repro ARTIFACT=artifacts/x/y`'; exit 1; fi; \
		TARGET=$$(echo '$(ARTIFACT)' | perl -p -e 's/^.*\bartifacts\/(.*?)\/.*$$/\1/'); RUSTC_BOOTSTRAP=1 cargo fuzz run $$TARGET "$(ARTIFACT)"
