# Aether SDK Examples

This directory contains examples demonstrating how to use the `aether-sdk`.

## Quickstart

Run a full end-to-end memory cycle (Ingest -> Extract -> Recall) using:

```bash
uv run examples/quickstart.py
```

This example uses:
*   **Mock Adapters**: To simulate LLM and Vector Store without real API keys.
*   **In-Memory**: No external databases required.

## Adapters

The SDK is designed to be **Protocol-First**. You bring your own database drivers.
See `quickstart.py` for how `QdrantAdapter` is passed to the `MemoryManager`.

## Directories
*   `quickstart.py`: The main entry point for learning the SDK.
