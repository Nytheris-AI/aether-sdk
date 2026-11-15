# Contributing to Aether SDK

Thank you for considering contributing to **Aether SDK**!

## Development Setup

We use [`uv`](https://docs.astral.sh/uv/) for dependency management and Python 3.10+.

```bash
git clone https://github.com/Nytheris-AI/aether-sdk.git
cd aether-sdk
uv sync --group dev
```

## Running Checks

Before opening a pull request, please run the linter, type checker, and unit tests:

```bash
uv run ruff check src tests
uv run ruff format --check src tests
uv run mypy src
uv run pytest tests/unit
```

## Documentation

Documentation is built with Docusaurus from `docs/` and `website/`:

```bash
cd website
npm ci
npm run start
```

## Pull Requests

1. Fork the repository and create a feature branch from `main`.
2. Add or update tests for any new functionality or bug fixes.
3. Ensure all checks pass locally.
4. Open a pull request with a clear description of the problem and solution.
