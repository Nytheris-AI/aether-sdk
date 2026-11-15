# AGENTS.md

Repository-level engineering rules for AI agents and contributors working in **Aether SDK**.

---

## 1. Micro Commits

Keep every commit small, atomic, and focused on a single logical change so history stays reviewable, bisectable, and easy to revert.

### DO
- **DO** scope each commit to one logical unit (e.g., one trait/interface, one bug fix, one refactor, or one documentation page).
- **DO** use clear, conventional commit messages (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`) that explain *what* and *why*.
- **DO** ensure the project builds and all relevant tests/linters pass before creating each commit.
- **DO** commit structural refactors separately from behavioral/feature changes.

### DO NOT
- **DO NOT** bundle unrelated changes (e.g., formatting, dependency bumps, docs, and feature logic) into a single giant commit.
- **DO NOT** commit broken, half-finished, or non-compiling code to `main`.
- **DO NOT** commit generated build artifacts, caches, secrets, or `.env` files.
- **DO NOT** rewrite or squash shared history without explicit approval.

---

## 2. Test-Driven Development (TDD)

Follow the **Red → Green → Refactor** cycle for all behavioral code changes.

### DO
- **DO** write a failing test first that defines the expected behavior, edge cases, or bug reproduction (**Red**).
- **DO** run the test and verify it fails for the expected reason before writing implementation code.
- **DO** write the minimal, clean implementation needed to make the test pass (**Green**).
- **DO** refactor for clarity, modularity, and zero duplication while keeping all tests green (**Refactor**).
- **DO** test against core interfaces/traits (using in-memory mocks/fakes) for fast, deterministic unit tests before adding integration tests.
- **DO** cover error paths, boundary conditions, and invalid inputs—not just happy paths.

### DO NOT
- **DO NOT** write production logic before a failing test exists for it.
- **DO NOT** skip, disable, or weaken assertions to make a failing test pass.
- **DO NOT** rely on live external services (LLMs, Qdrant, Neo4j) in unit tests—mock/stub at the protocol/trait boundary.
- **DO NOT** claim tests pass without running them after your final edit and observing the output.
