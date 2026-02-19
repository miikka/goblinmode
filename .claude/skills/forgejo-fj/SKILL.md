---
name: forgejo-fj
description: >
  Use this skill whenever you need to interact with a Forgejo git forge —
  reading issues, creating issues, opening pull requests, viewing PR status,
  or any other forge operation (comments, labels, milestones, etc.).
  Forgejo repos are self-hosted and use the `fj` CLI, NOT `gh`. Trigger this
  skill whenever you see a Forgejo remote URL, or whenever the user mentions
  issues, PRs, or pull requests in a repo that isn't hosted on GitHub.
---

# Working with Forgejo using `fj`

When you need to access issues or pull requests for a Forgejo-hosted repo, use
the `fj` CLI. Do not use `gh` — it only works with GitHub.

`fj` auto-detects the repo from the current git remote, so most commands work
without specifying a repo explicitly.

## Issues

```bash
# View an issue (body only by default)
fj issue view <ID>
fj issue view <ID> comments   # list all comments

# Search / list issues
fj issue search

# Create an issue
fj issue create "Title" --body "Description"

# Edit or close
fj issue edit <ID>
fj issue close <ID>

# Add a comment
fj issue comment <ID> --body "Comment text"
```

## Pull Requests

```bash
# Create a PR (autofill title/body from commits — good default)
fj pr create --autofill

# Create a PR with explicit title and body
fj pr create "Title" --body "Description" --base main

# View a PR
fj pr view <ID>

# Check CI and mergeability
fj pr status <ID>

# Search PRs
fj pr search

# Comment, edit, merge, close
fj pr comment <ID> --body "Comment"
fj pr merge <ID>
fj pr close <ID>
```

## Tips

- Use `--body-file <file>` to pass long issue/PR bodies from a file instead
  of quoting them on the command line — avoids shell escaping headaches.
- Prefix a PR title with `"WIP: "` to create it as a draft.
- Use `-R <remote>` if the repo has multiple git remotes and `fj` picks the
  wrong one.
- Run `fj issue --help` or `fj pr --help` for the full list of subcommands.
