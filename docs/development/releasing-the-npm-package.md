# Releasing the npm package

`@muninn-org/aether-mcp` lets people run `aether-mcp` with `npx`.
The package is a small launcher in `npm/aether-mcp/`. It downloads the matching `aether-mcp` release binary from GitHub Releases, checks its SHA-256 checksum, and runs it.

It is published by CI, and only when the `release` branch is updated.
A push to `main` never publishes.

## Release steps

1. Cut an `aether-mcp` release by pushing a `vX.Y.Z` tag. Wait for the release workflow to finish. The release must contain an archive and a checksum for every supported platform.
2. Open a pull request that sets `aetherMcpVersion` in `npm/aether-mcp/package.json` to that version. Bump the package `version` too, because npm never accepts the same version twice.
3. Merge it, then publish by moving the `release` branch to the commit you want:

   ```bash
   git push origin <commit>:release
   ```

4. Watch the **Publish npm package** workflow. It runs the launcher tests on Linux, macOS, and Windows, checks that the release has every build, and publishes with provenance.

If the package version is already on npm, the workflow publishes nothing.
If the release is missing a build, the workflow fails before publishing.

## One-time setup

- The package is published under the `muninn-org` organization on npm, so its name is `@muninn-org/aether-mcp`. The publishing account must be a member that can publish to it.
- Create an npm access token that can publish `@muninn-org/aether-mcp` and save it as the repository secret `NPM_TOKEN`. If the npm account requires a one-time code to publish, the token must be allowed to bypass that, or the publish step fails in CI.
- Optional: add required reviewers to the `npm` environment in the repository settings, so each publish needs an approval.
- Optional: protect the `release` branch so only maintainers can push to it.

## Settings users can change

| Environment variable | Purpose |
| --- | --- |
| `AETHER_MCP_VERSION` | Use a different `aether-mcp` release than the one the package was tested with |
| `AETHER_MCP_CACHE_DIR` | Where downloaded binaries are kept |
| `AETHER_MCP_DOWNLOAD_BASE` | Download releases from a mirror instead of GitHub |
