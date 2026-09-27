//! Thin hub help for `/ctx` and `/use` until full panels are ported.

/// Help text when `/ctx` is invoked with no args.
pub fn ctx_hub_help() -> &'static str {
    "context hub · /ctx <query> · /ctx memory|kb|sleep|evolution · /ctx <n> · /ctx save <n>"
}

/// Help text when `/use` is invoked with no args.
pub fn use_hub_help() -> &'static str {
    "integrations · /use status|repair|plugin|packages|reload"
}
