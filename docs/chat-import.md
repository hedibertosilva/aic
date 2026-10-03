# Importing conversations

Use **Import conversation** in the sidebar or **Import conversation (.json, .md)** in an empty chat. Importing creates a new conversation and keeps its original title. It does not send any messages automatically. The success notification offers **Undo**.

JSON and Markdown exported by aic are supported, including older exports. Other tools' export formats are not supported. Ordinary Markdown without an aic conversation structure opens as an unsent draft in a new chat.

Imported participants start fresh native sessions with the imported history as context. File editing is always disabled on import, even if the file enables it. Enable writing in the participant settings when needed. Unavailable agents are preserved as inactive participants; edit their settings to select an available agent and activate them.

Files must be at most 16 MiB and fit in the browser cache. Invalid files and cache failures do not create a partial conversation.

## Portable export format

JSON exports contain `version: 1`, `title`, `participants`, `log`, `round`, and `sendTo`. Local session IDs and read positions are omitted. JSON exports without a version are accepted as version 1. Unknown fields are ignored; authors and recipients must reference valid participants.

New Markdown exports include YAML frontmatter with an `aic` key whose value is a JSON flow object. It carries the same metadata, with each log entry's `text` replaced by `textLines`. Message text follows in the familiar `## you` and `## Name (Agent, Model)` sections. Line counts preserve embedded headings, fenced code, empty messages, and trailing newlines without duplicating message content. If editing an exported file manually, use JSON; changes to Markdown message lengths require matching changes to `textLines`.

Older Markdown exports use the title, Participants line, and message headings to reconstruct history. Participant settings that were absent from those exports use defaults; message headings inside fenced code are treated as content. Legacy Markdown cannot disambiguate message headings appearing verbatim outside fenced code.

## Tests

Run `node --test tests/chat-import.test.cjs`. These tests cover round trips, legacy formats, unavailable agents, invalid files, interrupted replies, drafts, undo, and cache failures.
