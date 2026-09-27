use super::*;
use crate::permission::auto_mode::ClassifierContext;
use crate::permission::auto_mode::ClassifierSecurityFinding::{UnresolvedArgument, UnvettedEnv};
use crate::permission::claude_settings::load_claude_settings;
use crate::permission::reasons::SESSION_GRANT;
use crate::permission::rules::parse_permission_rule;
use crate::permission::state::load_state_from_disk;
use crate::permission::types::{PermissionConfig, RuleAction};

const LOG_SCRIPT: &str = r#"LOG="/tmp/grok-permission-fixture/sim.log"
echo "===== exists ====="
ls -lh "$LOG"
echo "===== errors ====="
rg -n "ERROR|FATAL" "$LOG" | rg -v "summary|counts" | head -40
echo "===== summary ====="
rg -n "Summary|ERROR :|FATAL :" "$LOG" | tail -15"#;

const SETTINGS: &str = r#"{"permissions":{
    "allow":["Bash","Read(*)","Search(*)"],
    "ask":["Bash(rm *)","Bash(*rm -rf*)"]
}}"#;

const PAGER: ClientType = ClientType::GrokPager;
const REMOVES: &str = r#"LOG=/x; ls "$LOG"; rm "$LOG""#;
const WRAPPED_REMOVES: &str = r#"LOG=/x; ls "$LOG"; timeout 5 rm "$LOG""#;
/// `rg:*` covers neither `ls` nor `echo`, so no configured allow decides LOG_SCRIPT.
const PARTIAL: [(&str, RuleAction); 1] = [("Bash(rg:*)", RuleAction::Allow)];

struct FilenameFixture {
    directory: tempfile::TempDir,
    cwd: AbsPathBuf,
    manager: PermissionHandle,
    events: mpsc::UnboundedReceiver<PermissionEvent>,
    prompts: std::rc::Rc<std::cell::RefCell<Vec<acp::RequestPermissionRequest>>>,
    classifications: Arc<std::sync::Mutex<Vec<ClassifierContext>>>,
}

impl FilenameFixture {
    async fn new(
        mode: PromptPolicy,
        verdict: ClassifierVerdict,
        state: PermissionState,
    ) -> FilenameFixture {
        let directory = tempfile::tempdir().expect("fixture directory");
        let settings_path = directory.path().join("settings.json");
        std::fs::write(&settings_path, SETTINGS).expect("write settings");
        let settings = load_claude_settings(&settings_path).expect("load settings");
        let (mut config, warnings) = settings
            .permissions
            .expect("permissions")
            .into_permission_config();
        assert_eq!(1, warnings.len(), "only the Search rule is skipped");
        assert!(
            warnings
                .first()
                .expect("skipped rule")
                .contains("Search(*)")
        );
        assert_eq!(4, config.rules.len());
        config.prompt_policy = mode;
        let cwd = AbsPathBuf::new(directory.path().to_path_buf()).expect("absolute fixture cwd");
        persist_state(&cwd, &state, None).await;
        let client = RecordingClient::default();
        let prompts = client.prompts.clone();
        let (manager, events) = manager_with_recording_client_remember(
            &cwd,
            Some(config),
            client,
            ClientType::GrokPager,
            /*remember_tool_approvals*/ true,
        );
        let (classifier, classifications) = capturing_classifier(verdict);
        manager.set_classifier(Some(classifier));
        FilenameFixture {
            directory,
            cwd,
            manager,
            events,
            prompts,
            classifications,
        }
    }

