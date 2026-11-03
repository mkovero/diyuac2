# Notes for Claude Code

## Project tracking (pm)

Open work for this repo is tracked as GitHub issues (labels `next`, `blocked`, `needs-parts`, `cross-project`) on the owner's cross-project board. Keep them current:

- A commit that finishes an issue: put `Fixes #N` in the message. Pushing to the default branch closes it, and the board moves it to Done.
- Progress worth knowing, or a finding that changes an issue: `gh issue comment N`.
- A new blocker: add the `blocked` label and comment on what it waits for. Waiting on parts: add the `needs-parts` label. Add a line to `bom/needs.csv` only after the owner confirms the part is really needed (existing BOMs and baskets are mostly already ordered).
- Don't open, close or reprioritise issues on your own beyond the above. Propose them to the owner; a weekly `/pm` report covers the rest.
- The repo's status docs stay the detailed source of truth; the issues are the short list.
