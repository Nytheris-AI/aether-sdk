# E2E Test Datasets for `aether-sdk`

This directory contains the collected datasets used to run end-to-end tests of `aether-sdk` across **data-source transformation**, **ingestion/incorporation**, and **memory lifecycle management**.

## 1. `locomo_sample_2conv.json`

- **Source:** [snap-research/locomo](https://github.com/snap-research/locomo) (`data/locomo10.json`), conversations `conv-26` and `conv-30`.
- **Paper:** *"Evaluating Very Long-Term Conversational Memory of LLM Agents"* (Maharana et al., ACL 2024).
- **Contents:**
  - `conv-26` (Caroline & Melanie): 19 sessions, 419 dialogue turns, 184 extracted speaker observations, 19 session event summaries, and 199 QA probes with `evidence` turn IDs (`dia_id`) across 5 categories (`1` = multi-hop, `2` = temporal, `3` = open-domain, `4` = single-hop, `5` = adversarial).
  - `conv-30` (Jon & Gina): 19 sessions, 369 dialogue turns, 169 extracted speaker observations, 19 session event summaries, and 105 QA probes.
- **What it tests in `aether-sdk`:**
  - **Transform:** Mapping multi-session raw dialogues, session timestamps, speaker entities, and observations into `MemoryInput` (`MemoryTier::Episodic` / `Semantic`, `Entity`, `Relation` with `valid_at`, `idempotency_key`, `metadata`, and deterministic embeddings).
  - **Incorporate:** Multi-tenant isolation (`conv-26` vs `conv-30` `ProjectId`s), buffered + flushed batch ingestion through `Traced<Resilient<Timeout<...>>>`, and idempotent re-ingestion.
  - **Manage:** `Vector` / `Graph` / `Hybrid` recall with `FilterExpr` metadata predicates (speaker, session, source_type) and QA evidence hit verification.

## 2. `knowledge_lifecycle.json`

- **Source:** Curated structured knowledge & lifecycle fixture (`aether-knowledge-lifecycle-v1`).
- **Contents:** Multi-tier (`semantic`, `procedural`, `episodic`), multi-scope (`project`, `session`) records with explicit entity-relation provenance, CAS `updates`, and `forget_targets`.
- **What it tests in `aether-sdk`:**
  - **Transform:** Mapping structured JSON records into `MemoryInput` with typed entities, temporal relations, and rich filterable metadata.
  - **Incorporate:** Atomic `put_if_absent` idempotency and `WaitFlush` durability.
  - **Manage:** `FilterExpr` (`Eq`, `In`, `Range`, `And`), `TemporalFilter` point-in-time graph queries, CAS `update()` with `expected_version` and `AetherError::Conflict` on stale versions, and `forget()` removing both vector records and graph provenance edges.
