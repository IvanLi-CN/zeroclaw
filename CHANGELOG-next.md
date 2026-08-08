# ZeroClaw v0.8.5

ZeroClaw v0.8.5 adds Telegram group interaction controls and incorporates the
latest reliability, security, provider, channel, installer, and CI improvements
from the upstream project since v0.8.4.

## Highlights

- Authorize Telegram group and supergroup chats by exact chat ID without
  changing the existing private-chat binding flow.
- Add Telegram reaction parity and a policy-governed sticker tool backed by
  configured sticker sets, emoji metadata, and a three-sticker turn limit.
- Keep unauthorized group updates silent before reactions, downloads, pairing,
  or model execution.
- Publish the IvanLi-CN release binary and container matrix with checksums,
  SBOMs, provenance attestations, and signed GHCR images.

## Reliability and Security

- Validate initiative child pull requests and aggregate required CI correctly.
- Include upstream fixes for atomic session rewrites, serialized configuration
  writes, provider streaming timeouts, sandbox working directories, dependency
  advisories, and authenticated channel ingress.

## Breaking Changes

No new breaking configuration changes are introduced by this fork release.
Telegram `allowed_groups` defaults to an empty list, preserving prior group-chat
behavior until an allowlist is configured.

## Full Changelog

**Full diff:** https://github.com/IvanLi-CN/zeroclaw/compare/v0.8.4...v0.8.5
