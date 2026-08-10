# Issues self-service project destination

Status: implementation design, reviewed against the running product on 2026-08-10.

## Product seam

A signed-in organization can create repositories immediately, but the browser currently needs
`MYELIN_ISSUES_PROJECT`, `MYELIN_ISSUES_TYPE`, and `MYELIN_ISSUES_PREFIX` before it can create an
issue. Those identifiers are operator plumbing. The durable project API already lets a human create
an authorized project whose issue prefix and default type are platform metadata.

The Issues workspace should therefore discover the viewer's projects, ask which visible project
owns the issue, and help a new organization create its first project without leaving the flow. The
browser sends only a selected authorized project ID; Edge remains the permission and metadata
authority.

## Primary flow

1. The user opens **New issue**.
2. While visible projects load, the dialog is busy and no mutation control is enabled.
3. With projects available, the title receives focus. A labelled project selector shows human names
   and issue prefixes; the selected prefix previews the future issue key.
4. **Create issue** sends the project ID and title with one mutation identity. Edge supplies the
   project's current issue type and prefix, so stale browser metadata cannot choose either.
5. The existing accepted/activating/ready sequence remains unchanged.

If the viewer has no project, the same dialog becomes a short first-project setup step. Project name
and a 2–10 character uppercase key are the only fields. A successful project creation moves to the
issue form and focuses its title; it does not silently create an issue the user has not reviewed.
An established organization can enter the same setup step with **New project** and return with
**Back**, so adding a project is not a first-run-only or CLI-only capability.

## Wireframes

Projects available:

```text
┌ New issue ─────────────────────────────────────────┐
│ Capture work in a project you can access.          │
│                                                    │
│ Project                                            │
│ [ Developer experience — DX                 ▾ ]    │
│ New issues will be numbered DX-…                   │
│                                                    │
│ Title                                              │
│ [ Make onboarding uneventful                  ]    │
│ Up to 512 UTF-8 bytes.                             │
│                                                    │
│                              [Cancel] [Create issue]│
└────────────────────────────────────────────────────┘
```

No projects:

```text
┌ Set up issue tracking ─────────────────────────────┐
│ Issues live in projects. Create your first one;    │
│ its key keeps issue references short.              │
│                                                    │
│ Project name     [ Developer experience       ]    │
│ Issue key        [ DX                         ]    │
│                  Issues will look like DX-1.       │
│                                                    │
│                          [Cancel] [Create project]  │
└────────────────────────────────────────────────────┘
```

## State and safety design

- **Loading:** retain the dialog frame, announce “Loading projects…”, disable the primary action.
- **More projects:** page through the authorized keyset list inside the dialog; do not imply that a
  partial first page is the complete organization.
- **Read failure:** say projects could not be loaded and offer retry. Do not fall back to deployment
  identifiers or guess a destination.
- **Project conflict:** explain that the issue key is already used and keep the fields editable.
- **Ambiguous project write:** say creation could not be confirmed and ask the user to reload/check
  before retrying. Never claim that nothing was written.
- **Issue write failure:** retain the existing check-the-list-before-retrying language.
- **Authorization:** only project rows returned by the owner-scoped Edge list are rendered. A
  browser-tampered UUID is still checked by Edge and maps to the leak-free unavailable state.
- **Accessibility:** every field has a persistent label and hint; errors use `role=alert`; the
  dialog's initial focus follows the active step; keyboard and narrow-screen behavior remain part
  of the browser contract.

## Contract changes

- Add strict browser decoders for `GET/POST /v1/projects` and bounded project pagination input.
- Change the browser issue-create mutation from `{title}` to `{title, projectId}`.
- Send `POST /v1/issues` as `{project_id, title}` with an `Idempotency-Key`; Edge resolves durable
  project defaults.
- Remove the three web deployment variables after real-browser coverage proves the self-service
  journey.
