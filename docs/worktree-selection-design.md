# Worktree selection and migration design (P2)

Selection is restricted to live Git worktrees registered by the server's original
repository. Canonical paths must match `git worktree list --porcelain -z`; callers
cannot select arbitrary directories, symlink escapes, deleted/prunable worktrees
or another repository. No worktrees are created or removed by the web server.
The selector is a P2 control under Git context → Advanced → This conversation only,
separated from global Configuration and the consultative Environment panel. A
modal Configuration form makes the conversation control inert, so opening checkout
selection cannot close or discard unsaved global preferences.

The conversation stores its selected checkout and Git origin. Selecting another
worktree explicitly confirms the change, clears participant native session IDs
and cursors, and preserves visible history. A reply already running cannot switch
checkout. The backend remembers native session/checkouts and rejects a session
from another checkout. It resolves configuration, readiness, cwd and sandbox
mounts from the selected worktree. Cached readiness remains local to that worktree;
the user must test agents there before sending. The shared Git directory remains
mounted by the existing sandbox logic, without mounting other working trees.

The server's original SQLite database remains the conversation storage location;
choosing where agents execute does not silently migrate history. Migration is a
separate export/import operation: export a conversation and import it in the
other checkout's server. Imports generate new conversation identities and clear
native sessions, avoiding collision and reuse of credentials. Keep the source
copy until the destination is verified; no automatic moves or overwrites.

Security review: same-origin/Host checks cover mutations; selected paths are
validated again at each request, not trusted from persisted browser data. Session
provenance is validated server-side. Mount construction stays in Sandbox and the
configured read-only/edit-access policy remains unchanged. Paths/context, not
credentials/session content, are returned by the worktree list API.
