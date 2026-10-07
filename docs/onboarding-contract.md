# Onboarding implementation decisions

Canonical slug: `aic-chat-onboarding`, as explicitly requested by the user with
`/aic-plan aic-chat-onboarding`. References to `aic-chat-improvements` in the PM
review refer to an earlier proposal; this work retains the user's actual slug.

First release targets: Linux x86_64 and arm64, native static musl artifacts.
macOS, Windows, other CPU architectures and glibc artifacts are not release targets.
A static aic binary does not make third-party CLIs or container images portable.

Shared diagnostic schema version 1 contains environment, Git context, default
agent and sorted agent records. Each agent independently reports configuration,
installation, authentication, execution, quota and derived readiness, plus safe
reason/action codes, last-test UTC timestamp, verification origin and context.
Only successful real execution in the configured context establishes readiness.
Unknown authentication must stay unknown; quota errors never imply logout.

Last tests live in `.aic/diagnostics/tests.json`, local to the checkout, written
atomically with owner-only permissions. No credentials, session IDs, responses,
stderr or account details are stored. Results expire after 24 hours. Invalidation
includes binary path/size/modification time, runtime and executable identity,
image content ID, effective agent/sandbox configuration, relevant credential-file
metadata and fingerprints of relevant provider environment variables (values are
never persisted or displayed). A real
execution failure replaces the previous success. Quota exhaustion preserves
known authentication and independently reports quota exhaustion.

Diagnosis does not generate provider responses. `agent test` requires interactive
confirmation or explicit `--yes`, caps time/output and uses the existing read-only
agent invocation. The web refresh only rereads diagnosis/cache; paid retries use
the terminal command displayed by the panel. New chats never run paid tests.
