# Read-only conversation sharing

The optional sharing relay publishes only conversations selected by the owner.
It does not run agents, receive provider credentials, access repositories or
provide write operations.

## Start the relay

Run this on the public host. Docker is the default; pass `--runtime podman` when
appropriate.

```sh
aic server start \
  --public-url https://share.example.com \
  --host 127.0.0.1 \
  --port 8787
```

The command starts the `aic-share-server` container, creates `./.aic-share`, and
prints a publisher token. The SQLite database persists in that directory when
the container is replaced or stopped.

Place Caddy, Nginx or another TLS reverse proxy in front of
`127.0.0.1:8787`. The public URL must use HTTPS. The relay itself intentionally
does not manage DNS or certificates.

Useful administration commands:

```sh
aic server status
aic server token
aic server stop
```

Treat the publisher token like an administrator password. Anyone holding it can
replace or revoke content on the relay. It is separate from every visitor
password.

## Connect the local chat

Open a conversation's actions menu and choose **Share read-only**. On first use,
enter the public URL and publisher token. Then choose one or more conversations
and create the sharing link.

The dialog displays the visitor password once. Save or copy it before closing
the dialog; only its Argon2id hash is retained. Password rotation invalidates
all existing visitor sessions.

Visitors open the generated `/s/<shared_id>` URL, enter its password and see a
read-only list of the selected conversations. No account or email address is
required.

## Published data

The local chat remains the source of truth. It asynchronously sends a sanitized
projection containing conversation titles, public participant labels and
completed message text. It excludes:

- drafts and pasted-text backing data;
- native agent session identifiers;
- repository, checkout and host paths;
- Git context, plans, quotas and diagnostics;
- model activity/tool traces and write permissions;
- local configuration and provider credentials.

If the relay is unavailable, local chat saves continue normally. The sharing
dialog shows the last synchronization error and the next state change retries
publication.

## Security boundaries

- Keep the local chat bound to loopback; do not expose port 7878.
- Expose only the relay through the TLS reverse proxy.
- Visitor sessions use `HttpOnly`, `SameSite=Strict` cookies and expire after 24
  hours.
- Unlock attempts are rate-limited per connection address and sharing ID.
- Revoking a link or rotating its password invalidates prior sessions.
- Unknown and existing sharing IDs serve the same locked page.