    /// Explicit rules; the reported settings stay on disk only as the unchanged-file sentinel.
    async fn with_rules(
        mode: PromptPolicy,
        rules: &[(&str, RuleAction)],
        client_type: ClientType,
        state: PermissionState,
    ) -> FilenameFixture {
        let rules = rules
            .iter()
            .map(|(rule, action)| parse_permission_rule(rule, *action).expect("rule"));
        let mut config = PermissionConfig::new(rules.collect());
        config.prompt_policy = mode;
        let directory = tempfile::tempdir().expect("fixture directory");
        std::fs::write(directory.path().join("settings.json"), SETTINGS).expect("write settings");
        let cwd = AbsPathBuf::new(directory.path().to_path_buf()).expect("absolute fixture cwd");
        persist_state(&cwd, &state, None).await;
        let client = RecordingClient::default();
        let prompts = client.prompts.clone();
        let (manager, events) =
            manager_with_recording_client_remember(&cwd, Some(config), client, client_type, true);
        let mut fixture = FilenameFixture {
            directory,
            cwd,
            manager,
            events,
            prompts,
            classifications: Arc::default(),
        };
        fixture.classify(ClassifierVerdict::Allow);
        fixture
    }

    fn classify(&mut self, verdict: ClassifierVerdict) {
        let (classifier, classifications) = capturing_classifier(verdict);
        self.manager.set_classifier(Some(classifier));
        self.classifications = classifications;
    }

    fn classified(&self) -> usize {
        self.classifications.lock().expect("classifications").len()
    }

    async fn bash(&mut self, command: &str) -> (Decision, PermissionEvent) {
        self.request(AccessKind::Bash(command.to_owned())).await
    }

    async fn request(&mut self, access: AccessKind) -> (Decision, PermissionEvent) {
        let before = load_state_from_disk(&self.cwd, None).await;
        let mode = self.manager.is_auto_mode();
        let decision = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            decide(&self.manager, access, tool_call()),
        )
        .await
        .expect("permission decision");
        let event = self.events.try_recv().expect("permission event");
        assert!(!self.manager.is_yolo_mode());
        assert!(!event.yolo_mode);
        assert_eq!(mode, self.manager.is_auto_mode());
        assert_eq!(Some(true), event.remember_tool_approvals);
        assert_eq!(
            serde_json::to_value(before).expect("serialize grants"),
            serde_json::to_value(load_state_from_disk(&self.cwd, None).await)
                .expect("serialize grants"),
        );
        assert_eq!(
            SETTINGS,
            std::fs::read_to_string(self.directory.path().join("settings.json"))
                .expect("settings unchanged"),
        );
        (decision, event)
    }
}

#[tokio::test]
async fn configured_log_script_allows_without_mutating_grants() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let default = PermissionState::default;
            let mut fixture =
                FilenameFixture::new(PromptPolicy::Ask, ClassifierVerdict::Allow, default()).await;
            let (decision, event) = fixture
                .request(AccessKind::Bash("rg -n ERROR /tmp/sim.log".to_owned()))
                .await;
            assert_eq!(Decision::Allow, decision);
            assert!(!event.user_prompted);
            let mut auto =
                FilenameFixture::new(PromptPolicy::Auto, ClassifierVerdict::Block, default()).await;
            let mut toggled =
                FilenameFixture::new(PromptPolicy::Auto, ClassifierVerdict::Block, default()).await;
            toggled.manager.set_auto_mode(false);
            assert!(auto.manager.is_auto_mode() && !fixture.manager.is_auto_mode());
            assert!(!toggled.manager.is_auto_mode());
            for fixture in [&mut fixture, &mut auto, &mut toggled] {
                let (decision, event) = fixture.bash(LOG_SCRIPT).await;
                let reason = event.decision_reason.as_deref();
                let mode = if fixture.manager.is_auto_mode() {
                    "auto"
                } else {
                    "ask"
                };
                assert_eq!(Decision::Allow, decision);
                assert_eq!(Some(reasons::POLICY_ALLOW), reason);
                assert_eq!(Some(mode), event.permission_mode.as_deref());
                assert!(!event.user_prompted);
                assert_eq!(0, fixture.classified());
            }
            for (access, reason) in [
                (
                    AccessKind::Bash(REMOVES.to_owned()),
                    reasons::BASH_COMMAND_GATE_ASK,
                ),
                (
                    AccessKind::Bash("rm /tmp/victim".to_owned()),
                    reasons::POLICY_ASK,
                ),
                (
                    AccessKind::Bash("echo 'rm -rf'".to_owned()),
                    reasons::POLICY_ASK,
                ),
                (AccessKind::Edit("victim".to_owned()), reasons::NEEDS_USER),
            ] {
                let count = fixture.prompts.borrow().len();
                let (decision, event) = fixture.request(access).await;
                assert_eq!(
                    Decision::Reject("User rejected the execution".to_owned()),
                    decision,
                );
                assert_eq!(Some(reason), event.decision_reason.as_deref());
                assert_eq!(Some("ask"), event.permission_mode.as_deref());
                assert!(event.user_prompted);
                assert_eq!(count + 1, fixture.prompts.borrow().len());
            }
            assert!(
                fixture
                    .classifications
                    .lock()
                    .expect("classifications")
                    .is_empty()
            );
        })
        .await;
}

