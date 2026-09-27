use agent_client_protocol as acp;

use a3s_code_login::{AuthManager, GrokAuth};

/// Require A3S auth from a sync context: with no `.await` to refresh, a token inside the client's early-invalidation buffer still counts.
pub(crate) fn require_a3s_auth(
    auth_manager: &AuthManager,
    missing_message: &'static str,
    non_a3s_message: &'static str,
) -> Result<GrokAuth, acp::Error> {
    let auth = auth_manager
        .current_or_expired()
        .ok_or_else(|| acp::Error::auth_required().data(missing_message))?;
    if !auth.is_a3s_auth() {
        return Err(acp::Error::auth_required().data(non_a3s_message));
    }
    Ok(auth)
}
