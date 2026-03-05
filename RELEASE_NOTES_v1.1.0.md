# A3S Code v1.1.0 Release Notes

**Release Date**: 2026-03-05

## 🎉 What's New

### Real-time SubAgent Monitoring

v1.1.0 introduces comprehensive real-time monitoring capabilities for SubAgents, allowing main agents to track task lists, current activities, and execution states in real-time.

## ✨ New Features

### 1. Real-time Monitoring API

**11 new monitoring APIs** for complete SubAgent visibility:

- `list_subagents()` - Get all SubAgent information with metadata
- `get_subagent_info(id)` - Query specific SubAgent details
- `get_active_activities()` - Get all active SubAgent activities
- `get_all_states()` - Get all SubAgent states
- `active_count()` - Get active SubAgent count
- `pause_subagent(id)` - Pause specified SubAgent
- `resume_subagent(id)` - Resume specified SubAgent
- `cancel_subagent(id)` - Cancel specified SubAgent
- `wait_all()` - Wait for all SubAgents to complete
- `get_handle(id)` - Get SubAgent handle for direct control

### 2. Activity Tracking

**4 activity types** with real-time updates:

- **Idle** - SubAgent is idle
- **CallingTool** - Calling a tool (with tool name and arguments)
- **RequestingLlm** - Requesting LLM (with message count)
- **WaitingForControl** - Waiting for control signal (with reason)

### 3. SubAgentInfo Structure

Complete metadata for each SubAgent:

```rust
pub struct SubAgentInfo {
    pub id: String,
    pub agent_type: String,
    pub description: String,
    pub state: String,
    pub parent_id: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub current_activity: Option<SubAgentActivity>,
}
```

### 4. Full SDK Support

**Python SDK**:
- All 11 monitoring APIs implemented
- Complete type definitions
- Activity tracking support

**Node.js SDK**:
- All 11 monitoring APIs implemented
- TypeScript definitions with JSDoc
- Activity entry types for tuple returns

## 📚 Documentation

### Updated Documentation

- **orchestrator.mdx** (CN + EN) - Added real-time monitoring API section
- **API_REFERENCE.md** - Quick reference for all monitoring APIs
- **ORCHESTRATOR_MONITORING.md** - Complete usage guide (400+ lines)
- **REAL_TEST_REPORT.md** - Actual test results with Kimi API
- **TEST_REPORT.md** - Test status summary

### Code Examples

**Python**:
```python
# Get all SubAgent information
subagents = orch.list_subagents()
for info in subagents:
    print(f"{info.id}: {info.state}")
    if info.current_activity:
        print(f"  Activity: {info.current_activity.activity_type}")

# Get active activities
activities = orch.get_active_activities()
for subagent_id, activity in activities:
    print(f"{subagent_id}: {activity.activity_type}")
```

**TypeScript**:
```typescript
// Get all SubAgent information
const subagents = orch.listSubagents();
for (const info of subagents) {
  console.log(`${info.id}: ${info.state}`);
  if (info.currentActivity) {
    console.log(`  Activity: ${info.currentActivity.activityType}`);
  }
}

// Get active activities
const activities = orch.getActiveActivities();
for (const entry of activities) {
  console.log(`${entry.id}: ${entry.activity.activityType}`);
}
```

## 🧪 Testing

### Test Coverage

- ✅ 11/11 core APIs tested and working
- ✅ 4/4 activity types verified
- ✅ Real-time state and activity updates confirmed
- ✅ All APIs tested with real Kimi API execution

### Test Files

- `test_simple_fixed.py` - Simplified test (verified working)
- `test_real_kimi.py` / `test_real_kimi.ts` - Full-featured tests
- `test_apis.py` / `test_apis.ts` - API validation scripts

## 🔄 Migration Guide

### From v1.0.4 to v1.1.0

**No breaking changes!** All existing code continues to work.

**New capabilities available**:

```python
# Before v1.1.0 - Limited visibility
handle = orch.spawn_subagent(config)
state = handle.state()  # Only basic state

# After v1.1.0 - Full visibility
handle = orch.spawn_subagent(config)

# Get detailed information
info = orch.get_subagent_info(handle.id)
print(f"State: {info.state}")
print(f"Activity: {info.current_activity.activity_type}")
print(f"Created: {info.created_at}")

# Monitor all SubAgents
subagents = orch.list_subagents()
for info in subagents:
    print(f"{info.id}: {info.state} - {info.current_activity}")
```

## 📦 Installation

### Python

```bash
pip install a3s-code==1.1.0
```

### Node.js

```bash
npm install @a3s-lab/code@1.1.0
```

### Rust

```toml
[dependencies]
a3s-code-core = "1.1.0"
```

## 🔗 Links

- **GitHub Repository**: https://github.com/A3S-Lab/Code
- **Documentation**: https://a3s.ai/docs/code/orchestrator
- **Examples**: https://github.com/A3S-Lab/Code/tree/main/sdk/examples
- **Changelog**: https://github.com/A3S-Lab/Code/blob/main/core/CHANGELOG.md

## 🙏 Acknowledgments

Special thanks to all contributors and testers who helped make this release possible!

## 📝 Full Changelog

See [CHANGELOG.md](https://github.com/A3S-Lab/Code/blob/main/core/CHANGELOG.md) for complete details.

---

**Questions or Issues?**

- Report issues: https://github.com/A3S-Lab/Code/issues
- Discussions: https://github.com/A3S-Lab/Code/discussions
