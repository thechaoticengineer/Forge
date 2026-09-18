# Panel redesign: compact tabbed views

## Goal

Replace the panel's single long scrolling page with a compact, navigable panel: a fixed header, a row of tabs, and one focused view per tab that fits the window without scrolling in the common case. Details open on their own page on click instead of being expanded inline. Nothing the panel can do today is lost; it only moves.

## Problem

Today `quickshell/Panel.qml` (3,127 lines) renders everything on one page about 2,200 px tall, three screens at the default 760×760 window (see `design/before-top.png`):

- 10 settings buttons (planner, architect, routing, implementer, reviewer, two cadences, push, auto-approve), 4 lines of Claude/Fable quota and catalogue metadata sit above the goal.
- 13 goal/plan actions in three rows, most of them disabled in any given phase.
- The architecture card, role token totals and plan review status push the stages down.
- Every stage shows 8–9 lines of review policy and model routing, even after it is committed.
- Live / History / Reports sit at the very bottom, squeezed into what is left.

## Scope

- **Header (always visible):** `FORGE`, a project switcher (current project name and a ▾ menu listing all sessions with their status markers, plus "Change project…"), the phase badge, the current step text, and a `⋯` overflow menu.
- **Tab bar** under the header: Overview · Plan · Activity · Architecture · Features · Queue (with count) · Settings. The selected tab is underlined in the accent color.
- **Overview:** the goal, the phase's primary actions, and a status summary.
  - While busy: a "now working" card (stage, role, tool, model, elapsed time, latest output line), a one-line progress summary, compact stage rows and Stop.
  - While idle: the goal field with Create plan / Discuss first / Enhance with AI / Add to queue, plus a short card for the last run.
  - Plan ready or awaiting approval: Approve / Start / Edit / Discard as appropriate.
  - Mockups: `design/views.overview-running.png`, `design/views.overview-idle.png`.
- **Plan:** one line per stage (status icon, number and title, commit or status, duration, `›`), a one-line plan review strip that opens the plan review details, plan editing, plan Q&A and the "what should be improved" feedback field. Clicking a stage opens the **stage detail page**: breadcrumb back to Plan, previous/next stage, a status line, and sub-tabs Instructions / Acceptance / Review / Routing / Output that hold what is expanded inline today. Mockups: `design/views.plan.png`, `design/views.stage-detail.png`.
- **Activity:** the existing Live / History / Reports output with its filters, using the full height of the view.
- **Architecture:** the existing architecture card (context, guidance, risks, decisions), role token totals, and plan review status and history.
- **Features:** the existing feature specs list, as a tab instead of a pushed page.
- **Queue:** the existing queue list with Start queue, reorder and remove.
- **Settings:** agent tools, automatic routing, review cadences, push at end and auto-approve, shown as label/value rows that cycle on click. Also quota and catalogue metadata (including errors), Model settings & options, Refresh models, Refresh Claude limits, Update Forge and Change project. Mockup: `design/views.settings.png`.
- **`⋯` overflow menu:** rarely used or destructive actions that do not belong to one view: Update Forge, Discard plan, Refactor plan, View diff, Change project, Keyboard help.
- **Code structure:** each view lives in its own QML file. `Panel.qml` keeps engine state, polling, API actions and navigation, and shrinks accordingly.

## Out of scope

- Engine and API changes. The panel keeps using the same endpoints and state fields.
- New features or removed features. Every action, indicator and keyboard shortcut stays available.
- Theming changes. Colors, font and the `PanelButton` look stay as they are.
- The M5 feature viewer from `docs/features/feature-specs/`.
- Animations. Page and tab switches stay instant.

## Behavior

- The header and tab bar never scroll. Each view scrolls on its own only when its content exceeds the window.
- The selected tab and each view's reading position survive polling refreshes. Switching project keeps the selected tab.
- The panel opens on Overview the first time. Reopening restores the last tab.
- Pushed pages (Discussion, Model settings, Diff, Project chooser, Keyboard help, stage detail) sit on top of the current tab. Escape / q / Back return to the same tab with its selection and scroll position intact.
- An action button is shown in a view only when its action is possible in the current phase, or it is available through `⋯`. Views are not filled with disabled buttons.
- When a stage needs attention (blocked or failed), the Overview shows it at the top with a link to its stage detail.
- Keyboard:
  - All current normal-mode shortcuts keep their meaning.
  - New: `[` / `]` switch to the previous / next tab on tab views. While a stage detail page is open they move to the previous / next stage instead.
  - New: `g` followed by `o`, `p`, `a`, `r`, `f`, `q`, `s` jumps to Overview, Plan, Activity, Architecture, Features, Queue, Settings. `gg` keeps its current meaning.
  - `j`/`k`, `Enter`, `h`/`l`, `Tab` and the digit filters act on the view that shows the thing they control. For example, digits filter History in Activity.
  - The bottom hint line names the current view's keys, and keyboard help lists the new keys.

## Open questions

- None at spec time. The layout follows the mockups in `design/`. Details the mockups do not show are left to the implementer, within the behavior above.
