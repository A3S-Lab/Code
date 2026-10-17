//! CORE-VOCAB1: "is the run done" has one authority.
//!
//! Only `harness_loop` constructs a passing completion verdict
//! (`CompletionGate::Allow`). Planning and goal control supply evidence:
//! `GoalAchieved` is emitted only by the planning runtime, after the plan
//! returned through the gate, and the evidence check
//! `should_emit_goal_achieved*` is asked only where a gate verdict already
//! exists. Everything else may match on
//! a verdict but not create one. Test-only code is not scanned.

#[path = "support/production_graph.rs"]
mod production_graph;

use syn::visit::Visit;

const GATE_OWNERS: &[&str] = &["core/src/harness_loop.rs", "core/src/harness_loop/"];
const GOAL_EVENT_OWNERS: &[&str] = &[
    "core/src/agent/planning_runtime.rs",
    // Redaction rebuilds every event variant with scrubbed fields.
    "core/src/security/mod.rs",
];
const GOAL_EVIDENCE_CALLERS: &[&str] = &[
    "core/src/agent/planning_runtime.rs",
    // Durable goal evidence asks only after `decide_completion` allowed.
    "core/src/fact_control/goal_evidence.rs",
    // The thin `should_emit_goal_achieved` wrapper.
    "core/src/verification.rs",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Site {
    GateAllow,
    GoalAchievedEvent,
    GoalEvidenceCall,
}

#[derive(Default)]
struct Sites {
    found: Vec<Site>,
}

fn ends_with(path: &syn::Path, tail: &[&str]) -> bool {
    let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
    segments.len() >= tail.len()
        && segments[segments.len() - tail.len()..]
            .iter()
            .zip(tail)
            .all(|(segment, expected)| segment == expected)
}

fn is_test_fn(attrs: &[syn::Attribute]) -> bool {
    production_graph::is_cfg_test(attrs)
        || attrs.iter().any(|attr| {
            attr.path()
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "test")
        })
}

impl<'ast> Visit<'ast> for Sites {
    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        if !production_graph::is_cfg_test(&module.attrs) {
            syn::visit::visit_item_mod(self, module);
        }
    }

    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        if !is_test_fn(&function.attrs) {
            syn::visit::visit_item_fn(self, function);
        }
    }

    fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
        if !is_test_fn(&function.attrs) {
            syn::visit::visit_impl_item_fn(self, function);
        }
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = call.func.as_ref() {
            if ends_with(&path.path, &["CompletionGate", "Allow"]) {
                self.found.push(Site::GateAllow);
            }
            let last = path.path.segments.last().map(|s| s.ident.to_string());
            if matches!(
                last.as_deref(),
                Some("should_emit_goal_achieved" | "should_emit_goal_achieved_for_workspace")
            ) {
                self.found.push(Site::GoalEvidenceCall);
            }
        }
        syn::visit::visit_expr_call(self, call);
    }

    fn visit_expr_struct(&mut self, expr: &'ast syn::ExprStruct) {
        if ends_with(&expr.path, &["AgentEvent", "GoalAchieved"]) {
            self.found.push(Site::GoalAchievedEvent);
        }
        syn::visit::visit_expr_struct(self, expr);
    }
}

fn sites(syntax: &syn::File) -> Vec<Site> {
    let mut sites = Sites::default();
    sites.visit_file(syntax);
    sites.found
}

fn owned_by(path: &str, owners: &[&str]) -> bool {
    owners
        .iter()
        .any(|owner| path == *owner || owner.ends_with('/') && path.starts_with(owner))
}

#[test]
fn only_the_harness_loop_gate_decides_that_a_run_is_complete() {
    let mut problems = Vec::new();
    let mut gate_sites = 0;
    let mut goal_sites = 0;
    for (path, file) in production_graph::production_files() {
        for site in sites(&file.syntax) {
            let (owners, what) = match site {
                Site::GateAllow => {
                    gate_sites += 1;
                    (GATE_OWNERS, "constructs CompletionGate::Allow")
                }
                Site::GoalAchievedEvent => {
                    goal_sites += 1;
                    (GOAL_EVENT_OWNERS, "emits AgentEvent::GoalAchieved")
                }
                Site::GoalEvidenceCall => (GOAL_EVIDENCE_CALLERS, "asks should_emit_goal_achieved"),
            };
            if !owned_by(&path, owners) {
                problems.push(format!("{path} {what}; only {owners:?} may"));
            }
        }
    }
    assert!(gate_sites > 0, "the scan found no gate verdict at all");
    assert!(
        goal_sites > 0,
        "the scan found no GoalAchieved emission at all"
    );
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn the_scanner_sees_constructions_but_not_patterns_or_tests() {
    let syntax: syn::File = syn::parse_quote! {
        fn decide() -> CompletionGate {
            crate::harness_loop::CompletionGate::Allow(CompletionTerminal::Narrative)
        }
        fn read(gate: CompletionGate) -> bool {
            matches!(gate, CompletionGate::Allow(_))
        }
        fn emit() -> AgentEvent {
            let _ = verification::should_emit_goal_achieved(true, &[]);
            AgentEvent::GoalAchieved { goal: String::new(), total_steps: 0, duration_ms: 0 }
        }
        #[test]
        fn in_a_test() {
            let _ = CompletionGate::Allow(CompletionTerminal::Narrative);
        }
        #[cfg(test)]
        mod tests {
            fn helper() {
                let _ = CompletionGate::Allow(CompletionTerminal::Narrative);
            }
        }
    };
    assert_eq!(
        sites(&syntax),
        [
            Site::GateAllow,
            Site::GoalEvidenceCall,
            Site::GoalAchievedEvent
        ]
    );
    assert!(owned_by("core/src/harness_loop/receipt.rs", GATE_OWNERS));
    assert!(!owned_by("core/src/harness_loop_extra.rs", GATE_OWNERS));
}
