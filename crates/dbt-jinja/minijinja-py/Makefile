.PHONY: all
all: develop test

.venv:
	python3 -mvenv .venv
	.venv/bin/pip install --upgrade pip
	.venv/bin/pip install maturin pytest markupsafe black pyright

.PHONY: test
develop: .venv
	.venv/bin/maturin develop

.PHONY: develop-release
develop-release: .venv
	.venv/bin/maturin develop --release

.PHONY: test
test: .venv
	.venv/bin/pytest

.PHONY: format
format: .venv
	.venv/bin/black tests python

.PHONY: type-check
type-check: .venv
	.venv/bin/pyright python
