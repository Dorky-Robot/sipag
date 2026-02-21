# Setting Up a New Repo for sipag

Everything you need to do once to make a repo work well with sipag workers.

## 1. Create the `approved` label

Workers only pick up issues labeled `approved`. Create it in your repo:

```bash
gh label create approved \
  --color 0075ca \
  --description "Ready for a sipag worker to implement" \
  --repo owner/repo
```

Add any other label conventions your workflow needs:

```bash
gh label create needs-spec \
  --color e4e669 \
  --description "Issue needs more detail before it can be approved" \
  --repo owner/repo

gh label create in-progress \
  --color 0e8a16 \
  --description "A sipag worker is working on this" \
  --repo owner/repo
```

sipag manages `in-progress` automatically — it adds the label when a worker picks up an issue and removes it on completion.

## 2. Add a CLAUDE.md file

`CLAUDE.md` is how you tell Claude how to work with your repo. sipag's executor prompt explicitly instructs Claude to read it before writing any code.

Create it at the repo root:

```bash
touch CLAUDE.md
```

### Minimum viable CLAUDE.md

```markdown
## Project
[One paragraph: what this repo does, who uses it, what it's built with]

## Priorities
[What matters right now — stability, a specific feature area, a migration in progress]

## Architecture
[Key directories and what they do. Patterns to follow or avoid.]

## Testing
[Exact commands to run tests. What "passing" looks like.]
```

### Example CLAUDE.md

```markdown
## Project
myapp is a SaaS time-tracking tool for freelancers.
Node.js/Express backend, React frontend, PostgreSQL database.
~200 active users. Weekly releases.

## Priorities
Stability first. We're in the middle of migrating from REST to GraphQL.
Do not modify the REST endpoints — they're still in use by mobile clients.
New features should use the GraphQL API.

## Architecture
- src/api/rest/    — legacy REST handlers (do not modify without discussion)
- src/api/graphql/ — new GraphQL resolvers (preferred for new features)
- src/domain/      — business logic (no framework dependencies)
- src/db/          — knex migrations and query builders
- client/          — React frontend (TypeScript, no class components)

## Testing
npm test              # unit tests (vitest)
npm run test:e2e      # E2E tests (playwright, requires dev server)
npm run lint          # eslint + tsc

All tests must pass. If you're adding a feature, add tests for it.

## Labels
- `approved`      — ready for a sipag worker to implement
- `needs-spec`    — needs more detail
- `blocked`       — waiting on external dependency
- `breaking`      — involves a breaking change, needs extra care
```

### What Claude does with CLAUDE.md

When a worker starts, Claude reads `CLAUDE.md` before reading any source files. It uses it to:

- Understand what the project does and who it affects
- Know what's currently fragile or in-progress
- Follow the right patterns when writing code
- Know how to run tests and what "passing" means
- Apply the right labels when opening PRs

The more specific you are, the better the output.

## 3. Set up branch protection (recommended)

Protect `main` so workers can't push directly to it:

```bash
gh api repos/owner/repo/branches/main/protection \
  --method PUT \
  --field required_status_checks='{"strict":true,"contexts":[]}' \
  --field enforce_admins=false \
  --field required_pull_request_reviews='{"required_approving_review_count":1}' \
  --field restrictions=null
```

With branch protection, workers are forced to open PRs — they can't accidentally push to main even if something goes wrong.

## 4. Write good issues

Workers do their best work when issues are clear and self-contained. A good issue has:

**A specific title:**

```
Bad:  "Improve performance"
Good: "Add database connection pooling to reduce query latency"
```

**Concrete acceptance criteria:**

```
Bad:  "Users should be able to reset their passwords"
Good: "Add POST /auth/reset-password that:
       - Accepts {email}
       - Sends a reset link valid for 1 hour
       - Returns 200 if the email exists, 200 if it doesn't (no enumeration)
       - Rate limited to 3 requests per hour per IP"
```

**Context when the task is non-obvious:**

```
Context: The current password reset flow is broken because SendGrid's
transactional email API changed their authentication method in v3. We need
to update the email service to use the new API key auth instead of basic auth.

File to update: src/services/email.ts
Docs: https://docs.sendgrid.com/api-reference/mail-send/mail-send
```

**Tests or examples when helpful:**

```
The existing test suite covers the happy path in test/auth.test.ts.
Add tests for:
- Email not found (should still return 200)
- Rate limiting (4th request should return 429)
- Expired token (should return 400)
```

## 5. Register the repo with sipag

If you work with this repo regularly, register it by name:

```bash
sipag repo add myapp https://github.com/owner/myapp
```

Now you can use `myapp` everywhere a URL is accepted:

```bash
sipag start myapp
sipag merge myapp
```

## Checklist

Before using sipag with a new repo:

- [ ] `approved` label exists
- [ ] `CLAUDE.md` added to repo root
- [ ] Branch protection enabled on `main`
- [ ] Issues are written with clear acceptance criteria
- [ ] Repo registered with `sipag repo add` (optional, for convenience)

---

[Running sipag in CI →](ci-integration.md){ .md-button .md-button--primary }
[Configuration reference →](../configuration.md){ .md-button }
