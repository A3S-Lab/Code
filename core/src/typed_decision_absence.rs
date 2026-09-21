//! TD-A absence checks. The inventory may name typed decisions while the
//! thin feature set still does not enable Apofasi.

#[cfg(test)]
mod tests {
    use crate::sdk_capabilities::{sdk_capabilities, CapabilityTier};

    #[test]
    fn thin_profiles_do_not_enable_apofasi() {
        let manifest = include_str!("../Cargo.toml");
        for key in ["local-code =", "scientific =", "full ="] {
            let line = manifest
                .lines()
                .find(|line| line.trim_start().starts_with(key))
                .unwrap_or_else(|| panic!("missing feature line {key}"));
            assert!(
                !line.contains("apofasi"),
                "{key} must not enable Apofasi: {line}"
            );
        }
    }

    #[test]
    fn inventory_still_lists_typed_decisions_when_feature_is_off() {
        let capability = sdk_capabilities()
            .into_iter()
            .find(|item| item.id == "typed_decisions")
            .expect("typed_decisions");
        assert_eq!(capability.tier, CapabilityTier::Advanced);
        assert!(capability.host_owned);
        assert!(capability
            .operations
            .iter()
            .any(|operation| operation == "typed_decision.decide"));
    }
}
