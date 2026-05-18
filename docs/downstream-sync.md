# Downstream Sync Model

Rusty Red uses an upstream/downstream release model:

- This standalone repository is the upstream source for versioned Rusty Red releases.
- Product repositories consume Rusty Red as a downstream subtree at a configured path.
- Downstream-only adapters, deployment wiring, secrets, and product-specific behavior stay in the downstream repository.
- A downstream change does not flow back upstream unless it is intentionally contributed here.

This keeps public Rusty Red releases clean while still letting private product
deployments consume every upstream release.

## Recommended Git Shape

Use `git subtree` for downstream integrations. Subtree syncs preserve downstream
history, allow downstream-specific edits to remain local, and surface normal
merge conflicts when both upstream and downstream touch the same lines.

The default downstream path is `vendor/rusty-red`, but production integrations
should set `DOWNSTREAM_SYNC_PATH` to the path they already use.

## GitHub Sync Workflow

The included `.github/workflows/sync-downstream.yml` opens a pull request in a
configured downstream repository after a push to `main`.

Configure these repository variables and secrets in this upstream repository:

| Name | Kind | Required | Default | Purpose |
|------|------|----------|---------|---------|
| `DOWNSTREAM_SYNC_REPOSITORY` | variable | yes | none | Target repository in `owner/repo` form |
| `DOWNSTREAM_SYNC_TOKEN` | secret | yes | none | Token with write access to the target repository and pull requests |
| `DOWNSTREAM_SYNC_PATH` | variable | no | `vendor/rusty-red` | Path inside the downstream repository |
| `DOWNSTREAM_SYNC_BRANCH` | variable | no | `main` | Base branch in the downstream repository |
| `DOWNSTREAM_SYNC_MODE` | variable | no | `pull` | `pull` for established subtrees, `add` only for an empty first import |

The workflow intentionally opens a pull request instead of pushing directly to
the downstream base branch. That preserves downstream-only work and makes
conflicts visible at the right boundary.

## First Import

If the downstream path does not exist yet:

```bash
  scripts/sync-downstream-subtree.sh \
  --downstream /path/to/downstream-repo \
  --prefix vendor/rusty-red \
  --remote https://github.com/Travis-Gilbert/RustyRed-Graph-Database.git \
  --ref main \
  --mode add
```

If the downstream path already exists, align it in a one-time migration PR
before enabling automatic sync. After that, use:

```bash
  scripts/sync-downstream-subtree.sh \
  --downstream /path/to/downstream-repo \
  --prefix vendor/rusty-red \
  --remote https://github.com/Travis-Gilbert/RustyRed-Graph-Database.git \
  --ref main \
  --mode pull
```

## Boundary Rules

- Upstream-owned files should be edited in this repository first.
- Downstream-only behavior should live outside the upstream-owned path whenever practical.
- If downstream must patch an upstream-owned file, keep the patch small and expect future sync PRs to surface conflicts.
- Release notes and version bumps belong upstream.
- Deployment overlays, private adapters, and product-specific environment defaults belong downstream.
