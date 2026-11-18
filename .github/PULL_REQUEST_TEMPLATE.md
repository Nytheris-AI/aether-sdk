## Summary

Describe what this pull request changes and why.

## Type of Change

- [ ] Bug fix (non-breaking change that fixes an issue)
- [ ] New feature (non-breaking change that adds functionality)
- [ ] Breaking change (fix or feature that changes existing public behavior)
- [ ] Documentation or website update
- [ ] Refactor / chore (no functional change)

## Verification

Run the workspace gate locally before opening the PR:

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] `cargo test --workspace --all-features`
- [ ] `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps`
- [ ] `cd website && npm run typecheck && npm run build` (if docs or `website/` changed)

Paste or summarize the verification output here.
