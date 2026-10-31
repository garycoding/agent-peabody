# Contributing

The authoritative version of all of this is the [ParkviewLab handbook](https://github.com/ParkviewLab/handbook/tree/main), which this repository follows.

## Branch & PR flow

- Branch off `develop` into an ephemeral worktree named with a prefix: `feature-`, `bug-`/`fix-`, `doc-`, `test-`, `ops-`, `ci-`, `build-`, `release-` (hyphen, not slash). See the handbook's `branching.md`.
- Open a PR into `develop`. The repo is merge-commit only, so the merge button can only make a merge commit.
- Releases are cut from `main` via the CLI (`git merge --no-ff develop`, then bump + tag), not a PR, and end with the back-merge pull request from `back-merge-<tag>`, which `git back-merge` opens, checks and merges. See the handbook's `releases.md`.

## Commit / PR-title convention (this is what the changelog reads)

Because a PR is merged with a merge commit titled `<PR title> (#N)`, the PR title becomes the commit subject, and the changelog is generated from it. Prefix every PR title with a [Conventional Commit](https://www.conventionalcommits.org/) type (`feat:`, `fix:`, `perf:`, `refactor:`, `docs:`, `test:`, `build:`, `chore:`, `ci:`, `style:`; `!` after the type for a breaking change). See the handbook's `commits-and-changelogs.md`.

## Local checks before opening a PR

Run the same checks CI requires, so the PR is green on arrival:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
uvx --from "reuse[charset-normalizer]" reuse lint
```

The version lives in `VERSION.txt`; a feature PR never changes it (the `no-version-change` check), since the bump happens at release. The crates' own version in `Cargo.toml` stays `0.0.0` and is never shipped.
