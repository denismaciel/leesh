.PHONY: dev check test build release-check release-prepare

dev:
	uv sync --locked

check: dev
	uv run --no-sync ruff format --check .
	uv run --no-sync ruff check .
	uv run --no-sync ty check

test: dev
	uv run --no-sync pytest
	uv run --no-sync leesh check

build:
	uv build --no-sources

release-check: dev
	uv run --no-sync python scripts/release.py check

# RUN_ID is a successful Native release artifacts workflow run from this source.
release-prepare:
	uv run --no-project python scripts/release.py prepare $(RUN_ID)