#[tokio::test]
async fn uncovered_log_script_auto_classifies_unless_exactly_granted() {
    use crate::permission::auto_mode::{
        ClassifierFailure, ClassifierMessage, ClassifierPromptType, HeuristicPermissionClassifier,
        LlmPermissionClassifier,
    };
    tokio::task::LocalSet::new()
        .run_until(async {
            let findings: BashSecurityAssessment =
                [UnresolvedArgument, UnvettedEnv].into_iter().collect();
            let grants = ["none", "exact", "prefix", "glob", "blanket"];
            let outcomes = [
                (ClassifierVerdict::Allow, reasons::AUTO_CLASSIFIER_ALLOW),
                (ClassifierVerdict::Block, reasons::AUTO_CLASSIFIER_DENY),
                (
                    ClassifierVerdict::Unavailable,
                    reasons::AUTO_CLASSIFIER_UNAVAILABLE,
                ),
                (
                    ClassifierVerdict::Unavailable,
                    reasons::AUTO_CLASSIFIER_TIMEOUT,
                ),
            ];
            for (grant, (verdict, reason)) in grants
                .into_iter()
                .flat_map(|grant| outcomes.map(|outcome| (grant, outcome)))
            {
                let timeout = reason == reasons::AUTO_CLASSIFIER_TIMEOUT;
                let state = PermissionState {
                    allow_bash_execute: grant == "blanket",
                    allowed_bash_globs: (grant == "glob")
                        .then(|| "*".to_owned())
                        .into_iter()
                        .collect(),
                    allowed_bash_commands: match grant {
                        "exact" => HashSet::from([LOG_SCRIPT.to_owned()]),
                        "prefix" => HashSet::from(["ls".to_owned()]),
                        _ => HashSet::new(),
                    },
                    ..PermissionState::default()
                };
                let mut fixture =
                    FilenameFixture::with_rules(PromptPolicy::Auto, &PARTIAL, PAGER, state).await;
                fixture.classify(verdict);
                if timeout {
                    fixture
                        .manager
                        .set_classifier(Some(Arc::new(LlmPermissionClassifier {
                            classify_text: Some(Arc::new(|_: Vec<ClassifierMessage>| {
                                Box::pin(async { Err(ClassifierFailure::Timeout) })
                            })),
                            classify_channel: None,
                            fallback: HeuristicPermissionClassifier,
                            prompt_type: ClassifierPromptType::Full,
                        })));
                }
                let (decision, event) = fixture
                    .request(AccessKind::Bash(LOG_SCRIPT.to_owned()))
                    .await;
                let exact = grant == "exact";
                let allows = exact || verdict == ClassifierVerdict::Allow;
                let expected = if allows {
                    Decision::Allow
                } else {
                    Decision::Reject("User rejected the execution".to_owned())
                };
                let reason = if exact { SESSION_GRANT } else { reason };
                assert_eq!(expected, decision, "{grant} {reason}");
                assert!(fixture.manager.is_auto_mode());
                assert_eq!(Some(reason), event.decision_reason.as_deref());
                assert_eq!(Some("auto"), event.permission_mode.as_deref());
                assert_eq!(!allows, event.user_prompted);
                assert_eq!(usize::from(!allows), fixture.prompts.borrow().len());
                assert_eq!((!exact).then(|| findings.tokens()), event.security_findings);
                assert_eq!(usize::from(!exact && !timeout), fixture.classified());
                assert_eq!(Some(LOG_SCRIPT), event.access_detail.as_deref());
            }
        })
        .await;
}

