# Documentation

Start with the README for an overview and the [setup and recovery](onboarding.md)
guide to install and start your first chat.

## User guides

- [Setup and recovery](onboarding.md): installation, sandbox, diagnosis and common issues.
- [Worktrees](worktrees.md): how the chat identifies and selects checkouts.
- [Plans in chat](chat-plans.md): plans, tasks, execution and validation.
- [Conversation import](chat-import.md): importing and exporting history.
- [Read-only sharing](sharing.md): deploying a relay and publishing selected conversations.
- [Building from source](build-from-source.md): development and manual installation.

## Implementation references

These documents describe internal contracts, decisions and validation; they are
not required for first use:

- [Onboarding contract](onboarding-contract.md)
- [Onboarding validation](onboarding-validation.md)
- [Chat plans API](chat-plans-api.md)
- [Chat plans backend](chat-plans-backend.md)
- [Chat plans validation](chat-plans-validation.md)
- [Worktree selection design](worktree-selection-design.md)
- [Agent readiness spike](agent-readiness-spike.md)
- [Quota discovery spike](agent-quotas-spike.md)
- [Quota UX specification](agent-quotas-ux.md)
- [Onboarding release notes](release-notes-onboarding.md)

Technical references may record details from a specific version. For current
product behavior, prefer the guides and the code in `src/`.
