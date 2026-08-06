//! Policy-controlled Telegram sticker delivery for the active channel turn.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::json;
use sha2::{Digest, Sha256};
use zeroclaw_api::tool::{Tool, ToolOutput, ToolResult};
use zeroclaw_config::policy::{SecurityPolicy, ToolOperation};

const MAX_STICKERS_PER_TURN: usize = 3;
const STICKER_SET_CACHE_CAPACITY: usize = 32;
const TOOL_DESCRIPTION_KEY: &str = "tool-telegram-sticker-description";
const TOOL_EMOJI_PARAMETER_KEY: &str = "tool-telegram-sticker-param-emoji";

static TOOL_DESCRIPTION: OnceLock<String> = OnceLock::new();

fn tool_string(key: &str) -> String {
    crate::i18n::get_required_tool_string(key)
}

fn tool_string_with_args(key: &str, args: &[(&str, &str)]) -> String {
    crate::i18n::get_required_tool_string_with_args(key, args)
}

tokio::task_local! {
    /// The Telegram-only delivery target and success quota for one agent turn.
    pub static TURN_TELEGRAM_STICKER_CONTEXT: Option<TelegramStickerTurnContext>;
}

/// Per-turn Telegram state. It is scoped by the channel orchestrator and never
/// survives a turn, so a new turn starts with a fresh sticker quota.
#[derive(Clone)]
pub struct TelegramStickerTurnContext {
    alias: String,
    reply_target: String,
    successful_sends: Arc<tokio::sync::Mutex<usize>>,
}

impl TelegramStickerTurnContext {
    pub fn new(alias: impl Into<String>, reply_target: impl Into<String>) -> Self {
        Self {
            alias: alias.into(),
            reply_target: reply_target.into(),
            successful_sends: Arc::new(tokio::sync::Mutex::new(0)),
        }
    }

    pub fn alias(&self) -> &str {
        &self.alias
    }

    pub fn reply_target(&self) -> &str {
        &self.reply_target
    }

    pub async fn successful_sends(&self) -> usize {
        *self.successful_sends.lock().await
    }
}

/// Live Telegram configuration required to resolve and deliver a sticker.
/// The resolver supplies this at invocation time; the tool never owns a
/// configuration snapshot.
#[derive(Clone)]
pub struct TelegramStickerConfig {
    pub bot_token: String,
    pub api_base_url: String,
    pub proxy_url: Option<String>,
    pub sticker_sets: Vec<String>,
}

/// Resolves the globally configured sticker sets plus an active alias's Bot
/// API credentials from the current configuration.
pub type TelegramStickerConfigResolver =
    Arc<dyn Fn(&str) -> Option<TelegramStickerConfig> + Send + Sync>;

#[derive(Clone)]
struct ResolvedSticker {
    emoji: String,
    file_id: String,
}

/// Telegram file IDs are scoped to a bot, so metadata must never cross bot
/// credentials even when aliases share a sticker-set name. The cache retains a
/// fingerprint rather than a credential snapshot.
#[derive(Clone, Eq, Hash, PartialEq)]
struct StickerSetCacheKey {
    api_base_url: String,
    bot_token_fingerprint: String,
    set_name: String,
}

impl StickerSetCacheKey {
    fn from_config(config: &TelegramStickerConfig, set_name: &str) -> Self {
        Self {
            api_base_url: config.api_base_url.clone(),
            bot_token_fingerprint: format!("{:x}", Sha256::digest(config.bot_token.as_bytes())),
            set_name: set_name.to_string(),
        }
    }
}

#[derive(Default)]
struct StickerSetCache {
    entries: HashMap<StickerSetCacheKey, Vec<ResolvedSticker>>,
    recency: VecDeque<StickerSetCacheKey>,
}

impl StickerSetCache {
    fn get(&mut self, key: &StickerSetCacheKey) -> Option<Vec<ResolvedSticker>> {
        let stickers = self.entries.get(key).cloned()?;
        self.touch(key);
        Some(stickers)
    }

    fn insert(&mut self, key: StickerSetCacheKey, stickers: Vec<ResolvedSticker>) {
        if self.entries.contains_key(&key) {
            self.entries.insert(key.clone(), stickers);
            self.touch(&key);
            return;
        }

        if self.entries.len() == STICKER_SET_CACHE_CAPACITY
            && let Some(oldest) = self.recency.pop_front()
        {
            self.entries.remove(&oldest);
        }
        self.recency.push_back(key.clone());
        self.entries.insert(key, stickers);
    }