#[tokio::test]
async fn recovered_scripts_keep_restrictive_controls_and_remembered_denies() {
    const RM: &str = "Bash(rm *)";
    const QUOTED: &str = r#"LOG=/tmp/log; rg -n "ERROR" "$LOG""#;
    const RG_CONFIG: &str = r#"RIPGREP_CONFIG_PATH=/a; rg x "$RIPGREP_CONFIG_PATH""#;
    const LOADER: &str = r#"LD_LIBRARY_PATH=./lib; ls "$LD_LIBRARY_PATH""#;
    tokio::task::LocalSet::new()
        .run_until(async {
            let default = PermissionState::default;
            for mode in [PromptPolicy::Auto, PromptPolicy::Allow] {
                for client in [ClientType::Generic, ClientType::GrokPager] {
                    let mut fixture =
                        FilenameFixture::with_rules(mode, &PARTIAL, client, default()).await;
                    fixture.manager.set_auto_mode(true);
                    fixture.classify(ClassifierVerdict::Block);
                    let (decision, event) = fixture.bash(LOG_SCRIPT).await;
                    let reason = event.decision_reason.as_deref();
                    assert_ne!(Decision::Allow, decision, "{mode:?} {client:?}");
                    assert_eq!(Some(reasons::AUTO_CLASSIFIER_DENY), reason);
                    assert_eq!(1, fixture.classified());
                    let prompted = usize::from(client == ClientType::GrokPager);
                    assert_eq!(prompted, fixture.prompts.borrow().len());
                }
            }
            for (mode, rule, command, prompts) in [
                (PromptPolicy::Ask, "Read(secret)", LOG_SCRIPT, 1),
                (PromptPolicy::Deny, RM, LOG_SCRIPT, 0),
                (PromptPolicy::Auto, RM, REMOVES, 1),
                (PromptPolicy::Auto, "Bash(rg -n ERROR *)", QUOTED, 1),
                (PromptPolicy::Ask, RM, r#"ls "$LOG"; rm -rf /x"#, 0),
                (PromptPolicy::Ask, RM, RG_CONFIG, 1),
                (PromptPolicy::Ask, RM, LOADER, 1),
                (PromptPolicy::Ask, "", LOG_SCRIPT, 1),
            ] {
                let mut rules = vec![("Bash(rm -rf *)", RuleAction::Deny)];
                if !rule.is_empty() {
                    rules.extend([(rule, RuleAction::Ask), ("Bash", RuleAction::Allow)]);
                }
                let mut fixture = FilenameFixture::with_rules(mode, &rules, PAGER, default()).await;
                let (decision, _) = fixture.bash(command).await;
                assert_ne!(Decision::Allow, decision, "{command}");
                assert_eq!(prompts, fixture.prompts.borrow().len(), "{command}");
                assert_eq!(0, fixture.classified(), "{command}");
            }
            for mode in [PromptPolicy::Ask, PromptPolicy::Auto] {
                let state = PermissionState {
                    disallowed_bash_commands: HashSet::from(["rm".to_owned()]),
                    ..PermissionState::default()
                };
                let allow = [("Bash", RuleAction::Allow)];
                let mut fixture = FilenameFixture::with_rules(mode, &allow, PAGER, state).await;
                let (decision, event) = fixture.bash(WRAPPED_REMOVES).await;
                let reason = event.decision_reason.as_deref();
                assert!(matches!(decision, Decision::Reject(_)), "{mode:?}");
                assert_eq!(Some(reasons::SESSION_DENY), reason);
                assert_eq!((false, 0), (event.user_prompted, fixture.classified()));
            }
        })
        .await;
}
