## What and why

## Verification

- [ ] `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
- [ ] `pnpm build && pnpm typecheck && pnpm test`
- [ ] A test covers the change; resolution changes were checked against `tsc --listFilesOnly`
- [ ] `CHANGELOG.md` updated
