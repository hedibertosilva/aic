# Plans in chat

New chats remain free conversations. Participants can edit according to their
file permissions without creating a plan or checklist.

Use **Link or create plan**, beside the Git context, to select a checkout-local
plan or create one with an objective and acceptance criteria. Several chats can
share the same plan. Participant **Role / instructions** describes how that
participant should work; the plan holds the shared objective.

**Tasks** opens the shared checklist, objective and execution history. Add a
quick task there when a full planning document is unnecessary. **Refresh** reads
the latest server state; **Reload changes from plan files** imports deliberate
edits made outside the chat. Cancelled tasks are shown but cannot be executed.

In a planned chat, **Discuss — read only** keeps planning and review separate
from file edits. To write, choose **Execute tasks** and select a task for each
writing recipient. Task selections are saved per participant. Concurrent writers
must use separate tasks. Participants without edit permission remain read only.

An execution records participant, conversation, branch, checkout and changed
files. A successful agent response leaves its task awaiting validation. Use
**Validate**, describe what was checked and confirm the acceptance criteria to
mark the task completed. The server updates TASKS.md and EXECUTION.md.

Changing the plan of an empty chat updates that chat. With history, the button
explicitly creates a fork: messages and participants stay, native sessions
restart and task selections clear. The source chat keeps its plan. **Fork
history** identifies the source and previous plan. Ordinary `/fork` preserves
the shared plan. Changing to a free conversation follows the same rules.

The fork notification offers **Undo**. An unused fork can be removed; once a
turn has started, undo returns to the source while preserving the fork and its
execution records.

When changing a planned checkout under **Advanced**, explicitly detach, select
a destination plan or copy the current plan/checklist there. Copying preserves
the original plan and original execution identities in their checkout. Sessions
restart and task selections clear. A conversation with history forks for this
change as well.

Version conflicts never apply a silent overwrite. Inputs and composer drafts
remain available; refresh plan/task data or reload the conversation when the
chat snapshot changed in another window. Plans stay local to their checkout in
`.aic/plans/<slug>/`. Linking several chats does not share them automatically
between worktrees.

The [API contract](chat-plans-api.md) describes the server-owned records and
versions. Rebuild and restart `aic` after changing its embedded web assets.
