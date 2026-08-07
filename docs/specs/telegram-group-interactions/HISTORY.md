# Telegram Group Interactions History

> This file records durable design decisions. The current behavior contract
> remains in `./SPEC.md`.

## Decision Trace

### 2026-08-05: Establish the Telegram interaction boundary

- Group authorization is based on exact Telegram numeric chat IDs and permits
  all senders inside an authorized group.
- Direct messages retain sender-level authorization and `/bind`; authorization
  never propagates from a group to a direct message.
- Empty `allowed_groups` preserves existing behavior to avoid an upgrade-time
  breaking change.
- Reactions reuse the cross-channel reaction capability and must report
  Telegram API failures truthfully.
- Stickers use configured set names and Telegram emoji metadata rather than
  durable file IDs or hand-maintained labels.
- Sticker sends are action operations limited to three successful calls per
  turn, with free ordering relative to ordinary text.
- The initiative is developed and reviewed entirely in `IvanLi-CN/zeroclaw`.
  The final target is that repository's `master` branch.

## Key Reasons

- Chat-scoped authorization prevents a group allowlist from becoming a hidden
  user identity grant across Telegram conversations.
- Early rejection prevents unauthorized messages from causing observable or
  costly side effects.
- Typed sticker selection keeps model-controlled input within configured,
  auditable bounds.
- Non-persistent metadata caching avoids creating a second source of truth.

## References

- `./SPEC.md`
- `./IMPLEMENTATION.md`
