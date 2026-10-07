# Git context and worktrees

The chat displays the repository, branch and checkout path. In detached HEAD,
it displays the short commit SHA. The checkout is the Git working directory where
the server was started; a linked worktree is supported.

Before each send, the current branch is compared with the conversation's recorded
origin and last acknowledgement. A branch change displays a notice: the history
and participant sessions continue, but the next sends see the current branch's
files. Continue acknowledges that observed change only; Create new chat records
the current branch as its origin. No automatic history/session reset occurs.

`.aic/` is ignored by Git and local to each checkout. Changing branches in the
same directory preserves chats, diagnostic tests, plans, state and logs. A second
worktree has its own `.aic/`; these files are not in a PR or shared automatically.

```sh
git worktree add ../project-review -b review
aic chat --no-open
# In another terminal:
cd ../project-review
aic setup
aic chat --port 7879 --no-open
```

Inspecting `git show other-branch:path` or a Git diff reads committed Git data;
it does not switch the agent's working directory to another checkout. Agents in
a sandbox can see shared Git metadata, but only explicitly mounted working trees.
Selection of another worktree and copying conversations have separate controls;
never change an active native session's directory silently.

Configuration → Advanced → Change working checkout lists only registered worktrees
of the same repository. This P2 control is not a primary action in the chat footer.
Confirming a selection keeps visible history and starts fresh participant sessions;
it is unavailable while replies run. Run `aic agent test` in the destination
checkout before sending. The server stores this conversation in its original
checkout database; selecting an execution directory does not move history.

To copy a conversation to another checkout, export JSON from its menu and import
it in the destination's chat server. Imported sessions are cleared and identities
renewed. Verify the imported history before deleting the source; no automatic
move or overwrite is performed. See [selection design](worktree-selection-design.md).

Configuration also offers agent visibility controls. The effective default stays
visible and cannot be hidden. Hiding another agent affects selectors
only; it does not remove configuration, participants or earlier messages. The
skill autocomplete shows supported agents, a single-recipient requirement and
required file access, together with arguments and the reason a skill is unavailable.
