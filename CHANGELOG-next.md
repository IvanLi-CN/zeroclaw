# ZeroClaw v0.8.6

ZeroClaw v0.8.6 fixes an authorization fallback in Telegram group handling so
ordinary messages in allowed groups remain silent when `mention_only` filters
them, instead of asking the sender to run `bind-telegram`.

## Highlights

- Keep unmentioned messages in exact allowed Telegram groups silent when
  `mention_only` is enabled; they no longer trigger a sender-level bind prompt.
- Preserve alias-scoped operator binding prompts for unbound private messages
  and the existing security behavior for unauthorized groups.
- Publish the IvanLi-CN release binary and container matrix with checksums,
  SBOMs, provenance attestations, and signed GHCR images.

## Reliability and Security

- Add listener-level regression coverage for Telegram allowed-group filtering,
  including the absence of dispatch, replies, typing, and reactions.
- Keep exact group allowlist boundaries and private-chat authorization semantics
  unchanged.

## Breaking Changes

No new breaking configuration changes are introduced by this fork release.
Telegram `allowed_groups` defaults to an empty list, preserving prior group-chat
behavior until an allowlist is configured.

## Full Changelog

**Full diff:** https://github.com/IvanLi-CN/zeroclaw/compare/v0.8.5...v0.8.6
