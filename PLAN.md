# Plan: Trait-化 permissions / hitl / planning

## Step 1: permissions.rs → PermissionChecker trait
保留 PermissionDecision 枚举，抽出 trait，删除具体实现

## Step 2: hitl.rs → ConfirmationProvider trait  
保留核心类型，抽出 trait，删除 ConfirmationManager 实现

## Step 3: planning/ → Planner trait
保留数据类型，抽出 trait，删除 llm_planner.rs

## Step 4: 更新 session/mod.rs, manager.rs, agent_api.rs, tools/mod.rs

## Step 5: 更新 tests
