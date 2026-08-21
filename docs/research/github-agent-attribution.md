# GitHub attribution for agent-authored work

Research date: 2026-08-20

## Project decision

The project selected the reusable **machine-user** alternative described below and
implemented its fail-closed local workflow in
[PR #23](https://github.com/skulldogged/gpui-ghostty-agent-terminal/pull/23).
Agent-authored GitHub mutations now go through `agent-gh`, while signed commits and
pushes go through `agent-git`. The private-App option remains the researcher's
least-privilege recommendation for a repository-specific identity, not the project's
current setup.

## Research recommendation

Use a **private GitHub App**, installed only on `skulldogged/gpui-ghostty-agent-terminal`, for this repository's agent-authored GitHub writes. An installation access token attributes issues, pull requests, comments, reviews, and other API mutations to the App, visibly as an App bot rather than as `skulldogged`. Installation tokens last one hour, can be narrowed to selected repositories and permissions, are independent of a human account, and do not consume a GitHub seat. [GitHub App authentication and attribution](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/about-authentication-with-a-github-app), [App versus PAT](https://docs.github.com/en/apps/creating-github-apps/about-creating-github-apps/deciding-when-to-build-a-github-app), [installation-token endpoint](https://docs.github.com/en/rest/apps/apps#create-an-installation-access-token-for-an-app)

A **machine user** is permitted and would look more literally like a separate GitHub user: it has a normal `@username`, profile, sign-in, and notification inbox. It is reasonable only if that user-like profile or reuse across unrelated owners is more important than least privilege. For this public, personal-account repository it is the weaker default: the account can only be added as a write collaborator, and GitHub's current fine-grained-PAT limitations prevent a repository collaborator from using a fine-grained PAT to contribute. Local automation would therefore need a classic `public_repo` PAT for API/`gh` operations, plus either that PAT or an SSH key for Git. (`repo` would be required for private repositories.) The classic scope remains broader than the three narrow permissions required by the App. [Machine users](https://docs.github.com/en/authentication/connecting-to-github-with-ssh/managing-deploy-keys#machine-users), [fine-grained PAT limitations](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens#fine-grained-personal-access-token-limitations), [classic `public_repo` and `repo` scopes](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/scopes-for-oauth-apps#available-scopes), [personal-repository collaborator permissions](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/repository-access-and-collaboration/permission-levels-for-a-personal-account-repository)

The result with the recommended App would be an actor such as `ghostty-agent-terminal-agent[bot]`, not a human-style account. If the requirement is specifically a normal profile named something like `@ghostty-agent-terminal-agent`, choose the machine-user alternative with the tradeoffs documented below.

## The two identities are not equivalent

| Property | Private GitHub App | Machine-user account |
| --- | --- | --- |
| Visible actor for API-created issues, PRs, comments, and reviews | App bot, conventionally `<app-slug>[bot]` | Normal `@username` |
| Interactive profile and notification inbox | No normal sign-in or inbox | Yes |
| Authentication | App private key mints a short JWT, which mints a one-hour installation token | Account credential plus PAT and/or SSH key |
| Repository scope | Installation and each token can be restricted to selected repositories | Limited by collaborator access, but the necessary classic `public_repo` PAT applies broadly to public repositories |
| Permission granularity | Separate Contents, Issues, and Pull requests permissions | Personal repositories grant collaborators write access; classic `public_repo` covers public repository data broadly |
| Reuse | Reusable wherever the App is installed; a private App can only be installed on its owner's account | Reusable across repositories and owners that invite the account |
| Human-account lifecycle | Independent of a user and no seat | Account ownership, email, 2FA, recovery, PATs, and keys must be maintained |
| Billing | GitHub Apps do not consume seats | Free as a collaborator on this personal repository; a machine user can consume a seat as a member or outside collaborator on private organization repositories |

Sources: [App identity, permissions, lifecycle, and seats](https://docs.github.com/en/apps/creating-github-apps/about-creating-github-apps/deciding-when-to-build-a-github-app), [App versus OAuth/machine accounts](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/differences-between-github-apps-and-oauth-apps), [machine-user access](https://docs.github.com/en/authentication/connecting-to-github-with-ssh/managing-deploy-keys#machine-users), [personal-account collaborators](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/repository-access-and-collaboration), [organization license consumption](https://docs.github.com/en/enterprise-cloud@latest/billing/reference/github-license-users).

## What is actually attributed

### GitHub API and `gh` actions

The credential used for a mutation determines the actor. A GitHub App **installation** token attributes activity to the App. A GitHub App **user** token attributes it to the human user in conjunction with the App, so a user token does not solve this problem. A PAT attributes activity to the personal account that owns the PAT. [GitHub App token attribution](https://docs.github.com/en/apps/creating-github-apps/about-creating-github-apps/best-practices-for-creating-a-github-app#use-the-appropriate-token-type), [authentication modes](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/about-authentication-with-a-github-app)

The relevant endpoints accept App installation tokens. Creating an issue needs **Issues: write**; creating a pull request needs **Pull requests: write**; Git over HTTPS needs **Contents** permission. The stacked-pull-request API also accepts installation tokens and uses Pull requests permissions. [Create an issue](https://docs.github.com/en/rest/issues/issues#create-an-issue), [create a pull request](https://docs.github.com/en/rest/pulls/pulls#create-a-pull-request), [Git access for Apps](https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/choosing-permissions-for-a-github-app#choosing-permissions-for-git-access), [stacked pull request API](https://docs.github.com/en/rest/pulls/stacks)

### Git commits

Authentication used to push is separate from the author and committer stored in each Git commit. Changing the push token does not rewrite commit metadata, and changing Git configuration affects only future commits. GitHub associates command-line commits with an account using the commit email. [Commit email and attribution](https://docs.github.com/en/account-and-profile/how-tos/email-preferences/setting-your-commit-email-address), [email-address reference](https://docs.github.com/en/account-and-profile/concepts/email-addresses#commit-email-addresses)

For App-authored commits, GitHub's own `actions/create-github-app-token` project documents the supported identity:

```text
<app-slug>[bot]
<bot-user-id>+<app-slug>[bot]@users.noreply.github.com
```

The numeric bot user ID is obtained from `/users/<app-slug>[bot]`. For a machine user, use that account's verified address or its GitHub-provided noreply address. Configure both author and committer identity for new agent commits; do not rewrite already-merged commits merely to change attribution. [GitHub's App bot Git configuration](https://github.com/actions/create-github-app-token#configure-git-cli-for-an-apps-bot-user), [GitHub noreply addresses](https://docs.github.com/en/account-and-profile/reference/email-addresses-reference#your-noreply-email-address)

## Notifications and subscriptions

Moving authorship away from `skulldogged` also moves the automatic `author` subscription away from that account. GitHub automatically subscribes a user when they participate or are mentioned; review requests produce `review-requested` notifications; repository custom watching can subscribe a user to every issue and pull request. [Notification reasons](https://docs.github.com/en/subscriptions-and-notifications/reference/inbox-filters), [participating and custom watch settings](https://docs.github.com/en/subscriptions-and-notifications/get-started/configuring-notifications)

An App installation token cannot silently subscribe `skulldogged`: GitHub's notification and thread-subscription endpoints do not support App installation tokens and operate on the authenticated user's inbox. A machine user's subscription likewise belongs to the machine user's inbox, not `skulldogged`'s. [Notifications API authentication](https://docs.github.com/en/rest/activity/notifications)

The focused policy for this repository should be:

1. When the agent creates an issue, mention `@skulldogged` once in the issue body (or assign it when ownership is genuinely intended). The mention subscribes the user to that conversation.
2. Keep agent PRs draft while automated work is incomplete. Request `skulldogged` as reviewer once when the PR is ready for human review; do not request repeated Codex reviews.
3. Do not mention the user again on routine status comments. Once subscribed, GitHub delivers subsequent conversation updates according to the user's notification settings.
4. Use **Watch -> Custom -> Issues and Pull requests** only if the user wants every repository thread; GitHub documents that this subscribes the watcher to all selected event types, so it is intentionally noisier.

[Mention, review-request, and author notification reasons](https://docs.github.com/en/subscriptions-and-notifications/reference/email-notification-headers), [custom repository watching](https://docs.github.com/en/subscriptions-and-notifications/get-started/configuring-notifications#configuring-your-watch-settings-for-an-individual-repository)

## Recommended GitHub App permissions

Install the App on **only** `skulldogged/gpui-ghostty-agent-terminal` with:

- **Metadata: read**, GitHub's baseline repository metadata permission.
- **Contents: write** for branch creation, authenticated pushes, and content changes.
- **Issues: write** for issues, issue comments, labels, and issue state.
- **Pull requests: write** for PR creation and updates, reviews, review comments, and stack operations.
- **Workflows: write** only if the agent must add or modify `.github/workflows`; omit it otherwise.

GitHub Apps start with no permissions, and GitHub recommends selecting only the minimum needed. Installation tokens can be narrowed again at mint time to a subset of the installation's repositories and permissions. No administration, organization, account, webhooks, or user authorization is needed for this local mutation workflow. [Choosing App permissions](https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/choosing-permissions-for-a-github-app), [installation-token narrowing](https://docs.github.com/en/rest/apps/apps#create-an-installation-access-token-for-an-app)

Pull requests write permission technically permits merging. The local policy should still require explicit user authorization before a merge; authentication capability is not workflow authorization.

## Implementation-ready path for local/T3 Codex

### User-controlled GitHub setup

1. Register a private App under the `skulldogged` personal account with a project-specific name, disable webhooks, and choose the permissions above. A private App can only be installed on the account that owns it. [Registering a GitHub App](https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app)
2. Install it with **Only select repositories** and select only this repository. [Installing your own GitHub App](https://docs.github.com/en/apps/using-github-apps/installing-your-own-github-app)
3. Record the App client ID and installation ID, then generate one private key. GitHub stores only the public half; App private keys do not expire automatically and must be revoked or rotated manually. [Managing App private keys](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/managing-private-keys-for-github-apps)
4. Put the private key in a platform secret store or sign-only key vault. If this builder cannot use one, place it outside the repository in an owner-readable file as an interim measure and never copy it into Git, chat, shell history, process arguments, or logs. GitHub explicitly ranks an environment variable below a key vault because a compromised environment can read it. [GitHub credential-storage guidance](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/managing-private-keys-for-github-apps#storing-private-keys)

### Repository-side wrapper

After the user supplies those values, add a small wrapper outside the ordinary `gh` login path that:

1. Creates an RS256 App JWT valid for at most ten minutes.
2. Exchanges it for a one-hour installation token explicitly restricted to this repository and the requested operation's permissions.
3. Verifies the installation/App slug and repository before any mutation.
4. Runs `gh` with the token only in the child process and with an isolated GitHub CLI configuration, so a missing bot token fails closed instead of falling back to the owner's stored session.
5. Uses the installation token as the HTTP password for Git pushes, without embedding it in the remote URL or persisting it in `.git/config`.
6. Sets App bot author and committer identity only for agent-created commits.
7. Redacts token output, removes temporary material, and revokes the installation token at the end when practical.

GitHub specifies the JWT algorithm and ten-minute maximum, the JWT-to-installation-token exchange, the one-hour installation-token lifetime, optional repository/permission narrowing, and HTTP Git authentication. [Generating the App JWT](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-json-web-token-jwt-for-a-github-app), [authenticating as an installation](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/authenticating-as-a-github-app-installation), [Git access](https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/choosing-permissions-for-a-github-app#choosing-permissions-for-git-access)

Use the existing owner-authenticated GitHub connection for read-only inspection if convenient, but route every issue, PR, comment, review, stack mutation, and push that should carry the agent identity through this wrapper. OpenAI's documented Codex GitHub setup connects the user's GitHub account and exposes Codex review/cloud workflows; it does not document exporting those credentials or substituting an arbitrary local GitHub App credential for general local mutations. Therefore the wrapper is the implementation boundary unless T3/Codex later exposes first-class per-operation identity selection. This is an inference from the documented product surface. [OpenAI Codex GitHub integration](https://learn.chatgpt.com/docs/third-party/github), [OpenAI Codex cloud GitHub setup](https://learn.chatgpt.com/docs/cloud)

### Verification before regular use

Perform one disposable, explicitly approved test:

1. Mint a token and read back the expected App slug and selected repository.
2. Create a temporary branch and commit with the App bot author/committer identity.
3. Push it with the installation token.
4. Open a draft PR with the installation token and confirm the PR author is `<app-slug>[bot]` and the commit resolves to the same bot.
5. Mention `@skulldogged` once and confirm the human account receives the intended notification.
6. Close the draft PR, delete the temporary branch, and revoke the token.

Do not change the repository's normal remote or overwrite the owner's existing `gh` session during this test.

## Machine-user alternative

Choose this only if a normal profile/inbox or reuse across unrelated repository owners is a hard requirement.

1. A human must manually create the account. GitHub's Terms permit one free machine account in addition to a free personal account, but prohibit automated account registration; the human owner remains responsible for its actions. [GitHub Terms account requirements](https://docs.github.com/en/site-policy/github-terms/github-terms-of-service#3-account-requirements)
2. Give it a dedicated valid email address, enable 2FA, store recovery codes and multiple recovery methods securely, and retain clear human ownership. GitHub requires contributing accounts selected for mandatory 2FA—including unattended service accounts—to enroll, and GitHub Support cannot restore an account if all recovery methods are lost. [Mandatory 2FA](https://docs.github.com/en/authentication/securing-your-account-with-two-factor-authentication-2fa/about-mandatory-two-factor-authentication), [2FA recovery](https://docs.github.com/en/authentication/securing-your-account-with-two-factor-authentication-2fa/recovering-your-account-if-you-lose-your-2fa-credentials), [service-account 2FA guidance](https://docs.github.com/en/organizations/keeping-your-organization-secure/managing-two-factor-authentication-for-your-organization/managing-bots-and-service-accounts-with-two-factor-authentication)
3. Invite it as a collaborator on this personal repository. GitHub Free permits unlimited collaborators here, but personal-account repositories expose only owner and collaborator permission levels; an invited collaborator can push and manage issues and pull requests. [Personal-repository access](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/repository-access-and-collaboration), [collaborator permission level](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/repository-access-and-collaboration/permission-levels-for-a-personal-account-repository)
4. Use a dedicated SSH key for Git. For API/`gh`, use a classic PAT with `public_repo`, because this repository is public and GitHub currently says fine-grained PATs cannot contribute where their owner is an outside or repository collaborator. If the same account later needs a private repository, the classic token would need the broader `repo` scope. Classic scopes are not selectable-repository permissions, so the account itself should be invited to no repositories beyond those intentionally in scope. [Machine-user SSH setup](https://docs.github.com/en/authentication/connecting-to-github-with-ssh/managing-deploy-keys#machine-users), [fine-grained PAT limitation](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens#fine-grained-personal-access-token-limitations), [classic `public_repo` and `repo` scopes](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/scopes-for-oauth-apps#available-scopes)
5. Give each builder/environment its own revocable SSH key; keep the PAT out of the repository and inject it only into bot mutation commands. Set commit author/committer to the machine user's verified or noreply email and check `gh api user` before each mutation.
6. If repositories later move into an organization, reassess the design. A machine user added as a member or outside collaborator to private organization repositories can consume a paid seat, while a GitHub App does not. Organization policy can also restrict PATs or require approval. [Organization license users](https://docs.github.com/en/enterprise-cloud@latest/billing/reference/github-license-users), [organization PAT policies](https://docs.github.com/en/organizations/managing-programmatic-access-to-your-organization/setting-a-personal-access-token-policy-for-your-organization)

No App, account, credential, repository permission, notification setting, or other GitHub state was created or changed as part of this research.
