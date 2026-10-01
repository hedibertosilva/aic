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