    fn touch(&mut self, key: &StickerSetCacheKey) {
        self.recency.retain(|candidate| candidate != key);
        self.recency.push_back(key.clone());
    }
}

/// Sends configured Telegram stickers to the active Telegram channel turn.
pub struct TelegramStickerTool {
    security: Arc<SecurityPolicy>,
    config_resolver: TelegramStickerConfigResolver,
    cache: Mutex<StickerSetCache>,
}

impl TelegramStickerTool {
    pub fn new(
        security: Arc<SecurityPolicy>,
        config_resolver: TelegramStickerConfigResolver,
    ) -> Self {
        Self {
            security,
            config_resolver,
            cache: Mutex::new(StickerSetCache::default()),
        }
    }

    fn api_url(config: &TelegramStickerConfig, method: &str) -> String {
        format!(
            "{}/bot{}/{}",
            config.api_base_url.trim_end_matches('/'),
            config.bot_token,
            method
        )
    }

    fn target_parts(reply_target: &str) -> (&str, Option<&str>) {
        reply_target
            .split_once(':')
            .map_or((reply_target, None), |(chat_id, thread_id)| {
                (chat_id, Some(thread_id))
            })
    }

    async fn telegram_api_call(
        &self,
        config: &TelegramStickerConfig,
        method: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let response = zeroclaw_config::schema::build_channel_proxy_client(
            "channel.telegram",
            config.proxy_url.as_deref(),
        )
        .post(Self::api_url(config, method))
        .json(&body)
        .send()
        .await
        .map_err(|_| {
            tool_string_with_args(
                "tool-telegram-sticker-error-transport",
                &[("method", method)],
            )
        })?;
        let status = response.status();
        let response_body = response.text().await.map_err(|_| {
            tool_string_with_args(
                "tool-telegram-sticker-error-response-read",
                &[("method", method)],
            )
        })?;
        if !status.is_success() {
            return Err(tool_string_with_args(
                "tool-telegram-sticker-error-http",
                &[("method", method), ("status", status.as_str())],
            ));
        }

        let response_json: serde_json::Value =
            serde_json::from_str(&response_body).map_err(|_| {
                tool_string_with_args(
                    "tool-telegram-sticker-error-response-decode",
                    &[("method", method)],
                )
            })?;
        if response_json.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
            return Err(tool_string_with_args(
                "tool-telegram-sticker-error-api",
                &[("method", method)],
            ));
        }
        Ok(response_json)
    }

    async fn sticker_set(
        &self,
        config: &TelegramStickerConfig,
        set_name: &str,
    ) -> Result<Vec<ResolvedSticker>, String> {
        let cache_key = StickerSetCacheKey::from_config(config, set_name);
        if let Some(stickers) = self.cache.lock().get(&cache_key) {
            return Ok(stickers);
        }

        let response = self
            .telegram_api_call(config, "getStickerSet", json!({"name": set_name}))
            .await?;
        let stickers = response
            .get("result")
            .and_then(|result| result.get("stickers"))
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| tool_string("tool-telegram-sticker-error-missing-metadata"))?
            .iter()
            .filter_map(|sticker| {
                let emoji = sticker.get("emoji")?.as_str()?.to_string();
                let file_id = sticker.get("file_id")?.as_str()?.to_string();
                (!emoji.is_empty() && !file_id.is_empty())
                    .then_some(ResolvedSticker { emoji, file_id })
            })
            .collect::<Vec<_>>();

        self.cache.lock().insert(cache_key, stickers.clone());
        Ok(stickers)
    }

    async fn resolve_sticker(
        &self,
        config: &TelegramStickerConfig,
        emoji: &str,
    ) -> Result<(String, ResolvedSticker), String> {
        for set_name in config.sticker_sets.iter().map(String::as_str) {
            let stickers = self.sticker_set(config, set_name).await?;
            if let Some(sticker) = stickers.into_iter().find(|sticker| sticker.emoji == emoji) {
                return Ok((set_name.to_string(), sticker));
            }
        }
        Err(tool_string_with_args(
            "tool-telegram-sticker-error-no-match",
            &[("emoji", emoji)],
        ))
    }
}

#[async_trait]
impl Tool for TelegramStickerTool {
    fn name(&self) -> &str {
        "sticker"
    }

