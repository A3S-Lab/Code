pub const PAGER_CLIENT_TYPE: &str = "grok-pager";
pub const HEADLESS_CLIENT_TYPE: &str = "grok-shell";

pub const PAGER_CLIENT_VERSION: &str = a3s_code_version::VERSION;

/// `User-Agent` for the pager's own HTTP clients that call `api.a3s` directly (voice STT).
///
/// Matches the sampler's `a3s-shell/<version> (os; arch)` shape so server-side dashboards bucket voice traffic alongside chat / imagine requests.
pub fn client_user_agent() -> String {
    format!(
        "{}/{} ({}; {})",
        HEADLESS_CLIENT_TYPE,
        PAGER_CLIENT_VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_user_agent_has_expected_shape() {
        // e.g. "a3s-shell/1.2.3 (macos; aarch64)".
        // Servers parse this UA string, so pin the exact shape
        let ua = client_user_agent();
        assert_eq!(
            ua,
            format!(
                "grok-shell/{} ({}; {})",
                PAGER_CLIENT_VERSION,
                std::env::consts::OS,
                std::env::consts::ARCH
            )
        );
    }
}
