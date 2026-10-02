pub fn plan_template(title: &str) -> String {
    format!(
        "# {title}\n\n\
## Problem\n\n\
<!-- What problem does this solve? For whom? -->\n\n\
## Proposal\n\n\
<!-- What will be built, at a high level. -->\n\n\
## Alternatives considered\n\n\
<!-- What else was considered, and why it wasn't chosen. -->\n\n\
## Impact\n\n\
<!-- What changes for users, other teams, infrastructure. -->\n\n\
## Open questions\n\n\
<!-- What is still undecided. -->\n"
    )
}

/// Default `claude` version baked into the sandbox image; bump with
/// `aic sandbox build --claude-version <v>`.
pub const DEFAULT_CLAUDE_VERSION: &str = "2.1.287";

/// Dockerfile for `aic sandbox build`. `agy` has no pinned public install
/// method, so the host's binary is copied into the build context and
/// `COPY`d in when `include_agy` is set.
pub fn sandbox_dockerfile(include_agy: bool) -> String {
    let agy = if include_agy {
        "COPY agy /usr/local/bin/agy\n"
    } else {
        ""
    };
    format!(
        "FROM docker.io/library/node:22-bookworm-slim\n\
ARG CLAUDE_VERSION={DEFAULT_CLAUDE_VERSION}\n\
RUN apt-get update \\\n\
 && apt-get install -y --no-install-recommends git ca-certificates ripgrep openssh-client curl \\\n\
 && rm -rf /var/lib/apt/lists/* \\\n\
 && npm install -g @anthropic-ai/claude-code@${{CLAUDE_VERSION}} \\\n\
 && mkdir -p /home/aic && chmod 1777 /home/aic\n\
{agy}ENV HOME=/home/aic\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dockerfile_pins_claude_and_optionally_copies_agy() {
        let with = sandbox_dockerfile(true);
        assert!(with.contains("ARG CLAUDE_VERSION=2.1.287"));
        assert!(with.contains("claude-code@${CLAUDE_VERSION}"));
        assert!(with.contains("COPY agy /usr/local/bin/agy"));
        assert!(with.contains("chmod 1777 /home/aic"));
        assert!(!sandbox_dockerfile(false).contains("COPY agy"));
    }
}