    fn description(&self) -> &str {
        TOOL_DESCRIPTION
            .get_or_init(|| crate::i18n::get_required_tool_string(TOOL_DESCRIPTION_KEY))
            .as_str()
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "emoji": {
                    "type": "string",
                    "description": crate::i18n::get_required_tool_string(TOOL_EMOJI_PARAMETER_KEY),
                }
            },
            "required": ["emoji"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        if let Err(error) = self
            .security
            .enforce_tool_operation(ToolOperation::Act, self.name())
        {
            return Ok(ToolResult::err(tool_string_with_args(
                "tool-telegram-sticker-error-action-blocked",
                &[("error", &error)],
            )));
        }
        if args.get("file_id").is_some() {
            return Ok(ToolResult::err(tool_string(
                "tool-telegram-sticker-error-raw-file-id",
            )));
        }
        let Some(emoji) = args.get("emoji").and_then(serde_json::Value::as_str) else {
            return Ok(ToolResult::err(tool_string(
                "tool-telegram-sticker-error-missing-emoji",
            )));
        };
        if emoji.trim().is_empty() {
            return Ok(ToolResult::err(tool_string(
                "tool-telegram-sticker-error-empty-emoji",
            )));
        }
        let context = TURN_TELEGRAM_STICKER_CONTEXT
            .try_with(Clone::clone)
            .ok()
            .flatten();
        let Some(context) = context else {
            return Ok(ToolResult::err(tool_string(
                "tool-telegram-sticker-error-no-turn",
            )));
        };
        let Some(config) = (self.config_resolver)(context.alias()) else {
            return Ok(ToolResult::err(tool_string_with_args(
                "tool-telegram-sticker-error-alias-not-configured",
                &[("alias", context.alias())],
            )));
        };
        if config.bot_token.trim().is_empty() || config.api_base_url.trim().is_empty() {
            return Ok(ToolResult::err(tool_string(
                "tool-telegram-sticker-error-missing-credentials",
            )));
        }
        if config.sticker_sets.is_empty() {
            return Ok(ToolResult::err(tool_string(
                "tool-telegram-sticker-error-no-sets",
            )));
        }

        let (set_name, sticker) = match self.resolve_sticker(&config, emoji).await {
            Ok(sticker) => sticker,
            Err(error) => return Ok(ToolResult::err(error)),
        };

        // Hold the per-turn lock through the side effect: parallel tool calls
        // cannot pass the quota check concurrently and produce a fourth send.
        let mut successful_sends = context.successful_sends.lock().await;
        if *successful_sends >= MAX_STICKERS_PER_TURN {
            let limit = MAX_STICKERS_PER_TURN.to_string();
            return Ok(ToolResult::err(tool_string_with_args(
                "tool-telegram-sticker-error-quota",
                &[("limit", &limit)],
            )));
        }
        let (chat_id, thread_id) = Self::target_parts(context.reply_target());
        if chat_id.trim().is_empty() {
            return Ok(ToolResult::err(tool_string(
                "tool-telegram-sticker-error-empty-target",
            )));
        }
        let mut body = json!({
            "chat_id": chat_id,
            "sticker": sticker.file_id,
        });
        if let Some(thread_id) = thread_id.filter(|thread_id| !thread_id.is_empty()) {
            body["message_thread_id"] = serde_json::Value::String(thread_id.to_string());
        }
        if let Err(error) = self.telegram_api_call(&config, "sendSticker", body).await {
            return Ok(ToolResult::err(error));
        }
        *successful_sends += 1;

        Ok(ToolResult {
            success: true,
            output: ToolOutput::json(json!({
                "status": "sent",
                "emoji": emoji,
                "sticker_set": set_name,
                "successful_sends": *successful_sends,
            })),
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use wiremock::matchers::{body_json, method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use zeroclaw_api::tool::Tool;
    use zeroclaw_config::policy::{AutonomyLevel, SecurityPolicy};

    use super::*;

    fn config_resolver(api_base_url: String) -> TelegramStickerConfigResolver {
        Arc::new(move |alias| {
            (alias == "home").then(|| TelegramStickerConfig {
                bot_token: "test-token".into(),
                api_base_url: api_base_url.clone(),
                proxy_url: None,
                sticker_sets: vec!["mood_pack".into()],
            })
        })
    }

    #[tokio::test]
    async fn exact_emoji_from_configured_set_sends_sticker_without_exposing_file_id() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/bottest-token/getStickerSet$"))
            .and(body_json(json!({"name": "mood_pack"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "result": {"stickers": [{"emoji": "🔥", "file_id": "secret-file-id"}]},
            })))
            .expect(1)
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .and(path_regex(r"/bottest-token/sendSticker$"))
            .and(body_json(json!({
                "chat_id": "-100200300",
                "message_thread_id": "42",
                "sticker": "secret-file-id",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&mock_server)
            .await;

        let tool = TelegramStickerTool::new(
            Arc::new(SecurityPolicy::default()),
            config_resolver(mock_server.uri()),
        );
        let result = TURN_TELEGRAM_STICKER_CONTEXT
            .scope(
                Some(TelegramStickerTurnContext::new("home", "-100200300:42")),
                tool.execute(json!({"emoji": "🔥"})),
            )
            .await
            .expect("sticker execution returns a tool result");

        assert!(
            result.success,
            "exact configured emoji should send: {result:?}"
        );
        assert!(result.output.contains("mood_pack"));
        assert!(!result.output.contains("secret-file-id"));
    }

    #[tokio::test]
    async fn unmatched_or_raw_file_id_sticker_input_fails_without_sending() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/bottest-token/getStickerSet$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "result": {"stickers": [{"emoji": "🔥", "file_id": "secret-file-id"}]},
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let tool = TelegramStickerTool::new(
            Arc::new(SecurityPolicy::default()),
            config_resolver(mock_server.uri()),
        );
        let context = TelegramStickerTurnContext::new("home", "-100200300");
        let unmatched = TURN_TELEGRAM_STICKER_CONTEXT
            .scope(Some(context.clone()), tool.execute(json!({"emoji": "🔥️"})))
            .await
            .expect("unmatched sticker returns a tool result");
        assert!(!unmatched.success);
        assert!(unmatched.error.unwrap().contains("No configured"));

        let raw_id = TURN_TELEGRAM_STICKER_CONTEXT
            .scope(
                Some(context),
                tool.execute(json!({"emoji": "🔥", "file_id": "secret-file-id"})),
            )
            .await
            .expect("raw file_id rejection returns a tool result");
        assert!(!raw_id.success);
        assert!(raw_id.error.unwrap().contains("file_id"));
    }

    #[tokio::test]
    async fn fourth_successful_sticker_send_fails_and_cached_set_is_not_refetched() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/bottest-token/getStickerSet$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "result": {"stickers": [{"emoji": "🔥", "file_id": "secret-file-id"}]},
            })))
            .expect(1)
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .and(path_regex(r"/bottest-token/sendSticker$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(3)
            .mount(&mock_server)
            .await;

        let tool = TelegramStickerTool::new(
            Arc::new(SecurityPolicy::default()),
            config_resolver(mock_server.uri()),
        );
        let context = TelegramStickerTurnContext::new("home", "-100200300");
        for _ in 0..MAX_STICKERS_PER_TURN {
            let result = TURN_TELEGRAM_STICKER_CONTEXT
                .scope(Some(context.clone()), tool.execute(json!({"emoji": "🔥"})))
                .await
                .expect("sticker execution returns a tool result");
            assert!(
                result.success,
                "first three sends should succeed: {result:?}"
            );
        }

        let fourth = TURN_TELEGRAM_STICKER_CONTEXT
            .scope(Some(context.clone()), tool.execute(json!({"emoji": "🔥"})))
            .await
            .expect("quota rejection returns a tool result");
        assert!(!fourth.success);
        assert!(fourth.error.unwrap().contains("already sent"));
        assert_eq!(context.successful_sends().await, MAX_STICKERS_PER_TURN);
    }

    #[tokio::test]
    async fn telegram_api_failures_do_not_claim_sticker_delivery() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/bottest-token/getStickerSet$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "result": {"stickers": [{"emoji": "🔥", "file_id": "secret-file-id"}]},
            })))
            .expect(1)
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .and(path_regex(r"/bottest-token/sendSticker$"))
            .respond_with(ResponseTemplate::new(500).set_body_string("upstream unavailable"))
            .expect(1)
            .mount(&mock_server)
            .await;

        let tool = TelegramStickerTool::new(
            Arc::new(SecurityPolicy::default()),
            config_resolver(mock_server.uri()),
        );
        let context = TelegramStickerTurnContext::new("home", "-100200300");
        let result = TURN_TELEGRAM_STICKER_CONTEXT
            .scope(Some(context.clone()), tool.execute(json!({"emoji": "🔥"})))
            .await
            .expect("failed Telegram send returns a tool result");

        assert!(!result.success);
        assert!(result.error.unwrap().contains("sendSticker HTTP failure"));
        assert_eq!(context.successful_sends().await, 0);
    }

    #[tokio::test]
    async fn missing_sticker_pack_reports_api_failure_without_sending() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/bottest-token/getStickerSet$"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"ok": false, "description": "pack missing"})),
            )
            .expect(1)
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .and(path_regex(r"/bottest-token/sendSticker$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(0)
            .mount(&mock_server)
            .await;

        let tool = TelegramStickerTool::new(
            Arc::new(SecurityPolicy::default()),
            config_resolver(mock_server.uri()),
        );
        let context = TelegramStickerTurnContext::new("home", "-100200300");
        let result = TURN_TELEGRAM_STICKER_CONTEXT
            .scope(Some(context.clone()), tool.execute(json!({"emoji": "🔥"})))
            .await
            .expect("missing pack returns a tool result");

        assert!(!result.success);
        assert!(result.error.unwrap().contains("getStickerSet API failure"));
        assert_eq!(context.successful_sends().await, 0);
    }

    #[tokio::test]
    async fn telegram_sticker_requests_use_the_configured_channel_proxy() {
        let proxy_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_json(json!({"name": "mood_pack"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "result": {"stickers": []},
            })))
            .expect(1)
            .mount(&proxy_server)
            .await;
        let config = TelegramStickerConfig {
            bot_token: "test-token".into(),
            api_base_url: "http://unreachable.telegram.invalid".into(),
            proxy_url: Some(proxy_server.uri()),
            sticker_sets: vec!["mood_pack".into()],
        };
        let tool =
            TelegramStickerTool::new(Arc::new(SecurityPolicy::default()), Arc::new(|_| None));

        let response = tool
            .telegram_api_call(&config, "getStickerSet", json!({"name": "mood_pack"}))
            .await;

        assert!(
            response.is_ok(),
            "configured proxy should receive the request"
        );
    }

    #[tokio::test]
    async fn sticker_action_obeys_the_act_policy_gate() {
        let policy = SecurityPolicy {
            autonomy: AutonomyLevel::ReadOnly,
            ..SecurityPolicy::default()
        };
        let tool =
            TelegramStickerTool::new(Arc::new(policy), config_resolver("http://unused".into()));

        let result = tool
            .execute(json!({"emoji": "🔥"}))
            .await
            .expect("policy rejection returns a tool result");

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Action blocked"));
    }

    #[test]
    fn sticker_set_cache_is_bounded_and_refreshes_recency() {
        let mut cache = StickerSetCache::default();
        let cache_key = |set_name: String| StickerSetCacheKey {
            api_base_url: "https://api.telegram.org".into(),
            bot_token_fingerprint: "test-token-fingerprint".into(),
            set_name,
        };
        for index in 0..STICKER_SET_CACHE_CAPACITY {
            cache.insert(cache_key(format!("set-{index}")), Vec::new());
        }

        assert!(cache.get(&cache_key("set-0".into())).is_some());
        cache.insert(cache_key("newest".into()), Vec::new());

        assert_eq!(cache.entries.len(), STICKER_SET_CACHE_CAPACITY);
        assert!(cache.entries.contains_key(&cache_key("set-0".into())));
        assert!(!cache.entries.contains_key(&cache_key("set-1".into())));
        assert!(cache.entries.contains_key(&cache_key("newest".into())));
    }

    #[test]
    fn sticker_set_cache_does_not_share_file_ids_between_bots() {
        let mut cache = StickerSetCache::default();
        let first = TelegramStickerConfig {
            bot_token: "first-token".into(),
            api_base_url: "https://api.telegram.org".into(),
            proxy_url: None,
            sticker_sets: vec!["mood_pack".into()],
        };
        let second = TelegramStickerConfig {
            bot_token: "second-token".into(),
            ..first.clone()
        };
        let first_key = StickerSetCacheKey::from_config(&first, "mood_pack");
        let second_key = StickerSetCacheKey::from_config(&second, "mood_pack");

        cache.insert(first_key.clone(), Vec::new());

        assert!(cache.get(&first_key).is_some());
        assert!(cache.get(&second_key).is_none());
    }
}
